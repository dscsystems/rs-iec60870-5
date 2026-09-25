// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! End-to-end tests of the IEC 60870-5-101 link layer.
//!
//! The two stations are wired through the TCP encapsulation transport, so the
//! whole FT1.2 procedure — link initialization, FCB tracking, class 1/2
//! polling, ACD signalling and duplicate detection — runs exactly as it does
//! over a serial line, without needing one.

#![cfg(feature = "cs101")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;

use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs101::{
    Client, ClientHandler, ClientOption, Config, Server, ServerHandler, TcpConfig, TransmissionMode,
    TransportType,
};

/// Everything the test asserts on, recorded as it arrives.
#[derive(Default)]
struct Log {
    asdus: Mutex<Vec<Asdu>>,
    interrogations: AtomicUsize,
    commands: AtomicUsize,
    link_active: AtomicUsize,
}

impl Log {
    async fn first_of(&self, t: TypeId) -> Option<Asdu> {
        self.asdus
            .lock()
            .await
            .iter()
            .find(|a| a.type_id() == t)
            .cloned()
    }

    async fn count_of(&self, t: TypeId) -> usize {
        self.asdus
            .lock()
            .await
            .iter()
            .filter(|a| a.type_id() == t)
            .count()
    }
}

/// A secondary station that answers interrogation and confirms commands.
struct Outstation {
    log: Arc<Log>,
}

#[async_trait::async_trait]
impl ServerHandler for Outstation {
    async fn interrogation(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        qoi: QualifierOfInterrogation,
    ) -> rs_iec60870_5::Result<()> {
        self.log.interrogations.fetch_add(1, Ordering::SeqCst);
        if qoi != QualifierOfInterrogation::STATION {
            return c
                .send(pack.reply_mirror(Cause::ACTIVATION_CON).negated())
                .await;
        }
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;

        let coa = CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION);
        let ca = pack.common_addr();
        c.send_single(
            false,
            coa,
            ca,
            &[
                SinglePointInfo::new(100, true),
                SinglePointInfo {
                    ioa: 101,
                    value: false,
                    qds: QualityDescriptor::INVALID,
                    time: None,
                    time_flags: TimeTagFlags::GOOD,
                },
            ],
        )
        .await?;
        c.send_measured_value_float(
            false,
            coa,
            ca,
            &[MeasuredValueFloatInfo {
                ioa: 400,
                value: 22.5,
                ..Default::default()
            }],
        )
        .await?;
        c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    }

    async fn clock_sync(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        _t: Option<chrono::DateTime<chrono::Utc>>,
    ) -> rs_iec60870_5::Result<()> {
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await
    }

    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        self.log.asdus.lock().await.push(pack.clone());
        self.log.commands.fetch_add(1, Ordering::SeqCst);
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
        c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    }

    async fn on_link_active(&self, _c: &dyn Connect) {
        self.log.link_active.fetch_add(1, Ordering::SeqCst);
    }
}

/// A primary station that records everything it receives.
struct Master {
    log: Arc<Log>,
}

#[async_trait::async_trait]
impl ClientHandler for Master {
    async fn asdu_all(
        &self,
        _c: &dyn Connect,
        pack: &Asdu,
        _n: usize,
    ) -> rs_iec60870_5::Result<()> {
        self.log.asdus.lock().await.push(pack.clone());
        Ok(())
    }

