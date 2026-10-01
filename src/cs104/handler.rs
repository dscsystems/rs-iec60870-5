// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Application handler traits for the cs104 endpoints, and the routing that
//! decides which method a received ASDU reaches.
//!
//! Every method has a default implementation that does nothing, so an
//! application only writes the ones it cares about.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::asdu::{
    Asdu, Cause, Connect, InfoObjAddr, QualifierCountCall, QualifierOfInterrogation,
    QualifierOfResetProcessCmd, TypeId, INVALID_COMMON_ADDR, INFO_OBJ_ADDR_IRRELEVANT,
};
use crate::cs104::connection::{Connection, Dispatcher};
use crate::error::Result;

/// Handles requests arriving at a controlled station (server / outstation).
///
/// The session pre-validates the cause of transmission, the common address and
/// the information object address of the dedicated request types, and answers
/// `UnknownCOT` / `UnknownCA` / `UnknownIOA` mirrors on violations, so these
/// methods only ever see well-formed requests.
///
/// Control commands (`C_SC`, `C_DC`, `C_RC`, `C_SE`, `C_BO`) and parameter
/// commands arrive at [`ServerHandler::asdu`]. Returning an error from it makes
/// the session reply with an `UnknownTypeID` mirror.
#[async_trait::async_trait]
pub trait ServerHandler: Send + Sync + 'static {
    /// `C_IC_NA_1`: general or group interrogation.
    ///
    /// Confirm, send the process image with an interrogation cause, then
    /// terminate:
    ///
    /// ```no_run
    /// # use rs_iec60870_5::asdu::*;
    /// # async fn h(c: &dyn Connect, pack: &Asdu, qoi: QualifierOfInterrogation) -> rs_iec60870_5::Result<()> {
    /// if qoi != QualifierOfInterrogation::STATION {
    ///     return c.send(pack.reply_mirror(Cause::ACTIVATION_CON).negated()).await;
    /// }
    /// c.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
    /// c.send_single(
    ///     false,
    ///     CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION),
    ///     pack.common_addr(),
    ///     &[SinglePointInfo::new(100, true)],
    /// )
    /// .await?;
    /// c.send(pack.reply_mirror(Cause::ACTIVATION_TERM)).await
    /// # }
    /// ```
    async fn interrogation(
        &self,
        conn: &dyn Connect,
        pack: &Asdu,
        qoi: QualifierOfInterrogation,
    ) -> Result<()> {
        let (_, _, _) = (conn, pack, qoi);
        Ok(())
    }

    /// `C_CI_NA_1`: counter interrogation.
    async fn counter_interrogation(
        &self,
        conn: &dyn Connect,
        pack: &Asdu,
        qcc: QualifierCountCall,
    ) -> Result<()> {
        let (_, _, _) = (conn, pack, qcc);
        Ok(())
    }

    /// `C_RD_NA_1`: read the value of a single information object.
    async fn read(&self, conn: &dyn Connect, pack: &Asdu, ioa: InfoObjAddr) -> Result<()> {
        let (_, _, _) = (conn, pack, ioa);
        Ok(())
    }

    /// `C_CS_NA_1`: clock synchronization.
    async fn clock_sync(
        &self,
        conn: &dyn Connect,
        pack: &Asdu,
        time: Option<DateTime<Utc>>,
    ) -> Result<()> {
        let (_, _, _) = (conn, pack, time);
        Ok(())
    }

    /// `C_RP_NA_1`: reset process.
    async fn reset_process(
        &self,
        conn: &dyn Connect,
        pack: &Asdu,
        qrp: QualifierOfResetProcessCmd,
    ) -> Result<()> {
        let (_, _, _) = (conn, pack, qrp);
        Ok(())
    }

    /// `C_CD_NA_1`: delay acquisition, with the delay in milliseconds.
    async fn delay_acquisition(&self, conn: &dyn Connect, pack: &Asdu, msec: u16) -> Result<()> {
        let (_, _, _) = (conn, pack, msec);
        Ok(())
    }

    /// Everything else, including all control and parameter commands.
    ///
    /// Returning an error makes the session reply with an `UnknownTypeID` mirror.
    async fn asdu(&self, conn: &dyn Connect, pack: &Asdu) -> Result<()> {
        let (_, _) = (conn, pack);
        Ok(())
    }

    /// Called for *every* inbound ASDU before routing, for logging or
    /// forwarding. Its error does not stop the dispatch.
    async fn asdu_all(&self, conn: &dyn Connect, pack: &Asdu, server_number: usize) -> Result<()> {
        let (_, _, _) = (conn, pack, server_number);
        Ok(())
    }

    /// A master connected (controlled-station side).
    async fn on_connect(&self, conn: &dyn Connect) {
        let _ = conn;
    }

    /// A master disconnected.
    async fn on_connection_lost(&self, conn: &dyn Connect) {
        let _ = conn;
    }
}

/// Context handed to a master's handler, for data-forwarding gateways.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientContext {
    /// Index of this client, set with `Client::set_client_number`.
    pub client_number: usize,
}

