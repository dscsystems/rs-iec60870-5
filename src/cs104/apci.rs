// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The IEC 60870-5-104 Application Protocol Control Information.
//!
//! ```text
//! |              APCI                   |       ASDU         |
//! | start | APDU length | control field |       ASDU         |
//!                  |          APDU field size(253)           |
//! bytes |    1  |     1       |     4        |               |
//! ```
//!
//! Three frame formats share the four control octets:
//!
//! * **I** (information transfer) carries an ASDU and both sequence numbers.
//! * **S** (supervisory) acknowledges received I-frames and carries no data.
//! * **U** (unnumbered) starts, stops and tests the connection.

use std::fmt;

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::asdu::ASDU_SIZE_MAX;
use crate::error::{Error, Result};

/// The APDU start character.
pub(crate) const START_FRAME: u8 = 0x68;

/// Octets of the APCI control field.
pub(crate) const APCI_CTL_FIELD_SIZE: usize = 4;

/// Maximum APDU size: start + length + control field + ASDU.
pub(crate) const APDU_SIZE_MAX: usize = 255;

/// Maximum APDU field size: control field + ASDU.
pub(crate) const APDU_FIELD_SIZE_MAX: usize = APCI_CTL_FIELD_SIZE + ASDU_SIZE_MAX;

/// Sequence numbers are 15 bit and wrap at 32767.
pub(crate) const SEQ_MASK: u16 = 32767;

/// U-frame control functions, in the high six bits of the first control octet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum UFunction {
    /// 0x04: start data transfer, activation
    StartDtActive = 0x04,
    /// 0x08: start data transfer, confirmation
    StartDtConfirm = 0x08,
    /// 0x10: stop data transfer, activation
    StopDtActive = 0x10,
    /// 0x20: stop data transfer, confirmation
    StopDtConfirm = 0x20,
    /// 0x40: test frame, activation
    TestFrActive = 0x40,
    /// 0x80: test frame, confirmation
    TestFrConfirm = 0x80,
}

impl UFunction {
    /// Decode the function bits, rejecting combinations the standard does not define.
    pub(crate) fn from_bits(b: u8) -> Option<UFunction> {
        Some(match b {
            0x04 => UFunction::StartDtActive,
            0x08 => UFunction::StartDtConfirm,
            0x10 => UFunction::StopDtActive,
            0x20 => UFunction::StopDtConfirm,
            0x40 => UFunction::TestFrActive,
            0x80 => UFunction::TestFrConfirm,
            _ => return None,
        })
    }
}

impl fmt::Display for UFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            UFunction::StartDtActive => "StartDtActive",
            UFunction::StartDtConfirm => "StartDtConfirm",
            UFunction::StopDtActive => "StopDtActive",
            UFunction::StopDtConfirm => "StopDtConfirm",
            UFunction::TestFrActive => "TestFrActive",
            UFunction::TestFrConfirm => "TestFrConfirm",
        })
    }
}

/// A parsed APCI control field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apci {
    /// Information transfer format: send and receive sequence numbers.
    I {
        /// Send sequence number of this frame.
        send_sn: u16,
        /// Receive sequence number: everything below it is acknowledged.
        recv_sn: u16,
    },
    /// Supervisory format: acknowledges received I-frames.
    S {
        /// Receive sequence number.
        recv_sn: u16,
    },
    /// Unnumbered control format.
    U {
        /// The control function, or `None` for an undefined bit combination.
        function: Option<UFunction>,
    },
}

impl fmt::Display for Apci {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Apci::I { send_sn, recv_sn } => write!(f, "I[sendNO: {send_sn}, recvNO: {recv_sn}]"),
            Apci::S { recv_sn } => write!(f, "S[recvNO: {recv_sn}]"),
            Apci::U { function: Some(x) } => write!(f, "U[function: {x}]"),
            Apci::U { function: None } => f.write_str("U[function: Unknown]"),
        }
    }
}

