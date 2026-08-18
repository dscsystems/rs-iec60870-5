// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The [`Connect`] abstraction shared by every endpoint, and the [`ConnectExt`]
//! convenience methods that build and send an ASDU in one call.

use chrono::{DateTime, Utc};

use crate::asdu::codec::Asdu;
use crate::asdu::cpara::*;
use crate::asdu::cproc::*;
use crate::asdu::identifier::{CauseOfTransmission, CommonAddr, TypeId};
use crate::asdu::info::*;
use crate::asdu::mproc::*;
use crate::asdu::params::Params;
use crate::error::Result;

/// Anything an ASDU can be sent through: a cs104 client, server or session, or
/// a cs101 client or server.
///
/// Sends are queued rather than blocking. [`crate::Error::BufferFull`] and
/// [`crate::Error::SendQueueFull`] mean "back off and retry", not "dropped".
#[async_trait::async_trait]
pub trait Connect: Send + Sync {
    /// The system parameters this endpoint encodes and decodes with.
    fn params(&self) -> Params;

    /// Queue an ASDU for transmission.
    async fn send(&self, a: Asdu) -> Result<()>;

    /// The remote address, when the endpoint has a single one.
    fn peer_addr(&self) -> Option<std::net::SocketAddr> {
        None
    }
}

#[async_trait::async_trait]
impl<T: Connect + ?Sized> Connect for &T {
    fn params(&self) -> Params {
        (**self).params()
    }

    async fn send(&self, a: Asdu) -> Result<()> {
        (**self).send(a).await
    }

    fn peer_addr(&self) -> Option<std::net::SocketAddr> {
        (**self).peer_addr()
    }
}

#[async_trait::async_trait]
impl<T: Connect + ?Sized> Connect for std::sync::Arc<T> {
    fn params(&self) -> Params {
        (**self).params()
    }

    async fn send(&self, a: Asdu) -> Result<()> {
        (**self).send(a).await
    }

    fn peer_addr(&self) -> Option<std::net::SocketAddr> {
        (**self).peer_addr()
    }
}

/// Build-and-send helpers, one per ASDU family.
///
/// Blanket-implemented for every [`Connect`], including `&dyn Connect`, so a
/// handler can reply through the connection it was given:
///
/// ```no_run
/// # use rs_iec60870_5::asdu::*;
/// # async fn f(c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
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
#[async_trait::async_trait]
pub trait ConnectExt: Connect {
    /// Reply to `request` with the same payload and a different cause.
    async fn send_reply_mirror(
        &self,
        request: &Asdu,
        cause: crate::asdu::identifier::Cause,
    ) -> Result<()> {
        self.send(request.reply_mirror(cause)).await
    }

    // -- monitor direction ------------------------------------------------

    /// Send `M_SP_NA_1`: single-point information.
    async fn send_single(
        &self,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[SinglePointInfo],
    ) -> Result<()> {
        self.send(Asdu::single(self.params(), is_sequence, coa, ca, infos)?)
            .await
    }

