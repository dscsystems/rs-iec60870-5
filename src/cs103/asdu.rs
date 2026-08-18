// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The IEC 60870-5-103 application layer.
//!
//! ```text
//! ASDU = | type ID | VSQ | cause | common address | FUN | INF | elements... |
//! bytes |    1    |  1  |   1   |       1        |  1  |  1  |      n      |
//! ```
//!
//! Every identifier field is one octet — there is no `asdu::Params` to agree
//! on. Information objects are addressed by **function type** (FUN) and
//! **information number** (INF) rather than by an information object address.

use std::fmt;

use chrono::{DateTime, Utc};

use crate::asdu::time::{cp56time2a, parse_cp56time2a};
use crate::asdu::{TimeZone, VariableStruct};
use crate::cs103::elements::*;
use crate::error::{Error, Result};

/// Maximum ASDU length carried in one FT1.2 frame.
pub const ASDU_SIZE_MAX: usize = 253;

/// The fixed identifier size: type ID, VSQ, cause and common address.
pub const IDENTIFIER_SIZE: usize = 4;

/// The broadcast common address and link address of IEC 60870-5-103.
pub const GLOBAL_COMMON_ADDR: u8 = 255;

/// ASDU type identification of IEC 60870-5-103.
///
/// See IEC 60870-5-103, subclass 7.2.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TypeId(pub u8);

impl TypeId {
    // -- monitor direction --
    /// 1: time-tagged message
    pub const TIME_TAGGED: TypeId = TypeId(1);
    /// 2: time-tagged message with relative time
    pub const TIME_TAGGED_REL: TypeId = TypeId(2);
    /// 3: measurands I, up to four values
    pub const MEASURANDS_I: TypeId = TypeId(3);
    /// 4: time-tagged measurands with relative time
    pub const TIME_TAGGED_MEASURANDS: TypeId = TypeId(4);
    /// 5: identification message
    pub const IDENTIFICATION: TypeId = TypeId(5);
    /// 6: time synchronization, in both directions
    pub const TIME_SYNC: TypeId = TypeId(6);
    /// 7: general interrogation, control direction
    pub const GENERAL_INTERROGATION: TypeId = TypeId(7);
    /// 8: end of general interrogation
    pub const GI_TERMINATION: TypeId = TypeId(8);
    /// 9: measurands II, up to nine values
    pub const MEASURANDS_II: TypeId = TypeId(9);
    /// 10: generic data
    pub const GENERIC_DATA: TypeId = TypeId(10);
    /// 11: generic identification
    pub const GENERIC_IDENTIFICATION: TypeId = TypeId(11);

    // -- control direction --
    /// 20: general command
    pub const GENERAL_COMMAND: TypeId = TypeId(20);
    /// 21: generic command
    pub const GENERIC_COMMAND: TypeId = TypeId(21);

    // -- disturbance data transfer --
    /// 23: list of recorded disturbances
    pub const LIST_OF_RECORDED_DISTURBANCES: TypeId = TypeId(23);
    /// 24: order for disturbance data transmission
    pub const ORDER_DISTURBANCE_TRANSMISSION: TypeId = TypeId(24);
    /// 25: acknowledgement for disturbance data transmission
    pub const ACK_DISTURBANCE_TRANSMISSION: TypeId = TypeId(25);
    /// 26: ready for transmission of disturbance data
    pub const READY_DISTURBANCE_DATA: TypeId = TypeId(26);
    /// 27: ready for transmission of a channel
    pub const READY_DISTURBANCE_CHANNEL: TypeId = TypeId(27);
    /// 28: ready for transmission of tags
    pub const READY_DISTURBANCE_TAGS: TypeId = TypeId(28);
    /// 29: transmission of tags
    pub const TRANSMISSION_DISTURBANCE_TAGS: TypeId = TypeId(29);
    /// 30: transmission of disturbance values
    pub const TRANSMISSION_DISTURBANCE_VALUES: TypeId = TypeId(30);
    /// 31: end of transmission
    pub const END_DISTURBANCE_TRANSMISSION: TypeId = TypeId(31);
}

