// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! File transfer: the `F_*` types that move a file from a controlled station
//! to the master, section by section and segment by segment.
//!
//! See companion standard 101, subclass 7.3.6 (the ASDUs) and subclass 7.4.11
//! (the transfer procedures).
//!
//! | Type | Direction | Purpose |
//! |------|-----------|---------|
//! | [`F_FR_NA_1`](TypeId::F_FR_NA_1) `<120>` | monitor | file ready |
//! | [`F_SR_NA_1`](TypeId::F_SR_NA_1) `<121>` | monitor | section ready |
//! | [`F_SC_NA_1`](TypeId::F_SC_NA_1) `<122>` | control | call directory, select file, call file, call section |
//! | [`F_LS_NA_1`](TypeId::F_LS_NA_1) `<123>` | monitor | last section, last segment |
//! | [`F_AF_NA_1`](TypeId::F_AF_NA_1) `<124>` | control | acknowledge file, acknowledge section |
//! | [`F_SG_NA_1`](TypeId::F_SG_NA_1) `<125>` | monitor | segment |
//! | [`F_DR_TA_1`](TypeId::F_DR_TA_1) `<126>` | monitor | directory |
//!
//! `F_SC_NB_1 <127>` (query log) is not implemented.
//!
//! Every type but the directory carries exactly one information object
//! (SQ = 0). The segment type is the only one in the compatible range whose
//! object length is not fixed by the type identification: it carries its own
//! LOS (length of segment) octet, which is why
//! [`Asdu::unmarshal_binary`] sizes it from the payload rather than from the
//! variable structure qualifier.

use chrono::{DateTime, Utc};

use crate::asdu::codec::{Asdu, check_valid};
use crate::asdu::identifier::{
    Cause, CauseOfTransmission, CommonAddr, Identifier, TypeId, VariableStruct,
};
use crate::asdu::info::InfoObjAddr;
use crate::asdu::params::Params;
use crate::asdu::time::TimeTagFlags;
use crate::error::{Error, Result};

/// The name (identification) of a file.
///
/// See companion standard 101, subclass 7.2.6.33. Values above the compatible
/// range are available for private assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct NameOfFile(pub u16);

impl NameOfFile {
    /// 0: default.
    pub const DEFAULT: NameOfFile = NameOfFile(0);
    /// 1: transparent file.
    pub const TRANSPARENT: NameOfFile = NameOfFile(1);
    /// 2: transmission of disturbance data of protection equipment.
    pub const DISTURBANCE_DATA: NameOfFile = NameOfFile(2);
    /// 3: transmission of sequences of events.
    pub const SEQUENCES_OF_EVENTS: NameOfFile = NameOfFile(3);
    /// 4: transmission of sequences of recorded analogue values.
    pub const SEQUENCES_OF_ANALOGUES: NameOfFile = NameOfFile(4);
}

impl From<u16> for NameOfFile {
    fn from(v: u16) -> Self {
        NameOfFile(v)
    }
}

impl std::fmt::Display for NameOfFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match *self {
            NameOfFile::DEFAULT => "Default",
            NameOfFile::TRANSPARENT => "Transparent",
            NameOfFile::DISTURBANCE_DATA => "DisturbanceData",
            NameOfFile::SEQUENCES_OF_EVENTS => "SequencesOfEvents",
            NameOfFile::SEQUENCES_OF_ANALOGUES => "SequencesOfAnalogues",
            _ => return write!(f, "NameOfFile({})", self.0),
        };
        f.write_str(name)
    }
}

/// The error code in the high nibble of the select-and-call qualifier (SCQ)
/// and of the acknowledge qualifier (AFQ).
///
/// See companion standard 101, subclasses 7.2.6.31 and 7.2.6.33.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct FileError(pub u8);

impl FileError {
    /// 0: default, no error.
    pub const NONE: FileError = FileError(0);
    /// 1: requested memory space not available.
    pub const MEMORY_UNAVAILABLE: FileError = FileError(1);
    /// 2: checksum failed.
    pub const CHECKSUM_FAILED: FileError = FileError(2);
    /// 3: unexpected communication service.
    pub const UNEXPECTED_SERVICE: FileError = FileError(3);
    /// 4: unexpected name of file.
    pub const UNEXPECTED_NAME_OF_FILE: FileError = FileError(4);
    /// 5: unexpected name of section.
    pub const UNEXPECTED_NAME_OF_SECTION: FileError = FileError(5);

    /// True when no error is reported.
    pub const fn is_none(self) -> bool {
        self.0 == 0
    }
}

impl From<u8> for FileError {
    fn from(v: u8) -> Self {
        FileError(v)
    }
}

