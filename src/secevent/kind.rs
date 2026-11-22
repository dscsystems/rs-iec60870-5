// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

/// Area of protocol activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "snake_case")
)]
pub enum Category {
    /// Access events.
    Access,
    /// Connection events.
    Connection,
    /// Control events.
    Control,
    /// FileTransfer events.
    #[cfg_attr(feature = "serde", serde(rename = "file"))]
    FileTransfer,
    /// Flood events.
    Flood,
    /// Meta events.
    Meta,
    /// Peer events.
    Peer,
    /// Protocol events.
    Protocol,
    /// Resource events.
    Resource,
    /// Sequence events.
    Sequence,
    /// TLS events.
    Tls,
    /// Unsolicited events.
    Unsolicited,
}

/// Importance assigned to an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize),
    serde(rename_all = "snake_case")
)]
pub enum Severity {
    /// Info events.
    Info,
    /// Notice events.
    Notice,
    /// Warning events.
    Warning,
    /// Alert events.
    Alert,
    /// Critical events.
    Critical,
}

/// Default metadata for a built-in event.
#[derive(Debug, Clone, Copy)]
pub struct KindInfo {
    /// Event category.
    pub category: Category,
    /// Default severity.
    pub severity: Severity,
    /// MITRE ATT&CK for ICS technique hint.
    pub technique: Option<&'static str>,
    /// Whether each occurrence is delivered separately.
    pub always: bool,
}

macro_rules! kinds {
    ($($name:ident, $wire:literal, $category:ident, $severity:ident, $technique:expr, $always:literal;)*) => {
        /// Stable identifier for a security event.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[non_exhaustive]
        pub enum Kind {
            $(#[doc = $wire] $name,)*
        }

        impl Kind {
            /// All built-in event kinds.
            pub const ALL: &'static [Self] = &[$(Self::$name,)*];

            /// Stable dotted identifier used in event sinks.
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$name => $wire,)* }
            }

            /// Get the event's classification.
            pub const fn info(self) -> KindInfo {
                match self {
                    $(Self::$name => KindInfo {
                        category: Category::$category, severity: Severity::$severity,
                        technique: $technique, always: $always,
                    },)*
                }
            }
        }
    };
}

