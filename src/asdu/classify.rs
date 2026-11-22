// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

use super::{Asdu, TypeId, time::parse_cp56time2a};

impl TypeId {
    /// Whether this type carries a process operation.
    pub const fn is_process_command(self) -> bool {
        matches!(self.0, 45..=51 | 58..=64)
    }

    /// Whether this type carries a system command.
    pub const fn is_system_command(self) -> bool {
        matches!(self.0, 100..=107)
    }

    /// Whether this type loads or activates parameters.
    pub const fn is_parameter(self) -> bool {
        matches!(self.0, 110..=113)
    }

    /// Whether this type is a file transfer service.
    pub const fn is_file(self) -> bool {
        matches!(self.0, 120..=127)
    }

    /// Whether this type is assigned to IEC 62351-5 authentication.
    pub const fn is_security(self) -> bool {
        matches!(self.0, 81..=87 | 90..=95)
    }

    /// Whether this command carries a CP56Time2a tag.
    pub const fn is_time_tagged_command(self) -> bool {
        matches!(self.0, 58..=64 | 107)
    }

    /// Whether this type travels from the controlled station.
    pub const fn is_monitor_direction(self) -> bool {
        matches!(self.0, 1..=21 | 30..=41 | 70 | 120 | 121 | 123 | 125 | 126)
    }

    /// Whether this type is a request from the controlling station.
    pub const fn is_control_direction(self) -> bool {
        self.is_process_command()
            || self.is_system_command()
            || self.is_parameter()
            || matches!(self.0, 122 | 124 | 127)
    }

    /// Whether a system command can address all stations.
    pub const fn permits_broadcast(self) -> bool {
        matches!(self.0, 100 | 101 | 103 | 105)
    }

    /// Whether the type is selected for IEC 60870-5-104.
    pub const fn in_profile_104(self) -> bool {
        matches!(self.0, 1 | 3 | 5 | 7 | 9 | 11 | 13 | 15 | 20 | 21 | 30..=40 | 70 | 100..=103 | 105 | 107)
            || self.is_process_command()
            || self.is_parameter()
            || self.is_file()
            || self.is_security()
    }
}

/// A semantic defect in a decoded control-direction ASDU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RequestError {
    /// The request has more than one object or is a sequence.
    #[error("the command requires exactly one information object")]
    SingleObjectExpected,
    /// The type cannot be sent to the global address.
    #[error("this request cannot be broadcast")]
    BroadcastNotAllowed,
    /// The qualifier value is reserved.
    #[error("reserved command qualifier")]
    QualifierReserved,
    /// The double or regulating step command state is reserved.
    #[error("command state 0 or 3 is not permitted")]
    InvalidCommandState,
    /// A setpoint or parameter is NaN or infinite.
    #[error("command value must be finite")]
    NonFiniteValue,
    /// The CP56Time2a tag is invalid.
    #[error("invalid command time tag")]
    InvalidTimeTag,
}

impl Asdu {
    /// The first information object address, if the ASDU carries one.
    pub fn first_ioa(&self) -> Option<super::InfoObjAddr> {
        self.reader().info_obj_addr().ok()
    }

    /// Whether the common address is global for this ASDU's configured width.
    pub fn is_global(&self) -> bool {
        self.common_addr() == super::GLOBAL_COMMON_ADDR
            || (self.params.common_addr_size == 1 && self.common_addr() == 255)
    }

    /// The validated CP56Time2a of a time-tagged command.
    pub fn command_time_tag(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        if !self.type_id().is_time_tagged_command()
            || self.variable() != super::VariableStruct::single()
        {
            return None;
        }
        let bytes = self
            .info_obj
            .get(self.params.info_obj_addr_size as usize..)?;
        let offset = bytes.len().checked_sub(7)?;
        parse_cp56time2a(&bytes[offset..], self.params.info_obj_time_zone)
    }

    /// Validate request fields beyond the structural checks performed by the codec.
    pub fn validate_request(&self) -> Result<(), RequestError> {
        let t = self.type_id();
        if (t.is_process_command() || t.is_system_command())
            && self.variable() != super::VariableStruct::single()
        {
            return Err(RequestError::SingleObjectExpected);
        }
        if self.is_global()
            && (t.is_process_command()
                || t.is_parameter()
                || (t.is_system_command() && !t.permits_broadcast()))
        {
            return Err(RequestError::BroadcastNotAllowed);
        }
        let Some(body) = self.info_obj.get(self.params.info_obj_addr_size as usize..) else {
            return Ok(());
        };
        if let Some(&b) = body.first() {
            let invalid = match t {
                TypeId::C_IC_NA_1 => !matches!(b, 0 | 20..=36 | 64..=255),
                TypeId::C_CI_NA_1 => matches!(b & 0x3f, 0 | 6..=31),
                TypeId::C_RP_NA_1 => matches!(b, 0 | 3..=127),
                TypeId::C_DC_NA_1 | TypeId::C_DC_TA_1 | TypeId::C_RC_NA_1 | TypeId::C_RC_TA_1 => {
                    if matches!(b & 3, 0 | 3) {
                        return Err(RequestError::InvalidCommandState);
                    }
                    matches!((b >> 2) & 0x1f, 4..=15)
                }
                TypeId::C_SC_NA_1 | TypeId::C_SC_TA_1 => matches!((b >> 2) & 0x1f, 4..=15),
                _ => false,
            };
            if invalid {
                return Err(RequestError::QualifierReserved);
            }
        }
        if matches!(t, TypeId::C_SE_NC_1 | TypeId::C_SE_TC_1 | TypeId::P_ME_NC_1)
            && let Some(bytes) = body.first_chunk::<4>()
            && !f32::from_le_bytes(*bytes).is_finite()
        {
            return Err(RequestError::NonFiniteValue);
        }
        if (t.is_time_tagged_command() && self.command_time_tag().is_none())
            || (t == TypeId::C_CS_NA_1
                && parse_cp56time2a(body, self.params.info_obj_time_zone).is_none())
        {
            return Err(RequestError::InvalidTimeTag);
        }
        Ok(())
    }
}
