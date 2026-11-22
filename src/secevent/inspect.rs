// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

#[cfg(feature = "cs104")]
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::asdu::{Asdu, Cause, RequestError, TypeId};
#[cfg(feature = "cs104")]
use crate::asdu::{CommonAddr, InfoObjAddr};

#[cfg(feature = "cs104")]
use super::Severity;
use super::{Action, Buckets, Event, Kind, Session, UnknownObjects, over_limit};

#[derive(Default)]
pub(crate) struct Inspector {
    buckets: Buckets,
    unknown: UnknownObjects,
}

pub(crate) enum Decision {
    Deliver,
    Reply(Cause),
    Disconnect,
}

fn asdu_event(a: &Asdu, raw: &[u8], kind: Kind, action: Action, detail: &str) -> Event {
    let mut event = Event::new(kind, action, detail).with_asdu(a);
    event.raw = raw.to_vec();
    event
}

fn profile(session: &Session, a: &Asdu, raw: &[u8]) {
    let t = a.type_id();
    let event = if t.is_security() {
        Some((Kind::ProtoSecureAuth, "IEC 62351-5 authentication type"))
    } else if session.protocol == "iec104"
        && !t.in_profile_104()
        && !(t.0 >= 128 && session.policy.allow_private_types)
    {
        Some((
            Kind::ProtoNotInProfile,
            "type is outside the IEC 104 profile",
        ))
    } else {
        None
    };
    if let Some((kind, detail)) = event {
        session.emit(|| asdu_event(a, raw, kind, Action::Observed, detail));
    }
}

fn confirmation(cause: Cause) -> bool {
    matches!(
        cause,
        Cause::ACTIVATION_CON | Cause::DEACTIVATION_CON | Cause::ACTIVATION_TERM
    )
}

fn refusal(a: &Asdu) -> Cause {
    if !a.type_id().is_control_direction() || a.coa().cause == Cause::REQUEST {
        Cause::UNKNOWN_TYPE_ID
    } else if a.coa().cause == Cause::DEACTIVATION {
        Cause::DEACTIVATION_CON
    } else {
        Cause::ACTIVATION_CON
    }
}

impl Decision {
    pub(crate) fn reply(a: &Asdu, cause: Cause) -> Asdu {
        let reply = a.reply_mirror(cause);
        if confirmation(cause) {
            reply.negated()
        } else {
            reply
        }
    }
}

