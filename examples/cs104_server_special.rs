// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! A "special" IEC 60870-5-104 controlled station: it **dials out** to the
//! master instead of listening, which is how an outstation behind NAT or a
//! firewall reaches its control centre.
//!
//! Everything above the TCP connection is unchanged — it answers interrogation
//! and StartDT exactly like a listening [`Server`](rs_iec60870_5::cs104::Server) —
//! only the direction of the connection is reversed.
//!
//! ```sh
//! cargo run --example cs104_server_special                       # 127.0.0.1:2404
//! cargo run --example cs104_server_special -- master.example:2404
//! ```
//!
//! The peer must be a master that accepts inbound connections. To try it
//! locally, note that a normal 104 master *connects out*, so pair this with a
//! purpose-built listener rather than `cargo run --example cs104_client`.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{ClientOption, ServerHandler, ServerSpecial};

const COMMON_ADDR: CommonAddr = 1;

struct Outstation {
    /// A single measured value, so there is something to report.
    level: Arc<Mutex<f32>>,
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
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;

        let coa = CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION);
        c.send_single(
            false,
            coa,
            pack.common_addr(),
            &[SinglePointInfo::new(100, true)],
        )
        .await?;
        c.send_measured_value_float(
            false,
            coa,
            pack.common_addr(),
            &[MeasuredValueFloatInfo {
                ioa: 400,
                value: *self.level.lock().await,
                qds: QualityDescriptor::GOOD,
                time: None,
            }],
        )
        .await?;

        c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    }

    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        if pack.type_id() == TypeId::C_SC_NA_1 {
            let cmd = pack.get_single_cmd()?;
            println!("<- single command: ioa={} value={}", cmd.ioa, cmd.value);
            c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
            return c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await;
        }
        Err(rs_iec60870_5::Error::TypeIdentifier)
    }

    async fn on_connect(&self, c: &dyn Connect) {
        println!("reached the master at {:?}", c.peer_addr());
    }

    async fn on_connection_lost(&self, _c: &dyn Connect) {
        println!("connection to the master lost; will redial");
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

    let master = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:2404".to_string());

    let level = Arc::new(Mutex::new(10.0f32));

    // The endpoint comes from a ClientOption because this end initiates the
    // connection, even though the station behaves as a controlled station.
    let option = ClientOption::new()
        .with_server(&master)?
        .with_reconnect_interval(Duration::from_secs(5));

    let station = ServerSpecial::new(
        Outstation {
            level: Arc::clone(&level),
        },
        option,
    );
    station.start()?;

    println!("IEC 60870-5-104 outstation dialling out to {master}");
    println!("  common address {COMMON_ADDR}, points 100 (single), 400 (float)");

    // Report the level whenever the master has activated the link.
    let mut ticker = tokio::time::interval(Duration::from_secs(5));
    loop {
        ticker.tick().await;
        if !station.is_active() {
            continue;
        }

        let value = {
            let mut l = level.lock().await;
            *l += 0.5;
            *l
        };

        match station
            .send_measured_value_float_cp56time2a(
                CauseOfTransmission::new(Cause::SPONTANEOUS),
                COMMON_ADDR,
                &[MeasuredValueFloatInfo {
                    ioa: 400,
                    value,
                    qds: QualityDescriptor::GOOD,
                    time: Some(chrono::Utc::now()),
                }],
            )
            .await
        {
            Ok(()) => println!("-> spontaneous: level {value:.1}"),
            // A full queue means back off, not that the data was dropped
            // silently; a real application would retry.
            Err(e) => println!("-> send failed: {e}"),
        }
    }
}