/// Build an I-frame APDU carrying `asdu`.
pub(crate) fn new_i_frame(send_sn: u16, recv_sn: u16, asdu: &[u8]) -> Result<Vec<u8>> {
    if asdu.len() > ASDU_SIZE_MAX {
        return Err(Error::LengthOutOfRange);
    }
    let mut b = Vec::with_capacity(asdu.len() + 6);
    b.push(START_FRAME);
    b.push((asdu.len() + APCI_CTL_FIELD_SIZE) as u8);
    b.push((send_sn << 1) as u8);
    b.push((send_sn >> 7) as u8);
    b.push((recv_sn << 1) as u8);
    b.push((recv_sn >> 7) as u8);
    b.extend_from_slice(asdu);
    Ok(b)
}

/// Build an S-frame APDU acknowledging up to `recv_sn`.
pub(crate) fn new_s_frame(recv_sn: u16) -> [u8; 6] {
    [
        START_FRAME,
        4,
        0x01,
        0x00,
        (recv_sn << 1) as u8,
        (recv_sn >> 7) as u8,
    ]
}

/// Build a U-frame APDU for the given control function.
pub(crate) fn new_u_frame(which: UFunction) -> [u8; 6] {
    [START_FRAME, 4, which as u8 | 0x03, 0x00, 0x00, 0x00]
}

/// Split an APDU into its control field and the ASDU that follows.
///
/// `apdu` must be a complete frame of at least six octets, as produced by
/// [`read_apdu`].
pub(crate) fn parse(apdu: &[u8]) -> (Apci, &[u8]) {
    debug_assert!(apdu.len() >= 6);
    let (c1, c2, c3, c4) = (apdu[2], apdu[3], apdu[4], apdu[5]);
    let apci = if c1 & 0x01 == 0 {
        Apci::I {
            send_sn: (c1 as u16) >> 1 | (c2 as u16) << 7,
            recv_sn: (c3 as u16) >> 1 | (c4 as u16) << 7,
        }
    } else if c1 & 0x03 == 0x01 {
        Apci::S {
            recv_sn: (c3 as u16) >> 1 | (c4 as u16) << 7,
        }
    } else {
        Apci::U {
            function: UFunction::from_bits(c1 & 0xfc),
        }
    };
    (apci, &apdu[6..])
}

/// Read one complete APDU into `buf`, returning its total length.
///
/// Resynchronises on the start character: octets before a `0x68` are dropped,
/// as are frames whose length octet is out of range. Returns
/// [`Error::Io`] with kind `UnexpectedEof` when the peer closes the connection.
pub(crate) async fn read_apdu<R>(r: &mut R, buf: &mut [u8; APDU_SIZE_MAX]) -> Result<usize>
where
    R: AsyncRead + Unpin,
{
    loop {
        // Resynchronise: skip everything until a start character.
        loop {
            r.read_exact(&mut buf[..1]).await?;
            if buf[0] == START_FRAME {
                break;
            }
            tracing::trace!(octet = buf[0], "discarding octet while resynchronising");
        }

        r.read_exact(&mut buf[1..2]).await?;
        let field_len = buf[1] as usize;
        if !(APCI_CTL_FIELD_SIZE..=APDU_FIELD_SIZE_MAX).contains(&field_len) {
            tracing::warn!(field_len, "APDU length out of range, resynchronising");
            continue;
        }

        let total = field_len + 2;
        r.read_exact(&mut buf[2..total]).await?;
        return Ok(total);
    }
}

