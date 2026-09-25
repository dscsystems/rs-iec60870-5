# `asdu` — application layer reference

The `asdu` module implements the IEC 60870-5-101/-104 application layer: the
Application Service Data Unit with its data unit identifier, the standard
information object types, quality descriptors and time tags. Both transport
modules — [`cs104`](cs104.md) and [`cs101`](cs101.md) — exchange
[`Asdu`] values with your application through the [`Connect`] trait.

```text
ASDU = data unit identifier + information objects
       | type ID | variable structure | cause of transmission | common address | information objects... |
bytes |    1    |         1          |        1..2           |      1..2      |          n             |
```

## System parameters

[`Params`] fixes the on-wire width of the identifier fields. **Both peers must
use identical parameters** or every ASDU will be mis-parsed.

| Field | Meaning | Allowed |
|-------|---------|---------|
| `cause_size` | cause of transmission octets; 2 adds the originator address | 1, 2 |
| `orig_address` | originator address, `1..=255` or 0 for the default | any |
| `common_addr_size` | common (station) address octets | 1, 2 |
| `info_obj_addr_size` | information object address octets | 1, 2, 3 |
| `info_obj_time_zone` | zone used to encode and decode CP24/CP56 tags | [`TimeZone`] |
| `allow_trailing_octets` | accept an ASDU longer than its qualifier accounts for, discarding the surplus | `false` by default |

`allow_trailing_octets` is off by default, and should stay off. An ASDU's
length is fixed by the frame that carries it, its object count by the variable
structure qualifier and its object size by the type identification, so a
conforming sender cannot produce a surplus octet; one that arrives means the
frame is not what it claims, and accepting it means acting on a command nobody
can account for. Decoding such an ASDU yields [`Error::TrailingOctets`]. Turn
it on only for a device known to pad, knowing that a truncated interrogation
reply then looks the same as a complete one.

Predefined:

| Constant | COT | CA | IOA | Used by |
|----------|-----|----|----|---------|
| `PARAMS_WIDE` = `PARAMS_STANDARD_104` | 2 | 2 | 3 | **IEC 104 standard**, the `cs104` default |
| `PARAMS_STANDARD_101` | 1 | 1 | 2 | **IEC 101 standard**, the `cs101` default |
| `PARAMS_NARROW` | 1 | 1 | 1 | smallest legal configuration |

`TimeZone` is `Utc` (the default, and what the standard recommends), `Local`,
`Fixed(FixedOffset)`, or `Named(chrono_tz::Tz)` with the `tz` feature — an IANA
zone that does not depend on how the host is configured, for a device whose
profile fixes one the host does not share.

The zone decides the **SU (summer time) bit** of a CP56Time2a and CP32Time2a
tag as well as the wall clock reading. `Local` and `Named` set it while the
zone is on summer time, and honour it on decode; `Utc` and `Fixed` never do,
because neither observes summer time. That bit is what resolves the hour that
occurs twice when the clocks go back — without it the second reading decodes as
the first, which puts an event an hour before its cause.

## Identifier

Every ASDU carries an [`Identifier`]:

```rust
use rs_iec60870_5::asdu::*;

let id = Identifier {
    type_id: TypeId::M_SP_NA_1,
    variable: VariableStruct { number: 1, is_sequence: false },
    coa: CauseOfTransmission::new(Cause::SPONTANEOUS),
    orig_addr: 0,          // on the wire only when cause_size == 2
    common_addr: 1,        // the station; 0 is invalid, 65535 broadcast
};
# assert_eq!(id.type_id, TypeId::M_SP_NA_1);
```

`Identifier::new(type_id, variable, coa, common_addr)` fills the originator
address with 0.

### Cause of transmission

[`CauseOfTransmission`] is a [`Cause`] plus the T (test) and P/N (negative)
flags:

```rust
# use rs_iec60870_5::asdu::*;
let coa = CauseOfTransmission::new(Cause::SPONTANEOUS);
let rejected = CauseOfTransmission::new(Cause::ACTIVATION_CON).negative();
let under_test = CauseOfTransmission::new(Cause::PERIODIC).test();
# assert!(rejected.is_negative && under_test.is_test);
```

