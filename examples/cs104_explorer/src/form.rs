// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The command builder: which control-direction ASDUs can be sent, and how the
//! free-text value field is parsed for each.

use rs_iec60870_5::asdu::*;

/// One selectable entry in the command builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CmdKind {
    pub name: &'static str,
    pub type_id: TypeId,
    /// What the value field means for this command, shown as a hint.
    pub value_hint: &'static str,
}

/// The commands the explorer can build, in the order the selector cycles them.
pub const CMD_KINDS: &[CmdKind] = &[
    CmdKind {
        name: "Single command (C_SC_NA_1)",
        type_id: TypeId::C_SC_NA_1,
        value_hint: "on | off",
    },
    CmdKind {
        name: "Double command (C_DC_NA_1)",
        type_id: TypeId::C_DC_NA_1,
        value_hint: "on | off",
    },
    CmdKind {
        name: "Step command (C_RC_NA_1)",
        type_id: TypeId::C_RC_NA_1,
        value_hint: "up | down",
    },
    CmdKind {
        name: "Setpoint float (C_SE_NC_1)",
        type_id: TypeId::C_SE_NC_1,
        value_hint: "a number, e.g. 22.5",
    },
    CmdKind {
        name: "Setpoint scaled (C_SE_NB_1)",
        type_id: TypeId::C_SE_NB_1,
        value_hint: "an integer, -32768..32767",
    },
    CmdKind {
        name: "Setpoint normalized (C_SE_NA_1)",
        type_id: TypeId::C_SE_NA_1,
        value_hint: "a fraction, -1..1",
    },
    CmdKind {
        name: "Bit string (C_BO_NA_1)",
        type_id: TypeId::C_BO_NA_1,
        value_hint: "32 bits, e.g. 0xDEADBEEF",
    },
    CmdKind {
        name: "Read command (C_RD_NA_1)",
        type_id: TypeId::C_RD_NA_1,
        value_hint: "(ignored)",
    },
];

/// Parse the value field as a single-command state.
pub fn parse_bool(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "1" | "on" | "true" | "t" | "yes" | "y" | "close"
    )
}

/// Parse the value field as a double-command state.
pub fn parse_double(s: &str) -> DoubleCommand {
    match s.trim().to_ascii_lowercase().as_str() {
        "2" | "on" | "true" | "close" => DoubleCommand::On,
        _ => DoubleCommand::Off,
    }
}

/// Parse the value field as a step-command direction.
pub fn parse_step(s: &str) -> StepCommand {
    match s.trim().to_ascii_lowercase().as_str() {
        "2" | "up" | "higher" | "+" => StepCommand::StepUp,
        _ => StepCommand::StepDown,
    }
}

/// Parse an unsigned integer, accepting a `0x` prefix for hexadecimal.
pub fn parse_uint(s: &str) -> Result<u32, String> {
    let s = s.trim();
    let parsed = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => s.parse(),
    };
    parsed.map_err(|e| format!("{s:?}: {e}"))
}

/// Clamp a fraction into the normalized value range.
pub fn clamp_norm(f: f64) -> f64 {
    f.clamp(-1.0, 1.0)
}

/// Everything the form needs to build one command.
#[derive(Debug, Clone)]
pub struct FormInput<'a> {
    pub kind: CmdKind,
    pub ioa_text: &'a str,
    pub value_text: &'a str,
    pub qualifier_text: &'a str,
    /// The S/E bit: select rather than execute.
    pub select: bool,
    pub common_addr: CommonAddr,
    pub params: Params,
}

