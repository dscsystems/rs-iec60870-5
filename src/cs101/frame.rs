// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! FT1.2 frame format, per IEC 60870-5-1 and IEC 60870-5-2.
//!
//! Three frame formats share the line:
//!
//! ```text
//! single character   E5
//! fixed length       10 | C | A... | CS | 16
//! variable length    68 | L | L | 68 | C | A... | ASDU... | CS | 16
//! ```
//!
//! `C` is the control field, `A` the link address (1 or 2 octets, or none),
//! `L` the count of control + address + ASDU octets, and `CS` the arithmetic
//! sum of those same octets truncated to eight bits.

use std::fmt;

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::error::{Error, Result};

/// The single-character acknowledgement.
pub const SINGLE_CHAR_ACK: u8 = 0xE5;
/// Start character of a fixed-length frame.
pub const START_FIXED: u8 = 0x10;
/// Start character of a variable-length frame.
pub const START_VARIABLE: u8 = 0x68;
/// End character of both fixed- and variable-length frames.
pub const END_CHAR: u8 = 0x16;
/// The largest value the FT1.2 length octet can carry.
///
/// In a variable-length frame — `68 L L 68 | control | link address | ASDU |
/// checksum | 16` — the octet `L` counts the control field, the link address
/// and the ASDU. It is one octet, so it runs to 255, and the largest ASDU is
/// therefore `MAX_LENGTH_FIELD - 1 - link_addr_size`: 253 octets with a one
/// octet link address, which is the figure IEC 60870-5-103 quotes as its
/// maximum ASDU length.
pub const MAX_LENGTH_FIELD: usize = 255;

/// The largest variable-length frame on the wire.
///
/// `68 L L 68` (4 octets) + `L` + checksum + end (2 octets), so 261 — **not**
/// 255. Confusing the length field with the frame length costs six octets of
/// every frame, which is enough to refuse a maximum-size ASDU.
pub const MAX_FRAME_LEN: usize = MAX_LENGTH_FIELD + 6;

/// The largest ASDU that fits one variable-length frame for a given link
/// address size (0, 1 or 2 octets).
pub const fn max_asdu_len(link_addr_size: u8) -> usize {
    MAX_LENGTH_FIELD - 1 - link_addr_size as usize
}

// -- control field bits ---------------------------------------------------
/// DIR: direction. In balanced mode, set by station A.
pub const CTRL_DIR: u8 = 0x80;
/// PRM: primary message. Set on frames from the primary station.
pub const CTRL_PRM: u8 = 0x40;
/// FCB: frame count bit (primary frames).
pub const CTRL_FCB: u8 = 0x20;
/// FCV: frame count bit valid (primary frames).
pub const CTRL_FCV: u8 = 0x10;
/// ACD: access demand, class 1 data waiting (secondary frames).
pub const CTRL_ACD: u8 = 0x20;
/// DFC: data flow control, buffers full (secondary frames).
pub const CTRL_DFC: u8 = 0x10;
/// Mask of the function code, bits 0..=3.
pub const CTRL_FUNC_MASK: u8 = 0x0F;

/// Function codes sent by the primary station (PRM = 1).
pub mod prim_fc {
    /// 0: reset of remote link
    pub const RESET_LINK: u8 = 0;
    /// 1: reset of user process
    pub const RESET_USER: u8 = 1;
    /// 2: test function of link
    pub const TEST_LINK: u8 = 2;
    /// 3: user data, confirmed
    pub const USER_DATA_CONF: u8 = 3;
    /// 4: user data, unconfirmed
    pub const USER_DATA_NO_CONF: u8 = 4;
    /// 8: request access demand
    pub const REQ_ACCESS: u8 = 8;
    /// 9: request status of link
    pub const REQ_STATUS: u8 = 9;
    /// 10: request user data class 1
    pub const REQ_DATA1: u8 = 10;
    /// 11: request user data class 2
    pub const REQ_DATA2: u8 = 11;
}

