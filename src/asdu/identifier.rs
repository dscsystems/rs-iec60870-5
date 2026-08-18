// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The data unit identifier: type identification, variable structure qualifier,
//! cause of transmission and common address.

use std::fmt;

use crate::error::{Error, Result};

/// ASDU type identification. See companion standard 101, subclass 7.2.1.
///
/// A newtype over the wire octet so that private and reserved identifications
/// (`128..=255`) round-trip unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct TypeId(pub u8);

impl From<u8> for TypeId {
    fn from(v: u8) -> Self {
        TypeId(v)
    }
}

impl From<TypeId> for u8 {
    fn from(v: TypeId) -> Self {
        v.0
    }
}

/// The standard ASDU type identifications.
///
/// * `M_` monitored information, `C_` control information,
///   `P_` parameter, `F_` file transfer, `S_` IEC 62351-5 security.
/// * `<0>` unused, `<1..=127>` compatible standard definitions,
///   `<128..=255>` reserved for routed packets and private use.
///
/// Information objects with and without time tag are distinguished by
/// different type identifications, not by a flag.
impl TypeId {
    // -- Process information in the monitoring direction <1..=44> --------
    /// 1: single-point information
    pub const M_SP_NA_1: TypeId = TypeId(1);
    /// 2: single-point information with CP24Time2a time tag
    pub const M_SP_TA_1: TypeId = TypeId(2);
    /// 3: double-point information
    pub const M_DP_NA_1: TypeId = TypeId(3);
    /// 4: double-point information with CP24Time2a time tag
    pub const M_DP_TA_1: TypeId = TypeId(4);
    /// 5: step position information
    pub const M_ST_NA_1: TypeId = TypeId(5);
    /// 6: step position information with CP24Time2a time tag
    pub const M_ST_TA_1: TypeId = TypeId(6);
    /// 7: bitstring of 32 bits
    pub const M_BO_NA_1: TypeId = TypeId(7);
    /// 8: bitstring of 32 bits with CP24Time2a time tag
    pub const M_BO_TA_1: TypeId = TypeId(8);
    /// 9: measured value, normalized value
    pub const M_ME_NA_1: TypeId = TypeId(9);
    /// 10: measured value, normalized value with CP24Time2a time tag
    pub const M_ME_TA_1: TypeId = TypeId(10);
    /// 11: measured value, scaled value
    pub const M_ME_NB_1: TypeId = TypeId(11);
    /// 12: measured value, scaled value with CP24Time2a time tag
    pub const M_ME_TB_1: TypeId = TypeId(12);
    /// 13: measured value, short floating point number
    pub const M_ME_NC_1: TypeId = TypeId(13);
    /// 14: measured value, short floating point number with CP24Time2a time tag
    pub const M_ME_TC_1: TypeId = TypeId(14);
    /// 15: integrated totals
    pub const M_IT_NA_1: TypeId = TypeId(15);
    /// 16: integrated totals with CP24Time2a time tag
    pub const M_IT_TA_1: TypeId = TypeId(16);
    /// 17: event of protection equipment with CP24Time2a time tag
    pub const M_EP_TA_1: TypeId = TypeId(17);
    /// 18: packed start events of protection equipment with CP24Time2a time tag
    pub const M_EP_TB_1: TypeId = TypeId(18);
    /// 19: packed output circuit information of protection equipment, CP24Time2a
    pub const M_EP_TC_1: TypeId = TypeId(19);
    /// 20: packed single-point information with status change detection
    pub const M_PS_NA_1: TypeId = TypeId(20);
    /// 21: measured value, normalized value without quality descriptor
    pub const M_ME_ND_1: TypeId = TypeId(21);