Frequently used causes: `PERIODIC` (1), `BACKGROUND` (2), `SPONTANEOUS` (3),
`INITIALIZED` (4), `REQUEST` (5), `ACTIVATION` (6), `ACTIVATION_CON` (7),
`DEACTIVATION` (8), `DEACTIVATION_CON` (9), `ACTIVATION_TERM` (10),
`RETURN_INFO_REMOTE` (11), `RETURN_INFO_LOCAL` (12),
`INTERROGATED_BY_STATION` (20), `INTERROGATED_BY_GROUP1` (21) through
`INTERROGATED_BY_GROUP16` (36), `REQUEST_BY_GENERAL_COUNTER` (37) through
`REQUEST_BY_GROUP4_COUNTER` (41), `UNKNOWN_TYPE_ID` (44), `UNKNOWN_COT` (45),
`UNKNOWN_CA` (46), `UNKNOWN_IOA` (47).

Helpers: `Cause::interrogated_by_group(n)` for `n` in `1..=16`,
`Cause::is_interrogation()`, `Cause::is_counter_request()`.

### Common address

`INVALID_COMMON_ADDR` (0) is never a usable station address.
`GLOBAL_COMMON_ADDR` (65535) is the broadcast address — **use it even with
1-octet addressing**, where it is mapped to 255 on the wire automatically.
Broadcast is legal only for `C_IC_NA_1`, `C_CI_NA_1`, `C_CS_NA_1` and
`C_RP_NA_1`.

## Type identifications

Monitor direction (device to master):

| Type | Value | Content | Time tag |
|------|-------|---------|----------|
| `M_SP_NA_1` / `M_SP_TA_1` / `M_SP_TB_1` | 1/2/30 | single point | — / CP24 / CP56 |
| `M_DP_NA_1` / `M_DP_TA_1` / `M_DP_TB_1` | 3/4/31 | double point | — / CP24 / CP56 |
| `M_ST_NA_1` / `M_ST_TA_1` / `M_ST_TB_1` | 5/6/32 | step position | — / CP24 / CP56 |
| `M_BO_NA_1` / `M_BO_TA_1` / `M_BO_TB_1` | 7/8/33 | 32-bit bit string | — / CP24 / CP56 |
| `M_ME_NA_1` / `M_ME_TA_1` / `M_ME_TD_1` | 9/10/34 | normalized value | — / CP24 / CP56 |
| `M_ME_NB_1` / `M_ME_TB_1` / `M_ME_TE_1` | 11/12/35 | scaled value | — / CP24 / CP56 |
| `M_ME_NC_1` / `M_ME_TC_1` / `M_ME_TF_1` | 13/14/36 | short float | — / CP24 / CP56 |
| `M_IT_NA_1` / `M_IT_TA_1` / `M_IT_TB_1` | 15/16/37 | integrated totals | — / CP24 / CP56 |
| `M_EP_TA_1` / `M_EP_TD_1` | 17/38 | protection event | CP24 / CP56 |
| `M_EP_TB_1` / `M_EP_TE_1` | 18/39 | packed protection start events | CP24 / CP56 |
| `M_EP_TC_1` / `M_EP_TF_1` | 19/40 | packed output circuit information | CP24 / CP56 |
| `M_PS_NA_1` | 20 | packed single points with SCD | — |
| `M_ME_ND_1` | 21 | normalized value without quality | — |
| `M_EI_NA_1` | 70 | end of initialization | — |

Control direction (master to device):

