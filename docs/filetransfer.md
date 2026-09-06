# `filetransfer` reference

File transfer moves a file from a controlled station to a master — most often a
disturbance record off a protection relay. It is a multi-ASDU procedure with its
own handshake, so it lives in its own module rather than in an endpoint.

Two layers:

* **The ASDUs**, in [`asdu`](asdu.md): the `F_*` types and their qualifiers.
  Use these directly to speak to a device with a non-standard procedure.
* **The procedures**, in `filetransfer`: a [`Sender`] for the outstation side
  and a [`Receiver`] for the master side, which run the handshake for you.

Both procedures are transport agnostic. They act on an
[`asdu::Connect`](asdu.md), so the same code works with a `cs104` or a `cs101`
endpoint.

Enabled by the `filetransfer` feature, on by default.

## The ASDUs

| Type | Id | Direction | Purpose |
|------|----|-----------|---------|
| `F_FR_NA_1` | 120 | monitor | file ready |
| `F_SR_NA_1` | 121 | monitor | section ready |
| `F_SC_NA_1` | 122 | control | call directory, select file, call file, call section |
| `F_LS_NA_1` | 123 | monitor | last section, last segment |
| `F_AF_NA_1` | 124 | control | acknowledge file, acknowledge section |
| `F_SG_NA_1` | 125 | monitor | segment |
| `F_DR_TA_1` | 126 | monitor | directory |

`F_SC_NB_1` (127, query log) is not implemented.

Every type but the directory carries exactly one information object (SQ = 0).

### Structure of a transfer

A file is cut into **sections**, and each section into **segments**:

```text
file ─┬─ section 1 ─┬─ segment ─┬─ …            NOS is one octet: 255 sections
      │             │           └─ segment      LOS is one octet: 255 octets
      │             └─ checksum (CHS)           per segment, and the ASDU size
      ├─ section 2 …                            bounds it further
      └─ section n
```

The **segment** is the transport unit, bounded by
`Params::max_segment_size()` — the ASDU maximum minus the identifier, the IOA
and the four octets of NOF, NOS and LOS, and never more than 255 because LOS is
a single octet. With the IEC 104 parameters that is 232 octets.

The **section** is the unit that carries a checksum (CHS), the arithmetic sum
of its segment octets modulo 256. Smaller sections detect corruption earlier at
the cost of more round trips. A section may be up to 16 MB (its length is a
3-octet element), but a file may have at most **255 sections**, because the
name of section (NOS) is a single octet.

### Qualifiers

| Element | Type | Layout |
|---------|------|--------|
| FRQ | `FileReadyQualifier` | 7-bit qualifier + P/N in bit 7 |
| SRQ | `SectionReadyQualifier` | 7-bit qualifier + "not ready" in bit 7 |
| SCQ | `SelectAndCallQualifier` | `ScqAction` in bits 0–3, `FileError` in bits 4–7 |
| AFQ | `AckFileOrSectionQualifier` | `AfqAction` in bits 0–3, `FileError` in bits 4–7 |
| LSQ | `LastSectionQualifier` | one octet; `is_end_of_file()` separates file from section |
| SOF | `StatusOfFile` | 5-bit status + LFD, FOR and FA |

`file_checksum(&[u8]) -> u8` computes CHS.

## The procedures

### Outstation: serving files

```rust,no_run
use std::sync::Arc;
use rs_iec60870_5::asdu::{Asdu, Connect, NameOfFile};
use rs_iec60870_5::cs104::{Server, ServerHandler};
use rs_iec60870_5::filetransfer::{MemStore, Sender};

struct Outstation {
    files: Arc<Sender>,
}

#[async_trait::async_trait]
impl ServerHandler for Outstation {
    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        // The transfer runs itself; anything else is the application's.
        if self.files.handle(c, pack).await? {
            return Ok(());
        }
        Ok(())
    }
}

# fn main() -> std::io::Result<()> {
let store = Arc::new(MemStore::new());
store.insert(100, NameOfFile::DISTURBANCE_DATA, std::fs::read("record.bin")?);

let files = Arc::new(Sender::new(store));
files.set_section_size(4096);
let srv = Server::new(Outstation { files });
# let _ = srv;
# Ok(())
# }
```

