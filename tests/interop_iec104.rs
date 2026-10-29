// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Interoperability tests against the reference stacks of
//! `github.com/otfabric/iec104-interop`: lib60870-C, OpenMUC j60870,
//! wendy512/go-iecp5 and cB-Sigma/iec60870, packaged as Docker images.
//!
//! * `rust_master_with_<stack>_station`: this crate's [`Client`] against the
//!   reference controlled station serving the `baseline` fixture.
//! * `<stack>_master_with_rust_station`: this crate's [`Server`] serving the
//!   same fixture, driven by the reference client through the cases of
//!   `tests/interop/iec104/cases.tsv`.
//!
//! Needs Docker. Without it, or without the images, the tests print `SKIP:`
//! and pass; `IEC60870_INTEROP_REQUIRED=1` makes that a failure. The images
//! are `$IEC104_INTEROP_REGISTRY/iec104-interop-<stack>:$IEC104_INTEROP_VERSION`
//! (default `ghcr.io/otfabric`, `v0.3.1`) and are pulled when missing.
#![cfg(all(feature = "cs104", feature = "filetransfer"))]

mod common;
use common::WAIT;
use rs_iec60870_5::{asdu::*, cs104::*, filetransfer::*};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    process::Stdio,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::{net::TcpListener, process::Command, sync::Mutex};

// ---------------------------------------------------------------------------
// Docker harness
// ---------------------------------------------------------------------------

fn required() -> bool {
    std::env::var_os("IEC60870_INTEROP_REQUIRED").is_some()
}

fn skip_or_fail(why: String) -> bool {
    assert!(!required(), "{why}");
    eprintln!("SKIP: {why}");
    false
}

fn image(stack: &str) -> String {
    let registry =
        std::env::var("IEC104_INTEROP_REGISTRY").unwrap_or_else(|_| "ghcr.io/otfabric".into());
    let version = std::env::var("IEC104_INTEROP_VERSION").unwrap_or_else(|_| "v0.3.1".into());
    format!("{registry}/iec104-interop-{stack}:{version}")
}

/// Whether the image can be used; pulls it once when it is not local.
fn have_image(stack: &str) -> bool {
    static CACHE: OnceLock<std::sync::Mutex<BTreeMap<String, bool>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    let image = image(stack);
    *cache.entry(image.clone()).or_insert_with(|| {
        let docker = |args: &[&str]| {
            std::process::Command::new("docker")
                .args(args)
                .output()
                .map(|o| o.status.success())
        };
        match docker(&["image", "inspect", &image]) {
            Err(e) => return skip_or_fail(format!("docker is unavailable ({e})")),
            Ok(true) => return true,
            Ok(false) => {}
        }
        docker(&["pull", &image]).unwrap_or(false) || skip_or_fail(format!("cannot pull {image}"))
    })
}

fn capabilities(stack: &str) -> Value {
    let out = std::process::Command::new("docker")
        .args(["run", "--rm", &image(stack), "print-capabilities"])
        .output()
        .expect("docker");
    assert!(out.status.success(), "print-capabilities of {stack} failed");
    serde_json::from_slice(&out.stdout).expect("capabilities JSON")
}

fn feature(caps: &Value, name: &str) -> bool {
    caps["features"][name] == true
}

/// A reference controlled station in a container, removed on drop.
struct RefStation {
    name: String,
    addr: String,
}

impl RefStation {
    async fn start(stack: &str) -> RefStation {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let name = format!(
            "rs60870-{stack}-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let run = Command::new("docker")
            .args(["run", "-d", "--name", &name, "-p", "127.0.0.1::2404"])
            .arg(image(stack))
            .arg("server")
            .output()
            .await
            .expect("docker");
        assert!(
            run.status.success(),
            "starting {stack}: {}",
            String::from_utf8_lossy(&run.stderr)
        );
        let station = RefStation {
            name,
            addr: String::new(),
        };
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let ready = Command::new("docker")
                .args([
                    "exec",
                    &station.name,
                    "test",
                    "-f",
                    "/run/iec104-interop/ready",
                ])
                .output()
                .await
                .unwrap();
            if ready.status.success() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{stack} did not become ready"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let port = Command::new("docker")
            .args(["port", &station.name, "2404/tcp"])
            .output()
            .await
            .unwrap();
        let port = String::from_utf8_lossy(&port.stdout);
        let port = port.lines().next().expect("published port").trim();
        let mut station = station;
        station.addr = port.replace("0.0.0.0", "127.0.0.1");
        station
    }
}

impl Drop for RefStation {
    fn drop(&mut self) {
        if std::env::var_os("IEC104_INTEROP_KEEP").is_some() {
            return;
        }
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.name])
            .output();
    }
}

