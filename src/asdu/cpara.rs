// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Parameters in the control direction: the `P_*` types that configure
//! thresholds, smoothing factors and transmission limits of measured values.

use crate::asdu::codec::Asdu;
use crate::asdu::identifier::{
    Cause, CauseOfTransmission, CommonAddr, Identifier, TypeId, VariableStruct,
};
use crate::asdu::info::*;
use crate::asdu::params::Params;
use crate::error::{Error, Result};

/// A normalized measured value parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ParameterNormalInfo {
    /// Information object address of the measured value being parameterised.
    pub ioa: InfoObjAddr,
    /// The parameter value.
    pub value: Normalize,
    /// Which parameter this is, plus its change and operation flags.
    pub qpm: QualifierOfParameterMv,
}

/// A scaled measured value parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ParameterScaledInfo {
    /// Information object address of the measured value being parameterised.
    pub ioa: InfoObjAddr,
    /// The parameter value.
    pub value: i16,
    /// Which parameter this is, plus its change and operation flags.
    pub qpm: QualifierOfParameterMv,
}

/// A short floating point measured value parameter.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ParameterFloatInfo {
    /// Information object address of the measured value being parameterised.
    pub ioa: InfoObjAddr,
    /// The parameter value.
    pub value: f32,
    /// Which parameter this is, plus its change and operation flags.
    pub qpm: QualifierOfParameterMv,
}

/// A parameter activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ParameterActivationInfo {
    /// Information object address; 0 addresses the previously loaded parameters.
    pub ioa: InfoObjAddr,
    /// What is being (de)activated.
    pub qpa: QualifierOfParameterAct,
}