`Sender::offer(conn, ca, ioa, nof)` announces a file with `F_FR_NA_1`, which a
master usually answers by selecting it. Everything after that — the directory
reply, the section announcements, the segments, the retransmission of a section
the master rejected — is driven by `handle`.

### Master: fetching files

```rust,no_run
# use std::sync::Arc;
# use rs_iec60870_5::asdu::{Connect, NameOfFile};
# use rs_iec60870_5::filetransfer::{MemStore, Receiver};
# async fn f(c: &dyn Connect) -> rs_iec60870_5::Result<()> {
let files = Arc::new(Receiver::new(Arc::new(MemStore::new())));
files.set_file_handler(Box::new(|entry, data| {
    println!("IOA {}: {} octets", entry.ioa, data.len());
}));
files.set_directory_handler(Box::new(|ca, dir| {
    for e in dir {
        println!("{ca}: file {} is {} octets", e.nof, e.length_of_file);
    }
}));

files.request_directory(c, 1).await?;
files.request_file(c, 1, 100, NameOfFile::DISTURBANCE_DATA).await?;
# Ok(()) }
```

Feed every received ASDU to `handle`, exactly as on the outstation side. The
section requests, the checksum verification and the acknowledgements are then
handled for you, and the file handler fires when the file is complete.

By default a file the outstation *announces* is selected automatically. Call
`set_auto_accept(false)` to decide per file and call `request_file` yourself.

One transfer runs at a time on each side; a second `request_file` while one is
running returns `Error::TransferBusy`.

## Stores

Files are held by a `Store`:

```rust,ignore
#[async_trait::async_trait]
pub trait Store: Send + Sync + 'static {
    async fn list(&self) -> Result<Vec<Entry>>;
    async fn read(&self, ioa: InfoObjAddr, nof: NameOfFile) -> Result<Vec<u8>>;
    async fn write(&self, ioa: InfoObjAddr, nof: NameOfFile, data: Vec<u8>) -> Result<()>;
    async fn delete(&self, ioa: InfoObjAddr, nof: NameOfFile) -> Result<()>;
}
```

`MemStore` is the in-memory implementation. Implement the trait for any other
backing — a directory on disk, a database. `read` and `delete` return
`Error::FileNotFound` for an unknown file, which the sender turns into a
negative acknowledgement carrying `FileError::UNEXPECTED_NAME_OF_FILE`.

A `Receiver` built with `Receiver::without_store()` reports completed files
only through the file handler.

## Errors

| Error | Meaning |
|-------|---------|
| `FileNotFound` | the store has no such file, or the outstation refused it with a negative FRQ |
| `NoTransfer` | an ASDU arrived for a transfer that is not running, or naming a section that does not exist |
| `TransferBusy` | a second transfer was requested while one is running |
| `FileChecksum` | a section's checksum did not match; it is negatively acknowledged and served again |
| `FileServiceUnsupported` | an ASDU of the other direction, or `F_SC_NB_1` |

An error from `handle` is a report, not a reason to tear down the link: the
component has already told the peer whatever the procedure requires.

## Things to know

* **255 sections is the hard limit.** `set_section_size` is a preference: when
  a file would need more sections than NOS can name, the sections are grown to
  fit. Numbering more than 255 would wrap and make the receiver re-request a
  section it has already had — a transfer that never finishes.
* **Segments are sized from the connection's parameters**, so a transfer over a
  narrow 101 link automatically uses smaller segments than one over 104.
* **A checksum mismatch costs one section, not the file.** The receiver
  discards the section, negatively acknowledges it, and the sender serves it
  again.
* **An empty file is still a transfer**: one empty section, no segments, and
  the closing markers. It completes rather than hanging.
* **Bulk data fills the send queue.** A transfer of any size pushes hundreds of
  ASDUs; on a `cs104::Server`, prefer `Server::send_wait` or the
  [`waiting`](cs104.md) wrapper so a master that is briefly behind does not
  silently lose segments.
