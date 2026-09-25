// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The [`Asdu`] container plus the primitives that serialise information
//! elements into it and read them back out.
//!
//! ```text
//!       | data unit identification | information object <1..n> |
//!
//!       | <------------  data unit identification ------------>|
//!       | typeID | variable struct | cause  |  common address  |
//! bytes |    1   |        1        | [1,2]  |      [1,2]       |
//!       | <------------  information object ------------------>|
//!       | object address | element set  |  object time scale   |
//! bytes |     [1,2,3]    |              |                      |
//! ```

use chrono::{DateTime, Utc};

use crate::asdu::identifier::{
    Cause, CauseOfTransmission, CommonAddr, GLOBAL_COMMON_ADDR, INVALID_COMMON_ADDR, Identifier,
    TypeId, VariableStruct,
};
use crate::asdu::info::{
    BinaryCounterReading, InfoObjAddr, Normalize, StatusAndStatusChangeDetection,
};
use crate::asdu::params::Params;
use crate::asdu::time::{
    CP16TIME2A_SIZE, CP24TIME2A_SIZE, CP56TIME2A_SIZE, TimeTagFlags, cp16time2a, cp24time2a,
    cp24time2a_tag, cp56time2a, cp56time2a_tag, parse_cp16time2a, parse_cp24time2a,
    parse_cp24time2a_tag, parse_cp56time2a, parse_cp56time2a_tag,
};
use crate::error::{Error, Result};

/// Maximum size in octets of an ASDU, identifier included.
///
/// The 104 APDU is 255 octets, of which 4 are the APCI control field and 2 the
/// start/length octets.
pub const ASDU_SIZE_MAX: usize = 249;

/// An Application Service Data Unit: one application message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asdu {
    /// The system parameters that fix the field widths of this ASDU.
    pub params: Params,
    /// The data unit identifier.
    pub identifier: Identifier,
    /// The serialised information objects.
    pub info_obj: Vec<u8>,
}

impl Asdu {
    /// A new ASDU with the given parameters and an all-default identifier.
    pub fn new_empty(params: Params) -> Self {
        Asdu {
            params,
            identifier: Identifier {
                type_id: TypeId(0),
                variable: VariableStruct::default(),
                coa: CauseOfTransmission::default(),
                orig_addr: params.orig_address,
                common_addr: 0,
            },
            info_obj: Vec::new(),
        }
    }

    /// A new ASDU with the given parameters and identifier.
    pub fn new(params: Params, identifier: Identifier) -> Self {
        Asdu {
            params,
            identifier,
            info_obj: Vec::new(),
        }
    }

    /// The type identification.
    pub fn type_id(&self) -> TypeId {
        self.identifier.type_id
    }

    /// The variable structure qualifier.
    pub fn variable(&self) -> VariableStruct {
        self.identifier.variable
    }

    /// The cause of transmission.
    pub fn coa(&self) -> CauseOfTransmission {
        self.identifier.coa
    }

    /// Mutable access to the cause of transmission, for setting the P/N bit
    /// before mirroring a request back.
    pub fn coa_mut(&mut self) -> &mut CauseOfTransmission {
        &mut self.identifier.coa
    }

    /// The station (common) address.
    pub fn common_addr(&self) -> CommonAddr {
        self.identifier.common_addr
    }

    /// The originator address.
    pub fn orig_addr(&self) -> u8 {
        self.identifier.orig_addr
    }

    /// Set the number of information objects in the variable structure qualifier.
    ///
    /// See companion standard 101, subclass 7.2.2: the count must fit 7 bits.
    pub fn set_variable_number(&mut self, n: usize) -> Result<()> {
        if n >= 128 {
            return Err(Error::InfoObjIndexFit);
        }
        self.identifier.variable.number = n as u8;
        Ok(())
    }

    /// A copy of this ASDU addressed to `addr` with a different cause.
    pub fn reply(&self, cause: Cause, addr: CommonAddr) -> Asdu {
        let mut r = self.clone();
        r.identifier.common_addr = addr;
        r.identifier.coa.cause = cause;
        r
    }