/// Handles ASDUs arriving at a controlling station (master / client).
///
/// **Note the routing.** The dedicated methods receive the *mirrored command
/// confirmations* — the echoed `C_*` ASDUs with `ActivationCon` or
/// `ActivationTerm`. The actual process data (`M_SP`, `M_ME`, `M_IT`, …),
/// **including interrogation responses**, arrives at [`ClientHandler::asdu`].
#[async_trait::async_trait]
pub trait ClientHandler: Send + Sync + 'static {
    /// `C_IC_NA_1` confirmations.
    async fn interrogation(&self, conn: &dyn Connect, pack: &Asdu) -> Result<()> {
        let (_, _) = (conn, pack);
        Ok(())
    }

    /// `C_CI_NA_1` confirmations.
    async fn counter_interrogation(&self, conn: &dyn Connect, pack: &Asdu) -> Result<()> {
        let (_, _) = (conn, pack);
        Ok(())
    }

    /// `C_RD_NA_1` confirmations.
    async fn read(&self, conn: &dyn Connect, pack: &Asdu) -> Result<()> {
        let (_, _) = (conn, pack);
        Ok(())
    }

    /// `C_TS_NA_1` confirmations.
    async fn test_command(&self, conn: &dyn Connect, pack: &Asdu) -> Result<()> {
        let (_, _) = (conn, pack);
        Ok(())
    }

    /// `C_CS_NA_1` confirmations.
    async fn clock_sync(&self, conn: &dyn Connect, pack: &Asdu) -> Result<()> {
        let (_, _) = (conn, pack);
        Ok(())
    }

    /// `C_RP_NA_1` confirmations.
    async fn reset_process(&self, conn: &dyn Connect, pack: &Asdu) -> Result<()> {
        let (_, _) = (conn, pack);
        Ok(())
    }

    /// `C_CD_NA_1` confirmations.
    async fn delay_acquisition(&self, conn: &dyn Connect, pack: &Asdu) -> Result<()> {
        let (_, _) = (conn, pack);
        Ok(())
    }

    /// All process data and every other type: this is where `M_*` lands.
    async fn asdu(&self, conn: &dyn Connect, pack: &Asdu) -> Result<()> {
        let (_, _) = (conn, pack);
        Ok(())
    }

    /// Called for *every* inbound ASDU before routing.
    async fn asdu_all(&self, conn: &dyn Connect, pack: &Asdu, ctx: &ClientContext) -> Result<()> {
        let (_, _, _) = (conn, pack, ctx);
        Ok(())
    }

    /// The TCP connection came up, before `STARTDT` is confirmed.
    async fn on_connect(&self, conn: &dyn Connect) {
        let _ = conn;
    }

    /// `STARTDT` was confirmed: data transfer is active.
    async fn on_activated(&self, conn: &dyn Connect) {
        let _ = conn;
    }

    /// `STOPDT` was confirmed: data transfer is no longer active.
    async fn on_deactivated(&self, conn: &dyn Connect) {
        let _ = conn;
    }

    /// The connection went down.
    async fn on_connection_lost(&self, conn: &dyn Connect) {
        let _ = conn;
    }
}

/// Routes ASDUs received by a controlled station to a [`ServerHandler`].
pub(crate) struct ServerDispatcher<H: ServerHandler> {
    pub handler: Arc<H>,
    pub server_number: usize,
}

#[async_trait::async_trait]
impl<H: ServerHandler> Dispatcher for ServerDispatcher<H> {
    async fn dispatch(&self, conn: &Arc<Connection>, pack: Asdu) {
        tracing::debug!(asdu = %pack, "RX ASDU");
        let c: &dyn Connect = conn.as_ref();

        if let Err(e) = self.handler.asdu_all(c, &pack, self.server_number).await {
            tracing::warn!(error = %e, "asdu_all handler failed");
        }

        if let Err(e) = self.route(c, &pack).await {
            tracing::warn!(error = %e, "server handler failed");
        }
    }
}

impl<H: ServerHandler> ServerDispatcher<H> {
    /// Check the cause of transmission and common address of a dedicated
    /// request, replying with the matching protocol error mirror when either is
    /// wrong. `Ok(false)` means the request was already answered.
    ///
    /// This runs *before* the information object is decoded: a request with a
    /// cause the type does not permit earns an `UnknownCOT` mirror whether or
    /// not its payload also happens to be malformed, and a decode error must
    /// not swallow that answer.
    async fn precheck_header(
        &self,
        conn: &dyn Connect,
        pack: &Asdu,
        cause_ok: bool,
    ) -> Result<bool> {
        if !cause_ok {
            conn.send(pack.reply_mirror(Cause::UNKNOWN_COT)).await?;
            return Ok(false);
        }
        if pack.common_addr() == INVALID_COMMON_ADDR {
            conn.send(pack.reply_mirror(Cause::UNKNOWN_CA)).await?;
            return Ok(false);
        }
        Ok(true)
    }

    /// Check the information object address of a request that must carry the
    /// irrelevant one, replying `UnknownIOA` when it does not.
    async fn precheck_ioa(
        &self,
        conn: &dyn Connect,
        pack: &Asdu,
        ioa: InfoObjAddr,
    ) -> Result<bool> {
        if ioa != INFO_OBJ_ADDR_IRRELEVANT {
            conn.send(pack.reply_mirror(Cause::UNKNOWN_IOA)).await?;
            return Ok(false);
        }
        Ok(true)
    }