| Type | Value | Content |
|------|-------|---------|
| `C_SC_NA_1` / `C_SC_TA_1` | 45/58 | single command (/ CP56) |
| `C_DC_NA_1` / `C_DC_TA_1` | 46/59 | double command (/ CP56) |
| `C_RC_NA_1` / `C_RC_TA_1` | 47/60 | regulating step command (/ CP56) |
| `C_SE_NA_1` / `C_SE_TA_1` | 48/61 | set-point, normalized (/ CP56) |
| `C_SE_NB_1` / `C_SE_TB_1` | 49/62 | set-point, scaled (/ CP56) |
| `C_SE_NC_1` / `C_SE_TC_1` | 50/63 | set-point, short float (/ CP56) |
| `C_BO_NA_1` / `C_BO_TA_1` | 51/64 | 32-bit bit string (/ CP56) |
| `C_IC_NA_1` | 100 | interrogation command |
| `C_CI_NA_1` | 101 | counter interrogation command |
| `C_RD_NA_1` | 102 | read command |
| `C_CS_NA_1` | 103 | clock synchronization command |
| `C_TS_NA_1` / `C_TS_TA_1` | 104/107 | test command (/ CP56) |
| `C_RP_NA_1` | 105 | reset process command |
| `C_CD_NA_1` | 106 | delay acquisition command (101 only) |
| `P_ME_NA_1` / `P_ME_NB_1` / `P_ME_NC_1` | 110/111/112 | parameter of measured value |
| `P_AC_NA_1` | 113 | parameter activation |

`TypeId` is a newtype over the wire octet, so private and reserved
identifications (`128..=255`) round-trip unchanged. `TypeId::name()` gives the
mnemonic or `None`; `TypeId::info_obj_size()` gives the element size.

The file transfer types (120–126) are implemented — see the `filetransfer`
module for the procedures that drive them, and `docs/filetransfer.md` for the
ASDUs themselves. `F_SC_NB_1` (127, query log) and the IEC 62351-5 security
types are enumerated but not implemented.

## Building ASDUs

Every builder is an associated function on [`Asdu`] that takes the parameters
and returns `Result<Asdu>`. They validate the cause of transmission against the
standard and return [`Error::CmdCause`] when it is not permitted.

```rust
use rs_iec60870_5::asdu::*;

# fn main() -> rs_iec60870_5::Result<()> {
// Monitor direction. `is_sequence` packs consecutive addresses (SQ = 1).
let a = Asdu::single(
    PARAMS_WIDE,
    false,
    CauseOfTransmission::new(Cause::SPONTANEOUS),
    1,
    &[SinglePointInfo::new(100, true)],
)?;

// With a time tag; never a sequence.
let a = Asdu::single_cp56time2a(
    PARAMS_WIDE,
    CauseOfTransmission::new(Cause::SPONTANEOUS),
    1,
    &[SinglePointInfo { ioa: 100, value: true, time: Some(chrono::Utc::now()), ..Default::default() }],
)?;

// Control direction. The type identification selects the time-tag variant.
let a = Asdu::single_cmd(
    PARAMS_WIDE,
    TypeId::C_SC_NA_1,
    CauseOfTransmission::new(Cause::ACTIVATION),
    1,
    SingleCommandInfo { ioa: 6000, value: true, ..Default::default() },
)?;
# let _ = a;
# Ok(())
# }
```

### Monitor direction

| Family | Untagged | CP24Time2a | CP56Time2a |
|--------|----------|------------|------------|
| single point | `single` | `single_cp24time2a` | `single_cp56time2a` |
| double point | `double` | `double_cp24time2a` | `double_cp56time2a` |
| step position | `step` | `step_cp24time2a` | `step_cp56time2a` |
| bit string | `bitstring32` | `bitstring32_cp24time2a` | `bitstring32_cp56time2a` |
| normalized | `measured_value_normal` | `measured_value_normal_cp24time2a` | `measured_value_normal_cp56time2a` |
| normalized, no quality | `measured_value_normal_no_quality` | — | — |
| scaled | `measured_value_scaled` | `measured_value_scaled_cp24time2a` | `measured_value_scaled_cp56time2a` |
| short float | `measured_value_float` | `measured_value_float_cp24time2a` | `measured_value_float_cp56time2a` |
| integrated totals | `integrated_totals` | `integrated_totals_cp24time2a` | `integrated_totals_cp56time2a` |
| protection event | — | `event_of_protection_equipment_cp24time2a` | `event_of_protection_equipment_cp56time2a` |
| packed start events | — | `packed_start_events_cp24time2a` | `packed_start_events_cp56time2a` |
| packed output circuit | — | `packed_output_circuit_info_cp24time2a` | `packed_output_circuit_info_cp56time2a` |
| packed single point, SCD | `packed_single_point_with_scd` | — | — |
| end of initialization | `end_of_initialization` | — | — |

