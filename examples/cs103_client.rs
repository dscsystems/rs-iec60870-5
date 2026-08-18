// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! An IEC 60870-5-103 master for protection equipment.
//!
//! The client runs the whole procedure by itself: link initialization, the
//! device's identification message, then — with `auto_init` — a time
//! synchronization and a general interrogation, followed by continuous class 2
//! polling for cyclic measurands with class 1 fetches whenever the device
//! signals it has events waiting.
//!
//! ```sh
//! # over a serial port (needs the `serial` feature)
//! cargo run --features serial --example cs103_client -- /dev/ttyUSB0
//! cargo run --features serial --example cs103_client -- COM4 19200 3
//!
//! # over a terminal server
//! cargo run --example cs103_client -- tcp:10.0.0.9:2400
//!
//! RUST_LOG=rs_iec60870_5=debug cargo run --example cs103_client -- /dev/ttyUSB0
//! ```
//!
//! Unlike 101 and 104, information objects are addressed by function type
//! (FUN) and information number (INF) rather than by an address, and there is
//! no `asdu::Params` for the two ends to agree on.

use std::time::Duration;

use rs_iec60870_5::cs101::{SerialConfig, TcpConfig, TransportType};
use rs_iec60870_5::cs103::{
    Asdu, Cause, Client, ClientHandler, ClientOption, Config, Dco, IdentificationInfo, Link,
    MeasurandsInfo, TimeTaggedInfo, fun, inf,
};

struct Relays;

#[async_trait::async_trait]
impl ClientHandler for Relays {
    /// ASDU 1 and 2. The cause says what one means: an event, a reply to the
    /// general interrogation, or the result of a command.
    async fn time_tagged(
        &self,
        _link: &dyn Link,
        pack: &Asdu,
        info: TimeTaggedInfo,
    ) -> rs_iec60870_5::Result<()> {
        let what = match pack.coa {
            Cause::SPONTANEOUS => "event".to_string(),
            Cause::GI => "interrogation reply".to_string(),
            Cause::COMMAND_ACK_POS => format!("command accepted, RII {}", info.sin),
            Cause::COMMAND_ACK_NEG => format!("command REJECTED, RII {}", info.sin),
            Cause::RESET_CU | Cause::START_RESTART | Cause::POWER_ON => "restart".to_string(),
            other => other.to_string(),
        };
        let time = info
            .time
            .map(|t| format!(" at {}", t.format("%H:%M:%S%.3f")))
            .unwrap_or_default();

        println!(
            "device {:<3} {:<22} FUN={:<3} INF={:<3} {}{}",
            pack.common_addr,
            what,
            info.fun,
            info.inf,
            info.dpi,
            time
        );
        Ok(())
    }

    /// ASDU 3 and 9. A measurand is a fraction of full scale; the rated value
    /// is 1/1.2 or 1/2.4 of it, depending on how the device is parameterised.
    async fn measurands(
        &self,
        _link: &dyn Link,
        pack: &Asdu,
        info: MeasurandsInfo,
    ) -> rs_iec60870_5::Result<()> {
        let values: Vec<String> = info
            .values
            .iter()
            .map(|m| {
                let mut s = format!("{:.4}", m.f64());
                if m.overflow {
                    s.push_str(" (overflow)");
                }
                if m.invalid {
                    s.push_str(" (invalid)");
                }
                s
            })
            .collect();
        println!(
            "device {:<3} measurands            FUN={:<3} INF={:<3} [{}]",
            pack.common_addr,
            info.fun,
            info.inf,
            values.join(", ")
        );
        Ok(())
    }

    /// ASDU 5, which a device reports after a reset.
    async fn identification(
        &self,
        _link: &dyn Link,
        pack: &Asdu,
        info: IdentificationInfo,
    ) -> rs_iec60870_5::Result<()> {
        println!(
            "device {:<3} identified as {:?}, compatibility level {}",
            pack.common_addr, info.ascii, info.col
        );
        Ok(())
    }