    /// A mirror of this ASDU with a different cause, keeping every other field.
    ///
    /// This is the standard confirmation pattern: echo the request back with
    /// `ActivationCon`, then `ActivationTerm`, or with one of the
    /// `Unknown*` causes to report a protocol error.
    pub fn reply_mirror(&self, cause: Cause) -> Asdu {
        let mut r = self.clone();
        r.identifier.coa.cause = cause;
        r
    }

    /// A copy of this ASDU with the negative-confirmation (P/N) bit set.
    ///
    /// Chains after [`Asdu::reply_mirror`] to reject a request:
    /// `pack.reply_mirror(Cause::ACTIVATION_CON).negated()`.
    pub fn negated(mut self) -> Asdu {
        self.identifier.coa.is_negative = true;
        self
    }

    /// Serialise to the wire format.
    pub fn marshal_binary(&self) -> Result<Vec<u8>> {
        let id = &self.identifier;
        if id.coa.cause == Cause::UNUSED {
            return Err(Error::CauseZero);
        }
        if !(1..=2).contains(&self.params.cause_size)
            || !(1..=2).contains(&self.params.common_addr_size)
        {
            return Err(Error::Param);
        }
        if id.common_addr == INVALID_COMMON_ADDR {
            return Err(Error::CommonAddrZero);
        }
        if self.params.common_addr_size == 1
            && id.common_addr != GLOBAL_COMMON_ADDR
            && id.common_addr >= 255
        {
            return Err(Error::Param);
        }
        let len = self.params.identifier_size() + self.info_obj.len();
        if len > ASDU_SIZE_MAX {
            return Err(Error::LengthOutOfRange);
        }

        let mut raw = Vec::with_capacity(len);
        raw.push(id.type_id.0);
        raw.push(id.variable.value());
        raw.push(id.coa.value());
        if self.params.cause_size == 2 {
            raw.push(id.orig_addr);
        }
        if self.params.common_addr_size == 1 {
            // The 16-bit broadcast address maps to 255 in 8-bit mode.
            raw.push(if id.common_addr == GLOBAL_COMMON_ADDR {
                255
            } else {
                id.common_addr as u8
            });
        } else {
            raw.extend_from_slice(&id.common_addr.to_le_bytes());
        }
        raw.extend_from_slice(&self.info_obj);
        Ok(raw)
    }

    /// Parse an ASDU from the wire format using the given parameters.
    ///
    /// The information object payload is trimmed to exactly the length implied
    /// by the type identification and the variable structure qualifier; a
    /// payload shorter than that is rejected with [`Error::UnexpectedEof`].
    pub fn unmarshal_binary(params: Params, raw: &[u8]) -> Result<Asdu> {
        if !(1..=2).contains(&params.cause_size) || !(1..=2).contains(&params.common_addr_size) {
            return Err(Error::Param);
        }
        let len_dui = params.identifier_size();
        if len_dui > raw.len() {
            return Err(Error::UnexpectedEof);
        }

        let type_id = TypeId(raw[0]);
        let variable = VariableStruct::parse(raw[1]);
        let coa = CauseOfTransmission::parse(raw[2]);
        let orig_addr = if params.cause_size == 1 { 0 } else { raw[3] };
        let common_addr = if params.common_addr_size == 1 {
            let a = raw[len_dui - 1] as CommonAddr;
            // Map the 8-bit broadcast address to its 16-bit equivalent.
            if a == 255 { GLOBAL_COMMON_ADDR } else { a }
        } else {
            u16::from_le_bytes([raw[len_dui - 2], raw[len_dui - 1]])
        };

        let mut asdu = Asdu {
            params,
            identifier: Identifier {
                type_id,
                variable,
                coa,
                orig_addr,
                common_addr,
            },
            info_obj: raw[len_dui..].to_vec(),
        };
        asdu.fix_info_obj_size()?;
        Ok(asdu)
    }