    /// 30: single-point information with CP56Time2a time tag
    pub const M_SP_TB_1: TypeId = TypeId(30);
    /// 31: double-point information with CP56Time2a time tag
    pub const M_DP_TB_1: TypeId = TypeId(31);
    /// 32: step position information with CP56Time2a time tag
    pub const M_ST_TB_1: TypeId = TypeId(32);
    /// 33: bitstring of 32 bits with CP56Time2a time tag
    pub const M_BO_TB_1: TypeId = TypeId(33);
    /// 34: measured value, normalized value with CP56Time2a time tag
    pub const M_ME_TD_1: TypeId = TypeId(34);
    /// 35: measured value, scaled value with CP56Time2a time tag
    pub const M_ME_TE_1: TypeId = TypeId(35);
    /// 36: measured value, short floating point number with CP56Time2a time tag
    pub const M_ME_TF_1: TypeId = TypeId(36);
    /// 37: integrated totals with CP56Time2a time tag
    pub const M_IT_TB_1: TypeId = TypeId(37);
    /// 38: event of protection equipment with CP56Time2a time tag
    pub const M_EP_TD_1: TypeId = TypeId(38);
    /// 39: packed start events of protection equipment with CP56Time2a time tag
    pub const M_EP_TE_1: TypeId = TypeId(39);
    /// 40: packed output circuit information of protection equipment, CP56Time2a
    pub const M_EP_TF_1: TypeId = TypeId(40);
    /// 41: integrated totals containing time-tagged security statistics
    pub const S_IT_TC_1: TypeId = TypeId(41);

    // -- Process information in the control direction <45..=69> ----------
    /// 45: single command
    pub const C_SC_NA_1: TypeId = TypeId(45);
    /// 46: double command
    pub const C_DC_NA_1: TypeId = TypeId(46);
    /// 47: regulating step command
    pub const C_RC_NA_1: TypeId = TypeId(47);
    /// 48: set-point command, normalized value
    pub const C_SE_NA_1: TypeId = TypeId(48);
    /// 49: set-point command, scaled value
    pub const C_SE_NB_1: TypeId = TypeId(49);
    /// 50: set-point command, short floating point number
    pub const C_SE_NC_1: TypeId = TypeId(50);
    /// 51: bitstring of 32 bits command
    pub const C_BO_NA_1: TypeId = TypeId(51);
    /// 58: single command with CP56Time2a time tag
    pub const C_SC_TA_1: TypeId = TypeId(58);
    /// 59: double command with CP56Time2a time tag
    pub const C_DC_TA_1: TypeId = TypeId(59);
    /// 60: regulating step command with CP56Time2a time tag
    pub const C_RC_TA_1: TypeId = TypeId(60);
    /// 61: set-point command, normalized value, with CP56Time2a time tag
    pub const C_SE_TA_1: TypeId = TypeId(61);
    /// 62: set-point command, scaled value, with CP56Time2a time tag
    pub const C_SE_TB_1: TypeId = TypeId(62);
    /// 63: set-point command, short float, with CP56Time2a time tag
    pub const C_SE_TC_1: TypeId = TypeId(63);
    /// 64: bitstring of 32 bits command with CP56Time2a time tag
    pub const C_BO_TA_1: TypeId = TypeId(64);

    // -- System information in the monitoring direction <70..=99> --------
    /// 70: end of initialization
    pub const M_EI_NA_1: TypeId = TypeId(70);
    /// 81: authentication challenge (IEC 62351-5)
    pub const S_CH_NA_1: TypeId = TypeId(81);
    /// 82: authentication reply (IEC 62351-5)
    pub const S_RP_NA_1: TypeId = TypeId(82);
    /// 83: aggressive mode authentication request (IEC 62351-5)
    pub const S_AR_NA_1: TypeId = TypeId(83);
    /// 84: session key status request (IEC 62351-5)
    pub const S_KR_NA_1: TypeId = TypeId(84);
    /// 85: session key status (IEC 62351-5)
    pub const S_KS_NA_1: TypeId = TypeId(85);
    /// 86: session key change (IEC 62351-5)
    pub const S_KC_NA_1: TypeId = TypeId(86);
    /// 87: authentication error (IEC 62351-5)
    pub const S_ER_NA_1: TypeId = TypeId(87);
    /// 90: user status change (IEC 62351-5)
    pub const S_US_NA_1: TypeId = TypeId(90);
    /// 91: update key change request (IEC 62351-5)
    pub const S_UQ_NA_1: TypeId = TypeId(91);
    /// 92: update key change reply (IEC 62351-5)
    pub const S_UR_NA_1: TypeId = TypeId(92);
    /// 93: update key change, symmetric (IEC 62351-5)
    pub const S_UK_NA_1: TypeId = TypeId(93);
    /// 94: update key change, asymmetric (IEC 62351-5)
    pub const S_UA_NA_1: TypeId = TypeId(94);
    /// 95: update key change confirmation (IEC 62351-5)
    pub const S_UC_NA_1: TypeId = TypeId(95);