impl std::fmt::Display for FileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match *self {
            FileError::NONE => "None",
            FileError::MEMORY_UNAVAILABLE => "MemoryUnavailable",
            FileError::CHECKSUM_FAILED => "ChecksumFailed",
            FileError::UNEXPECTED_SERVICE => "UnexpectedService",
            FileError::UNEXPECTED_NAME_OF_FILE => "UnexpectedNameOfFile",
            FileError::UNEXPECTED_NAME_OF_SECTION => "UnexpectedNameOfSection",
            _ => return write!(f, "FileError({})", self.0),
        };
        f.write_str(name)
    }
}

/// The action in the low nibble of the select-and-call qualifier (SCQ).
///
/// See companion standard 101, subclass 7.2.6.31.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct ScqAction(pub u8);

impl ScqAction {
    /// 0: default (call directory).
    pub const DEFAULT: ScqAction = ScqAction(0);
    /// 1: select file.
    pub const SELECT_FILE: ScqAction = ScqAction(1);
    /// 2: request file.
    pub const REQUEST_FILE: ScqAction = ScqAction(2);
    /// 3: deactivate file.
    pub const DEACTIVATE_FILE: ScqAction = ScqAction(3);
    /// 4: delete file.
    pub const DELETE_FILE: ScqAction = ScqAction(4);
    /// 5: select section.
    pub const SELECT_SECTION: ScqAction = ScqAction(5);
    /// 6: request section.
    pub const REQUEST_SECTION: ScqAction = ScqAction(6);
    /// 7: deactivate section.
    pub const DEACTIVATE_SECTION: ScqAction = ScqAction(7);
}

impl From<u8> for ScqAction {
    fn from(v: u8) -> Self {
        ScqAction(v)
    }
}

/// The action in the low nibble of the acknowledge file or section qualifier
/// (AFQ).
///
/// See companion standard 101, subclass 7.2.6.33.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct AfqAction(pub u8);

impl AfqAction {
    /// 0: not used.
    pub const NOT_USED: AfqAction = AfqAction(0);
    /// 1: positive acknowledge of file transfer.
    pub const POS_ACK_FILE: AfqAction = AfqAction(1);
    /// 2: negative acknowledge of file transfer.
    pub const NEG_ACK_FILE: AfqAction = AfqAction(2);
    /// 3: positive acknowledge of section transfer.
    pub const POS_ACK_SECTION: AfqAction = AfqAction(3);
    /// 4: negative acknowledge of section transfer.
    pub const NEG_ACK_SECTION: AfqAction = AfqAction(4);
}

impl From<u8> for AfqAction {
    fn from(v: u8) -> Self {
        AfqAction(v)
    }
}

/// The last section or segment qualifier (LSQ).
///
/// See companion standard 101, subclass 7.2.6.32.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct LastSectionQualifier(pub u8);

impl LastSectionQualifier {
    /// 0: not used.
    pub const NOT_USED: LastSectionQualifier = LastSectionQualifier(0);
    /// 1: file transfer without deactivation.
    pub const FILE_WITHOUT_DEACTIVATE: LastSectionQualifier = LastSectionQualifier(1);
    /// 2: file transfer with deactivation.
    pub const FILE_WITH_DEACTIVATE: LastSectionQualifier = LastSectionQualifier(2);
    /// 3: section transfer without deactivation.
    pub const SECTION_WITHOUT_DEACTIVATE: LastSectionQualifier = LastSectionQualifier(3);
    /// 4: section transfer with deactivation.
    pub const SECTION_WITH_DEACTIVATE: LastSectionQualifier = LastSectionQualifier(4);

    /// True when this marks the end of a whole file rather than of a section.
    pub const fn is_end_of_file(self) -> bool {
        self.0 == 1 || self.0 == 2
    }
}

impl From<u8> for LastSectionQualifier {
    fn from(v: u8) -> Self {
        LastSectionQualifier(v)
    }
}

/// The file ready qualifier (FRQ).
///
/// See companion standard 101, subclass 7.2.6.29. `qual` is bits 0..=6 and the
/// P/N bit is bit 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileReadyQualifier {
    /// Implementation-defined qualifier, `0..=127`.
    pub qual: u8,
    /// P/N: the transfer of this file is *not* possible.
    pub is_negative: bool,
}

impl FileReadyQualifier {
    /// Decode the wire octet.
    pub const fn parse(b: u8) -> Self {
        FileReadyQualifier {
            qual: b & 0x7f,
            is_negative: b & 0x80 == 0x80,
        }
    }

    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        let mut v = self.qual & 0x7f;
        if self.is_negative {
            v |= 0x80;
        }
        v
    }
}

/// The section ready qualifier (SRQ).
///
/// See companion standard 101, subclass 7.2.6.30. `qual` is bits 0..=6 and
/// bit 7 says the section is not ready to load.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SectionReadyQualifier {
    /// Implementation-defined qualifier, `0..=127`.
    pub qual: u8,
    /// The section is not ready to load.
    pub is_not_ready: bool,
}

