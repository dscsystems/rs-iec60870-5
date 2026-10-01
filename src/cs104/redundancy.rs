// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Redundancy groups: how the masters connected to one controlled station
//! share its spontaneous data.
//!
//! IEC 60870-5-104, clause 10: the connections of a redundancy group reach
//! the same controlled station, and exactly one of them is in data transfer
//! at a time. The others stand by, stopped, exchanging only test frames, until
//! the controlling side switches over by starting one of them.

use std::collections::VecDeque;
use std::net::IpAddr;
use std::sync::Arc;

use crate::asdu::Asdu;
use crate::cs104::connection::Connection;
use crate::error::{Error, Result};

/// How the masters connected to a [`Server`](crate::cs104::Server) are grouped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ServerMode {
    /// Every connection is a redundancy group of its own. Each master in data
    /// transfer receives every broadcast. This is the default.
    #[default]
    ConnectionIsRedundancyGroup,
    /// All connections form a single redundancy group: one master is in data
    /// transfer and the rest stand by. Starting another connection takes the
    /// previous one out of data transfer.
    SingleRedundancyGroup,
    /// Connections are assigned to the given groups by the master's IP
    /// address; within each group one connection is in data transfer.
    ///
    /// A group that lists no address takes every master no other group names.
    /// A master that matches no group is refused.
    MultipleRedundancyGroups(Vec<RedundancyGroup>),
}

/// One redundancy group of [`ServerMode::MultipleRedundancyGroups`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedundancyGroup {
    name: String,
    clients: Vec<IpAddr>,
}

impl RedundancyGroup {
    /// A group with no addresses yet: until one is added, it takes every
    /// master no other group names.
    pub fn new(name: impl Into<String>) -> RedundancyGroup {
        RedundancyGroup {
            name: name.into(),
            clients: Vec::new(),
        }
    }

    /// Admit the master connecting from `ip` to this group.
    pub fn with_client(mut self, ip: IpAddr) -> RedundancyGroup {
        if !self.clients.contains(&ip) {
            self.clients.push(ip);
        }
        self
    }

    /// The group's name, as used in log lines.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The addresses admitted; empty for the catch-all group.
    pub fn clients(&self) -> &[IpAddr] {
        &self.clients
    }
}

/// The live state of one group.
struct Group {
    name: String,
    members: Vec<Arc<Connection>>,
    /// The member in data transfer, the only one sent spontaneous data.
    active: Option<Arc<Connection>>,
    /// Data broadcast while no member was in data transfer, replayed to the
    /// next one that starts.
    buffer: VecDeque<Asdu>,
    /// Created for a single connection, and gone with it.
    per_connection: bool,
}

impl Group {
    fn new(name: String, per_connection: bool) -> Group {
        Group {
            name,
            members: Vec::new(),
            active: None,
            buffer: VecDeque::new(),
            per_connection,
        }
    }

    fn contains(&self, c: &Arc<Connection>) -> bool {
        self.members.iter().any(|m| Arc::ptr_eq(m, c))
    }
}

/// Where an ASDU went, per group.
pub(crate) enum Route {
    /// Queued on the group's active connection.
    Sent,
    /// Kept in the group's buffer for the next connection to start.
    Buffered,
    /// The active connection refused it.
    Refused(Arc<Connection>, Error),
    /// Neither sent nor kept: no connection is started and the buffer is full.
    Lost(Error),
}

/// The groups of one server, behind the server's lock.
pub(crate) struct Groups {
    mode: ServerMode,
    groups: Vec<Group>,
    buffer_size: usize,
}

impl Groups {
    pub(crate) fn new(mode: ServerMode, buffer_size: usize) -> Groups {
        let groups = match &mode {
            ServerMode::ConnectionIsRedundancyGroup => Vec::new(),
            ServerMode::SingleRedundancyGroup => vec![Group::new("single".into(), false)],
            ServerMode::MultipleRedundancyGroups(gs) => gs
                .iter()
                .map(|g| Group::new(g.name.clone(), false))
                .collect(),
        };
        Groups {
            mode,
            groups,
            buffer_size,
        }
    }

