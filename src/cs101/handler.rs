// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Application handler traits for the cs101 endpoints.
//!
//! Every method has a default implementation that does nothing, so an
//! application only writes the ones it needs. Both traits receive a
//! `&dyn Connect` for the endpoint, so a handler can reply or buffer data
//! without holding a reference back to the station.

use chrono::{DateTime, Utc};

use crate::asdu::{
    Asdu, Connect, InfoObjAddr, QualifierCountCall, QualifierOfInterrogation,
    QualifierOfResetProcessCmd,
};
use crate::error::Result;

/// Handles requests arriving at a secondary station (outstation / slave).
///
/// Control commands and everything without a dedicated method arrive at
/// [`ServerHandler::asdu`]. Replies are buffered with
/// [`Connect::send`] and handed to the primary when it polls; in unbalanced
/// mode the standard forbids unsolicited transmission.
#[async_trait::async_trait]
pub trait ServerHandler: Send + Sync + 'static {
    /// `C_IC_NA_1`: general or group interrogation.
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

    /// `C_RD_NA_1`: read one information object.
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
    async fn asdu(&self, conn: &dyn Connect, pack: &Asdu) -> Result<()> {
        let (_, _) = (conn, pack);
        Ok(())
    }

    /// Called for every inbound ASDU before routing, for logging or forwarding.
    async fn asdu_all(&self, conn: &dyn Connect, pack: &Asdu, server_number: usize) -> Result<()> {
        let (_, _, _) = (conn, pack, server_number);
        Ok(())
    }

    /// The link became active: the primary reset it and it is now usable.
    async fn on_link_active(&self, conn: &dyn Connect) {
        let _ = conn;
    }

    /// The transport went down.
    async fn on_connection_lost(&self, conn: &dyn Connect) {
        let _ = conn;
    }
}

/// Handles ASDUs arriving at a primary station (master).
///
/// Routing follows the cause of transmission first and the type identification
/// second: interrogation-caused monitor data reaches
/// [`ClientHandler::interrogation`], counter-request-caused `M_IT_*` reaches
/// [`ClientHandler::counter_interrogation`], the mirrored confirmations of the
/// system commands reach their matching method, and everything else —
/// spontaneous data, command confirmations, end of initialization — reaches
/// [`ClientHandler::asdu`].
#[async_trait::async_trait]
pub trait ClientHandler: Send + Sync + 'static {
    /// Interrogation responses, and the mirrored `C_IC_NA_1` confirmations.
    async fn interrogation(&self, conn: &dyn Connect, pack: &Asdu) -> Result<()> {
        let (_, _) = (conn, pack);
        Ok(())
    }

    /// Counter responses, and the mirrored `C_CI_NA_1` confirmations.
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

    /// Spontaneous data, command confirmations and everything else.
    async fn asdu(&self, conn: &dyn Connect, pack: &Asdu, client_number: usize) -> Result<()> {
        let (_, _, _) = (conn, pack, client_number);
        Ok(())
    }

    /// Called for every inbound ASDU before routing.
    async fn asdu_all(&self, conn: &dyn Connect, pack: &Asdu, client_number: usize) -> Result<()> {
        let (_, _, _) = (conn, pack, client_number);
        Ok(())
    }

    /// The line came alive: the first frame of this connection was received.
    async fn on_connect(&self, conn: &dyn Connect) {
        let _ = conn;
    }

    /// A secondary station's link became active after reset.
    async fn on_link_active(&self, conn: &dyn Connect, link_addr: u16) {
        let (_, _) = (conn, link_addr);
    }

    /// The transport went down.
    async fn on_connection_lost(&self, conn: &dyn Connect) {
        let _ = conn;
    }
}

