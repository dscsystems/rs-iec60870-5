# User guide

A task-oriented tour of `rs-iec60870-5`. For the exhaustive per-module references
see [`asdu`](asdu.md), [`cs104`](cs104.md), [`cs101`](cs101.md) and
[`cs103`](cs103.md).

**Contents.** [Which endpoint](#1-pick-the-right-endpoint) ·
[Concepts](#2-the-five-concepts-that-matter) ·
[A master](#3-writing-a-master) · [An outstation](#4-writing-an-outstation) ·
[Commands](#5-sending-commands) · [Time](#6-time-and-time-tags) ·
[Errors](#7-errors-and-backpressure) · [Serial](#8-serial-lines-101-and-103) ·
[TLS](#9-tls) · [Logging](#10-logging-and-diagnostics) ·
[Testing](#11-testing-without-hardware) · [Performance](#12-performance-notes) ·
[Pitfalls](#13-pitfalls)

## 1. Pick the right endpoint

| You are building | Use |
|---|---|
| A master polling devices over TCP | `cs104::Client` |
| An outstation serving masters over TCP | `cs104::Server` |
| An outstation that dials out to the master (NAT) | `cs104::ServerSpecial` |
| A master on a serial line | `cs101::Client` |
| An outstation on a serial line | `cs101::Server` |
| A master for protection relays | `cs103::Client` |
| 101 or 103 through a terminal server | the same, with `Config::transport = TcpClient`/`TcpServer` |
| Only encoding and decoding, no I/O | `asdu` alone — turn off the transport features |

Vocabulary: *master* = controlling station = client; *outstation* = RTU = slave
= controlled station = server. *Monitor direction* is data flowing to the
master (`M_*`); *control direction* is commands to the outstation (`C_*`).

## 2. The five concepts that matter

**ASDU.** One application message: a type identification, a count of
information objects, a cause of transmission, a common address, and the objects
themselves. Each object has an information object address — the point number.

**Params.** The octet widths of those identifier fields. `PARAMS_WIDE` for 104,
`PARAMS_STANDARD_101` for 101. **Both peers must match**, or every ASDU decodes
as garbage. This is the single most common misconfiguration.

**Cause of transmission.** What a message *means*. Commands go out with
`ACTIVATION`; the outstation confirms with `ACTIVATION_CON` and finishes with
`ACTIVATION_TERM`; events use `SPONTANEOUS`; interrogation responses use
`INTERROGATED_BY_STATION`; cyclic data uses `PERIODIC`. The builders validate
the cause against the standard and return `Error::CmdCause` when it is wrong.

**Connect.** The trait every endpoint implements. `ConnectExt` adds a
build-and-send method per ASDU family. The same data-sending code therefore
works against a 104 client, a 104 server, a 104 session, or a 101 station.

**Handlers.** You implement a trait; the library calls it. Every method has a
default that does nothing, so write only what you need.

## 3. Writing a master

```rust,no_run
use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{Client, ClientHandler, ClientOption};

struct Master;

#[async_trait::async_trait]
impl ClientHandler for Master {
    async fn asdu(&self, _c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        match pack.type_id() {
            TypeId::M_SP_NA_1 | TypeId::M_SP_TA_1 | TypeId::M_SP_TB_1 => {
                for p in pack.get_single_point()? {
                    println!("ioa={} value={} quality={}", p.ioa, p.value, p.qds);
                }
            }
            TypeId::M_ME_NC_1 | TypeId::M_ME_TC_1 | TypeId::M_ME_TF_1 => {
                for m in pack.get_measured_value_float()? {
                    println!("ioa={} value={}", m.ioa, m.value);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

# #[tokio::main]
# async fn main() -> rs_iec60870_5::Result<()> {
let client = Client::new(Master, ClientOption::new().with_server("127.0.0.1:2404")?);
client.start()?;                 // returns immediately, connects in the background
client.wait_active().await;      // STARTDT confirmed

client
    .interrogation_cmd(CauseOfTransmission::new(Cause::ACTIVATION), 1, QualifierOfInterrogation::STATION)
    .await
# }
```

Three things to know:

* **Interrogation data arrives at `asdu`**, not `interrogation`. The dedicated
  methods see only the mirrored command confirmations. This trips up almost
  everyone once.
* Group the time-tagged and untagged variants of a family in one match arm.
  The getter handles all three and fills `time` with `None` for the untagged
  one, so downstream code does not care which arrived.
* `wait_active()` does not resolve if the connection drops. Wrap it in
  `tokio::time::timeout` when the wait must be bounded.

### Reacting to connection state

```rust,no_run
# use rs_iec60870_5::asdu::*;
# use rs_iec60870_5::cs104::ClientHandler;
# struct Master;
#[async_trait::async_trait]
impl ClientHandler for Master {
    async fn on_activated(&self, c: &dyn Connect) {
        // A good place to interrogate: it runs on every reconnection, not just
        // the first, so the process image is refreshed after every outage.
        let _ = c.send_interrogation_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION),
            1,
            QualifierOfInterrogation::STATION,
        ).await;
    }

    async fn on_connection_lost(&self, _c: &dyn Connect) {
        // Mark your cached points stale here.
    }
}
```

## 4. Writing an outstation

An outstation answers interrogation with its whole process image, in a fixed
shape: confirm, send the data with an interrogation cause, terminate.

```rust,no_run
use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{Server, ServerHandler};

struct Outstation;

#[async_trait::async_trait]
impl ServerHandler for Outstation {
    async fn interrogation(
        &self,
        c: &dyn Connect,
        pack: &Asdu,
        qoi: QualifierOfInterrogation,
    ) -> rs_iec60870_5::Result<()> {
        if qoi != QualifierOfInterrogation::STATION {
            // Reject a group this station does not implement.
            return c.send(pack.reply_mirror(Cause::ACTIVATION_CON).negated()).await;
        }
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;

        let coa = CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION);
        let ca = pack.common_addr();
        c.send_single(false, coa, ca, &[
            SinglePointInfo::new(100, true),
            SinglePointInfo::new(101, false),
        ]).await?;
        c.send_measured_value_float(false, coa, ca, &[
            MeasuredValueFloatInfo { ioa: 400, value: 22.5, ..Default::default() },
        ]).await?;

        c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    }
}

# #[tokio::main]
# async fn main() -> rs_iec60870_5::Result<()> {
let srv = Server::new(Outstation);
srv.listen_and_serve("0.0.0.0:2404").await
# }
```

Inside a handler the `&dyn Connect` argument is the **single session** the
request came from. Calling `send` on the `Server` itself instead broadcasts to
every master in data transfer, which is how spontaneous data is published:

```rust,no_run
# use std::sync::Arc;
# use std::time::Duration;
# use rs_iec60870_5::asdu::*;
# use rs_iec60870_5::cs104::{Server, ServerHandler};
# struct H; impl ServerHandler for H {}
# async fn f() {
# let srv = Server::new(H);
let publisher = Arc::clone(&srv);
tokio::spawn(async move {
    let mut ticker = tokio::time::interval(Duration::from_secs(5));
    loop {
        ticker.tick().await;
        if publisher.session_count() == 0 { continue; }
        let _ = publisher.send_measured_value_float_cp56time2a(
            CauseOfTransmission::new(Cause::SPONTANEOUS),
            1,
            &[MeasuredValueFloatInfo {
                ioa: 400, value: 22.5,
                time: Some(chrono::Utc::now()),
                ..Default::default()
            }],
        ).await;
    }
});
# }
```

### Choosing a type

* Boolean status → `M_SP_*`; three-state switchgear → `M_DP_*`.
* Analog: short float `M_ME_NC_1` is the easiest; scaled `int16` is
  `M_ME_NB_1`; normalized `−1..1` is `M_ME_NA_1`.
* Counters → `M_IT_*`. Thirty-two status bits → `M_BO_*`.
* The suffix picks the time tag: `_NA` none, `_TA`/`_TB` in 1–16 CP24,
  `_TB`/`_TD`/`_TE`/`_TF` in 30+ CP56. **Prefer CP56 for events**; use untagged
  types for interrogation and cyclic data.

### Rejecting properly

An outstation should say what went wrong rather than stay silent:

```rust,no_run
# use rs_iec60870_5::asdu::*;
# async fn f(c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
// The point exists but the operation failed:
c.send(pack.reply_mirror(Cause::ACTIVATION_CON).negated()).await?;

// The type is not implemented — returning Err from `asdu` does this for you:
c.send(pack.reply_mirror(Cause::UNKNOWN_TYPE_ID)).await
# }
```

The session already answers `UnknownCOT`, `UnknownCA` and `UnknownIOA` for
malformed requests to the dedicated handlers, so your code sees only
well-formed ones.

## 5. Sending commands

```rust,no_run
# use rs_iec60870_5::asdu::*;
# async fn f(c: &dyn Connect) -> rs_iec60870_5::Result<()> {
# let (coa, ca) = (CauseOfTransmission::new(Cause::ACTIVATION), 1);
c.send_single_cmd(TypeId::C_SC_NA_1, coa, ca, SingleCommandInfo {
    ioa: 6000,
    value: true,
    qoc: QualifierOfCommand { qual: QocQual::SHORT_PULSE_DURATION, in_select: false },
    ..Default::default()
}).await
# }
```

Pass `TypeId::C_SC_TA_1` instead for the CP56Time2a variant, in which case the
`time` field is encoded.

**Select-before-execute** is application-level in both directions. A master
sends the command twice, first with `in_select: true` and then with
`in_select: false`. An outstation checks `cmd.qoc.in_select` and confirms a
select **without operating**:

```rust,no_run
# use rs_iec60870_5::asdu::*;
# async fn f(c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
let cmd = pack.get_single_cmd()?;
if cmd.qoc.in_select {
    return c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await;
}
// ... operate ...
c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
# }
```

After a command changes something, report the new state back with cause
`RETURN_INFO_REMOTE`, as the standard expects.

## 6. Time and time tags

All timestamps in the API are `chrono::DateTime<Utc>`. An `Option<DateTime>` of
`None` means "no valid time" — either the type carries no tag, or the tag's IV
bit was set.

`Params::info_obj_time_zone` decides how tags are *encoded and decoded on the
wire*, not how they are represented in Rust. `TimeZone::Utc` is the default and
what the standard recommends; `Local` and `Fixed(offset)` exist for devices
configured in local time.

CP24Time2a carries only minutes and milliseconds, so decoding completes it from
the host clock, and a tag more than five minutes ahead of the current minute is
taken to belong to the previous hour. CP32Time2a in 103 does the same with
days. Both are best avoided for anything that must survive a clock difference —
prefer CP56Time2a.

## 7. Errors and backpressure

Everything returns `rs_iec60870_5::Result<T>` with one crate-wide `Error`.

**Programming errors** show up at build time: `CmdCause` (wrong cause for the
type), `TypeIdNotMatch`, `InfoObjAddrFit`, `LengthOutOfRange`,
`NotAnyObjInfo`. Fix the call.

**State errors** are transient: `UseClosedConnection` (not connected),
`NotActive` (104, StartDT not confirmed), `LinkNotActive` (101/103).

**Backpressure** is `BufferFull` and `SendQueueFull`. These mean *back off and
retry* — the data was not sent, and it was not silently dropped:

```rust,no_run
# use rs_iec60870_5::asdu::*;
# use std::time::Duration;
# async fn f(c: &dyn Connect, a: Asdu) -> rs_iec60870_5::Result<()> {
loop {
    match c.send(a.clone()).await {
        Ok(()) => break Ok(()),
        Err(rs_iec60870_5::Error::BufferFull | rs_iec60870_5::Error::SendQueueFull) => {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err(e) => break Err(e),
    }
}
# }
```

A queue that stays full means the peer is not acknowledging — on 104, the k
window is closed; on 101, the primary is not polling.

## 8. Serial lines (101 and 103)

Enable the `serial` feature. 8E1 is the framing the standard specifies and the
default here:

```rust
# use rs_iec60870_5::cs101::{Config, Parity, SerialConfig, StopBits};
let mut cfg = Config::new();
cfg.serial = SerialConfig {
    address: "/dev/ttyUSB0".into(),
    baud_rate: 9600,
    data_bits: 8,
    parity: Parity::Even,
    stop_bits: StopBits::One,
    timeout: None,
};
cfg.link_address = 1;
cfg.link_addr_size = 1;
# assert_eq!(cfg.transport_label(), "/dev/ttyUSB0");
```

Without the feature the serial transport returns a clear `Config` error, and
the TCP transports still work — which is how the tests run the full FT1.2
procedure with no hardware.

`timeout_send_link_msg` is the polling period of an unbalanced primary. The
200 ms default is a reasonable compromise; lower it for latency on a fast line,
raise it on a slow multi-drop one.

## 9. TLS

Enable the `tls` feature. For 104, use a `tls://` endpoint and supply a rustls
configuration:

```rust,ignore
use std::sync::Arc;
use rs_iec60870_5::cs104::{ClientOption, TlsClientConfig};

let option = ClientOption::new()
    .with_server("tls://outstation.example:19998")?
    .with_tls(TlsClientConfig::new(Arc::new(client_config)));
```

`TlsClientConfig::with_server_name` overrides the name checked against the
certificate when it differs from the endpoint host. On the listening side,
`Server::with_tls(TlsServerConfig::new(Arc::new(server_config)))`. The 101 and
103 TCP transports take the same configurations through `TcpConfig`.

The library never builds a rustls configuration itself — you pass one in — so
the choice of cryptographic provider is yours. It pulls `tokio-rustls` in with
the **`ring`** provider rather than the default `aws-lc-rs`, because
`aws-lc-sys` needs cmake and bindgen and does not cross-compile to the musl,
32-bit ARM and RISC-V targets this project ships binaries for. If your own
crate depends on `rustls` with `aws-lc-rs` enabled, Cargo will unify the
features and both providers end up compiled in; `ClientConfig::builder()` then
panics because the default is ambiguous. Either match this crate's choice:

```toml
rustls = { version = "0.23", default-features = false, features = ["ring", "logging", "tls12"] }
```

or install the one you want explicitly at startup:

```rust,ignore
rustls::crypto::ring::default_provider().install_default().unwrap();
```

## 10. Logging and diagnostics

The library logs through `tracing`. Frame-level detail lives at `debug` and
`trace`:

```sh
RUST_LOG=rs_iec60870_5=debug cargo run --example cs104_client
```

`Asdu` implements `Display`, giving a compact non-destructive dump that is safe
to call at any point:

```text
TID<M_SP_NA_1> COT<Spontaneous> @1 VSQ<2> IOA-Width=3 items=2 [100=true, 101=false QDS=0x80]
```

In a terminal UI, install a `tracing` layer that routes into your own log panel
rather than stdout — `examples/cs104_explorer` does exactly that.

## 11. Testing without hardware

* **104**: loop a `Client` against a `Server` on `127.0.0.1:0`. Bind the
  listener yourself and pass it to `serve` so you know the port.
* **101 and 103**: use the TCP encapsulation transports. One side is
  `TcpServer`, the other `TcpClient`, and the whole FT1.2 procedure —
  initialization, FCB, class polling, ACD — runs unchanged. `Server::bind`
  makes `listen_addr()` observable before the first connection.
* **103 devices**: the module is a master only, so drive it against a simulated
  relay. `tests/common/relay.rs` is a working one.
* **Third-party interop**: OpenMUC j60870, QTester104, or any
  104 test set at the default parameters.

The repository's own tests are worth reading as recipes:
`tests/cs104_loopback.rs`, `tests/cs101_loopback.rs`,
`tests/cs103_loopback.rs`.

## 12. Performance notes

* Each connection runs four tasks — reader, writer, protocol driver and
  dispatcher — so a slow handler delays only that connection's ASDU delivery,
  never its acknowledgements or keep-alives.
* Getters borrow the payload and allocate one `Vec` for the decoded objects.
  Decoding is cheap; do it once and keep the result rather than calling a
  getter repeatedly in a hot loop.
* An ASDU holds at most 249 octets. With `PARAMS_WIDE` that is 60 untagged
  single points per message, or 240 in a sequence. Sequences cost one address
  instead of one per object, so use them for contiguous ranges.
* The k window bounds in-flight data: at k = 12 a master will not run ahead of
  the outstation by more than twelve I-frames. Raising k raises throughput on a
  high-latency link, at the cost of more unacknowledged data in flight.

## 13. Pitfalls

1. **`Params` must match on both peers.** A mismatch decodes as garbage.
2. **A 104 connection starts in STOPDT.** Nothing flows until StartDT is
   confirmed. This crate sends it automatically; disable with
   `with_auto_start_dt(false)` if you want to drive it.
3. **On a 104 master, interrogation data reaches `asdu`**, not
   `interrogation`.
4. **On a 101 secondary, `send` buffers.** Unbalanced mode forbids unsolicited
   transmission; the primary collects the data by polling. Check `buffered()`.
5. Sends are queued: `BufferFull` and `SendQueueFull` mean retry.
6. An ASDU holds at most 249 octets — batch, or use a sequence.
7. Broadcast is `GLOBAL_COMMON_ADDR` (65535) even with 1-octet addressing, and
   is legal only for `C_IC`, `C_CI`, `C_CS` and `C_RP`.
8. Information object address 0 means "irrelevant" and is reserved for system
   commands; real points start at 1.
9. Common address 0 is never valid.
10. Select-before-execute is yours to implement; check `qoc.in_select`.
11. Handlers must not block. Hand long work to your own task and send later —
    every endpoint is `Send + Sync` and cloneable behind an `Arc`.
12. File transfer (`F_*`) and IEC 62351-5 (`S_*`) types are not implemented.