impl From<u8> for TypeId {
    fn from(v: u8) -> Self {
        TypeId(v)
    }
}

impl fmt::Display for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match *self {
            TypeId::TIME_TAGGED => "TimeTagged",
            TypeId::TIME_TAGGED_REL => "TimeTaggedRel",
            TypeId::MEASURANDS_I => "MeasurandsI",
            TypeId::TIME_TAGGED_MEASURANDS => "TimeTaggedMeasurands",
            TypeId::IDENTIFICATION => "Identification",
            TypeId::TIME_SYNC => "TimeSync",
            TypeId::GENERAL_INTERROGATION => "GeneralInterrogation",
            TypeId::GI_TERMINATION => "GITermination",
            TypeId::MEASURANDS_II => "MeasurandsII",
            TypeId::GENERIC_DATA => "GenericData",
            TypeId::GENERIC_IDENTIFICATION => "GenericIdentification",
            TypeId::GENERAL_COMMAND => "GeneralCommand",
            TypeId::GENERIC_COMMAND => "GenericCommand",
            _ => return write!(f, "TypeID<{}>", self.0),
        };
        write!(f, "{name}<{}>", self.0)
    }
}

/// Cause of transmission of IEC 60870-5-103, one octet.
///
/// See IEC 60870-5-103, subclass 7.2.3. Values 8, 9, 20, 31, 40 and 42 exist
/// in both directions with related meanings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Cause(pub u8);

impl Cause {
    /// 1: spontaneous
    pub const SPONTANEOUS: Cause = Cause(1);
    /// 2: cyclic
    pub const CYCLIC: Cause = Cause(2);
    /// 3: reset frame count bit
    pub const RESET_FCB: Cause = Cause(3);
    /// 4: reset communication unit
    pub const RESET_CU: Cause = Cause(4);
    /// 5: start / restart
    pub const START_RESTART: Cause = Cause(5);
    /// 6: power on
    pub const POWER_ON: Cause = Cause(6);
    /// 7: test mode
    pub const TEST_MODE: Cause = Cause(7);
    /// 8: time synchronization, both directions
    pub const TIME_SYNC: Cause = Cause(8);
    /// 9: general interrogation — initiation in the control direction, reply
    /// in the monitor direction
    pub const GI: Cause = Cause(9);
    /// 10: end of general interrogation
    pub const GI_TERMINATION: Cause = Cause(10);
    /// 11: local operation
    pub const LOCAL_OPERATION: Cause = Cause(11);
    /// 12: remote operation
    pub const REMOTE_OPERATION: Cause = Cause(12);
    /// 20: positive command acknowledgement (monitor); general command (control)
    pub const COMMAND_ACK_POS: Cause = Cause(20);
    /// 20: general command, control direction
    pub const GENERAL_COMMAND: Cause = Cause(20);
    /// 21: negative command acknowledgement
    pub const COMMAND_ACK_NEG: Cause = Cause(21);
    /// 31: transmission of disturbance data
    pub const DISTURBANCE_DATA: Cause = Cause(31);
    /// 40: positive acknowledgement of a generic write; generic write (control)
    pub const GENERIC_WRITE_ACK_POS: Cause = Cause(40);
    /// 40: generic write, control direction
    pub const GENERIC_WRITE: Cause = Cause(40);
    /// 41: negative acknowledgement of a generic write
    pub const GENERIC_WRITE_ACK_NEG: Cause = Cause(41);
    /// 42: valid data response to a generic read; generic read (control)
    pub const GENERIC_READ_VALID: Cause = Cause(42);
    /// 42: generic read, control direction
    pub const GENERIC_READ: Cause = Cause(42);
    /// 43: invalid data response to a generic read
    pub const GENERIC_READ_INVALID: Cause = Cause(43);
    /// 44: generic write confirmation
    pub const GENERIC_WRITE_CONF: Cause = Cause(44);
}