Each family also has a `*_with_type` form taking an explicit `TypeId`, for
building a variant the convenience wrappers do not name.

**Permitted causes.** `single`, `double`, `step` and
`packed_single_point_with_scd` accept `BACKGROUND`, `SPONTANEOUS`, `REQUEST`,
`RETURN_INFO_REMOTE`, `RETURN_INFO_LOCAL` and the interrogation causes; their
time-tagged variants drop `BACKGROUND`. The measured-value families accept
`PERIODIC`, `BACKGROUND`, `SPONTANEOUS`, `REQUEST` and the interrogation
causes, and their tagged variants only `SPONTANEOUS` and `REQUEST`.
`bitstring32` matches the measured values but without `PERIODIC`.
`integrated_totals` accepts `SPONTANEOUS` and the counter request causes only.
The protection families accept `SPONTANEOUS` only.

### Control direction

```rust
# use rs_iec60870_5::asdu::*;
# fn f() -> rs_iec60870_5::Result<()> {
# let (p, coa, ca) = (PARAMS_WIDE, CauseOfTransmission::new(Cause::ACTIVATION), 1);
Asdu::single_cmd(p, TypeId::C_SC_NA_1, coa, ca, SingleCommandInfo::default())?;
Asdu::double_cmd(p, TypeId::C_DC_NA_1, coa, ca, DoubleCommandInfo::default())?;
Asdu::step_cmd(p, TypeId::C_RC_NA_1, coa, ca, StepCommandInfo::default())?;
Asdu::setpoint_cmd_normal(p, TypeId::C_SE_NA_1, coa, ca, SetpointCommandNormalInfo::default())?;
Asdu::setpoint_cmd_scaled(p, TypeId::C_SE_NB_1, coa, ca, SetpointCommandScaledInfo::default())?;
Asdu::setpoint_cmd_float(p, TypeId::C_SE_NC_1, coa, ca, SetpointCommandFloatInfo::default())?;
Asdu::bits_string32_cmd(p, TypeId::C_BO_NA_1, coa, ca, BitsString32CommandInfo::default())?;
# Ok(())
# }
```

Pass the `_TA_1` type constant instead to get the CP56Time2a variant, in which
case the info struct's `time` field is encoded. Commands accept `ACTIVATION` and
`DEACTIVATION` only.

### System and parameter commands

```rust
# use rs_iec60870_5::asdu::*;
# fn f() -> rs_iec60870_5::Result<()> {
# let (p, coa, ca) = (PARAMS_WIDE, CauseOfTransmission::new(Cause::ACTIVATION), 1);
Asdu::interrogation_cmd(p, coa, ca, QualifierOfInterrogation::STATION)?;
Asdu::counter_interrogation_cmd(p, coa, ca, QualifierCountCall {
    request: QccRequest::TOTAL, freeze: QccFreeze::READ })?;
Asdu::read_cmd(p, coa, ca, 400)?;                       // cause forced to Request
Asdu::clock_synchronization_cmd(p, coa, ca, chrono::Utc::now())?;
Asdu::test_command(p, coa, ca)?;
Asdu::test_command_cp56time2a(p, coa, ca, chrono::Utc::now())?;
Asdu::reset_process_cmd(p, coa, ca, QualifierOfResetProcessCmd::GENERAL_RESET)?;
Asdu::delay_acquire_command(p, coa, ca, 2500)?;         // IEC 101 only
Asdu::parameter_normal(p, coa, ca, ParameterNormalInfo::default())?;
Asdu::parameter_scaled(p, coa, ca, ParameterScaledInfo::default())?;
Asdu::parameter_float(p, coa, ca, ParameterFloatInfo::default())?;
Asdu::parameter_activation(p, coa, ca, ParameterActivationInfo::default())?;
Asdu::end_of_initialization(p, coa, ca, 0, CauseOfInitial::default())?;
# Ok(())
# }
```