/// Runs one operation of the reference client against `port` on the host.
/// Returns the exit code and the result document.
async fn ref_client(stack: &str, port: u16, args: &[&str]) -> (i32, Value) {
    let out = tokio::time::timeout(
        Duration::from_secs(60),
        Command::new("docker")
            .args([
                "run",
                "--rm",
                "--add-host=host.docker.internal:host-gateway",
            ])
            .arg(image(stack))
            .arg("client")
            .arg(args[0])
            .args([
                "--host",
                "host.docker.internal",
                "--port",
                &port.to_string(),
            ])
            .args(&args[1..])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("reference client timed out")
    .expect("docker");
    let doc = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{stack} {args:?}: no result document ({e}); stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        )
    });
    (out.status.code().unwrap_or(-1), doc)
}

/// `ok=true error=- conf=7,9n term=true asdus=type/cot,...` as in cases.tsv.
fn summary(doc: &Value) -> String {
    let conf: Vec<String> = doc["confirmations"]
        .as_array()
        .map(|c| {
            c.iter()
                .map(|c| {
                    format!(
                        "{}{}",
                        c["cot"],
                        if c["negative"] == true { "n" } else { "" }
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let asdus: Vec<String> = doc["asdus"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|a| format!("{}/{}", a["type"].as_str().unwrap_or("?"), a["cot"]))
                .collect()
        })
        .unwrap_or_default();
    format!(
        "ok={} error={} conf={} term={} asdus={}",
        doc["ok"],
        doc["error"]["code"].as_str().unwrap_or("-"),
        conf.join(","),
        doc["terminated"],
        asdus.join(",")
    )
}

// ---------------------------------------------------------------------------
// Direction 1: this crate's master against a reference controlled station
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Log(Mutex<Vec<Asdu>>);

impl Log {
    async fn wait_for(&self, what: &str, pred: impl Fn(&Asdu) -> bool) -> Asdu {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            if let Some(a) = self.0.lock().await.iter().find(|a| pred(a)).cloned() {
                return a;
            }
            if tokio::time::Instant::now() >= deadline {
                let seen: Vec<String> = self.0.lock().await.iter().map(|a| a.to_string()).collect();
                panic!("timed out waiting for {what}; received: {seen:#?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
    async fn count(&self, pred: impl Fn(&Asdu) -> bool) -> usize {
        self.0.lock().await.iter().filter(|a| pred(a)).count()
    }
}

struct Master {
    log: Arc<Log>,
    receiver: Mutex<Receiver>,
}

/// Lets the test keep the handler while the client owns it too.
struct Shared(Arc<Master>);

#[async_trait::async_trait]
impl ClientHandler for Shared {
    async fn asdu_all(
        &self,
        c: &dyn Connect,
        a: &Asdu,
        ctx: &ClientContext,
    ) -> rs_iec60870_5::Result<()> {
        self.0.asdu_all(c, a, ctx).await
    }
    async fn asdu(&self, c: &dyn Connect, a: &Asdu) -> rs_iec60870_5::Result<()> {
        self.0.asdu(c, a).await
    }
}

#[async_trait::async_trait]
impl ClientHandler for Master {
    async fn asdu_all(
        &self,
        _: &dyn Connect,
        a: &Asdu,
        _: &ClientContext,
    ) -> rs_iec60870_5::Result<()> {
        self.log.0.lock().await.push(a.clone());
        Ok(())
    }
    async fn asdu(&self, c: &dyn Connect, a: &Asdu) -> rs_iec60870_5::Result<()> {
        self.receiver.lock().await.handle(c, a).await?;
        Ok(())
    }
}

/// The octets of a fixture file: `(i + ioa) mod 251`.
fn fixture_file(ioa: u32, size: usize) -> Vec<u8> {
    (0..size).map(|i| ((i as u32 + ioa) % 251) as u8).collect()
}

fn act() -> CauseOfTransmission {
    CauseOfTransmission::new(Cause::ACTIVATION)
}

fn is_cause(t: TypeId, cause: Cause) -> impl Fn(&Asdu) -> bool {
    move |a| a.type_id() == t && a.coa().cause == cause
}

async fn rust_master_against(stack: &str) {
    if !have_image(stack) {
        return;
    }
    let caps = capabilities(stack);
    if caps["roles"]["server"] != true {
        return;
    }
    let station = RefStation::start(stack).await;
    let log = Arc::new(Log::default());
    let files = Arc::new(std::sync::Mutex::new(Vec::<Vec<u8>>::new()));
    let receiver = Receiver::without_store();
    let sink = files.clone();
    receiver.set_file_handler(Box::new(move |_, data| sink.lock().unwrap().push(data)));
    let master = Arc::new(Master {
        log: log.clone(),
        receiver: Mutex::new(receiver),
    });
    let client = Client::new(
        Shared(master.clone()),
        ClientOption::new()
            .with_server(&station.addr)
            .unwrap()
            .with_auto_reconnect(false),
    );
    client.start().unwrap();
    tokio::time::timeout(WAIT, client.wait_active())
        .await
        .unwrap_or_else(|_| panic!("no STARTDT confirmation from {stack}"));

    // -- general interrogation: every point of the baseline fixture --------
    client
        .send_interrogation_cmd(act(), 1, QualifierOfInterrogation::STATION)
        .await
        .unwrap();
    log.wait_for(
        "GI termination",
        is_cause(TypeId::C_IC_NA_1, Cause::ACTIVATION_TERM),
    )
    .await;
    assert_eq!(
        log.count(is_cause(TypeId::C_IC_NA_1, Cause::ACTIVATION_CON))
            .await,
        1
    );
    let mut points = Vec::new();
    for a in log.0.lock().await.iter() {
        assert_eq!(a.common_addr(), 1);
        let cause = a.coa().cause;
        if cause != Cause::INTERROGATED_BY_STATION {
            continue;
        }
        match a.type_id() {
            TypeId::M_SP_NA_1 => points.extend(
                a.get_single_point()
                    .unwrap()
                    .iter()
                    .map(|p| (p.ioa, format!("sp {} {:#04x}", p.value, p.qds.0))),
            ),
            TypeId::M_DP_NA_1 => points.extend(
                a.get_double_point()
                    .unwrap()
                    .iter()
                    .map(|p| (p.ioa, format!("dp {}", p.value as u8))),
            ),
            TypeId::M_ST_NA_1 => points.extend(
                a.get_step_position()
                    .unwrap()
                    .iter()
                    .map(|p| (p.ioa, format!("st {}", p.value.val))),
            ),
            TypeId::M_BO_NA_1 => points.extend(
                a.get_bitstring32()
                    .unwrap()
                    .iter()
                    .map(|p| (p.ioa, format!("bo {:#x}", p.value))),
            ),
            TypeId::M_ME_NA_1 => points.extend(
                a.get_measured_value_normal()
                    .unwrap()
                    .iter()
                    .map(|p| (p.ioa, format!("na {}", p.value.0))),
            ),
            TypeId::M_ME_NB_1 => points.extend(
                a.get_measured_value_scaled()
                    .unwrap()
                    .iter()
                    .map(|p| (p.ioa, format!("nb {} {:#04x}", p.value, p.qds.0))),
            ),
            TypeId::M_ME_NC_1 => points.extend(
                a.get_measured_value_float()
                    .unwrap()
                    .iter()
                    .map(|p| (p.ioa, format!("nc {}", p.value))),
            ),
            t => panic!("{stack} sent {t:?} in a general interrogation"),
        }
    }
    let expected = [
        (100, "sp true 0x00"),
        (101, "dp 2"),
        (102, "st 5"),
        (103, "bo 0xaaaaaaaa"),
        (110, "sp false 0xc0"),
        (111, "sp true 0x30"),
        (200, "na 16384"),
        (201, "nb 1234 0x00"),
        (202, "nc 230.5"),
        (210, "nb -32768 0x01"),
        (211, "nb -1 0x00"),
    ];
    // Quality is compared where the stack can carry it (see CAPABILITIES.md).
    let expected: Vec<(u32, String)> = expected.iter().map(|(i, v)| (*i, v.to_string())).collect();
    points.sort();
    assert_eq!(
        points, expected,
        "{stack}: points of the general interrogation"
    );

    // -- counter interrogation and read -------------------------------------
    client
        .send_counter_interrogation_cmd(
            act(),
            1,
            QualifierCountCall {
                request: QccRequest::TOTAL,
                freeze: QccFreeze::READ,
            },
        )
        .await
        .unwrap();
    let it = log
        .wait_for(
            "integrated totals",
            is_cause(TypeId::M_IT_NA_1, Cause::REQUEST_BY_GENERAL_COUNTER),
        )
        .await;
    let it = it.get_integrated_totals().unwrap()[0];
    assert_eq!(
        (it.ioa, it.value.counter_reading, it.value.seq_number),
        (300, 123456, 1)
    );
    log.wait_for(
        "CI termination",
        is_cause(TypeId::C_CI_NA_1, Cause::ACTIVATION_TERM),
    )
    .await;

    // j60870 1.8.0 cannot decode C_RD_NA_1 and drops the connection (a known
    // upstream bug, see iec104-interop docs/COMPATIBILITY.md).
    let read_works = !(stack == "openmuc" && caps["upstream"]["version"] == "1.8.0");
    if read_works {
        client
            .send_read_cmd(CauseOfTransmission::new(Cause::REQUEST), 1, 202)
            .await
            .unwrap();
        let read = log
            .wait_for("read answer", is_cause(TypeId::M_ME_NC_1, Cause::REQUEST))
            .await;
        assert_eq!(read.get_measured_value_float().unwrap()[0].value, 230.5);

        client
            .send_read_cmd(CauseOfTransmission::new(Cause::REQUEST), 1, 999)
            .await
            .unwrap();
        let refused = log
            .wait_for(
                "refusal of an unknown address",
                is_cause(TypeId::C_RD_NA_1, Cause::UNKNOWN_IOA),
            )
            .await;
        assert!(refused.coa().is_negative);
    }

    // -- system commands ----------------------------------------------------
    client
        .send_clock_synchronization_cmd(act(), 1, chrono::Utc::now())
        .await
        .unwrap();
    log.wait_for(
        "clock sync confirmation",
        is_cause(TypeId::C_CS_NA_1, Cause::ACTIVATION_CON),
    )
    .await;

    // -- commands: direct, then select before operate ------------------------
    client
        .send_single_cmd(
            TypeId::C_SC_NA_1,
            act(),
            1,
            SingleCommandInfo {
                ioa: 500,
                value: false,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let report = log
        .wait_for(
            "report of the command",
            is_cause(TypeId::M_SP_TB_1, Cause::RETURN_INFO_REMOTE),
        )
        .await;
    let report = &report.get_single_point().unwrap()[0];
    assert_eq!((report.ioa, report.value), (100, false));
    assert!(report.time.is_some(), "the report carries a CP56Time2a");
    log.wait_for(
        "command termination",
        is_cause(TypeId::C_SC_NA_1, Cause::ACTIVATION_TERM),
    )
    .await;

    // 510 only executes after a select.
    let single = |select: bool, value: bool| SingleCommandInfo {
        ioa: 510,
        value,
        qoc: QualifierOfCommand {
            in_select: select,
            ..Default::default()
        },
        ..Default::default()
    };
    client
        .send_single_cmd(TypeId::C_SC_NA_1, act(), 1, single(false, true))
        .await
        .unwrap();
    let refused = log
        .wait_for("refusal of an execute without select", |a| {
            a.type_id() == TypeId::C_SC_NA_1
                && a.coa().is_negative
                && a.get_single_cmd().is_ok_and(|c| c.ioa == 510)
        })
        .await;
    assert_eq!(refused.coa().cause, Cause::ACTIVATION_CON);
    let before = log
        .count(is_cause(TypeId::C_SC_NA_1, Cause::ACTIVATION_CON))
        .await;
    client
        .send_single_cmd(TypeId::C_SC_NA_1, act(), 1, single(true, true))
        .await
        .unwrap();
    common::eventually("select confirmation", async || {
        log.count(is_cause(TypeId::C_SC_NA_1, Cause::ACTIVATION_CON))
            .await
            > before
    })
    .await;
    client
        .send_single_cmd(TypeId::C_SC_NA_1, act(), 1, single(false, true))
        .await
        .unwrap();
    common::eventually("execute after select", async || {
        log.count(is_cause(TypeId::C_SC_NA_1, Cause::ACTIVATION_TERM))
            .await
            == 2
    })
    .await;

    // The other command types change what a later read reports.
    client
        .send_setpoint_cmd_float(
            TypeId::C_SE_NC_1,
            act(),
            1,
            SetpointCommandFloatInfo {
                ioa: 502,
                value: 49.5,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let report = log
        .wait_for(
            "float report",
            is_cause(TypeId::M_ME_TF_1, Cause::RETURN_INFO_REMOTE),
        )
        .await;
    let report = report.get_measured_value_float().unwrap()[0];
    assert_eq!((report.ioa, report.value), (202, 49.5));

    // A double command with a state that is not permitted is refused.
    client
        .send_double_cmd(
            TypeId::C_DC_NA_1,
            act(),
            1,
            DoubleCommandInfo {
                ioa: 501,
                value: DoubleCommand::NotAllowed0,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let refused = log
        .wait_for("refusal of double command state 0", |a| {
            a.type_id() == TypeId::C_DC_NA_1 && a.coa().is_negative
        })
        .await;
    assert_eq!(refused.coa().cause, Cause::ACTIVATION_CON);

    // -- file transfer ---------------------------------------------------------
    if feature(&caps, "fileServer") {
        for (ioa, size) in [(30000u32, 1000usize), (30001, 5000)] {
            master
                .receiver
                .lock()
                .await
                .request_file(client.as_ref(), 1, ioa, NameOfFile(1))
                .await
                .unwrap();
            let deadline = tokio::time::Instant::now() + WAIT;
            while files.lock().unwrap().is_empty() {
                if tokio::time::Instant::now() >= deadline {
                    let seen: Vec<String> = log
                        .0
                        .lock()
                        .await
                        .iter()
                        .rev()
                        .take(12)
                        .map(|a| a.to_string())
                        .collect();
                    panic!(
                        "{stack}: file {ioa} not received; traffic since the commands: {seen:#?}"
                    );
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            let data = files.lock().unwrap().remove(0);
            assert_eq!(data, fixture_file(ioa, size), "{stack} file {ioa}");
        }
    }
    client.close();
}

#[tokio::test]
async fn rust_master_with_lib60870_station() {
    rust_master_against("lib60870").await
}
#[tokio::test]
async fn rust_master_with_openmuc_station() {
    rust_master_against("openmuc").await
}
#[tokio::test]
async fn rust_master_with_wendy512_station() {
    rust_master_against("wendy512").await
}
#[tokio::test]
async fn rust_master_with_cbsigma_station() {
    rust_master_against("cbsigma").await
}

// ---------------------------------------------------------------------------
// Direction 2: this crate's controlled station against a reference master
// ---------------------------------------------------------------------------

/// The common address the fixture serves, and the broadcast address.
fn for_station(pack: &Asdu) -> bool {
    matches!(pack.common_addr(), 1 | 65535)
}

/// The `baseline` fixture, served the way docs/FIXTURES.md defines.
struct Baseline {
    state: std::sync::Mutex<Points>,
    sender: Sender,
}

struct Points {
    sp100: bool,
    dp101: u8,
    st102: i8,
    na200: i16,
    nb201: i16,
    nc202: f32,
    selected: std::collections::BTreeSet<u32>,
}

impl Baseline {
    fn new() -> Self {
        let store = Arc::new(MemStore::new());
        store.insert(30000, NameOfFile(1), fixture_file(30000, 1000));
        store.insert(30001, NameOfFile(1), fixture_file(30001, 5000));
        let sender = Sender::new(store);
        sender.set_section_size(2048);
        Baseline {
            state: std::sync::Mutex::new(Points {
                sp100: true,
                dp101: 2,
                st102: 5,
                na200: 16384,
                nb201: 1234,
                nc202: 230.5,
                selected: Default::default(),
            }),
            sender,
        }
    }
}

fn dp(v: u8) -> DoublePoint {
    match v {
        0 => DoublePoint::IndeterminateOrIntermediate,
        1 => DoublePoint::DeterminedOff,
        2 => DoublePoint::DeterminedOn,
        _ => DoublePoint::Indeterminate,
    }
}

/// A process command reduced to what the fixture needs.
enum Cmd {
    Single(bool),
    Double(u8),
    Step(u8),
    Normal(i16),
    Scaled(i16),
    Float(f32),
}

/// `(object, command type, target, report cause, select required)`
const COMMANDS: [(u32, TypeId, u32, Cause, bool); 7] = [
    (
        500,
        TypeId::C_SC_NA_1,
        100,
        Cause::RETURN_INFO_REMOTE,
        false,
    ),
    (501, TypeId::C_DC_NA_1, 101, Cause::SPONTANEOUS, false),
    (
        502,
        TypeId::C_SE_NC_1,
        202,
        Cause::RETURN_INFO_REMOTE,
        false,
    ),
    (
        503,
        TypeId::C_SE_NA_1,
        200,
        Cause::RETURN_INFO_REMOTE,
        false,
    ),
    (
        504,
        TypeId::C_SE_NB_1,
        201,
        Cause::RETURN_INFO_REMOTE,
        false,
    ),
    (
        505,
        TypeId::C_RC_NA_1,
        102,
        Cause::RETURN_INFO_REMOTE,
        false,
    ),
    (510, TypeId::C_SC_NA_1, 100, Cause::RETURN_INFO_REMOTE, true),
];

#[async_trait::async_trait]
impl ServerHandler for Baseline {
    async fn interrogation(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        qoi: QualifierOfInterrogation,
    ) -> rs_iec60870_5::Result<()> {
        if !for_station(pack) {
            return c.send(pack.reply_mirror(Cause::UNKNOWN_CA).negated()).await;
        }
        if qoi != QualifierOfInterrogation::STATION {
            return c
                .send(pack.reply_mirror(Cause::ACTIVATION_CON).negated())
                .await;
        }
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
        let (p, coa) = {
            let p = self.state.lock().unwrap();
            (
                (p.sp100, p.dp101, p.st102, p.na200, p.nb201, p.nc202),
                CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION),
            )
        };
        let ca = 1;
        let sp = |ioa, value, qds| SinglePointInfo {
            ioa,
            value,
            qds,
            ..Default::default()
        };
        c.send_single(false, coa, ca, &[sp(100, p.0, QualityDescriptor::GOOD)])
            .await?;
        c.send_double(
            false,
            coa,
            ca,
            &[DoublePointInfo {
                ioa: 101,
                value: dp(p.1),
                ..Default::default()
            }],
        )
        .await?;
        c.send_step(
            false,
            coa,
            ca,
            &[StepPositionInfo {
                ioa: 102,
                value: StepPosition {
                    val: p.2,
                    has_transient: false,
                },
                ..Default::default()
            }],
        )
        .await?;
        c.send_bitstring32(
            false,
            coa,
            ca,
            &[BitString32Info {
                ioa: 103,
                value: 0xAAAA_AAAA,
                ..Default::default()
            }],
        )
        .await?;
        c.send_single(
            false,
            coa,
            ca,
            &[
                sp(
                    110,
                    false,
                    QualityDescriptor::NOT_TOPICAL | QualityDescriptor::INVALID,
                ),
                sp(
                    111,
                    true,
                    QualityDescriptor::BLOCKED | QualityDescriptor::SUBSTITUTED,
                ),
            ],
        )
        .await?;
        c.send_measured_value_normal(
            false,
            coa,
            ca,
            &[MeasuredValueNormalInfo {
                ioa: 200,
                value: Normalize(p.3),
                ..Default::default()
            }],
        )
        .await?;
        let nb = |ioa, value, qds| MeasuredValueScaledInfo {
            ioa,
            value,
            qds,
            ..Default::default()
        };
        c.send_measured_value_scaled(false, coa, ca, &[nb(201, p.4, QualityDescriptor::GOOD)])
            .await?;
        c.send_measured_value_float(
            false,
            coa,
            ca,
            &[MeasuredValueFloatInfo {
                ioa: 202,
                value: p.5,
                ..Default::default()
            }],
        )
        .await?;
        c.send_measured_value_scaled(
            false,
            coa,
            ca,
            &[
                nb(210, i16::MIN, QualityDescriptor::OVERFLOW),
                nb(211, -1, QualityDescriptor::GOOD),
            ],
        )
        .await?;
        c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    }

    async fn counter_interrogation(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        qcc: QualifierCountCall,
    ) -> rs_iec60870_5::Result<()> {
        if !for_station(pack) {
            return c.send(pack.reply_mirror(Cause::UNKNOWN_CA).negated()).await;
        }
        if qcc.request != QccRequest::TOTAL || qcc.freeze != QccFreeze::READ {
            return c
                .send(pack.reply_mirror(Cause::ACTIVATION_CON).negated())
                .await;
        }
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
        c.send_integrated_totals(
            false,
            CauseOfTransmission::new(Cause::REQUEST_BY_GENERAL_COUNTER),
            1,
            &[BinaryCounterReadingInfo {
                ioa: 300,
                value: BinaryCounterReading {
                    counter_reading: 123456,
                    seq_number: 1,
                    ..Default::default()
                },
                time: None,
                time_flags: TimeTagFlags::GOOD,
            }],
        )
        .await?;
        c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    }

    async fn read(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        ioa: InfoObjAddr,
    ) -> rs_iec60870_5::Result<()> {
        if !for_station(pack) {
            return c.send(pack.reply_mirror(Cause::UNKNOWN_CA).negated()).await;
        }
        let coa = CauseOfTransmission::new(Cause::REQUEST);
        let p = {
            let p = self.state.lock().unwrap();
            (p.sp100, p.dp101, p.st102, p.na200, p.nb201, p.nc202)
        };
        let ca = 1;
        match ioa {
            100 => {
                c.send_single(false, coa, ca, &[SinglePointInfo::new(100, p.0)])
                    .await
            }
            101 => {
                let info = DoublePointInfo {
                    ioa,
                    value: dp(p.1),
                    ..Default::default()
                };
                c.send_double(false, coa, ca, &[info]).await
            }
            102 => {
                let value = StepPosition {
                    val: p.2,
                    has_transient: false,
                };
                let info = StepPositionInfo {
                    ioa,
                    value,
                    ..Default::default()
                };
                c.send_step(false, coa, ca, &[info]).await
            }
            103 => {
                let info = BitString32Info {
                    ioa,
                    value: 0xAAAA_AAAA,
                    ..Default::default()
                };
                c.send_bitstring32(false, coa, ca, &[info]).await
            }
            110 | 111 => {
                let (value, qds) = if ioa == 110 {
                    (
                        false,
                        QualityDescriptor::NOT_TOPICAL | QualityDescriptor::INVALID,
                    )
                } else {
                    (
                        true,
                        QualityDescriptor::BLOCKED | QualityDescriptor::SUBSTITUTED,
                    )
                };
                let info = SinglePointInfo {
                    ioa,
                    value,
                    qds,
                    ..Default::default()
                };
                c.send_single(false, coa, ca, &[info]).await
            }
            200 => {
                let info = MeasuredValueNormalInfo {
                    ioa,
                    value: Normalize(p.3),
                    ..Default::default()
                };
                c.send_measured_value_normal(false, coa, ca, &[info]).await
            }
            201 | 210 | 211 => {
                let (value, qds) = match ioa {
                    201 => (p.4, QualityDescriptor::GOOD),
                    210 => (i16::MIN, QualityDescriptor::OVERFLOW),
                    _ => (-1, QualityDescriptor::GOOD),
                };
                let info = MeasuredValueScaledInfo {
                    ioa,
                    value,
                    qds,
                    ..Default::default()
                };
                c.send_measured_value_scaled(false, coa, ca, &[info]).await
            }
            202 => {
                let info = MeasuredValueFloatInfo {
                    ioa,
                    value: p.5,
                    ..Default::default()
                };
                c.send_measured_value_float(false, coa, ca, &[info]).await
            }
            300 => {
                let info = BinaryCounterReadingInfo {
                    ioa,
                    value: BinaryCounterReading {
                        counter_reading: 123456,
                        seq_number: 1,
                        ..Default::default()
                    },
                    time: None,
                    time_flags: TimeTagFlags::GOOD,
                };
                c.send_integrated_totals(false, coa, ca, &[info]).await
            }
            _ => {
                c.send(pack.reply_mirror(Cause::UNKNOWN_IOA).negated())
                    .await
            }
        }
    }

    async fn clock_sync(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        _: Option<chrono::DateTime<chrono::Utc>>,
    ) -> rs_iec60870_5::Result<()> {
        if !for_station(pack) {
            return c.send(pack.reply_mirror(Cause::UNKNOWN_CA).negated()).await;
        }
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await
    }

    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        if self.sender.handle(c, pack).await? {
            return Ok(());
        }
        if !for_station(pack) {
            return c.send(pack.reply_mirror(Cause::UNKNOWN_CA).negated()).await;
        }
        let (ioa, select, value) = match pack.type_id() {
            TypeId::C_TS_TA_1 => return c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await,
            TypeId::C_SC_NA_1 | TypeId::C_SC_TA_1 => {
                let i = pack.get_single_cmd()?;
                (i.ioa, i.qoc.in_select, Cmd::Single(i.value))
            }
            TypeId::C_DC_NA_1 | TypeId::C_DC_TA_1 => {
                let i = pack.get_double_cmd()?;
                (i.ioa, i.qoc.in_select, Cmd::Double(i.value as u8))
            }
            TypeId::C_RC_NA_1 | TypeId::C_RC_TA_1 => {
                let i = pack.get_step_cmd()?;
                (i.ioa, i.qoc.in_select, Cmd::Step(i.value as u8))
            }
            TypeId::C_SE_NA_1 | TypeId::C_SE_TA_1 => {
                let i = pack.get_setpoint_normal_cmd()?;
                (i.ioa, i.qos.in_select, Cmd::Normal(i.value.0))
            }
            TypeId::C_SE_NB_1 | TypeId::C_SE_TB_1 => {
                let i = pack.get_setpoint_scaled_cmd()?;
                (i.ioa, i.qos.in_select, Cmd::Scaled(i.value))
            }
            TypeId::C_SE_NC_1 | TypeId::C_SE_TC_1 => {
                let i = pack.get_setpoint_float_cmd()?;
                (i.ioa, i.qos.in_select, Cmd::Float(i.value))
            }
            _ => return Err(rs_iec60870_5::Error::TypeIdentifier),
        };
        let cause = pack.coa().cause;
        if cause != Cause::ACTIVATION && cause != Cause::DEACTIVATION {
            return c
                .send(pack.reply_mirror(Cause::UNKNOWN_COT).negated())
                .await;
        }
        // The CP56Time2a variant is the same object as the plain command type.
        let plain = |t: TypeId| match t {
            TypeId::C_SC_TA_1 => TypeId::C_SC_NA_1,
            TypeId::C_DC_TA_1 => TypeId::C_DC_NA_1,
            TypeId::C_RC_TA_1 => TypeId::C_RC_NA_1,
            TypeId::C_SE_TA_1 => TypeId::C_SE_NA_1,
            TypeId::C_SE_TB_1 => TypeId::C_SE_NB_1,
            TypeId::C_SE_TC_1 => TypeId::C_SE_NC_1,
            t => t,
        };
        let Some(&(_, _, target, report, select_required)) = COMMANDS
            .iter()
            .find(|k| k.0 == ioa && k.1 == plain(pack.type_id()))
        else {
            return c
                .send(pack.reply_mirror(Cause::UNKNOWN_IOA).negated())
                .await;
        };
        let refuse = |p: &Asdu| p.reply_mirror(Cause::ACTIVATION_CON).negated();
        if cause == Cause::DEACTIVATION {
            self.state.lock().unwrap().selected.remove(&ioa);
            return c.send(pack.reply_mirror(Cause::DEACTIVATION_CON)).await;
        }
        if matches!(value, Cmd::Double(0 | 3) | Cmd::Step(0 | 3)) {
            return c.send(refuse(pack)).await;
        }
        if select {
            self.state.lock().unwrap().selected.insert(ioa);
            return c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await;
        }
        let now = Some(chrono::Utc::now());
        let coa = CauseOfTransmission::new(report);
        // Decide and update under the lock; send afterwards.
        enum Report {
            Single(bool),
            Double(u8),
            Step(i8),
            Normal(i16),
            Scaled(i16),
            Float(f32),
        }
        let outcome = {
            let mut p = self.state.lock().unwrap();
            let selected = p.selected.remove(&ioa);
            if select_required && !selected {
                None
            } else {
                match value {
                    Cmd::Single(v) => {
                        p.sp100 = v;
                        Some(Report::Single(v))
                    }
                    Cmd::Double(v) => {
                        p.dp101 = v;
                        Some(Report::Double(v))
                    }
                    Cmd::Step(v) => {
                        let next = p.st102 + if v == 2 { 1 } else { -1 };
                        if !(-64..=63).contains(&next) {
                            None
                        } else {
                            p.st102 = next;
                            Some(Report::Step(next))
                        }
                    }
                    Cmd::Normal(v) => {
                        p.na200 = v;
                        Some(Report::Normal(v))
                    }
                    Cmd::Scaled(v) => {
                        p.nb201 = v;
                        Some(Report::Scaled(v))
                    }
                    Cmd::Float(v) => {
                        p.nc202 = v;
                        Some(Report::Float(v))
                    }
                }
            }
        };
        let Some(outcome) = outcome else {
            return c.send(refuse(pack)).await;
        };
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
        match outcome {
            Report::Single(value) => {
                let i = SinglePointInfo {
                    ioa: target,
                    value,
                    time: now,
                    ..Default::default()
                };
                c.send_single_cp56time2a(coa, 1, &[i]).await?
            }
            Report::Double(v) => {
                let i = DoublePointInfo {
                    ioa: target,
                    value: dp(v),
                    time: now,
                    ..Default::default()
                };
                c.send_double_cp56time2a(coa, 1, &[i]).await?
            }
            Report::Step(val) => {
                let i = StepPositionInfo {
                    ioa: target,
                    value: StepPosition {
                        val,
                        has_transient: false,
                    },
                    time: now,
                    ..Default::default()
                };
                c.send_step_cp56time2a(coa, 1, &[i]).await?
            }
            Report::Normal(v) => {
                let i = MeasuredValueNormalInfo {
                    ioa: target,
                    value: Normalize(v),
                    time: now,
                    ..Default::default()
                };
                c.send_measured_value_normal_cp56time2a(coa, 1, &[i])
                    .await?
            }
            Report::Scaled(value) => {
                let i = MeasuredValueScaledInfo {
                    ioa: target,
                    value,
                    time: now,
                    ..Default::default()
                };
                c.send_measured_value_scaled_cp56time2a(coa, 1, &[i])
                    .await?
            }
            Report::Float(value) => {
                let i = MeasuredValueFloatInfo {
                    ioa: target,
                    value,
                    time: now,
                    ..Default::default()
                };
                c.send_measured_value_float_cp56time2a(coa, 1, &[i]).await?
            }
        }
        c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    }
}

/// Every case of `cases.tsv`, with a fresh Rust station each.
async fn reference_master_against_rust(stack: &str) {
    if !have_image(stack) {
        return;
    }
    let caps = capabilities(stack);
    if caps["roles"]["client"] != true {
        return;
    }
    // j60870 1.8.0 cannot receive C_RD_NA_1, nor can it cope with a refused read.
    let read_broken = stack == "openmuc" && caps["upstream"]["version"] == "1.8.0";
    let mut ran = 0;
    let mut failures = Vec::new();
    for line in include_str!("interop/iec104/cases.tsv").lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let (exit, expected, needs, args) = (f[1], f[2], f[3], f[4]);
        // This station serves time-tagged commands and files.
        let supported = needs == "-"
            || needs.split(',').all(|n| match n.split_once(':') {
                Some(("server", feat)) => ["timeTaggedCommands", "fileServer"].contains(&feat),
                Some(("client", feat)) => feature(&caps, feat),
                _ => false,
            });
        if !supported || (read_broken && args.starts_with("read")) {
            continue;
        }
        let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = Server::new(Baseline::new());
        let serving = server.clone();
        let task = tokio::spawn(async move { serving.serve(listener).await });
        let args: Vec<&str> = args.split_whitespace().collect();
        let (code, doc) = ref_client(stack, port, &args).await;
        server.close();
        let _ = task.await;
        ran += 1;
        let got = summary(&doc);
        if got != expected || code.to_string() != exit {
            failures.push(format!(
                "{args:?}\n  expected exit {exit}: {expected}\n  got      exit {code}: {got}\n  error: {}",
                doc["error"]
            ));
        }
        // A downloaded file must be what the fixture says.
        if args[0] == "file-get" && doc["ok"] == true {
            use sha2::{Digest, Sha256};
            let sha256_hex = |d: &[u8]| -> String {
                Sha256::digest(d)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect()
            };
            let ioa: u32 = args[2].parse().unwrap();
            let size = if ioa == 30000 { 1000 } else { 5000 };
            if doc["file"]["sha256"] != sha256_hex(&fixture_file(ioa, size)) {
                failures.push(format!("{args:?}: file content differs: {}", doc["file"]));
            }
        }
    }
    assert!(ran > 0, "{stack}: no case ran");
    assert!(
        failures.is_empty(),
        "{stack} client against the Rust station: {} of {ran} cases differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[tokio::test]
async fn lib60870_master_with_rust_station() {
    reference_master_against_rust("lib60870").await
}
#[tokio::test]
async fn openmuc_master_with_rust_station() {
    reference_master_against_rust("openmuc").await
}
#[tokio::test]
async fn wendy512_master_with_rust_station() {
    reference_master_against_rust("wendy512").await
}
#[tokio::test]
async fn cbsigma_master_with_rust_station() {
    reference_master_against_rust("cbsigma").await
}