impl From<u8> for Cause {
    fn from(v: u8) -> Self {
        Cause(v)
    }
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.0 {
            1 => "Spontaneous",
            2 => "Cyclic",
            3 => "ResetFCB",
            4 => "ResetCU",
            5 => "StartRestart",
            6 => "PowerOn",
            7 => "TestMode",
            8 => "TimeSync",
            9 => "GeneralInterrogation",
            10 => "GITermination",
            11 => "LocalOperation",
            12 => "RemoteOperation",
            20 => "CommandAckPos/GeneralCommand",
            21 => "CommandAckNeg",
            31 => "DisturbanceData",
            40 => "GenericWriteAckPos/GenericWrite",
            41 => "GenericWriteAckNeg",
            42 => "GenericReadValid/GenericRead",
            43 => "GenericReadInvalid",
            44 => "GenericWriteConf",
            v => return write!(f, "Cause<{v}>"),
        };
        f.write_str(name)
    }
}

/// One IEC 60870-5-103 application message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asdu {
    /// Type identification.
    pub type_id: TypeId,
    /// Variable structure qualifier: SQ bit plus the element count.
    pub variable: VariableStruct,
    /// Cause of transmission.
    pub coa: Cause,
    /// Common address, conventionally equal to the link address.
    pub common_addr: u8,
    /// Information object octets, starting at FUN.
    pub info_obj: Vec<u8>,
    /// The time zone used to interpret this ASDU's time tags.
    pub time_zone: TimeZone,
}

/// The variable structure qualifier used by the single-object 103 messages.
fn vsq_one() -> VariableStruct {
    VariableStruct {
        number: 1,
        is_sequence: true,
    }
}

impl Asdu {
    /// Build an ASDU with the given identifier and an empty payload.
    pub fn new(type_id: TypeId, variable: VariableStruct, coa: Cause, common_addr: u8) -> Asdu {
        Asdu {
            type_id,
            variable,
            coa,
            common_addr,
            info_obj: Vec::new(),
            time_zone: TimeZone::Utc,
        }
    }

    /// Append raw information object octets.
    pub fn append(&mut self, b: &[u8]) -> &mut Self {
        self.info_obj.extend_from_slice(b);
        self
    }

    /// The function type of the (first) information object.
    pub fn fun(&self) -> u8 {
        self.info_obj.first().copied().unwrap_or(0)
    }

    /// The information number of the (first) information object.
    pub fn inf(&self) -> u8 {
        self.info_obj.get(1).copied().unwrap_or(0)
    }

    /// Serialise to the wire format.
    pub fn marshal_binary(&self) -> Result<Vec<u8>> {
        if self.type_id.0 == 0 {
            return Err(Error::TypeIdentifier);
        }
        if self.coa.0 == 0 {
            return Err(Error::CauseZero);
        }
        if IDENTIFIER_SIZE + self.info_obj.len() > ASDU_SIZE_MAX {
            return Err(Error::LengthOutOfRange);
        }
        let mut raw = Vec::with_capacity(IDENTIFIER_SIZE + self.info_obj.len());
        raw.push(self.type_id.0);
        raw.push(self.variable.value());
        raw.push(self.coa.0);
        raw.push(self.common_addr);
        raw.extend_from_slice(&self.info_obj);
        Ok(raw)
    }

    /// Parse an ASDU from the wire format.
    pub fn unmarshal_binary(raw: &[u8]) -> Result<Asdu> {
        if raw.len() < IDENTIFIER_SIZE {
            return Err(Error::UnexpectedEof);
        }
        Ok(Asdu {
            type_id: TypeId(raw[0]),
            variable: VariableStruct::parse(raw[1]),
            coa: Cause(raw[2]),
            common_addr: raw[3],
            info_obj: raw[IDENTIFIER_SIZE..].to_vec(),
            time_zone: TimeZone::Utc,
        })
    }