impl SectionReadyQualifier {
    /// Decode the wire octet.
    pub const fn parse(b: u8) -> Self {
        SectionReadyQualifier {
            qual: b & 0x7f,
            is_not_ready: b & 0x80 == 0x80,
        }
    }

    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        let mut v = self.qual & 0x7f;
        if self.is_not_ready {
            v |= 0x80;
        }
        v
    }
}

/// The select and call qualifier (SCQ).
///
/// See companion standard 101, subclass 7.2.6.31. The action is bits 0..=3 and
/// the error code bits 4..=7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SelectAndCallQualifier {
    /// What the master is asking for.
    pub action: ScqAction,
    /// Why the request could not be met, or [`FileError::NONE`].
    pub error: FileError,
}

impl SelectAndCallQualifier {
    /// Decode the wire octet.
    pub const fn parse(b: u8) -> Self {
        SelectAndCallQualifier {
            action: ScqAction(b & 0x0f),
            error: FileError(b >> 4),
        }
    }

    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        (self.action.0 & 0x0f) | (self.error.0 << 4)
    }
}

/// The acknowledge file or section qualifier (AFQ).
///
/// See companion standard 101, subclass 7.2.6.33. The action is bits 0..=3 and
/// the error code bits 4..=7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AckFileOrSectionQualifier {
    /// What is being acknowledged.
    pub action: AfqAction,
    /// Why it is a negative acknowledgement, or [`FileError::NONE`].
    pub error: FileError,
}

impl AckFileOrSectionQualifier {
    /// Decode the wire octet.
    pub const fn parse(b: u8) -> Self {
        AckFileOrSectionQualifier {
            action: AfqAction(b & 0x0f),
            error: FileError(b >> 4),
        }
    }

    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        (self.action.0 & 0x0f) | (self.error.0 << 4)
    }
}

/// The status of file (SOF).
///
/// See companion standard 101, subclass 7.2.6.38. `status` is bits 0..=4, LFD
/// is bit 5, FOR is bit 6 and FA is bit 7.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StatusOfFile {
    /// Implementation-defined status, `0..=31`.
    pub status: u8,
    /// LFD: this is the last file of the directory.
    pub is_last_file_of_directory: bool,
    /// FOR: this entry names a subdirectory rather than a file.
    pub is_directory: bool,
    /// FA: the transfer of this file is active.
    pub is_transfer_active: bool,
}

impl StatusOfFile {
    /// Decode the wire octet.
    pub const fn parse(b: u8) -> Self {
        StatusOfFile {
            status: b & 0x1f,
            is_last_file_of_directory: b & 0x20 == 0x20,
            is_directory: b & 0x40 == 0x40,
            is_transfer_active: b & 0x80 == 0x80,
        }
    }

    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        let mut v = self.status & 0x1f;
        if self.is_last_file_of_directory {
            v |= 0x20;
        }
        if self.is_directory {
            v |= 0x40;
        }
        if self.is_transfer_active {
            v |= 0x80;
        }
        v
    }
}

/// The largest value the 3 octet length of file (LOF) element can carry.
pub const LENGTH_OF_FILE_MAX: u32 = (1 << 24) - 1;

/// The checksum (CHS) of a section: the arithmetic sum of all its segment
/// octets, modulo 256.
///
/// See companion standard 101, subclass 7.2.6.35.
pub fn file_checksum(b: &[u8]) -> u8 {
    b.iter().fold(0u8, |sum, v| sum.wrapping_add(*v))
}

impl Params {
    /// The largest segment payload that fits one `F_SG_NA_1` ASDU.
    ///
    /// Bounded by the ASDU size and, because LOS is a single octet, by 255.
    pub fn max_segment_size(&self) -> usize {
        // identifier + IOA + NOF(2) + NOS(1) + LOS(1)
        crate::asdu::ASDU_SIZE_MAX
            .saturating_sub(self.identifier_size() + self.info_obj_addr_size as usize + 4)
            .min(255)
    }
}

/// Validate the cause of transmission of a file transfer ASDU.
fn check_file_cause(params: &Params, coa: CauseOfTransmission, allowed: &[Cause]) -> Result<()> {
    if !allowed.contains(&coa.cause) {
        return Err(Error::CmdCause);
    }
    params.valid()
}

/// Start a file transfer ASDU carrying a single information object.
fn start_file(
    params: Params,
    type_id: TypeId,
    coa: CauseOfTransmission,
    ca: CommonAddr,
) -> Asdu {
    Asdu::new(
        params,
        Identifier::new(type_id, VariableStruct::single(), coa, ca),
    )
}

/// The information object of `F_FR_NA_1`: file ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileReadyInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of the file.
    pub nof: NameOfFile,
    /// Total length of the file in octets.
    pub length_of_file: u32,
    /// File ready qualifier.
    pub frq: FileReadyQualifier,
}