    /// Check the information object payload against the size implied by the
    /// type identification and the variable structure qualifier.
    ///
    /// A short payload is [`Error::UnexpectedEof`]. A long one is
    /// [`Error::TrailingOctets`], unless
    /// [`Params::allow_trailing_octets`](crate::asdu::Params::allow_trailing_octets)
    /// is set, in which case the surplus is discarded.
    pub fn fix_info_obj_size(&mut self) -> Result<()> {
        // A variable-length object carries its own length, so "expected" is
        // only meaningful once that octet is present.
        let size = match self.variable_info_obj_size() {
            Some(0) => return Err(Error::UnexpectedEof),
            Some(size) => size,
            None => {
                let obj_size = self.identifier.type_id.info_obj_size()?;
                let n = self.identifier.variable.number as usize;
                let addr_size = self.params.info_obj_addr_size as usize;
                let size = if self.identifier.variable.is_sequence {
                    addr_size + n * obj_size
                } else {
                    n * (addr_size + obj_size)
                };
                if size == 0 {
                    return Err(Error::InfoObjIndexFit);
                }
                size
            }
        };

        if size > self.info_obj.len() {
            return Err(Error::UnexpectedEof);
        }
        if size < self.info_obj.len() {
            if !self.params.allow_trailing_octets {
                return Err(Error::TrailingOctets);
            }
            self.info_obj.truncate(size);
        }
        Ok(())
    }

    /// The size of an information object whose length is not fixed by the type
    /// identification, or `None` when this type is not such a case.
    ///
    /// The compatible range defines one: `F_SG_NA_1` (segment), whose length is
    /// carried in its own LOS (length of segment) octet. `Some(0)` means the
    /// type was recognised but the payload is too short to hold that octet.
    fn variable_info_obj_size(&self) -> Option<usize> {
        if self.identifier.type_id != TypeId::F_SG_NA_1 {
            return None;
        }
        // IOA + NOF(2) + NOS(1) + LOS(1) + segment data(LOS)
        let head = self.params.info_obj_addr_size as usize + 4;
        if self.info_obj.len() < head {
            return Some(0);
        }
        Some(head + self.info_obj[self.params.info_obj_addr_size as usize + 3] as usize)
    }

    /// An appender that writes information elements into this ASDU.
    pub fn encoder(&mut self) -> Encoder<'_> {
        Encoder {
            params: self.params,
            buf: &mut self.info_obj,
        }
    }

    /// A cursor that reads information elements back out, without consuming
    /// them: the ASDU is left intact and can be decoded repeatedly.
    pub fn reader(&self) -> InfoObjReader<'_> {
        InfoObjReader {
            params: self.params,
            buf: &self.info_obj,
            pos: 0,
            last_flags: TimeTagFlags::GOOD,
        }
    }

    /// Append raw octets to the information object payload.
    pub fn append_bytes(&mut self, b: &[u8]) -> &mut Self {
        self.info_obj.extend_from_slice(b);
        self
    }
}

/// Appends information elements to an ASDU's payload in wire order.
///
/// All multi-octet integers are little endian, per the standard.
pub struct Encoder<'a> {
    params: Params,
    buf: &'a mut Vec<u8>,
}