/// Function codes sent by the secondary station (PRM = 0).
pub mod sec_fc {
    /// 0: positive acknowledgement
    pub const CONF_ACK: u8 = 0;
    /// 1: negative acknowledgement, link busy
    pub const CONF_NACK: u8 = 1;
    /// 8: user data
    pub const USER_DATA_CONF: u8 = 8;
    /// 9: negative acknowledgement, requested data not available
    pub const USER_DATA_NO_REP: u8 = 9;
    /// 11: status of link / access demand
    pub const RESP_STATUS: u8 = 11;
    /// 14: link service not functioning
    pub const RESP_LINK_NF: u8 = 14;
    /// 15: link service not implemented
    pub const RESP_LINK_NI: u8 = 15;
}

/// A decoded control field octet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ControlField {
    /// Direction bit; in balanced mode station A sets it.
    pub dir: bool,
    /// This frame comes from a primary station.
    pub prm: bool,
    /// Frame count bit (primary frames only).
    pub fcb: bool,
    /// Frame count bit valid (primary frames only).
    pub fcv: bool,
    /// Access demand: class 1 data is waiting (secondary frames only).
    pub acd: bool,
    /// Data flow control: the sender's buffers are full (secondary frames only).
    pub dfc: bool,
    /// Function code, bits 0..=3.
    pub fun: u8,
}

impl ControlField {
    /// Decode the wire octet.
    pub const fn parse(b: u8) -> ControlField {
        let prm = (b & CTRL_PRM) != 0;
        ControlField {
            dir: (b & CTRL_DIR) != 0,
            prm,
            // The same two bits mean FCB/FCV on primary frames and ACD/DFC on
            // secondary ones, so only one pair is ever meaningful.
            fcb: prm && (b & CTRL_FCB) != 0,
            fcv: prm && (b & CTRL_FCV) != 0,
            acd: !prm && (b & CTRL_ACD) != 0,
            dfc: !prm && (b & CTRL_DFC) != 0,
            fun: b & CTRL_FUNC_MASK,
        }
    }

    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        let mut b = 0u8;
        if self.dir {
            b |= CTRL_DIR;
        }
        if self.prm {
            b |= CTRL_PRM;
            if self.fcb {
                b |= CTRL_FCB;
            }
            if self.fcv {
                b |= CTRL_FCV;
            }
        } else {
            if self.acd {
                b |= CTRL_ACD;
            }
            if self.dfc {
                b |= CTRL_DFC;
            }
        }
        b | (self.fun & CTRL_FUNC_MASK)
    }

    /// A primary control field.
    pub const fn primary(fun: u8, fcv: bool, fcb: bool, dir: bool) -> ControlField {
        ControlField {
            dir,
            prm: true,
            fcb: fcv && fcb,
            fcv,
            acd: false,
            dfc: false,
            fun,
        }
    }

    /// A secondary control field.
    pub const fn secondary(fun: u8, acd: bool, dir: bool) -> ControlField {
        ControlField {
            dir,
            prm: false,
            fcb: false,
            fcv: false,
            acd,
            dfc: false,
            fun,
        }
    }
}

impl fmt::Display for ControlField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CTRL<{}", if self.prm { "PRM" } else { "SEC" })?;
        if self.dir {
            f.write_str(",DIR")?;
        }
        write!(f, " FC=0x{:02X}", self.fun)?;
        if self.fcv {
            write!(f, " FCB={}", u8::from(self.fcb))?;
        }
        if !self.prm && self.acd {
            f.write_str(" ACD=1")?;
        }
        if !self.prm && self.dfc {
            f.write_str(" DFC=1")?;
        }
        f.write_str(">")
    }
}

/// An FT1.2 frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// `E5`: the single-character positive acknowledgement.
    SingleCharAck,
    /// A fixed-length frame: control field and link address only.
    Fixed {
        /// The control field.
        control: ControlField,
        /// The link address, as configured (0, 1 or 2 octets).
        link_addr: u16,
    },
    /// A variable-length frame carrying an ASDU.
    Variable {
        /// The control field.
        control: ControlField,
        /// The link address, as configured (0, 1 or 2 octets).
        link_addr: u16,
        /// The serialised ASDU.
        asdu: Vec<u8>,
    },
}

impl Frame {
    /// The control field, or `None` for a single-character acknowledgement.
    pub fn control(&self) -> Option<ControlField> {
        match self {
            Frame::SingleCharAck => None,
            Frame::Fixed { control, .. } | Frame::Variable { control, .. } => Some(*control),
        }
    }