/// The information object of `F_SR_NA_1`: section ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SectionReadyInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of the file.
    pub nof: NameOfFile,
    /// Name (number) of the section.
    pub nos: u8,
    /// Length of this section in octets.
    pub length_of_section: u32,
    /// Section ready qualifier.
    pub srq: SectionReadyQualifier,
}

/// The information object of `F_SC_NA_1`: call directory, select file, call
/// file, call section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CallOrSelectFileInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of the file.
    pub nof: NameOfFile,
    /// Name (number) of the section.
    pub nos: u8,
    /// Select and call qualifier.
    pub scq: SelectAndCallQualifier,
}

/// The information object of `F_LS_NA_1`: last section, last segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct LastSectionOrSegmentInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of the file.
    pub nof: NameOfFile,
    /// Name (number) of the section.
    pub nos: u8,
    /// Last section or segment qualifier.
    pub lsq: LastSectionQualifier,
    /// Checksum of the section; 0 when `lsq` marks the end of a file rather
    /// than the end of a section.
    pub chs: u8,
}

/// The information object of `F_AF_NA_1`: acknowledge file or section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AckFileOrSectionInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of the file.
    pub nof: NameOfFile,
    /// Name (number) of the section.
    pub nos: u8,
    /// Acknowledge qualifier.
    pub afq: AckFileOrSectionQualifier,
}

/// The information object of `F_SG_NA_1`: a segment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SegmentInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of the file.
    pub nof: NameOfFile,
    /// Name (number) of the section.
    pub nos: u8,
    /// The segment payload. Its length must not exceed
    /// [`Params::max_segment_size`].
    pub segment: Vec<u8>,
}

/// One directory entry of `F_DR_TA_1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DirectoryInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of the file.
    pub nof: NameOfFile,
    /// Total length of the file in octets.
    pub length_of_file: u32,
    /// Status of the file.
    pub sof: StatusOfFile,
    /// Creation time of the file.
    pub time: Option<DateTime<Utc>>,
    /// IV (invalid) and SB (substituted) of the time tag.
    ///
    /// `time` holds the reading even when it is invalid, so check
    /// [`TimeTagFlags::is_valid`] before taking it as the time of the
    /// event. [`TimeTagFlags::GOOD`] for the untagged types.
    pub time_flags: TimeTagFlags,
}

impl Asdu {
    // -- F_FR_NA_1 file ready ---------------------------------------------

    /// Build `F_FR_NA_1`: file ready.
    ///
    /// See companion standard 101, subclass 7.3.6.1. Permitted cause in the
    /// monitor direction: `FileTransfer`.
    pub fn file_ready(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: FileReadyInfo,
    ) -> Result<Asdu> {
        check_file_cause(&params, coa, &[Cause::FILE_TRANSFER])?;
        if info.length_of_file > LENGTH_OF_FILE_MAX {
            return Err(Error::LengthOutOfRange);
        }
        let mut u = start_file(params, TypeId::F_FR_NA_1, coa, ca);
        u.encoder()
            .info_obj_addr(info.ioa)?
            .u16(info.nof.0)
            .length_of_file(info.length_of_file)
            .byte(info.frq.value());
        Ok(u)
    }

    /// Decode `F_FR_NA_1`.
    pub fn get_file_ready(&self) -> Result<FileReadyInfo> {
        let mut r = self.reader();
        Ok(FileReadyInfo {
            ioa: r.info_obj_addr()?,
            nof: NameOfFile(r.u16()?),
            length_of_file: r.length_of_file()?,
            frq: FileReadyQualifier::parse(r.byte()?),
        })
    }

    // -- F_SR_NA_1 section ready ------------------------------------------

    /// Build `F_SR_NA_1`: section ready.
    ///
    /// See companion standard 101, subclass 7.3.6.2. Permitted cause in the
    /// monitor direction: `FileTransfer`.
    pub fn section_ready(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: SectionReadyInfo,
    ) -> Result<Asdu> {
        check_file_cause(&params, coa, &[Cause::FILE_TRANSFER])?;
        if info.length_of_section > LENGTH_OF_FILE_MAX {
            return Err(Error::LengthOutOfRange);
        }
        let mut u = start_file(params, TypeId::F_SR_NA_1, coa, ca);
        u.encoder()
            .info_obj_addr(info.ioa)?
            .u16(info.nof.0)
            .byte(info.nos)
            .length_of_file(info.length_of_section)
            .byte(info.srq.value());
        Ok(u)
    }

    /// Decode `F_SR_NA_1`.
    pub fn get_section_ready(&self) -> Result<SectionReadyInfo> {
        let mut r = self.reader();
        Ok(SectionReadyInfo {
            ioa: r.info_obj_addr()?,
            nof: NameOfFile(r.u16()?),
            nos: r.byte()?,
            length_of_section: r.length_of_file()?,
            srq: SectionReadyQualifier::parse(r.byte()?),
        })
    }