    fn need(&self, n: usize) -> Result<&[u8]> {
        if self.info_obj.len() < n {
            return Err(Error::UnexpectedEof);
        }
        Ok(&self.info_obj)
    }

    // -- decoders ---------------------------------------------------------

    /// Decode ASDU 1 (time-tagged message) or ASDU 2 (with relative time).
    pub fn get_time_tagged(&self) -> Result<TimeTaggedInfo> {
        let rel = match self.type_id {
            TypeId::TIME_TAGGED => false,
            TypeId::TIME_TAGGED_REL => true,
            _ => return Err(Error::TypeIdNotMatch),
        };
        // FUN INF DPI CP32(4) SIN, plus RET(2) FAN(2) for ASDU 2.
        let b = self.need(if rel { 12 } else { 8 })?;

        let mut info = TimeTaggedInfo {
            fun: b[0],
            inf: b[1],
            dpi: Dpi::parse(b[2]),
            ..Default::default()
        };
        let rest = &b[3..];
        let rest = if rel {
            info.relative_time = u16::from_le_bytes([rest[0], rest[1]]);
            info.fault_number = u16::from_le_bytes([rest[2], rest[3]]);
            &rest[4..]
        } else {
            rest
        };
        info.time = parse_cp32time2a(rest, self.time_zone);
        info.sin = rest[4];
        Ok(info)
    }

    /// Decode ASDU 3 (measurands I) or ASDU 9 (measurands II).
    pub fn get_measurands(&self) -> Result<MeasurandsInfo> {
        if self.type_id != TypeId::MEASURANDS_I && self.type_id != TypeId::MEASURANDS_II {
            return Err(Error::TypeIdNotMatch);
        }
        let n = self.variable.number as usize;
        let b = self.need(2 + 2 * n)?;
        Ok(MeasurandsInfo {
            fun: b[0],
            inf: b[1],
            values: (0..n)
                .map(|i| Measurand::parse(u16::from_le_bytes([b[2 + 2 * i], b[3 + 2 * i]])))
                .collect(),
        })
    }

    /// Decode ASDU 4 (time-tagged measurands with relative time).
    pub fn get_time_tagged_measurands(&self) -> Result<TimeTaggedMeasurandsInfo> {
        if self.type_id != TypeId::TIME_TAGGED_MEASURANDS {
            return Err(Error::TypeIdNotMatch);
        }
        // FUN INF SCL(4) RET(2) FAN(2) CP32(4)
        let b = self.need(14)?;
        Ok(TimeTaggedMeasurandsInfo {
            fun: b[0],
            inf: b[1],
            value: f32::from_bits(u32::from_le_bytes([b[2], b[3], b[4], b[5]])),
            relative_time: u16::from_le_bytes([b[6], b[7]]),
            fault_number: u16::from_le_bytes([b[8], b[9]]),
            time: parse_cp32time2a(&b[10..], self.time_zone),
        })
    }

    /// Decode ASDU 5 (identification message).
    pub fn get_identification(&self) -> Result<IdentificationInfo> {
        if self.type_id != TypeId::IDENTIFICATION {
            return Err(Error::TypeIdNotMatch);
        }
        let b = self.need(3)?;
        let rest = &b[3..];
        let n = rest.len().min(8);
        Ok(IdentificationInfo {
            fun: b[0],
            inf: b[1],
            col: b[2],
            ascii: String::from_utf8_lossy(&rest[..n])
                .trim_end_matches(['\0', ' '])
                .to_string(),
            extra: if rest.len() > 8 {
                rest[8..].to_vec()
            } else {
                Vec::new()
            },
        })
    }

    /// Decode ASDU 6 (time synchronization, either direction).
    pub fn get_time_sync(&self) -> Result<Option<DateTime<Utc>>> {
        if self.type_id != TypeId::TIME_SYNC {
            return Err(Error::TypeIdNotMatch);
        }
        // FUN INF CP56(7)
        let b = self.need(9)?;
        Ok(parse_cp56time2a(&b[2..], self.time_zone))
    }