impl Encoder<'_> {
    /// Append raw octets.
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(b);
        self
    }

    /// Append one octet.
    pub fn byte(&mut self, b: u8) -> &mut Self {
        self.buf.push(b);
        self
    }

    /// Append a little-endian `u16`.
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Append an information object address in the configured width.
    pub fn info_obj_addr(&mut self, addr: InfoObjAddr) -> Result<&mut Self> {
        match self.params.info_obj_addr_size {
            1 => {
                if addr > 255 {
                    return Err(Error::InfoObjAddrFit);
                }
                self.buf.push(addr as u8);
            }
            2 => {
                if addr > 65535 {
                    return Err(Error::InfoObjAddrFit);
                }
                self.buf.extend_from_slice(&(addr as u16).to_le_bytes());
            }
            3 => {
                if addr > 16_777_215 {
                    return Err(Error::InfoObjAddrFit);
                }
                self.buf
                    .extend_from_slice(&addr.to_le_bytes()[..3]);
            }
            _ => return Err(Error::Param),
        }
        Ok(self)
    }

    /// Append a 3-octet length of file (LOF).
    ///
    /// The element is 3 octets, so only the low 24 bits are written; callers
    /// bound the value against
    /// [`LENGTH_OF_FILE_MAX`](crate::asdu::LENGTH_OF_FILE_MAX) first.
    pub fn length_of_file(&mut self, n: u32) -> &mut Self {
        self.buf.extend_from_slice(&n.to_le_bytes()[..3]);
        self
    }

    /// Append a normalized value (NVA).
    pub fn normalize(&mut self, n: Normalize) -> &mut Self {
        self.buf.extend_from_slice(&n.0.to_le_bytes());
        self
    }

    /// Append a scaled value (SVA). See companion standard 101, subclass 7.2.6.7.
    pub fn scaled(&mut self, v: i16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Append a short floating point number (R32-IEEE STD 754).
    ///
    /// See companion standard 101, subclass 7.2.6.8.
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_bits().to_le_bytes());
        self
    }

    /// Append a binary counter reading (BCR).
    ///
    /// See companion standard 101, subclass 7.2.6.9.
    pub fn binary_counter_reading(&mut self, v: BinaryCounterReading) -> &mut Self {
        let mut flags = v.seq_number & 0x1f;
        if v.has_carry {
            flags |= 0x20;
        }
        if v.is_adjusted {
            flags |= 0x40;
        }
        if v.is_invalid {
            flags |= 0x80;
        }
        self.buf.extend_from_slice(&v.counter_reading.to_le_bytes());
        self.buf.push(flags);
        self
    }

    /// Append a 32 bit string (BSI). See companion standard 101, subclass 7.2.6.13.
    pub fn bits_string32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Append a CP56Time2a time tag in the ASDU's configured time zone.
    pub fn cp56time2a(&mut self, t: Option<DateTime<Utc>>) -> &mut Self {
        self.buf
            .extend_from_slice(&cp56time2a(t, self.params.info_obj_time_zone));
        self
    }

    /// Append a CP24Time2a time tag in the ASDU's configured time zone.
    pub fn cp24time2a(&mut self, t: Option<DateTime<Utc>>) -> &mut Self {
        self.buf
            .extend_from_slice(&cp24time2a(t, self.params.info_obj_time_zone));
        self
    }

    /// Append a CP56Time2a time tag carrying the given IV and SB flags.
    pub fn cp56time2a_tag(&mut self, t: Option<DateTime<Utc>>, flags: TimeTagFlags) -> &mut Self {
        self.buf
            .extend_from_slice(&cp56time2a_tag(t, flags, self.params.info_obj_time_zone));
        self
    }

    /// Append a CP24Time2a time tag carrying the given IV and SB flags.
    pub fn cp24time2a_tag(&mut self, t: Option<DateTime<Utc>>, flags: TimeTagFlags) -> &mut Self {
        self.buf
            .extend_from_slice(&cp24time2a_tag(t, flags, self.params.info_obj_time_zone));
        self
    }

    /// Append a CP16Time2a elapsed-millisecond tag.
    pub fn cp16time2a(&mut self, msec: u16) -> &mut Self {
        self.buf.extend_from_slice(&cp16time2a(msec));
        self
    }

    /// Append a status and status change detection word (SCD).
    pub fn scd(&mut self, scd: StatusAndStatusChangeDetection) -> &mut Self {
        self.buf.extend_from_slice(&scd.0.to_le_bytes());
        self
    }
}

/// Reads information elements out of an ASDU payload.
///
/// The reader borrows the payload and tracks its own position, so decoding
/// never modifies the ASDU and any number of readers may be taken.
#[derive(Debug, Clone)]
pub struct InfoObjReader<'a> {
    params: Params,
    buf: &'a [u8],
    pos: usize,
    /// The validity flags of the time tag read last by a `*_tag` method.
    last_flags: TimeTagFlags,
}