    /// ASDU 8, closing a general interrogation.
    async fn gi_termination(
        &self,
        _link: &dyn Link,
        pack: &Asdu,
        scn: u8,
    ) -> rs_iec60870_5::Result<()> {
        println!(
            "device {:<3} general interrogation {scn} complete",
            pack.common_addr
        );
        Ok(())
    }

    /// ASDU 4, 6, the generic services and disturbance data.
    async fn asdu(&self, _link: &dyn Link, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        println!("device {:<3} {}", pack.common_addr, pack);
        Ok(())
    }

    async fn on_device_active(&self, _link: &dyn Link, addr: u8) {
        println!("device {addr} link active; time sync and interrogation queued");
    }

    async fn on_connection_lost(&self, _link: &dyn Link) {
        println!("the line went down");
    }
}

/// Build the configuration from the command line, returning it with the
/// device address to command.
fn config_from_args() -> (Config, u8) {
    let mut args = std::env::args().skip(1);
    let endpoint = args.next().unwrap_or_else(|| "tcp:127.0.0.1:2400".into());

    let mut cfg = Config {
        // Automatic time sync and general interrogation on every device that
        // comes up. On by default.
        auto_init: true,
        timeout_send_link_msg: Duration::from_millis(100),
        ..Default::default()
    };

    let baud_or_addr = args.next();
    match endpoint.strip_prefix("tcp:") {
        Some(address) => {
            cfg.transport = TransportType::TcpClient;
            cfg.tcp = TcpConfig {
                address: address.to_string(),
                ..Default::default()
            };
            cfg.link_address = baud_or_addr.and_then(|a| a.parse().ok()).unwrap_or(1);
            println!(
                "IEC 60870-5-103 master over TCP to {address}, device {}",
                cfg.link_address
            );
        }
        None => {
            let baud = baud_or_addr.and_then(|b| b.parse().ok()).unwrap_or(9600);
            cfg.transport = TransportType::Serial;
            cfg.serial = SerialConfig::new(&endpoint, baud);
            cfg.link_address = args.next().and_then(|a| a.parse().ok()).unwrap_or(1);
            println!(
                "IEC 60870-5-103 master on {endpoint} at {baud} baud, 8E1, device {}",
                cfg.link_address
            );
        }
    }
    let device = cfg.link_address;
    (cfg, device)
}

#[tokio::main]
async fn main() -> rs_iec60870_5::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "rs_iec60870_5=info".into()),
        )
        .init();

    let (cfg, device) = config_from_args();

    let option = ClientOption::new()
        .with_config(cfg)?
        .with_reconnect_interval(Duration::from_secs(5));
    // On a multi-drop line, add every relay instead:
    //     .with_secondary_address(3).with_secondary_address(4)

    let client = Client::new(Relays, option);
    client.start()?;

    println!("waiting for the link to initialize...");
    tokio::time::timeout(Duration::from_secs(30), client.wait_link_active())
        .await
        .map_err(|_| rs_iec60870_5::Error::LinkNotActive)?;

    // Let the automatic time sync and interrogation finish before commanding.
    tokio::time::sleep(Duration::from_secs(2)).await;

    // A general command. The acknowledgement returns as an ASDU 1 with cause
    // 20 or 21, carrying this RII in its supplementary information — which is
    // how a reply is matched to the command that caused it.
    println!("\n-> general command: reset the LEDs (RII 42)");
    client.general_command(
        device,
        fun::OVERCURRENT_PROTECTION,
        inf::LED_RESET,
        Dco::On,
        42,
    )?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // An explicit interrogation, with a scan number to match its termination.
    println!("\n-> general interrogation, scan number 7");
    client.general_interrogation(device, 7)?;

    println!("\npolling; press Ctrl-C to stop\n");
    tokio::signal::ctrl_c()
        .await
        .map_err(rs_iec60870_5::Error::from)?;
    client.close();
    Ok(())
}
