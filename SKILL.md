---
name: rs-iec60870-5
description: Build IEC 60870-5-104 (TCP), IEC 60870-5-101 (serial) and IEC 60870-5-103 (protection relays) SCADA/telecontrol masters and outstations in Rust with the rs-iec60870-5 crate. Use when implementing an IEC 104 client/server, an IEC 101 primary/secondary station, an IEC 103 relay master, a protocol gateway, an RTU simulator, or when parsing/sending ASDUs (single points, measured values, commands, interrogation).
---

# Building IEC 60870-5-104/101/103 apps with rs-iec60870-5

Crate: `rs-iec60870-5` (Rust ≥ 1.85, edition 2024, async on Tokio).
Modules: `asdu` (101/104 application layer), `cs104` (TCP), `cs101` (serial),
`cs103` (protection relays, master only).
Detailed docs: `docs/asdu.md`, `docs/cs104.md`, `docs/cs101.md`,
`docs/cs103.md`, `docs/guide.md`.

```toml
[dependencies]
rs-iec60870-5 = "0.1"
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"
chrono = "0.4"
```

Features: `cs104`, `cs101`, `cs103` on by default; `serial` for real serial
ports; `tls`; `serde`. The `asdu` layer is always built.

## Pick the right endpoint

| You are building | Use |
|---|---|
| Master polling devices over TCP | `cs104::Client` |
| Outstation/RTU serving masters over TCP | `cs104::Server` (listens) |
| Outstation that dials out to the master (NAT) | `cs104::ServerSpecial` |
| Master on a serial line (RS-232/485) | `cs101::Client` |
| Outstation on a serial line | `cs101::Server` |
| Master for protection relays | `cs103::Client` |
| 101/103 through a terminal server | same endpoints with `cfg.transport = TransportType::TcpClient`/`TcpServer` |

Vocabulary: master = controlling station = client; outstation = RTU = slave =
controlled station = server. Monitor direction = data to the master (`M_*`);
control direction = commands to the outstation (`C_*`).

## Core concepts (5 minutes)

- **ASDU**: one application message. `Identifier` = type ID + variable
  structure (object count + SQ bit) + cause of transmission + common address
  (the station). Each information object has an information object address
  (IOA, the point number).
- **`asdu::Params`** sets the octet widths of COT/CA/IOA. It must be identical
  on both peers. `PARAMS_WIDE` for 104 (its standard, the `cs104` default),
  `PARAMS_STANDARD_101` for 101 (the `cs101` default). Override only when the
  device profile says so.
- **`asdu::Connect`** is the trait every endpoint implements (`params()`,
  `send()`). **`asdu::ConnectExt`** is blanket-implemented on top and adds a
  build-and-send method per family, so the same code works on any endpoint,
  including `&dyn Connect` inside a handler.
- **COT matters.** Builders validate it and return `Error::CmdCause` when
  wrong. Commands go with `ACTIVATION`; the outstation confirms with
  `ACTIVATION_CON` and finishes with `ACTIVATION_TERM`; events use
  `SPONTANEOUS`; interrogation responses use `INTERROGATED_BY_STATION`; cyclic
  data uses `PERIODIC`.
- **Sends are queued, not blocking.** `Error::BufferFull` and
  `Error::SendQueueFull` mean back off and retry.
- **Handlers**: implement a trait; every method has a default that does
  nothing, so write only what you need. All are `async fn` via `async_trait`.

## Recipe: IEC 104 outstation

