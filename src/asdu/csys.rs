// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! System information in the control direction: interrogation, counter
//! interrogation, read, clock synchronization, test, reset process and delay
//! acquisition, plus the end-of-initialization message in the monitor direction.

use chrono::{DateTime, Utc};

use crate::asdu::codec::Asdu;
use crate::asdu::identifier::{
    Cause, CauseOfTransmission, CommonAddr, Identifier, TypeId, VariableStruct,
};
use crate::asdu::info::*;
use crate::asdu::params::Params;
use crate::error::{Error, Result};

/// Start a single-object system ASDU addressed to the irrelevant IOA.
fn start_sys(
    params: Params,
    type_id: TypeId,
    coa: CauseOfTransmission,
    ca: CommonAddr,
) -> Result<Asdu> {
    params.valid()?;
    Ok(Asdu::new(
        params,
        Identifier::new(type_id, VariableStruct::single(), coa, ca),
    ))
}

impl Asdu {
    /// Build `C_IC_NA_1`: an interrogation command.
    ///
    /// See companion standard 101, subclass 7.3.4.1.
    /// Permitted causes: `Activation`, `Deactivation`.
    pub fn interrogation_cmd(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        qoi: QualifierOfInterrogation,
    ) -> Result<Asdu> {
        if coa.cause != Cause::ACTIVATION && coa.cause != Cause::DEACTIVATION {
            return Err(Error::CmdCause);
        }
        let mut u = start_sys(params, TypeId::C_IC_NA_1, coa, ca)?;
        u.encoder()
            .info_obj_addr(INFO_OBJ_ADDR_IRRELEVANT)?
            .byte(qoi.0);
        Ok(u)
    }