    async fn interrogation(&self, _c: &dyn Connect, _pack: &Asdu) -> rs_iec60870_5::Result<()> {
        self.log.interrogations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn on_link_active(&self, _c: &dyn Connect, _addr: u16) {
        self.log.link_active.fetch_add(1, Ordering::SeqCst);
    }
}

/// Link timings tuned for a fast test while staying in the legal ranges.
fn brisk(mode: TransmissionMode) -> Config {
    Config {
        mode,
        link_address: 1,
        link_addr_size: 1,
        timeout_response_t1: Duration::from_secs(2),
        timeout_repeat_t2: Duration::from_secs(1),
        timeout_test_t3: Duration::from_secs(30),
        timeout_send_link_msg: Duration::from_millis(10),
        ..Default::default()
    }
}

/// Bring up a secondary listening on an ephemeral port and a primary dialling
/// into it, both over the TCP encapsulation transport.
async fn pair(
    mode: TransmissionMode,
) -> (
    Arc<Server<Outstation>>,
    Arc<Client<Master>>,
    Arc<Log>,
    Arc<Log>,
) {
    pair_with(mode, |_| {}).await
}

/// As [`pair`], with `tweak` applied to both stations' configuration.
async fn pair_with(
    mode: TransmissionMode,
    tweak: impl Fn(&mut Config),
) -> (
    Arc<Server<Outstation>>,
    Arc<Client<Master>>,
    Arc<Log>,
    Arc<Log>,
) {
    let server_log = Arc::new(Log::default());
    let client_log = Arc::new(Log::default());

    let mut server_cfg = brisk(mode);
    tweak(&mut server_cfg);
    server_cfg.transport = TransportType::TcpServer;
    server_cfg.tcp = TcpConfig {
        address: "127.0.0.1:0".into(),
        ..Default::default()
    };

    let srv = Server::new(Outstation {
        log: Arc::clone(&server_log),
    })
    .with_config(server_cfg)
    .unwrap();
    srv.bind().await.unwrap();
    let addr = srv.listen_addr().expect("bound").to_string();
    srv.start().unwrap();

    let mut client_cfg = brisk(mode);
    tweak(&mut client_cfg);
    client_cfg.transport = TransportType::TcpClient;
    client_cfg.tcp = TcpConfig {
        address: addr,
        ..Default::default()
    };

    let cli = Client::new(
        Master {
            log: Arc::clone(&client_log),
        },
        ClientOption::new().with_config(client_cfg).unwrap(),
    );
    cli.start().unwrap();

    tokio::time::timeout(Duration::from_secs(10), cli.wait_link_active())
        .await
        .expect("the link did not become active");

    (srv, cli, server_log, client_log)
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
async fn the_link_initialises_and_both_ends_agree_it_is_active() {
    let (srv, cli, server_log, client_log) = pair(TransmissionMode::Unbalanced).await;

    assert!(cli.is_connected());
    assert!(cli.is_link_active());
    eventually("the secondary reports its link active", async || {
        srv.is_link_active()
    })
    .await;

    assert_eq!(client_log.link_active.load(Ordering::SeqCst), 1);
    assert_eq!(server_log.link_active.load(Ordering::SeqCst), 1);

    cli.close();
    srv.close();
}

#[tokio::test]
async fn interrogation_data_is_collected_by_polling() {
    let (srv, cli, server_log, client_log) = pair(TransmissionMode::Unbalanced).await;

    cli.interrogation_cmd(
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        QualifierOfInterrogation::STATION,
    )
    .await
    .unwrap();

    eventually("the interrogation terminates", async || {
        client_log
            .asdus
            .lock()
            .await
            .iter()
            .any(|a| a.type_id() == TypeId::C_IC_NA_1 && a.coa().cause == Cause::ACTIVATION_TERM)
    })
    .await;

    assert_eq!(server_log.interrogations.load(Ordering::SeqCst), 1);

    // The whole process image came back through the class 1 polls.
    let sp = client_log.first_of(TypeId::M_SP_NA_1).await.unwrap();
    let points = sp.get_single_point().unwrap();
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].ioa, 100);
    assert!(points[0].value);
    assert_eq!(points[1].qds, QualityDescriptor::INVALID);
    assert_eq!(sp.coa().cause, Cause::INTERROGATED_BY_STATION);

    let me = client_log.first_of(TypeId::M_ME_NC_1).await.unwrap();
    assert_eq!(me.get_measured_value_float().unwrap()[0].value, 22.5);

    // The master's dedicated handler saw the confirmation, the termination and
    // the interrogation-caused data.
    assert!(client_log.interrogations.load(Ordering::SeqCst) >= 2);

    cli.close();
    srv.close();
}