impl Inspector {
    pub(crate) fn inspect(&mut self, session: &Session, a: &Asdu, raw: &[u8]) -> Decision {
        let policy = &session.policy;
        let t = a.type_id();
        let enabled = session.security.enabled();
        let emit = |kind, action, detail| session.emit(|| asdu_event(a, raw, kind, action, detail));
        if enabled {
            profile(session, a, raw);
            if t.is_file() && t.is_monitor_direction() {
                emit(
                    Kind::FileDownloadAttempt,
                    Action::Observed,
                    "peer is pushing a file",
                );
            } else if t.is_monitor_direction()
                || (t.is_control_direction() && confirmation(a.coa().cause))
            {
                emit(
                    Kind::ProtoWrongDirection,
                    Action::Observed,
                    "unexpected ASDU direction at the controlled station",
                );
            }
            if let Err(error) = a.validate_request() {
                let kind = match error {
                    RequestError::SingleObjectExpected => Kind::AccessOversized,
                    RequestError::BroadcastNotAllowed => Kind::AccessBroadcastMisuse,
                    _ => Kind::ProtoFieldInvalid,
                };
                session.emit(|| asdu_event(a, raw, kind, Action::Observed, &error.to_string()));
            }
            if t.is_control_direction()
                && !policy.allowed_originators.is_empty()
                && !policy.allowed_originators.contains(&a.orig_addr())
            {
                emit(
                    Kind::AccessOriginator,
                    Action::Observed,
                    "unexpected originator address",
                );
            }
            if !policy.disable_audit {
                audit(session, a, raw);
            }
        }

        if let Some(space) = policy
            .address_space
            .as_ref()
            .filter(|_| t.is_control_direction() && !a.is_global())
        {
            let unknown = if !space.known_ca(a.common_addr()) {
                Some((
                    Kind::AccessUnknownCa,
                    Cause::UNKNOWN_CA,
                    "unknown common address",
                ))
            } else if (!t.is_system_command() || t == TypeId::C_RD_NA_1)
                && a.first_ioa()
                    .is_some_and(|ioa| !space.known_ioa(a.common_addr(), t, ioa))
            {
                Some((
                    Kind::AccessUnknownIoa,
                    Cause::UNKNOWN_IOA,
                    "unknown information object address",
                ))
            } else {
                None
            };
            if let Some((kind, cause, detail)) = unknown {
                let action = if policy.auto_reject_unknown {
                    Action::Rejected
                } else {
                    Action::Observed
                };
                emit(kind, action, detail);
                self.note_unknown(session, a);
                if policy.auto_reject_unknown {
                    return Decision::Reply(cause);
                }
            }
        }

        if policy.functions.denies(a) {
            emit(
                Kind::AccessTypeBlocked,
                Action::Rejected,
                "function denied by policy",
            );
            let cause = if policy.functions.deny_with_unknown_type {
                Cause::UNKNOWN_TYPE_ID
            } else {
                refusal(a)
            };
            return Decision::Reply(cause);
        }

        if (!policy.max_command_age.is_zero() || !policy.max_command_future.is_zero())
            && let Some(time) = a.command_time_tag()
        {
            let delta = chrono::Utc::now() - time;
            if exceeds(delta, policy.max_command_age) || exceeds(-delta, policy.max_command_future)
            {
                let action = if policy.reject_stale_commands {
                    Action::Rejected
                } else {
                    Action::Observed
                };
                emit(
                    Kind::CmdStaleTimeTag,
                    action,
                    "command time tag is outside the configured window",
                );
                if policy.reject_stale_commands {
                    return Decision::Reply(refusal(a));
                }
            }
        }
        if enabled
            && !policy.duplicate_window.is_zero()
            && (t.is_process_command() || t.is_parameter() || t == TypeId::C_RP_NA_1)
            && session.security.seen_command(a, policy.duplicate_window)
        {
            emit(
                Kind::CmdDuplicate,
                Action::Observed,
                "identical command within the duplicate window",
            );
        }

        let action = policy.flood_action();
        if !enabled && action == Action::Observed {
            return Decision::Deliver;
        }
        let limits = &policy.limits;
        let (kind, rate) = match t {
            TypeId::C_IC_NA_1 => (Kind::FloodInterrogation, limits.interrogation),
            TypeId::C_CI_NA_1 => (Kind::FloodCounterInterr, limits.counter_interrogation),
            TypeId::C_RD_NA_1 => (Kind::FloodRead, limits.read),
            TypeId::C_CS_NA_1 => (Kind::FloodClockSync, limits.clock_sync),
            TypeId::C_RP_NA_1 => (Kind::FloodResetProcess, limits.reset_process),
            _ if t.is_process_command() => (Kind::FloodCommands, limits.commands),
            _ => return Decision::Deliver,
        };
        if over_limit(&mut self.buckets, kind, rate) {
            emit(kind, action, "request rate exceeded");
            return match action {
                Action::Throttled => Decision::Reply(refusal(a)),
                Action::Disconnected => Decision::Disconnect,
                _ => Decision::Deliver,
            };
        }
        Decision::Deliver
    }

    fn note_unknown(&mut self, session: &Session, a: &Asdu) {
        if !session.security.enabled()
            || session.policy.scan_threshold == 0
            || session.policy.scan_window.is_zero()
        {
            return;
        }
        let now = Instant::now();
        let window = session.policy.scan_window;
        let threshold = session.policy.scan_threshold;
        self.unknown
            .retain(|_, seen| now.duration_since(*seen) < window);
        let key = (a.type_id(), a.common_addr(), a.first_ioa());
        if self.unknown.insert(key, now).is_none() && self.unknown.len() == threshold {
            session.emit(|| {
                Event::new(
                    Kind::AccessScan,
                    Action::Observed,
                    "many distinct unknown objects requested",
                )
                .with_asdu(a)
            });
        }
        if self.unknown.len() > threshold
            && let Some(oldest) = self
                .unknown
                .iter()
                .min_by_key(|(_, seen)| *seen)
                .map(|(k, _)| *k)
        {
            self.unknown.remove(&oldest);
        }
    }

