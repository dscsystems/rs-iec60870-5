// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Shared harness for the go-iecp5 interoperability tests.
//!
//! Builds the Go helper peers on demand and collects the JSON event lines they
//! write to stdout, so an assertion can name the exact information object that
//! disagreed.

#![allow(dead_code)] // each test binary uses a different subset

pub mod relay;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// How long an interop assertion waits before giving up.
pub const WAIT: Duration = Duration::from_secs(20);

fn interop_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/interop")
}

/// Build the Go helper binaries once per test process.
///
/// Returns `None` when the Go toolchain or the go-iecp5 checkout is missing,
/// which makes the caller skip rather than fail.
pub fn go_bins() -> Option<&'static PathBuf> {
    static BINS: OnceLock<Option<PathBuf>> = OnceLock::new();
    BINS.get_or_init(|| {
        let dir = interop_dir();
        if !dir.join("go-iecp5/go.mod").exists() {
            eprintln!(
                "SKIP: go-iecp5 checkout missing at {}; see tests/interop/README.md",
                dir.join("go-iecp5").display()
            );
            return None;
        }
        let out = std::process::Command::new("go")
            .current_dir(dir.join("go"))
            .args(["build", "-o", "bin/", "./..."])
            .output();

        match out {
            Err(e) => {
                eprintln!("SKIP: the Go toolchain is unavailable ({e})");
                None
            }
            Ok(o) if !o.status.success() => {
                eprintln!(
                    "SKIP: building the Go peers failed:\n{}",
                    String::from_utf8_lossy(&o.stderr)
                );
                None
            }
            Ok(_) => Some(dir.join("go/bin")),
        }
    })
    .as_ref()
}

/// A running Go peer whose JSON event stream is collected in the background.
pub struct GoPeer {
    child: Child,
    events: Arc<Mutex<Vec<Value>>>,
    name: String,
}

impl GoPeer {
    /// Start `bin` with `args`, capturing its event stream.
    pub async fn spawn(bin: &Path, args: &[&str]) -> GoPeer {
        let name = bin
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut child = Command::new(bin)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|e| panic!("failed to start {}: {e}", bin.display()));

        let events = Arc::new(Mutex::new(Vec::new()));
        let stdout = child.stdout.take().expect("piped stdout");
        {
            let events = Arc::clone(&events);
            let name = name.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    match serde_json::from_str::<Value>(&line) {
                        Ok(v) => events.lock().await.push(v),
                        // go-iecp5's own logger writes plain text; pass it through.
                        Err(_) => eprintln!("{name}: {line}"),
                    }
                }
            });
        }
        let stderr = child.stderr.take().expect("piped stderr");
        {
            let name = name.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    eprintln!("{name}[stderr]: {line}");
                }
            });
        }

        GoPeer {
            child,
            events,
            name,
        }
    }

    /// Wait for an event matching `pred` and return it.
    pub async fn wait(&self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            if let Some(v) = self.events.lock().await.iter().find(|v| pred(v)).cloned() {
                return v;
            }
            if tokio::time::Instant::now() >= deadline {
                let seen = self.events.lock().await.clone();
                panic!(
                    "timed out waiting for {what}; {} reported: {seen:#?}",
                    self.name
                );
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Wait for the `ready` event and return the address the peer bound.
    pub async fn wait_ready(&self) -> String {
        let v = self.wait("the Go peer to bind", |v| v["event"] == "ready").await;
        v["addr"].as_str().expect("addr").to_string()
    }

    /// Every event of a given `"event"` kind seen so far.
    pub async fn all_of(&self, kind: &str) -> Vec<Value> {
        self.events
            .lock()
            .await
            .iter()
            .filter(|v| v["event"] == kind)
            .cloned()
            .collect()
    }

    /// Stop the peer.
    pub async fn kill(mut self) {
        let _ = self.child.kill().await;
    }
}

/// A predicate matching an event of `kind` whose `field` equals `value`.
pub fn field_is(
    kind: &'static str,
    field: &'static str,
    value: Value,
) -> impl Fn(&Value) -> bool {
    move |v: &Value| v["event"] == kind && v[field] == value
}

/// Poll until `f` holds, or fail the test.
pub async fn eventually(label: &str, mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while tokio::time::Instant::now() < deadline {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for: {label}");
}