/// Route one received ASDU to the matching [`ClientHandler`] method.
pub(crate) async fn dispatch_client<H: ClientHandler>(
    handler: &H,
    conn: &dyn Connect,
    pack: &Asdu,
    client_number: usize,
) {
    use crate::asdu::TypeId;

    tracing::debug!(asdu = %pack, "RX ASDU");
    if let Err(e) = handler.asdu_all(conn, pack, client_number).await {
        tracing::warn!(error = %e, "asdu_all handler failed");
    }

    let cause = pack.coa().cause;
    let type_id = pack.type_id();

    let r = match type_id {
        // Mirrored confirmations of the system commands.
        TypeId::C_IC_NA_1 => handler.interrogation(conn, pack).await,
        TypeId::C_CI_NA_1 => handler.counter_interrogation(conn, pack).await,
        TypeId::C_RD_NA_1 => handler.read(conn, pack).await,
        TypeId::C_TS_NA_1 => handler.test_command(conn, pack).await,
        TypeId::C_CS_NA_1 => handler.clock_sync(conn, pack).await,
        TypeId::C_RP_NA_1 => handler.reset_process(conn, pack).await,
        TypeId::C_CD_NA_1 => handler.delay_acquisition(conn, pack).await,

        // Monitor data, routed by cause.
        _ if cause.is_interrogation() && is_monitor_type(type_id) => {
            handler.interrogation(conn, pack).await
        }
        _ if cause.is_counter_request() && is_counter_type(type_id) => {
            handler.counter_interrogation(conn, pack).await
        }
        _ => handler.asdu(conn, pack, client_number).await,
    };
    if let Err(e) = r {
        tracing::warn!(error = %e, "client handler failed");
    }
}

/// True for the process information types in the monitoring direction.
fn is_monitor_type(t: crate::asdu::TypeId) -> bool {
    use crate::asdu::TypeId;
    (t >= TypeId::M_SP_NA_1 && t <= TypeId::M_ME_ND_1)
        || (t >= TypeId::M_SP_TB_1 && t <= TypeId::M_EP_TF_1)
}

/// True for the integrated total types.
fn is_counter_type(t: crate::asdu::TypeId) -> bool {
    use crate::asdu::TypeId;
    t == TypeId::M_IT_NA_1 || t == TypeId::M_IT_TA_1 || t == TypeId::M_IT_TB_1
}

/// Route one received ASDU to the matching [`ServerHandler`] method.
pub(crate) async fn dispatch_server<H: ServerHandler>(
    handler: &H,
    conn: &dyn Connect,
    pack: &Asdu,
    server_number: usize,
) {
    use crate::asdu::TypeId;

    tracing::debug!(asdu = %pack, "RX ASDU");
    if let Err(e) = handler.asdu_all(conn, pack, server_number).await {
        tracing::warn!(error = %e, "asdu_all handler failed");
    }

    let r = async {
        match pack.type_id() {
            TypeId::C_IC_NA_1 => {
                let (_, qoi) = pack.get_interrogation_cmd()?;
                handler.interrogation(conn, pack, qoi).await
            }
            TypeId::C_CI_NA_1 => {
                let (_, qcc) = pack.get_counter_interrogation_cmd()?;
                handler.counter_interrogation(conn, pack, qcc).await
            }
            TypeId::C_RD_NA_1 => {
                let ioa = pack.get_read_cmd()?;
                handler.read(conn, pack, ioa).await
            }
            TypeId::C_CS_NA_1 => {
                let (_, t) = pack.get_clock_synchronization_cmd()?;
                handler.clock_sync(conn, pack, t).await
            }
            TypeId::C_RP_NA_1 => {
                let (_, qrp) = pack.get_reset_process_cmd()?;
                handler.reset_process(conn, pack, qrp).await
            }
            TypeId::C_CD_NA_1 => {
                let (_, msec) = pack.get_delay_acquire_command()?;
                handler.delay_acquisition(conn, pack, msec).await
            }
            _ => handler.asdu(conn, pack).await,
        }
    }
    .await;

    if let Err(e) = r {
        tracing::warn!(error = %e, "server handler failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::TypeId;

    #[test]
    fn monitor_types_cover_both_standard_ranges() {
        assert!(is_monitor_type(TypeId::M_SP_NA_1));
        assert!(is_monitor_type(TypeId::M_ME_ND_1));
        assert!(is_monitor_type(TypeId::M_SP_TB_1));
        assert!(is_monitor_type(TypeId::M_EP_TF_1));
        assert!(!is_monitor_type(TypeId::S_IT_TC_1));
        assert!(!is_monitor_type(TypeId::C_SC_NA_1));
        assert!(!is_monitor_type(TypeId::M_EI_NA_1));
    }

    #[test]
    fn counter_types_are_the_integrated_totals() {
        assert!(is_counter_type(TypeId::M_IT_NA_1));
        assert!(is_counter_type(TypeId::M_IT_TA_1));
        assert!(is_counter_type(TypeId::M_IT_TB_1));
        assert!(!is_counter_type(TypeId::M_ME_NC_1));
    }
}