    /// Which group a master connecting from `ip` joins, or `None` when it
    /// belongs to none and must be refused. A group naming the address takes
    /// precedence over the catch-all.
    pub(crate) fn admit(&self, ip: IpAddr) -> Option<usize> {
        match &self.mode {
            ServerMode::ConnectionIsRedundancyGroup => Some(usize::MAX),
            ServerMode::SingleRedundancyGroup => Some(0),
            ServerMode::MultipleRedundancyGroups(gs) => gs
                .iter()
                .position(|g| g.clients.contains(&ip))
                .or_else(|| gs.iter().position(|g| g.clients.is_empty())),
        }
    }

    /// Add a connection to the group chosen by [`Groups::admit`].
    pub(crate) fn join(&mut self, group: usize, c: Arc<Connection>) {
        if group == usize::MAX {
            let name = c
                .peer_addr()
                .map(|p| p.to_string())
                .unwrap_or_else(|| "connection".into());
            let mut g = Group::new(name, true);
            g.members.push(c);
            self.groups.push(g);
        } else if let Some(g) = self.groups.get_mut(group) {
            g.members.push(c);
        }
    }

    /// A connection left: drop it from its group, and drop a group that
    /// existed only for it together with anything it had buffered.
    pub(crate) fn leave(&mut self, c: &Arc<Connection>) {
        for g in &mut self.groups {
            g.members.retain(|m| !Arc::ptr_eq(m, c));
            if g.active.as_ref().is_some_and(|a| Arc::ptr_eq(a, c)) {
                g.active = None;
            }
        }
        self.groups
            .retain(|g| !(g.per_connection && g.members.is_empty()));
    }

    /// A connection received STARTDT. It becomes its group's connection in
    /// data transfer: any other member that was is taken out of it, and the
    /// data buffered meanwhile is replayed to the new one, oldest first.
    pub(crate) fn activated(&mut self, c: &Arc<Connection>) {
        let Some(g) = self.groups.iter_mut().find(|g| g.contains(c)) else {
            return;
        };
        for m in &g.members {
            if !Arc::ptr_eq(m, c) && m.is_active() {
                tracing::warn!(
                    group = %g.name,
                    peer = ?m.peer_addr(),
                    "another connection of the redundancy group started, this one stops"
                );
                m.demote();
            }
        }
        g.active = Some(Arc::clone(c));

        while let Some(a) = g.buffer.front() {
            match c.try_enqueue(a) {
                Ok(()) => {
                    g.buffer.pop_front();
                }
                Err(e) => {
                    // The rest stays buffered and follows the next broadcast.
                    tracing::warn!(group = %g.name, error = %e, left = g.buffer.len(),
                        "could not replay all buffered data");
                    break;
                }
            }
        }
    }

    /// A connection received STOPDT, or was taken out of data transfer.
    pub(crate) fn deactivated(&mut self, c: &Arc<Connection>) {
        for g in &mut self.groups {
            if g.active.as_ref().is_some_and(|a| Arc::ptr_eq(a, c)) {
                g.active = None;
            }
        }
    }

    /// Hand `a` to every group: to its connection in data transfer, or into
    /// its buffer when none is.
    ///
    /// A group with no connection in data transfer and no buffer is skipped:
    /// it is not in data transfer and keeps nothing.
    pub(crate) fn route(&mut self, a: &Asdu) -> Vec<Route> {
        let mut out = Vec::with_capacity(self.groups.len());
        for g in &mut self.groups {
            // A member may have been stopped since it started.
            if g.active.as_ref().is_some_and(|c| !c.is_active()) {
                g.active = None;
            }
            match &g.active {
                Some(c) => out.push(match c.try_enqueue(a) {
                    Ok(()) => Route::Sent,
                    Err(e) => Route::Refused(Arc::clone(c), e),
                }),
                None if self.buffer_size == 0 => {}
                None if g.buffer.len() < self.buffer_size => {
                    g.buffer.push_back(a.clone());
                    out.push(Route::Buffered);
                }
                None => {
                    tracing::warn!(group = %g.name, "event buffer full, ASDU refused");
                    out.push(Route::Lost(Error::SendQueueFull));
                }
            }
        }
        out
    }