    /// Decode ASDU 8 and return the scan number of the completed interrogation.
    pub fn get_gi_termination(&self) -> Result<u8> {
        if self.type_id != TypeId::GI_TERMINATION {
            return Err(Error::TypeIdNotMatch);
        }
        Ok(self.need(3)?[2])
    }

    /// Decode a control-direction ASDU 7 and return its scan number.
    pub fn get_general_interrogation(&self) -> Result<u8> {
        if self.type_id != TypeId::GENERAL_INTERROGATION {
            return Err(Error::TypeIdNotMatch);
        }
        Ok(self.need(3)?[2])
    }

    /// Decode ASDU 20 (general command).
    pub fn get_general_command(&self) -> Result<GeneralCommandInfo> {
        if self.type_id != TypeId::GENERAL_COMMAND {
            return Err(Error::TypeIdNotMatch);
        }
        // FUN INF DCO RII
        let b = self.need(4)?;
        Ok(GeneralCommandInfo {
            fun: b[0],
            inf: b[1],
            dco: Dco::parse(b[2]),
            rii: b[3],
        })
    }

    // -- control-direction builders ---------------------------------------

    /// Build a control-direction ASDU 6 (time synchronization).
    pub fn time_sync(common_addr: u8, t: DateTime<Utc>) -> Asdu {
        let mut a = Asdu::new(
            TypeId::TIME_SYNC,
            vsq_one(),
            Cause::TIME_SYNC,
            common_addr,
        );
        a.append(&[fun::GLOBAL, 0]);
        a.append(&cp56time2a(Some(t), a.time_zone));
        a
    }

    /// Build a control-direction ASDU 7 (initiation of general interrogation).
    ///
    /// The device answers with ASDU 1 messages carrying cause 9 and terminates
    /// with an ASDU 8 carrying the same scan number.
    pub fn general_interrogation(common_addr: u8, scn: u8) -> Asdu {
        let mut a = Asdu::new(
            TypeId::GENERAL_INTERROGATION,
            vsq_one(),
            Cause::GI,
            common_addr,
        );
        a.append(&[fun::GLOBAL, 0, scn]);
        a
    }

    /// Build a control-direction ASDU 20 (general command).
    ///
    /// The acknowledgement returns as an ASDU 1 with cause 20 (positive) or 21
    /// (negative), carrying `rii` in its supplementary information.
    pub fn general_command(common_addr: u8, fun: u8, inf: u8, dco: Dco, rii: u8) -> Asdu {
        let mut a = Asdu::new(
            TypeId::GENERAL_COMMAND,
            vsq_one(),
            Cause::GENERAL_COMMAND,
            common_addr,
        );
        a.append(&[fun, inf, dco.value(), rii]);
        a
    }
}

impl fmt::Display for Asdu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} @{} VSQ<{}>",
            self.type_id, self.coa, self.common_addr, self.variable.number
        )?;
        if self.info_obj.len() >= 2 {
            write!(f, " FUN={} INF={}", self.info_obj[0], self.info_obj[1])?;
        }
        match self.type_id {
            TypeId::TIME_TAGGED | TypeId::TIME_TAGGED_REL => {
                if let Ok(info) = self.get_time_tagged() {
                    write!(f, " DPI={}", info.dpi)?;
                    if let Some(t) = info.time {
                        write!(f, " @{}", t.format("%H:%M:%S%.3f"))?;
                    }
                }
            }
            TypeId::MEASURANDS_I | TypeId::MEASURANDS_II => {
                if let Ok(info) = self.get_measurands() {
                    f.write_str(" [")?;
                    for (i, m) in info.values.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{:.4}", m.f64())?;
                    }
                    f.write_str("]")?;
                }
            }
            TypeId::IDENTIFICATION => {
                if let Ok(id) = self.get_identification() {
                    write!(f, " COL={} {:?}", id.col, id.ascii)?;
                }
            }
            TypeId::GI_TERMINATION => {
                if let Ok(scn) = self.get_gi_termination() {
                    write!(f, " SCN={scn}")?;
                }
            }
            TypeId::GENERAL_COMMAND => {
                if let Ok(cmd) = self.get_general_command() {
                    write!(f, " DCO={} RII={}", cmd.dco, cmd.rii)?;
                }
            }
            _ => write!(f, " payload={}B", self.info_obj.len())?,
        }
        Ok(())
    }
}