    // -- System information in the control direction <100..=109> ---------
    /// 100: interrogation command
    pub const C_IC_NA_1: TypeId = TypeId(100);
    /// 101: counter interrogation command
    pub const C_CI_NA_1: TypeId = TypeId(101);
    /// 102: read command
    pub const C_RD_NA_1: TypeId = TypeId(102);
    /// 103: clock synchronization command
    pub const C_CS_NA_1: TypeId = TypeId(103);
    /// 104: test command
    pub const C_TS_NA_1: TypeId = TypeId(104);
    /// 105: reset process command
    pub const C_RP_NA_1: TypeId = TypeId(105);
    /// 106: delay acquisition command (IEC 101 only)
    pub const C_CD_NA_1: TypeId = TypeId(106);
    /// 107: test command with CP56Time2a time tag
    pub const C_TS_TA_1: TypeId = TypeId(107);

    // -- Parameter in the control direction <110..=119> ------------------
    /// 110: parameter of measured value, normalized value
    pub const P_ME_NA_1: TypeId = TypeId(110);
    /// 111: parameter of measured value, scaled value
    pub const P_ME_NB_1: TypeId = TypeId(111);
    /// 112: parameter of measured value, short floating point number
    pub const P_ME_NC_1: TypeId = TypeId(112);
    /// 113: parameter activation
    pub const P_AC_NA_1: TypeId = TypeId(113);

    // -- File transfer <120..=127> ---------------------------------------
    /// 120: file ready
    pub const F_FR_NA_1: TypeId = TypeId(120);
    /// 121: section ready
    pub const F_SR_NA_1: TypeId = TypeId(121);
    /// 122: call directory, select file, call file, call section
    pub const F_SC_NA_1: TypeId = TypeId(122);
    /// 123: last section, last segment
    pub const F_LS_NA_1: TypeId = TypeId(123);
    /// 124: ack file, ack section
    pub const F_AF_NA_1: TypeId = TypeId(124);
    /// 125: segment
    pub const F_SG_NA_1: TypeId = TypeId(125);
    /// 126: directory
    pub const F_DR_TA_1: TypeId = TypeId(126);
    /// 127: QueryLog, request archive file (IEC 104 section)
    pub const F_SC_NB_1: TypeId = TypeId(127);

