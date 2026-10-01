// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Interoperability tests against `github.com/riclolsen/go-iecp5`.
//!
//! Two directions are covered:
//!
//! * this crate's [`Client`] against the Go controlled station (`gosrv`), and
//! * this crate's [`Server`] against the Go master (`gocli`).
//!
//! The Go peers report what they send and receive as JSON lines, so a failure
//! points at the exact information object that disagreed.
//!
//! The harness needs the Go toolchain and a checkout of go-iecp5 at
//! `tests/interop/go-iecp5` (see `tests/interop/README.md`). When either is
//! missing the tests skip rather than fail, so `cargo test` still works on a
//! machine without Go.

#![cfg(feature = "cs104")]

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{Client, ClientHandler, ClientOption, Server, ServerHandler};

mod common;
use common::{GoPeer, field_is, go_bins};

// ---------------------------------------------------------------------------
// direction 1: this crate's master against the Go controlled station
// ---------------------------------------------------------------------------

/// Records every ASDU the Rust master receives.
#[derive(Default)]
struct RecordingMaster {
    asdus: Mutex<Vec<Asdu>>,
}

#[async_trait::async_trait]
impl ClientHandler for RecordingMaster {
    async fn asdu_all(
        &self,
        _c: &dyn Connect,
        pack: &Asdu,
        _ctx: &rs_iec60870_5::cs104::ClientContext,
    ) -> rs_iec60870_5::Result<()> {
        self.asdus.lock().await.push(pack.clone());
        Ok(())
    }
}