    /// Send `M_SP_TA_1`: single-point information with a CP24Time2a time tag.
    async fn send_single_cp24time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[SinglePointInfo],
    ) -> Result<()> {
        self.send(Asdu::single_cp24time2a(self.params(), coa, ca, infos)?)
            .await
    }

    /// Send `M_SP_TB_1`: single-point information with a CP56Time2a time tag.
    async fn send_single_cp56time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[SinglePointInfo],
    ) -> Result<()> {
        self.send(Asdu::single_cp56time2a(self.params(), coa, ca, infos)?)
            .await
    }

    /// Send `M_DP_NA_1`: double-point information.
    async fn send_double(
        &self,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[DoublePointInfo],
    ) -> Result<()> {
        self.send(Asdu::double(self.params(), is_sequence, coa, ca, infos)?)
            .await
    }

    /// Send `M_DP_TA_1`: double-point information with a CP24Time2a time tag.
    async fn send_double_cp24time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[DoublePointInfo],
    ) -> Result<()> {
        self.send(Asdu::double_cp24time2a(self.params(), coa, ca, infos)?)
            .await
    }

    /// Send `M_DP_TB_1`: double-point information with a CP56Time2a time tag.
    async fn send_double_cp56time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[DoublePointInfo],
    ) -> Result<()> {
        self.send(Asdu::double_cp56time2a(self.params(), coa, ca, infos)?)
            .await
    }

    /// Send `M_ST_NA_1`: step position information.
    async fn send_step(
        &self,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[StepPositionInfo],
    ) -> Result<()> {
        self.send(Asdu::step(self.params(), is_sequence, coa, ca, infos)?)
            .await
    }

    /// Send `M_ST_TA_1`: step position information with a CP24Time2a time tag.
    async fn send_step_cp24time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[StepPositionInfo],
    ) -> Result<()> {
        self.send(Asdu::step_cp24time2a(self.params(), coa, ca, infos)?)
            .await
    }

    /// Send `M_ST_TB_1`: step position information with a CP56Time2a time tag.
    async fn send_step_cp56time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[StepPositionInfo],
    ) -> Result<()> {
        self.send(Asdu::step_cp56time2a(self.params(), coa, ca, infos)?)
            .await
    }

    /// Send `M_BO_NA_1`: bit string of 32 bits.
    async fn send_bitstring32(
        &self,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BitString32Info],
    ) -> Result<()> {
        self.send(Asdu::bitstring32(self.params(), is_sequence, coa, ca, infos)?)
            .await
    }

    /// Send `M_BO_TA_1`: bit string of 32 bits with a CP24Time2a time tag.
    async fn send_bitstring32_cp24time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BitString32Info],
    ) -> Result<()> {
        self.send(Asdu::bitstring32_cp24time2a(self.params(), coa, ca, infos)?)
            .await
    }

    /// Send `M_BO_TB_1`: bit string of 32 bits with a CP56Time2a time tag.
    async fn send_bitstring32_cp56time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BitString32Info],
    ) -> Result<()> {
        self.send(Asdu::bitstring32_cp56time2a(self.params(), coa, ca, infos)?)
            .await
    }

    /// Send `M_ME_NA_1`: normalized measured value.
    async fn send_measured_value_normal(
        &self,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueNormalInfo],
    ) -> Result<()> {
        self.send(Asdu::measured_value_normal(
            self.params(),
            is_sequence,
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_ME_TA_1`: normalized measured value with a CP24Time2a time tag.
    async fn send_measured_value_normal_cp24time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueNormalInfo],
    ) -> Result<()> {
        self.send(Asdu::measured_value_normal_cp24time2a(
            self.params(),
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_ME_TD_1`: normalized measured value with a CP56Time2a time tag.
    async fn send_measured_value_normal_cp56time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueNormalInfo],
    ) -> Result<()> {
        self.send(Asdu::measured_value_normal_cp56time2a(
            self.params(),
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_ME_ND_1`: normalized measured value without quality descriptor.
    async fn send_measured_value_normal_no_quality(
        &self,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueNormalInfo],
    ) -> Result<()> {
        self.send(Asdu::measured_value_normal_no_quality(
            self.params(),
            is_sequence,
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_ME_NB_1`: scaled measured value.
    async fn send_measured_value_scaled(
        &self,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueScaledInfo],
    ) -> Result<()> {
        self.send(Asdu::measured_value_scaled(
            self.params(),
            is_sequence,
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_ME_TB_1`: scaled measured value with a CP24Time2a time tag.
    async fn send_measured_value_scaled_cp24time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueScaledInfo],
    ) -> Result<()> {
        self.send(Asdu::measured_value_scaled_cp24time2a(
            self.params(),
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_ME_TE_1`: scaled measured value with a CP56Time2a time tag.
    async fn send_measured_value_scaled_cp56time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueScaledInfo],
    ) -> Result<()> {
        self.send(Asdu::measured_value_scaled_cp56time2a(
            self.params(),
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_ME_NC_1`: short floating point measured value.
    async fn send_measured_value_float(
        &self,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueFloatInfo],
    ) -> Result<()> {
        self.send(Asdu::measured_value_float(
            self.params(),
            is_sequence,
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_ME_TC_1`: short float measured value with a CP24Time2a time tag.
    async fn send_measured_value_float_cp24time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueFloatInfo],
    ) -> Result<()> {
        self.send(Asdu::measured_value_float_cp24time2a(
            self.params(),
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_ME_TF_1`: short float measured value with a CP56Time2a time tag.
    async fn send_measured_value_float_cp56time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueFloatInfo],
    ) -> Result<()> {
        self.send(Asdu::measured_value_float_cp56time2a(
            self.params(),
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_IT_NA_1`: integrated totals.
    async fn send_integrated_totals(
        &self,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BinaryCounterReadingInfo],
    ) -> Result<()> {
        self.send(Asdu::integrated_totals(
            self.params(),
            is_sequence,
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_IT_TA_1`: integrated totals with a CP24Time2a time tag.
    async fn send_integrated_totals_cp24time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BinaryCounterReadingInfo],
    ) -> Result<()> {
        self.send(Asdu::integrated_totals_cp24time2a(
            self.params(),
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_IT_TB_1`: integrated totals with a CP56Time2a time tag.
    async fn send_integrated_totals_cp56time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BinaryCounterReadingInfo],
    ) -> Result<()> {
        self.send(Asdu::integrated_totals_cp56time2a(
            self.params(),
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_EP_TA_1`: event of protection equipment, CP24Time2a.
    async fn send_event_of_protection_equipment_cp24time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[EventOfProtectionEquipmentInfo],
    ) -> Result<()> {
        self.send(Asdu::event_of_protection_equipment_cp24time2a(
            self.params(),
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_EP_TD_1`: event of protection equipment, CP56Time2a.
    async fn send_event_of_protection_equipment_cp56time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[EventOfProtectionEquipmentInfo],
    ) -> Result<()> {
        self.send(Asdu::event_of_protection_equipment_cp56time2a(
            self.params(),
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_EP_TB_1`: packed start events of protection equipment, CP24Time2a.
    async fn send_packed_start_events_cp24time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: PackedStartEventsOfProtectionEquipmentInfo,
    ) -> Result<()> {
        self.send(Asdu::packed_start_events_cp24time2a(
            self.params(),
            coa,
            ca,
            info,
        )?)
        .await
    }

    /// Send `M_EP_TE_1`: packed start events of protection equipment, CP56Time2a.
    async fn send_packed_start_events_cp56time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: PackedStartEventsOfProtectionEquipmentInfo,
    ) -> Result<()> {
        self.send(Asdu::packed_start_events_cp56time2a(
            self.params(),
            coa,
            ca,
            info,
        )?)
        .await
    }

    /// Send `M_EP_TC_1`: packed output circuit information, CP24Time2a.
    async fn send_packed_output_circuit_info_cp24time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: PackedOutputCircuitInfoInfo,
    ) -> Result<()> {
        self.send(Asdu::packed_output_circuit_info_cp24time2a(
            self.params(),
            coa,
            ca,
            info,
        )?)
        .await
    }

    /// Send `M_EP_TF_1`: packed output circuit information, CP56Time2a.
    async fn send_packed_output_circuit_info_cp56time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: PackedOutputCircuitInfoInfo,
    ) -> Result<()> {
        self.send(Asdu::packed_output_circuit_info_cp56time2a(
            self.params(),
            coa,
            ca,
            info,
        )?)
        .await
    }

    /// Send `M_PS_NA_1`: packed single points with status change detection.
    async fn send_packed_single_point_with_scd(
        &self,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[PackedSinglePointWithScdInfo],
    ) -> Result<()> {
        self.send(Asdu::packed_single_point_with_scd(
            self.params(),
            is_sequence,
            coa,
            ca,
            infos,
        )?)
        .await
    }

    /// Send `M_EI_NA_1`: end of initialization.
    async fn send_end_of_initialization(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        ioa: InfoObjAddr,
        coi: CauseOfInitial,
    ) -> Result<()> {
        self.send(Asdu::end_of_initialization(
            self.params(),
            coa,
            ca,
            ioa,
            coi,
        )?)
        .await
    }

    // -- control direction ------------------------------------------------

    /// Send `C_SC_NA_1` or `C_SC_TA_1`: a single command.
    async fn send_single_cmd(
        &self,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: SingleCommandInfo,
    ) -> Result<()> {
        self.send(Asdu::single_cmd(self.params(), type_id, coa, ca, cmd)?)
            .await
    }

    /// Send `C_DC_NA_1` or `C_DC_TA_1`: a double command.
    async fn send_double_cmd(
        &self,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: DoubleCommandInfo,
    ) -> Result<()> {
        self.send(Asdu::double_cmd(self.params(), type_id, coa, ca, cmd)?)
            .await
    }

    /// Send `C_RC_NA_1` or `C_RC_TA_1`: a regulating step command.
    async fn send_step_cmd(
        &self,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: StepCommandInfo,
    ) -> Result<()> {
        self.send(Asdu::step_cmd(self.params(), type_id, coa, ca, cmd)?)
            .await
    }

    /// Send `C_SE_NA_1` or `C_SE_TA_1`: a normalized set-point command.
    async fn send_setpoint_cmd_normal(
        &self,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: SetpointCommandNormalInfo,
    ) -> Result<()> {
        self.send(Asdu::setpoint_cmd_normal(
            self.params(),
            type_id,
            coa,
            ca,
            cmd,
        )?)
        .await
    }

    /// Send `C_SE_NB_1` or `C_SE_TB_1`: a scaled set-point command.
    async fn send_setpoint_cmd_scaled(
        &self,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: SetpointCommandScaledInfo,
    ) -> Result<()> {
        self.send(Asdu::setpoint_cmd_scaled(
            self.params(),
            type_id,
            coa,
            ca,
            cmd,
        )?)
        .await
    }

    /// Send `C_SE_NC_1` or `C_SE_TC_1`: a short float set-point command.
    async fn send_setpoint_cmd_float(
        &self,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: SetpointCommandFloatInfo,
    ) -> Result<()> {
        self.send(Asdu::setpoint_cmd_float(
            self.params(),
            type_id,
            coa,
            ca,
            cmd,
        )?)
        .await
    }

    /// Send `C_BO_NA_1` or `C_BO_TA_1`: a 32 bit string command.
    async fn send_bits_string32_cmd(
        &self,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        cmd: BitsString32CommandInfo,
    ) -> Result<()> {
        self.send(Asdu::bits_string32_cmd(
            self.params(),
            type_id,
            coa,
            ca,
            cmd,
        )?)
        .await
    }

    // -- system information -----------------------------------------------

    /// Send `C_IC_NA_1`: an interrogation command.
    async fn send_interrogation_cmd(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        qoi: QualifierOfInterrogation,
    ) -> Result<()> {
        self.send(Asdu::interrogation_cmd(self.params(), coa, ca, qoi)?)
            .await
    }

    /// Send `C_CI_NA_1`: a counter interrogation command.
    async fn send_counter_interrogation_cmd(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        qcc: QualifierCountCall,
    ) -> Result<()> {
        self.send(Asdu::counter_interrogation_cmd(self.params(), coa, ca, qcc)?)
            .await
    }

    /// Send `C_RD_NA_1`: a read command.
    async fn send_read_cmd(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        ioa: InfoObjAddr,
    ) -> Result<()> {
        self.send(Asdu::read_cmd(self.params(), coa, ca, ioa)?)
            .await
    }

    /// Send `C_CS_NA_1`: a clock synchronization command.
    async fn send_clock_synchronization_cmd(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        t: DateTime<Utc>,
    ) -> Result<()> {
        self.send(Asdu::clock_synchronization_cmd(self.params(), coa, ca, t)?)
            .await
    }

    /// Send `C_TS_NA_1`: a test command.
    async fn send_test_command(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
    ) -> Result<()> {
        self.send(Asdu::test_command(self.params(), coa, ca)?).await
    }

    /// Send `C_TS_TA_1`: a test command with a CP56Time2a time tag.
    async fn send_test_command_cp56time2a(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        t: DateTime<Utc>,
    ) -> Result<()> {
        self.send(Asdu::test_command_cp56time2a(self.params(), coa, ca, t)?)
            .await
    }

    /// Send `C_RP_NA_1`: a reset process command.
    async fn send_reset_process_cmd(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        qrp: QualifierOfResetProcessCmd,
    ) -> Result<()> {
        self.send(Asdu::reset_process_cmd(self.params(), coa, ca, qrp)?)
            .await
    }

    /// Send `C_CD_NA_1`: a delay acquisition command (IEC 60870-5-101 only).
    async fn send_delay_acquire_command(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        msec: u16,
    ) -> Result<()> {
        self.send(Asdu::delay_acquire_command(self.params(), coa, ca, msec)?)
            .await
    }

    // -- parameters ---------------------------------------------------------

    /// Send `P_ME_NA_1`: a normalized measured value parameter.
    async fn send_parameter_normal(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        p: ParameterNormalInfo,
    ) -> Result<()> {
        self.send(Asdu::parameter_normal(self.params(), coa, ca, p)?)
            .await
    }

    /// Send `P_ME_NB_1`: a scaled measured value parameter.
    async fn send_parameter_scaled(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        p: ParameterScaledInfo,
    ) -> Result<()> {
        self.send(Asdu::parameter_scaled(self.params(), coa, ca, p)?)
            .await
    }

    /// Send `P_ME_NC_1`: a short float measured value parameter.
    async fn send_parameter_float(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        p: ParameterFloatInfo,
    ) -> Result<()> {
        self.send(Asdu::parameter_float(self.params(), coa, ca, p)?)
            .await
    }

    /// Send `P_AC_NA_1`: a parameter activation.
    async fn send_parameter_activation(
        &self,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        p: ParameterActivationInfo,
    ) -> Result<()> {
        self.send(Asdu::parameter_activation(self.params(), coa, ca, p)?)
            .await
    }
}

impl<T: Connect + ?Sized> ConnectExt for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::identifier::Cause;
    use crate::asdu::params::PARAMS_WIDE;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        sent: Mutex<Vec<Asdu>>,
    }

    #[async_trait::async_trait]
    impl Connect for Recorder {
        fn params(&self) -> Params {
            PARAMS_WIDE
        }

        async fn send(&self, a: Asdu) -> Result<()> {
            self.sent.lock().unwrap().push(a);
            Ok(())
        }
    }

    #[tokio::test]
    async fn ext_helpers_build_and_queue_through_a_dyn_connect() {
        let rec = Recorder::default();
        let c: &dyn Connect = &rec;

        c.send_single(
            false,
            CauseOfTransmission::new(Cause::SPONTANEOUS),
            1,
            &[SinglePointInfo::new(100, true)],
        )
        .await
        .unwrap();
        c.send_interrogation_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION),
            1,
            QualifierOfInterrogation::STATION,
        )
        .await
        .unwrap();

        let sent = rec.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].type_id(), TypeId::M_SP_NA_1);
        assert_eq!(sent[1].type_id(), TypeId::C_IC_NA_1);
    }

    #[tokio::test]
    async fn ext_helpers_propagate_build_errors_without_sending() {
        let rec = Recorder::default();
        let err = rec
            .send_single(
                false,
                CauseOfTransmission::new(Cause::ACTIVATION),
                1,
                &[SinglePointInfo::new(1, true)],
            )
            .await
            .unwrap_err();
        assert_eq!(err, crate::Error::CmdCause);
        assert!(rec.sent.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn reply_mirror_helper_echoes_the_request() {
        let rec = Recorder::default();
        let req = Asdu::interrogation_cmd(
            PARAMS_WIDE,
            CauseOfTransmission::new(Cause::ACTIVATION),
            7,
            QualifierOfInterrogation::STATION,
        )
        .unwrap();
        rec.send_reply_mirror(&req, Cause::ACTIVATION_CON)
            .await
            .unwrap();
        let sent = rec.sent.lock().unwrap();
        assert_eq!(sent[0].coa().cause, Cause::ACTIVATION_CON);
        assert_eq!(sent[0].common_addr(), 7);
    }
}