Several of these force the cause the standard requires — counter interrogation,
clock synchronization, test, reset process and parameter commands to
`ACTIVATION`, read to `REQUEST`, and end of initialization to `INITIALIZED` —
so passing anything else is corrected rather than rejected.

## Decoding received ASDUs

Switch on `pack.type_id()` and call the matching getter. **Getters are
non-destructive**: they borrow the payload and can be called in any order, any
number of times, and freely mixed with `reply_mirror`.

```rust
use rs_iec60870_5::asdu::*;

fn handle(pack: &Asdu) -> rs_iec60870_5::Result<()> {
    match pack.type_id() {
        TypeId::M_SP_NA_1 | TypeId::M_SP_TA_1 | TypeId::M_SP_TB_1 => {
            for p in pack.get_single_point()? {
                // p.ioa, p.value (bool), p.qds, p.time (None when untagged)
                let _ = (p.ioa, p.value, p.qds, p.time);
            }
        }
        TypeId::C_SC_NA_1 | TypeId::C_SC_TA_1 => {
            let cmd = pack.get_single_cmd()?;
            let _ = (cmd.ioa, cmd.value, cmd.qoc.in_select, cmd.qoc.qual);
        }
        _ => {}
    }
    Ok(())
}
```

Available getters: `get_single_point`, `get_double_point`, `get_step_position`,
`get_bitstring32`, `get_measured_value_normal`, `get_measured_value_scaled`,
`get_measured_value_float`, `get_integrated_totals`,
`get_event_of_protection_equipment`, `get_packed_start_events`,
`get_packed_output_circuit_info`, `get_packed_single_point_with_scd`,
`get_end_of_initialization`, `get_single_cmd`, `get_double_cmd`,
`get_step_cmd`, `get_setpoint_normal_cmd`, `get_setpoint_scaled_cmd`,
`get_setpoint_float_cmd`, `get_bits_string32_cmd`, `get_interrogation_cmd`,
`get_counter_interrogation_cmd`, `get_read_cmd`,
`get_clock_synchronization_cmd`, `get_test_command`,
`get_test_command_cp56time2a`, `get_reset_process_cmd`,
`get_delay_acquire_command`, `get_parameter_normal`, `get_parameter_scaled`,
`get_parameter_float`, `get_parameter_activation`.

Each returns `Result`: a truncated payload yields [`Error::UnexpectedEof`], and
a type identification the getter does not handle yields
[`Error::TypeIdNotMatch`]. Nothing panics.

## Replying

`reply_mirror` echoes a received ASDU back with a different cause — the
standard confirmation pattern — and `negated` sets the P/N bit:

```rust
# use rs_iec60870_5::asdu::*;
# async fn f(c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;      // positive
// ... do the work ...
c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await?;     // finished

c.send(pack.reply_mirror(Cause::ACTIVATION_CON).negated()).await?;  // rejected
c.send(pack.reply_mirror(Cause::UNKNOWN_TYPE_ID)).await        // protocol error
# }
```

`reply(cause, addr)` does the same but also re-addresses the copy.

## Sending

[`Connect`] is what every endpoint implements:

```rust
# use rs_iec60870_5::asdu::*;
# use async_trait::async_trait;
# struct MyEndpoint;
#[async_trait]
impl Connect for MyEndpoint {
    fn params(&self) -> Params { PARAMS_WIDE }
    async fn send(&self, a: Asdu) -> rs_iec60870_5::Result<()> { let _ = a; Ok(()) }
}
```