    async fn route(&self, conn: &dyn Connect, pack: &Asdu) -> Result<()> {
        let cause = pack.coa().cause;
        match pack.type_id() {
            TypeId::C_IC_NA_1 => {
                let ok = cause == Cause::ACTIVATION || cause == Cause::DEACTIVATION;
                if self.precheck_header(conn, pack, ok).await? {
                    let (ioa, qoi) = pack.get_interrogation_cmd()?;
                    if self.precheck_ioa(conn, pack, ioa).await? {
                        self.handler.interrogation(conn, pack, qoi).await?;
                    }
                }
            }
            TypeId::C_CI_NA_1 => {
                let ok = cause == Cause::ACTIVATION;
                if self.precheck_header(conn, pack, ok).await? {
                    let (ioa, qcc) = pack.get_counter_interrogation_cmd()?;
                    if self.precheck_ioa(conn, pack, ioa).await? {
                        self.handler.counter_interrogation(conn, pack, qcc).await?;
                    }
                }
            }
            TypeId::C_RD_NA_1 => {
                let ok = cause == Cause::REQUEST;
                if self.precheck_header(conn, pack, ok).await? {
                    // A read command names a real point, so the IOA is not checked.
                    let ioa = pack.get_read_cmd()?;
                    self.handler.read(conn, pack, ioa).await?;
                }
            }
            TypeId::C_CS_NA_1 => {
                let ok = cause == Cause::ACTIVATION;
                if self.precheck_header(conn, pack, ok).await? {
                    let (ioa, time) = pack.get_clock_synchronization_cmd()?;
                    if self.precheck_ioa(conn, pack, ioa).await? {
                        self.handler.clock_sync(conn, pack, time).await?;
                    }
                }
            }
            TypeId::C_TS_NA_1 => {
                let ok = cause == Cause::ACTIVATION;
                if self.precheck_header(conn, pack, ok).await? {
                    let (ioa, _) = pack.get_test_command()?;
                    if self.precheck_ioa(conn, pack, ioa).await? {
                        // Test commands are confirmed by the session itself.
                        conn.send(pack.reply_mirror(Cause::ACTIVATION_CON)).await?;
                    }
                }
            }
            TypeId::C_RP_NA_1 => {
                let ok = cause == Cause::ACTIVATION;
                if self.precheck_header(conn, pack, ok).await? {
                    let (ioa, qrp) = pack.get_reset_process_cmd()?;
                    if self.precheck_ioa(conn, pack, ioa).await? {
                        self.handler.reset_process(conn, pack, qrp).await?;
                    }
                }
            }
            TypeId::C_CD_NA_1 => {
                let ok = cause == Cause::ACTIVATION || cause == Cause::SPONTANEOUS;
                if self.precheck_header(conn, pack, ok).await? {
                    let (ioa, msec) = pack.get_delay_acquire_command()?;
                    if self.precheck_ioa(conn, pack, ioa).await? {
                        self.handler.delay_acquisition(conn, pack, msec).await?;
                    }
                }
            }
            _ => {
                if self.handler.asdu(conn, pack).await.is_err() {
                    conn.send(pack.reply_mirror(Cause::UNKNOWN_TYPE_ID)).await?;
                }
            }
        }
        Ok(())
    }
}

/// Routes ASDUs received by a master to a [`ClientHandler`].
pub(crate) struct ClientDispatcher<H: ClientHandler> {
    pub handler: Arc<H>,
    pub ctx: ClientContext,
}

#[async_trait::async_trait]
impl<H: ClientHandler> Dispatcher for ClientDispatcher<H> {
    async fn dispatch(&self, conn: &Arc<Connection>, pack: Asdu) {
        tracing::debug!(asdu = %pack, "RX ASDU");
        let c: &dyn Connect = conn.as_ref();

        if let Err(e) = self.handler.asdu_all(c, &pack, &self.ctx).await {
            tracing::warn!(error = %e, "asdu_all handler failed");
        }

        // On the master side the dedicated methods receive command
        // confirmations; process data goes to `asdu`.
        let r = match pack.type_id() {
            TypeId::C_IC_NA_1 => self.handler.interrogation(c, &pack).await,
            TypeId::C_CI_NA_1 => self.handler.counter_interrogation(c, &pack).await,
            TypeId::C_RD_NA_1 => self.handler.read(c, &pack).await,
            TypeId::C_CS_NA_1 => self.handler.clock_sync(c, &pack).await,
            TypeId::C_TS_NA_1 => self.handler.test_command(c, &pack).await,
            TypeId::C_RP_NA_1 => self.handler.reset_process(c, &pack).await,
            TypeId::C_CD_NA_1 => self.handler.delay_acquisition(c, &pack).await,
            _ => self.handler.asdu(c, &pack).await,
        };
        if let Err(e) = r {
            tracing::warn!(error = %e, "client handler failed");
        }
    }
}
