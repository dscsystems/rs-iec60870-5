# rs-iec60870-5

Pure Rust implementation of the IEC 60870-5 telecontrol protocols: **-104**
(TCP/IP), **-101** (serial FT1.2) and **-103** (protection equipment), with both
client and server sides for 101 and 104.

Async throughout, built on Tokio. Verified for wire compatibility against
[`go-iecp5`](https://github.com/riclolsen/go-iecp5) in every direction.

```toml
[dependencies]
rs-iec60870-5 = "0.1"
```

## Modules

| Module | Purpose |
|--------|---------|
| [`asdu`] | Application layer shared by 101 and 104: ASDU encoding and decoding for the standard type identifications, causes of transmission, quality descriptors and time tags |
| [`cs104`] | IEC 60870-5-104 master and controlled station over TCP/IP, optionally TLS |
| [`cs101`] | IEC 60870-5-101 primary and secondary station over serial FT1.2, unbalanced (multi-drop) and balanced |
| [`cs103`] | IEC 60870-5-103 master for protection equipment |
| [`filetransfer`] | The file transfer procedures (types 120–126) on any endpoint: fetching disturbance records and the like |

Vocabulary: *master* = controlling station = client; *outstation* = RTU = slave
= controlled station = server. *Monitor direction* is data flowing to the master
(`M_*` types); *control direction* is commands to the outstation (`C_*` types).

## Quick start

### IEC 104 controlled station

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
            return c.send(pack.reply_mirror(Cause::ACTIVATION_CON).negated()).await;
        }
        // Confirm, send the process image, terminate.
        c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;

        let coa = CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION);
        c.send_single(false, coa, pack.common_addr(), &[SinglePointInfo::new(100, true)]).await?;
        c.send_measured_value_float(
            false,
            coa,
            pack.common_addr(),
            &[MeasuredValueFloatInfo { ioa: 400, value: 22.5, ..Default::default() }],
        )
        .await?;

        c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    }

    // Control commands arrive here; parse, act, confirm.
    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        if pack.type_id() == TypeId::C_SC_NA_1 {
            let cmd = pack.get_single_cmd()?;
            println!("single command ioa={} value={}", cmd.ioa, cmd.value);
            c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
            return c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await;
        }
        // An error makes the session reply UnknownTypeID.
        Err(rs_iec60870_5::Error::TypeIdentifier)
    }
}

# #[tokio::main]
# async fn main() -> rs_iec60870_5::Result<()> {
let srv = Server::new(Outstation);

// Push spontaneous data to every connected master at any time.
let publisher = srv.clone();
tokio::spawn(async move {
    let info = SinglePointInfo {
        ioa: 100, value: false, time: Some(chrono::Utc::now()), ..Default::default()
    };
    let _ = publisher
        .send_single_cp56time2a(CauseOfTransmission::new(Cause::SPONTANEOUS), 1, &[info])
        .await;
});

srv.listen_and_serve("0.0.0.0:2404").await
# }
```

### IEC 104 master

```rust,no_run
use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{Client, ClientHandler, ClientOption};

struct Master;

#[async_trait::async_trait]
impl ClientHandler for Master {
    // Process data lands here, including interrogation responses.
    async fn asdu(&self, _c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        if let TypeId::M_ME_NC_1 | TypeId::M_ME_TF_1 = pack.type_id() {
            for m in pack.get_measured_value_float()? {
                println!("ioa={} value={} quality={}", m.ioa, m.value, m.qds);
            }
        }
        Ok(())
    }
}

# #[tokio::main]
# async fn main() -> rs_iec60870_5::Result<()> {
let client = Client::new(Master, ClientOption::new().with_server("127.0.0.1:2404")?);
client.start()?;
client.wait_active().await;

client
    .interrogation_cmd(
        CauseOfTransmission::new(Cause::ACTIVATION),
        1,
        QualifierOfInterrogation::STATION,
    )
    .await
# }
```

A 104 connection starts in STOPDT and carries nothing until `STARTDT` is
confirmed. This crate sends it automatically on connect; disable that with
`ClientOption::with_auto_start_dt(false)` to drive activation by hand.

### IEC 101 and 103

Both run the FT1.2 link procedure over a serial port, or over TCP when the line
is reached through a terminal server. Only the transport changes — the handlers
and the ASDUs are identical:

```rust
use rs_iec60870_5::cs101::{Config, SerialConfig, TcpConfig, TransportType};

// A local serial port: 8E1 is the standard framing, and the default.
let mut cfg = Config::new();
cfg.serial = SerialConfig::new("/dev/ttyUSB0", 9600);
cfg.link_address = 1;