impl<'a> InfoObjReader<'a> {
    /// Octets left to read.
    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(Error::UnexpectedEof)?;
        if end > self.buf.len() {
            return Err(Error::UnexpectedEof);
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// Read one octet.
    pub fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// Read a little-endian `u16`.
    pub fn u16(&mut self) -> Result<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    /// Read an information object address in the configured width.
    pub fn info_obj_addr(&mut self) -> Result<InfoObjAddr> {
        match self.params.info_obj_addr_size {
            1 => Ok(self.byte()? as InfoObjAddr),
            2 => {
                let b = self.take(2)?;
                Ok(u16::from_le_bytes([b[0], b[1]]) as InfoObjAddr)
            }
            3 => {
                let b = self.take(3)?;
                Ok(u32::from_le_bytes([b[0], b[1], b[2], 0]))
            }
            _ => Err(Error::Param),
        }
    }

    /// Read a 3-octet length of file (LOF).
    pub fn length_of_file(&mut self) -> Result<u32> {
        let b = self.take(3)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], 0]))
    }

    /// Read `n` raw octets, as the payload of a variable-length element.
    pub fn take_bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        self.take(n)
    }

    /// Read a normalized value (NVA).
    pub fn normalize(&mut self) -> Result<Normalize> {
        let b = self.take(2)?;
        Ok(Normalize(i16::from_le_bytes([b[0], b[1]])))
    }

    /// Read a scaled value (SVA).
    pub fn scaled(&mut self) -> Result<i16> {
        let b = self.take(2)?;
        Ok(i16::from_le_bytes([b[0], b[1]]))
    }

    /// Read a short floating point number (R32-IEEE STD 754).
    pub fn f32(&mut self) -> Result<f32> {
        let b = self.take(4)?;
        Ok(f32::from_bits(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
    }

    /// Read a binary counter reading (BCR).
    pub fn binary_counter_reading(&mut self) -> Result<BinaryCounterReading> {
        let b = self.take(5)?;
        let v = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let f = b[4];
        Ok(BinaryCounterReading {
            counter_reading: v,
            seq_number: f & 0x1f,
            has_carry: f & 0x20 == 0x20,
            is_adjusted: f & 0x40 == 0x40,
            is_invalid: f & 0x80 == 0x80,
        })
    }

    /// Read a 32 bit string (BSI).
    pub fn bits_string32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Read a CP56Time2a time tag; `None` when the IV bit is set.
    pub fn cp56time2a(&mut self) -> Result<Option<DateTime<Utc>>> {
        let b = self.take(CP56TIME2A_SIZE)?;
        Ok(parse_cp56time2a(b, self.params.info_obj_time_zone))
    }

    /// Read a CP24Time2a time tag; `None` when the IV bit is set.
    pub fn cp24time2a(&mut self) -> Result<Option<DateTime<Utc>>> {
        let b = self.take(CP24TIME2A_SIZE)?;
        Ok(parse_cp24time2a(b, self.params.info_obj_time_zone))
    }

    /// Read a CP56Time2a time tag's reading whatever its validity, keeping
    /// its flags for [`InfoObjReader::time_flags`].
    pub fn cp56time2a_tag(&mut self) -> Result<Option<DateTime<Utc>>> {
        let b = self.take(CP56TIME2A_SIZE)?;
        let (t, flags) = parse_cp56time2a_tag(b, self.params.info_obj_time_zone);
        self.last_flags = flags;
        Ok(t)
    }

    /// Read a CP24Time2a time tag's reading whatever its validity, keeping
    /// its flags for [`InfoObjReader::time_flags`].
    pub fn cp24time2a_tag(&mut self) -> Result<Option<DateTime<Utc>>> {
        let b = self.take(CP24TIME2A_SIZE)?;
        let (t, flags) = parse_cp24time2a_tag(b, self.params.info_obj_time_zone);
        self.last_flags = flags;
        Ok(t)
    }

    /// The IV and SB flags of the time tag read last with
    /// [`InfoObjReader::cp56time2a_tag`] or [`InfoObjReader::cp24time2a_tag`],
    /// and [`TimeTagFlags::GOOD`] before any has been read.
    pub fn time_flags(&self) -> TimeTagFlags {
        self.last_flags
    }

    /// Read a CP16Time2a elapsed-millisecond tag.
    pub fn cp16time2a(&mut self) -> Result<u16> {
        let b = self.take(CP16TIME2A_SIZE)?;
        Ok(parse_cp16time2a(b))
    }

    /// Read a status and status change detection word (SCD).
    pub fn scd(&mut self) -> Result<StatusAndStatusChangeDetection> {
        Ok(StatusAndStatusChangeDetection(self.bits_string32()?))
    }

    /// Read the address of the information object at index `i` of the payload.
    ///
    /// For a sequence (SQ = 1) only the first object carries an address; the
    /// rest are implied by incrementing it. `previous` is the address decoded
    /// for the preceding object.
    pub(crate) fn next_obj_addr(
        &mut self,
        i: usize,
        is_sequence: bool,
        previous: InfoObjAddr,
    ) -> Result<InfoObjAddr> {
        if !is_sequence || i == 0 {
            self.info_obj_addr()
        } else {
            Ok(previous.wrapping_add(1))
        }
    }
}

/// Validate the common arguments of a send helper.
///
/// Checks that at least one information object was given, that the type is
/// sizeable, that the parameters are in range, and that the resulting ASDU
/// fits within [`ASDU_SIZE_MAX`].
pub(crate) fn check_valid(
    params: &Params,
    type_id: TypeId,
    is_sequence: bool,
    infos_len: usize,
) -> Result<()> {
    if infos_len == 0 {
        return Err(Error::NotAnyObjInfo);
    }
    let obj_size = type_id.info_obj_size()?;
    params.valid()?;

    let addr_size = params.info_obj_addr_size as usize;
    let asdu_len = if is_sequence {
        params.identifier_size() + infos_len * obj_size + addr_size
    } else {
        params.identifier_size() + infos_len * (obj_size + addr_size)
    };

    if asdu_len > ASDU_SIZE_MAX {
        return Err(Error::LengthOutOfRange);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::params::{PARAMS_NARROW, PARAMS_STANDARD_101, PARAMS_WIDE};

    fn ident(t: TypeId, n: u8, cause: Cause, ca: CommonAddr) -> Identifier {
        Identifier::new(
            t,
            VariableStruct::new(n),
            CauseOfTransmission::new(cause),
            ca,
        )
    }

    #[test]
    fn marshal_wide_writes_six_identifier_octets() {
        let mut a = Asdu::new(
            PARAMS_WIDE,
            ident(TypeId::M_SP_NA_1, 1, Cause::SPONTANEOUS, 0x1234),
        );
        a.identifier.orig_addr = 7;
        a.encoder().info_obj_addr(0x010203).unwrap().byte(0x01);
        let raw = a.marshal_binary().unwrap();
        assert_eq!(raw, vec![1, 1, 3, 7, 0x34, 0x12, 0x03, 0x02, 0x01, 0x01]);
    }

    #[test]
    fn marshal_narrow_writes_four_identifier_octets() {
        let mut a = Asdu::new(
            PARAMS_STANDARD_101,
            ident(TypeId::M_SP_NA_1, 1, Cause::SPONTANEOUS, 0x12),
        );
        a.encoder().info_obj_addr(0x0102).unwrap().byte(0x01);
        let raw = a.marshal_binary().unwrap();
        assert_eq!(raw, vec![1, 1, 3, 0x12, 0x02, 0x01, 0x01]);
    }

    #[test]
    fn round_trip_through_unmarshal() {
        let mut a = Asdu::new(
            PARAMS_WIDE,
            ident(TypeId::M_ME_NC_1, 2, Cause::PERIODIC, 1),
        );
        {
            let mut e = a.encoder();
            e.info_obj_addr(100).unwrap().f32(1.5).byte(0);
            e.info_obj_addr(101).unwrap().f32(-2.5).byte(0x80);
        }
        let raw = a.marshal_binary().unwrap();
        let b = Asdu::unmarshal_binary(PARAMS_WIDE, &raw).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn global_common_addr_maps_to_255_in_eight_bit_mode() {
        let mut a = Asdu::new(
            PARAMS_NARROW,
            ident(TypeId::C_IC_NA_1, 1, Cause::ACTIVATION, GLOBAL_COMMON_ADDR),
        );
        a.encoder().info_obj_addr(0).unwrap().byte(20);
        let raw = a.marshal_binary().unwrap();
        assert_eq!(raw[3], 255);
        let b = Asdu::unmarshal_binary(PARAMS_NARROW, &raw).unwrap();
        assert_eq!(b.common_addr(), GLOBAL_COMMON_ADDR);
    }

    #[test]
    fn marshal_rejects_invalid_identifiers() {
        let a = Asdu::new(PARAMS_WIDE, ident(TypeId::M_SP_NA_1, 1, Cause::UNUSED, 1));
        assert_eq!(a.marshal_binary(), Err(Error::CauseZero));

        let a = Asdu::new(
            PARAMS_WIDE,
            ident(TypeId::M_SP_NA_1, 1, Cause::SPONTANEOUS, 0),
        );
        assert_eq!(a.marshal_binary(), Err(Error::CommonAddrZero));

        // In 8-bit mode 255 is reserved for the broadcast mapping.
        let a = Asdu::new(
            PARAMS_NARROW,
            ident(TypeId::M_SP_NA_1, 1, Cause::SPONTANEOUS, 255),
        );
        assert_eq!(a.marshal_binary(), Err(Error::Param));
    }

    #[test]
    fn marshal_rejects_oversized_payloads() {
        let mut a = Asdu::new(
            PARAMS_WIDE,
            ident(TypeId::M_SP_NA_1, 1, Cause::SPONTANEOUS, 1),
        );
        a.info_obj = vec![0; ASDU_SIZE_MAX];
        assert_eq!(a.marshal_binary(), Err(Error::LengthOutOfRange));
    }

    #[test]
    fn unmarshal_rejects_trailing_octets_and_short_payloads() {
        // One M_SP_NA_1 object: 3 address octets + 1 value octet.
        let exact = [1u8, 1, 3, 0, 1, 0, 1, 0, 0, 0x01];
        assert_eq!(
            Asdu::unmarshal_binary(PARAMS_WIDE, &exact).unwrap().info_obj.len(),
            4
        );

        // A conforming sender cannot produce a surplus octet, so two extra
        // ones mean the frame is not what its qualifier claims.
        let padded = [1u8, 1, 3, 0, 1, 0, 1, 0, 0, 0x01, 0xde, 0xad];
        assert_eq!(
            Asdu::unmarshal_binary(PARAMS_WIDE, &padded),
            Err(Error::TrailingOctets)
        );

        let short = [1u8, 1, 3, 0, 1, 0, 1, 0];
        assert_eq!(
            Asdu::unmarshal_binary(PARAMS_WIDE, &short),
            Err(Error::UnexpectedEof)
        );
    }

    #[test]
    fn trailing_octets_are_discarded_only_when_the_parameter_allows_it() {
        let lenient = Params {
            allow_trailing_octets: true,
            ..PARAMS_WIDE
        };
        let padded = [1u8, 1, 3, 0, 1, 0, 1, 0, 0, 0x01, 0xde, 0xad];
        let a = Asdu::unmarshal_binary(lenient, &padded).unwrap();
        assert_eq!(a.info_obj, vec![1, 0, 0, 0x01], "the surplus is discarded");
    }

    #[test]
    fn a_file_segment_is_sized_by_its_own_length_octet() {
        // F_SG_NA_1: IOA(3) + NOF(2) + NOS(1) + LOS(1) + 3 segment octets.
        let raw = [
            125u8, 1, 13, 0, 1, 0, // identifier: F_SG_NA_1, 1 object, FileTransfer, CA 1
            0x64, 0, 0, // IOA 100
            0x02, 0, // NOF
            0x01, // NOS
            0x03, // LOS = 3
            0xaa, 0xbb, 0xcc,
        ];
        let a = Asdu::unmarshal_binary(PARAMS_WIDE, &raw).unwrap();
        assert_eq!(a.info_obj.len(), 10);

        // One octet short of what LOS promises.
        assert_eq!(
            Asdu::unmarshal_binary(PARAMS_WIDE, &raw[..raw.len() - 1]),
            Err(Error::UnexpectedEof)
        );
        // One octet more than LOS accounts for.
        let mut padded = raw.to_vec();
        padded.push(0xdd);
        assert_eq!(
            Asdu::unmarshal_binary(PARAMS_WIDE, &padded),
            Err(Error::TrailingOctets)
        );
        // Truncated before the LOS octet itself.
        assert_eq!(
            Asdu::unmarshal_binary(PARAMS_WIDE, &raw[..9]),
            Err(Error::UnexpectedEof)
        );
    }

    #[test]
    fn unmarshal_rejects_unknown_type_identifications() {
        let raw = [200u8, 1, 3, 0, 1, 0, 1, 0, 0, 0x01];
        assert_eq!(
            Asdu::unmarshal_binary(PARAMS_WIDE, &raw),
            Err(Error::TypeIdentifier)
        );
    }

    #[test]
    fn sequence_addresses_are_implied_by_increment() {
        let mut a = Asdu::new(
            PARAMS_WIDE,
            Identifier::new(
                TypeId::M_SP_NA_1,
                VariableStruct {
                    number: 3,
                    is_sequence: true,
                },
                CauseOfTransmission::new(Cause::SPONTANEOUS),
                1,
            ),
        );
        a.encoder().info_obj_addr(100).unwrap().byte(1).byte(0).byte(1);
        let mut r = a.reader();
        let mut addr = 0;
        for i in 0..3 {
            addr = r.next_obj_addr(i, true, addr).unwrap();
            let _ = r.byte().unwrap();
            assert_eq!(addr, 100 + i as u32);
        }
    }

    #[test]
    fn reader_reports_truncation_instead_of_panicking() {
        let a = Asdu::new(PARAMS_WIDE, ident(TypeId::M_SP_NA_1, 1, Cause::SPONTANEOUS, 1));
        let mut r = a.reader();
        assert_eq!(r.byte(), Err(Error::UnexpectedEof));
        assert_eq!(r.cp56time2a(), Err(Error::UnexpectedEof));
    }

    #[test]
    fn encoder_enforces_the_address_width() {
        let mut a = Asdu::new(PARAMS_NARROW, ident(TypeId::M_SP_NA_1, 1, Cause::SPONTANEOUS, 1));
        assert_eq!(
            a.encoder().info_obj_addr(256).err(),
            Some(Error::InfoObjAddrFit)
        );
        let mut a = Asdu::new(PARAMS_WIDE, ident(TypeId::M_SP_NA_1, 1, Cause::SPONTANEOUS, 1));
        assert!(a.encoder().info_obj_addr(16_777_215).is_ok());
        assert_eq!(
            a.encoder().info_obj_addr(16_777_216).err(),
            Some(Error::InfoObjAddrFit)
        );
    }

    #[test]
    fn check_valid_bounds_the_object_count() {
        assert_eq!(
            check_valid(&PARAMS_WIDE, TypeId::M_SP_NA_1, false, 0),
            Err(Error::NotAnyObjInfo)
        );
        // 6 identifier + n*(3+1) <= 249  =>  n <= 60
        assert!(check_valid(&PARAMS_WIDE, TypeId::M_SP_NA_1, false, 60).is_ok());
        assert_eq!(
            check_valid(&PARAMS_WIDE, TypeId::M_SP_NA_1, false, 61),
            Err(Error::LengthOutOfRange)
        );
        // A sequence pays the address only once.
        assert!(check_valid(&PARAMS_WIDE, TypeId::M_SP_NA_1, true, 240).is_ok());
    }

    #[test]
    fn reply_mirror_keeps_the_payload_and_changes_the_cause() {
        let mut a = Asdu::new(
            PARAMS_WIDE,
            ident(TypeId::C_IC_NA_1, 1, Cause::ACTIVATION, 5),
        );
        a.encoder().info_obj_addr(0).unwrap().byte(20);
        let m = a.reply_mirror(Cause::ACTIVATION_CON);
        assert_eq!(m.coa().cause, Cause::ACTIVATION_CON);
        assert_eq!(m.common_addr(), 5);
        assert_eq!(m.info_obj, a.info_obj);
        // The original is untouched.
        assert_eq!(a.coa().cause, Cause::ACTIVATION);
    }

    #[test]
    fn binary_counter_reading_round_trips() {
        let bcr = BinaryCounterReading {
            counter_reading: -12345,
            seq_number: 17,
            has_carry: true,
            is_adjusted: false,
            is_invalid: true,
        };
        let mut a = Asdu::new(PARAMS_WIDE, ident(TypeId::M_IT_NA_1, 1, Cause::SPONTANEOUS, 1));
        a.encoder().binary_counter_reading(bcr);
        assert_eq!(a.reader().binary_counter_reading().unwrap(), bcr);
    }
}