#[tokio::test]
async fn periodic_data_is_buffered_as_class_2_and_events_as_class_1() {
    let (srv, cli, _, client_log) = pair(TransmissionMode::Unbalanced).await;

    // Periodic goes to class 2, spontaneous to class 1.
    srv.send_measured_value_float(
        false,
        CauseOfTransmission::new(Cause::PERIODIC),
        1,
        &[MeasuredValueFloatInfo {
            ioa: 500,
            value: 1.5,
            ..Default::default()
        }],
    )
    .await
    .unwrap();
    srv.send_single(
        false,
        CauseOfTransmission::new(Cause::SPONTANEOUS),
        1,
        &[SinglePointInfo::new(600, true)],
    )
    .await
    .unwrap();

    let (c1, c2) = srv.buffered();
    assert_eq!((c1, c2), (1, 1), "one event and one cyclic value buffered");

    eventually("both buffers drain to the master", async || {
        let asdus = client_log.asdus.lock().await;
        asdus.iter().any(|a| a.type_id() == TypeId::M_ME_NC_1)
            && asdus.iter().any(|a| a.type_id() == TypeId::M_SP_NA_1)
    })
    .await;

    let sp = client_log.first_of(TypeId::M_SP_NA_1).await.unwrap();
    assert_eq!(sp.coa().cause, Cause::SPONTANEOUS);
    assert_eq!(sp.get_single_point().unwrap()[0].ioa, 600);

    cli.close();
    srv.close();
}

#[tokio::test]
async fn control_commands_reach_the_outstation_and_are_confirmed() {
    let (srv, cli, server_log, client_log) = pair(TransmissionMode::Unbalanced).await;

    cli.send_single_cmd(
        TypeId::C_SC_NA_1,
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        SingleCommandInfo {
            ioa: 6000,
            value: true,
            qoc: QualifierOfCommand {
                qual: QocQual::SHORT_PULSE_DURATION,
                in_select: false,
            },
            time: None,
            time_flags: TimeTagFlags::GOOD,
        },
    )
    .await
    .unwrap();

    eventually("the command terminates", async || {
        client_log
            .asdus
            .lock()
            .await
            .iter()
            .any(|a| a.type_id() == TypeId::C_SC_NA_1 && a.coa().cause == Cause::ACTIVATION_TERM)
    })
    .await;

    let received = server_log.first_of(TypeId::C_SC_NA_1).await.unwrap();
    let cmd = received.get_single_cmd().unwrap();
    assert_eq!(cmd.ioa, 6000);
    assert!(cmd.value);
    assert_eq!(cmd.qoc.qual, QocQual::SHORT_PULSE_DURATION);

    cli.close();
    srv.close();
}