```rust,no_run
use std::sync::Arc;
use std::time::Duration;
use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{Server, ServerHandler};

struct H;

#[async_trait::async_trait]
impl ServerHandler for H {
    async fn interrogation(
        &self, c: &dyn Connect, pack: &Asdu, qoi: QualifierOfInterrogation,
    ) -> rs_iec60870_5::Result<()> {
        if qoi != QualifierOfInterrogation::STATION {
            return c.send(pack.reply_mirror(Cause::ACTIVATION_CON).negated()).await;
        }
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;

        let coa = CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION);
        let ca = pack.common_addr();
        c.send_single(false, coa, ca, &[SinglePointInfo::new(100, true)]).await?;
        c.send_measured_value_float(false, coa, ca,
            &[MeasuredValueFloatInfo { ioa: 400, value: 22.5, ..Default::default() }]).await?;

        c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    }

    // Control commands arrive here; an Err reply sends an UnknownTypeID mirror.
    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        match pack.type_id() {
            TypeId::C_SC_NA_1 | TypeId::C_SC_TA_1 => {
                let cmd = pack.get_single_cmd()?;
                if cmd.qoc.in_select {              // select: confirm, do not operate
                    return c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await;
                }
                c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
                c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
            }
            _ => Err(rs_iec60870_5::Error::TypeIdentifier),
        }
    }
}

#[tokio::main]
async fn main() -> rs_iec60870_5::Result<()> {
    let srv = Server::new(H);

    // Server::send broadcasts to every connected master.
    let publisher = Arc::clone(&srv);
    tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_secs(10));
        loop {
            t.tick().await;
            let _ = publisher.send_single_cp56time2a(
                CauseOfTransmission::new(Cause::SPONTANEOUS), 1,
                &[SinglePointInfo { ioa: 100, value: true,
                    qds: QualityDescriptor::GOOD, time: Some(chrono::Utc::now()) }],
            ).await;
        }
    });

    srv.listen_and_serve("0.0.0.0:2404").await   // blocks
}
```

## Recipe: IEC 104 master

```rust,no_run
use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{Client, ClientHandler, ClientOption};

struct H;

#[async_trait::async_trait]
impl ClientHandler for H {
    // All process data lands here, including interrogation responses.
    async fn asdu(&self, _c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        match pack.type_id() {
            TypeId::M_SP_NA_1 | TypeId::M_SP_TA_1 | TypeId::M_SP_TB_1 => {
                for p in pack.get_single_point()? {
                    println!("SP ioa={} v={} q={} t={:?}", p.ioa, p.value, p.qds, p.time);
                }
            }
            TypeId::M_ME_NC_1 | TypeId::M_ME_TC_1 | TypeId::M_ME_TF_1 => {
                for m in pack.get_measured_value_float()? {
                    println!("ME ioa={} v={} q={}", m.ioa, m.value, m.qds);
                }
            }
            _ => {}
        }
        Ok(())
    }

    // Interrogate on every activation, so a reconnection refreshes the image.
    async fn on_activated(&self, c: &dyn Connect) {
        let _ = c.send_interrogation_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION), 1,
            QualifierOfInterrogation::STATION).await;
    }
}

#[tokio::main]
async fn main() -> rs_iec60870_5::Result<()> {
    let client = Client::new(H, ClientOption::new().with_server("127.0.0.1:2404")?);
    client.start()?;                 // returns immediately
    client.wait_active().await;      // STARTDT confirmed

    client.send_single_cmd(
        TypeId::C_SC_NA_1,
        CauseOfTransmission::new(Cause::ACTIVATION), 1,
        SingleCommandInfo { ioa: 6000, value: true, ..Default::default() },
    ).await?;

    std::future::pending::<()>().await;
    Ok(())
}
```

`with_auto_start_dt` is **on by default**, so the link activates itself. Turn
it off to drive `send_start_dt()` by hand.

## Recipe: IEC 101

Master and outstation mirror the 104 recipes with these differences:

```rust,no_run
use rs_iec60870_5::cs101::{Client, ClientHandler, ClientOption, Config, SerialConfig,
                        Server, ServerHandler, TcpConfig, TransportType};
# struct H; impl ClientHandler for H {} impl ServerHandler for H {}
# fn f() -> rs_iec60870_5::Result<()> {
let mut cfg = Config::new();
cfg.serial = SerialConfig::new("/dev/ttyUSB0", 9600);   // 8E1 is the default
cfg.link_address = 1;
// TCP encapsulation instead:
//   cfg.transport = TransportType::TcpClient;   // or TcpServer to listen
//   cfg.tcp = TcpConfig { address: "10.0.0.9:2400".into(), ..Default::default() };

// master
let cli = Client::new(H, ClientOption::new()
    .with_config(cfg.clone())?
    .with_secondary_address(1)          // multi-drop: add each station
    .with_secondary_address(2));
cli.start()?;                            // link init + class polling is automatic

// outstation
let srv = Server::new(H).with_config(cfg)?;
srv.start()?;
# Ok(())
# }
```

- The link procedure runs automatically: status of link → reset of remote link
  → class 2 polling, with a class 1 request whenever a response sets ACD. FCB
  is tracked per station across **every** FCV frame.
- `cli.wait_link_active().await` waits for initialization; `is_link_active()`
  reports it.
