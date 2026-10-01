// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! An IEC 60870-5-104 controlled station (outstation) with a small process
//! image, serving any number of masters.
//!
//! ```sh
//! cargo run --example cs104_server            # listens on 0.0.0.0:2404
//! cargo run --example cs104_server -- :2404
//! RUST_LOG=rs_iec60870_5=debug cargo run --example cs104_server
//! ```
//!
//! Point a master at it — `cargo run --example cs104_client`, QTester104,
//! OpenMUC j60870, or anything else that speaks 104.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{Server, ServerHandler};

/// The station address this outstation answers for.
const COMMON_ADDR: CommonAddr = 1;

/// A tiny process image: two switches and two measurements.
#[derive(Debug, Clone)]
struct ProcessImage {
    /// IOA 100, 101: single points.
    switches: [bool; 2],
    /// IOA 400: a short float measurement.
    temperature: f32,
    /// IOA 500: an energy counter.
    energy: i32,
}

impl Default for ProcessImage {
    fn default() -> Self {
        ProcessImage {
            switches: [true, false],
            temperature: 22.5,
            energy: 0,
        }
    }
}

struct Outstation {
    image: Arc<Mutex<ProcessImage>>,
}

#[async_trait::async_trait]
impl ServerHandler for Outstation {
    /// Answer a station interrogation with the whole process image.
    ///
    /// The pattern is always the same: confirm, send the data with an
    /// interrogation cause, then terminate.
    async fn interrogation(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        qoi: QualifierOfInterrogation,
    ) -> rs_iec60870_5::Result<()> {
        println!("<- interrogation, qualifier {}", qoi.0);

        if qoi != QualifierOfInterrogation::STATION {
            // This outstation only implements the station interrogation, so
            // reject any group with a negative confirmation.
            return c
                .send(pack.reply_mirror(Cause::ACTIVATION_CON).negated())
                .await;
        }
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

    /// Answer a counter interrogation with the energy total.
    async fn counter_interrogation(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        qcc: QualifierCountCall,
    ) -> rs_iec60870_5::Result<()> {
        println!("<- counter interrogation, request {}", qcc.request.0);
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;

        let energy = self.image.lock().await.energy;
        c.send_integrated_totals(
            false,
            CauseOfTransmission::new(Cause::REQUEST_BY_GENERAL_COUNTER),
            pack.common_addr(),
            &[BinaryCounterReadingInfo {
                ioa: 500,
                value: BinaryCounterReading {
                    counter_reading: energy,
                    seq_number: 0,
                    ..Default::default()
                },
                time: None,
                time_flags: TimeTagFlags::GOOD,
            }],
        )
        .await?;

        c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    }

    /// Accept the master's clock. A real outstation would set its RTC here.
    async fn clock_sync(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        time: Option<chrono::DateTime<chrono::Utc>>,
    ) -> rs_iec60870_5::Result<()> {
        println!("<- clock synchronization: {time:?}");
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await
    }

    /// Control commands and everything else without a dedicated method.
    ///
    /// Returning an error makes the session reply `UnknownTypeID`, which is how
    /// an outstation tells a master it does not implement a type.
    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        match pack.type_id() {
            TypeId::C_SC_NA_1 | TypeId::C_SC_TA_1 => {
                let cmd = pack.get_single_cmd()?;
                println!(
                    "<- single command: ioa={} value={} {}",
                    cmd.ioa, cmd.value, cmd.qoc
                );

                // Select-before-execute is the application's business: confirm
                // a select without touching the output.
                if cmd.qoc.in_select {
                    return c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await;
                }

                let index = match cmd.ioa {
                    100 => 0,
                    101 => 1,
                    _ => {
                        // No such point: reject rather than pretend.
                        return c
                            .send(pack.reply_mirror(Cause::ACTIVATION_CON).negated())
                            .await;
                    }
                };
                self.image.lock().await.switches[index] = cmd.value;

                c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
                c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await?;

                // Report the new state back, as the standard expects after a
                // command that changed something.
                c.send_single(
                    false,
                    CauseOfTransmission::new(Cause::RETURN_INFO_REMOTE),
                    pack.common_addr(),
                    &[SinglePointInfo::new(cmd.ioa, cmd.value)],
                )
                .await
            }

            TypeId::C_SE_NC_1 => {
                let sp = pack.get_setpoint_float_cmd()?;
                println!("<- float setpoint: ioa={} value={}", sp.ioa, sp.value);
                if sp.ioa == 400 {
                    self.image.lock().await.temperature = sp.value;
                }
                c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await
            }

            other => {
                println!("<- unhandled type {other}");
                Err(rs_iec60870_5::Error::TypeIdentifier)
            }
        }
    }

    async fn on_connect(&self, c: &dyn Connect) {
        println!("master connected: {:?}", c.peer_addr());
    }

    async fn on_connection_lost(&self, c: &dyn Connect) {
        println!("master disconnected: {:?}", c.peer_addr());
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

    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:2404".to_string());

    let image = Arc::new(Mutex::new(ProcessImage::default()));
    let srv = Server::new(Outstation {
        image: Arc::clone(&image),
    });

    // Publish spontaneous data. `Server::send` broadcasts to every connected
    // master, which is how an outstation reports events.
    {
        let srv = Arc::clone(&srv);
        let image = Arc::clone(&image);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(5));
            loop {
                ticker.tick().await;
                if srv.session_count() == 0 {
                    continue;
                }

                let (temperature, energy) = {
                    let mut img = image.lock().await;
                    // Wander the measurement and advance the counter.
                    img.temperature += 0.25;
                    img.energy += 17;
                    (img.temperature, img.energy)
                };

                let now = Some(chrono::Utc::now());
                let coa = CauseOfTransmission::new(Cause::SPONTANEOUS);

                let _ = srv
                    .send_measured_value_float_cp56time2a(
                        coa,
                        COMMON_ADDR,
                        &[MeasuredValueFloatInfo {
                            ioa: 400,
                            value: temperature,
                            qds: QualityDescriptor::GOOD,
                            time: now,
                            time_flags: TimeTagFlags::GOOD,
                        }],
                    )
                    .await;

                let _ = srv
                    .send_integrated_totals_cp56time2a(
                        coa,
                        COMMON_ADDR,
                        &[BinaryCounterReadingInfo {
                            ioa: 500,
                            value: BinaryCounterReading {
                                counter_reading: energy,
                                seq_number: 0,
                                ..Default::default()
                            },
                            time: now,
                            time_flags: TimeTagFlags::GOOD,
                        }],
                    )
                    .await;

                println!("-> spontaneous: temperature {temperature:.2}, energy {energy}");
            }
        });
    }

    println!("IEC 60870-5-104 outstation listening on {addr}");
    println!("  common address {COMMON_ADDR}, points 100/101 (single), 400 (float), 500 (counter)");
    srv.listen_and_serve(addr.as_str()).await
}
