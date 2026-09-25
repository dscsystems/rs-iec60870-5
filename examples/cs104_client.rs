// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! An IEC 60870-5-104 master: connect, interrogate, print everything that
//! arrives, and issue a command.
//!
//! ```sh
//! cargo run --example cs104_client                      # 127.0.0.1:2404
//! cargo run --example cs104_client -- 10.0.0.5:2404
//! RUST_LOG=rs_iec60870_5=debug cargo run --example cs104_client
//! ```
//!
//! Pair it with `cargo run --example cs104_server`.

use std::time::Duration;

use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{Client, ClientContext, ClientHandler, ClientOption};

/// The station address of the outstation being polled.
const COMMON_ADDR: CommonAddr = 1;

struct Master;

#[async_trait::async_trait]
impl ClientHandler for Master {
    /// Process data lands here: interrogation responses *and* spontaneous
    /// events. The dedicated methods below see only command confirmations.
    async fn asdu(&self, _c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        match pack.type_id() {
            TypeId::M_SP_NA_1 | TypeId::M_SP_TA_1 | TypeId::M_SP_TB_1 => {
                for p in pack.get_single_point()? {
                    println!(
                        "  single  ioa={:<5} value={:<5} quality={}{}",
                        p.ioa,
                        p.value,
                        p.qds,
                        stamp(p.time)
                    );
                }
            }
            TypeId::M_DP_NA_1 | TypeId::M_DP_TA_1 | TypeId::M_DP_TB_1 => {
                for p in pack.get_double_point()? {
                    println!(
                        "  double  ioa={:<5} value={:<5} quality={}{}",
                        p.ioa,
                        p.value,
                        p.qds,
                        stamp(p.time)
                    );
                }
            }
            TypeId::M_ME_NA_1 | TypeId::M_ME_TA_1 | TypeId::M_ME_TD_1 | TypeId::M_ME_ND_1 => {
                for m in pack.get_measured_value_normal()? {
                    println!(
                        "  normal  ioa={:<5} value={:<8.5} quality={}{}",
                        m.ioa,
                        m.value.f64(),
                        m.qds,
                        stamp(m.time)
                    );
                }
            }
            TypeId::M_ME_NB_1 | TypeId::M_ME_TB_1 | TypeId::M_ME_TE_1 => {
                for m in pack.get_measured_value_scaled()? {
                    println!(
                        "  scaled  ioa={:<5} value={:<8} quality={}{}",
                        m.ioa,
                        m.value,
                        m.qds,
                        stamp(m.time)
                    );
                }
            }
            TypeId::M_ME_NC_1 | TypeId::M_ME_TC_1 | TypeId::M_ME_TF_1 => {
                for m in pack.get_measured_value_float()? {
                    println!(
                        "  float   ioa={:<5} value={:<8} quality={}{}",
                        m.ioa,
                        m.value,
                        m.qds,
                        stamp(m.time)
                    );
                }
            }
            TypeId::M_IT_NA_1 | TypeId::M_IT_TA_1 | TypeId::M_IT_TB_1 => {
                for c in pack.get_integrated_totals()? {
                    println!(
                        "  counter ioa={:<5} value={:<8} seq={}{}",
                        c.ioa,
                        c.value.counter_reading,
                        c.value.seq_number,
                        stamp(c.time)
                    );
                }
            }
            TypeId::M_EI_NA_1 => {
                let (ioa, coi) = pack.get_end_of_initialization()?;
                println!("  end of initialization: ioa={ioa} cause={}", coi.cause.0);
            }
            // Control commands are echoed back as confirmations. Only the
            // system commands get dedicated handler methods; the rest arrive
            // here, mirrored, with ActivationCon and then ActivationTerm.
            TypeId::C_SC_NA_1 | TypeId::C_SC_TA_1 => {
                let cmd = pack.get_single_cmd()?;
                println!(
                    "  single command ioa={} value={} -> {}",
                    cmd.ioa,
                    cmd.value,
                    describe(pack)
                );
            }
            TypeId::C_SE_NC_1 | TypeId::C_SE_TC_1 => {
                let cmd = pack.get_setpoint_float_cmd()?;
                println!(
                    "  float setpoint ioa={} value={} -> {}",
                    cmd.ioa,
                    cmd.value,
                    describe(pack)
                );
            }
            // Anything else still has a readable dump.
            _ => println!("  {pack}"),
        }
        Ok(())
    }