    /// How many ASDUs wait in the buffers.
    pub(crate) fn buffered(&self) -> usize {
        self.groups.iter().map(|g| g.buffer.len()).sum()
    }
}

/// Fold the per-group outcome of a broadcast into one result.
///
/// Nothing sent and nothing kept is [`Error::NotActive`]: the data went
/// nowhere. Every group failing is that group's error; some failing is
/// [`Error::PartialBroadcast`].
pub(crate) fn outcome(routes: &[Result<()>]) -> Result<()> {
    let total = routes.len();
    if total == 0 {
        return Err(Error::NotActive);
    }
    let failed: Vec<&Error> = routes.iter().filter_map(|r| r.as_ref().err()).collect();
    match failed.len() {
        0 => Ok(()),
        n if n == total => Err(failed[0].clone()),
        n => Err(Error::PartialBroadcast { failed: n, total }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(10, 0, 0, last))
    }

    #[test]
    fn a_master_joins_the_group_that_names_it_before_the_catch_all() {
        let g = Groups::new(
            ServerMode::MultipleRedundancyGroups(vec![
                RedundancyGroup::new("rest"),
                RedundancyGroup::new("control-centre").with_client(ip(1)).with_client(ip(2)),
            ]),
            0,
        );
        assert_eq!(g.admit(ip(1)), Some(1));
        assert_eq!(g.admit(ip(2)), Some(1));
        assert_eq!(g.admit(ip(9)), Some(0), "the catch-all takes the rest");
    }

    #[test]
    fn a_master_no_group_names_is_refused_without_a_catch_all() {
        let g = Groups::new(
            ServerMode::MultipleRedundancyGroups(vec![
                RedundancyGroup::new("a").with_client(ip(1)),
            ]),
            0,
        );
        assert_eq!(g.admit(ip(1)), Some(0));
        assert_eq!(g.admit(ip(2)), None);
    }

    #[test]
    fn the_outcome_distinguishes_nowhere_all_and_some() {
        assert_eq!(outcome(&[]), Err(Error::NotActive));
        assert_eq!(outcome(&[Ok(()), Ok(())]), Ok(()));
        assert_eq!(
            outcome(&[Err(Error::BufferFull), Err(Error::SendQueueFull)]),
            Err(Error::BufferFull)
        );
        assert_eq!(
            outcome(&[Ok(()), Err(Error::BufferFull)]),
            Err(Error::PartialBroadcast { failed: 1, total: 2 })
        );
    }

    #[test]
    fn a_static_group_buffers_while_no_master_is_connected() {
        // The point of a redundancy group's buffer: events raised while the
        // control centre is away are not lost.
        let mut g = Groups::new(ServerMode::SingleRedundancyGroup, 2);
        let a = Asdu::new_empty(crate::asdu::PARAMS_WIDE);
        assert!(matches!(g.route(&a)[..], [Route::Buffered]));
        assert!(matches!(g.route(&a)[..], [Route::Buffered]));
        assert!(
            matches!(g.route(&a)[..], [Route::Lost(Error::SendQueueFull)]),
            "a full buffer refuses rather than overwrites"
        );
        assert_eq!(g.buffered(), 2);
    }

    #[test]
    fn without_a_buffer_a_group_with_nobody_started_is_skipped() {
        let mut g = Groups::new(ServerMode::SingleRedundancyGroup, 0);
        let a = Asdu::new_empty(crate::asdu::PARAMS_WIDE);
        assert!(g.route(&a).is_empty());
    }
}