    /// The mnemonic of a standard type identification, e.g. `"M_SP_NA_1"`.
    ///
    /// Returns `None` for reserved and private identifications.
    pub const fn name(self) -> Option<&'static str> {
        const NAMES_1_21: [&str; 21] = [
            "M_SP_NA_1", "M_SP_TA_1", "M_DP_NA_1", "M_DP_TA_1", "M_ST_NA_1", "M_ST_TA_1",
            "M_BO_NA_1", "M_BO_TA_1", "M_ME_NA_1", "M_ME_TA_1", "M_ME_NB_1", "M_ME_TB_1",
            "M_ME_NC_1", "M_ME_TC_1", "M_IT_NA_1", "M_IT_TA_1", "M_EP_TA_1", "M_EP_TB_1",
            "M_EP_TC_1", "M_PS_NA_1", "M_ME_ND_1",
        ];
        const NAMES_30_41: [&str; 12] = [
            "M_SP_TB_1", "M_DP_TB_1", "M_ST_TB_1", "M_BO_TB_1", "M_ME_TD_1", "M_ME_TE_1",
            "M_ME_TF_1", "M_IT_TB_1", "M_EP_TD_1", "M_EP_TE_1", "M_EP_TF_1", "S_IT_TC_1",
        ];
        const NAMES_45_51: [&str; 7] = [
            "C_SC_NA_1", "C_DC_NA_1", "C_RC_NA_1", "C_SE_NA_1", "C_SE_NB_1", "C_SE_NC_1",
            "C_BO_NA_1",
        ];
        const NAMES_58_64: [&str; 7] = [
            "C_SC_TA_1", "C_DC_TA_1", "C_RC_TA_1", "C_SE_TA_1", "C_SE_TB_1", "C_SE_TC_1",
            "C_BO_TA_1",
        ];
        const NAMES_81_87: [&str; 7] = [
            "S_CH_NA_1", "S_RP_NA_1", "S_AR_NA_1", "S_KR_NA_1", "S_KS_NA_1", "S_KC_NA_1",
            "S_ER_NA_1",
        ];
        const NAMES_90_95: [&str; 6] = [
            "S_US_NA_1", "S_UQ_NA_1", "S_UR_NA_1", "S_UK_NA_1", "S_UA_NA_1", "S_UC_NA_1",
        ];
        const NAMES_100_107: [&str; 8] = [
            "C_IC_NA_1", "C_CI_NA_1", "C_RD_NA_1", "C_CS_NA_1", "C_TS_NA_1", "C_RP_NA_1",
            "C_CD_NA_1", "C_TS_TA_1",
        ];
        const NAMES_110_113: [&str; 4] = ["P_ME_NA_1", "P_ME_NB_1", "P_ME_NC_1", "P_AC_NA_1"];
        const NAMES_120_127: [&str; 8] = [
            "F_FR_NA_1", "F_SR_NA_1", "F_SC_NA_1", "F_LS_NA_1", "F_AF_NA_1", "F_SG_NA_1",
            "F_DR_TA_1", "F_SC_NB_1",
        ];

        let v = self.0;
        match v {
            1..=21 => Some(NAMES_1_21[(v - 1) as usize]),
            30..=41 => Some(NAMES_30_41[(v - 30) as usize]),
            45..=51 => Some(NAMES_45_51[(v - 45) as usize]),
            58..=64 => Some(NAMES_58_64[(v - 58) as usize]),
            70 => Some("M_EI_NA_1"),
            81..=87 => Some(NAMES_81_87[(v - 81) as usize]),
            90..=95 => Some(NAMES_90_95[(v - 90) as usize]),
            100..=107 => Some(NAMES_100_107[(v - 100) as usize]),
            110..=113 => Some(NAMES_110_113[(v - 110) as usize]),
            120..=127 => Some(NAMES_120_127[(v - 120) as usize]),
            _ => None,
        }
    }

    /// Size in octets of one information element set of this type, excluding
    /// the information object address.
    ///
    /// Returns [`Error::TypeIdentifier`] for types this library cannot size,
    /// which includes the variable-length `F_SG_NA_1` segment type.
    pub const fn info_obj_size(self) -> Result<usize> {
        let size = match self.0 {
            1 | 3 => 1,     // M_SP_NA_1, M_DP_NA_1
            2 | 4 => 4,     // M_SP_TA_1, M_DP_TA_1
            5 => 2,         // M_ST_NA_1
            6 => 5,         // M_ST_TA_1
            7 => 5,         // M_BO_NA_1
            8 => 8,         // M_BO_TA_1
            9 | 11 => 3,    // M_ME_NA_1, M_ME_NB_1
            10 | 12 => 6,   // M_ME_TA_1, M_ME_TB_1
            13 => 5,        // M_ME_NC_1
            14 => 8,        // M_ME_TC_1
            15 => 5,        // M_IT_NA_1
            16 => 8,        // M_IT_TA_1
            17 => 6,        // M_EP_TA_1
            18 | 19 => 7,   // M_EP_TB_1, M_EP_TC_1
            20 => 5,        // M_PS_NA_1
            21 => 2,        // M_ME_ND_1
            30 | 31 => 8,   // M_SP_TB_1, M_DP_TB_1
            32 => 9,        // M_ST_TB_1
            33 => 12,       // M_BO_TB_1
            34 | 35 => 10,  // M_ME_TD_1, M_ME_TE_1
            36 | 37 => 12,  // M_ME_TF_1, M_IT_TB_1
            38 => 10,       // M_EP_TD_1
            39 | 40 => 11,  // M_EP_TE_1, M_EP_TF_1
            45..=47 => 1,   // C_SC_NA_1, C_DC_NA_1, C_RC_NA_1
            48 | 49 => 3,   // C_SE_NA_1, C_SE_NB_1
            50 => 5,        // C_SE_NC_1
            51 => 4,        // C_BO_NA_1
            58..=60 => 8,   // C_SC_TA_1, C_DC_TA_1, C_RC_TA_1
            61 | 62 => 10,  // C_SE_TA_1, C_SE_TB_1
            63 => 12,       // C_SE_TC_1
            64 => 11,       // C_BO_TA_1
            70 => 1,        // M_EI_NA_1
            100 | 101 => 1, // C_IC_NA_1, C_CI_NA_1
            102 => 0,       // C_RD_NA_1
            103 => 7,       // C_CS_NA_1
            104 => 2,       // C_TS_NA_1
            105 => 1,       // C_RP_NA_1
            106 => 2,       // C_CD_NA_1
            107 => 9,       // C_TS_TA_1
            110 | 111 => 3, // P_ME_NA_1, P_ME_NB_1
            112 => 5,       // P_ME_NC_1
            113 => 1,       // P_AC_NA_1
            120 => 6,       // F_FR_NA_1
            121 => 7,       // F_SR_NA_1
            122 => 4,       // F_SC_NA_1
            123 => 5,       // F_LS_NA_1
            124 => 4,       // F_AF_NA_1
            126 => 13,      // F_DR_TA_1
            _ => return Err(Error::TypeIdentifier),
        };
        Ok(size)
    }
}

