# cs104-explorer

An interactive terminal IEC 60870-5-104 master, built on
[`rs-iec60870-5`](../..) and [ratatui](https://ratatui.rs).

Connect to controlled stations, issue requests, build and send control
commands, and watch the received points beside a live protocol log.

```
IEC 60870-5-104 Explorer   ● active
server 127.0.0.1:2404  •  common addr 1  •  params 104-wide (COT2/CA2/IOA3)  •  protocol log off
 1 Points  2 Log  3 Send Command
┌ Points (4) ─────────────────────────────────────────────────────────────────┐
│ IOA      Type         Value           Quality      Cause              Time  │
│ 100      M_SP_NA_1    on              Good         InterrogatedBy…     …    │
│ 101      M_SP_NA_1    off             Invalid      InterrogatedBy…     …    │
│ 400      M_ME_TF_1    23.25           Good         Spontaneous     23:27:30 │
│ 500      M_IT_TB_1    34 (seq 0)                   Spontaneous     23:27:30 │
└─────────────────────────────────────────────────────────────────────────────┘
1/2/3 tabs • c connect • x disconnect • e edit target • g GI • …
```

This is a **separate package** — a workspace member, but not part of
`rs-iec60870-5`, so its terminal UI dependencies never reach anyone depending on
the library. It mirrors the `_examples/cs104_explorer` module of
[go-iecp5](https://github.com/riclolsen/go-iecp5).

## Running it

From this directory, or from the repository root with `-p cs104-explorer`:

```sh
cargo run                      # start; set the target and connect in the UI
cargo run -- 10.0.0.5:2404     # preset the server address
```

Against the library's own outstation, from two terminals at the repository
root:

```sh
cargo run --example cs104_server         # the outstation
cargo run -p cs104-explorer -- 127.0.0.1:2404
```

It also talks to any conforming 104 outstation — `lib60870`, OpenMUC j60870, a
real RTU, or a test set.

## Keys

| Key | Action |
|-----|--------|
| `1` `2` `3`, `tab` | switch between Points, Log and Send Command |
| `e` | edit the target address and common address |
| `c` / `x` | connect / disconnect |
| `s` / `S` | send `STARTDT act` / `STOPDT act` by hand |
| `g` | general interrogation |
| `C` | counter interrogation |
| `y` | clock synchronization |
| `t` | test command |
| `z` | reset process |
| `i` | edit and send a control command (on the Send tab) |
| `v` | toggle protocol-level logging into the Log panel |
| `↑` `↓`, `home`, `end` | navigate the points table or scroll the log |
| `ctrl-l` / `ctrl-r` | clear the log / clear the points |
| `q`, `ctrl-c` | quit |

In the connection editor and the command builder: `tab` moves between fields,
`←`/`→` changes an option, `enter` applies or sends, `esc` goes back.

## The Send Command tab

Press `i` to edit. The command kind cycles with `←`/`→`; the value field is
interpreted per kind:

| Command | Value |
|---------|-------|
| Single, double | `on` / `off` (also `1`/`0`, `true`/`false`, `close`) |
| Step | `up` / `down` |
| Setpoint float, scaled | a number |
| Setpoint normalized | a fraction in `-1..1`, clamped |
| Bit string | 32 bits, decimal or `0x`-prefixed |
| Read | ignored — the command only names a point |

**Mode** is the S/E bit: `Execute` operates, `Select` only selects, which is how
select-before-execute is driven. **Qualifier** is the pulse qualifier (`QOC`) for
commands, or the setpoint qualifier (`QOS`) for setpoints.

Commands are queued: the log shows `-> queued` when the ASDU was accepted by the
send queue, and the outstation's `ActivationCon` and `ActivationTerm` arrive
afterwards as received ASDUs.

## How it is put together

| File | Role |
|------|------|
| `main.rs` | terminal setup, the event loop, and the bridge from `tracing` into the log panel |
| `app.rs` | state, key handling, client lifecycle and the request actions |
| `handler.rs` | the `ClientHandler` that feeds the UI, and ASDU-to-row decoding |
| `form.rs` | the command builder: what can be sent, and how the value field parses |
| `ui.rs` | rendering |
| `event.rs` | the messages the protocol tasks push to the UI |
| `e2e.rs` | headless end-to-end tests against a real outstation |

The protocol runs on its own Tokio tasks and never touches the terminal:
everything reaches the UI as an `Event` over a channel. That includes the
library's `tracing` output, which a custom subscriber layer redirects into the
log panel — printing to stdout would corrupt the alternate screen.

## Tests

```sh
cargo test           # here, or `cargo test -p cs104-explorer` from the root
```

40 tests: value parsing and command building, ASDU decoding for every
monitor-direction family, key handling and panel state, rendering at several
terminal sizes, and four end-to-end tests that drive the whole app headlessly
against a real `rs-iec60870-5` outstation — connect, interrogate, and send a
command, checking what the outstation actually received.

## License

Source-available under the DSC Systems Source-Available License; see
[LICENSE](../../LICENSE).

Copyright © 2026 Ricardo Olsen / DSC Systems.
