// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! An IEC 60870-5-101 secondary station (outstation).
//!
//! The link procedure is answered automatically. Note what `send` means here:
//! in unbalanced mode nothing is transmitted spontaneously — the standard
//! forbids it — so data is **buffered** and handed to the primary when it
//! polls. Causes `Periodic` and `Background` go to the class 2 buffer, and
//! everything else to class 1, whose presence is announced with the ACD bit.
//!
//! ```sh
//! # over a serial port (needs the `serial` feature)
//! cargo run --features serial --example cs101_server -- /dev/ttyUSB1
//! cargo run --features serial --example cs101_server -- COM4 19200
//!
//! # listening for a terminal server, or for the cs101_client example
//! cargo run --example cs101_server -- tcp:127.0.0.1:2404
//! ```

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs101::{
    Config, SerialConfig, Server, ServerHandler, TcpConfig, TransportType,
};

const LINK_ADDR: u16 = 1;
const COMMON_ADDR: CommonAddr = 1;

/// A tiny process image.
#[derive(Debug, Clone)]
struct ProcessImage {
    switches: [bool; 2],
    temperature: f32,
}

impl Default for ProcessImage {
    fn default() -> Self {
        ProcessImage {
            switches: [true, false],
            temperature: 22.5,
        }
    }
}

struct Outstation {
    image: Arc<Mutex<ProcessImage>>,
}

#[async_trait::async_trait]
impl ServerHandler for Outstation {
    async fn interrogation(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        qoi: QualifierOfInterrogation,
    ) -> rs_iec60870_5::Result<()> {
        println!("<- interrogation, qualifier {}", qoi.0);
        if qoi != QualifierOfInterrogation::STATION {
            return c
                .send(pack.reply_mirror(Cause::ACTIVATION_CON).negated())
                .await;
        }

        // Each of these lands in the class 1 buffer and is collected by the
        // primary's next polls, in this order.
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;

        let image = self.image.lock().await.clone();
        let coa = CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION);
        let ca = pack.common_addr();

        c.send_single(
            false,
            coa,
            ca,
            &[
                SinglePointInfo::new(100, image.switches[0]),
                SinglePointInfo::new(101, image.switches[1]),
            ],
        )
        .await?;
        c.send_measured_value_float(
            false,
            coa,
            ca,
            &[MeasuredValueFloatInfo {
                ioa: 400,
                value: image.temperature,
                qds: QualityDescriptor::GOOD,
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
        time: Option<chrono::DateTime<chrono::Utc>>,
    ) -> rs_iec60870_5::Result<()> {
        println!("<- clock synchronization: {time:?}");
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await
    }

    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        match pack.type_id() {
            TypeId::C_SC_NA_1 | TypeId::C_SC_TA_1 => {
                let cmd = pack.get_single_cmd()?;
                println!("<- single command: ioa={} value={}", cmd.ioa, cmd.value);

                if cmd.qoc.in_select {
                    return c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await;
                }
                match cmd.ioa {
                    100 => self.image.lock().await.switches[0] = cmd.value,
                    101 => self.image.lock().await.switches[1] = cmd.value,
                    _ => {
                        return c
                            .send(pack.reply_mirror(Cause::ACTIVATION_CON).negated())
                            .await;
                    }
                }
                c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
                c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
            }
            other => {
                println!("<- unhandled type {other}");
                Ok(())
            }
        }
    }

    async fn on_link_active(&self, _c: &dyn Connect) {
        println!("the primary reset the link; it is now active");
    }

    async fn on_connection_lost(&self, _c: &dyn Connect) {
        println!("the line went down");
    }
}

/// Build the link configuration from the command line.
fn config_from_args() -> Config {
    let mut args = std::env::args().skip(1);
    let endpoint = args.next().unwrap_or_else(|| "tcp:127.0.0.1:2404".into());

    let mut cfg = Config {
        link_address: LINK_ADDR,
        link_addr_size: 1,
        timeout_send_link_msg: Duration::from_millis(100),
        ..Default::default()
    };

    match endpoint.strip_prefix("tcp:") {
        Some(address) => {
            // Listen, so the primary (or a terminal server) dials in.
            cfg.transport = TransportType::TcpServer;
            cfg.tcp = TcpConfig {
                address: address.to_string(),
                ..Default::default()
            };
            println!("IEC 60870-5-101 outstation listening on {address}");
        }
        None => {
            let baud = args.next().and_then(|b| b.parse().ok()).unwrap_or(9600);
            cfg.transport = TransportType::Serial;
            cfg.serial = SerialConfig::new(&endpoint, baud);
            println!("IEC 60870-5-101 outstation on {endpoint} at {baud} baud, 8E1");
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

    let image = Arc::new(Mutex::new(ProcessImage::default()));
    let srv = Server::new(Outstation {
        image: Arc::clone(&image),
    })
    .with_config(config_from_args())?;

    srv.start()?;
    println!(
        "  link address {LINK_ADDR}, common address {COMMON_ADDR}, \
         points 100/101 (single), 400 (float)"
    );

    // Buffer data for the primary to collect. `Periodic` goes to class 2, so it
    // rides along with the routine polling; an event would go to class 1 and
    // raise the ACD bit instead.
    let mut ticker = tokio::time::interval(Duration::from_secs(5));
    loop {
        ticker.tick().await;
        if !srv.is_link_active() {
            continue;
        }

        let temperature = {
            let mut img = image.lock().await;
            img.temperature += 0.25;
            img.temperature
        };

        match srv
            .send_measured_value_float(
                false,
                CauseOfTransmission::new(Cause::PERIODIC),
                COMMON_ADDR,
                &[MeasuredValueFloatInfo {
                    ioa: 400,
                    value: temperature,
                    qds: QualityDescriptor::GOOD,
                    time: None,
                    time_flags: TimeTagFlags::GOOD,
                }],
            )
            .await
        {
            Ok(()) => {
                let (c1, c2) = srv.buffered();
                println!("-> buffered periodic {temperature:.2} (class 1: {c1}, class 2: {c2})");
            }
            Err(e) => println!("-> buffering failed: {e}"),
        }
    }
}