/// Content of ASDU 1 (time-tagged message) and ASDU 2 (with relative time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimeTaggedInfo {
    /// Function type.
    pub fun: u8,
    /// Information number.
    pub inf: u8,
    /// The double-point state.
    pub dpi: Dpi,
    /// Relative time (RET); ASDU 2 only.
    pub relative_time: u16,
    /// Fault number (FAN); ASDU 2 only.
    pub fault_number: u16,
    /// Time of the event.
    pub time: Option<DateTime<Utc>>,
    /// Supplementary information. For a command acknowledgement (cause 20 or
    /// 21) this carries the RII of the original command.
    pub sin: u8,
}

/// Content of ASDU 3 (measurands I) and ASDU 9 (measurands II).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MeasurandsInfo {
    /// Function type.
    pub fun: u8,
    /// Information number, which names the measurand set.
    pub inf: u8,
    /// The measured values, in the order the information number defines.
    pub values: Vec<Measurand>,
}

/// Content of ASDU 4 (time-tagged measurands with relative time).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TimeTaggedMeasurandsInfo {
    /// Function type.
    pub fun: u8,
    /// Information number.
    pub inf: u8,
    /// Short-circuit location (SCL).
    pub value: f32,
    /// Relative time (RET).
    pub relative_time: u16,
    /// Fault number (FAN).
    pub fault_number: u16,
    /// Time of the event.
    pub time: Option<DateTime<Utc>>,
}

/// Content of ASDU 5 (identification message).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IdentificationInfo {
    /// Function type.
    pub fun: u8,
    /// Information number.
    pub inf: u8,
    /// Compatibility level.
    pub col: u8,
    /// Eight ASCII characters: manufacturer and product designation.
    pub ascii: String,
    /// Remaining freely assignable identification octets.
    pub extra: Vec<u8>,
}