// ...or the same link inside a TCP stream.
let mut cfg = Config::new();
cfg.transport = TransportType::TcpClient;       // or TcpServer to listen
cfg.tcp = TcpConfig { address: "10.0.0.9:2400".into(), ..Default::default() };
# assert_eq!(cfg.transport_label(), "10.0.0.9:2400");
```

The serial transport needs the `serial` feature. TCP encapsulation is still
101/103 framing inside a pipe — not IEC 104 — so keep t₁ and t₂ generous enough
for the added network latency.

## Examples

Each is runnable and self-contained:

```sh
cargo run --example cs104_server                  # controlled station on :2404
cargo run --example cs104_client                  # master against it
cargo run --example cs104_server_special          # outstation that dials out (NAT)

cargo run --example cs101_server -- tcp:127.0.0.1:2404
cargo run --example cs101_client -- tcp:127.0.0.1:2404
cargo run --features serial --example cs101_client -- /dev/ttyUSB0 9600

cargo run --features serial --example cs103_client -- /dev/ttyUSB0 9600 3
```

Set `RUST_LOG=rs_iec60870_5=debug` for frame-level logging.

### Terminal explorer

[`examples/cs104_explorer`](examples/cs104_explorer) is an interactive IEC 104
master: connect to outstations, run interrogations and clock sync, build and
send control commands, and watch the received points beside a live protocol
log. It is a separate package, so its terminal UI dependencies are never
dependencies of this library.

```sh
cargo run -p cs104-explorer -- 127.0.0.1:2404
```

## Documentation

| Document | Contents |
|----------|----------|
| [User guide](docs/guide.md) | task-oriented tour: writing a master, an outstation, commands, time, errors, serial, TLS, testing, pitfalls |
| [`asdu` reference](docs/asdu.md) | the application layer: parameters, every type identification, builders, getters, quality, time tags |
| [`cs104` reference](docs/cs104.md) | IEC 104 client, server and reverse-connection station; k/w windows and t₀–t₃ |
| [`cs101` reference](docs/cs101.md) | IEC 101 primary and secondary; FT1.2, class buffering, balanced mode |
| [`cs103` reference](docs/cs103.md) | IEC 103 relay master: FUN/INF addressing, measurands, CP32 |
| [`filetransfer` reference](docs/filetransfer.md) | file transfer: the `F_*` ASDUs, the sender and receiver procedures, stores |
| [SKILL.md](SKILL.md) | condensed build guide for AI coding agents |
| [docs.rs](https://docs.rs/rs-iec60870-5) | generated API documentation |

## Features

| Feature | Default | Effect |
|---------|---------|--------|
| `cs104` | yes | IEC 60870-5-104 over TCP/IP |
| `cs101` | yes | IEC 60870-5-101 over FT1.2 |
| `cs103` | yes | IEC 60870-5-103 master (implies `cs101`) |
| `filetransfer` | yes | the file transfer procedures (types 120–126) on any endpoint |
| `serial` | no | real serial ports via `tokio-serial`; without it, 101 and 103 still work over their TCP transports |
| `tls` | no | TLS via `tokio-rustls` (the `ring` provider), for the `tls://` endpoints and TLS listeners |
| `serde` | no | `Serialize`/`Deserialize` on the ASDU types |
| `tz` | no | named IANA time zones (`TimeZone::Named`) via `chrono-tz`, for a device whose profile fixes a zone the host does not share |

The `asdu` application layer is always built, so a codec-only dependency can
turn every transport off.

## What is implemented

**Application layer (101/104).** Every process information type in the monitor
direction with and without CP24/CP56 time tags — single and double point, step
position, bit string, normalized, scaled and short float measured values,
integrated totals, protection events, packed single points with SCD. The
control direction: single, double and step commands, all three set-point
families and bit string commands, including the CP56Time2a variants. System
information: end of initialization, interrogation, counter interrogation, read,
clock synchronization, test, reset process, delay acquisition, and the parameter
commands. Sequence (SQ = 1) encoding, the 1/2/3-octet address widths, and
cause-of-transmission validation per type.

**cs104.** The APCI state machine: I/S/U frames, the k and w flow-control
windows, the t₀–t₃ timers, StartDT/StopDT and TestFR keep-alive, 15-bit sequence
numbers with wraparound. A listening server accepting any number of masters, a
master with automatic reconnection, and a reverse-connection outstation for NAT
traversal. Optional TLS.

**cs101.** FT1.2 framing with checksum and length validation; unbalanced mode
with correct per-station FCB tracking, class 1/2 buffering, ACD and DFC
signalling and multi-drop round-robin polling; balanced mode with both stations
transmitting spontaneously. Serial, TCP dial-out and TCP listen transports.

