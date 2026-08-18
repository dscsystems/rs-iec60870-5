// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Process information in the control direction: the `C_*` command types sent
//! from the master to the controlled station.
//!
//! Every command ASDU carries exactly one information object (SQ = 0). Pass the
//! `_TA_1` type identification to a builder to get the CP56Time2a variant, in
//! which case the info struct's `time` field is encoded.
//!
//! Select-before-execute is left to the application: check `qoc.in_select` (or
//! `qos.in_select`) and confirm without operating when the command is a select.

use chrono::{DateTime, Utc};

use crate::asdu::codec::Asdu;
use crate::asdu::identifier::{
    Cause, CauseOfTransmission, CommonAddr, Identifier, TypeId, VariableStruct,
};
use crate::asdu::info::*;
use crate::asdu::params::Params;
use crate::error::{Error, Result};

/// A single command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SingleCommandInfo {
    /// Information object address of the controlled point.
    pub ioa: InfoObjAddr,
    /// The commanded state.
    pub value: bool,
    /// Qualifier: pulse behaviour and the select/execute bit.
    pub qoc: QualifierOfCommand,
    /// Time tag; encoded only by `C_SC_TA_1`.
    pub time: Option<DateTime<Utc>>,
}

/// A double command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DoubleCommandInfo {
    /// Information object address of the controlled point.
    pub ioa: InfoObjAddr,
    /// The commanded state.
    pub value: DoubleCommand,
    /// Qualifier: pulse behaviour and the select/execute bit.
    pub qoc: QualifierOfCommand,
    /// Time tag; encoded only by `C_DC_TA_1`.
    pub time: Option<DateTime<Utc>>,
}

/// A regulating step command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StepCommandInfo {
    /// Information object address of the controlled point.
    pub ioa: InfoObjAddr,
    /// Step up or step down.
    pub value: StepCommand,
    /// Qualifier: pulse behaviour and the select/execute bit.
    pub qoc: QualifierOfCommand,
    /// Time tag; encoded only by `C_RC_TA_1`.
    pub time: Option<DateTime<Utc>>,
}

/// A set-point command with a normalized value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SetpointCommandNormalInfo {
    /// Information object address of the controlled point.
    pub ioa: InfoObjAddr,
    /// The set-point in `[-1, 1 - 2^-15]`.
    pub value: Normalize,
    /// Qualifier: implementation-defined value plus the select/execute bit.
    pub qos: QualifierOfSetpointCmd,
    /// Time tag; encoded only by `C_SE_TA_1`.
    pub time: Option<DateTime<Utc>>,
}

/// A set-point command with a scaled value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SetpointCommandScaledInfo {
    /// Information object address of the controlled point.
    pub ioa: InfoObjAddr,
    /// The set-point.
    pub value: i16,
    /// Qualifier: implementation-defined value plus the select/execute bit.
    pub qos: QualifierOfSetpointCmd,
    /// Time tag; encoded only by `C_SE_TB_1`.
    pub time: Option<DateTime<Utc>>,
}

/// A set-point command with a short floating point value.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SetpointCommandFloatInfo {
    /// Information object address of the controlled point.
    pub ioa: InfoObjAddr,
    /// The set-point.
    pub value: f32,
    /// Qualifier: implementation-defined value plus the select/execute bit.
    pub qos: QualifierOfSetpointCmd,
    /// Time tag; encoded only by `C_SE_TC_1`.
    pub time: Option<DateTime<Utc>>,
}

/// A 32 bit string command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BitsString32CommandInfo {
    /// Information object address of the controlled point.
    pub ioa: InfoObjAddr,
    /// The 32 commanded bits.
    pub value: u32,
    /// Time tag; encoded only by `C_BO_TA_1`.
    pub time: Option<DateTime<Utc>>,
}

/// Commands may only be sent with `Activation` or `Deactivation`.
fn check_command_cause(coa: CauseOfTransmission) -> Result<()> {
    if coa.cause == Cause::ACTIVATION || coa.cause == Cause::DEACTIVATION {
        Ok(())
    } else {
        Err(Error::CmdCause)
    }
}

