// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The crate-wide error type.

use std::sync::Arc;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Everything that can go wrong encoding, decoding or transporting an ASDU.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    // -- application layer ------------------------------------------------
    /// The type identification is unknown to this implementation.
    #[error("asdu: type identification unknown")]
    TypeIdentifier,

    /// Cause of transmission 0 is not used.
    #[error("asdu: cause of transmission 0 is not used")]
    CauseZero,

    /// Common address 0 is not used.
    #[error("asdu: common address 0 is not used")]
    CommonAddrZero,

    /// A system parameter is out of the range allowed by the standard.
    #[error("asdu: system parameter out of range")]
    Param,

    /// A time tag could not be interpreted.
    #[error("asdu: invalid time tag")]
    InvalidTimeTag,

    /// An originator address was given although the cause size is 1.
    #[error("asdu: originator address not allowed with cause size 1 system parameter")]
    OriginAddrFit,

    /// The common address does not fit the configured `common_addr_size`.
    #[error("asdu: common address exceeds size system parameter")]
    CommonAddrFit,

    /// The information object address does not fit `info_obj_addr_size`.
    #[error("asdu: information object address exceeds size system parameter")]
    InfoObjAddrFit,

    /// The information object count is not in `[1, 127]`.
    #[error("asdu: information object index not in [1, 127]")]
    InfoObjIndexFit,

    /// An interrogation group number exceeds 16.
    #[error("asdu: interrogation group number exceeds 16")]
    InroGroupNumFit,

    /// The encoded ASDU would exceed the 249 octet maximum.
    #[error("asdu: asdu field length larger than max {}", crate::asdu::ASDU_SIZE_MAX)]
    LengthOutOfRange,

    /// A send helper was called without any information object.
    #[error("asdu: not any object information")]
    NotAnyObjInfo,

    /// The type identification does not match the helper or time tag used.
    #[error("asdu: type identifier doesn't match call or time tag")]
    TypeIdNotMatch,

    /// The cause of transmission is not permitted for this type identification.
    #[error("asdu: cause of transmission for command not standard requirement")]
    CmdCause,

    /// The buffer ended before the ASDU was complete.
    #[error("asdu: unexpected end of information object buffer")]
    UnexpectedEof,

    // -- link and transport layers ----------------------------------------
    /// The connection is closed or was never established.
    #[error("use of closed connection")]
    UseClosedConnection,

    /// The outbound queue is full; back off and retry.
    #[error("buffer is full")]
    BufferFull,

    /// The outbound ASDU or class buffer is full.
    #[error("send queue is full")]
    SendQueueFull,

    /// Data transfer is not active: StartDT has not been confirmed.
    #[error("station is not active")]
    NotActive,

    /// The link layer is not initialized yet.
    #[error("link is not active")]
    LinkNotActive,

    /// A confirmed link frame was not answered within t1, after one repetition.
    #[error("link response timeout t1")]
    TimeoutT1,

    /// An FT1.2 frame failed its checksum or length check.
    #[error("frame: {0}")]
    Frame(&'static str),

    /// The configuration is invalid.
    #[error("config: {0}")]
    Config(&'static str),

    /// The remote endpoint address could not be parsed.
    #[error("invalid endpoint address: {0}")]
    InvalidAddress(String),

    /// An underlying I/O operation failed.
    #[error("io: {0}")]
    Io(#[from] Arc<std::io::Error>),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(Arc::new(e))
    }
}

impl PartialEq for Error {
    /// Compares the error kind. Two [`Error::Io`] values never compare equal,
    /// since the underlying `io::Error` has no meaningful equality.
    fn eq(&self, other: &Self) -> bool {
        use Error::*;
        match (self, other) {
            (Io(_), _) | (_, Io(_)) => false,
            (Frame(a), Frame(b)) => a == b,
            (Config(a), Config(b)) => a == b,
            (InvalidAddress(a), InvalidAddress(b)) => a == b,
            _ => std::mem::discriminant(self) == std::mem::discriminant(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_compares_kinds_and_ignores_io() {
        assert_eq!(Error::Param, Error::Param);
        assert_ne!(Error::Param, Error::CmdCause);
        assert_eq!(Error::Frame("bad checksum"), Error::Frame("bad checksum"));
        assert_ne!(Error::Frame("a"), Error::Frame("b"));
        let io = || Error::from(std::io::Error::other("x"));
        assert_ne!(io(), io());
    }
}