fn start_para(
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
    /// Build `P_ME_NA_1`: a normalized measured value parameter.
    ///
    /// See companion standard 101, subclass 7.3.5.1. Only `Activation` is
    /// permitted in the control direction.
    pub fn parameter_normal(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        p: ParameterNormalInfo,
    ) -> Result<Asdu> {
        if coa.cause != Cause::ACTIVATION {
            return Err(Error::CmdCause);
        }
        let mut u = start_para(params, TypeId::P_ME_NA_1, coa, ca)?;
        u.encoder()
            .info_obj_addr(p.ioa)?
            .normalize(p.value)
            .byte(p.qpm.value());
        Ok(u)
    }

    /// Decode `P_ME_NA_1`.
    pub fn get_parameter_normal(&self) -> Result<ParameterNormalInfo> {
        let mut r = self.reader();
        Ok(ParameterNormalInfo {
            ioa: r.info_obj_addr()?,
            value: r.normalize()?,
            qpm: QualifierOfParameterMv::parse(r.byte()?),
        })
    }

    /// Build `P_ME_NB_1`: a scaled measured value parameter.
    ///
    /// See companion standard 101, subclass 7.3.5.2.
    pub fn parameter_scaled(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        p: ParameterScaledInfo,
    ) -> Result<Asdu> {
        if coa.cause != Cause::ACTIVATION {
            return Err(Error::CmdCause);
        }
        let mut u = start_para(params, TypeId::P_ME_NB_1, coa, ca)?;
        u.encoder()
            .info_obj_addr(p.ioa)?
            .scaled(p.value)
            .byte(p.qpm.value());
        Ok(u)
    }

    /// Decode `P_ME_NB_1`.
    pub fn get_parameter_scaled(&self) -> Result<ParameterScaledInfo> {
        let mut r = self.reader();
        Ok(ParameterScaledInfo {
            ioa: r.info_obj_addr()?,
            value: r.scaled()?,
            qpm: QualifierOfParameterMv::parse(r.byte()?),
        })
    }

    /// Build `P_ME_NC_1`: a short floating point measured value parameter.
    ///
    /// See companion standard 101, subclass 7.3.5.3.
    pub fn parameter_float(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        p: ParameterFloatInfo,
    ) -> Result<Asdu> {
        if coa.cause != Cause::ACTIVATION {
            return Err(Error::CmdCause);
        }
        let mut u = start_para(params, TypeId::P_ME_NC_1, coa, ca)?;
        u.encoder()
            .info_obj_addr(p.ioa)?
            .f32(p.value)
            .byte(p.qpm.value());
        Ok(u)
    }

    /// Decode `P_ME_NC_1`.
    pub fn get_parameter_float(&self) -> Result<ParameterFloatInfo> {
        let mut r = self.reader();
        Ok(ParameterFloatInfo {
            ioa: r.info_obj_addr()?,
            value: r.f32()?,
            qpm: QualifierOfParameterMv::parse(r.byte()?),
        })
    }

    /// Build `P_AC_NA_1`: a parameter activation.
    ///
    /// See companion standard 101, subclass 7.3.5.4.
    /// Permitted causes: `Activation`, `Deactivation`.
    pub fn parameter_activation(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        p: ParameterActivationInfo,
    ) -> Result<Asdu> {
        if coa.cause != Cause::ACTIVATION && coa.cause != Cause::DEACTIVATION {
            return Err(Error::CmdCause);
        }
        let mut u = start_para(params, TypeId::P_AC_NA_1, coa, ca)?;
        u.encoder().info_obj_addr(p.ioa)?.byte(p.qpa.0);
        Ok(u)
    }

    /// Decode `P_AC_NA_1`.
    pub fn get_parameter_activation(&self) -> Result<ParameterActivationInfo> {
        let mut r = self.reader();
        Ok(ParameterActivationInfo {
            ioa: r.info_obj_addr()?,
            qpa: QualifierOfParameterAct(r.byte()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::params::PARAMS_WIDE;

    fn act() -> CauseOfTransmission {
        CauseOfTransmission::new(Cause::ACTIVATION)
    }

    #[test]
    fn parameter_families_round_trip() {
        let qpm = QualifierOfParameterMv {
            category: QpmCategory::THRESHOLD,
            is_change: true,
            is_in_operation: false,
        };

        let n = ParameterNormalInfo {
            ioa: 400,
            value: Normalize(1024),
            qpm,
        };
        assert_eq!(
            Asdu::parameter_normal(PARAMS_WIDE, act(), 1, n)
                .unwrap()
                .get_parameter_normal()
                .unwrap(),
            n
        );

        let s = ParameterScaledInfo {
            ioa: 401,
            value: -500,
            qpm,
        };
        assert_eq!(
            Asdu::parameter_scaled(PARAMS_WIDE, act(), 1, s)
                .unwrap()
                .get_parameter_scaled()
                .unwrap(),
            s
        );

        let f = ParameterFloatInfo {
            ioa: 402,
            value: 12.5,
            qpm,
        };
        assert_eq!(
            Asdu::parameter_float(PARAMS_WIDE, act(), 1, f)
                .unwrap()
                .get_parameter_float()
                .unwrap(),
            f
        );

        let a = ParameterActivationInfo {
            ioa: 403,
            qpa: QualifierOfParameterAct::DEACT_OBJECT_PARAMETER,
        };
        assert_eq!(
            Asdu::parameter_activation(PARAMS_WIDE, act(), 1, a)
                .unwrap()
                .get_parameter_activation()
                .unwrap(),
            a
        );
    }

    #[test]
    fn parameters_require_activation() {
        let coa = CauseOfTransmission::new(Cause::SPONTANEOUS);
        assert_eq!(
            Asdu::parameter_normal(PARAMS_WIDE, coa, 1, ParameterNormalInfo::default()),
            Err(Error::CmdCause)
        );
        // Parameter activation additionally accepts Deactivation.
        assert!(
            Asdu::parameter_activation(
                PARAMS_WIDE,
                CauseOfTransmission::new(Cause::DEACTIVATION),
                1,
                ParameterActivationInfo::default()
            )
            .is_ok()
        );
    }
}