    /// The mirrored `C_IC_NA_1` confirmations, not the data they carry.
    async fn interrogation(&self, _c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        println!("interrogation {}", describe(pack));
        Ok(())
    }

    async fn counter_interrogation(&self, _c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        println!("counter interrogation {}", describe(pack));
        Ok(())
    }

    async fn clock_sync(&self, _c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        println!("clock synchronization {}", describe(pack));
        Ok(())
    }

    async fn asdu_all(
        &self,
        _c: &dyn Connect,
        pack: &Asdu,
        _ctx: &ClientContext,
    ) -> rs_iec60870_5::Result<()> {
        // Every inbound ASDU passes here first, before routing.
        tracing::debug!(asdu = %pack, "received");
        Ok(())
    }

    async fn on_connect(&self, c: &dyn Connect) {
        println!("connected to {:?}", c.peer_addr());
    }

    async fn on_activated(&self, _c: &dyn Connect) {
        println!("data transfer active (StartDT confirmed)");
    }

    async fn on_connection_lost(&self, _c: &dyn Connect) {
        println!("connection lost");
    }
}

/// Render an optional time tag, or nothing when the type carries none.
fn stamp(t: Option<chrono::DateTime<chrono::Utc>>) -> String {
    match t {
        Some(t) => format!(" at {}", t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        None => String::new(),
    }
}

/// Describe a command confirmation: the cause, and whether it was rejected.
fn describe(pack: &Asdu) -> String {
    let coa = pack.coa();
    if coa.is_negative {
        format!("{} (rejected)", coa.cause)
    } else {
        coa.cause.to_string()
    }
}

#[tokio::main]
async fn main() -> rs_iec60870_5::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rs_iec60870_5=info".into()),
        )
        .init();

    let server = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:2404".to_string());

    // `with_auto_start_dt` is on by default, so the connection activates
    // itself; a 104 link that stays in STOPDT carries no data at all.
    let option = ClientOption::new()
        .with_server(&server)?
        .with_reconnect_interval(Duration::from_secs(5));

    let client = Client::new(Master, option);
    client.start()?;

    println!("connecting to {server}...");
    tokio::time::timeout(Duration::from_secs(30), client.wait_active())
        .await
        .map_err(|_| rs_iec60870_5::Error::NotActive)?;

    // Read the whole process image.
    println!("\n-> station interrogation");
    client
        .interrogation_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION),
            COMMON_ADDR,
            QualifierOfInterrogation::STATION,
        )
        .await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Read the counters.
    println!("\n-> counter interrogation");
    client
        .counter_interrogation_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION),
            COMMON_ADDR,
            QualifierCountCall {
                request: QccRequest::TOTAL,
                freeze: QccFreeze::READ,
            },
        )
        .await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Set the outstation's clock.
    println!("\n-> clock synchronization");
    client
        .clock_synchronization_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION),
            COMMON_ADDR,
            chrono::Utc::now(),
        )
        .await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Operate a switch. `in_select: false` executes directly; set it to true
    // first for select-before-execute.
    println!("\n-> single command: close point 101");
    client
        .send_single_cmd(
            TypeId::C_SC_NA_1,
            CauseOfTransmission::new(Cause::ACTIVATION),
            COMMON_ADDR,
            SingleCommandInfo {
                ioa: 101,
                value: true,
                qoc: QualifierOfCommand {
                    qual: QocQual::SHORT_PULSE_DURATION,
                    in_select: false,
                },
                time: None,
                time_flags: TimeTagFlags::GOOD,
            },
        )
        .await?;

    println!("\nwatching for spontaneous data; press Ctrl-C to stop\n");
    tokio::signal::ctrl_c()
        .await
        .map_err(rs_iec60870_5::Error::from)?;
    client.close();
    Ok(())
}