kinds! {
    ConnAccepted, "conn.accepted", Connection, Info, None, false;
    ConnUnexpected, "conn.unexpected", Connection, Warning, Some("T0846"), true;
    ConnUnexpectedMac, "conn.unexpected_mac", Connection, Alert, Some("T0849"), true;
    ConnRejectedLimit, "conn.rejected_limit", Connection, Warning, Some("T0814"), false;
    ConnRejectedBusy, "conn.rejected_busy", Connection, Notice, None, false;
    ConnRate, "conn.rate", Connection, Alert, Some("T0814"), false;
    ConnNoData, "conn.no_data", Connection, Notice, Some("T0846"), false;
    ConnNeverStarted, "conn.never_started", Connection, Warning, Some("T0814"), false;
    ConnClosed, "conn.closed", Connection, Info, None, false;
    ConnT1Timeout, "conn.t1_timeout", Connection, Notice, None, false;
    ConnDialFailed, "conn.dial_failed", Connection, Notice, None, false;
    ConnAcceptError, "conn.accept_error", Resource, Critical, Some("T0814"), true;
    ConnSecondActive, "conn.redundancy_second_active", Connection, Notice, None, false;
    TlsCertRejected, "tls.cert_rejected", Tls, Alert, Some("T0849"), true;
    TlsNoClientCert, "tls.no_client_cert", Tls, Alert, Some("T0849"), true;
    TlsLocalCertRejected, "tls.local_cert_rejected", Tls, Warning, Some("T0830"), true;
    TlsPlaintext, "tls.plaintext_on_tls_port", Tls, Warning, None, false;
    TlsNonTlsProbe, "tls.non_tls_probe", Tls, Notice, Some("T0846"), false;
    TlsHandshakeFailed, "tls.handshake_failed", Tls, Warning, None, false;
    TlsHandshakeTo, "tls.handshake_timeout", Tls, Warning, Some("T0814"), false;
    TlsWeakParams, "tls.weak_params", Tls, Notice, None, false;
    TlsCertExpiring, "tls.cert_expiring", Tls, Notice, None, false;
    TlsCertChanged, "tls.cert_changed", Tls, Notice, Some("T0830"), true;
    PeerMacChanged, "peer.mac_changed", Peer, Alert, Some("T0830"), true;
    PeerMacChangedLive, "peer.mac_changed_midsession", Peer, Critical, Some("T0830"), true;
    PeerMacPinning, "peer.mac_pinning_violation", Peer, Critical, Some("T0849"), true;
    PeerIpChanged, "peer.ip_changed", Peer, Info, None, false;
    PeerMacShared, "peer.mac_shared", Peer, Notice, Some("T0830"), false;
    PeerMacUnavailable, "peer.mac_unavailable", Peer, Info, None, false;
    AccessUnknownCa, "access.unknown_ca", Access, Warning, Some("T0861"), false;
    AccessUnknownIoa, "access.unknown_ioa", Access, Warning, Some("T0861"), false;
    AccessUnknownType, "access.unknown_type", Access, Warning, Some("T0861"), false;
    AccessBroadcastMisuse, "access.broadcast_misuse", Access, Alert, Some("T0855"), false;
    AccessScan, "access.scan", Access, Alert, Some("T0861"), false;
    AccessTypeBlocked, "access.type_blocked", Access, Warning, Some("T0855"), false;
    AccessOriginator, "access.originator_unexpected", Access, Notice, None, false;
    AccessOversized, "access.oversized_request", Access, Warning, Some("T0814"), false;
    LinkForeignAddress, "link.foreign_address", Access, Notice, None, false;
    LinkUnexpectedAddr, "link.unexpected_address", Access, Warning, Some("T0849"), false;
    LinkBroadcastReply, "link.broadcast_reply_requested", Access, Warning, None, false;
    LinkWrongPrm, "link.wrong_prm", Access, Warning, None, false;
    ProtoGarbage, "proto.garbage", Protocol, Notice, None, false;
    ProtoBadLength, "proto.bad_length", Protocol, Warning, Some("T0866"), false;
    ProtoBadApci, "proto.bad_apci", Protocol, Warning, Some("T0866"), false;
    ProtoIWhileStopped, "proto.i_while_stopped", Protocol, Warning, Some("T0855"), false;
    ProtoAsduInvalid, "proto.asdu_invalid", Protocol, Warning, Some("T0866"), false;
    ProtoFieldInvalid, "proto.field_invalid", Protocol, Warning, Some("T0866"), false;
    ProtoUnknownCot, "proto.unknown_cot", Protocol, Warning, None, false;
    ProtoWrongDirection, "proto.wrong_direction", Protocol, Warning, Some("T0856"), false;
    ProtoUWrongDir, "proto.u_wrong_direction", Protocol, Warning, None, false;
    ProtoNotInProfile, "proto.type_not_in_profile", Protocol, Notice, None, false;
    ProtoSecureAuth, "proto.secure_auth_type", Protocol, Notice, None, false;
    ProtoCs101Frame, "proto.cs101_frame", Protocol, Warning, None, false;
    ProtoCs101Function, "proto.cs101_function", Protocol, Warning, None, false;
    ProtoCs101Fcv, "proto.cs101_fcv", Protocol, Notice, None, false;
    ProtoCs103Unknown, "proto.cs103_unknown", Protocol, Notice, None, false;
    CtrlStartDt, "ctrl.startdt", Control, Info, None, false;
    CtrlStopDt, "ctrl.stopdt", Control, Info, None, false;
    CtrlResetProcess, "ctrl.reset_process", Control, Alert, Some("T0816"), true;
    CtrlClockSync, "ctrl.clock_sync", Control, Notice, None, false;
    CtrlClockStep, "ctrl.clock_step", Control, Alert, Some("T0855"), true;
    CtrlTestCommand, "ctrl.test_command", Control, Info, None, false;
    CtrlDelayAcq, "ctrl.delay_acquisition", Control, Info, None, false;
    CtrlParamChange, "ctrl.parameter_change", Control, Alert, Some("T0836"), true;
    CtrlParamActivate, "ctrl.parameter_activation", Control, Alert, Some("T0836"), true;
    CtrlInterrogation, "ctrl.interrogation", Control, Info, None, false;
    CtrlEndOfInit, "ctrl.end_of_init", Control, Notice, Some("T0816"), true;
    CtrlLinkReset, "ctrl.link_reset", Control, Notice, None, false;
    CtrlCs103Setting, "ctrl.cs103_setting_change", Control, Alert, Some("T0836"), true;
    CmdReceived, "cmd.received", Control, Notice, Some("T0855"), true;
    CmdSelectNoExec, "cmd.select_without_execute", Control, Warning, Some("T0855"), false;
    CmdExecNoSelect, "cmd.execute_without_select", Control, Warning, Some("T0855"), false;
    CmdRejected, "cmd.rejected", Control, Notice, Some("T0855"), false;
    CmdBruteForce, "cmd.brute_force", Control, Alert, Some("T0806"), false;
    CmdStaleTimeTag, "cmd.stale_timetag", Control, Alert, Some("T0855"), true;
    CmdDuplicate, "cmd.duplicate", Control, Warning, Some("T0855"), true;
    RemoteRejected, "remote.rejected", Control, Notice, None, false;
    SeqNsDuplicate, "seq.ns_duplicate", Sequence, Alert, Some("T0830"), true;
    SeqNsGap, "seq.ns_gap", Sequence, Alert, Some("T0830"), true;
    SeqNsBehind, "seq.ns_behind", Sequence, Alert, Some("T0830"), true;
    SeqNrAhead, "seq.nr_ahead", Sequence, Alert, Some("T0830"), true;
    SeqNrRegressed, "seq.nr_regressed", Sequence, Warning, Some("T0830"), true;
    SeqFcbRepeat, "seq.fcb_repeat", Sequence, Notice, None, false;
    UnsolUConfirm, "unsol.u_confirm", Unsolicited, Warning, Some("T0856"), false;
    UnsolActResponse, "unsol.act_response", Unsolicited, Warning, Some("T0856"), false;
    UnsolInterrogated, "unsol.interrogated", Unsolicited, Notice, Some("T0856"), false;
    UnsolLinkConfirm, "unsol.link_confirm", Unsolicited, Notice, None, false;
    UnsolFile, "unsol.file", Unsolicited, Warning, None, false;
    FileDirectoryCall, "file.directory_call", FileTransfer, Info, Some("T0846"), false;
    FileUpload, "file.upload", FileTransfer, Notice, Some("T0845"), true;
    FileDownloadAttempt, "file.download_attempt", FileTransfer, Alert, Some("T0843"), true;
    FileRejected, "file.rejected", FileTransfer, Warning, None, false;
    FileChecksum, "file.checksum", FileTransfer, Warning, None, false;
    FileOversized, "file.oversized", FileTransfer, Warning, Some("T0814"), false;
    FileUnsupported, "file.unsupported", FileTransfer, Notice, None, false;
    FloodCommands, "flood.commands", Flood, Alert, Some("T0806"), false;
    FloodInterrogation, "flood.interrogation", Flood, Warning, Some("T0814"), false;
    FloodCounterInterr, "flood.counter_interrogation", Flood, Warning, Some("T0814"), false;
    FloodRead, "flood.read", Flood, Warning, Some("T0814"), false;
    FloodClockSync, "flood.clock_sync", Flood, Warning, Some("T0814"), false;
    FloodResetProcess, "flood.reset_process", Flood, Alert, Some("T0816"), false;
    FloodTestFr, "flood.testfr", Flood, Warning, Some("T0814"), false;
    FloodUToggle, "flood.u_toggle", Flood, Warning, Some("T0814"), false;
    FloodSFrames, "flood.s_frames", Flood, Warning, Some("T0814"), false;
    FloodFrames, "flood.frames", Flood, Alert, Some("T0814"), false;
    FloodCs101Requests, "flood.cs101_requests", Flood, Warning, Some("T0814"), false;
    DosRxBacklog, "dos.rx_backlog", Resource, Warning, Some("T0814"), false;
    DosTxBacklog, "dos.tx_backlog", Resource, Warning, Some("T0814"), false;
    DosWindowStall, "dos.window_stall", Resource, Warning, Some("T0814"), false;
    DosWriteStall, "dos.write_stall", Resource, Alert, Some("T0814"), true;
    DosSlowFrame, "dos.slow_frame", Resource, Alert, Some("T0814"), true;
    MetaEventsDropped, "meta.events_dropped", Meta, Warning, None, true;
    MetaSinkPanic, "meta.sink_panic", Meta, Alert, None, true;
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for Kind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