    /// Decode `C_IC_NA_1` into its information object address and qualifier.
    pub fn get_interrogation_cmd(&self) -> Result<(InfoObjAddr, QualifierOfInterrogation)> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        Ok((ioa, QualifierOfInterrogation(r.byte()?)))
    }

    /// Build `C_CI_NA_1`: a counter interrogation command.
    ///
    /// See companion standard 101, subclass 7.3.4.2. The cause is forced to
    /// `Activation`, which is the only one the standard permits.
    pub fn counter_interrogation_cmd(
        params: Params,
        mut coa: CauseOfTransmission,
        ca: CommonAddr,
        qcc: QualifierCountCall,
    ) -> Result<Asdu> {
        coa.cause = Cause::ACTIVATION;
        let mut u = start_sys(params, TypeId::C_CI_NA_1, coa, ca)?;
        u.encoder()
            .info_obj_addr(INFO_OBJ_ADDR_IRRELEVANT)?
            .byte(qcc.value());
        Ok(u)
    }

    /// Decode `C_CI_NA_1`.
    pub fn get_counter_interrogation_cmd(&self) -> Result<(InfoObjAddr, QualifierCountCall)> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        Ok((ioa, QualifierCountCall::parse(r.byte()?)))
    }

    /// Build `C_RD_NA_1`: a read command for one information object.
    ///
    /// See companion standard 101, subclass 7.3.4.3. The cause is forced to
    /// `Request`.
    pub fn read_cmd(
        params: Params,
        mut coa: CauseOfTransmission,
        ca: CommonAddr,
        ioa: InfoObjAddr,
    ) -> Result<Asdu> {
        coa.cause = Cause::REQUEST;
        let mut u = start_sys(params, TypeId::C_RD_NA_1, coa, ca)?;
        u.encoder().info_obj_addr(ioa)?;
        Ok(u)
    }

    /// Decode `C_RD_NA_1`.
    pub fn get_read_cmd(&self) -> Result<InfoObjAddr> {
        self.reader().info_obj_addr()
    }

    /// Build `C_CS_NA_1`: a clock synchronization command.
    ///
    /// See companion standard 101, subclass 7.3.4.4. The cause is forced to
    /// `Activation`.
    pub fn clock_synchronization_cmd(
        params: Params,
        mut coa: CauseOfTransmission,
        ca: CommonAddr,
        t: DateTime<Utc>,
    ) -> Result<Asdu> {
        coa.cause = Cause::ACTIVATION;
        let mut u = start_sys(params, TypeId::C_CS_NA_1, coa, ca)?;
        u.encoder()
            .info_obj_addr(INFO_OBJ_ADDR_IRRELEVANT)?
            .cp56time2a(Some(t));
        Ok(u)
    }

    /// Decode `C_CS_NA_1`.
    pub fn get_clock_synchronization_cmd(
        &self,
    ) -> Result<(InfoObjAddr, Option<DateTime<Utc>>)> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        Ok((ioa, r.cp56time2a()?))
    }

    /// Build `C_TS_NA_1`: a test command carrying the fixed test bit pattern.
    ///
    /// See companion standard 101, subclass 7.3.4.5. The cause is forced to
    /// `Activation`.
    pub fn test_command(
        params: Params,
        mut coa: CauseOfTransmission,
        ca: CommonAddr,
    ) -> Result<Asdu> {
        coa.cause = Cause::ACTIVATION;
        let mut u = start_sys(params, TypeId::C_TS_NA_1, coa, ca)?;
        u.encoder()
            .info_obj_addr(INFO_OBJ_ADDR_IRRELEVANT)?
            .u16(FBP_TEST_WORD);
        Ok(u)
    }

    /// Decode `C_TS_NA_1`; the flag reports whether the test pattern matched.
    pub fn get_test_command(&self) -> Result<(InfoObjAddr, bool)> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        Ok((ioa, r.u16()? == FBP_TEST_WORD))
    }

    /// Build `C_TS_TA_1`: a test command with a CP56Time2a time tag.
    pub fn test_command_cp56time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        t: DateTime<Utc>,
    ) -> Result<Asdu> {
        let mut u = start_sys(params, TypeId::C_TS_TA_1, coa, ca)?;
        u.encoder()
            .info_obj_addr(INFO_OBJ_ADDR_IRRELEVANT)?
            .u16(FBP_TEST_WORD)
            .cp56time2a(Some(t));
        Ok(u)
    }

    /// Decode `C_TS_TA_1`.
    pub fn get_test_command_cp56time2a(
        &self,
    ) -> Result<(InfoObjAddr, bool, Option<DateTime<Utc>>)> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        let ok = r.u16()? == FBP_TEST_WORD;
        Ok((ioa, ok, r.cp56time2a()?))
    }

    /// Build `C_RP_NA_1`: a reset process command.
    ///
    /// See companion standard 101, subclass 7.3.4.6. The cause is forced to
    /// `Activation`.
    pub fn reset_process_cmd(
        params: Params,
        mut coa: CauseOfTransmission,
        ca: CommonAddr,
        qrp: QualifierOfResetProcessCmd,
    ) -> Result<Asdu> {
        coa.cause = Cause::ACTIVATION;
        let mut u = start_sys(params, TypeId::C_RP_NA_1, coa, ca)?;
        u.encoder()
            .info_obj_addr(INFO_OBJ_ADDR_IRRELEVANT)?
            .byte(qrp.0);
        Ok(u)
    }

    /// Decode `C_RP_NA_1`.
    pub fn get_reset_process_cmd(&self) -> Result<(InfoObjAddr, QualifierOfResetProcessCmd)> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        Ok((ioa, QualifierOfResetProcessCmd(r.byte()?)))
    }

    /// Build `C_CD_NA_1`: a delay acquisition command (IEC 60870-5-101 only).
    ///
    /// See companion standard 101, subclass 7.3.4.7.
    /// Permitted causes: `Spontaneous`, `Activation`.
    pub fn delay_acquire_command(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        msec: u16,
    ) -> Result<Asdu> {
        if coa.cause != Cause::SPONTANEOUS && coa.cause != Cause::ACTIVATION {
            return Err(Error::CmdCause);
        }
        let mut u = start_sys(params, TypeId::C_CD_NA_1, coa, ca)?;
        u.encoder()
            .info_obj_addr(INFO_OBJ_ADDR_IRRELEVANT)?
            .cp16time2a(msec);
        Ok(u)
    }

    /// Decode `C_CD_NA_1`.
    pub fn get_delay_acquire_command(&self) -> Result<(InfoObjAddr, u16)> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        Ok((ioa, r.u16()?))
    }

    /// Build `M_EI_NA_1`: end of initialization, in the monitor direction.
    ///
    /// See companion standard 101, subclass 7.3.3.1. The cause is forced to
    /// `Initialized`.
    pub fn end_of_initialization(
        params: Params,
        mut coa: CauseOfTransmission,
        ca: CommonAddr,
        ioa: InfoObjAddr,
        coi: CauseOfInitial,
    ) -> Result<Asdu> {
        coa.cause = Cause::INITIALIZED;
        let mut u = start_sys(params, TypeId::M_EI_NA_1, coa, ca)?;
        u.encoder().info_obj_addr(ioa)?.byte(coi.value());
        Ok(u)
    }

    /// Decode `M_EI_NA_1`.
    pub fn get_end_of_initialization(&self) -> Result<(InfoObjAddr, CauseOfInitial)> {
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        Ok((ioa, CauseOfInitial::parse(r.byte()?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::identifier::GLOBAL_COMMON_ADDR;
    use crate::asdu::params::{PARAMS_STANDARD_101, PARAMS_WIDE};
    use chrono::TimeZone as _;

    fn act() -> CauseOfTransmission {
        CauseOfTransmission::new(Cause::ACTIVATION)
    }

    #[test]
    fn interrogation_cmd_uses_the_irrelevant_address() {
        let a =
            Asdu::interrogation_cmd(PARAMS_WIDE, act(), 1, QualifierOfInterrogation::STATION)
                .unwrap();
        assert_eq!(a.info_obj, vec![0, 0, 0, 20]);
        assert_eq!(
            a.get_interrogation_cmd().unwrap(),
            (0, QualifierOfInterrogation::STATION)
        );
    }

    #[test]
    fn interrogation_cmd_rejects_other_causes() {
        assert_eq!(
            Asdu::interrogation_cmd(
                PARAMS_WIDE,
                CauseOfTransmission::new(Cause::SPONTANEOUS),
                1,
                QualifierOfInterrogation::STATION
            ),
            Err(Error::CmdCause)
        );
    }

    #[test]
    fn interrogation_cmd_may_be_broadcast() {
        let a = Asdu::interrogation_cmd(
            PARAMS_STANDARD_101,
            act(),
            GLOBAL_COMMON_ADDR,
            QualifierOfInterrogation::GROUP1,
        )
        .unwrap();
        let raw = a.marshal_binary().unwrap();
        assert_eq!(raw[3], 255);
    }

    #[test]
    fn counter_interrogation_forces_activation() {
        let qcc = QualifierCountCall {
            request: QccRequest::TOTAL,
            freeze: QccFreeze::FREEZE_RESET,
        };
        let a = Asdu::counter_interrogation_cmd(
            PARAMS_WIDE,
            CauseOfTransmission::new(Cause::SPONTANEOUS),
            1,
            qcc,
        )
        .unwrap();
        assert_eq!(a.coa().cause, Cause::ACTIVATION);
        assert_eq!(a.get_counter_interrogation_cmd().unwrap(), (0, qcc));
    }

    #[test]
    fn read_cmd_forces_request_and_keeps_the_address() {
        let a = Asdu::read_cmd(PARAMS_WIDE, act(), 1, 4242).unwrap();
        assert_eq!(a.coa().cause, Cause::REQUEST);
        assert_eq!(a.get_read_cmd().unwrap(), 4242);
    }

    #[test]
    fn clock_sync_round_trips_the_time() {
        let t = Utc.with_ymd_and_hms(2026, 8, 17, 12, 34, 56).unwrap();
        let a = Asdu::clock_synchronization_cmd(PARAMS_WIDE, act(), 1, t).unwrap();
        assert_eq!(a.coa().cause, Cause::ACTIVATION);
        assert_eq!(a.get_clock_synchronization_cmd().unwrap(), (0, Some(t)));
    }

    #[test]
    fn test_commands_carry_the_fixed_bit_pattern() {
        let a = Asdu::test_command(PARAMS_WIDE, act(), 1).unwrap();
        assert_eq!(a.info_obj, vec![0, 0, 0, 0xaa, 0x55]);
        assert_eq!(a.get_test_command().unwrap(), (0, true));

        let t = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let a = Asdu::test_command_cp56time2a(PARAMS_WIDE, act(), 1, t).unwrap();
        assert_eq!(a.get_test_command_cp56time2a().unwrap(), (0, true, Some(t)));
    }

    #[test]
    fn reset_process_and_delay_acquisition_round_trip() {
        let a = Asdu::reset_process_cmd(
            PARAMS_WIDE,
            act(),
            1,
            QualifierOfResetProcessCmd::GENERAL_RESET,
        )
        .unwrap();
        assert_eq!(
            a.get_reset_process_cmd().unwrap(),
            (0, QualifierOfResetProcessCmd::GENERAL_RESET)
        );

        let a = Asdu::delay_acquire_command(PARAMS_STANDARD_101, act(), 1, 2500).unwrap();
        assert_eq!(a.get_delay_acquire_command().unwrap(), (0, 2500));
        assert_eq!(
            Asdu::delay_acquire_command(
                PARAMS_STANDARD_101,
                CauseOfTransmission::new(Cause::REQUEST),
                1,
                0
            ),
            Err(Error::CmdCause)
        );
    }

    #[test]
    fn end_of_initialization_forces_the_initialized_cause() {
        let coi = CauseOfInitial {
            cause: CoiCause::REMOTE_RESET,
            is_local_change: true,
        };
        let a = Asdu::end_of_initialization(PARAMS_WIDE, act(), 1, 0, coi).unwrap();
        assert_eq!(a.coa().cause, Cause::INITIALIZED);
        assert_eq!(a.get_end_of_initialization().unwrap(), (0, coi));
    }

    #[test]
    fn system_commands_survive_a_wire_round_trip() {
        let p = PARAMS_WIDE;
        let t = Utc.with_ymd_and_hms(2026, 5, 4, 3, 2, 1).unwrap();
        for a in [
            Asdu::interrogation_cmd(p, act(), 1, QualifierOfInterrogation::STATION).unwrap(),
            Asdu::counter_interrogation_cmd(p, act(), 1, QualifierCountCall::default()).unwrap(),
            Asdu::read_cmd(p, act(), 1, 99).unwrap(),
            Asdu::clock_synchronization_cmd(p, act(), 1, t).unwrap(),
            Asdu::test_command(p, act(), 1).unwrap(),
            Asdu::test_command_cp56time2a(p, act(), 1, t).unwrap(),
            Asdu::reset_process_cmd(p, act(), 1, QualifierOfResetProcessCmd::GENERAL_RESET)
                .unwrap(),
            Asdu::delay_acquire_command(p, act(), 1, 10).unwrap(),
            Asdu::end_of_initialization(p, act(), 1, 0, CauseOfInitial::default()).unwrap(),
        ] {
            let raw = a.marshal_binary().unwrap();
            assert_eq!(
                Asdu::unmarshal_binary(p, &raw).unwrap(),
                a,
                "round trip failed for {}",
                a.type_id()
            );
        }
    }

    #[test]
    fn a_clock_sync_with_an_invalid_time_carries_no_time() {
        // A clock must never be set from a time its sender marks invalid, so
        // this decoder keeps the strict reading.
        let t = chrono::Utc.with_ymd_and_hms(2026, 3, 4, 5, 6, 7).unwrap();
        let mut a = Asdu::clock_synchronization_cmd(
            PARAMS_WIDE,
            CauseOfTransmission::new(Cause::ACTIVATION),
            1,
            t,
        )
        .unwrap();
        a.info_obj[3 + 2] |= 0x80;
        assert_eq!(a.get_clock_synchronization_cmd().unwrap().1, None);
    }
}