- **On the outstation, `send` buffers**: `Periodic`/`Background` causes go to
  class 2, everything else to class 1, announced with the ACD bit. Unbalanced
  mode forbids unsolicited transmission. `srv.buffered()` shows the depths.
- Multi-drop targeting: `cli.send_to(asdu, link_addr)`; `Connect::send` targets
  the first configured address.
- Balanced point-to-point: `cfg.mode = TransmissionMode::Balanced` on both ends
  with the same link address; then both sides transmit spontaneously and the
  class buffers are unused.
- The serial transport needs the `serial` feature; the TCP transports do not.

## Recipe: IEC 103 relay master

103 is a different application layer (its own `Asdu`, no `asdu::Params`):
FUN/INF addressing, 13-bit measurands, CP32 time tags. The client automates the
whole procedure — link init, identification, time sync + general interrogation
(`auto_init`, on by default), class 2 polling with class 1 fetch on ACD.

```rust,no_run
use rs_iec60870_5::cs103::{Asdu, Client, ClientHandler, ClientOption, Config, Dco,
                        Link, MeasurandsInfo, SerialConfig, TimeTaggedInfo, fun, inf};

struct H;

#[async_trait::async_trait]
impl ClientHandler for H {
    async fn time_tagged(&self, _l: &dyn Link, pack: &Asdu, info: TimeTaggedInfo)
        -> rs_iec60870_5::Result<()> {
        // pack.coa distinguishes: SPONTANEOUS event, GI reply,
        // COMMAND_ACK_POS/NEG with the RII in info.sin.
        println!("device {} FUN={} INF={} {}", pack.common_addr, info.fun, info.inf, info.dpi);
        Ok(())
    }
    async fn measurands(&self, _l: &dyn Link, _p: &Asdu, info: MeasurandsInfo)
        -> rs_iec60870_5::Result<()> {
        for m in &info.values { println!("{:.4} of full scale", m.f64()); }
        Ok(())
    }
}

# #[tokio::main]
# async fn main() -> rs_iec60870_5::Result<()> {
let mut cfg = Config::new();
cfg.serial = SerialConfig::new("/dev/ttyUSB0", 9600);
cfg.link_address = 3;                       // the relay address

let cli = Client::new(H, ClientOption::new().with_config(cfg)?);
cli.start()?;
cli.wait_link_active().await;

cli.general_command(3, fun::OVERCURRENT_PROTECTION, inf::AUTO_RECLOSER_ACTIVE, Dco::On, 42)?;
cli.general_interrogation(3, 1)?;
cli.time_sync(3)?;
# Ok(())
# }
```

Handler dispatch: ASDU 1/2 → `time_tagged`; 3/9 → `measurands`; 5 →
`identification`; 8 → `gi_termination`; rest → `asdu`. The device is identified
by `pack.common_addr`. Command acknowledgements come back as ASDU 1 with cause
20 (positive) or 21 (negative), the RII in `info.sin`. Not implemented: generic
services codecs, disturbance data, the device side.

## Cause-of-transmission cheat sheet

| Situation | COT |
|---|---|
| Master sends any command | `Cause::ACTIVATION` |
| Outstation confirms | `pack.reply_mirror(Cause::ACTIVATION_CON)` |
| Outstation finishes | `pack.reply_mirror(Cause::ACTIVATION_TERM)` |
| Outstation rejects | `pack.reply_mirror(Cause::ACTIVATION_CON).negated()` |
| Interrogation response data | `Cause::INTERROGATED_BY_STATION` (or a group) |
| Event / change data | `Cause::SPONTANEOUS` |
| Cyclic measurements | `Cause::PERIODIC` (untagged types only) |
| Counter responses | `Cause::REQUEST_BY_GENERAL_COUNTER` … `REQUEST_BY_GROUP4_COUNTER` |
| Return info after a command | `Cause::RETURN_INFO_REMOTE` |
| Protocol errors | `Cause::UNKNOWN_TYPE_ID` / `UNKNOWN_COT` / `UNKNOWN_CA` / `UNKNOWN_IOA` |

## Choosing a type identification

- Boolean status → `M_SP_*`; three-state switchgear → `M_DP_*`.
- Analog: short float `M_ME_NC_1` (easiest), scaled `int16` `M_ME_NB_1`,
  normalized −1..1 `M_ME_NA_1` (`Normalize::f64()` / `from_f64`).