    // -- F_SC_NA_1 call or select ------------------------------------------

    /// Build `F_SC_NA_1`: call directory, select file, call file, call section.
    ///
    /// See companion standard 101, subclass 7.3.6.3. Permitted causes in the
    /// control direction: `Request` (to call the directory) and `FileTransfer`.
    pub fn call_or_select_file(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: CallOrSelectFileInfo,
    ) -> Result<Asdu> {
        check_file_cause(&params, coa, &[Cause::REQUEST, Cause::FILE_TRANSFER])?;
        let mut u = start_file(params, TypeId::F_SC_NA_1, coa, ca);
        u.encoder()
            .info_obj_addr(info.ioa)?
            .u16(info.nof.0)
            .byte(info.nos)
            .byte(info.scq.value());
        Ok(u)
    }

    /// Decode `F_SC_NA_1`.
    pub fn get_call_or_select_file(&self) -> Result<CallOrSelectFileInfo> {
        let mut r = self.reader();
        Ok(CallOrSelectFileInfo {
            ioa: r.info_obj_addr()?,
            nof: NameOfFile(r.u16()?),
            nos: r.byte()?,
            scq: SelectAndCallQualifier::parse(r.byte()?),
        })
    }

    // -- F_LS_NA_1 last section or segment ---------------------------------

    /// Build `F_LS_NA_1`: last section, last segment.
    ///
    /// See companion standard 101, subclass 7.3.6.4. Permitted cause in the
    /// monitor direction: `FileTransfer`.
    pub fn last_section_or_segment(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: LastSectionOrSegmentInfo,
    ) -> Result<Asdu> {
        check_file_cause(&params, coa, &[Cause::FILE_TRANSFER])?;
        let mut u = start_file(params, TypeId::F_LS_NA_1, coa, ca);
        u.encoder()
            .info_obj_addr(info.ioa)?
            .u16(info.nof.0)
            .byte(info.nos)
            .byte(info.lsq.0)
            .byte(info.chs);
        Ok(u)
    }

    /// Decode `F_LS_NA_1`.
    pub fn get_last_section_or_segment(&self) -> Result<LastSectionOrSegmentInfo> {
        let mut r = self.reader();
        Ok(LastSectionOrSegmentInfo {
            ioa: r.info_obj_addr()?,
            nof: NameOfFile(r.u16()?),
            nos: r.byte()?,
            lsq: LastSectionQualifier(r.byte()?),
            chs: r.byte()?,
        })
    }

    // -- F_AF_NA_1 acknowledge ---------------------------------------------

    /// Build `F_AF_NA_1`: acknowledge file, acknowledge section.
    ///
    /// See companion standard 101, subclass 7.3.6.5. Permitted cause in the
    /// control direction: `FileTransfer`.
    pub fn ack_file_or_section(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: AckFileOrSectionInfo,
    ) -> Result<Asdu> {
        check_file_cause(&params, coa, &[Cause::FILE_TRANSFER])?;
        let mut u = start_file(params, TypeId::F_AF_NA_1, coa, ca);
        u.encoder()
            .info_obj_addr(info.ioa)?
            .u16(info.nof.0)
            .byte(info.nos)
            .byte(info.afq.value());
        Ok(u)
    }

    /// Decode `F_AF_NA_1`.
    pub fn get_ack_file_or_section(&self) -> Result<AckFileOrSectionInfo> {
        let mut r = self.reader();
        Ok(AckFileOrSectionInfo {
            ioa: r.info_obj_addr()?,
            nof: NameOfFile(r.u16()?),
            nos: r.byte()?,
            afq: AckFileOrSectionQualifier::parse(r.byte()?),
        })
    }

    // -- F_SG_NA_1 segment -------------------------------------------------

    /// Build `F_SG_NA_1`: a segment.
    ///
    /// See companion standard 101, subclass 7.3.6.6. Permitted cause in the
    /// monitor direction: `FileTransfer`. The payload must not exceed
    /// [`Params::max_segment_size`], which is what the single LOS octet and the
    /// ASDU size between them allow.
    pub fn file_segment(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: &SegmentInfo,
    ) -> Result<Asdu> {
        check_file_cause(&params, coa, &[Cause::FILE_TRANSFER])?;
        if info.segment.len() > params.max_segment_size() {
            return Err(Error::LengthOutOfRange);
        }
        let mut u = start_file(params, TypeId::F_SG_NA_1, coa, ca);
        u.encoder()
            .info_obj_addr(info.ioa)?
            .u16(info.nof.0)
            .byte(info.nos)
            .byte(info.segment.len() as u8)
            .bytes(&info.segment);
        Ok(u)
    }