impl fmt::Display for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.name() {
            Some(n) => write!(f, "TID<{n}>"),
            None => write!(f, "TID<{}>", self.0),
        }
    }
}

/// Variable structure qualifier. See companion standard 101, subclass 7.2.2.
///
/// * `number` (bits 0..=6): count of information objects or elements, `0..=127`.
/// * `is_sequence` (bit 7): when set, the objects share one starting address and
///   the following addresses are implied by incrementing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct VariableStruct {
    /// Number of information objects (SQ = 0) or elements (SQ = 1).
    pub number: u8,
    /// The SQ bit: a sequence of elements sharing one starting address.
    pub is_sequence: bool,
}

impl VariableStruct {
    /// A non-sequence qualifier for `number` information objects.
    pub const fn new(number: u8) -> Self {
        VariableStruct {
            number,
            is_sequence: false,
        }
    }

    /// A qualifier for a single information object (the common case for commands).
    pub const fn single() -> Self {
        VariableStruct {
            number: 1,
            is_sequence: false,
        }
    }

    /// Parse the wire octet.
    pub const fn parse(b: u8) -> Self {
        VariableStruct {
            number: b & 0x7f,
            is_sequence: (b & 0x80) == 0x80,
        }
    }

    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        if self.is_sequence {
            self.number | 0x80
        } else {
            self.number
        }
    }
}

impl fmt::Display for VariableStruct {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_sequence {
            write!(f, "VSQ<sq,{}>", self.number)
        } else {
            write!(f, "VSQ<{}>", self.number)
        }
    }
}

/// Cause of transmission, bits 0..=5. See companion standard 101, subclass 7.2.3.
///
/// `<0>` undefined, `<1..=47>` standard definitions, `<48..=63>` private range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Cause(pub u8);