/// Number of frames between `next_ack_no` and `next_seq_no`, accounting for the
/// 15 bit wraparound.
pub(crate) fn seq_no_count(next_ack_no: u16, next_seq_no: u16) -> u16 {
    let seq = if next_ack_no > next_seq_no {
        next_seq_no + 32768
    } else {
        next_seq_no
    };
    seq - next_ack_no
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn i_frame_encodes_both_sequence_numbers() {
        let f = new_i_frame(0, 0, &[1, 2, 3]).unwrap();
        assert_eq!(f, vec![0x68, 7, 0, 0, 0, 0, 1, 2, 3]);
        assert_eq!(parse(&f), (Apci::I { send_sn: 0, recv_sn: 0 }, &[1u8, 2, 3][..]));

        // 15 bit numbers straddle the two octets: value << 1.
        let f = new_i_frame(0x7fff, 0x1234, &[]).unwrap();
        assert_eq!(&f[2..6], &[0xfe, 0xff, 0x68, 0x24]);
        assert_eq!(
            parse(&f).0,
            Apci::I {
                send_sn: 0x7fff,
                recv_sn: 0x1234
            }
        );
    }

    #[test]
    fn i_frame_rejects_an_oversized_asdu() {
        assert_eq!(
            new_i_frame(0, 0, &vec![0; ASDU_SIZE_MAX + 1]),
            Err(Error::LengthOutOfRange)
        );
        assert!(new_i_frame(0, 0, &vec![0; ASDU_SIZE_MAX]).is_ok());
    }

    #[test]
    fn s_and_u_frames_round_trip() {
        let f = new_s_frame(300);
        assert_eq!(parse(&f), (Apci::S { recv_sn: 300 }, &[][..]));

        for func in [
            UFunction::StartDtActive,
            UFunction::StartDtConfirm,
            UFunction::StopDtActive,
            UFunction::StopDtConfirm,
            UFunction::TestFrActive,
            UFunction::TestFrConfirm,
        ] {
            let f = new_u_frame(func);
            assert_eq!(
                parse(&f).0,
                Apci::U {
                    function: Some(func)
                }
            );
        }
    }

    #[test]
    fn start_dt_active_matches_the_standard_octets() {
        assert_eq!(
            new_u_frame(UFunction::StartDtActive),
            [0x68, 0x04, 0x07, 0x00, 0x00, 0x00]
        );
        assert_eq!(
            new_u_frame(UFunction::TestFrConfirm),
            [0x68, 0x04, 0x83, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn undefined_u_functions_decode_to_none() {
        let frame = [0x68, 4, 0x0c | 0x03, 0, 0, 0];
        assert_eq!(parse(&frame).0, Apci::U { function: None });
    }

    #[test]
    fn seq_no_count_handles_wraparound() {
        assert_eq!(seq_no_count(0, 0), 0);
        assert_eq!(seq_no_count(0, 12), 12);
        assert_eq!(seq_no_count(32760, 5), 13);
        assert_eq!(seq_no_count(32767, 0), 1);
    }

    #[tokio::test]
    async fn read_apdu_resynchronises_on_the_start_character() {
        let mut stream: &[u8] = &[
            0xaa, 0xbb, // garbage
            0x68, 0x04, 0x07, 0x00, 0x00, 0x00, // StartDtActive
            0x68, 0x05, 0x00, 0x00, 0x00, 0x00, 0x64, // I-frame, 1 ASDU octet
        ];
        let mut buf = [0u8; APDU_SIZE_MAX];

        let n = read_apdu(&mut stream, &mut buf).await.unwrap();
        assert_eq!(n, 6);
        assert_eq!(
            parse(&buf[..n]).0,
            Apci::U {
                function: Some(UFunction::StartDtActive)
            }
        );

        let n = read_apdu(&mut stream, &mut buf).await.unwrap();
        assert_eq!(n, 7);
        let (apci, asdu) = parse(&buf[..n]);
        assert_eq!(apci, Apci::I { send_sn: 0, recv_sn: 0 });
        assert_eq!(asdu, &[0x64]);
    }

    #[tokio::test]
    async fn read_apdu_skips_frames_with_an_impossible_length() {
        let mut stream: &[u8] = &[
            0x68, 0x02, // length below the control field size
            0x68, 0x04, 0x07, 0x00, 0x00, 0x00,
        ];
        let mut buf = [0u8; APDU_SIZE_MAX];
        // The bad header is dropped and the following valid frame is returned.
        let n = read_apdu(&mut stream, &mut buf).await.unwrap();
        assert_eq!(n, 6);
        assert!(matches!(parse(&buf[..n]).0, Apci::U { .. }));
    }

    #[tokio::test]
    async fn read_apdu_reports_eof() {
        let mut stream: &[u8] = &[0x68, 0x04, 0x07];
        let mut buf = [0u8; APDU_SIZE_MAX];
        assert!(read_apdu(&mut stream, &mut buf).await.is_err());
    }
}