    /// Decode `F_SG_NA_1`.
    pub fn get_file_segment(&self) -> Result<SegmentInfo> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        let nof = NameOfFile(r.u16()?);
        let nos = r.byte()?;
        let los = r.byte()? as usize;
        Ok(SegmentInfo {
            ioa,
            nof,
            nos,
            segment: r.take_bytes(los)?.to_vec(),
        })
    }

    // -- F_DR_TA_1 directory -----------------------------------------------

    /// Build `F_DR_TA_1`: a directory, one information object per entry.
    ///
    /// See companion standard 101, subclass 7.3.6.7. Permitted causes in the
    /// monitor direction: `Spontaneous` and `Request`.
    pub fn file_directory(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[DirectoryInfo],
    ) -> Result<Asdu> {
        check_file_cause(&params, coa, &[Cause::SPONTANEOUS, Cause::REQUEST])?;
        check_valid(&params, TypeId::F_DR_TA_1, false, infos.len())?;

        let mut u = Asdu::new(
            params,
            Identifier::new(
                TypeId::F_DR_TA_1,
                VariableStruct {
                    number: 0,
                    is_sequence: false,
                },
                coa,
                ca,
            ),
        );
        u.set_variable_number(infos.len())?;
        for v in infos {
            if v.length_of_file > LENGTH_OF_FILE_MAX {
                return Err(Error::LengthOutOfRange);
            }
            let mut e = u.encoder();
            e.info_obj_addr(v.ioa)?
                .u16(v.nof.0)
                .length_of_file(v.length_of_file)
                .byte(v.sof.value())
                .cp56time2a_tag(v.time, v.time_flags);
        }
        Ok(u)
    }

    /// Decode `F_DR_TA_1`.
    pub fn get_file_directory(&self) -> Result<Vec<DirectoryInfo>> {
        let n = self.variable().number as usize;
        let mut r = self.reader();
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(DirectoryInfo {
                ioa: r.info_obj_addr()?,
                nof: NameOfFile(r.u16()?),
                length_of_file: r.length_of_file()?,
                sof: StatusOfFile::parse(r.byte()?),
                time: r.cp56time2a_tag()?,
                time_flags: r.time_flags(),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::params::{PARAMS_NARROW, PARAMS_STANDARD_101, PARAMS_WIDE};
    use chrono::TimeZone as _;

    fn coa(c: Cause) -> CauseOfTransmission {
        CauseOfTransmission::new(c)
    }

    fn transfer() -> CauseOfTransmission {
        coa(Cause::FILE_TRANSFER)
    }

    /// Every family must survive marshal → unmarshal at every address width.
    fn round_trip(a: &Asdu) -> Asdu {
        let wire = a.marshal_binary().expect("marshal");
        Asdu::unmarshal_binary(a.params, &wire).expect("unmarshal")
    }

    #[test]
    fn qualifiers_round_trip_through_their_octets() {
        for b in 0u8..=255 {
            assert_eq!(FileReadyQualifier::parse(b).value(), b);
            assert_eq!(SectionReadyQualifier::parse(b).value(), b);
            assert_eq!(SelectAndCallQualifier::parse(b).value(), b);
            assert_eq!(AckFileOrSectionQualifier::parse(b).value(), b);
            assert_eq!(StatusOfFile::parse(b).value(), b);
        }
    }

    #[test]
    fn qualifiers_split_the_documented_bit_fields() {
        // SCQ and AFQ are action in the low nibble, error in the high one.
        let scq = SelectAndCallQualifier::parse(0x25);
        assert_eq!(scq.action, ScqAction::SELECT_SECTION);
        assert_eq!(scq.error, FileError::CHECKSUM_FAILED);

        let afq = AckFileOrSectionQualifier::parse(0x13);
        assert_eq!(afq.action, AfqAction::POS_ACK_SECTION);
        assert_eq!(afq.error, FileError::MEMORY_UNAVAILABLE);

        // FRQ and SRQ are a 7 bit qualifier plus one flag in bit 7.
        assert_eq!(
            FileReadyQualifier::parse(0x81),
            FileReadyQualifier { qual: 1, is_negative: true }
        );
        assert_eq!(
            SectionReadyQualifier::parse(0x02),
            SectionReadyQualifier { qual: 2, is_not_ready: false }
        );

        // SOF is a 5 bit status plus LFD, FOR and FA.
        assert_eq!(
            StatusOfFile::parse(0xe3),
            StatusOfFile {
                status: 3,
                is_last_file_of_directory: true,
                is_directory: true,
                is_transfer_active: true,
            }
        );
    }

    #[test]
    fn every_family_survives_a_wire_round_trip() {
        for params in [PARAMS_NARROW, PARAMS_STANDARD_101, PARAMS_WIDE] {
            let ioa = 1;

            let fr = FileReadyInfo {
                ioa,
                nof: NameOfFile::DISTURBANCE_DATA,
                length_of_file: 0x01_2345,
                frq: FileReadyQualifier { qual: 1, is_negative: false },
            };
            let a = Asdu::file_ready(params, transfer(), 1, fr).unwrap();
            assert_eq!(round_trip(&a).get_file_ready().unwrap(), fr);

            let sr = SectionReadyInfo {
                ioa,
                nof: NameOfFile::DISTURBANCE_DATA,
                nos: 2,
                length_of_section: 4096,
                srq: SectionReadyQualifier { qual: 0, is_not_ready: false },
            };
            let a = Asdu::section_ready(params, transfer(), 1, sr).unwrap();
            assert_eq!(round_trip(&a).get_section_ready().unwrap(), sr);

            let cs = CallOrSelectFileInfo {
                ioa,
                nof: NameOfFile::DISTURBANCE_DATA,
                nos: 2,
                scq: SelectAndCallQualifier {
                    action: ScqAction::REQUEST_SECTION,
                    error: FileError::NONE,
                },
            };
            let a = Asdu::call_or_select_file(params, transfer(), 1, cs).unwrap();
            assert_eq!(round_trip(&a).get_call_or_select_file().unwrap(), cs);

            let ls = LastSectionOrSegmentInfo {
                ioa,
                nof: NameOfFile::DISTURBANCE_DATA,
                nos: 2,
                lsq: LastSectionQualifier::SECTION_WITHOUT_DEACTIVATE,
                chs: 0x5a,
            };
            let a = Asdu::last_section_or_segment(params, transfer(), 1, ls).unwrap();
            assert_eq!(round_trip(&a).get_last_section_or_segment().unwrap(), ls);

            let af = AckFileOrSectionInfo {
                ioa,
                nof: NameOfFile::DISTURBANCE_DATA,
                nos: 2,
                afq: AckFileOrSectionQualifier {
                    action: AfqAction::POS_ACK_SECTION,
                    error: FileError::NONE,
                },
            };
            let a = Asdu::ack_file_or_section(params, transfer(), 1, af).unwrap();
            assert_eq!(round_trip(&a).get_ack_file_or_section().unwrap(), af);

            let sg = SegmentInfo {
                ioa,
                nof: NameOfFile::DISTURBANCE_DATA,
                nos: 2,
                segment: (0u8..64).collect(),
            };
            let a = Asdu::file_segment(params, transfer(), 1, &sg).unwrap();
            assert_eq!(round_trip(&a).get_file_segment().unwrap(), sg);

            let t = Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).unwrap();
            let dir = [
                DirectoryInfo {
                    ioa,
                    nof: NameOfFile::DISTURBANCE_DATA,
                    length_of_file: 1024,
                    sof: StatusOfFile { status: 1, ..Default::default() },
                    time: Some(t),
                    time_flags: TimeTagFlags::GOOD,
                },
                DirectoryInfo {
                    ioa: 2,
                    nof: NameOfFile(0x1234),
                    length_of_file: LENGTH_OF_FILE_MAX,
                    sof: StatusOfFile {
                        status: 2,
                        is_last_file_of_directory: true,
                        is_directory: false,
                        is_transfer_active: true,
                    },
                    time: Some(t),
                    time_flags: TimeTagFlags::GOOD,
                },
            ];
            let a = Asdu::file_directory(params, coa(Cause::REQUEST), 1, &dir).unwrap();
            assert_eq!(round_trip(&a).get_file_directory().unwrap(), dir);
        }
    }

    #[test]
    fn file_ready_writes_the_documented_octet_layout() {
        let a = Asdu::file_ready(
            PARAMS_WIDE,
            transfer(),
            1,
            FileReadyInfo {
                ioa: 0x0201,
                nof: NameOfFile::DISTURBANCE_DATA,
                // LOF is 3 octets, little endian.
                length_of_file: 0x00_03_02_01,
                frq: FileReadyQualifier { qual: 0x7f, is_negative: true },
            },
        )
        .unwrap();
        let wire = a.marshal_binary().unwrap();
        assert_eq!(wire[0], 120, "F_FR_NA_1");
        assert_eq!(wire[1], 1, "one object, SQ = 0");
        assert_eq!(wire[2], Cause::FILE_TRANSFER.0);
        assert_eq!(&wire[6..9], &[0x01, 0x02, 0x00], "IOA, 3 octets");
        assert_eq!(&wire[9..11], &[0x02, 0x00], "NOF, 2 octets");
        assert_eq!(&wire[11..14], &[0x01, 0x02, 0x03], "LOF, 3 octets");
        assert_eq!(wire[14], 0xff, "FRQ with the P/N bit set");
    }

    #[test]
    fn a_segment_is_bounded_by_the_length_of_segment_octet() {
        for params in [PARAMS_NARROW, PARAMS_STANDARD_101, PARAMS_WIDE] {
            let max = params.max_segment_size();
            // LOS is one octet, so no configuration can carry more than 255.
            assert!(max <= 255, "{max}");

            let at_max = SegmentInfo {
                ioa: 1,
                nof: NameOfFile::TRANSPARENT,
                nos: 1,
                segment: vec![0xab; max],
            };
            let a = Asdu::file_segment(params, transfer(), 1, &at_max)
                .unwrap_or_else(|e| panic!("the largest segment was refused: {e}"));
            assert_eq!(round_trip(&a).get_file_segment().unwrap(), at_max);

            let over = SegmentInfo {
                segment: vec![0xab; max + 1],
                ..at_max
            };
            assert_eq!(
                Asdu::file_segment(params, transfer(), 1, &over),
                Err(Error::LengthOutOfRange)
            );
        }
    }

    #[test]
    fn a_segment_is_sized_from_its_own_length_octet_not_the_qualifier() {
        // F_SG_NA_1 is the one type in the compatible range whose object
        // length the type identification does not fix, so unmarshal has to
        // read LOS to know where the object ends.
        let sg = SegmentInfo {
            ioa: 100,
            nof: NameOfFile::DISTURBANCE_DATA,
            nos: 1,
            segment: vec![1, 2, 3, 4, 5],
        };
        let wire = Asdu::file_segment(PARAMS_WIDE, transfer(), 1, &sg)
            .unwrap()
            .marshal_binary()
            .unwrap();
        let back = Asdu::unmarshal_binary(PARAMS_WIDE, &wire).unwrap();
        assert_eq!(back.get_file_segment().unwrap(), sg);

        // A truncated segment is not silently short-read.
        assert_eq!(
            Asdu::unmarshal_binary(PARAMS_WIDE, &wire[..wire.len() - 1]),
            Err(Error::UnexpectedEof)
        );
    }

    #[test]
    fn the_length_of_file_element_is_three_octets() {
        let a = Asdu::file_ready(
            PARAMS_WIDE,
            transfer(),
            1,
            FileReadyInfo {
                ioa: 1,
                length_of_file: LENGTH_OF_FILE_MAX,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            round_trip(&a).get_file_ready().unwrap().length_of_file,
            LENGTH_OF_FILE_MAX
        );

        // One more than the element can carry is refused rather than truncated.
        assert_eq!(
            Asdu::file_ready(
                PARAMS_WIDE,
                transfer(),
                1,
                FileReadyInfo {
                    ioa: 1,
                    length_of_file: LENGTH_OF_FILE_MAX + 1,
                    ..Default::default()
                },
            ),
            Err(Error::LengthOutOfRange)
        );
    }

    #[test]
    fn the_causes_the_standard_permits_are_enforced() {
        let bad = coa(Cause::PERIODIC);
        assert_eq!(
            Asdu::file_ready(PARAMS_WIDE, bad, 1, FileReadyInfo { ioa: 1, ..Default::default() }),
            Err(Error::CmdCause)
        );
        assert_eq!(
            Asdu::section_ready(PARAMS_WIDE, bad, 1, SectionReadyInfo { ioa: 1, ..Default::default() }),
            Err(Error::CmdCause)
        );
        assert_eq!(
            Asdu::ack_file_or_section(
                PARAMS_WIDE,
                bad,
                1,
                AckFileOrSectionInfo { ioa: 1, ..Default::default() }
            ),
            Err(Error::CmdCause)
        );
        assert_eq!(
            Asdu::file_directory(
                PARAMS_WIDE,
                coa(Cause::FILE_TRANSFER),
                1,
                &[DirectoryInfo { ioa: 1, ..Default::default() }]
            ),
            Err(Error::CmdCause),
            "a directory is spontaneous or requested, never a file transfer"
        );

        // A call may be a request (for the directory) or a file transfer.
        for c in [Cause::REQUEST, Cause::FILE_TRANSFER] {
            assert!(
                Asdu::call_or_select_file(
                    PARAMS_WIDE,
                    coa(c),
                    1,
                    CallOrSelectFileInfo { ioa: 1, ..Default::default() }
                )
                .is_ok(),
                "{c:?}"
            );
        }
    }

    #[test]
    fn the_section_checksum_is_the_arithmetic_sum_modulo_256() {
        assert_eq!(file_checksum(&[]), 0);
        assert_eq!(file_checksum(&[1, 2, 3]), 6);
        // It wraps rather than saturating or overflowing.
        assert_eq!(file_checksum(&[0xff, 0x02]), 1);
        assert_eq!(file_checksum(&vec![0xff; 256]), 0);
    }

    #[test]
    fn a_truncated_information_object_is_reported_not_guessed() {
        let a = Asdu::file_ready(
            PARAMS_WIDE,
            transfer(),
            1,
            FileReadyInfo { ioa: 1, ..Default::default() },
        )
        .unwrap();
        let mut short = a.clone();
        short.info_obj.truncate(4);
        assert_eq!(short.get_file_ready(), Err(Error::UnexpectedEof));
    }
}