    /// The link address, or `None` for a single-character acknowledgement.
    pub fn link_addr(&self) -> Option<u16> {
        match self {
            Frame::SingleCharAck => None,
            Frame::Fixed { link_addr, .. } | Frame::Variable { link_addr, .. } => Some(*link_addr),
        }
    }

    /// The ASDU payload of a variable-length frame.
    pub fn asdu(&self) -> Option<&[u8]> {
        match self {
            Frame::Variable { asdu, .. } => Some(asdu),
            _ => None,
        }
    }

    /// Serialise using a link address of `link_addr_size` octets (0, 1 or 2).
    pub fn marshal(&self, link_addr_size: u8) -> Result<Vec<u8>> {
        if link_addr_size > 2 {
            return Err(Error::Frame("link address size must be 0, 1 or 2"));
        }
        match self {
            Frame::SingleCharAck => Ok(vec![SINGLE_CHAR_ACK]),

            Frame::Fixed {
                control,
                link_addr,
            } => {
                let addr = encode_link_addr(*link_addr, link_addr_size);
                let mut buf = Vec::with_capacity(4 + addr.len());
                buf.push(START_FIXED);
                buf.push(control.value());
                buf.extend_from_slice(&addr);
                buf.push(checksum(control.value(), &addr, &[]));
                buf.push(END_CHAR);
                Ok(buf)
            }

            Frame::Variable {
                control,
                link_addr,
                asdu,
            } => {
                let addr = encode_link_addr(*link_addr, link_addr_size);
                // The length field counts control + link address + ASDU. It is
                // computed as a usize and checked before it is narrowed to an
                // octet: narrowing first would wrap a 256 octet total to L = 0
                // and put a frame on the wire whose length field describes
                // none of it.
                let len_field = 1 + addr.len() + asdu.len();
                if len_field > MAX_LENGTH_FIELD {
                    return Err(Error::Frame("frame length exceeds the FT1.2 maximum"));
                }
                let mut buf = Vec::with_capacity(len_field + 6);
                buf.push(START_VARIABLE);
                buf.push(len_field as u8);
                buf.push(len_field as u8);
                buf.push(START_VARIABLE);
                buf.push(control.value());
                buf.extend_from_slice(&addr);
                buf.extend_from_slice(asdu);
                buf.push(checksum(control.value(), &addr, asdu));
                buf.push(END_CHAR);
                Ok(buf)
            }
        }
    }
}

/// Encode a link address into `size` little-endian octets.
pub(crate) fn encode_link_addr(addr: u16, size: u8) -> Vec<u8> {
    match size {
        1 => vec![addr as u8],
        2 => addr.to_le_bytes().to_vec(),
        _ => Vec::new(),
    }
}

/// The FT1.2 check octet: the arithmetic sum of the control field, the link
/// address and the ASDU, truncated to eight bits.
pub(crate) fn checksum(control: u8, link_addr: &[u8], asdu: &[u8]) -> u8 {
    let mut sum = control;
    for b in link_addr.iter().chain(asdu) {
        sum = sum.wrapping_add(*b);
    }
    sum
}

fn decode_link_addr(b: &[u8]) -> u16 {
    match b.len() {
        1 => b[0] as u16,
        2 => u16::from_le_bytes([b[0], b[1]]),
        _ => 0,
    }
}

