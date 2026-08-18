// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! End-to-end tests of the IEC 60870-5-103 master against a simulated
//! protection device.
//!
//! The device runs the FT1.2 secondary procedure over the TCP encapsulation
//! transport, so link initialization, FCB tracking, class 1/2 polling and ACD
//! signalling are exercised exactly as they would be over a serial line.

#![cfg(feature = "cs103")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;

use rs_iec60870_5::cs101::{TcpConfig, TransportType};
use rs_iec60870_5::cs103::{
    Asdu, Cause, Client, ClientHandler, ClientOption, Config, Dco, Dpi, IdentificationInfo, Link,
    MeasurandsInfo, TimeTaggedInfo, TypeId, fun, inf,
};

mod common;
use common::relay::{Event, RelaySim};

/// Everything the master's handler saw.
#[derive(Default)]
struct Log {
    asdus: Mutex<Vec<Asdu>>,
    time_tagged: Mutex<Vec<TimeTaggedInfo>>,
    measurands: Mutex<Vec<MeasurandsInfo>>,
    identifications: Mutex<Vec<IdentificationInfo>>,
    gi_terminations: Mutex<Vec<u8>>,
    devices_active: AtomicUsize,
}

struct Master {
    log: Arc<Log>,
}

#[async_trait::async_trait]
impl ClientHandler for Master {
    async fn time_tagged(
        &self,
        _l: &dyn Link,
        _pack: &Asdu,
        info: TimeTaggedInfo,
    ) -> rs_iec60870_5::Result<()> {
        self.log.time_tagged.lock().await.push(info);
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

    async fn identification(
        &self,
        _l: &dyn Link,
        _pack: &Asdu,
        info: IdentificationInfo,
    ) -> rs_iec60870_5::Result<()> {
        self.log.identifications.lock().await.push(info);
        Ok(())
    }

    async fn gi_termination(&self, _l: &dyn Link, _pack: &Asdu, scn: u8) -> rs_iec60870_5::Result<()> {
        self.log.gi_terminations.lock().await.push(scn);
        Ok(())
    }

    async fn asdu_all(&self, _l: &dyn Link, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        self.log.asdus.lock().await.push(pack.clone());
        Ok(())
    }

    async fn on_device_active(&self, _l: &dyn Link, _addr: u8) {
        self.log.devices_active.fetch_add(1, Ordering::SeqCst);
    }
}

/// Timings tuned for a fast test while staying in the legal ranges.
fn cfg(addr: u8, auto_init: bool) -> Config {
    Config {
        link_address: addr,
        timeout_response_t1: Duration::from_secs(2),
        timeout_repeat_t2: Duration::from_secs(1),
        timeout_test_t3: Duration::from_secs(30),
        timeout_send_link_msg: Duration::from_millis(10),
        auto_init,
        ..Default::default()
    }
}

/// Bring up a simulated relay and a master dialling into it.
async fn pair(auto_init: bool) -> (Arc<RelaySim>, Arc<Client<Master>>, Arc<Log>) {
    const ADDR: u8 = 3;
    let (sock, sim) = RelaySim::spawn(ADDR).await;

    let mut c = cfg(ADDR, auto_init);
    c.transport = TransportType::TcpClient;
    c.tcp = TcpConfig {
        address: sock.to_string(),
        ..Default::default()
    };

    let log = Arc::new(Log::default());
    let cli = Client::new(
        Master {
            log: Arc::clone(&log),
        },
        ClientOption::new().with_config(c).unwrap(),
    );
    cli.start().unwrap();

    tokio::time::timeout(Duration::from_secs(10), cli.wait_link_active())
        .await
        .expect("the link did not become active");

    (sim, cli, log)
}

async fn eventually(label: &str, mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for: {label}");
}

#[tokio::test]
async fn the_link_initialises_with_status_then_reset() {
    let (sim, cli, log) = pair(false).await;

    assert!(cli.is_connected());
    assert!(cli.is_link_active());

    let events = sim.events().await;
    assert_eq!(
        &events[..2],
        &[Event::StatusRequest, Event::ResetCu],
        "the standard 103 start-up is status of link, then reset of the CU"
    );
    assert_eq!(log.devices_active.load(Ordering::SeqCst), 1);

    cli.close();
}

#[tokio::test]
async fn the_identification_message_is_collected_after_the_reset() {
    let (_sim, cli, log) = pair(false).await;

    // The device sets ACD on its reset acknowledgement, so the master fetches
    // the identification with a class 1 poll without being told to.
    eventually("the identification arrives", async || {
        !log.identifications.lock().await.is_empty()
    })
    .await;

    let ids = log.identifications.lock().await;
    assert_eq!(ids[0].ascii, "DSCRELAY");
    assert_eq!(ids[0].col, 4, "compatibility level");
    assert_eq!(ids[0].fun, fun::GLOBAL);

    cli.close();
}

#[tokio::test]
async fn cyclic_measurands_arrive_through_class_two_polls() {
    let (sim, cli, log) = pair(false).await;

    eventually("measurands arrive", async || {
        !log.measurands.lock().await.is_empty()
    })
    .await;

    let m = log.measurands.lock().await;
    assert_eq!(m[0].fun, fun::OVERCURRENT_PROTECTION);
    assert_eq!(m[0].inf, inf::MEASURAND_IV);
    assert_eq!(m[0].values.len(), 2);
    assert_eq!(m[0].values[0].f64(), 0.5);
    assert_eq!(m[0].values[1].f64(), -0.25);

    assert!(
        sim.events().await.contains(&Event::Class2Poll),
        "cyclic data is collected by class 2 polls"
    );

    cli.close();
}

#[tokio::test]
async fn auto_init_sends_a_time_sync_and_a_general_interrogation() {
    let (sim, cli, log) = pair(true).await;

    sim.wait("the auto-init sequence", |events| {
        let sent: Vec<TypeId> = events
            .iter()
            .filter_map(|e| match e {
                Event::UserData(a) => Some(a.type_id),
                _ => None,
            })
            .collect();
        sent.contains(&TypeId::TIME_SYNC) && sent.contains(&TypeId::GENERAL_INTERROGATION)
    })
    .await;

    // The order matters: set the clock before reading the process image.
    let sent: Vec<TypeId> = sim
        .received_asdus()
        .await
        .iter()
        .map(|a| a.type_id)
        .collect();
    let ts = sent.iter().position(|t| *t == TypeId::TIME_SYNC).unwrap();
    let gi = sent
        .iter()
        .position(|t| *t == TypeId::GENERAL_INTERROGATION)
        .unwrap();
    assert!(ts < gi, "the time synchronization precedes the interrogation");

    // The interrogation reply and its termination both come back.
    eventually("the interrogation terminates", async || {
        !log.gi_terminations.lock().await.is_empty()
    })
    .await;
    assert_eq!(log.gi_terminations.lock().await[0], 0, "scan number 0");

    let replies = log.time_tagged.lock().await;
    let gi_replies: Vec<_> = replies.iter().filter(|_| true).collect();
    assert!(
        gi_replies.len() >= 2,
        "the device reported two objects: {gi_replies:#?}"
    );

    cli.close();
}

#[tokio::test]
async fn a_general_command_is_acknowledged_with_its_rii() {
    let (sim, cli, log) = pair(false).await;

    cli.general_command(
        3,
        fun::OVERCURRENT_PROTECTION,
        inf::AUTO_RECLOSER_ACTIVE,
        Dco::On,
        42,
    )
    .unwrap();

    sim.wait("the device to receive the command", |events| {
        events.iter().any(|e| {
            matches!(e, Event::UserData(a) if a.type_id == TypeId::GENERAL_COMMAND)
        })
    })
    .await;

    let received = sim
        .received_asdus()
        .await
        .into_iter()
        .find(|a| a.type_id == TypeId::GENERAL_COMMAND)
        .unwrap();
    let cmd = received.get_general_command().unwrap();
    assert_eq!(cmd.fun, fun::OVERCURRENT_PROTECTION);
    assert_eq!(cmd.inf, inf::AUTO_RECLOSER_ACTIVE);
    assert_eq!(cmd.dco, Dco::On);
    assert_eq!(cmd.rii, 42);

    // The acknowledgement comes back as ASDU 1 with cause 20 and the RII in SIN.
    eventually("the command acknowledgement", async || {
        log.time_tagged
            .lock()
            .await
            .iter()
            .any(|i| i.sin == 42 && i.dpi == Dpi::On)
    })
    .await;

    let ack = log
        .asdus
        .lock()
        .await
        .iter()
        .find(|a| a.type_id == TypeId::TIME_TAGGED && a.coa == Cause::COMMAND_ACK_POS)
        .cloned()
        .expect("a positive command acknowledgement");
    assert_eq!(ack.get_time_tagged().unwrap().sin, 42);

    cli.close();
}

#[tokio::test]
async fn an_explicit_general_interrogation_returns_its_scan_number() {
    let (_sim, cli, log) = pair(false).await;

    cli.general_interrogation(3, 7).unwrap();

    eventually("the interrogation terminates", async || {
        log.gi_terminations.lock().await.contains(&7)
    })
    .await;

    cli.close();
}

#[tokio::test]
async fn a_time_synchronisation_is_mirrored_back() {
    let (_sim, cli, log) = pair(false).await;

    cli.time_sync(3).unwrap();

    eventually("the time sync mirror", async || {
        log.asdus
            .lock()
            .await
            .iter()
            .any(|a| a.type_id == TypeId::TIME_SYNC && a.coa == Cause::TIME_SYNC)
    })
    .await;

    let mirror = log
        .asdus
        .lock()
        .await
        .iter()
        .find(|a| a.type_id == TypeId::TIME_SYNC)
        .cloned()
        .unwrap();
    assert!(
        mirror.get_time_sync().unwrap().is_some(),
        "the mirrored CP56Time2a must decode"
    );

    cli.close();
}

#[tokio::test]
async fn the_frame_count_bit_never_makes_the_device_see_a_repeat() {
    let (sim, cli, _log) = pair(true).await;

    // Drive a long exchange so many FCV frames alternate.
    for i in 0..15u8 {
        cli.general_command(3, fun::OVERCURRENT_PROTECTION, inf::LED_RESET, Dco::On, i)
            .unwrap();
    }
    sim.wait("every command to arrive", |events| {
        events
            .iter()
            .filter(|e| matches!(e, Event::UserData(a) if a.type_id == TypeId::GENERAL_COMMAND))
            .count()
            == 15
    })
    .await;

    assert!(
        !sim.events().await.contains(&Event::RepeatedFrame),
        "a correctly alternating FCB never looks like a repetition"
    );

    // And every command arrived exactly once, in order.
    let riis: Vec<u8> = sim
        .received_asdus()
        .await
        .iter()
        .filter(|a| a.type_id == TypeId::GENERAL_COMMAND)
        .map(|a| a.get_general_command().unwrap().rii)
        .collect();
    assert_eq!(riis, (0..15).collect::<Vec<u8>>());

    cli.close();
}

#[tokio::test]
async fn sending_before_the_line_is_open_is_refused() {
    let mut c = cfg(3, false);
    c.transport = TransportType::TcpClient;
    c.tcp = TcpConfig {
        address: "127.0.0.1:1".into(),
        ..Default::default()
    };

    let cli = Client::new(
        Master {
            log: Arc::new(Log::default()),
        },
        ClientOption::new().with_config(c).unwrap(),
    );
    assert!(!cli.is_link_active());
    assert_eq!(
        cli.general_interrogation(3, 0),
        Err(rs_iec60870_5::Error::UseClosedConnection)
    );
}