#[tokio::test]
async fn a_long_command_burst_keeps_its_order_and_loses_nothing() {
    let (srv, cli, server_log, _) = pair(TransmissionMode::Unbalanced).await;

    const N: u32 = 25;
    for i in 0..N {
        cli.send_single_cmd(
            TypeId::C_SC_NA_1,
            CauseOfTransmission::new(Cause::ACTIVATION),
            1,
            SingleCommandInfo {
                ioa: 7000 + i,
                value: i % 2 == 0,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }

    let log = Arc::clone(&server_log);
    eventually("every command arrives", async move || {
        log.commands.load(Ordering::SeqCst) == N as usize
    })
    .await;

    // FCB alternation must not duplicate or drop a single frame.
    let received: Vec<u32> = server_log
        .asdus
        .lock()
        .await
        .iter()
        .map(|a| a.get_single_cmd().unwrap().ioa)
        .collect();
    assert_eq!(received, (0..N).map(|i| 7000 + i).collect::<Vec<_>>());

    cli.close();
    srv.close();
}

#[tokio::test]
async fn clock_synchronisation_is_confirmed() {
    let (srv, cli, _, client_log) = pair(TransmissionMode::Unbalanced).await;

    cli.clock_synchronization_cmd(
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        chrono::Utc::now(),
    )
    .await
    .unwrap();

    eventually("the clock sync is confirmed", async || {
        client_log
            .asdus
            .lock()
            .await
            .iter()
            .any(|a| a.type_id() == TypeId::C_CS_NA_1 && a.coa().cause == Cause::ACTIVATION_CON)
    })
    .await;

    cli.close();
    srv.close();
}

#[tokio::test]
async fn balanced_mode_lets_the_outstation_transmit_spontaneously() {
    let (srv, cli, _, client_log) = pair(TransmissionMode::Balanced).await;

    // No polling happens in balanced mode: the secondary's own primary role
    // pushes this out as confirmed user data.
    srv.send_single_cp56time2a(
        CauseOfTransmission::new(Cause::SPONTANEOUS),
        1,
        &[SinglePointInfo {
            ioa: 900,
            value: true,
            qds: QualityDescriptor::GOOD,
            time: Some(chrono::Utc::now()),
            time_flags: TimeTagFlags::GOOD,
        }],
    )
    .await
    .unwrap();

    eventually("the spontaneous event reaches the master", async || {
        client_log.count_of(TypeId::M_SP_TB_1).await > 0
    })
    .await;

    let a = client_log.first_of(TypeId::M_SP_TB_1).await.unwrap();
    let p = a.get_single_point().unwrap();
    assert_eq!(p[0].ioa, 900);
    assert!(p[0].time.is_some());

    cli.close();
    srv.close();
}

#[tokio::test]
async fn a_send_on_a_closed_link_is_refused() {
    let srv = Server::new(Outstation {
        log: Arc::new(Log::default()),
    });
    assert_eq!(
        srv.send_single(
            false,
            CauseOfTransmission::new(Cause::SPONTANEOUS),
            1,
            &[SinglePointInfo::new(1, true)],
        )
        .await,
        Err(rs_iec60870_5::Error::UseClosedConnection)
    );
}

#[tokio::test]
async fn a_full_buffer_reports_send_queue_full() {
    let mut cfg = brisk(TransmissionMode::Unbalanced);
    cfg.transport = TransportType::TcpServer;
    cfg.tcp = TcpConfig {
        address: "127.0.0.1:0".into(),
        ..Default::default()
    };
    cfg.max_send_queue_size = 2;

    let srv = Server::new(Outstation {
        log: Arc::new(Log::default()),
    })
    .with_config(cfg)
    .unwrap();
    srv.bind().await.unwrap();
    let addr = srv.listen_addr().expect("bound");
    srv.start().unwrap();

    // The transport only counts as open once a peer is on the line; a bare
    // socket is enough, since nothing needs to be exchanged here.
    let _peer = tokio::net::TcpStream::connect(addr).await.unwrap();
    eventually("the transport opens", async || srv.is_connected()).await;

    let coa = CauseOfTransmission::new(Cause::SPONTANEOUS);
    for i in 0..2 {
        srv.send_single(false, coa, 1, &[SinglePointInfo::new(i, true)])
            .await
            .unwrap();
    }
    assert_eq!(
        srv.send_single(false, coa, 1, &[SinglePointInfo::new(9, true)])
            .await,
        Err(rs_iec60870_5::Error::SendQueueFull)
    );
    assert_eq!(srv.buffered(), (2, 0));

    srv.close();
}

#[tokio::test]
async fn single_character_acknowledgements_carry_a_whole_interrogation() {
    // With E5 on, most confirmations and every empty poll are a single octet
    // with no address or control field; the link must still initialise and
    // move a full interrogation, including the ACD-driven class 1 fetches.
    for mode in [TransmissionMode::Unbalanced, TransmissionMode::Balanced] {
        let (srv, cli, server_log, client_log) =
            pair_with(mode, |c| c.use_single_char_ack = true).await;

        cli.interrogation_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION),
            1,
            QualifierOfInterrogation::STATION,
        )
        .await
        .unwrap();

        eventually("the interrogation terminates", async || {
            client_log.asdus.lock().await.iter().any(|a| {
                a.type_id() == TypeId::C_IC_NA_1 && a.coa().cause == Cause::ACTIVATION_TERM
            })
        })
        .await;
        assert_eq!(server_log.interrogations.load(Ordering::SeqCst), 1, "{mode:?}");
        assert!(
            client_log.first_of(TypeId::M_ME_NC_1).await.is_some(),
            "{mode:?}: the process image did not arrive"
        );

        cli.close();
        srv.close();
    }
}
