// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! IEC 60870-5-101 interoperability tests against `github.com/riclolsen/go-iecp5`.
//!
//! Both directions are covered — this crate's primary station against the Go
//! secondary, and this crate's secondary against the Go primary — with the
//! FT1.2 frames carried over the TCP encapsulation transport, so the full link
//! procedure runs without serial hardware.
//!
//! The tests skip when the Go toolchain or the go-iecp5 checkout is missing.

#![cfg(feature = "cs101")]

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::sync::Mutex;

use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs101::{
    Client, ClientHandler, ClientOption, Config, Server, ServerHandler, TcpConfig, TransportType,
};

mod common;
use common::{GoPeer, eventually, field_is, go_bins};

/// Link timings matching the Go peers, brisk enough for a test.
fn cfg() -> Config {
    Config {
        link_address: 1,
        link_addr_size: 1,
        timeout_response_t1: Duration::from_secs(2),
        timeout_repeat_t2: Duration::from_secs(1),
        timeout_test_t3: Duration::from_secs(30),
        timeout_send_link_msg: Duration::from_millis(20),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// direction 1: this crate's primary against the Go secondary
// ---------------------------------------------------------------------------

/// Records every ASDU the Rust primary receives.
#[derive(Default)]
struct RecordingMaster {
    asdus: Mutex<Vec<Asdu>>,
}

impl RecordingMaster {
    async fn wait_for(&self, what: &str, pred: impl Fn(&Asdu) -> bool) -> Asdu {
        let deadline = tokio::time::Instant::now() + common::WAIT;
        loop {
            if let Some(a) = self.asdus.lock().await.iter().find(|a| pred(a)).cloned() {
                return a;
            }
            if tokio::time::Instant::now() >= deadline {
                let seen: Vec<String> =
                    self.asdus.lock().await.iter().map(|a| a.to_string()).collect();
                panic!("timed out waiting for {what}; received: {seen:#?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

/// Forwards to a handler the test also holds.
struct SharedMaster(Arc<RecordingMaster>);

#[async_trait::async_trait]
impl ClientHandler for SharedMaster {
    async fn asdu_all(&self, _c: &dyn Connect, pack: &Asdu, _n: usize) -> rs_iec60870_5::Result<()> {
        self.0.asdus.lock().await.push(pack.clone());
        Ok(())
    }
}

#[tokio::test]
async fn rust_primary_interoperates_with_the_go_secondary() {
    let Some(bins) = go_bins() else { return };

    let go = GoPeer::spawn(&bins.join("go101srv"), &[]).await;
    let addr = go.wait_ready().await;

    let mut c = cfg();
    c.transport = TransportType::TcpClient;
    c.tcp = TcpConfig {
        address: addr,
        ..Default::default()
    };

    let handler = Arc::new(RecordingMaster::default());
    let cli = Client::new(
        SharedMaster(Arc::clone(&handler)),
        ClientOption::new().with_config(c).unwrap(),
    );
    cli.start().unwrap();

    tokio::time::timeout(Duration::from_secs(15), cli.wait_link_active())
        .await
        .expect("the link never became active against go-iecp5");

    // -- interrogation ----------------------------------------------------
    cli.interrogation_cmd(
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        QualifierOfInterrogation::STATION,
    )
    .await
    .unwrap();

    go.wait("the Go secondary to log the interrogation", |v| {
        v["event"] == "interrogation" && v["qoi"] == 20
    })
    .await;

    handler
        .wait_for("the interrogation to terminate", |a| {
            a.type_id() == TypeId::C_IC_NA_1 && a.coa().cause == Cause::ACTIVATION_TERM
        })
        .await;

    let sp = handler
        .wait_for("single points", |a| a.type_id() == TypeId::M_SP_NA_1)
        .await;
    let points = sp.get_single_point().unwrap();
    assert_eq!(points.len(), 2);
    assert_eq!((points[0].ioa, points[0].value), (100, true));
    assert_eq!((points[1].ioa, points[1].value), (101, false));
    assert_eq!(points[1].qds, QualityDescriptor::INVALID);
    assert_eq!(sp.coa().cause, Cause::INTERROGATED_BY_STATION);

    let dp = handler
        .wait_for("double points", |a| a.type_id() == TypeId::M_DP_NA_1)
        .await;
    assert_eq!(
        dp.get_double_point().unwrap()[0].value,
        DoublePoint::DeterminedOn
    );

    let nb = handler
        .wait_for("scaled values", |a| a.type_id() == TypeId::M_ME_NB_1)
        .await;
    assert_eq!(nb.get_measured_value_scaled().unwrap()[0].value, -1234);

    let nc = handler
        .wait_for("float values", |a| {
            a.type_id() == TypeId::M_ME_NC_1 && a.coa().cause == Cause::INTERROGATED_BY_STATION
        })
        .await;
    assert_eq!(nc.get_measured_value_float().unwrap()[0].value, 22.5);

    let tagged = handler
        .wait_for("a CP56Time2a single point", |a| {
            a.type_id() == TypeId::M_SP_TB_1
        })
        .await;
    assert_eq!(
        tagged.get_single_point().unwrap()[0]
            .time
            .expect("a valid time tag")
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "2026-08-17T12:34:56.789Z"
    );

    // -- control direction -------------------------------------------------
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
    let ev = go
        .wait("the single command", field_is("single_cmd", "ioa", 6000.into()))
        .await;
    assert_eq!(ev["value"], true);
    assert_eq!(ev["qual"], 1);

    cli.send_setpoint_cmd_float(
        TypeId::C_SE_NC_1,
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        SetpointCommandFloatInfo {
            ioa: 6002,
            value: -12.75,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let ev = go
        .wait(
            "the float setpoint",
            field_is("setpoint_float", "ioa", 6002.into()),
        )
        .await;
    assert_eq!(ev["value"].as_f64().unwrap(), -12.75);

    // The confirmations came back through the class 1 polls.
    handler
        .wait_for("the single command confirmation", |a| {
            a.type_id() == TypeId::C_SC_NA_1 && a.coa().cause == Cause::ACTIVATION_CON
        })
        .await;

    // -- system commands ---------------------------------------------------
    cli.clock_synchronization_cmd(
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    go.wait("the clock synchronization", |v| v["event"] == "clock_sync")
        .await;

    cli.counter_interrogation_cmd(
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        QualifierCountCall {
            request: QccRequest::TOTAL,
            freeze: QccFreeze::READ,
        },
    )
    .await
    .unwrap();
    go.wait("the counter interrogation", |v| {
        v["event"] == "counter_interrogation"
    })
    .await;
    handler
        .wait_for("the counter response", |a| {
            a.type_id() == TypeId::M_IT_NA_1
                && a.coa().cause == Cause::REQUEST_BY_GENERAL_COUNTER
        })
        .await;

    // -- class 2 polling collects the Go peer's cyclic data ----------------
    handler
        .wait_for("periodic measured values", |a| {
            a.type_id() == TypeId::M_ME_NC_1 && a.coa().cause == Cause::PERIODIC
        })
        .await;

    cli.close();
    go.kill().await;
}

// ---------------------------------------------------------------------------
// direction 2: this crate's secondary against the Go primary
// ---------------------------------------------------------------------------

/// A Rust secondary station serving the same process image as `go101srv`.
struct RustOutstation;

#[async_trait::async_trait]
impl ServerHandler for RustOutstation {
    async fn interrogation(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        qoi: QualifierOfInterrogation,
    ) -> rs_iec60870_5::Result<()> {
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
        c.send_double(
            false,
            coa,
            ca,
            &[DoublePointInfo {
                ioa: 200,
                value: DoublePoint::DeterminedOn,
                ..Default::default()
            }],
        )
        .await?;
        c.send_measured_value_scaled(
            false,
            coa,
            ca,
            &[MeasuredValueScaledInfo {
                ioa: 380,
                value: -1234,
                ..Default::default()
            }],
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
        c.send_single_cp56time2a(
            CauseOfTransmission::new(Cause::SPONTANEOUS),
            ca,
            &[SinglePointInfo {
                ioa: 600,
                value: true,
                qds: QualityDescriptor::GOOD,
                time: chrono::DateTime::parse_from_rfc3339("2026-08-17T12:34:56.789Z")
                    .ok()
                    .map(|t| t.to_utc()),
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
        _t: Option<chrono::DateTime<chrono::Utc>>,
    ) -> rs_iec60870_5::Result<()> {
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await
    }

    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
        c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    }
}

#[tokio::test]
async fn go_primary_interoperates_with_the_rust_secondary() {
    let Some(bins) = go_bins() else { return };

    let mut c = cfg();
    c.transport = TransportType::TcpServer;
    c.tcp = TcpConfig {
        address: "127.0.0.1:0".into(),
        ..Default::default()
    };

    let srv = Server::new(RustOutstation).with_config(c).unwrap();
    srv.bind().await.unwrap();
    let addr = srv.listen_addr().expect("bound").to_string();
    srv.start().unwrap();

    let go = GoPeer::spawn(&bins.join("go101cli"), &[&addr]).await;

    go.wait("the Go primary to activate the link", |v| {
        v["event"] == "link_active"
    })
    .await;

    // The secondary buffers its answers; the Go primary collects them by
    // polling, and reports each information object.
    let sp = {
        go.wait("single points", |v| {
            v["event"] == "single_point" && v["type"] == "M_SP_NA_1"
        })
        .await;
        go.all_of("single_point").await
    };
    let untagged: Vec<&Value> = sp.iter().filter(|v| v["type"] == "M_SP_NA_1").collect();
    assert_eq!(untagged.len(), 2);
    assert_eq!(untagged[0]["ioa"], 100);
    assert_eq!(untagged[0]["value"], true);
    assert_eq!(untagged[0]["cause"], 20);
    assert_eq!(untagged[1]["ioa"], 101);
    assert_eq!(untagged[1]["qds"], 0x80, "the IV bit must survive");

    let dp = go.all_of("double_point").await;
    assert_eq!(dp[0]["ioa"], 200);
    assert_eq!(dp[0]["value"], 2);

    let ms = go.all_of("measured_scaled").await;
    assert_eq!(ms[0]["ioa"], 380);
    assert_eq!(ms[0]["value"], -1234);

    let mf = go.all_of("measured_float").await;
    assert!(
        mf.iter()
            .any(|v| v["ioa"] == 400 && v["value"].as_f64() == Some(22.5)),
        "the float measurement must arrive: {mf:#?}"
    );

    let tagged = go
        .wait("a CP56Time2a single point", |v| {
            v["event"] == "single_point" && v["type"] == "M_SP_TB_1"
        })
        .await;
    assert_eq!(tagged["ioa"], 600);
    assert_eq!(
        tagged["time"].as_str().unwrap(),
        "2026-08-17T12:34:56.789Z",
        "CP56Time2a must round-trip through go-iecp5"
    );

    // -- control direction: commands issued by the Go primary --------------
    go.wait("all Go-side commands to be issued", |v| v["event"] == "done")
        .await;

    let confirmations = go.all_of("asdu").await;
    let has = |t: &str, cause: u64| {
        confirmations
            .iter()
            .any(|v| v["type"] == t && v["cause"] == cause)
    };
    assert!(has("C_SC_NA_1", 7), "single command confirmed: {confirmations:#?}");
    assert!(has("C_SC_NA_1", 10), "single command terminated");
    assert!(has("C_SE_NC_1", 7), "float setpoint confirmed");
    assert!(has("C_CS_NA_1", 7), "clock synchronization confirmed");

    // The class buffers drained, so nothing is stuck.
    eventually("the class buffers drain", async || {
        srv.buffered() == (0, 0)
    })
    .await;

    go.kill().await;
    srv.close();
}