impl Cause {
    /// 0: unused (invalid on the wire)
    pub const UNUSED: Cause = Cause(0);
    /// 1: periodic, cyclic
    pub const PERIODIC: Cause = Cause(1);
    /// 2: background scan
    pub const BACKGROUND: Cause = Cause(2);
    /// 3: spontaneous
    pub const SPONTANEOUS: Cause = Cause(3);
    /// 4: initialized
    pub const INITIALIZED: Cause = Cause(4);
    /// 5: request or requested
    pub const REQUEST: Cause = Cause(5);
    /// 6: activation
    pub const ACTIVATION: Cause = Cause(6);
    /// 7: activation confirmation
    pub const ACTIVATION_CON: Cause = Cause(7);
    /// 8: deactivation
    pub const DEACTIVATION: Cause = Cause(8);
    /// 9: deactivation confirmation
    pub const DEACTIVATION_CON: Cause = Cause(9);
    /// 10: activation termination
    pub const ACTIVATION_TERM: Cause = Cause(10);
    /// 11: return information caused by a remote command
    pub const RETURN_INFO_REMOTE: Cause = Cause(11);
    /// 12: return information caused by a local command
    pub const RETURN_INFO_LOCAL: Cause = Cause(12);
    /// 13: file transfer
    pub const FILE_TRANSFER: Cause = Cause(13);
    /// 14: authentication (IEC 62351-5)
    pub const AUTHENTICATION: Cause = Cause(14);
    /// 15: maintenance of authentication session key (IEC 62351-5)
    pub const SESSION_KEY: Cause = Cause(15);
    /// 16: maintenance of user role and update key (IEC 62351-5)
    pub const USER_ROLE_AND_UPDATE_KEY: Cause = Cause(16);
    /// 20: interrogated by station interrogation
    pub const INTERROGATED_BY_STATION: Cause = Cause(20);
    /// 21: interrogated by group 1 interrogation
    pub const INTERROGATED_BY_GROUP1: Cause = Cause(21);
    /// 36: interrogated by group 16 interrogation
    pub const INTERROGATED_BY_GROUP16: Cause = Cause(36);
    /// 37: requested by general counter request
    pub const REQUEST_BY_GENERAL_COUNTER: Cause = Cause(37);
    /// 38: requested by group 1 counter request
    pub const REQUEST_BY_GROUP1_COUNTER: Cause = Cause(38);
    /// 41: requested by group 4 counter request
    pub const REQUEST_BY_GROUP4_COUNTER: Cause = Cause(41);
    /// 44: unknown type identification
    pub const UNKNOWN_TYPE_ID: Cause = Cause(44);
    /// 45: unknown cause of transmission
    pub const UNKNOWN_COT: Cause = Cause(45);
    /// 46: unknown common address of ASDU
    pub const UNKNOWN_CA: Cause = Cause(46);
    /// 47: unknown information object address
    pub const UNKNOWN_IOA: Cause = Cause(47);

    /// `interrogated by group N interrogation` for `n` in `1..=16`.
    pub const fn interrogated_by_group(n: u8) -> Result<Cause> {
        if n == 0 || n > 16 {
            return Err(Error::InroGroupNumFit);
        }
        Ok(Cause(20 + n))
    }

    /// True when this is `InterrogatedByStation` or any group interrogation cause.
    pub const fn is_interrogation(self) -> bool {
        self.0 >= Self::INTERROGATED_BY_STATION.0 && self.0 <= Self::INTERROGATED_BY_GROUP16.0
    }

    /// True when this is any of the counter request causes (37..=41).
    pub const fn is_counter_request(self) -> bool {
        self.0 >= Self::REQUEST_BY_GENERAL_COUNTER.0 && self.0 <= Self::REQUEST_BY_GROUP4_COUNTER.0
    }