/// Read one FT1.2 frame from `r`.
///
/// Malformed frames — a bad start or end character, mismatched length octets,
/// or a failed checksum — yield [`Error::Frame`]; the caller resynchronises by
/// simply calling again, which is what both endpoints do. A closed stream
/// yields [`Error::Io`].
pub async fn read_frame<R>(r: &mut R, link_addr_size: u8) -> Result<Frame>
where
    R: AsyncRead + Unpin,
{
    if link_addr_size > 2 {
        return Err(Error::Frame("link address size must be 0, 1 or 2"));
    }
    let addr_len = link_addr_size as usize;

    let mut start = [0u8; 1];
    r.read_exact(&mut start).await?;

    match start[0] {
        SINGLE_CHAR_ACK => Ok(Frame::SingleCharAck),

        START_FIXED => {
            // control + address + checksum + end
            let mut buf = vec![0u8; 1 + addr_len + 2];
            r.read_exact(&mut buf).await?;
            let control = buf[0];
            let addr = &buf[1..1 + addr_len];
            let cs = buf[buf.len() - 2];
            let end = buf[buf.len() - 1];

            if end != END_CHAR {
                return Err(Error::Frame("invalid end character"));
            }
            if checksum(control, addr, &[]) != cs {
                return Err(Error::Frame("checksum mismatch"));
            }
            Ok(Frame::Fixed {
                control: ControlField::parse(control),
                link_addr: decode_link_addr(addr),
            })
        }

        START_VARIABLE => {
            let mut header = [0u8; 4]; // L, L, 0x68, control
            r.read_exact(&mut header).await?;
            let (l1, l2, start2, control) = (header[0], header[1], header[2], header[3]);

            if l1 != l2 {
                return Err(Error::Frame("length fields do not match"));
            }
            if start2 != START_VARIABLE {
                return Err(Error::Frame("invalid second start character"));
            }
            // The length counts control + address + ASDU.
            if (l1 as usize) < 1 + addr_len {
                return Err(Error::Frame("frame is too short for its header"));
            }
            // There is no upper bound left to check: `l1` is one octet, so it
            // cannot exceed MAX_LENGTH_FIELD, and every value up to it
            // describes a legal frame of `l1 + 6` octets. Rejecting anything
            // above 249 here refused the largest frames the standard defines.

            // The length octet counts control + address + ASDU, and the
            // control octet has already been read, so what remains is
            // (l1 - 1) + checksum + end.
            let body_len = l1 as usize + 1;
            let mut body = vec![0u8; body_len];
            r.read_exact(&mut body).await?;

            let addr = &body[..addr_len];
            let asdu = &body[addr_len..body_len - 2];
            let cs = body[body_len - 2];
            let end = body[body_len - 1];

            if end != END_CHAR {
                return Err(Error::Frame("invalid end character"));
            }
            if checksum(control, addr, asdu) != cs {
                return Err(Error::Frame("checksum mismatch"));
            }
            Ok(Frame::Variable {
                control: ControlField::parse(control),
                link_addr: decode_link_addr(addr),
                asdu: asdu.to_vec(),
            })
        }

        _ => Err(Error::Frame("invalid start character")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_field_round_trips_for_primary_frames() {
        for fun in 0u8..16 {
            for fcv in [false, true] {
                for fcb in [false, true] {
                    for dir in [false, true] {
                        let c = ControlField::primary(fun, fcv, fcb, dir);
                        assert_eq!(ControlField::parse(c.value()), c);
                    }
                }
            }
        }
    }

    #[test]
    fn control_field_round_trips_for_secondary_frames() {
        for fun in 0u8..16 {
            for acd in [false, true] {
                for dir in [false, true] {
                    let c = ControlField::secondary(fun, acd, dir);
                    assert_eq!(ControlField::parse(c.value()), c);
                }
            }
        }
    }

    #[test]
    fn fcb_is_only_encoded_when_fcv_is_set() {
        // FCB without FCV is meaningless and must not reach the wire.
        let c = ControlField::primary(prim_fc::RESET_LINK, false, true, false);
        assert_eq!(c.value() & CTRL_FCB, 0);
    }

    #[test]
    fn reset_link_matches_the_standard_octets() {
        // Reset of remote link to station 1: PRM=1, FCV=0, FC=0.
        let f = Frame::Fixed {
            control: ControlField::primary(prim_fc::RESET_LINK, false, false, false),
            link_addr: 1,
        };
        assert_eq!(f.marshal(1).unwrap(), vec![0x10, 0x40, 0x01, 0x41, 0x16]);
    }

    #[test]
    fn request_class2_matches_the_standard_octets() {
        // Request user data class 2, FCV=1, FCB=1, station 1.
        let f = Frame::Fixed {
            control: ControlField::primary(prim_fc::REQ_DATA2, true, true, false),
            link_addr: 1,
        };
        // 0x40 PRM | 0x20 FCB | 0x10 FCV | 0x0b FC
        assert_eq!(f.marshal(1).unwrap(), vec![0x10, 0x7b, 0x01, 0x7c, 0x16]);
    }

    #[test]
    fn variable_frame_layout_and_checksum() {
        let f = Frame::Variable {
            control: ControlField::primary(prim_fc::USER_DATA_CONF, true, true, false),
            link_addr: 1,
            asdu: vec![0x64, 0x01, 0x06, 0x01, 0x00, 0x00, 0x14],
        };
        let raw = f.marshal(1).unwrap();
        assert_eq!(raw[0], START_VARIABLE);
        assert_eq!(raw[1], raw[2], "the length octet is repeated");
        assert_eq!(raw[1] as usize, 1 + 1 + 7, "control + address + ASDU");
        assert_eq!(raw[3], START_VARIABLE);
        assert_eq!(*raw.last().unwrap(), END_CHAR);
        let cs = raw[raw.len() - 2];
        let sum: u8 = raw[4..raw.len() - 2]
            .iter()
            .fold(0u8, |a, b| a.wrapping_add(*b));
        assert_eq!(cs, sum);
    }

    async fn round_trip(f: Frame, size: u8) -> Frame {
        let raw = f.marshal(size).unwrap();
        let mut r = raw.as_slice();
        read_frame(&mut r, size).await.unwrap()
    }

    #[tokio::test]
    async fn frames_round_trip_through_the_reader() {
        for size in [1u8, 2] {
            let addr = if size == 1 { 200 } else { 0x1234 };
            for f in [
                Frame::SingleCharAck,
                Frame::Fixed {
                    control: ControlField::secondary(sec_fc::CONF_ACK, true, false),
                    link_addr: addr,
                },
                Frame::Variable {
                    control: ControlField::primary(prim_fc::USER_DATA_CONF, true, false, true),
                    link_addr: addr,
                    asdu: vec![1, 2, 3, 4, 5],
                },
            ] {
                assert_eq!(round_trip(f.clone(), size).await, f, "size {size}");
            }
        }
    }

    #[tokio::test]
    async fn a_corrupted_checksum_is_rejected() {
        let f = Frame::Fixed {
            control: ControlField::primary(prim_fc::RESET_LINK, false, false, false),
            link_addr: 1,
        };
        let mut raw = f.marshal(1).unwrap();
        raw[3] ^= 0xff; // corrupt the checksum
        let mut r = raw.as_slice();
        assert_eq!(
            read_frame(&mut r, 1).await,
            Err(Error::Frame("checksum mismatch"))
        );
    }

    #[tokio::test]
    async fn a_missing_end_character_is_rejected() {
        let f = Frame::Fixed {
            control: ControlField::primary(prim_fc::RESET_LINK, false, false, false),
            link_addr: 1,
        };
        let mut raw = f.marshal(1).unwrap();
        *raw.last_mut().unwrap() = 0x00;
        let mut r = raw.as_slice();
        assert_eq!(
            read_frame(&mut r, 1).await,
            Err(Error::Frame("invalid end character"))
        );
    }

    #[tokio::test]
    async fn mismatched_length_octets_are_rejected() {
        let mut raw = Frame::Variable {
            control: ControlField::primary(prim_fc::USER_DATA_CONF, true, true, false),
            link_addr: 1,
            asdu: vec![1, 2, 3],
        }
        .marshal(1)
        .unwrap();
        raw[2] = raw[2].wrapping_add(1);
        let mut r = raw.as_slice();
        assert_eq!(
            read_frame(&mut r, 1).await,
            Err(Error::Frame("length fields do not match"))
        );
    }

    #[tokio::test]
    async fn an_unknown_start_character_is_rejected_one_octet_at_a_time() {
        // The endpoints resynchronise by calling read_frame again, which is
        // what makes line noise recoverable.
        let mut stream: &[u8] = &[0xaa, 0xbb, 0xe5];
        assert!(read_frame(&mut stream, 1).await.is_err());
        assert!(read_frame(&mut stream, 1).await.is_err());
        assert_eq!(read_frame(&mut stream, 1).await.unwrap(), Frame::SingleCharAck);
    }

    #[tokio::test]
    async fn a_variable_frame_shorter_than_its_header_is_rejected() {
        // Length 1 cannot hold the control octet plus a 2 octet address.
        let mut stream: &[u8] = &[0x68, 0x01, 0x01, 0x68, 0x40, 0x40, 0x16];
        assert_eq!(
            read_frame(&mut stream, 2).await,
            Err(Error::Frame("frame is too short for its header"))
        );
    }

    #[test]
    fn max_asdu_len_matches_the_standard() {
        // L counts control + link address + ASDU, and runs to 255.
        assert_eq!(max_asdu_len(0), 254);
        assert_eq!(max_asdu_len(1), 253, "the figure IEC 60870-5-103 quotes");
        assert_eq!(max_asdu_len(2), 252);
        assert_eq!(MAX_FRAME_LEN, 261, "the largest frame on the wire is L+6");
    }

    /// Assemble a wire-format frame by hand, so the reader is tested against
    /// the format rather than against `marshal`.
    fn build_variable_frame(control: u8, link_addr: &[u8], asdu: &[u8]) -> Vec<u8> {
        let l = (1 + link_addr.len() + asdu.len()) as u8;
        let mut out = vec![START_VARIABLE, l, l, START_VARIABLE, control];
        out.extend_from_slice(link_addr);
        out.extend_from_slice(asdu);
        out.push(checksum(control, link_addr, asdu));
        out.push(END_CHAR);
        out
    }

    #[tokio::test]
    async fn the_reader_accepts_every_legal_length_field() {
        for size in [1u8, 2] {
            let max = max_asdu_len(size);
            for asdu_len in [0, 1, 247, 248, max - 1, max] {
                let addr = vec![0x01; size as usize];
                let asdu = vec![0xab; asdu_len];
                let wire = build_variable_frame(0x53, &addr, &asdu);

                let want_l = 1 + size as usize + asdu_len;
                assert_eq!(wire[1] as usize, want_l);
                assert_eq!(wire.len(), want_l + 6, "the frame is L+6 octets");

                let mut r = wire.as_slice();
                let f = read_frame(&mut r, size)
                    .await
                    .unwrap_or_else(|e| panic!("size {size}, ASDU {asdu_len} (L={want_l}): {e}"));
                assert_eq!(f.asdu(), Some(&asdu[..]), "size {size}, ASDU {asdu_len}");
            }
        }
    }

    #[tokio::test]
    async fn the_largest_asdu_round_trips() {
        for size in [1u8, 2] {
            let f = Frame::Variable {
                control: ControlField::primary(prim_fc::USER_DATA_CONF, true, true, false),
                link_addr: 1,
                asdu: vec![0xcd; max_asdu_len(size)],
            };
            let raw = f
                .marshal(size)
                .unwrap_or_else(|e| panic!("the largest ASDU was refused for size {size}: {e}"));
            assert_eq!(raw[1] as usize, MAX_LENGTH_FIELD);
            assert_eq!(raw.len(), MAX_FRAME_LEN);

            let mut r = raw.as_slice();
            assert_eq!(read_frame(&mut r, size).await.unwrap(), f, "size {size}");
        }
    }

    #[test]
    fn marshal_rejects_an_oversized_asdu() {
        // One octet more than fits must be refused, not wrapped to L = 0.
        for size in [0u8, 1, 2] {
            let f = Frame::Variable {
                control: ControlField::primary(prim_fc::USER_DATA_CONF, true, true, false),
                link_addr: 1,
                asdu: vec![0; max_asdu_len(size) + 1],
            };
            assert_eq!(
                f.marshal(size),
                Err(Error::Frame("frame length exceeds the FT1.2 maximum")),
                "size {size}"
            );
        }
    }

    #[test]
    fn accessors_expose_the_frame_parts() {
        let f = Frame::Variable {
            control: ControlField::secondary(sec_fc::USER_DATA_CONF, true, false),
            link_addr: 7,
            asdu: vec![9, 9],
        };
        assert_eq!(f.link_addr(), Some(7));
        assert_eq!(f.asdu(), Some(&[9u8, 9][..]));
        assert!(f.control().unwrap().acd);
        assert_eq!(Frame::SingleCharAck.control(), None);
        assert_eq!(Frame::SingleCharAck.asdu(), None);
    }
}
