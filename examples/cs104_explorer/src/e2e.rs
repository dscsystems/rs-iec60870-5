// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! End-to-end test: the explorer driven headlessly against a real outstation.
//!
//! The UI loop is replaced by a pump that feeds keystrokes and drains the
//! event channel, so the whole path is exercised — connect, StartDT, the
//! request keys, the command builder — without a terminal.

#![cfg(test)]

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{Server, ServerHandler};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::app::{App, Focus, Tab};
use crate::event::{Receiver, channel};

/// A minimal outstation: answers interrogation with two points, and confirms
/// single commands.
struct TestOutstation {
    commands: Arc<Mutex<Vec<SingleCommandInfo>>>,
}

#[async_trait::async_trait]
impl ServerHandler for TestOutstation {
    async fn interrogation(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        _qoi: QualifierOfInterrogation,
    ) -> rs_iec60870_5::Result<()> {
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;

        let coa = CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION);
        c.send_single(
            false,
            coa,
            pack.common_addr(),
            &[SinglePointInfo::new(100, true)],
        )
        .await?;
        c.send_measured_value_float(
            false,
            coa,
            pack.common_addr(),
            &[MeasuredValueFloatInfo {
                ioa: 400,
                value: 21.5,
                qds: QualityDescriptor::GOOD,
                time: None,
            }],
        )
        .await?;

        c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    }

    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        if pack.type_id() == TypeId::C_SC_NA_1 {
            self.commands.lock().await.push(pack.get_single_cmd()?);
            c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
            return c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await;
        }
        Ok(())
    }
}

/// The explorer under test, wired to an outstation on loopback.
struct Harness {
    app: App,
    rx: Receiver,
    commands: Arc<Mutex<Vec<SingleCommandInfo>>>,
    server: Arc<Server<TestOutstation>>,
}

impl Harness {
    async fn start() -> Harness {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let server = Server::new(TestOutstation {
            commands: Arc::clone(&commands),
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        {
            let server = Arc::clone(&server);
            tokio::spawn(async move { server.serve(listener).await });
        }

        let (tx, rx) = channel();
        let app = App::new(tx, Arc::new(AtomicBool::new(false)), Some(addr));

        Harness {
            app,
            rx,
            commands,
            server,
        }
    }

    fn press(&mut self, code: KeyCode) {
        self.app.key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
    }

    /// Drain whatever the protocol tasks have produced.
    fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            self.app.apply(msg);
        }
    }

    /// Pump until `done` holds, or fail the test.
    async fn until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            self.pump();
            if done(&self.app) {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!(
                    "timed out waiting for {what}; log:\n{}",
                    self.app.logs.join("\n")
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn stop(&mut self) {
        self.app.key(KeyEvent {
            code: KeyCode::Char('x'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        self.server.close();
    }
}

#[tokio::test]
async fn connecting_and_interrogating_fills_the_points_table() {
    let mut h = Harness::start().await;

    // 'c' connects; StartDT is automatic, so the link should go active.
    h.press(KeyCode::Char('c'));
    h.until("the link to become active", |a| a.active).await;
    assert!(h.app.connected);
    assert!(
        h.app.logs.iter().any(|l| l.contains("STARTDT confirmed")),
        "the activation is reported in the log"
    );

    // 'g' runs a general interrogation.
    h.press(KeyCode::Char('g'));
    h.until("the interrogation response", |a| a.points.len() >= 2)
        .await;

    let single = &h.app.points[&100];
    assert_eq!(single.type_name, "M_SP_NA_1");
    assert_eq!(single.value, "on");
    assert_eq!(single.cause, "InterrogatedByStation");

    let measured = &h.app.points[&400];
    assert_eq!(measured.type_name, "M_ME_NC_1");
    assert_eq!(measured.value, "21.5");

    // Both the request and the received ASDUs are in the log.
    assert!(h.app.logs.iter().any(|l| l.contains("[tx] general interrogation -> sent")));
    assert!(h.app.logs.iter().any(|l| l.contains("[rx] TID<M_SP_NA_1>")));

    h.stop();
}

#[tokio::test]
async fn the_command_builder_sends_what_the_form_describes() {
    let mut h = Harness::start().await;
    h.press(KeyCode::Char('c'));
    h.until("the link to become active", |a| a.active).await;

    // Open the builder and fill it in: single command, IOA 6000, on.
    h.press(KeyCode::Char('3'));
    assert_eq!(h.app.tab, Tab::Send);
    h.press(KeyCode::Char('i'));
    assert_eq!(h.app.focus, Focus::Form);

    h.press(KeyCode::Tab); // to IOA
    for c in "6000".chars() {
        h.press(KeyCode::Char(c));
    }
    h.press(KeyCode::Tab); // to value
    for c in "on".chars() {
        h.press(KeyCode::Char(c));
    }
    h.press(KeyCode::Enter);

    // The outstation received exactly what the form described.
    let commands = Arc::clone(&h.commands);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        h.pump();
        if !commands.lock().await.is_empty() {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the command never arrived; log:\n{}",
            h.app.logs.join("\n")
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let received = commands.lock().await[0];
    assert_eq!(received.ioa, 6000);
    assert!(received.value);
    assert!(!received.qoc.in_select, "execute, not select");

    // The confirmation came back and is visible.
    h.until("the command confirmation", |a| {
        a.logs
            .iter()
            .any(|l| l.contains("C_SC_NA_1") && l.contains("ActivationTerm"))
    })
    .await;

    h.stop();
}

#[tokio::test]
async fn disconnecting_clears_the_status_and_stops_the_client() {
    let mut h = Harness::start().await;
    h.press(KeyCode::Char('c'));
    h.until("the link to become active", |a| a.active).await;

    h.press(KeyCode::Char('x'));
    assert!(!h.app.connected);
    assert!(!h.app.active);
    assert!(h.app.client.is_none());

    // Requests are refused again once disconnected.
    let before = h.app.logs.len();
    h.press(KeyCode::Char('g'));
    assert!(h.app.logs[before..].iter().any(|l| l.contains("not active")));

    h.server.close();
}

#[tokio::test]
async fn a_bad_target_address_is_reported_rather_than_panicking() {
    let (tx, _rx) = channel();
    let mut app = App::new(
        tx,
        Arc::new(AtomicBool::new(false)),
        Some("not a valid address".into()),
    );

    app.key(KeyEvent {
        code: KeyCode::Char('c'),
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    });

    assert!(app.client.is_none());
    assert!(
        app.logs.iter().any(|l| l.contains("bad server address")),
        "log was: {:?}",
        app.logs
    );
}