    /// The semantic name of this cause, e.g. `"Spontaneous"`.
    pub const fn name(self) -> &'static str {
        const SEMANTICS: [&str; 64] = [
            "Unused0",
            "Periodic",
            "Background",
            "Spontaneous",
            "Initialized",
            "Request",
            "Activation",
            "ActivationCon",
            "Deactivation",
            "DeactivationCon",
            "ActivationTerm",
            "ReturnInfoRemote",
            "ReturnInfoLocal",
            "FileTransfer",
            "Authentication",
            "SessionKey",
            "UserRoleAndUpdateKey",
            "Reserved17",
            "Reserved18",
            "Reserved19",
            "InterrogatedByStation",
            "InterrogatedByGroup1",
            "InterrogatedByGroup2",
            "InterrogatedByGroup3",
            "InterrogatedByGroup4",
            "InterrogatedByGroup5",
            "InterrogatedByGroup6",
            "InterrogatedByGroup7",
            "InterrogatedByGroup8",
            "InterrogatedByGroup9",
            "InterrogatedByGroup10",
            "InterrogatedByGroup11",
            "InterrogatedByGroup12",
            "InterrogatedByGroup13",
            "InterrogatedByGroup14",
            "InterrogatedByGroup15",
            "InterrogatedByGroup16",
            "RequestByGeneralCounter",
            "RequestByGroup1Counter",
            "RequestByGroup2Counter",
            "RequestByGroup3Counter",
            "RequestByGroup4Counter",
            "Reserved42",
            "Reserved43",
            "UnknownTypeID",
            "UnknownCOT",
            "UnknownCA",
            "UnknownIOA",
            "Special48",
            "Special49",
            "Special50",
            "Special51",
            "Special52",
            "Special53",
            "Special54",
            "Special55",
            "Special56",
            "Special57",
            "Special58",
            "Special59",
            "Special60",
            "Special61",
            "Special62",
            "Special63",
        ];
        SEMANTICS[(self.0 & 0x3f) as usize]
    }
}

impl From<u8> for Cause {
    fn from(v: u8) -> Self {
        Cause(v & 0x3f)
    }
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The cause of transmission octet: cause plus the T and P/N flags.
///
/// ```text
/// | T | P/N | 5..0 cause |
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CauseOfTransmission {
    /// The T bit: this transmission is caused by a test.
    pub is_test: bool,
    /// The P/N bit: negative confirmation of the requested activation.
    pub is_negative: bool,
    /// The cause itself, bits 0..=5.
    pub cause: Cause,
}

impl CauseOfTransmission {
    /// A plain cause of transmission with both flags clear.
    pub const fn new(cause: Cause) -> Self {
        CauseOfTransmission {
            is_test: false,
            is_negative: false,
            cause,
        }
    }

    /// Parse the wire octet.
    pub const fn parse(b: u8) -> Self {
        CauseOfTransmission {
            is_negative: (b & 0x40) == 0x40,
            is_test: (b & 0x80) == 0x80,
            cause: Cause(b & 0x3f),
        }
    }

    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        let mut v = self.cause.0 & 0x3f;
        if self.is_negative {
            v |= 0x40;
        }
        if self.is_test {
            v |= 0x80;
        }
        v
    }

    /// Return a copy with the negative confirmation flag set.
    pub const fn negative(mut self) -> Self {
        self.is_negative = true;
        self
    }

    /// Return a copy with the test flag set.
    pub const fn test(mut self) -> Self {
        self.is_test = true;
        self
    }
}

impl From<Cause> for CauseOfTransmission {
    fn from(cause: Cause) -> Self {
        CauseOfTransmission::new(cause)
    }
}

impl fmt::Display for CauseOfTransmission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "COT<{}", self.cause.name())?;
        match (self.is_negative, self.is_test) {
            (true, true) => f.write_str(",neg,test")?,
            (true, false) => f.write_str(",neg")?,
            (false, true) => f.write_str(",test")?,
            (false, false) => {}
        }
        f.write_str(">")
    }
}

/// Originator address `[1, 255]`, or 0 for the default.
///
/// Only carried on the wire when `Params::cause_size == 2`.
pub type OriginAddr = u8;

/// A station (common) address. See companion standard 101, subclass 7.2.4.
///
/// The width on the wire is controlled by `Params::common_addr_size`:
/// width 1 uses `<1..=254>` with `255` as the global address, width 2 uses
/// `<1..=65534>` with `65535` as the global address.
pub type CommonAddr = u16;

/// The invalid common address; 0 is never a usable station address.
pub const INVALID_COMMON_ADDR: CommonAddr = 0;

/// The broadcast (global) common address.
///
/// Use this value even with 1-octet addressing — it is mapped to `255` on the
/// wire automatically. Broadcast is only legal for `C_IC_NA_1`, `C_CI_NA_1`,
/// `C_CS_NA_1` and `C_RP_NA_1`.
pub const GLOBAL_COMMON_ADDR: CommonAddr = 65535;