impl RecordingMaster {
    async fn wait_for(&self, what: &str, pred: impl Fn(&Asdu) -> bool) -> Asdu {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(a) = self.asdus.lock().await.iter().find(|a| pred(a)).cloned() {
                return a;
            }
            if tokio::time::Instant::now() >= deadline {
                let seen: Vec<String> = self
                    .asdus
                    .lock()
                    .await
                    .iter()
                    .map(|a| a.to_string())
                    .collect();
                panic!("timed out waiting for {what}; received: {seen:#?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

#[tokio::test]
async fn rust_master_interoperates_with_the_go_controlled_station() {
    let Some(bins) = go_bins() else { return };

    let go = GoPeer::spawn(&bins.join("gosrv"), &[]).await;
    let addr = go.wait_ready().await;

    let handler = Arc::new(RecordingMaster::default());
    let cli = Client::new(
        SharedMaster(Arc::clone(&handler)),
        ClientOption::new().with_server(&addr).unwrap(),
    );
    cli.start().unwrap();
    tokio::time::timeout(Duration::from_secs(15), cli.wait_active())
        .await
        .expect("the Rust master did not activate against go-iecp5");

    // -- interrogation ---------------------------------------------------
    cli.interrogation_cmd(
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        QualifierOfInterrogation::STATION,
    )
    .await
    .unwrap();

    go.wait("the Go server to log the interrogation", |v| {
        v["event"] == "interrogation" && v["qoi"] == 20
    })
    .await;

    handler
        .wait_for("the interrogation to terminate", |a| {
            a.type_id() == TypeId::C_IC_NA_1 && a.coa().cause == Cause::ACTIVATION_TERM
        })
        .await;

    // Every monitor-direction family go-iecp5 sent must decode identically.
    let sp = handler
        .wait_for("single points", |a| a.type_id() == TypeId::M_SP_NA_1)
        .await;
    let points = sp.get_single_point().unwrap();
    assert_eq!(points.len(), 2);
    assert_eq!(points[0].ioa, 100);
    assert!(points[0].value);
    assert_eq!(points[0].qds, QualityDescriptor::GOOD);
    assert_eq!(points[1].ioa, 101);
    assert!(!points[1].value);
    assert_eq!(points[1].qds, QualityDescriptor::INVALID);
    assert_eq!(sp.coa().cause, Cause::INTERROGATED_BY_STATION);

    let dp = handler
        .wait_for("double points", |a| a.type_id() == TypeId::M_DP_NA_1)
        .await;
    let d = dp.get_double_point().unwrap();
    assert_eq!(d[0].ioa, 200);
    assert_eq!(d[0].value, DoublePoint::DeterminedOn);

    let st = handler
        .wait_for("step positions", |a| a.type_id() == TypeId::M_ST_NA_1)
        .await;
    let s = st.get_step_position().unwrap();
    assert_eq!(s[0].ioa, 250);
    assert_eq!(s[0].value.val, -17, "the 7 bit signed field must sign-extend");
    assert!(s[0].value.has_transient);

    let bo = handler
        .wait_for("bit strings", |a| a.type_id() == TypeId::M_BO_NA_1)
        .await;
    assert_eq!(bo.get_bitstring32().unwrap()[0].value, 0xdead_beef);

    let na = handler
        .wait_for("normalized values", |a| a.type_id() == TypeId::M_ME_NA_1)
        .await;
    let n = na.get_measured_value_normal().unwrap();
    assert_eq!(n[0].ioa, 350);
    assert_eq!(n[0].value, Normalize(16384));
    assert_eq!(n[0].value.f64(), 0.5);

    let nb = handler
        .wait_for("scaled values", |a| a.type_id() == TypeId::M_ME_NB_1)
        .await;
    assert_eq!(nb.get_measured_value_scaled().unwrap()[0].value, -1234);

    // Two float ASDUs arrive: the addressed pair, then a sequence.
    let floats: Vec<Asdu> = {
        handler
            .wait_for("the float sequence", |a| {
                a.type_id() == TypeId::M_ME_NC_1 && a.variable().is_sequence
            })
            .await;
        handler
            .asdus
            .lock()
            .await
            .iter()
            .filter(|a| a.type_id() == TypeId::M_ME_NC_1)
            .cloned()
            .collect()
    };
    let addressed = floats.iter().find(|a| !a.variable().is_sequence).unwrap();
    let f = addressed.get_measured_value_float().unwrap();
    assert_eq!((f[0].ioa, f[0].value), (400, 22.5));
    assert_eq!((f[1].ioa, f[1].value), (401, -1.25));
    assert_eq!(f[1].qds, QualityDescriptor::OVERFLOW);

    let sequence = floats.iter().find(|a| a.variable().is_sequence).unwrap();
    let f = sequence.get_measured_value_float().unwrap();
    assert_eq!(
        f.iter().map(|i| (i.ioa, i.value)).collect::<Vec<_>>(),
        vec![(450, 1.0), (451, 2.0), (452, 3.0)],
        "a sequence carries one address and implies the rest"
    );

    let it = handler
        .wait_for("integrated totals", |a| a.type_id() == TypeId::M_IT_NA_1)
        .await;
    let c = it.get_integrated_totals().unwrap();
    assert_eq!(c[0].ioa, 500);
    assert_eq!(c[0].value.counter_reading, 4242);
    assert_eq!(c[0].value.seq_number, 1);
    assert!(c[0].value.has_carry);

    // CP56Time2a, encoded by go-iecp5 and decoded here.
    let tagged = handler
        .wait_for("a CP56Time2a single point", |a| {
            a.type_id() == TypeId::M_SP_TB_1
        })
        .await;
    let t = tagged.get_single_point().unwrap()[0]
        .time
        .expect("a valid time tag");
    assert_eq!(
        t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "2026-08-17T12:34:56.789Z"
    );

    // -- commands in the control direction --------------------------------
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
    assert_eq!(ev["select"], false);

    cli.send_double_cmd(
        TypeId::C_DC_NA_1,
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        DoubleCommandInfo {
            ioa: 6001,
            value: DoubleCommand::On,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let ev = go
        .wait("the double command", field_is("double_cmd", "ioa", 6001.into()))
        .await;
    assert_eq!(ev["value"], 2);

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

    cli.send_bits_string32_cmd(
        TypeId::C_BO_NA_1,
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        BitsString32CommandInfo {
            ioa: 6003,
            value: 0x0f0f_0f0f,
            time: None,
            time_flags: TimeTagFlags::GOOD,
        },
    )
    .await
    .unwrap();
    let ev = go
        .wait(
            "the bit string command",
            field_is("bitstring_cmd", "ioa", 6003.into()),
        )
        .await;
    assert_eq!(ev["value"], 0x0f0f_0f0fu32);

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
    let ev = go
        .wait("the counter interrogation", |v| {
            v["event"] == "counter_interrogation"
        })
        .await;
    assert_eq!(ev["request"], 5);
    handler
        .wait_for("the counter response", |a| {
            a.type_id() == TypeId::M_IT_NA_1
                && a.coa().cause == Cause::REQUEST_BY_GENERAL_COUNTER
        })
        .await;

    cli.read_cmd(CauseOfTransmission::new(Cause::REQUEST), 1, 400)
        .await
        .unwrap();
    go.wait("the read command", field_is("read", "ioa", 400.into()))
        .await;

    cli.reset_process_cmd(
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        QualifierOfResetProcessCmd::GENERAL_RESET,
    )
    .await
    .unwrap();
    go.wait("the reset process command", |v| v["event"] == "reset_process")
        .await;

    // -- spontaneous data pushed by the Go server -------------------------
    let spont = handler
        .wait_for("spontaneous measured values", |a| {
            a.type_id() == TypeId::M_ME_TF_1 && a.coa().cause == Cause::SPONTANEOUS
        })
        .await;
    let f = spont.get_measured_value_float().unwrap();
    assert_eq!(f[0].ioa, 700);
    assert_eq!(f[0].value, 3.5);
    assert!(f[0].time.is_some(), "a spontaneous event carries a time tag");

    cli.close();
    go.kill().await;
}

/// Lets the test hold the handler while the client owns it too.
struct SharedMaster(Arc<RecordingMaster>);

#[async_trait::async_trait]
impl ClientHandler for SharedMaster {
    async fn asdu_all(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        ctx: &rs_iec60870_5::cs104::ClientContext,
    ) -> rs_iec60870_5::Result<()> {
        self.0.asdu_all(c, pack, ctx).await
    }
}

// ---------------------------------------------------------------------------
// direction 2: this crate's controlled station against the Go master
// ---------------------------------------------------------------------------

/// A Rust controlled station serving the same process image as `gosrv`.
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
        c.send_step(
            false,
            coa,
            ca,
            &[StepPositionInfo {
                ioa: 250,
                value: StepPosition {
                    val: -17,
                    has_transient: true,
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
                ioa: 300,
                value: 0xdead_beef,
                ..Default::default()
            }],
        )
        .await?;
        c.send_measured_value_normal(
            false,
            coa,
            ca,
            &[MeasuredValueNormalInfo {
                ioa: 350,
                value: Normalize(16384),
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
            &[
                MeasuredValueFloatInfo {
                    ioa: 400,
                    value: 22.5,
                    ..Default::default()
                },
                MeasuredValueFloatInfo {
                    ioa: 401,
                    value: -1.25,
                    qds: QualityDescriptor::OVERFLOW,
                    time: None,
                    time_flags: TimeTagFlags::GOOD,
                },
            ],
        )
        .await?;
        // A sequence, to check that go-iecp5 expands the implied addresses.
        c.send_measured_value_float(
            true,
            coa,
            ca,
            &[
                MeasuredValueFloatInfo {
                    ioa: 450,
                    value: 1.0,
                    ..Default::default()
                },
                MeasuredValueFloatInfo {
                    ioa: 451,
                    value: 2.0,
                    ..Default::default()
                },
                MeasuredValueFloatInfo {
                    ioa: 452,
                    value: 3.0,
                    ..Default::default()
                },
            ],
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
                    has_carry: true,
                    ..Default::default()
                },
                time: None,
                time_flags: TimeTagFlags::GOOD,
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

    async fn counter_interrogation(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        _qcc: QualifierCountCall,
    ) -> rs_iec60870_5::Result<()> {
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
        c.send_integrated_totals(
            false,
            CauseOfTransmission::new(Cause::REQUEST_BY_GENERAL_COUNTER),
            pack.common_addr(),
            &[BinaryCounterReadingInfo {
                ioa: 501,
                value: BinaryCounterReading {
                    counter_reading: 99,
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
        _t: Option<chrono::DateTime<chrono::Utc>>,
    ) -> rs_iec60870_5::Result<()> {
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await
    }

    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        match pack.type_id() {
            TypeId::C_SC_NA_1
            | TypeId::C_DC_NA_1
            | TypeId::C_SE_NC_1
            | TypeId::C_BO_NA_1 => {
                c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
                c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
            }
            _ => Err(rs_iec60870_5::Error::TypeIdentifier),
        }
    }
}

#[tokio::test]
async fn go_master_interoperates_with_the_rust_controlled_station() {
    let Some(bins) = go_bins() else { return };

    let srv = Server::new(RustOutstation);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    {
        let srv = Arc::clone(&srv);
        tokio::spawn(async move { srv.serve(listener).await });
    }

    let go = GoPeer::spawn(&bins.join("gocli"), &[&addr]).await;

    go.wait("the Go master to activate", |v| v["event"] == "activated")
        .await;
    go.wait("the interrogation to be confirmed", |v| {
        v["event"] == "interrogation_reply" && v["cause"] == 7
    })
    .await;
    go.wait("the interrogation to terminate", |v| {
        v["event"] == "interrogation_reply" && v["cause"] == 10
    })
    .await;

    // -- monitor direction, as decoded by go-iecp5 -------------------------
    let sp = go.all_of("single_point").await;
    let untagged: Vec<&Value> = sp.iter().filter(|v| v["type"] == "M_SP_NA_1").collect();
    assert_eq!(untagged.len(), 2, "two single points in the process image");
    assert_eq!(untagged[0]["ioa"], 100);
    assert_eq!(untagged[0]["value"], true);
    assert_eq!(untagged[0]["qds"], 0);
    assert_eq!(untagged[0]["cause"], 20);
    assert_eq!(untagged[1]["ioa"], 101);
    assert_eq!(untagged[1]["value"], false);
    assert_eq!(untagged[1]["qds"], 0x80, "the IV bit must survive");

    let tagged = sp
        .iter()
        .find(|v| v["type"] == "M_SP_TB_1")
        .expect("a CP56Time2a single point");
    assert_eq!(tagged["ioa"], 600);
    assert_eq!(
        tagged["time"].as_str().unwrap(),
        "2026-08-17T12:34:56.789Z",
        "CP56Time2a must round-trip through go-iecp5"
    );

    let dp = go.all_of("double_point").await;
    assert_eq!(dp[0]["ioa"], 200);
    assert_eq!(dp[0]["value"], 2);

    let st = go.all_of("step_position").await;
    assert_eq!(st[0]["ioa"], 250);
    assert_eq!(st[0]["value"], -17);
    assert_eq!(st[0]["transient"], true);

    let bo = go.all_of("bitstring").await;
    assert_eq!(bo[0]["value"], 0xdead_beefu32);

    let mn = go.all_of("measured_normal").await;
    assert_eq!(mn[0]["ioa"], 350);
    assert_eq!(mn[0]["raw"], 16384);
    assert_eq!(mn[0]["value"].as_f64().unwrap(), 0.5);

    let ms = go.all_of("measured_scaled").await;
    assert_eq!(ms[0]["ioa"], 380);
    assert_eq!(ms[0]["value"], -1234);

    let mf = go.all_of("measured_float").await;
    let addressed: Vec<&Value> = mf.iter().filter(|v| v["sq"] == false).collect();
    assert_eq!(addressed[0]["ioa"], 400);
    assert_eq!(addressed[0]["value"].as_f64().unwrap(), 22.5);
    assert_eq!(addressed[1]["ioa"], 401);
    assert_eq!(addressed[1]["value"].as_f64().unwrap(), -1.25);
    assert_eq!(addressed[1]["qds"], 1, "the OV bit must survive");

    let sequence: Vec<&Value> = mf.iter().filter(|v| v["sq"] == true).collect();
    assert_eq!(
        sequence
            .iter()
            .map(|v| (v["ioa"].as_u64().unwrap(), v["value"].as_f64().unwrap()))
            .collect::<Vec<_>>(),
        vec![(450, 1.0), (451, 2.0), (452, 3.0)],
        "go-iecp5 must expand the implied addresses of our sequence"
    );

    let it = go.all_of("integrated_total").await;
    let first = &it[0];
    assert_eq!(first["ioa"], 500);
    assert_eq!(first["count"], 4242);
    assert_eq!(first["seq"], 1);
    assert_eq!(first["carry"], true);

    // -- control direction: commands issued by the Go master ---------------
    go.wait("all Go-side commands to be issued", |v| v["event"] == "done")
        .await;

    // Each command family was confirmed and, where applicable, terminated.
    let confirmations = go.all_of("asdu").await;
    let has = |t: &str, cause: u64| {
        confirmations
            .iter()
            .any(|v| v["type"] == t && v["cause"] == cause)
    };
    assert!(has("C_SC_NA_1", 7), "single command confirmed");
    assert!(has("C_SC_NA_1", 10), "single command terminated");
    assert!(has("C_DC_NA_1", 7), "double command confirmed");
    assert!(has("C_SE_NC_1", 7), "float setpoint confirmed");
    assert!(has("C_BO_NA_1", 7), "bit string command confirmed");

    go.wait("the clock synchronization to be confirmed", |v| {
        v["event"] == "clock_sync_reply" && v["cause"] == 7
    })
    .await;
    go.wait("the counter interrogation to be confirmed", |v| {
        v["event"] == "counter_reply" && v["cause"] == 7
    })
    .await;
    let counters = go.all_of("integrated_total").await;
    assert!(
        counters.iter().any(|v| v["ioa"] == 501 && v["cause"] == 37),
        "the counter response must carry RequestByGeneralCounter"
    );
    go.wait("the test command to be confirmed", |v| {
        v["event"] == "test_reply" && v["cause"] == 7
    })
    .await;

    go.kill().await;
    srv.close();
}