/// Start a single-object command ASDU.
fn start_cmd(
    params: Params,
    type_id: TypeId,
    coa: CauseOfTransmission,
    ca: CommonAddr,
) -> Result<Asdu> {
    params.valid()?;
    Ok(Asdu::new(
        params,
        Identifier::new(type_id, VariableStruct::single(), coa, ca),
    ))
}

impl Asdu {
    /// Build `C_SC_NA_1` or `C_SC_TA_1`: a single command.
    ///
    /// See companion standard 101, subclass 7.3.2.1.
    /// Permitted causes in the control direction: `Activation`, `Deactivation`.
    pub fn single_cmd(
        params: Params,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: SingleCommandInfo,
    ) -> Result<Asdu> {
        check_command_cause(coa)?;
        let mut u = start_cmd(params, type_id, coa, ca)?;
        let mut e = u.encoder();
        e.info_obj_addr(cmd.ioa)?;
        e.byte(cmd.qoc.value() | u8::from(cmd.value));
        match type_id {
            TypeId::C_SC_NA_1 => {}
            TypeId::C_SC_TA_1 => {
                e.cp56time2a(cmd.time);
            }
            _ => return Err(Error::TypeIdNotMatch),
        }
        Ok(u)
    }

    /// Decode `C_SC_NA_1` or `C_SC_TA_1`.
    pub fn get_single_cmd(&self) -> Result<SingleCommandInfo> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        let value = r.byte()?;
        let time = match self.type_id() {
            TypeId::C_SC_NA_1 => None,
            TypeId::C_SC_TA_1 => r.cp56time2a()?,
            _ => return Err(Error::TypeIdNotMatch),
        };
        Ok(SingleCommandInfo {
            ioa,
            value: value & 0x01 == 0x01,
            qoc: QualifierOfCommand::parse(value),
            time,
        })
    }

    /// Build `C_DC_NA_1` or `C_DC_TA_1`: a double command.
    ///
    /// See companion standard 101, subclass 7.3.2.2.
    pub fn double_cmd(
        params: Params,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: DoubleCommandInfo,
    ) -> Result<Asdu> {
        check_command_cause(coa)?;
        let mut u = start_cmd(params, type_id, coa, ca)?;
        let mut e = u.encoder();
        e.info_obj_addr(cmd.ioa)?;
        e.byte(cmd.qoc.value() | cmd.value.value());
        match type_id {
            TypeId::C_DC_NA_1 => {}
            TypeId::C_DC_TA_1 => {
                e.cp56time2a(cmd.time);
            }
            _ => return Err(Error::TypeIdNotMatch),
        }
        Ok(u)
    }

    /// Decode `C_DC_NA_1` or `C_DC_TA_1`.
    pub fn get_double_cmd(&self) -> Result<DoubleCommandInfo> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        let value = r.byte()?;
        let time = match self.type_id() {
            TypeId::C_DC_NA_1 => None,
            TypeId::C_DC_TA_1 => r.cp56time2a()?,
            _ => return Err(Error::TypeIdNotMatch),
        };
        Ok(DoubleCommandInfo {
            ioa,
            value: DoubleCommand::parse(value),
            qoc: QualifierOfCommand::parse(value),
            time,
        })
    }

    /// Build `C_RC_NA_1` or `C_RC_TA_1`: a regulating step command.
    ///
    /// See companion standard 101, subclass 7.3.2.3.
    pub fn step_cmd(
        params: Params,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: StepCommandInfo,
    ) -> Result<Asdu> {
        check_command_cause(coa)?;
        let mut u = start_cmd(params, type_id, coa, ca)?;
        let mut e = u.encoder();
        e.info_obj_addr(cmd.ioa)?;
        e.byte(cmd.qoc.value() | cmd.value.value());
        match type_id {
            TypeId::C_RC_NA_1 => {}
            TypeId::C_RC_TA_1 => {
                e.cp56time2a(cmd.time);
            }
            _ => return Err(Error::TypeIdNotMatch),
        }
        Ok(u)
    }

    /// Decode `C_RC_NA_1` or `C_RC_TA_1`.
    pub fn get_step_cmd(&self) -> Result<StepCommandInfo> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        let value = r.byte()?;
        let time = match self.type_id() {
            TypeId::C_RC_NA_1 => None,
            TypeId::C_RC_TA_1 => r.cp56time2a()?,
            _ => return Err(Error::TypeIdNotMatch),
        };
        Ok(StepCommandInfo {
            ioa,
            value: StepCommand::parse(value),
            qoc: QualifierOfCommand::parse(value),
            time,
        })
    }

    /// Build `C_SE_NA_1` or `C_SE_TA_1`: a normalized set-point command.
    ///
    /// See companion standard 101, subclass 7.3.2.4.
    pub fn setpoint_cmd_normal(
        params: Params,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: SetpointCommandNormalInfo,
    ) -> Result<Asdu> {
        check_command_cause(coa)?;
        let mut u = start_cmd(params, type_id, coa, ca)?;
        let mut e = u.encoder();
        e.info_obj_addr(cmd.ioa)?;
        e.normalize(cmd.value).byte(cmd.qos.value());
        match type_id {
            TypeId::C_SE_NA_1 => {}
            TypeId::C_SE_TA_1 => {
                e.cp56time2a(cmd.time);
            }
            _ => return Err(Error::TypeIdNotMatch),
        }
        Ok(u)
    }

    /// Decode `C_SE_NA_1` or `C_SE_TA_1`.
    pub fn get_setpoint_normal_cmd(&self) -> Result<SetpointCommandNormalInfo> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        let value = r.normalize()?;
        let qos = QualifierOfSetpointCmd::parse(r.byte()?);
        let time = match self.type_id() {
            TypeId::C_SE_NA_1 => None,
            TypeId::C_SE_TA_1 => r.cp56time2a()?,
            _ => return Err(Error::TypeIdNotMatch),
        };
        Ok(SetpointCommandNormalInfo {
            ioa,
            value,
            qos,
            time,
        })
    }

    /// Build `C_SE_NB_1` or `C_SE_TB_1`: a scaled set-point command.
    ///
    /// See companion standard 101, subclass 7.3.2.5.
    pub fn setpoint_cmd_scaled(
        params: Params,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: SetpointCommandScaledInfo,
    ) -> Result<Asdu> {
        check_command_cause(coa)?;
        let mut u = start_cmd(params, type_id, coa, ca)?;
        let mut e = u.encoder();
        e.info_obj_addr(cmd.ioa)?;
        e.scaled(cmd.value).byte(cmd.qos.value());
        match type_id {
            TypeId::C_SE_NB_1 => {}
            TypeId::C_SE_TB_1 => {
                e.cp56time2a(cmd.time);
            }
            _ => return Err(Error::TypeIdNotMatch),
        }
        Ok(u)
    }

    /// Decode `C_SE_NB_1` or `C_SE_TB_1`.
    pub fn get_setpoint_scaled_cmd(&self) -> Result<SetpointCommandScaledInfo> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        let value = r.scaled()?;
        let qos = QualifierOfSetpointCmd::parse(r.byte()?);
        let time = match self.type_id() {
            TypeId::C_SE_NB_1 => None,
            TypeId::C_SE_TB_1 => r.cp56time2a()?,
            _ => return Err(Error::TypeIdNotMatch),
        };
        Ok(SetpointCommandScaledInfo {
            ioa,
            value,
            qos,
            time,
        })
    }

    /// Build `C_SE_NC_1` or `C_SE_TC_1`: a short float set-point command.
    ///
    /// See companion standard 101, subclass 7.3.2.6.
    pub fn setpoint_cmd_float(
        params: Params,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: SetpointCommandFloatInfo,
    ) -> Result<Asdu> {
        check_command_cause(coa)?;
        let mut u = start_cmd(params, type_id, coa, ca)?;
        let mut e = u.encoder();
        e.info_obj_addr(cmd.ioa)?;
        e.f32(cmd.value).byte(cmd.qos.value());
        match type_id {
            TypeId::C_SE_NC_1 => {}
            TypeId::C_SE_TC_1 => {
                e.cp56time2a(cmd.time);
            }
            _ => return Err(Error::TypeIdNotMatch),
        }
        Ok(u)
    }

    /// Decode `C_SE_NC_1` or `C_SE_TC_1`.
    pub fn get_setpoint_float_cmd(&self) -> Result<SetpointCommandFloatInfo> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        let value = r.f32()?;
        let qos = QualifierOfSetpointCmd::parse(r.byte()?);
        let time = match self.type_id() {
            TypeId::C_SE_NC_1 => None,
            TypeId::C_SE_TC_1 => r.cp56time2a()?,
            _ => return Err(Error::TypeIdNotMatch),
        };
        Ok(SetpointCommandFloatInfo {
            ioa,
            value,
            qos,
            time,
        })
    }

    /// Build `C_BO_NA_1` or `C_BO_TA_1`: a 32 bit string command.
    ///
    /// See companion standard 101, subclass 7.3.2.7.
    pub fn bits_string32_cmd(
        params: Params,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: BitsString32CommandInfo,
    ) -> Result<Asdu> {
        check_command_cause(coa)?;
        let mut u = start_cmd(params, type_id, coa, ca)?;
        let mut e = u.encoder();
        e.info_obj_addr(cmd.ioa)?;
        e.bits_string32(cmd.value);
        match type_id {
            TypeId::C_BO_NA_1 => {}
            TypeId::C_BO_TA_1 => {
                e.cp56time2a(cmd.time);
            }
            _ => return Err(Error::TypeIdNotMatch),
        }
        Ok(u)
    }

    /// Decode `C_BO_NA_1` or `C_BO_TA_1`.
    pub fn get_bits_string32_cmd(&self) -> Result<BitsString32CommandInfo> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        let value = r.bits_string32()?;
        let time = match self.type_id() {
            TypeId::C_BO_NA_1 => None,
            TypeId::C_BO_TA_1 => r.cp56time2a()?,
            _ => return Err(Error::TypeIdNotMatch),
        };
        Ok(BitsString32CommandInfo { ioa, value, time })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::params::PARAMS_WIDE;
    use chrono::TimeZone as _;

    fn act() -> CauseOfTransmission {
        CauseOfTransmission::new(Cause::ACTIVATION)
    }

    fn t() -> Option<DateTime<Utc>> {
        Some(Utc.with_ymd_and_hms(2026, 8, 17, 10, 30, 15).unwrap())
    }

    #[test]
    fn single_cmd_packs_value_and_qualifier_in_one_octet() {
        let cmd = SingleCommandInfo {
            ioa: 6000,
            value: true,
            qoc: QualifierOfCommand {
                qual: QocQual::SHORT_PULSE_DURATION,
                in_select: true,
            },
            time: None,
        };
        let a = Asdu::single_cmd(PARAMS_WIDE, TypeId::C_SC_NA_1, act(), 1, cmd).unwrap();
        // 0x80 select | (1 << 2) short pulse | 0x01 on
        assert_eq!(a.info_obj.last(), Some(&0x85));
        assert_eq!(a.get_single_cmd().unwrap(), cmd);
    }

    #[test]
    fn commands_reject_non_activation_causes() {
        let cmd = SingleCommandInfo::default();
        assert_eq!(
            Asdu::single_cmd(
                PARAMS_WIDE,
                TypeId::C_SC_NA_1,
                CauseOfTransmission::new(Cause::SPONTANEOUS),
                1,
                cmd
            ),
            Err(Error::CmdCause)
        );
        assert!(
            Asdu::single_cmd(
                PARAMS_WIDE,
                TypeId::C_SC_NA_1,
                CauseOfTransmission::new(Cause::DEACTIVATION),
                1,
                cmd
            )
            .is_ok()
        );
    }

    #[test]
    fn commands_reject_a_mismatched_type_identification() {
        assert_eq!(
            Asdu::single_cmd(
                PARAMS_WIDE,
                TypeId::C_DC_NA_1,
                act(),
                1,
                SingleCommandInfo::default()
            ),
            Err(Error::TypeIdNotMatch)
        );
    }

    #[test]
    fn every_command_family_round_trips_untagged_and_tagged() {
        let p = PARAMS_WIDE;
        let qoc = QualifierOfCommand {
            qual: QocQual::LONG_PULSE_DURATION,
            in_select: false,
        };
        let qos = QualifierOfSetpointCmd {
            qual: QosQual(3),
            in_select: true,
        };

        let sc = SingleCommandInfo {
            ioa: 1,
            value: true,
            qoc,
            time: None,
        };
        assert_eq!(
            Asdu::single_cmd(p, TypeId::C_SC_NA_1, act(), 1, sc)
                .unwrap()
                .get_single_cmd()
                .unwrap(),
            sc
        );
        let sc = SingleCommandInfo { time: t(), ..sc };
        assert_eq!(
            Asdu::single_cmd(p, TypeId::C_SC_TA_1, act(), 1, sc)
                .unwrap()
                .get_single_cmd()
                .unwrap(),
            sc
        );

        let dc = DoubleCommandInfo {
            ioa: 2,
            value: DoubleCommand::On,
            qoc,
            time: t(),
        };
        assert_eq!(
            Asdu::double_cmd(p, TypeId::C_DC_TA_1, act(), 1, dc)
                .unwrap()
                .get_double_cmd()
                .unwrap(),
            dc
        );

        let rc = StepCommandInfo {
            ioa: 3,
            value: StepCommand::StepUp,
            qoc,
            time: None,
        };
        assert_eq!(
            Asdu::step_cmd(p, TypeId::C_RC_NA_1, act(), 1, rc)
                .unwrap()
                .get_step_cmd()
                .unwrap(),
            rc
        );

        let na = SetpointCommandNormalInfo {
            ioa: 4,
            value: Normalize(-16384),
            qos,
            time: t(),
        };
        assert_eq!(
            Asdu::setpoint_cmd_normal(p, TypeId::C_SE_TA_1, act(), 1, na)
                .unwrap()
                .get_setpoint_normal_cmd()
                .unwrap(),
            na
        );

        let nb = SetpointCommandScaledInfo {
            ioa: 5,
            value: -30000,
            qos,
            time: None,
        };
        assert_eq!(
            Asdu::setpoint_cmd_scaled(p, TypeId::C_SE_NB_1, act(), 1, nb)
                .unwrap()
                .get_setpoint_scaled_cmd()
                .unwrap(),
            nb
        );

        let nc = SetpointCommandFloatInfo {
            ioa: 6,
            value: -1.75,
            qos,
            time: t(),
        };
        assert_eq!(
            Asdu::setpoint_cmd_float(p, TypeId::C_SE_TC_1, act(), 1, nc)
                .unwrap()
                .get_setpoint_float_cmd()
                .unwrap(),
            nc
        );

        let bo = BitsString32CommandInfo {
            ioa: 7,
            value: 0xcafe_babe,
            time: t(),
        };
        assert_eq!(
            Asdu::bits_string32_cmd(p, TypeId::C_BO_TA_1, act(), 1, bo)
                .unwrap()
                .get_bits_string32_cmd()
                .unwrap(),
            bo
        );
    }

    #[test]
    fn tagged_commands_survive_a_wire_round_trip() {
        // go-iecp5 cannot size types 58..=64 and drops them on receipt; this
        // implementation encodes and decodes them per the standard.
        let cmd = SingleCommandInfo {
            ioa: 6000,
            value: true,
            qoc: QualifierOfCommand::default(),
            time: t(),
        };
        let a = Asdu::single_cmd(PARAMS_WIDE, TypeId::C_SC_TA_1, act(), 1, cmd).unwrap();
        let raw = a.marshal_binary().unwrap();
        let b = Asdu::unmarshal_binary(PARAMS_WIDE, &raw).unwrap();
        assert_eq!(b.get_single_cmd().unwrap(), cmd);
    }
}