[`ConnectExt`] is blanket-implemented for every `Connect`, including
`&dyn Connect`, and adds a build-and-send method per family — `send_single`,
`send_measured_value_float_cp56time2a`, `send_single_cmd`,
`send_interrogation_cmd` and so on, one for each builder above, plus
`send_reply_mirror`:

```rust
# use rs_iec60870_5::asdu::*;
# async fn f(c: &dyn Connect) -> rs_iec60870_5::Result<()> {
c.send_single(
    false,
    CauseOfTransmission::new(Cause::SPONTANEOUS),
    1,
    &[SinglePointInfo::new(100, true)],
)
.await
# }
```

Sends are queued, not blocking. [`Error::BufferFull`] and
[`Error::SendQueueFull`] mean back off and retry; nothing was dropped silently.

## Information elements

**Quality.** [`QualityDescriptor`] is a bit field: `GOOD` (0), `OVERFLOW`,
`BLOCKED`, `SUBSTITUTED`, `NOT_TOPICAL`, `INVALID`, combined with `|`. Protection
equipment uses [`QualityDescriptorProtection`]: `GOOD`, `ELAPSED_TIME_INVALID`,
`BLOCKED`, `SUBSTITUTED`, `NOT_TOPICAL`, `INVALID`. Note that the short-float
family transmits only OV, NT and IV (mask `0xf1`), per the standard.

**Values.** [`Normalize`] is a 16-bit fraction in `[-1, 1 − 2⁻¹⁵]`;
`f64()` converts to a fraction and `from_f64` back. [`StepPosition`] is a
7-bit signed value with a transient flag. [`BinaryCounterReading`] carries the
count, a sequence number and the CY, CA and IV flags.
[`StatusAndStatusChangeDetection`] splits into `status()` and
`change_detection()`.

**Qualifiers.** [`QualifierOfCommand`] (pulse behaviour plus the S/E bit),
[`QualifierOfSetpointCmd`], [`QualifierOfInterrogation`] (`STATION`, or
`group(n)`), [`QualifierCountCall`] (request plus freeze),
[`QualifierOfResetProcessCmd`], [`QualifierOfParameterMv`],
[`QualifierOfParameterAct`].

## Time tags

The `asdu::time` module encodes and decodes the three binary time formats.

* **CP56Time2a** — 7 octets, full date and time to the millisecond.
* **CP24Time2a** — 3 octets, minutes and milliseconds only. On decode the date
  and hour come from the host clock; a tag more than five minutes *ahead* of
  the current minute is taken to belong to the previous hour.
* **CP16Time2a** — 2 octets, an elapsed millisecond count.

Each tagged information object carries its tag in two fields: `time`, the
reading, and `time_flags`, a [`TimeTagFlags`] with the two validity bits of the
minutes octet:

| Flag | Bit | Meaning |
|------|-----|---------|
| `invalid` | IV, bit 7 | the station's clock was not synchronized or could not be read |
| `substituted` | SB, bit 6 | the time was substituted by an intermediate station |

`time` holds the reading **even when IV is set**, as long as the octets name a
real instant: a device whose clock has not been synchronized still tags its
events, and their order is worth keeping even when their absolute value is
not. So check `time_flags.is_valid()` before taking `time` as the time of the
event. `time` is `None` only when the octets hold no time at all, and
`time_flags` is `TimeTagFlags::GOOD` for the untagged types. On the sending
side, set `time_flags` to mark an unsynchronized clock; `time: None` always
encodes an all-zero tag with IV set.

Two decoders are available when working with raw octets:
`parse_cp56time2a` returns only a time that can be trusted (`None` when IV is
set) and `parse_cp56time2a_tag` returns the reading and the flags; likewise for
CP24Time2a and, in `cs103`, CP32Time2a. The clock synchronization command
uses the strict one: a clock is never set from a time its sender marks
invalid.

## Wire format

