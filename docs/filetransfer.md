# File transfer

The `filetransfer` module implements IEC 60870-5-101 §7.4.11 in the monitor
direction: the outstation serves files to a master. It works through `Connect`
on IEC 101 and IEC 104. Types 120–126 have `Asdu` constructors, non-consuming
getters, and `ConnectExt` send helpers.

Use one `Sender` or `Receiver` per connection. Store it in a Tokio mutex in
an application handler, and pass file ASDUs to its `handle` method. `Ok(false)`
means the ASDU belongs to another service. Call `abort` after connection loss.
Sends await queue space for at most 30 seconds; the application dispatcher is
separate from the protocol loop, so link acknowledgements continue meanwhile.

```rust,no_run
use std::sync::Arc;
use rs_iec60870_5::{asdu::*, filetransfer::*};

async fn serve(c: &dyn Connect, request: &Asdu) -> rs_iec60870_5::Result<()> {
    let store = Arc::new(MemStore::new());
    store.write(100, 2, b"disturbance record")?;
    let mut sender = Sender::new(store);
    sender.handle(c, request).await?;
    // Keep sender in the handler for subsequent requests.
    Ok(())
}

async fn fetch(c: &dyn Connect) -> rs_iec60870_5::Result<()> {
    let mut receiver = Receiver::new(None);
    receiver.request_directory(c, 1).await?;
    receiver.request_file(c, 1, 100, 2).await?;
    // Keep receiver in the handler and call handle(c, pack) for each file ASDU.
    // take_directory() drains directory entries; take_completed() drains files.
    Ok(())
}
```

`Sender::offer` announces a stored file. The receiver accepts offers automatically;
`set_auto_accept(false)` leaves selection to the application. `Store` supports
list/read/write/delete and must be safe for concurrent use. `MemStore` copies
bytes and sorts directory entries by IOA and file name. A receiver's store is
optional; completed bytes are also available through `take_completed`.

The sender splits files into sections (4096 octets by default) and segments
(236 octets for standard IEC 104). Each section has an arithmetic checksum
modulo 256. A checksum or section-length mismatch triggers a negative section
acknowledgement and retransmission. Directory replies are split across ASDUs
without losing the last-entry flag. Empty files contain one empty section.

Limits: one transfer per service instance, at most 255 sections, file lengths
up to 0xffffff octets, monitor direction only. Adjust `set_section_size` for
large files. The announced file length is descriptive; the receiver validates
section lengths and checksums. Query-log type 127 and IEC 103 disturbance
transfer are separate services and remain unsupported.

Verification covers multi-section, empty, offered and corrupted files, small
queues on an IEC 101 connection, downloads in both directions against the Go
reference, and downloads plus all seven codec layouts against lib60870-C.
See [the interoperability harness](../tests/interop/README.md) for pinned
reference revisions and commands.
