// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! IEC 60870-5-103 interoperability against `github.com/riclolsen/go-iecp5`.
//!
//! Both implementations are masters only, so there is no client/server pairing
//! to test. Instead the **same simulated protection device** is driven by each
//! master in turn, and the two runs are compared: the device must see the same
//! link procedure, and each master must decode the device's replies to the same
//! values.
//!
//! The test skips when the Go toolchain or the go-iecp5 checkout is missing.

#![cfg(feature = "cs103")]

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use rs_iec60870_5::cs101::{TcpConfig, TransportType};
use rs_iec60870_5::cs103::{
    Asdu, Client, ClientHandler, ClientOption, Config, Dco, IdentificationInfo, Link,
    MeasurandsInfo, TypeId, fun, inf,
};

mod common;
use common::relay::{Event, RelaySim};
use common::{GoPeer, go_bins};

/// The link and common address of the simulated relay, matching `go103cli`.
const DEVICE: u8 = 3;

/// The link procedure a master must perform, in order, ignoring how many
/// polls it interleaves.
fn procedure(events: &[Event]) -> Vec<&'static str> {
    let mut out = Vec::new();
    for e in events {
        let label = match e {
            Event::StatusRequest => "status",
            Event::ResetCu => "reset-cu",
            Event::ResetFcb => "reset-fcb",
            Event::Class1Poll => "class1",
            Event::Class2Poll => "class2",
            Event::RepeatedFrame => "repeat",
            Event::UserData(a) => match a.type_id {
                TypeId::TIME_SYNC => "time-sync",
                TypeId::GENERAL_INTERROGATION => "interrogation",
                TypeId::GENERAL_COMMAND => "command",
                _ => "other",
            },
        };
        // Collapse runs of the same step, so poll counts do not matter.
        if out.last() != Some(&label) {
            out.push(label);
        }
    }
    out
}

/// What this crate's master decoded from the device.
#[derive(Default)]
struct Log {
    identifications: Mutex<Vec<IdentificationInfo>>,
    measurands: Mutex<Vec<MeasurandsInfo>>,
    command_acks: Mutex<Vec<u8>>,
    gi_terminations: Mutex<Vec<u8>>,
}

struct Master {
    log: Arc<Log>,
}

#[async_trait::async_trait]
impl ClientHandler for Master {
    async fn identification(
        &self,
        _l: &dyn Link,
        _pack: &Asdu,
        info: IdentificationInfo,
    ) -> rs_iec60870_5::Result<()> {
        self.log.identifications.lock().await.push(info);
        Ok(())
    }

    async fn measurands(
        &self,
        _l: &dyn Link,
        _pack: &Asdu,
        info: MeasurandsInfo,
    ) -> rs_iec60870_5::Result<()> {
        self.log.measurands.lock().await.push(info);
        Ok(())
    }

    async fn time_tagged(
        &self,
        _l: &dyn Link,
        pack: &Asdu,
        info: rs_iec60870_5::cs103::TimeTaggedInfo,
    ) -> rs_iec60870_5::Result<()> {
        if pack.coa == rs_iec60870_5::cs103::Cause::COMMAND_ACK_POS {
            self.log.command_acks.lock().await.push(info.sin);
        }
        Ok(())
    }

    async fn gi_termination(&self, _l: &dyn Link, _pack: &Asdu, scn: u8) -> rs_iec60870_5::Result<()> {
        self.log.gi_terminations.lock().await.push(scn);
        Ok(())
    }
}