```rust
# use rs_iec60870_5::asdu::*;
# fn f() -> rs_iec60870_5::Result<()> {
# let a = Asdu::single(PARAMS_WIDE, false, CauseOfTransmission::new(Cause::SPONTANEOUS), 1,
#     &[SinglePointInfo::new(100, true)])?;
let wire: Vec<u8> = a.marshal_binary()?;
let back = Asdu::unmarshal_binary(PARAMS_WIDE, &wire)?;
assert_eq!(a, back);
# Ok(())
# }
```

`unmarshal_binary` trims the payload to exactly the length the type
identification and the variable structure qualifier imply; a shorter payload is
rejected. For hand-built ASDUs, [`Asdu::encoder`] appends information elements
and [`Asdu::reader`] reads them back.

`ASDU_SIZE_MAX` is 249 octets including the identifier. Builders check it and
return [`Error::LengthOutOfRange`]; batch large point sets across several calls,
or use `is_sequence = true` for contiguous addresses.

## Diagnostics

`Asdu` implements `Display`, giving a compact non-destructive dump:

```text
TID<M_SP_NA_1> COT<Spontaneous> @1 VSQ<2> IOA-Width=3 items=2 [100=true, 101=false QDS=0x80]
```

A payload too short to decode formats as `<undecodable payload=NB>` rather than
failing. With the `serde` feature the ASDU types also derive `Serialize` and
`Deserialize`.

## Errors

| Error | Meaning |
|-------|---------|
| `CmdCause` | cause of transmission not allowed for this type |
| `TypeIdNotMatch` | the type does not match the builder or getter used |
| `TypeIdentifier` | unknown type identification |
| `Param` | system parameters out of range |
| `CommonAddrZero` | common address 0 is not usable |
| `CommonAddrFit` | common address exceeds `common_addr_size` |
| `InfoObjAddrFit` | information object address exceeds `info_obj_addr_size` |
| `InfoObjIndexFit` | object count not in `[1, 127]` |
| `LengthOutOfRange` | more than 249 octets |
| `NotAnyObjInfo` | no information objects were passed |
| `UnexpectedEof` | the payload ended mid-object |
| `CauseZero` | cause of transmission 0 is not used |

[`Asdu`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.Asdu.html
[`Asdu::encoder`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.Asdu.html#method.encoder
[`Asdu::reader`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.Asdu.html#method.reader
[`Cause`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.Cause.html
[`CauseOfTransmission`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.CauseOfTransmission.html
[`Connect`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/trait.Connect.html
[`ConnectExt`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/trait.ConnectExt.html
[`Identifier`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.Identifier.html
[`Params`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.Params.html
[`TimeZone`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/enum.TimeZone.html
[`TimeTagFlags`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.TimeTagFlags.html
[`Error::TrailingOctets`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/enum.Error.html
[`Normalize`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.Normalize.html
[`StepPosition`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.StepPosition.html
[`BinaryCounterReading`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.BinaryCounterReading.html
[`StatusAndStatusChangeDetection`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.StatusAndStatusChangeDetection.html
[`QualityDescriptor`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.QualityDescriptor.html
[`QualityDescriptorProtection`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.QualityDescriptorProtection.html
[`QualifierOfCommand`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.QualifierOfCommand.html
[`QualifierOfSetpointCmd`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.QualifierOfSetpointCmd.html
[`QualifierOfInterrogation`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.QualifierOfInterrogation.html
[`QualifierCountCall`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.QualifierCountCall.html
[`QualifierOfResetProcessCmd`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.QualifierOfResetProcessCmd.html
[`QualifierOfParameterMv`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.QualifierOfParameterMv.html
[`QualifierOfParameterAct`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/asdu/struct.QualifierOfParameterAct.html
[`Error::CmdCause`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/enum.Error.html
[`Error::UnexpectedEof`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/enum.Error.html
[`Error::TypeIdNotMatch`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/enum.Error.html
[`Error::LengthOutOfRange`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/enum.Error.html
[`Error::BufferFull`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/enum.Error.html
[`Error::SendQueueFull`]: https://docs.rs/rs-iec60870-5/latest/rs_iec60870_5/enum.Error.html
