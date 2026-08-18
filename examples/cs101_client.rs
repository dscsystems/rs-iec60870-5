// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! An IEC 60870-5-101 primary station (master).
//!
//! The link procedure runs by itself — request status of link, reset of remote
//! link, then continuous class 2 polling with a class 1 request whenever a
//! response sets the ACD bit. Commands are queued and interleaved with polling.
//!
//! ```sh
//! # over a serial port (needs the `serial` feature)
//! cargo run --features serial --example cs101_client -- /dev/ttyUSB0
//! cargo run --features serial --example cs101_client -- COM3 19200
//!
//! # over a terminal server, or against the cs101_server example
//! cargo run --example cs101_client -- tcp:127.0.0.1:2404
//!
//! RUST_LOG=rs_iec60870_5=debug cargo run --example cs101_client -- tcp:127.0.0.1:2404
//! ```

use std::time::Duration;

use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs101::{
    Client, ClientHandler, ClientOption, Config, SerialConfig, TcpConfig, TransportType,
};

/// The link address of the secondary station, and the ASDU common address.
///
/// These are separate concepts the standard keeps apart; in the field they are
/// very often configured equal, as here.
const LINK_ADDR: u16 = 1;
const COMMON_ADDR: CommonAddr = 1;

struct Master;

#[async_trait::async_trait]
impl ClientHandler for Master {
    /// Interrogation-caused data, and the mirrored `C_IC_NA_1` confirmations.
    async fn interrogation(&self, _c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        if pack.type_id() == TypeId::C_IC_NA_1 {
            println!("interrogation {}", pack.coa().cause);
        } else {
            print_data("interrogation", pack)?;
        }
        Ok(())
    }

    /// Counter-request-caused `M_IT_*`, and the `C_CI_NA_1` confirmations.
    async fn counter_interrogation(&self, _c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        if pack.type_id() == TypeId::C_CI_NA_1 {
            println!("counter interrogation {}", pack.coa().cause);
        } else {
            print_data("counter", pack)?;
        }
        Ok(())
    }

    async fn clock_sync(&self, _c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        println!("clock synchronization {}", pack.coa().cause);
        Ok(())
    }

    /// Spontaneous data, command confirmations and end of initialization.
    async fn asdu(&self, _c: &dyn Connect, pack: &Asdu, _n: usize) -> rs_iec60870_5::Result<()> {
        print_data("spontaneous", pack)
    }

    async fn on_connect(&self, _c: &dyn Connect) {
        println!("the line is alive (first frame received)");
    }

    async fn on_link_active(&self, _c: &dyn Connect, link_addr: u16) {
        println!("link to station {link_addr} is active");
    }

    async fn on_connection_lost(&self, _c: &dyn Connect) {
        println!("the line went down");
    }
}

/// Decode and print whatever an ASDU carries.
fn print_data(via: &str, pack: &Asdu) -> rs_iec60870_5::Result<()> {
    match pack.type_id() {
        TypeId::M_SP_NA_1 | TypeId::M_SP_TA_1 | TypeId::M_SP_TB_1 => {
            for p in pack.get_single_point()? {
                println!("  [{via}] single  ioa={:<5} value={}", p.ioa, p.value);
            }
        }
        TypeId::M_DP_NA_1 | TypeId::M_DP_TA_1 | TypeId::M_DP_TB_1 => {
            for p in pack.get_double_point()? {
                println!("  [{via}] double  ioa={:<5} value={}", p.ioa, p.value);
            }
        }
        TypeId::M_ME_NB_1 | TypeId::M_ME_TB_1 | TypeId::M_ME_TE_1 => {
            for m in pack.get_measured_value_scaled()? {
                println!("  [{via}] scaled  ioa={:<5} value={}", m.ioa, m.value);
            }
        }
        TypeId::M_ME_NC_1 | TypeId::M_ME_TC_1 | TypeId::M_ME_TF_1 => {
            for m in pack.get_measured_value_float()? {
                println!("  [{via}] float   ioa={:<5} value={}", m.ioa, m.value);
            }
        }
        TypeId::M_IT_NA_1 | TypeId::M_IT_TA_1 | TypeId::M_IT_TB_1 => {
            for c in pack.get_integrated_totals()? {
                println!(
                    "  [{via}] counter ioa={:<5} value={}",
                    c.ioa, c.value.counter_reading
                );
            }
        }
        _ => println!("  [{via}] {pack}"),
    }
    Ok(())
}

/// Build the link configuration from the command line.
///
/// `tcp:host:port` selects the TCP encapsulation transport; anything else is
/// taken as a serial port name, with an optional baud rate after it.
fn config_from_args() -> Config {
    let mut args = std::env::args().skip(1);
    let endpoint = args.next().unwrap_or_else(|| "tcp:127.0.0.1:2404".into());

    let mut cfg = Config {
        link_address: LINK_ADDR,
        link_addr_size: 1,
        // The polling period: every tick the primary transmits queued data or
        // polls the next station. Lower it for latency, raise it for less
        // traffic on the line.
        timeout_send_link_msg: Duration::from_millis(100),
        ..Default::default()
    };

    match endpoint.strip_prefix("tcp:") {
        Some(address) => {
            cfg.transport = TransportType::TcpClient;
            cfg.tcp = TcpConfig {
                address: address.to_string(),
                ..Default::default()
            };
            println!("IEC 60870-5-101 master over TCP to {address}");
        }
        None => {
            let baud = args
                .next()
                .and_then(|b| b.parse().ok())
                .unwrap_or(9600);
            cfg.transport = TransportType::Serial;
            // 8E1 is the framing the standard specifies, and the default.
            cfg.serial = SerialConfig::new(&endpoint, baud);
            println!("IEC 60870-5-101 master on {endpoint} at {baud} baud, 8E1");
        }
    }
    cfg
}

#[tokio::main]
async fn main() -> rs_iec60870_5::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rs_iec60870_5=info".into()),
        )
        .init();

    let option = ClientOption::new()
        .with_config(config_from_args())?
        .with_reconnect_interval(Duration::from_secs(5));
    // On a multi-drop line, add every station instead:
    //     .with_secondary_address(1).with_secondary_address(2)

    let client = Client::new(Master, option);
    client.start()?;

    println!("waiting for the link to initialize...");
    tokio::time::timeout(Duration::from_secs(30), client.wait_link_active())
        .await
        .map_err(|_| rs_iec60870_5::Error::LinkNotActive)?;

    println!("\n-> station interrogation");
    client
        .interrogation_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION),
            COMMON_ADDR,
            QualifierOfInterrogation::STATION,
        )
        .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    println!("\n-> clock synchronization");
    client
        .clock_synchronization_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION),
            COMMON_ADDR,
            chrono::Utc::now(),
        )
        .await?;
    tokio::time::sleep(Duration::from_secs(1)).await;

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
            },
        )
        .await?;

    println!("\npolling; press Ctrl-C to stop\n");
    tokio::signal::ctrl_c()
        .await
        .map_err(rs_iec60870_5::Error::from)?;
    client.close();
    Ok(())
}