    #[cfg(feature = "cs104")]
    pub(crate) fn outbound(&mut self, session: &Session, a: &Asdu) {
        if !session.security.enabled() {
            return;
        }
        let (kind, detail) = match a.coa().cause {
            Cause::UNKNOWN_TYPE_ID => (Kind::AccessUnknownType, "unknown type refused"),
            Cause::UNKNOWN_COT => (
                Kind::ProtoUnknownCot,
                "unknown cause of transmission refused",
            ),
            Cause::UNKNOWN_CA => (Kind::AccessUnknownCa, "unknown common address refused"),
            Cause::UNKNOWN_IOA => (Kind::AccessUnknownIoa, "unknown information object refused"),
            Cause::ACTIVATION_CON | Cause::DEACTIVATION_CON
                if a.coa().is_negative && a.type_id().is_control_direction() =>
            {
                (Kind::CmdRejected, "negative command confirmation sent")
            }
            _ => return,
        };
        session.emit(|| Event::new(kind, Action::Rejected, detail).with_asdu(a));
        match kind {
            Kind::AccessUnknownType | Kind::AccessUnknownCa | Kind::AccessUnknownIoa => {
                self.note_unknown(session, a)
            }
            Kind::CmdRejected
                if over_limit(
                    &mut self.buckets,
                    Kind::CmdBruteForce,
                    super::Rate::new(10, Duration::from_secs(60)),
                ) =>
            {
                session.emit(|| {
                    Event::new(
                        Kind::CmdBruteForce,
                        Action::Observed,
                        "repeated command refusals",
                    )
                    .with_asdu(a)
                });
            }
            _ => {}
        }
    }
}

fn exceeds(delta: chrono::TimeDelta, threshold: Duration) -> bool {
    !threshold.is_zero() && delta.to_std().is_ok_and(|elapsed| elapsed > threshold)
}

fn audit(session: &Session, a: &Asdu, raw: &[u8]) {
    let kind = match a.type_id() {
        TypeId::C_IC_NA_1 | TypeId::C_CI_NA_1 => Kind::CtrlInterrogation,
        TypeId::C_RP_NA_1 => Kind::CtrlResetProcess,
        TypeId::C_CS_NA_1 => Kind::CtrlClockSync,
        TypeId::C_TS_NA_1 | TypeId::C_TS_TA_1 => Kind::CtrlTestCommand,
        TypeId::C_CD_NA_1 => Kind::CtrlDelayAcq,
        TypeId::P_AC_NA_1 => Kind::CtrlParamActivate,
        TypeId::F_SC_NA_1 if a.coa().cause == Cause::REQUEST => Kind::FileDirectoryCall,
        TypeId::F_SC_NA_1 => Kind::FileUpload,
        t if t.is_parameter() && a.coa().cause == Cause::ACTIVATION => Kind::CtrlParamChange,
        t if t.is_process_command()
            && matches!(a.coa().cause, Cause::ACTIVATION | Cause::DEACTIVATION) =>
        {
            Kind::CmdReceived
        }
        _ => return,
    };
    session.emit(|| {
        let mut event = asdu_event(a, raw, kind, Action::Observed, "control function received");
        if kind == Kind::CmdReceived {
            event.value = command_value(a);
        }
        event
    });
    if kind == Kind::CtrlClockSync
        && let Ok((_, Some(time))) = a.get_clock_synchronization_cmd()
    {
        let delta = (time - chrono::Utc::now()).abs();
        if exceeds(delta, session.policy.clock_step_threshold) {
            session.emit(|| {
                asdu_event(
                    a,
                    raw,
                    Kind::CtrlClockStep,
                    Action::Observed,
                    "requested clock adjustment exceeds threshold",
                )
            });
        }
    }
}

fn command_value(a: &Asdu) -> Option<String> {
    let mut reader = a.reader();
    reader.info_obj_addr().ok()?;
    let t = if a.type_id().is_time_tagged_command() {
        a.type_id().0 - 13
    } else {
        a.type_id().0
    };
    match TypeId(t) {
        TypeId::C_SC_NA_1 => reader
            .byte()
            .ok()
            .map(|b| format!("{} value={}", mode(b), b & 1 != 0)),
        TypeId::C_DC_NA_1 | TypeId::C_RC_NA_1 => reader
            .byte()
            .ok()
            .map(|b| format!("{} value={}", mode(b), b & 3)),
        TypeId::C_SE_NA_1 | TypeId::C_SE_NB_1 => {
            let value = reader.scaled().ok()?;
            Some(format!("{} value={value}", mode(reader.byte().ok()?)))
        }
        TypeId::C_SE_NC_1 => {
            let value = reader.f32().ok()?;
            Some(format!("{} value={value}", mode(reader.byte().ok()?)))
        }
        TypeId::C_BO_NA_1 => reader
            .bits_string32()
            .ok()
            .map(|v| format!("value=0x{v:08x}")),
        _ => None,
    }
}

