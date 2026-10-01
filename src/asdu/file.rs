// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! IEC 60870-5-101 §7.3.6 file-transfer information objects.
use super::*;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
/// File identifier; 0 is default, 1 transparent, 2 disturbance data,
/// 3 sequences of events, and 4 sequences of analogues.
pub type NameOfFile = u16;
/// Largest length representable by the three-octet LOF/LOS field.
pub const LENGTH_OF_FILE_MAX: u32 = 0xffffff;
/// Arithmetic section checksum, modulo 256.
pub fn file_checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |s, b| s.wrapping_add(*b))
}
impl Params {
    /// Maximum segment payload fitting an ASDU (236 octets for IEC 104).
    pub fn max_segment_size(self) -> usize {
        ASDU_SIZE_MAX
            .saturating_sub(self.identifier_size() + self.info_obj_addr_size as usize + 4)
            .min(255)
    }
}
/// Information object for `F_FR_NA_1`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileReadyInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of file.
    pub nof: NameOfFile,
    /// Total file length in octets.
    pub length_of_file: u32,
    /// File-ready qualifier: bit 7 means negative.
    pub frq: u8,
}
/// Information object for `F_SR_NA_1`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SectionReadyInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of file.
    pub nof: NameOfFile,
    /// Section number (1..=255).
    pub nos: u8,
    /// Section length in octets.
    pub length_of_section: u32,
    /// Section-ready qualifier: bit 7 means not ready.
    pub srq: u8,
}
/// Information object for `F_SC_NA_1`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CallOrSelectFileInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of file.
    pub nof: NameOfFile,
    /// Section number (1..=255).
    pub nos: u8,
    /// Select/call qualifier: low nibble action, high nibble error.
    pub scq: u8,
}
/// Information object for `F_LS_NA_1`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LastSectionOrSegmentInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of file.
    pub nof: NameOfFile,
    /// Section number (1..=255).
    pub nos: u8,
    /// Last-section qualifier: 1/2 end file, 3/4 end section.
    pub lsq: u8,
    /// Arithmetic section checksum.
    pub chs: u8,
}
/// Information object for `F_AF_NA_1`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AckFileOrSectionInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of file.
    pub nof: NameOfFile,
    /// Section number (1..=255).
    pub nos: u8,
    /// Acknowledgement qualifier: low nibble action, high nibble error.
    pub afq: u8,
}
/// Information object for `F_SG_NA_1`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SegmentInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of file.
    pub nof: NameOfFile,
    /// Section number (1..=255).
    pub nos: u8,
    /// Owned segment payload.
    pub segment: Vec<u8>,
}
/// Information object for `F_DR_TA_1`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirectoryInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of file.
    pub nof: NameOfFile,
    /// Total file length in octets.
    pub length_of_file: u32,
    /// Status: bit 5 last entry, bit 6 directory, bit 7 active.
    pub sof: u8,
    /// Creation or acquisition time.
    pub time: Option<DateTime<Utc>>,
}
fn start(params: Params, tid: TypeId, coa: CauseOfTransmission, ca: CommonAddr) -> Result<Asdu> {
    params.valid()?;
    let allowed = match tid {
        TypeId::F_SC_NA_1 => coa.cause == Cause::REQUEST || coa.cause == Cause::FILE_TRANSFER,
        TypeId::F_DR_TA_1 => coa.cause == Cause::REQUEST || coa.cause == Cause::SPONTANEOUS,
        _ => coa.cause == Cause::FILE_TRANSFER,
    };
    if !allowed {
        return Err(Error::CmdCause);
    }
    Ok(Asdu::new(
        params,
        Identifier::new(tid, VariableStruct::single(), coa, ca),
    ))
}
fn length(e: &mut Encoder<'_>, n: u32) -> Result<()> {
    if n > LENGTH_OF_FILE_MAX {
        return Err(Error::LengthOutOfRange);
    }
    e.bytes(&n.to_le_bytes()[..3]);
    Ok(())
}
fn read_length(r: &mut InfoObjReader<'_>) -> Result<u32> {
    Ok(r.byte()? as u32 | (r.byte()? as u32) << 8 | (r.byte()? as u32) << 16)
}
impl Asdu {
    /// Build `F_FR_NA_1`.
    pub fn file_ready(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: FileReadyInfo,
    ) -> Result<Self> {
        let mut a = start(params, TypeId::F_FR_NA_1, coa, ca)?;
        let mut e = a.encoder();
        e.info_obj_addr(info.ioa)?.u16(info.nof);
        length(&mut e, info.length_of_file)?;
        e.byte(info.frq);
        a.marshal_binary()?;
        Ok(a)
    }
    /// Decode `F_FR_NA_1` without consuming the ASDU.
    pub fn get_file_ready(&self) -> Result<FileReadyInfo> {
        if self.type_id() != TypeId::F_FR_NA_1 {
            return Err(Error::TypeIdNotMatch);
        }
        self.validate_info_obj()?;
        let mut r = self.reader();
        let mut info = FileReadyInfo {
            ioa: r.info_obj_addr()?,
            nof: r.u16()?,
            ..Default::default()
        };
        info.length_of_file = read_length(&mut r)?;
        info.frq = r.byte()?;
        Ok(info)
    }
    /// Build `F_SR_NA_1`.
    pub fn section_ready(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: SectionReadyInfo,
    ) -> Result<Self> {
        let mut a = start(params, TypeId::F_SR_NA_1, coa, ca)?;
        let mut e = a.encoder();
        e.info_obj_addr(info.ioa)?.u16(info.nof);
        e.byte(info.nos);
        length(&mut e, info.length_of_section)?;
        e.byte(info.srq);
        a.marshal_binary()?;
        Ok(a)
    }
    /// Decode `F_SR_NA_1` without consuming the ASDU.
    pub fn get_section_ready(&self) -> Result<SectionReadyInfo> {
        if self.type_id() != TypeId::F_SR_NA_1 {
            return Err(Error::TypeIdNotMatch);
        }
        self.validate_info_obj()?;
        let mut r = self.reader();
        let mut info = SectionReadyInfo {
            ioa: r.info_obj_addr()?,
            nof: r.u16()?,
            ..Default::default()
        };
        info.nos = r.byte()?;
        info.length_of_section = read_length(&mut r)?;
        info.srq = r.byte()?;
        Ok(info)
    }
    /// Build `F_SC_NA_1`.
    pub fn call_or_select_file(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: CallOrSelectFileInfo,
    ) -> Result<Self> {
        let mut a = start(params, TypeId::F_SC_NA_1, coa, ca)?;
        let mut e = a.encoder();
        e.info_obj_addr(info.ioa)?.u16(info.nof);
        e.byte(info.nos);
        e.byte(info.scq);
        a.marshal_binary()?;
        Ok(a)
    }
    /// Decode `F_SC_NA_1` without consuming the ASDU.
    pub fn get_call_or_select_file(&self) -> Result<CallOrSelectFileInfo> {
        if self.type_id() != TypeId::F_SC_NA_1 {
            return Err(Error::TypeIdNotMatch);
        }
        self.validate_info_obj()?;
        let mut r = self.reader();
        let mut info = CallOrSelectFileInfo {
            ioa: r.info_obj_addr()?,
            nof: r.u16()?,
            ..Default::default()
        };
        info.nos = r.byte()?;
        info.scq = r.byte()?;
        Ok(info)
    }
    /// Build `F_LS_NA_1`.
    pub fn last_section_or_segment(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: LastSectionOrSegmentInfo,
    ) -> Result<Self> {
        let mut a = start(params, TypeId::F_LS_NA_1, coa, ca)?;
        let mut e = a.encoder();
        e.info_obj_addr(info.ioa)?.u16(info.nof);
        e.byte(info.nos);
        e.byte(info.lsq);
        e.byte(info.chs);
        a.marshal_binary()?;
        Ok(a)
    }
    /// Decode `F_LS_NA_1` without consuming the ASDU.
    pub fn get_last_section_or_segment(&self) -> Result<LastSectionOrSegmentInfo> {
        if self.type_id() != TypeId::F_LS_NA_1 {
            return Err(Error::TypeIdNotMatch);
        }
        self.validate_info_obj()?;
        let mut r = self.reader();
        let mut info = LastSectionOrSegmentInfo {
            ioa: r.info_obj_addr()?,
            nof: r.u16()?,
            ..Default::default()
        };
        info.nos = r.byte()?;
        info.lsq = r.byte()?;
        info.chs = r.byte()?;
        Ok(info)
    }
    /// Build `F_AF_NA_1`.
    pub fn ack_file_or_section(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: AckFileOrSectionInfo,
    ) -> Result<Self> {
        let mut a = start(params, TypeId::F_AF_NA_1, coa, ca)?;
        let mut e = a.encoder();
        e.info_obj_addr(info.ioa)?.u16(info.nof);
        e.byte(info.nos);
        e.byte(info.afq);
        a.marshal_binary()?;
        Ok(a)
    }
    /// Decode `F_AF_NA_1` without consuming the ASDU.
    pub fn get_ack_file_or_section(&self) -> Result<AckFileOrSectionInfo> {
        if self.type_id() != TypeId::F_AF_NA_1 {
            return Err(Error::TypeIdNotMatch);
        }
        self.validate_info_obj()?;
        let mut r = self.reader();
        let mut info = AckFileOrSectionInfo {
            ioa: r.info_obj_addr()?,
            nof: r.u16()?,
            ..Default::default()
        };
        info.nos = r.byte()?;
        info.afq = r.byte()?;
        Ok(info)
    }
    /// Build `F_SG_NA_1`.
    pub fn file_segment(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: SegmentInfo,
    ) -> Result<Self> {
        let mut a = start(params, TypeId::F_SG_NA_1, coa, ca)?;
        if info.segment.len() > params.max_segment_size() {
            return Err(Error::LengthOutOfRange);
        }
        let mut e = a.encoder();
        e.info_obj_addr(info.ioa)?.u16(info.nof);
        e.byte(info.nos);
        e.byte(info.segment.len() as u8).bytes(&info.segment);
        a.marshal_binary()?;
        Ok(a)
    }
    /// Decode `F_SG_NA_1` without consuming the ASDU.
    pub fn get_file_segment(&self) -> Result<SegmentInfo> {
        if self.type_id() != TypeId::F_SG_NA_1 {
            return Err(Error::TypeIdNotMatch);
        }
        self.validate_info_obj()?;
        let mut r = self.reader();
        let mut info = SegmentInfo {
            ioa: r.info_obj_addr()?,
            nof: r.u16()?,
            ..Default::default()
        };
        info.nos = r.byte()?;
        let n = r.byte()?;
        for _ in 0..n {
            info.segment.push(r.byte()?);
        }
        Ok(info)
    }
    /// Build `F_DR_TA_1`.
    pub fn file_directory(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: &[DirectoryInfo],
    ) -> Result<Self> {
        let mut a = start(params, TypeId::F_DR_TA_1, coa, ca)?;
        if info.is_empty() {
            return Err(Error::NotAnyObjInfo);
        }
        a.set_variable_number(info.len())?;
        for info in info {
            let mut e = a.encoder();
            e.info_obj_addr(info.ioa)?.u16(info.nof);
            length(&mut e, info.length_of_file)?;
            e.byte(info.sof);
            e.cp56time2a(info.time);
        }
        a.marshal_binary()?;
        Ok(a)
    }
    /// Decode `F_DR_TA_1` without consuming the ASDU.
    pub fn get_file_directory(&self) -> Result<Vec<DirectoryInfo>> {
        if self.type_id() != TypeId::F_DR_TA_1 {
            return Err(Error::TypeIdNotMatch);
        }
        self.validate_info_obj()?;
        let mut r = self.reader();
        let mut result = Vec::new();
        for _ in 0..self.variable().number {
            let mut info = DirectoryInfo {
                ioa: r.info_obj_addr()?,
                nof: r.u16()?,
                ..Default::default()
            };
            info.length_of_file = read_length(&mut r)?;
            info.sof = r.byte()?;
            info.time = r.cp56time2a()?;
            result.push(info);
        }
        Ok(result)
    }
}