/// Drive the simulated relay with this crate's master and report what happened.
async fn run_rust_master() -> (Vec<Event>, Arc<Log>) {
    let (sock, sim) = RelaySim::spawn(DEVICE).await;

    let cfg = Config {
        transport: TransportType::TcpClient,
        tcp: TcpConfig {
            address: sock.to_string(),
            ..Default::default()
        },
        link_address: DEVICE,
        timeout_response_t1: Duration::from_secs(2),
        timeout_repeat_t2: Duration::from_secs(1),
        timeout_test_t3: Duration::from_secs(30),
        timeout_send_link_msg: Duration::from_millis(20),
        auto_init: true,
        ..Default::default()
    };

    let log = Arc::new(Log::default());
    let cli = Client::new(
        Master {
            log: Arc::clone(&log),
        },
        ClientOption::new().with_config(cfg).unwrap(),
    );
    cli.start().unwrap();
    tokio::time::timeout(Duration::from_secs(10), cli.wait_link_active())
        .await
        .expect("the Rust master did not activate the link");

    // The same script go103cli runs: let auto-init finish, then command and
    // interrogate.
    tokio::time::sleep(Duration::from_millis(700)).await;
    cli.general_command(
        DEVICE,
        fun::OVERCURRENT_PROTECTION,
        inf::AUTO_RECLOSER_ACTIVE,
        Dco::On,
        42,
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    cli.general_interrogation(DEVICE, 7).unwrap();

    sim.wait("the interrogation with scan number 7", |events| {
        events.iter().any(|e| {
            matches!(e, Event::UserData(a)
                if a.type_id == TypeId::GENERAL_INTERROGATION
                    && a.get_general_interrogation() == Ok(7))
        })
    })
    .await;
    tokio::time::sleep(Duration::from_millis(400)).await;

    let events = sim.events().await;
    cli.close();
    (events, log)
}

/// Drive the same simulated relay with go-iecp5's master.
async fn run_go_master(bins: &std::path::Path) -> (Vec<Event>, GoPeer) {
    let (sock, sim) = RelaySim::spawn(DEVICE).await;
    let go = GoPeer::spawn(&bins.join("go103cli"), &[&sock.to_string()]).await;

    go.wait("the Go master to activate the link", |v| {
        v["event"] == "link_active"
    })
    .await;
    go.wait("the Go master to finish its script", |v| v["event"] == "done")
        .await;

    (sim.events().await, go)
}

#[tokio::test]
async fn both_masters_drive_the_device_through_the_same_procedure() {
    let Some(bins) = go_bins() else { return };

    let (rust_events, rust_log) = run_rust_master().await;
    let (go_events, go) = run_go_master(bins).await;

    // -- the link procedure the device observed ---------------------------
    let rust = procedure(&rust_events);
    let go_seq = procedure(&go_events);

    // Both must open with status of link followed by reset of the CU.
    assert_eq!(
        &rust[..2],
        &["status", "reset-cu"],
        "rust procedure was {rust:?}"
    );
    assert_eq!(
        &go_seq[..2],
        &["status", "reset-cu"],
        "go procedure was {go_seq:?}"
    );

    // Both must run the same auto-init: time sync, then interrogation.
    for (name, seq) in [("rust", &rust), ("go", &go_seq)] {
        let ts = seq.iter().position(|s| *s == "time-sync");
        let gi = seq.iter().position(|s| *s == "interrogation");
        assert!(
            ts.is_some() && gi.is_some() && ts < gi,
            "{name} must auto-init with a time sync before the interrogation: {seq:?}"
        );
        assert!(
            seq.contains(&"class1"),
            "{name} must fetch class 1 data on access demand: {seq:?}"
        );
        assert!(
            seq.contains(&"class2"),
            "{name} must poll class 2 for cyclic data: {seq:?}"
        );
        assert!(
            !seq.contains(&"repeat"),
            "{name} sent a frame the device saw as a repetition: {seq:?}"
        );
    }

    // -- the ASDUs the device received ------------------------------------
    let asdu_types = |events: &[Event]| -> Vec<u8> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::UserData(a) => Some(a.type_id.0),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        asdu_types(&rust_events),
        asdu_types(&go_events),
        "both masters must send the same control-direction ASDUs in the same order"
    );

    // The general command must arrive identically from both.
    let command_of = |events: &[Event]| {
        events
            .iter()
            .find_map(|e| match e {
                Event::UserData(a) if a.type_id == TypeId::GENERAL_COMMAND => {
                    a.get_general_command().ok()
                }
                _ => None,
            })
            .expect("a general command")
    };
    let rust_cmd = command_of(&rust_events);
    let go_cmd = command_of(&go_events);
    assert_eq!(rust_cmd, go_cmd, "the encoded general command must match");
    assert_eq!(rust_cmd.fun, fun::OVERCURRENT_PROTECTION);
    assert_eq!(rust_cmd.inf, inf::AUTO_RECLOSER_ACTIVE);
    assert_eq!(rust_cmd.dco, Dco::On);
    assert_eq!(rust_cmd.rii, 42);

    // -- what each master decoded from the device -------------------------
    let ids = rust_log.identifications.lock().await;
    assert_eq!(ids[0].ascii, "DSCRELAY");
    assert_eq!(ids[0].col, 4);

    let go_id = go
        .wait("the Go master to decode the identification", |v| {
            v["event"] == "identification"
        })
        .await;
    assert_eq!(
        go_id["ascii"].as_str().unwrap(),
        ids[0].ascii,
        "both masters must decode the same identification"
    );
    assert_eq!(go_id["col"].as_u64().unwrap() as u8, ids[0].col);

    let rust_measurands: Vec<f64> = rust_log.measurands.lock().await[0]
        .values
        .iter()
        .map(|m| m.f64())
        .collect();
    let go_meas = go
        .wait("the Go master to decode measurands", |v| {
            v["event"] == "measurands"
        })
        .await;
    let go_values: Vec<f64> = go_meas["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    assert_eq!(
        rust_measurands, go_values,
        "both masters must scale the 13 bit measurands identically"
    );
    assert_eq!(rust_measurands, vec![0.5, -0.25]);

    // The command acknowledgement carries the RII in its supplementary
    // information; both masters must recover it.
    assert!(
        rust_log.command_acks.lock().await.contains(&42),
        "the Rust master must see the RII echoed"
    );
    let go_ack = go
        .wait("the Go master to see the command acknowledgement", |v| {
            v["event"] == "time_tagged" && v["cause"] == 20
        })
        .await;
    assert_eq!(go_ack["sin"].as_u64().unwrap(), 42);

    // Both must report the explicit interrogation terminating with scan 7.
    assert!(rust_log.gi_terminations.lock().await.contains(&7));
    go.wait("the Go master to see GI termination 7", |v| {
        v["event"] == "gi_termination" && v["scn"] == 7
    })
    .await;

    go.kill().await;
}