fn mode(b: u8) -> &'static str {
    if b & 0x80 == 0 { "Execute" } else { "Select" }
}

#[cfg(feature = "cs104")]
type RequestKey = (TypeId, CommonAddr, Option<InfoObjAddr>);

#[cfg(feature = "cs104")]
#[derive(Default)]
pub(crate) struct Requests {
    pending: HashMap<RequestKey, (Instant, bool)>,
}

#[cfg(feature = "cs104")]
impl Requests {
    fn prune(&mut self, now: Instant) {
        self.pending.retain(|_, (expiry, _)| now < *expiry);
    }

    pub(crate) fn sent(&mut self, a: &Asdu) {
        if !a.type_id().is_control_direction() {
            return;
        }
        let now = Instant::now();
        self.prune(now);
        let key = (a.type_id(), a.common_addr(), a.first_ioa());
        if !self.pending.contains_key(&key)
            && self.pending.len() >= 4096
            && let Some(oldest) = self
                .pending
                .iter()
                .min_by_key(|(_, (expiry, _))| *expiry)
                .map(|(k, _)| *k)
        {
            self.pending.remove(&oldest);
        }
        self.pending
            .insert(key, (now + Duration::from_secs(120), a.is_global()));
    }

    pub(crate) fn inspect(&mut self, session: &Session, a: &Asdu, raw: &[u8]) {
        if !session.security.enabled() {
            return;
        }
        let t = a.type_id();
        let coa = a.coa();
        let emit =
            |kind, detail| session.emit(|| asdu_event(a, raw, kind, Action::Observed, detail));
        profile(session, a, raw);
        let confirming = confirmation(coa.cause);
        let unknown = matches!(coa.cause.0, 44..=47);
        if t.is_control_direction() && !confirming && !unknown {
            emit(
                Kind::ProtoWrongDirection,
                "unexpected control-direction ASDU at the controlling station",
            );
        }
        if unknown || (confirming && coa.is_negative) {
            emit(
                Kind::RemoteRejected,
                "the controlled station refused the request",
            );
        }
        let now = Instant::now();
        self.prune(now);
        if t.is_control_direction() && (confirming || unknown) {
            let key = (t, a.common_addr(), a.first_ioa());
            let exact = self.pending.get_mut(&key);
            if let Some((expiry, global)) = exact {
                if !*global && (unknown || coa.is_negative || coa.cause == Cause::ACTIVATION_TERM) {
                    *expiry = now + Duration::from_secs(2);
                }
            } else if confirming
                && !self
                    .pending
                    .iter()
                    .any(|((kind, _, ioa), (_, global))| *global && *kind == t && *ioa == key.2)
            {
                emit(
                    Kind::UnsolActResponse,
                    "confirmation without a matching outstanding request",
                );
            }
        }
        if t.is_monitor_direction() {
            let request = if coa.cause.is_interrogation() {
                Some(TypeId::C_IC_NA_1)
            } else if coa.cause.is_counter_request() {
                Some(TypeId::C_CI_NA_1)
            } else if coa.cause == Cause::REQUEST {
                Some(TypeId::C_RD_NA_1)
            } else {
                None
            };
            if let Some(request) = request
                && !self.pending.iter().any(|((kind, ca, _), (_, global))| {
                    *kind == request && (*global || *ca == a.common_addr())
                })
            {
                emit(
                    Kind::UnsolInterrogated,
                    "requested data without a matching outstanding request",
                );
            }
        }
        if t == TypeId::M_EI_NA_1 && !session.policy.disable_audit {
            session.emit(|| {
                let mut event = asdu_event(
                    a,
                    raw,
                    Kind::CtrlEndOfInit,
                    Action::Observed,
                    "controlled station reported end of initialisation",
                );
                if a.info_obj
                    .get(a.params.info_obj_addr_size as usize)
                    .is_some_and(|b| b & 0x7f == 2)
                {
                    event.severity = Severity::Warning;
                }
                event
            });
        }
    }
}