/// The complete application service data unit identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Identifier {
    /// Type identification: what the information objects contain.
    pub type_id: TypeId,
    /// Variable structure qualifier: how many objects, and whether they are a sequence.
    pub variable: VariableStruct,
    /// Cause of transmission.
    pub coa: CauseOfTransmission,
    /// Originator address; on the wire only when `cause_size == 2`.
    pub orig_addr: OriginAddr,
    /// Station address. Zero is not used.
    pub common_addr: CommonAddr,
}

impl Identifier {
    /// Build an identifier with a default (zero) originator address.
    pub const fn new(
        type_id: TypeId,
        variable: VariableStruct,
        coa: CauseOfTransmission,
        common_addr: CommonAddr,
    ) -> Self {
        Identifier {
            type_id,
            variable,
            coa,
            orig_addr: 0,
            common_addr,
        }
    }
}

impl fmt::Display for Identifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.orig_addr == 0 {
            write!(f, "{} {} @{}", self.type_id, self.coa, self.common_addr)
        } else {
            write!(
                f,
                "{} {} {}@{}",
                self.type_id, self.coa, self.orig_addr, self.common_addr
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_id_names_cover_the_standard_ranges() {
        assert_eq!(TypeId::M_SP_NA_1.name(), Some("M_SP_NA_1"));
        assert_eq!(TypeId::M_ME_ND_1.name(), Some("M_ME_ND_1"));
        assert_eq!(TypeId::M_SP_TB_1.name(), Some("M_SP_TB_1"));
        assert_eq!(TypeId::S_IT_TC_1.name(), Some("S_IT_TC_1"));
        assert_eq!(TypeId::C_BO_TA_1.name(), Some("C_BO_TA_1"));
        assert_eq!(TypeId::M_EI_NA_1.name(), Some("M_EI_NA_1"));
        assert_eq!(TypeId::C_TS_TA_1.name(), Some("C_TS_TA_1"));
        assert_eq!(TypeId::F_SC_NB_1.name(), Some("F_SC_NB_1"));
        assert_eq!(TypeId(200).name(), None);
        assert_eq!(TypeId(200).to_string(), "TID<200>");
    }

    #[test]
    fn variable_struct_round_trips() {
        for n in 0u8..=127 {
            for sq in [false, true] {
                let v = VariableStruct {
                    number: n,
                    is_sequence: sq,
                };
                assert_eq!(VariableStruct::parse(v.value()), v);
            }
        }
    }

    #[test]
    fn cause_of_transmission_round_trips() {
        for b in 0u8..=255 {
            assert_eq!(CauseOfTransmission::parse(b).value(), b);
        }
    }

    #[test]
    fn cause_classification() {
        assert!(Cause::INTERROGATED_BY_STATION.is_interrogation());
        assert!(Cause::INTERROGATED_BY_GROUP16.is_interrogation());
        assert!(!Cause::REQUEST_BY_GENERAL_COUNTER.is_interrogation());
        assert!(Cause::REQUEST_BY_GROUP4_COUNTER.is_counter_request());
        assert_eq!(Cause::interrogated_by_group(1), Ok(Cause(21)));
        assert_eq!(Cause::interrogated_by_group(16), Ok(Cause(36)));
        assert!(Cause::interrogated_by_group(17).is_err());
    }

    #[test]
    fn info_obj_sizes_match_the_standard() {
        assert_eq!(TypeId::M_SP_NA_1.info_obj_size(), Ok(1));
        assert_eq!(TypeId::M_ME_NC_1.info_obj_size(), Ok(5));
        assert_eq!(TypeId::M_ME_TF_1.info_obj_size(), Ok(12));
        assert_eq!(TypeId::C_RD_NA_1.info_obj_size(), Ok(0));
        assert_eq!(TypeId::C_TS_TA_1.info_obj_size(), Ok(9));
        assert!(TypeId::F_SG_NA_1.info_obj_size().is_err());
        assert!(TypeId(0).info_obj_size().is_err());
    }
}
