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

Both are optional for local development: when either is missing the interop
tests print `SKIP:` and pass. A present checkout that fails to build is always
a test failure. Set `IEC60870_INTEROP_REQUIRED=1` to make missing dependencies
fail as well; CI does this. The checkout and binaries are git-ignored.

The current parity baseline is go-iecp5 commit
`778fe38466157a0acf7a5db39949ced1b2bfa68b` (2026-09-29). The file peers require
that revision or newer. The CI workflow pins it for reproducibility.

## What is covered

| Test | Direction |
|------|-----------|
| `interop_go_iecp5` | this crate's 104 master ↔ the Go controlled station, and this crate's 104 controlled station ↔ the Go master |
| `interop_go_iecp5_cs101` | this crate's 101 primary ↔ the Go secondary, and this crate's 101 secondary ↔ the Go primary |
| `interop_go_iecp5_cs103` | both 103 masters driven against the same simulated relay |
| `interop_lib60870` | 104 client/server exchanges with lib60870-C; file downloads with both lib60870 and Go; all seven file codecs compared with lib60870; 101 primary/secondary exchanges through PTYs on Unix with `serial` enabled |

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

## lib60870-C

The tested revision is `7a388e3e133999e1ca77ba7521d55d074b7cd2bc` from
[mz-automation/lib60870](https://github.com/mz-automation/lib60870).
It is an external test dependency, not linked into the Rust library.

```sh
git clone https://github.com/mz-automation/lib60870.git /tmp/lib60870
git -C /tmp/lib60870 checkout 7a388e3e133999e1ca77ba7521d55d074b7cd2bc
make -C /tmp/lib60870/lib60870-C -j2
LIB60870_ROOT=/tmp/lib60870/lib60870-C IEC60870_INTEROP_REQUIRED=1 \
  cargo test --all-features
```

`LIB60870_ROOT` names the directory containing `src/inc/api` and the built
`build/liblib60870.a`. `interop_lib60870` compiles `lib60870/peer.c` with `cc`
and links that archive. Without the variable the lib60870 cases skip unless
strict mode is enabled. A configured build failure always fails the test.
Unix serial cases need Python 3; `lib60870/pty_bridge.py` joins two raw pseudo
terminals, exercising both real serial backends, FT1.2 initialization, ACD,
class polling, interrogation and select-command confirmation. This does not
simulate hardware parity faults or physical line timing. Both 101 directions
use COT=1, CA=1, IOA=2 and one-octet link addresses.

The C peer uses lib60870's public information-object codecs and protocol
engines. Its file request/acknowledgement procedure is implemented by the test
peer; it does not use lib60870's optional file-server plugin. TCP tests verify
actual downloads both ways, including maximum-size segments and multiple
sections. The codec comparison covers types 120–126, including CP56 directory
times with an explicit day-of-week (lib60870 otherwise leaves that optional
field zero). The Go file peers use go-iecp5's actual Sender/Receiver services.

Local regression suites add empty files, automatic offers, corrupted segments
with section retry, large directory batching, a one-ASDU IEC 101 class buffer,
STOPDT draining and cancellation by STARTDT, malformed APCI rejection,
broadcast handling, DFC backpressure, and local DST ambiguity.

Coverage is interoperability evidence for these exchanges, not IEC 60870-5-604
conformance certification or a hardware test. IEC 103 uses the Go/relay harness;
lib60870-C supplies IEC 101/104 rather than an IEC 103 reference.
