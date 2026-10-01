// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! End-to-end tests: a [`Client`] driven against a [`Server`] over loopback TCP.
//!
//! These exercise the whole stack — APCI framing, the k/w windows, StartDT
//! activation, request routing and ASDU encoding — with no external peer.

#![cfg(feature = "cs104")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::Mutex;

use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{Client, ClientOption, Server, ServerHandler, ClientHandler, Config};

/// Anything the test wants to assert on, recorded as it arrives.
#[derive(Default)]
struct Log {
    asdus: Mutex<Vec<Asdu>>,
    interrogations: AtomicUsize,
    commands: AtomicUsize,
}

impl Log {
    async fn types(&self) -> Vec<TypeId> {
        self.asdus.lock().await.iter().map(|a| a.type_id()).collect()
    }

    async fn first_of(&self, t: TypeId) -> Option<Asdu> {
        self.asdus
            .lock()
            .await
            .iter()
            .find(|a| a.type_id() == t)
            .cloned()
    }
}

/// A controlled station that answers station interrogation with a small
/// process image and echoes control commands.
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
        c.send_integrated_totals(
            false,
            CauseOfTransmission::new(Cause::SPONTANEOUS),
            ca,
            &[BinaryCounterReadingInfo {
                ioa: 500,
                value: BinaryCounterReading {
                    counter_reading: 4242,
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

    async fn clock_sync(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        _time: Option<chrono::DateTime<chrono::Utc>>,
    ) -> rs_iec60870_5::Result<()> {
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await
    }

    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        self.log.asdus.lock().await.push(pack.clone());
        match pack.type_id() {
            TypeId::C_SC_NA_1 | TypeId::C_SC_TA_1 => {
                self.log.commands.fetch_add(1, Ordering::SeqCst);
                c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
                c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
            }
            TypeId::C_SE_NC_1 => {
                self.log.commands.fetch_add(1, Ordering::SeqCst);
                c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await
            }
            // Anything else is unknown to this outstation.
            _ => Err(rs_iec60870_5::Error::TypeIdentifier),
        }
    }
}

/// A master that records every ASDU it receives.
struct Master {
    log: Arc<Log>,
}

#[async_trait::async_trait]
impl ClientHandler for Master {
    async fn asdu_all(
        &self,
        _c: &dyn Connect,
        pack: &Asdu,
        _ctx: &rs_iec60870_5::cs104::ClientContext,
    ) -> rs_iec60870_5::Result<()> {
        self.log.asdus.lock().await.push(pack.clone());
        Ok(())
    }

    async fn interrogation(&self, _c: &dyn Connect, _pack: &Asdu) -> rs_iec60870_5::Result<()> {
        self.log.interrogations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// Bring up a server on an ephemeral port and an activated client against it.
async fn pair() -> (
    Arc<Server<Outstation>>,
    Arc<Client<Master>>,
    Arc<Log>,
    Arc<Log>,
) {
    let server_log = Arc::new(Log::default());
    let client_log = Arc::new(Log::default());

    let srv = Server::new(Outstation {
        log: Arc::clone(&server_log),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    {
        let srv = Arc::clone(&srv);
        tokio::spawn(async move { srv.serve(listener).await });
    }

    let option = ClientOption::new()
        .with_server(&addr.to_string())
        .unwrap()
        .with_config(Config {
            // Keep the test brisk without leaving the legal ranges.
            send_unack_timeout1: Duration::from_secs(2),
            recv_unack_timeout2: Duration::from_secs(1),
            idle_timeout3: Duration::from_secs(1),
            ..Default::default()
        });
    let cli = Client::new(
        Master {
            log: Arc::clone(&client_log),
        },
        option,
    );
    cli.start().unwrap();

    tokio::time::timeout(Duration::from_secs(5), cli.wait_active())
        .await
        .expect("client did not activate");

    (srv, cli, server_log, client_log)
}

/// Poll until `f` holds, or fail the test.
async fn eventually(label: &str, mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("timed out waiting for: {label}");
}

#[tokio::test]
async fn client_activates_and_the_server_sees_the_session() {
    let (srv, cli, _, _) = pair().await;
    assert!(cli.is_active().await);
    assert!(cli.is_connected().await);
    eventually("server registers the session", async || {
        srv.session_count() == 1
    })
    .await;

    cli.close();
    eventually("server drops the session", async || {
        srv.session_count() == 0
    })
    .await;
    srv.close();
}

#[tokio::test]
async fn interrogation_returns_the_process_image() {
    let (srv, cli, server_log, client_log) = pair().await;

    cli.interrogation_cmd(
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        QualifierOfInterrogation::STATION,
    )
    .await
    .unwrap();

    eventually("interrogation completes", async || {
        client_log
            .asdus
            .lock()
            .await
            .iter()
            .any(|a| a.type_id() == TypeId::C_IC_NA_1 && a.coa().cause == Cause::ACTIVATION_TERM)
    })
    .await;

    assert_eq!(server_log.interrogations.load(Ordering::SeqCst), 1);

    let types = client_log.types().await;
    // Confirmation, the three data ASDUs, then termination.
    assert_eq!(
        types,
        vec![
            TypeId::C_IC_NA_1,
            TypeId::M_SP_NA_1,
            TypeId::M_ME_NC_1,
            TypeId::M_IT_NA_1,
            TypeId::C_IC_NA_1,
        ]
    );

    let sp = client_log.first_of(TypeId::M_SP_NA_1).await.unwrap();
    assert_eq!(sp.coa().cause, Cause::INTERROGATED_BY_STATION);
    let points = sp.get_single_point().unwrap();
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].ioa, 100);
    assert!(points[0].value);
    assert_eq!(points[1].ioa, 101);
    assert!(!points[1].value);
    assert_eq!(points[1].qds, QualityDescriptor::INVALID);

    let me = client_log.first_of(TypeId::M_ME_NC_1).await.unwrap();
    assert_eq!(me.get_measured_value_float().unwrap()[0].value, 22.5);

    let it = client_log.first_of(TypeId::M_IT_NA_1).await.unwrap();
    assert_eq!(
        it.get_integrated_totals().unwrap()[0].value.counter_reading,
        4242
    );

    // The master's dedicated handler saw both C_IC confirmations.
    assert_eq!(client_log.interrogations.load(Ordering::SeqCst), 2);

    cli.close();
    srv.close();
}

#[tokio::test]
async fn a_rejected_interrogation_group_is_confirmed_negatively() {
    let (srv, cli, _, client_log) = pair().await;

    cli.interrogation_cmd(
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        QualifierOfInterrogation::GROUP1,
    )
    .await
    .unwrap();

    eventually("negative confirmation arrives", async || {
        client_log
            .asdus
            .lock()
            .await
            .iter()
            .any(|a| a.type_id() == TypeId::C_IC_NA_1 && a.coa().is_negative)
    })
    .await;

    cli.close();
    srv.close();
}

#[tokio::test]
async fn control_commands_are_confirmed_and_terminated() {
    let (srv, cli, server_log, client_log) = pair().await;

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

    eventually("command terminates", async || {
        client_log
            .asdus
            .lock()
            .await
            .iter()
            .any(|a| a.type_id() == TypeId::C_SC_NA_1 && a.coa().cause == Cause::ACTIVATION_TERM)
    })
    .await;

    assert_eq!(server_log.commands.load(Ordering::SeqCst), 1);
    let received = server_log.first_of(TypeId::C_SC_NA_1).await.unwrap();
    let cmd = received.get_single_cmd().unwrap();
    assert_eq!(cmd.ioa, 6000);
    assert!(cmd.value);
    assert_eq!(cmd.qoc.qual, QocQual::SHORT_PULSE_DURATION);
    assert!(!cmd.qoc.in_select);

    cli.close();
    srv.close();
}

#[tokio::test]
async fn an_unhandled_type_is_answered_with_unknown_type_id() {
    let (srv, cli, _, client_log) = pair().await;

    // The outstation's handler rejects everything but C_SC and C_SE_NC.
    cli.send_double_cmd(
        TypeId::C_DC_NA_1,
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        DoubleCommandInfo {
            ioa: 7000,
            value: DoubleCommand::On,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    eventually("UnknownTypeID mirror arrives", async || {
        client_log
            .asdus
            .lock()
            .await
            .iter()
            .any(|a| a.type_id() == TypeId::C_DC_NA_1 && a.coa().cause == Cause::UNKNOWN_TYPE_ID)
    })
    .await;

    cli.close();
    srv.close();
}

#[tokio::test]
async fn a_bad_cause_of_transmission_is_answered_with_unknown_cot() {
    let (srv, cli, server_log, client_log) = pair().await;

    // C_IC_NA_1 only accepts Activation and Deactivation; hand-build one with
    // Spontaneous to reach the session's pre-validation.
    let mut bad = Asdu::interrogation_cmd(
        cli.params(),
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        QualifierOfInterrogation::STATION,
    )
    .unwrap();
    bad.coa_mut().cause = Cause::SPONTANEOUS;
    cli.send(bad).await.unwrap();

    eventually("UnknownCOT mirror arrives", async || {
        client_log
            .asdus
            .lock()
            .await
            .iter()
            .any(|a| a.type_id() == TypeId::C_IC_NA_1 && a.coa().cause == Cause::UNKNOWN_COT)
    })
    .await;

    // The handler was never reached.
    assert_eq!(server_log.interrogations.load(Ordering::SeqCst), 0);

    cli.close();
    srv.close();
}

#[tokio::test]
async fn the_server_broadcasts_spontaneous_data_to_every_master() {
    let server_log = Arc::new(Log::default());
    let srv = Server::new(Outstation {
        log: Arc::clone(&server_log),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    {
        let srv = Arc::clone(&srv);
        tokio::spawn(async move { srv.serve(listener).await });
    }

    let mut clients = Vec::new();
    let mut logs = Vec::new();
    for _ in 0..3 {
        let log = Arc::new(Log::default());
        let cli = Client::new(
            Master {
                log: Arc::clone(&log),
            },
            ClientOption::new().with_server(&addr).unwrap(),
        );
        cli.start().unwrap();
        tokio::time::timeout(Duration::from_secs(5), cli.wait_active())
            .await
            .expect("client did not activate");
        clients.push(cli);
        logs.push(log);
    }
    eventually("all three sessions register", async || {
        srv.session_count() == 3
    })
    .await;

    srv.send_single_cp56time2a(
        CauseOfTransmission::new(Cause::SPONTANEOUS),
        1,
        &[SinglePointInfo {
            ioa: 100,
            value: true,
            qds: QualityDescriptor::GOOD,
            time: Some(chrono::Utc::now()),
            time_flags: TimeTagFlags::GOOD,
        }],
    )
    .await
    .unwrap();

    for log in &logs {
        let log = Arc::clone(log);
        eventually("every master receives the broadcast", async move || {
            log.asdus
                .lock()
                .await
                .iter()
                .any(|a| a.type_id() == TypeId::M_SP_TB_1)
        })
        .await;
    }

    // The time tag survived the round trip.
    let a = logs[0].first_of(TypeId::M_SP_TB_1).await.unwrap();
    assert!(a.get_single_point().unwrap()[0].time.is_some());

    for c in &clients {
        c.close();
    }
    srv.close();
}

#[tokio::test]
async fn sending_before_activation_is_refused() {
    let srv = Server::new(Outstation {
        log: Arc::new(Log::default()),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    {
        let srv = Arc::clone(&srv);
        tokio::spawn(async move { srv.serve(listener).await });
    }

    // With auto StartDT disabled the link stays in STOPDT.
    let cli = Client::new(
        Master {
            log: Arc::new(Log::default()),
        },
        ClientOption::new()
            .with_server(&addr)
            .unwrap()
            .with_auto_start_dt(false),
    );
    cli.start().unwrap();
    eventually("client connects", async || cli.is_connected().await).await;

    assert_eq!(
        cli.interrogation_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION),
            1,
            QualifierOfInterrogation::STATION,
        )
        .await,
        Err(rs_iec60870_5::Error::NotActive)
    );
    assert!(!cli.is_active().await);

    // Activating by hand makes it work.
    cli.send_start_dt().await;
    tokio::time::timeout(Duration::from_secs(5), cli.wait_active())
        .await
        .expect("client did not activate after manual StartDT");
    assert!(
        cli.interrogation_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION),
            1,
            QualifierOfInterrogation::STATION,
        )
        .await
        .is_ok()
    );

    cli.close();
    srv.close();
}

#[tokio::test]
async fn many_asdus_flow_through_the_k_and_w_windows() {
    // k = 1 forces an acknowledgement round trip per frame, which exercises the
    // window bookkeeping far harder than the defaults.
    let server_log = Arc::new(Log::default());
    let srv = Server::new(Outstation {
        log: Arc::clone(&server_log),
    })
    .with_config(Config {
        send_unack_limit_k: 1,
        recv_unack_limit_w: 1,
        recv_unack_timeout2: Duration::from_secs(1),
        ..Default::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    {
        let srv = Arc::clone(&srv);
        tokio::spawn(async move { srv.serve(listener).await });
    }

    let client_log = Arc::new(Log::default());
    let cli = Client::new(
        Master {
            log: Arc::clone(&client_log),
        },
        ClientOption::new()
            .with_server(&addr)
            .unwrap()
            .with_config(Config {
                send_unack_limit_k: 1,
                recv_unack_limit_w: 1,
                recv_unack_timeout2: Duration::from_secs(1),
                ..Default::default()
            }),
    );
    cli.start().unwrap();
    tokio::time::timeout(Duration::from_secs(5), cli.wait_active())
        .await
        .expect("client did not activate");

    const N: usize = 40;
    for i in 0..N {
        // Retry while the single-frame window is closed.
        loop {
            let r = cli
                .send_single_cmd(
                    TypeId::C_SC_NA_1,
                    CauseOfTransmission::new(Cause::ACTIVATION),
                    1,
                    SingleCommandInfo {
                        ioa: 6000 + i as u32,
                        value: i % 2 == 0,
                        ..Default::default()
                    },
                )
                .await;
            match r {
                Ok(()) => break,
                Err(rs_iec60870_5::Error::BufferFull) => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Err(e) => panic!("send failed: {e}"),
            }
        }
    }

    let log = Arc::clone(&server_log);
    eventually("every command arrives in order", async move || {
        log.commands.load(Ordering::SeqCst) == N
    })
    .await;

    // Sequence integrity: the outstation saw each IOA exactly once, in order.
    let received: Vec<u32> = server_log
        .asdus
        .lock()
        .await
        .iter()
        .map(|a| a.get_single_cmd().unwrap().ioa)
        .collect();
    assert_eq!(received, (0..N as u32).map(|i| 6000 + i).collect::<Vec<_>>());

    cli.close();
    srv.close();
}

#[tokio::test]
async fn the_client_reconnects_after_the_server_restarts() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let srv1 = Server::new(Outstation {
        log: Arc::new(Log::default()),
    });
    {
        let srv1 = Arc::clone(&srv1);
        tokio::spawn(async move { srv1.serve(listener).await });
    }

    let cli = Client::new(
        Master {
            log: Arc::new(Log::default()),
        },
        ClientOption::new()
            .with_server(&addr.to_string())
            .unwrap()
            .with_reconnect_interval(Duration::from_millis(100)),
    );
    cli.start().unwrap();
    tokio::time::timeout(Duration::from_secs(5), cli.wait_active())
        .await
        .expect("first activation");

    // Drop the server; the client must notice and keep retrying.
    srv1.close();
    eventually("client notices the drop", async || {
        !cli.is_active().await
    })
    .await;

    // Bring a new server up on the same port.
    let listener = TcpListener::bind(addr).await.unwrap();
    let srv2 = Server::new(Outstation {
        log: Arc::new(Log::default()),
    });
    {
        let srv2 = Arc::clone(&srv2);
        tokio::spawn(async move { srv2.serve(listener).await });
    }

    tokio::time::timeout(Duration::from_secs(10), cli.wait_active())
        .await
        .expect("client did not reconnect");
    assert!(cli.is_active().await);

    cli.close();
    srv2.close();
}

/// Two masters on one outstation, as a redundancy group: one started, one on
/// standby. Spontaneous data goes to the started one only (IEC 60870-5-104,
/// clause 10), and the standby neither receives it nor makes the broadcast
/// fail once its own queue would have filled.
#[tokio::test]
async fn a_standby_master_receives_no_spontaneous_data() {
    let srv = Server::new(Outstation {
        log: Arc::new(Log::default()),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    {
        let srv = Arc::clone(&srv);
        tokio::spawn(async move { srv.serve(listener).await });
    }

    let connect = |auto_start: bool, log: Arc<Log>| {
        let option = ClientOption::new()
            .with_server(&addr.to_string())
            .unwrap()
            .with_auto_start_dt(auto_start);
        let cli = Client::new(Master { log }, option);
        cli.start().unwrap();
        cli
    };

    let active_log = Arc::new(Log::default());
    let standby_log = Arc::new(Log::default());
    let active = connect(true, Arc::clone(&active_log));
    let standby = connect(false, Arc::clone(&standby_log));

    tokio::time::timeout(Duration::from_secs(5), active.wait_active())
        .await
        .expect("the active master did not start");
    eventually("both masters connected", async || srv.session_count() == 2).await;

    // Far more than one session queue holds: were the standby buffering
    // copies, its queue would overflow and the broadcast would report it.
    let coa = CauseOfTransmission::new(Cause::SPONTANEOUS);
    for i in 0..400u32 {
        let info = SinglePointInfo::new(1000 + i, i % 2 == 0);
        srv.send_single(false, coa, 1, &[info])
            .await
            .unwrap_or_else(|e| panic!("broadcast {i} failed: {e}"));
        if i % 50 == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    eventually("the active master received everything", async || {
        active_log
            .types()
            .await
            .iter()
            .filter(|t| **t == TypeId::M_SP_NA_1)
            .count()
            == 400
    })
    .await;
    assert!(
        standby_log.types().await.is_empty(),
        "the standby master was sent data"
    );

    active.close();
    standby.close();
}

#[tokio::test]
async fn a_broadcast_with_no_started_master_reports_it() {
    let srv = Server::new(Outstation {
        log: Arc::new(Log::default()),
    });
    let coa = CauseOfTransmission::new(Cause::SPONTANEOUS);
    assert_eq!(
        srv.send_single(false, coa, 1, &[SinglePointInfo::new(1, true)])
            .await,
        Err(rs_iec60870_5::Error::NotActive),
        "the data went nowhere, and the caller must be able to tell"
    );
}