**cs103.** Master only: automatic link initialization (status, reset of the
communication unit), identification collection, automatic time synchronization
and general interrogation, cyclic measurand polling with event fetch on ACD,
general commands with RII-matched acknowledgements, multi-drop.

## Not implemented

* IEC 62351-5 security ASDUs (`S_*`) — enumerated only.
* `F_SC_NB_1` (127, query log) — enumerated only; the rest of the file transfer
  set (120–126) is implemented, see [`filetransfer`].
* Select-before-execute supervision is left to the application: command ASDUs
  reach the handler, which decides how to confirm and execute them. The S/E bit
  is available as `QualifierOfCommand::in_select`.
* cs103: the generic services (structured GIN/GDD/GID codecs), disturbance data
  transfer (ASDU 23–31), and the secondary (device) side.

## Interoperability

Verified against [`go-iecp5`](https://github.com/riclolsen/go-iecp5) in every
direction the two libraries can be paired — this crate's 104 master against the
Go outstation and vice versa, the same for 101, and both 103 masters driven
against a shared simulated relay. The tests assert per-information-object
equality, including CP56Time2a round-trips, SQ sequences, quality bits and the
sign extension of the 7-bit step position and 13-bit measurand fields. See
[`tests/interop/README.md`](tests/interop/README.md); they skip cleanly when Go
is unavailable.

The defaults (k = 12, w = 8, t₁ = 15 s, t₂ = 10 s, t₃ = 20 s, `PARAMS_WIDE` for
104 and `PARAMS_STANDARD_101` for 101) are chosen to interoperate with
other publicly available projects and conforming test sets.

### One deliberate difference from go-iecp5

go-iecp5 cannot size type identifications 58–64 (the CP56Time2a-tagged command
types), so it drops them on receipt. This crate encodes *and* decodes them per
the standard, which is a strict superset: a go-iecp5 peer will still not accept
them, so avoid those types when the other end is go-iecp5.

## Pitfalls

1. **`Params` must match on both peers.** A mismatch decodes as garbage — wrong
   types, wrong causes. Use `PARAMS_WIDE` for 104 and `PARAMS_STANDARD_101` for
   101 unless the device profile says otherwise.
2. **On a 104 master, interrogation *data* arrives at `ClientHandler::asdu`**,
   not `interrogation`, which sees only the mirrored `C_IC` confirmations.
3. Sends are queued. `Error::BufferFull` and `Error::SendQueueFull` mean back
   off and retry, not that anything was dropped.
4. An ASDU holds at most 249 octets. Batch large point sets across several
   calls, or use `is_sequence = true` for contiguous addresses.
5. Broadcast is `GLOBAL_COMMON_ADDR` (65535) even with 1-octet addressing — it
   maps to 255 on the wire — and is legal only for `C_IC`, `C_CI`, `C_CS` and
   `C_RP`.
6. Information object address 0 means "irrelevant" and is reserved for system
   commands; real points start at 1.
7. On a cs101 secondary, `send` **buffers**; in unbalanced mode the standard
   forbids unsolicited transmission, and the primary collects the data by
   polling.

## Releases

Prebuilt binaries for each release are attached to the
[GitHub release](https://github.com/dscsystems/rs-iec60870-5/releases): one
archive per platform, containing every runnable example plus `cs104-explorer`,
built with `serial` and `tls` enabled. Check a download against the
`SHA256SUMS` file published alongside them:

```sh
sha256sum -c SHA256SUMS --ignore-missing
```

Platforms built: Linux (gnu and musl) on x86-64, aarch64, armv7, armv6 and
i686; macOS on x86-64 and Apple silicon; Windows on x86-64 and aarch64; and,
best-effort, riscv64 Linux and x86-64 FreeBSD.

`.github/workflows/release.yml` runs the whole thing. Pushing a `v*` tag tests
and lints the workspace, builds the matrix, and publishes the release; a tag
containing `-alpha`, `-beta`, `-rc` or `-pre` is marked a prerelease.

```sh
git tag -a v0.1.0 -m "v0.1.0" && git push origin v0.1.0
```

Running it from the Actions tab (`workflow_dispatch`) builds the same matrix
and uploads the archives as workflow artifacts without publishing anything,
which is how to check a matrix change before committing to a tag.

## License

Source-available under the DSC Systems Source-Available License; see
[LICENSE](LICENSE). Inspection, evaluation, development and testing are free;
production and commercial use require a commercial license from DSC Systems.

Copyright © 2026 Ricardo Olsen / DSC Systems.
Contact: <https://www.linkedin.com/in/ricardo-olsen/>

[`asdu`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/
[`cs101`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/cs101/
[`cs103`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/cs103/
[`cs104`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/cs104/