- Counters → `M_IT_*`. Thirty-two status bits → `M_BO_*`.
- Suffix picks the time tag: `_NA` none, `_TA`/`_TB` (1–16) CP24,
  `_TB`/`_TD`/`_TE`/`_TF` (30+) CP56. Prefer CP56 for events; untagged for
  interrogation and cyclic data.
- Commands: `C_SC` (bool), `C_DC` (`DoubleCommand::On`/`Off`), `C_RC` (step up
  or down), `C_SE_NA`/`NB`/`NC` (setpoints), `C_BO` (bit string).

## API shape

- **Build**: `Asdu::single(params, is_sequence, coa, ca, &[info])?` and one
  builder per family; `Asdu::single_cp56time2a(...)` for the tagged variant;
  `Asdu::single_cmd(params, type_id, coa, ca, info)?` for commands, where the
  type identification picks the tagged variant.
- **Send**: `conn.send(asdu).await`, or the `ConnectExt` one-liners
  `conn.send_single(...)`, `conn.send_measured_value_float_cp56time2a(...)`,
  `conn.send_single_cmd(...)`, `conn.send_interrogation_cmd(...)`.
- **Decode**: `pack.get_single_point()?`, `pack.get_measured_value_float()?`,
  `pack.get_single_cmd()?` … Non-destructive, callable in any order, any number
  of times, and they return `Result` rather than panicking.
- **Reply**: `pack.reply_mirror(cause)` and `.negated()`.
- **Wire**: `asdu.marshal_binary()?` / `Asdu::unmarshal_binary(params, &raw)?`.
- **Diagnostics**: `Asdu` implements `Display`.

## Verification without hardware

- 104: loop a `cs104::Client` against a `cs104::Server`. Bind the listener
  yourself (`TcpListener::bind("127.0.0.1:0")`) and pass it to `serve` so the
  test knows the port.
- 101/103: use the TCP encapsulation transports — one side `TcpServer`, the
  other `TcpClient`. The full FT1.2 procedure runs unchanged.
  `cs101::Server::bind()` makes `listen_addr()` observable before the first
  connection.
- 103 devices: master only, so drive it against a simulated relay;
  `tests/common/relay.rs` is a working one.
- Read `tests/cs104_loopback.rs`, `tests/cs101_loopback.rs` and
  `tests/cs103_loopback.rs` as recipes.
- Third-party: `lib60870` (C), OpenMUC j60870, QTester104, or any 104 test set
  at the defaults k=12, w=8, t1=15 s, t2=10 s, t3=20 s.

## Pitfalls (read before debugging)

1. **`asdu::Params` mismatch = garbage decoding** (wrong types and causes).
   Both ends identical: 104 ⇒ `PARAMS_WIDE`, 101 ⇒ `PARAMS_STANDARD_101`.
2. **A 104 link starts in STOPDT**; nothing flows until StartDT is confirmed
   and `send` returns `Error::NotActive`. This crate sends it automatically —
   do not also send it by hand unless you set `with_auto_start_dt(false)`.
3. **On a 104 master, interrogation *data* arrives at `ClientHandler::asdu`**,
   not `interrogation`, which sees the `C_IC` confirmations.
4. `Error::CmdCause` means the COT is not allowed for that type — see the cheat
   sheet.
5. Getters are non-destructive and return `Result`; a truncated payload gives
   `Error::UnexpectedEof`, never a panic.
6. Broadcast is `GLOBAL_COMMON_ADDR` (65535) — never 255, even with 1-octet
   addressing. Legal only for `C_IC`, `C_CI`, `C_CS`, `C_RP`.
7. IOA 0 is "irrelevant" and reserved for system commands; real points start at
   1. Common address 0 is never valid.
8. Max 249 octets per ASDU (`Error::LengthOutOfRange`): batch large point sets
   across calls, or use `is_sequence = true` for contiguous IOAs.
9. **On a `cs101::Server`, `send` buffers** into class 1/2 and is collected by
   polling. Nothing is transmitted spontaneously in unbalanced mode.
10. Handlers must not block; hand long work to your own task and send later.
    Every endpoint is `Send + Sync` and shared behind an `Arc`.
11. Select-before-execute is application-level: check `cmd.qoc.in_select` (or
    `qos.in_select`) and confirm without operating when it is a select.
12. File transfer (`F_*`) and IEC 62351-5 (`S_*`) types are not implemented.
13. go-iecp5 cannot size types 58–64 (CP56-tagged commands) and drops them on
    receipt; this crate handles them, but avoid those types against a go-iecp5
    peer.
