# Interoperability harness

These tests verify this crate against
[`github.com/riclolsen/go-iecp5`](https://github.com/riclolsen/go-iecp5), the
Go implementation this library was ported from. Each Go peer reports what it
sends and receives as JSON lines, so a failing assertion names the exact
information object that disagreed rather than just "the frames differ".

## Requirements

* the Go toolchain (1.25 or newer), and
* a checkout of go-iecp5 at `tests/interop/go-iecp5`.

```sh
git clone --depth 1 https://github.com/riclolsen/go-iecp5.git \
    tests/interop/go-iecp5
```

Both are optional: when either is missing the interop tests print `SKIP:` and
pass, so `cargo test` still works on a machine without Go. The checkout and the
compiled binaries are git-ignored.

## What is covered

| Test | Direction |
|------|-----------|
| `interop_go_iecp5` | this crate's 104 master ↔ the Go controlled station, and this crate's 104 controlled station ↔ the Go master |
| `interop_go_iecp5_cs101` | this crate's 101 primary ↔ the Go secondary, and this crate's 101 secondary ↔ the Go primary |
| `interop_go_iecp5_cs103` | both 103 masters driven against the same simulated relay |

The 101 and 103 peers run over the TCP encapsulation transport, so the full
FT1.2 procedure — link initialization, FCB tracking, class 1/2 polling, ACD
signalling and duplicate detection — is exercised without serial hardware.

IEC 60870-5-103 defines a master side only in both implementations, so there is
no client/server pairing to test. Instead `tests/common/relay.rs` simulates a
protection device, each master is driven against it in turn, and the two runs
are compared: the device must observe the same link procedure and the same
control-direction ASDUs, and both masters must decode the device's replies to
the same values.

## Running them

```sh
cargo test --test interop_go_iecp5
cargo test --test interop_go_iecp5_cs101
cargo test --test interop_go_iecp5_cs103
```

Set `IECP5_DEBUG=1` to turn on go-iecp5's own protocol logging, and
`RUST_LOG=rs_iec60870_5=debug` for this crate's.

## Layout

```
tests/interop/
├── go-iecp5/          the reference implementation (git-ignored)
└── go/
    ├── gosrv/         104 controlled station
    ├── gocli/         104 master
    ├── go101srv/      101 secondary station
    ├── go101cli/      101 primary station
    └── go103cli/      103 master
```

The Go module resolves go-iecp5 through a `replace` directive pointing at the
sibling checkout, so the harness builds offline once cloned.