/// Build the ASDU the form describes.
///
/// Returns a human-readable message on invalid input, which the caller shows
/// in the log panel rather than treating as a protocol error.
pub fn build(input: &FormInput<'_>) -> Result<Asdu, String> {
    let ioa: InfoObjAddr = parse_uint(input.ioa_text).map_err(|e| format!("invalid IOA {e}"))?;
    let qual = parse_uint(input.qualifier_text).unwrap_or(0) as u8;

    let ca = input.common_addr;
    let p = input.params;
    let coa = CauseOfTransmission::new(Cause::ACTIVATION);
    let qoc = QualifierOfCommand {
        qual: QocQual(qual),
        in_select: input.select,
    };
    let qos = QualifierOfSetpointCmd {
        qual: QosQual(qual),
        in_select: input.select,
    };
    let value = input.value_text;

    let asdu = match input.kind.type_id {
        // A read command names a point and carries no value; its cause is
        // forced to Request by the builder.
        TypeId::C_RD_NA_1 => Asdu::read_cmd(p, CauseOfTransmission::new(Cause::REQUEST), ca, ioa),

        TypeId::C_SC_NA_1 => Asdu::single_cmd(
            p,
            TypeId::C_SC_NA_1,
            coa,
            ca,
            SingleCommandInfo {
                ioa,
                value: parse_bool(value),
                qoc,
                time: None,
                time_flags: TimeTagFlags::GOOD,
            },
        ),

        TypeId::C_DC_NA_1 => Asdu::double_cmd(
            p,
            TypeId::C_DC_NA_1,
            coa,
            ca,
            DoubleCommandInfo {
                ioa,
                value: parse_double(value),
                qoc,
                time: None,
                time_flags: TimeTagFlags::GOOD,
            },
        ),

        TypeId::C_RC_NA_1 => Asdu::step_cmd(
            p,
            TypeId::C_RC_NA_1,
            coa,
            ca,
            StepCommandInfo {
                ioa,
                value: parse_step(value),
                qoc,
                time: None,
                time_flags: TimeTagFlags::GOOD,
            },
        ),

        TypeId::C_SE_NC_1 => {
            let v: f32 = value
                .trim()
                .parse()
                .map_err(|e| format!("invalid float value: {e}"))?;
            Asdu::setpoint_cmd_float(
                p,
                TypeId::C_SE_NC_1,
                coa,
                ca,
                SetpointCommandFloatInfo {
                    ioa,
                    value: v,
                    qos,
                    time: None,
                    time_flags: TimeTagFlags::GOOD,
                },
            )
        }

        TypeId::C_SE_NB_1 => {
            let v: i16 = value
                .trim()
                .parse()
                .map_err(|e| format!("invalid scaled value: {e}"))?;
            Asdu::setpoint_cmd_scaled(
                p,
                TypeId::C_SE_NB_1,
                coa,
                ca,
                SetpointCommandScaledInfo {
                    ioa,
                    value: v,
                    qos,
                    time: None,
                    time_flags: TimeTagFlags::GOOD,
                },
            )
        }

        TypeId::C_SE_NA_1 => {
            let v: f64 = value
                .trim()
                .parse()
                .map_err(|e| format!("invalid normalized value (expected -1..1): {e}"))?;
            Asdu::setpoint_cmd_normal(
                p,
                TypeId::C_SE_NA_1,
                coa,
                ca,
                SetpointCommandNormalInfo {
                    ioa,
                    value: Normalize::from_f64(clamp_norm(v)),
                    qos,
                    time: None,
                    time_flags: TimeTagFlags::GOOD,
                },
            )
        }

        TypeId::C_BO_NA_1 => {
            let v = parse_uint(value).map_err(|e| format!("invalid bit string {e}"))?;
            Asdu::bits_string32_cmd(
                p,
                TypeId::C_BO_NA_1,
                coa,
                ca,
                BitsString32CommandInfo {
                    ioa,
                    value: v,
                    time: None,
                    time_flags: TimeTagFlags::GOOD,
                },
            )
        }

        other => return Err(format!("unsupported command type {other}")),
    };

    asdu.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(kind: CmdKind, ioa: &'a str, value: &'a str) -> FormInput<'a> {
        FormInput {
            kind,
            ioa_text: ioa,
            value_text: value,
            qualifier_text: "0",
            select: false,
            common_addr: 1,
            params: PARAMS_WIDE,
        }
    }

    fn kind_of(type_id: TypeId) -> CmdKind {
        *CMD_KINDS
            .iter()
            .find(|k| k.type_id == type_id)
            .expect("a known command kind")
    }

    #[test]
    fn boolean_values_accept_the_usual_spellings() {
        for s in ["on", "ON", "1", "true", "yes", "y", "close", " on "] {
            assert!(parse_bool(s), "{s:?} should be on");
        }
        for s in ["off", "0", "false", "no", "open", ""] {
            assert!(!parse_bool(s), "{s:?} should be off");
        }
    }

    #[test]
    fn double_and_step_values_default_to_the_safe_direction() {
        assert_eq!(parse_double("on"), DoubleCommand::On);
        assert_eq!(parse_double("anything else"), DoubleCommand::Off);
        assert_eq!(parse_step("up"), StepCommand::StepUp);
        assert_eq!(parse_step("+"), StepCommand::StepUp);
        assert_eq!(parse_step("anything else"), StepCommand::StepDown);
    }

    #[test]
    fn integers_accept_decimal_and_hexadecimal() {
        assert_eq!(parse_uint("6000"), Ok(6000));
        assert_eq!(parse_uint(" 0xDEADBEEF "), Ok(0xdead_beef));
        assert_eq!(parse_uint("0Xff"), Ok(255));
        assert!(parse_uint("not a number").is_err());
    }

    #[test]
    fn normalized_values_are_clamped_to_the_representable_range() {
        assert_eq!(clamp_norm(2.0), 1.0);
        assert_eq!(clamp_norm(-2.0), -1.0);
        assert_eq!(clamp_norm(0.5), 0.5);
    }

    #[test]
    fn a_single_command_carries_its_value_and_qualifier() {
        let mut i = input(kind_of(TypeId::C_SC_NA_1), "6000", "on");
        i.qualifier_text = "1";
        i.select = true;

        let a = build(&i).unwrap();
        assert_eq!(a.type_id(), TypeId::C_SC_NA_1);
        assert_eq!(a.coa().cause, Cause::ACTIVATION);
        let cmd = a.get_single_cmd().unwrap();
        assert_eq!(cmd.ioa, 6000);
        assert!(cmd.value);
        assert_eq!(cmd.qoc.qual, QocQual::SHORT_PULSE_DURATION);
        assert!(cmd.qoc.in_select, "the S/E bit must reach the wire");
    }

    #[test]
    fn a_read_command_is_sent_with_the_request_cause() {
        let a = build(&input(kind_of(TypeId::C_RD_NA_1), "400", "")).unwrap();
        assert_eq!(a.type_id(), TypeId::C_RD_NA_1);
        assert_eq!(a.coa().cause, Cause::REQUEST);
        assert_eq!(a.get_read_cmd().unwrap(), 400);
    }

    #[test]
    fn every_setpoint_family_round_trips_its_value() {
        let a = build(&input(kind_of(TypeId::C_SE_NC_1), "1", "-12.75")).unwrap();
        assert_eq!(a.get_setpoint_float_cmd().unwrap().value, -12.75);

        let a = build(&input(kind_of(TypeId::C_SE_NB_1), "2", "-1234")).unwrap();
        assert_eq!(a.get_setpoint_scaled_cmd().unwrap().value, -1234);

        let a = build(&input(kind_of(TypeId::C_SE_NA_1), "3", "0.5")).unwrap();
        assert_eq!(a.get_setpoint_normal_cmd().unwrap().value.f64(), 0.5);

        // Out of range normalizes to the endpoint rather than failing.
        let a = build(&input(kind_of(TypeId::C_SE_NA_1), "3", "9")).unwrap();
        assert_eq!(a.get_setpoint_normal_cmd().unwrap().value, Normalize(32767));
    }

    #[test]
    fn a_bit_string_command_accepts_hexadecimal() {
        let a = build(&input(kind_of(TypeId::C_BO_NA_1), "4", "0xDEADBEEF")).unwrap();
        assert_eq!(a.get_bits_string32_cmd().unwrap().value, 0xdead_beef);
    }

    #[test]
    fn bad_input_is_reported_rather_than_sent() {
        let e = build(&input(kind_of(TypeId::C_SC_NA_1), "not-a-number", "on")).unwrap_err();
        assert!(e.contains("invalid IOA"), "{e}");

        let e = build(&input(kind_of(TypeId::C_SE_NC_1), "1", "not-a-float")).unwrap_err();
        assert!(e.contains("invalid float"), "{e}");

        // An address too wide for the configured IOA size is a protocol error,
        // surfaced the same way.
        let mut i = input(kind_of(TypeId::C_SC_NA_1), "70000", "on");
        i.params = PARAMS_STANDARD_101; // 2 octet addresses
        assert!(build(&i).is_err());
    }

    #[test]
    fn every_listed_kind_can_be_built() {
        for kind in CMD_KINDS {
            let value = match kind.type_id {
                TypeId::C_SE_NC_1 | TypeId::C_SE_NA_1 => "0.5",
                TypeId::C_SE_NB_1 => "100",
                TypeId::C_BO_NA_1 => "0xFF",
                _ => "on",
            };
            let a = build(&input(*kind, "100", value))
                .unwrap_or_else(|e| panic!("{} failed to build: {e}", kind.name));
            assert_eq!(a.type_id(), kind.type_id);
            // Whatever it is, it must survive a wire round trip.
            let raw = a.marshal_binary().unwrap();
            assert_eq!(Asdu::unmarshal_binary(PARAMS_WIDE, &raw).unwrap(), a);
        }
    }
}