/// Content of ASDU 20 (general command).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GeneralCommandInfo {
    /// Function type of the commanded object.
    pub fun: u8,
    /// Information number of the commanded object.
    pub inf: u8,
    /// The commanded state.
    pub dco: Dco,
    /// Return information identifier, echoed in the acknowledgement.
    pub rii: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone as _;

    #[test]
    fn identifier_round_trips_through_the_wire_format() {
        let mut a = Asdu::new(
            TypeId::TIME_TAGGED,
            VariableStruct {
                number: 1,
                is_sequence: true,
            },
            Cause::SPONTANEOUS,
            3,
        );
        a.append(&[fun::OVERCURRENT_PROTECTION, inf::GENERAL_TRIP]);
        let raw = a.marshal_binary().unwrap();
        assert_eq!(raw[..4], [1, 0x81, 1, 3]);
        assert_eq!(Asdu::unmarshal_binary(&raw).unwrap(), a);
    }

    #[test]
    fn marshal_rejects_an_unusable_identifier() {
        let a = Asdu::new(TypeId(0), vsq_one(), Cause::SPONTANEOUS, 1);
        assert_eq!(a.marshal_binary(), Err(Error::TypeIdentifier));

        let a = Asdu::new(TypeId::TIME_TAGGED, vsq_one(), Cause(0), 1);
        assert_eq!(a.marshal_binary(), Err(Error::CauseZero));

        let mut a = Asdu::new(TypeId::TIME_TAGGED, vsq_one(), Cause::SPONTANEOUS, 1);
        a.info_obj = vec![0; ASDU_SIZE_MAX];
        assert_eq!(a.marshal_binary(), Err(Error::LengthOutOfRange));
    }

    #[test]
    fn unmarshal_rejects_a_truncated_identifier() {
        assert_eq!(Asdu::unmarshal_binary(&[1, 2, 3]), Err(Error::UnexpectedEof));
    }

    /// Build an ASDU 1 the way a device would, to decode it back.
    fn time_tagged_frame(dpi: Dpi, sin: u8) -> Asdu {
        let mut a = Asdu::new(
            TypeId::TIME_TAGGED,
            vsq_one(),
            Cause::SPONTANEOUS,
            3,
        );
        a.append(&[fun::OVERCURRENT_PROTECTION, inf::GENERAL_TRIP, dpi.value()]);
        a.append(&cp32time2a(Some(Utc::now()), TimeZone::Utc));
        a.append(&[sin]);
        a
    }

    #[test]
    fn time_tagged_decodes_fun_inf_dpi_and_supplementary_information() {
        let a = time_tagged_frame(Dpi::On, 42);
        let info = a.get_time_tagged().unwrap();
        assert_eq!(info.fun, fun::OVERCURRENT_PROTECTION);
        assert_eq!(info.inf, inf::GENERAL_TRIP);
        assert_eq!(info.dpi, Dpi::On);
        assert_eq!(info.sin, 42, "a command acknowledgement carries the RII here");
        assert!(info.time.is_some());
    }

    #[test]
    fn time_tagged_with_relative_time_decodes_ret_and_fan() {
        let mut a = Asdu::new(TypeId::TIME_TAGGED_REL, vsq_one(), Cause::SPONTANEOUS, 3);
        a.append(&[fun::DISTANCE_PROTECTION, inf::TRIP_L1, Dpi::On.value()]);
        a.append(&1234u16.to_le_bytes());
        a.append(&7u16.to_le_bytes());
        a.append(&cp32time2a(Some(Utc::now()), TimeZone::Utc));
        a.append(&[9]);

        let info = a.get_time_tagged().unwrap();
        assert_eq!(info.relative_time, 1234);
        assert_eq!(info.fault_number, 7);
        assert_eq!(info.sin, 9);
    }

    #[test]
    fn a_truncated_time_tagged_message_is_rejected() {
        let mut a = Asdu::new(TypeId::TIME_TAGGED, vsq_one(), Cause::SPONTANEOUS, 3);
        a.append(&[160, 68, 2]);
        assert_eq!(a.get_time_tagged(), Err(Error::UnexpectedEof));
    }

    #[test]
    fn the_decoders_reject_a_mismatched_type() {
        let a = time_tagged_frame(Dpi::On, 0);
        assert_eq!(a.get_measurands(), Err(Error::TypeIdNotMatch));
        assert_eq!(a.get_identification(), Err(Error::TypeIdNotMatch));
        assert_eq!(a.get_gi_termination(), Err(Error::TypeIdNotMatch));
        assert_eq!(a.get_general_command(), Err(Error::TypeIdNotMatch));
        assert_eq!(a.get_time_sync(), Err(Error::TypeIdNotMatch));
    }

    #[test]
    fn measurands_decode_every_value_in_order() {
        let values = [
            Measurand {
                val: 2048,
                ..Default::default()
            },
            Measurand {
                val: -1024,
                overflow: true,
                invalid: false,
            },
            Measurand {
                val: 0,
                overflow: false,
                invalid: true,
            },
        ];
        let mut a = Asdu::new(
            TypeId::MEASURANDS_I,
            VariableStruct {
                number: values.len() as u8,
                is_sequence: true,
            },
            Cause::CYCLIC,
            3,
        );
        a.append(&[fun::OVERCURRENT_PROTECTION, inf::MEASURAND_IVPQ]);
        for m in &values {
            a.append(&m.value().to_le_bytes());
        }

        let info = a.get_measurands().unwrap();
        assert_eq!(info.inf, inf::MEASURAND_IVPQ);
        assert_eq!(info.values, values);
        assert_eq!(info.values[0].f64(), 0.5);
    }

    #[test]
    fn identification_trims_the_ascii_field() {
        let mut a = Asdu::new(TypeId::IDENTIFICATION, vsq_one(), Cause::RESET_CU, 3);
        a.append(&[fun::GLOBAL, inf::RESET_CU, 4]);
        a.append(b"RELAY-1 ");
        a.append(&[0xaa, 0xbb]);

        let id = a.get_identification().unwrap();
        assert_eq!(id.col, 4);
        assert_eq!(id.ascii, "RELAY-1");
        assert_eq!(id.extra, vec![0xaa, 0xbb]);
    }

    #[test]
    fn the_control_direction_builders_match_the_standard_layout() {
        let t = Utc.with_ymd_and_hms(2026, 8, 17, 12, 0, 0).unwrap();
        let a = Asdu::time_sync(3, t);
        assert_eq!(a.type_id, TypeId::TIME_SYNC);
        assert_eq!(a.coa, Cause::TIME_SYNC);
        assert_eq!(a.info_obj[..2], [fun::GLOBAL, 0]);
        assert_eq!(a.info_obj.len(), 2 + 7);
        assert_eq!(a.get_time_sync().unwrap(), Some(t));

        let a = Asdu::general_interrogation(3, 5);
        assert_eq!(a.coa, Cause::GI);
        assert_eq!(a.info_obj, vec![fun::GLOBAL, 0, 5]);
        assert_eq!(a.get_general_interrogation().unwrap(), 5);

        let a = Asdu::general_command(
            3,
            fun::OVERCURRENT_PROTECTION,
            inf::AUTO_RECLOSER_ACTIVE,
            Dco::On,
            42,
        );
        assert_eq!(a.coa, Cause::GENERAL_COMMAND);
        let cmd = a.get_general_command().unwrap();
        assert_eq!(cmd.fun, fun::OVERCURRENT_PROTECTION);
        assert_eq!(cmd.inf, inf::AUTO_RECLOSER_ACTIVE);
        assert_eq!(cmd.dco, Dco::On);
        assert_eq!(cmd.rii, 42);
    }

    #[test]
    fn gi_termination_carries_the_scan_number() {
        let mut a = Asdu::new(TypeId::GI_TERMINATION, vsq_one(), Cause::GI_TERMINATION, 3);
        a.append(&[fun::GLOBAL, 0, 5]);
        assert_eq!(a.get_gi_termination().unwrap(), 5);
    }

    #[test]
    fn time_tagged_measurands_decode_the_short_circuit_location() {
        let mut a = Asdu::new(
            TypeId::TIME_TAGGED_MEASURANDS,
            vsq_one(),
            Cause::SPONTANEOUS,
            3,
        );
        a.append(&[fun::DISTANCE_PROTECTION, inf::FAULT_LOCATION]);
        a.append(&12.5f32.to_bits().to_le_bytes());
        a.append(&100u16.to_le_bytes());
        a.append(&3u16.to_le_bytes());
        a.append(&cp32time2a(Some(Utc::now()), TimeZone::Utc));

        let info = a.get_time_tagged_measurands().unwrap();
        assert_eq!(info.value, 12.5);
        assert_eq!(info.relative_time, 100);
        assert_eq!(info.fault_number, 3);
        assert!(info.time.is_some());
    }

    #[test]
    fn display_summarises_without_panicking_on_short_payloads() {
        let a = time_tagged_frame(Dpi::On, 1);
        let s = a.to_string();
        assert!(s.starts_with("TimeTagged<1> Spontaneous @3"));
        assert!(s.contains("FUN=160 INF=68"));
        assert!(s.contains("DPI=On"));

        // A payload too short to decode must still format.
        let mut short = Asdu::new(TypeId::TIME_TAGGED, vsq_one(), Cause::SPONTANEOUS, 3);
        short.append(&[1, 2]);
        assert!(!short.to_string().is_empty());
    }
}
