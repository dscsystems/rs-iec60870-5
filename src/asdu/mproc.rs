// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Process information in the monitoring direction: the `M_*` types that carry
//! measurements and status from the controlled station to the master.
//!
//! Each family has a builder per time-tag variant (none, CP24Time2a,
//! CP56Time2a) and one getter that decodes all three.

use chrono::{DateTime, Utc};

use crate::asdu::codec::{Asdu, check_valid};
use crate::asdu::identifier::{Cause, CauseOfTransmission, CommonAddr, Identifier, TypeId, VariableStruct};
use crate::asdu::info::*;
use crate::asdu::params::Params;
use crate::error::{Error, Result};

/// A single-point measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SinglePointInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// The point state.
    pub value: bool,
    /// Quality descriptor; [`QualityDescriptor::GOOD`] means no remarks.
    pub qds: QualityDescriptor,
    /// Time tag; ignored by the untagged type.
    pub time: Option<DateTime<Utc>>,
}

impl SinglePointInfo {
    /// A good-quality, untagged point.
    pub fn new(ioa: InfoObjAddr, value: bool) -> Self {
        SinglePointInfo {
            ioa,
            value,
            qds: QualityDescriptor::GOOD,
            time: None,
        }
    }
}

/// A double-point measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DoublePointInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// The point state.
    pub value: DoublePoint,
    /// Quality descriptor.
    pub qds: QualityDescriptor,
    /// Time tag; ignored by the untagged type.
    pub time: Option<DateTime<Utc>>,
}

/// A step (tap changer) position measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StepPositionInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Step position and transient indication.
    pub value: StepPosition,
    /// Quality descriptor.
    pub qds: QualityDescriptor,
    /// Time tag; ignored by the untagged type.
    pub time: Option<DateTime<Utc>>,
}

/// A 32 bit string measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BitString32Info {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// The 32 status bits.
    pub value: u32,
    /// Quality descriptor.
    pub qds: QualityDescriptor,
    /// Time tag; ignored by the untagged type.
    pub time: Option<DateTime<Utc>>,
}

/// A normalized measured value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MeasuredValueNormalInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// The normalized value in `[-1, 1 - 2^-15]`.
    pub value: Normalize,
    /// Quality descriptor; not transmitted by `M_ME_ND_1`.
    pub qds: QualityDescriptor,
    /// Time tag; ignored by the untagged types.
    pub time: Option<DateTime<Utc>>,
}

/// A scaled measured value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MeasuredValueScaledInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// The scaled value.
    pub value: i16,
    /// Quality descriptor.
    pub qds: QualityDescriptor,
    /// Time tag; ignored by the untagged type.
    pub time: Option<DateTime<Utc>>,
}

/// A short floating point measured value.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MeasuredValueFloatInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// The measured value.
    pub value: f32,
    /// Quality descriptor.
    pub qds: QualityDescriptor,
    /// Time tag; ignored by the untagged type.
    pub time: Option<DateTime<Utc>>,
}

/// An integrated total (counter) reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BinaryCounterReadingInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// The counter reading and its flags.
    pub value: BinaryCounterReading,
    /// Time tag; ignored by the untagged type.
    pub time: Option<DateTime<Utc>>,
}

/// An event of protection equipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EventOfProtectionEquipmentInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// The event state.
    pub event: SingleEvent,
    /// Protection quality descriptor.
    pub qdp: QualityDescriptorProtection,
    /// Elapsed time of the protection operation, in milliseconds (CP16Time2a).
    pub msec: u16,
    /// Time tag of the event.
    pub time: Option<DateTime<Utc>>,
}

/// Packed start events of protection equipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PackedStartEventsOfProtectionEquipmentInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Which phases started.
    pub event: StartEvent,
    /// Protection quality descriptor.
    pub qdp: QualityDescriptorProtection,
    /// Relay duration time in milliseconds (CP16Time2a).
    pub msec: u16,
    /// Time tag of the event.
    pub time: Option<DateTime<Utc>>,
}

/// Packed output circuit information of protection equipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PackedOutputCircuitInfoInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Which output circuits were commanded.
    pub oci: OutputCircuitInfo,
    /// Protection quality descriptor.
    pub qdp: QualityDescriptorProtection,
    /// Relay operating time in milliseconds (CP16Time2a).
    pub msec: u16,
    /// Time tag of the event.
    pub time: Option<DateTime<Utc>>,
}

/// Packed single-point information with status change detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PackedSinglePointWithScdInfo {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// 16 status bits plus 16 change-detection bits.
    pub scd: StatusAndStatusChangeDetection,
    /// Quality descriptor.
    pub qds: QualityDescriptor,
}

/// Causes permitted for untagged process information in the monitor direction.
fn check_untagged_cause(coa: CauseOfTransmission) -> Result<()> {
    let c = coa.cause;
    if c == Cause::BACKGROUND
        || c == Cause::SPONTANEOUS
        || c == Cause::REQUEST
        || c == Cause::RETURN_INFO_REMOTE
        || c == Cause::RETURN_INFO_LOCAL
        || c.is_interrogation()
    {
        Ok(())
    } else {
        Err(Error::CmdCause)
    }
}

/// Causes permitted for time-tagged process information in the monitor direction.
fn check_tagged_cause(coa: CauseOfTransmission) -> Result<()> {
    let c = coa.cause;
    if c == Cause::SPONTANEOUS
        || c == Cause::REQUEST
        || c == Cause::RETURN_INFO_REMOTE
        || c == Cause::RETURN_INFO_LOCAL
    {
        Ok(())
    } else {
        Err(Error::CmdCause)
    }
}

/// Causes permitted for untagged measured values (adds `Periodic`, drops return info).
fn check_measured_cause(coa: CauseOfTransmission) -> Result<()> {
    let c = coa.cause;
    if c == Cause::PERIODIC
        || c == Cause::BACKGROUND
        || c == Cause::SPONTANEOUS
        || c == Cause::REQUEST
        || c.is_interrogation()
    {
        Ok(())
    } else {
        Err(Error::CmdCause)
    }
}

/// Causes permitted for time-tagged measured values and bit strings.
fn check_spontaneous_or_request(coa: CauseOfTransmission) -> Result<()> {
    if coa.cause == Cause::SPONTANEOUS || coa.cause == Cause::REQUEST {
        Ok(())
    } else {
        Err(Error::CmdCause)
    }
}

/// Causes permitted for integrated totals.
fn check_counter_cause(coa: CauseOfTransmission) -> Result<()> {
    if coa.cause == Cause::SPONTANEOUS || coa.cause.is_counter_request() {
        Ok(())
    } else {
        Err(Error::CmdCause)
    }
}

/// Start an ASDU for a set of information objects, with the object count set.
fn start(
    params: Params,
    type_id: TypeId,
    is_sequence: bool,
    coa: CauseOfTransmission,
    ca: CommonAddr,
    n: usize,
) -> Result<Asdu> {
    check_valid(&params, type_id, is_sequence, n)?;
    let mut u = Asdu::new(
        params,
        Identifier::new(
            type_id,
            VariableStruct {
                number: 0,
                is_sequence,
            },
            coa,
            ca,
        ),
    );
    u.set_variable_number(n)?;
    Ok(u)
}

/// True when the information object at index `i` carries its own address.
fn writes_addr(is_sequence: bool, i: usize) -> bool {
    !is_sequence || i == 0
}

impl Asdu {
    // -- single point ----------------------------------------------------

    /// Build `M_SP_NA_1`, `M_SP_TA_1` or `M_SP_TB_1`: single-point information.
    ///
    /// See companion standard 101, subclasses 7.3.1.1, 7.3.1.2 and 7.3.1.22.
    pub fn single_with_type(
        params: Params,
        type_id: TypeId,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[SinglePointInfo],
    ) -> Result<Asdu> {
        let mut u = start(params, type_id, is_sequence, coa, ca, infos.len())?;
        for (i, v) in infos.iter().enumerate() {
            let mut e = u.encoder();
            if writes_addr(is_sequence, i) {
                e.info_obj_addr(v.ioa)?;
            }
            e.byte(u8::from(v.value) | (v.qds.0 & 0xf0));
            match type_id {
                TypeId::M_SP_NA_1 => {}
                TypeId::M_SP_TA_1 => {
                    e.cp24time2a(v.time);
                }
                TypeId::M_SP_TB_1 => {
                    e.cp56time2a(v.time);
                }
                _ => return Err(Error::TypeIdNotMatch),
            }
        }
        Ok(u)
    }

    /// Build `M_SP_NA_1`: single-point information without time tag.
    ///
    /// Permitted causes: `Background`, `Spontaneous`, `Request`,
    /// `ReturnInfoRemote`, `ReturnInfoLocal` and the interrogation causes.
    pub fn single(
        params: Params,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[SinglePointInfo],
    ) -> Result<Asdu> {
        check_untagged_cause(coa)?;
        Self::single_with_type(params, TypeId::M_SP_NA_1, is_sequence, coa, ca, infos)
    }

    /// Build `M_SP_TA_1`: single-point information with a CP24Time2a time tag.
    ///
    /// Never a sequence. Permitted causes: `Spontaneous`, `Request`,
    /// `ReturnInfoRemote`, `ReturnInfoLocal`.
    pub fn single_cp24time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[SinglePointInfo],
    ) -> Result<Asdu> {
        check_tagged_cause(coa)?;
        Self::single_with_type(params, TypeId::M_SP_TA_1, false, coa, ca, infos)
    }

    /// Build `M_SP_TB_1`: single-point information with a CP56Time2a time tag.
    ///
    /// Never a sequence. Permitted causes: `Spontaneous`, `Request`,
    /// `ReturnInfoRemote`, `ReturnInfoLocal`.
    pub fn single_cp56time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[SinglePointInfo],
    ) -> Result<Asdu> {
        check_tagged_cause(coa)?;
        Self::single_with_type(params, TypeId::M_SP_TB_1, false, coa, ca, infos)
    }

    /// Decode `M_SP_NA_1`, `M_SP_TA_1` or `M_SP_TB_1`.
    pub fn get_single_point(&self) -> Result<Vec<SinglePointInfo>> {
        let seq = self.variable().is_sequence;
        let n = self.variable().number as usize;
        let mut r = self.reader();
        let mut out = Vec::with_capacity(n);
        let mut ioa = 0;
        for i in 0..n {
            ioa = r.next_obj_addr(i, seq, ioa)?;
            let value = r.byte()?;
            let time = match self.type_id() {
                TypeId::M_SP_NA_1 => None,
                TypeId::M_SP_TA_1 => r.cp24time2a()?,
                TypeId::M_SP_TB_1 => r.cp56time2a()?,
                _ => return Err(Error::TypeIdNotMatch),
            };
            out.push(SinglePointInfo {
                ioa,
                value: value & 0x01 == 0x01,
                qds: QualityDescriptor(value & 0xf0),
                time,
            });
        }
        Ok(out)
    }

    // -- double point ----------------------------------------------------

    /// Build `M_DP_NA_1`, `M_DP_TA_1` or `M_DP_TB_1`: double-point information.
    ///
    /// See companion standard 101, subclasses 7.3.1.3, 7.3.1.4 and 7.3.1.23.
    pub fn double_with_type(
        params: Params,
        type_id: TypeId,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[DoublePointInfo],
    ) -> Result<Asdu> {
        let mut u = start(params, type_id, is_sequence, coa, ca, infos.len())?;
        for (i, v) in infos.iter().enumerate() {
            let mut e = u.encoder();
            if writes_addr(is_sequence, i) {
                e.info_obj_addr(v.ioa)?;
            }
            e.byte(v.value.value() | (v.qds.0 & 0xf0));
            match type_id {
                TypeId::M_DP_NA_1 => {}
                TypeId::M_DP_TA_1 => {
                    e.cp24time2a(v.time);
                }
                TypeId::M_DP_TB_1 => {
                    e.cp56time2a(v.time);
                }
                _ => return Err(Error::TypeIdNotMatch),
            }
        }
        Ok(u)
    }

    /// Build `M_DP_NA_1`: double-point information without time tag.
    pub fn double(
        params: Params,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[DoublePointInfo],
    ) -> Result<Asdu> {
        check_untagged_cause(coa)?;
        Self::double_with_type(params, TypeId::M_DP_NA_1, is_sequence, coa, ca, infos)
    }

    /// Build `M_DP_TA_1`: double-point information with a CP24Time2a time tag.
    pub fn double_cp24time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[DoublePointInfo],
    ) -> Result<Asdu> {
        check_tagged_cause(coa)?;
        Self::double_with_type(params, TypeId::M_DP_TA_1, false, coa, ca, infos)
    }

    /// Build `M_DP_TB_1`: double-point information with a CP56Time2a time tag.
    pub fn double_cp56time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[DoublePointInfo],
    ) -> Result<Asdu> {
        check_tagged_cause(coa)?;
        Self::double_with_type(params, TypeId::M_DP_TB_1, false, coa, ca, infos)
    }

    /// Decode `M_DP_NA_1`, `M_DP_TA_1` or `M_DP_TB_1`.
    pub fn get_double_point(&self) -> Result<Vec<DoublePointInfo>> {
        let seq = self.variable().is_sequence;
        let n = self.variable().number as usize;
        let mut r = self.reader();
        let mut out = Vec::with_capacity(n);
        let mut ioa = 0;
        for i in 0..n {
            ioa = r.next_obj_addr(i, seq, ioa)?;
            let value = r.byte()?;
            let time = match self.type_id() {
                TypeId::M_DP_NA_1 => None,
                TypeId::M_DP_TA_1 => r.cp24time2a()?,
                TypeId::M_DP_TB_1 => r.cp56time2a()?,
                _ => return Err(Error::TypeIdNotMatch),
            };
            out.push(DoublePointInfo {
                ioa,
                value: DoublePoint::parse(value),
                qds: QualityDescriptor(value & 0xf0),
                time,
            });
        }
        Ok(out)
    }

    // -- step position ---------------------------------------------------

    /// Build `M_ST_NA_1`, `M_ST_TA_1` or `M_ST_TB_1`: step position information.
    ///
    /// See companion standard 101, subclasses 7.3.1.5, 7.3.1.6 and 7.3.1.24.
    pub fn step_with_type(
        params: Params,
        type_id: TypeId,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[StepPositionInfo],
    ) -> Result<Asdu> {
        let mut u = start(params, type_id, is_sequence, coa, ca, infos.len())?;
        for (i, v) in infos.iter().enumerate() {
            let mut e = u.encoder();
            if writes_addr(is_sequence, i) {
                e.info_obj_addr(v.ioa)?;
            }
            e.byte(v.value.value()).byte(v.qds.0);
            match type_id {
                TypeId::M_ST_NA_1 => {}
                TypeId::M_ST_TA_1 => {
                    e.cp24time2a(v.time);
                }
                TypeId::M_ST_TB_1 => {
                    e.cp56time2a(v.time);
                }
                _ => return Err(Error::TypeIdNotMatch),
            }
        }
        Ok(u)
    }

    /// Build `M_ST_NA_1`: step position information without time tag.
    pub fn step(
        params: Params,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[StepPositionInfo],
    ) -> Result<Asdu> {
        check_untagged_cause(coa)?;
        Self::step_with_type(params, TypeId::M_ST_NA_1, is_sequence, coa, ca, infos)
    }

    /// Build `M_ST_TA_1`: step position information with a CP24Time2a time tag.
    pub fn step_cp24time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[StepPositionInfo],
    ) -> Result<Asdu> {
        check_tagged_cause(coa)?;
        Self::step_with_type(params, TypeId::M_ST_TA_1, false, coa, ca, infos)
    }

    /// Build `M_ST_TB_1`: step position information with a CP56Time2a time tag.
    pub fn step_cp56time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[StepPositionInfo],
    ) -> Result<Asdu> {
        check_tagged_cause(coa)?;
        Self::step_with_type(params, TypeId::M_ST_TB_1, false, coa, ca, infos)
    }

    /// Decode `M_ST_NA_1`, `M_ST_TA_1` or `M_ST_TB_1`.
    pub fn get_step_position(&self) -> Result<Vec<StepPositionInfo>> {
        let seq = self.variable().is_sequence;
        let n = self.variable().number as usize;
        let mut r = self.reader();
        let mut out = Vec::with_capacity(n);
        let mut ioa = 0;
        for i in 0..n {
            ioa = r.next_obj_addr(i, seq, ioa)?;
            let value = StepPosition::parse(r.byte()?);
            let qds = QualityDescriptor(r.byte()?);
            let time = match self.type_id() {
                TypeId::M_ST_NA_1 => None,
                TypeId::M_ST_TA_1 => r.cp24time2a()?,
                TypeId::M_ST_TB_1 => r.cp56time2a()?,
                _ => return Err(Error::TypeIdNotMatch),
            };
            out.push(StepPositionInfo {
                ioa,
                value,
                qds,
                time,
            });
        }
        Ok(out)
    }

    // -- bit string ------------------------------------------------------

    /// Build `M_BO_NA_1`, `M_BO_TA_1` or `M_BO_TB_1`: bit string of 32 bits.
    ///
    /// See companion standard 101, subclasses 7.3.1.7, 7.3.1.8 and 7.3.1.25.
    pub fn bitstring32_with_type(
        params: Params,
        type_id: TypeId,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BitString32Info],
    ) -> Result<Asdu> {
        let mut u = start(params, type_id, is_sequence, coa, ca, infos.len())?;
        for (i, v) in infos.iter().enumerate() {
            let mut e = u.encoder();
            if writes_addr(is_sequence, i) {
                e.info_obj_addr(v.ioa)?;
            }
            e.bits_string32(v.value).byte(v.qds.0);
            match type_id {
                TypeId::M_BO_NA_1 => {}
                TypeId::M_BO_TA_1 => {
                    e.cp24time2a(v.time);
                }
                TypeId::M_BO_TB_1 => {
                    e.cp56time2a(v.time);
                }
                _ => return Err(Error::TypeIdNotMatch),
            }
        }
        Ok(u)
    }

    /// Build `M_BO_NA_1`: bit string of 32 bits without time tag.
    ///
    /// Permitted causes: `Background`, `Spontaneous`, `Request` and the
    /// interrogation causes.
    pub fn bitstring32(
        params: Params,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BitString32Info],
    ) -> Result<Asdu> {
        let c = coa.cause;
        if !(c == Cause::BACKGROUND
            || c == Cause::SPONTANEOUS
            || c == Cause::REQUEST
            || c.is_interrogation())
        {
            return Err(Error::CmdCause);
        }
        Self::bitstring32_with_type(params, TypeId::M_BO_NA_1, is_sequence, coa, ca, infos)
    }

    /// Build `M_BO_TA_1`: bit string of 32 bits with a CP24Time2a time tag.
    pub fn bitstring32_cp24time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BitString32Info],
    ) -> Result<Asdu> {
        check_spontaneous_or_request(coa)?;
        Self::bitstring32_with_type(params, TypeId::M_BO_TA_1, false, coa, ca, infos)
    }

    /// Build `M_BO_TB_1`: bit string of 32 bits with a CP56Time2a time tag.
    pub fn bitstring32_cp56time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BitString32Info],
    ) -> Result<Asdu> {
        check_spontaneous_or_request(coa)?;
        Self::bitstring32_with_type(params, TypeId::M_BO_TB_1, false, coa, ca, infos)
    }

    /// Decode `M_BO_NA_1`, `M_BO_TA_1` or `M_BO_TB_1`.
    pub fn get_bitstring32(&self) -> Result<Vec<BitString32Info>> {
        let seq = self.variable().is_sequence;
        let n = self.variable().number as usize;
        let mut r = self.reader();
        let mut out = Vec::with_capacity(n);
        let mut ioa = 0;
        for i in 0..n {
            ioa = r.next_obj_addr(i, seq, ioa)?;
            let value = r.bits_string32()?;
            let qds = QualityDescriptor(r.byte()?);
            let time = match self.type_id() {
                TypeId::M_BO_NA_1 => None,
                TypeId::M_BO_TA_1 => r.cp24time2a()?,
                TypeId::M_BO_TB_1 => r.cp56time2a()?,
                _ => return Err(Error::TypeIdNotMatch),
            };
            out.push(BitString32Info {
                ioa,
                value,
                qds,
                time,
            });
        }
        Ok(out)
    }

    // -- measured value, normalized --------------------------------------

    /// Build `M_ME_NA_1`, `M_ME_TA_1`, `M_ME_TD_1` or `M_ME_ND_1`: a
    /// normalized measured value.
    ///
    /// `M_ME_ND_1` carries no quality descriptor; the field is ignored.
    /// See companion standard 101, subclasses 7.3.1.9, 7.3.1.10, 7.3.1.26
    /// and 7.3.1.21.
    pub fn measured_value_normal_with_type(
        params: Params,
        type_id: TypeId,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueNormalInfo],
    ) -> Result<Asdu> {
        let mut u = start(params, type_id, is_sequence, coa, ca, infos.len())?;
        for (i, v) in infos.iter().enumerate() {
            let mut e = u.encoder();
            if writes_addr(is_sequence, i) {
                e.info_obj_addr(v.ioa)?;
            }
            e.normalize(v.value);
            match type_id {
                TypeId::M_ME_NA_1 => {
                    e.byte(v.qds.0);
                }
                TypeId::M_ME_TA_1 => {
                    e.byte(v.qds.0).cp24time2a(v.time);
                }
                TypeId::M_ME_TD_1 => {
                    e.byte(v.qds.0).cp56time2a(v.time);
                }
                TypeId::M_ME_ND_1 => {} // without quality descriptor
                _ => return Err(Error::TypeIdNotMatch),
            }
        }
        Ok(u)
    }

    /// Build `M_ME_NA_1`: normalized measured value without time tag.
    ///
    /// Permitted causes: `Periodic`, `Background`, `Spontaneous`, `Request`
    /// and the interrogation causes.
    pub fn measured_value_normal(
        params: Params,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueNormalInfo],
    ) -> Result<Asdu> {
        check_measured_cause(coa)?;
        Self::measured_value_normal_with_type(
            params,
            TypeId::M_ME_NA_1,
            is_sequence,
            coa,
            ca,
            infos,
        )
    }

    /// Build `M_ME_TA_1`: normalized measured value with a CP24Time2a time tag.
    pub fn measured_value_normal_cp24time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueNormalInfo],
    ) -> Result<Asdu> {
        check_spontaneous_or_request(coa)?;
        Self::measured_value_normal_with_type(params, TypeId::M_ME_TA_1, false, coa, ca, infos)
    }

    /// Build `M_ME_TD_1`: normalized measured value with a CP56Time2a time tag.
    pub fn measured_value_normal_cp56time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueNormalInfo],
    ) -> Result<Asdu> {
        check_spontaneous_or_request(coa)?;
        Self::measured_value_normal_with_type(params, TypeId::M_ME_TD_1, false, coa, ca, infos)
    }

    /// Build `M_ME_ND_1`: normalized measured value without a quality descriptor.
    ///
    /// The quality is implicitly good.
    pub fn measured_value_normal_no_quality(
        params: Params,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueNormalInfo],
    ) -> Result<Asdu> {
        check_measured_cause(coa)?;
        Self::measured_value_normal_with_type(
            params,
            TypeId::M_ME_ND_1,
            is_sequence,
            coa,
            ca,
            infos,
        )
    }

    /// Decode `M_ME_NA_1`, `M_ME_TA_1`, `M_ME_TD_1` or `M_ME_ND_1`.
    pub fn get_measured_value_normal(&self) -> Result<Vec<MeasuredValueNormalInfo>> {
        let seq = self.variable().is_sequence;
        let n = self.variable().number as usize;
        let mut r = self.reader();
        let mut out = Vec::with_capacity(n);
        let mut ioa = 0;
        for i in 0..n {
            ioa = r.next_obj_addr(i, seq, ioa)?;
            let value = r.normalize()?;
            let (qds, time) = match self.type_id() {
                TypeId::M_ME_NA_1 => (QualityDescriptor(r.byte()?), None),
                TypeId::M_ME_TA_1 => {
                    let q = QualityDescriptor(r.byte()?);
                    (q, r.cp24time2a()?)
                }
                TypeId::M_ME_TD_1 => {
                    let q = QualityDescriptor(r.byte()?);
                    (q, r.cp56time2a()?)
                }
                TypeId::M_ME_ND_1 => (QualityDescriptor::GOOD, None),
                _ => return Err(Error::TypeIdNotMatch),
            };
            out.push(MeasuredValueNormalInfo {
                ioa,
                value,
                qds,
                time,
            });
        }
        Ok(out)
    }

    // -- measured value, scaled ------------------------------------------

    /// Build `M_ME_NB_1`, `M_ME_TB_1` or `M_ME_TE_1`: a scaled measured value.
    ///
    /// See companion standard 101, subclasses 7.3.1.11, 7.3.1.12 and 7.3.1.27.
    pub fn measured_value_scaled_with_type(
        params: Params,
        type_id: TypeId,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueScaledInfo],
    ) -> Result<Asdu> {
        let mut u = start(params, type_id, is_sequence, coa, ca, infos.len())?;
        for (i, v) in infos.iter().enumerate() {
            let mut e = u.encoder();
            if writes_addr(is_sequence, i) {
                e.info_obj_addr(v.ioa)?;
            }
            e.scaled(v.value).byte(v.qds.0);
            match type_id {
                TypeId::M_ME_NB_1 => {}
                TypeId::M_ME_TB_1 => {
                    e.cp24time2a(v.time);
                }
                TypeId::M_ME_TE_1 => {
                    e.cp56time2a(v.time);
                }
                _ => return Err(Error::TypeIdNotMatch),
            }
        }
        Ok(u)
    }

    /// Build `M_ME_NB_1`: scaled measured value without time tag.
    pub fn measured_value_scaled(
        params: Params,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueScaledInfo],
    ) -> Result<Asdu> {
        check_measured_cause(coa)?;
        Self::measured_value_scaled_with_type(
            params,
            TypeId::M_ME_NB_1,
            is_sequence,
            coa,
            ca,
            infos,
        )
    }

    /// Build `M_ME_TB_1`: scaled measured value with a CP24Time2a time tag.
    pub fn measured_value_scaled_cp24time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueScaledInfo],
    ) -> Result<Asdu> {
        check_spontaneous_or_request(coa)?;
        Self::measured_value_scaled_with_type(params, TypeId::M_ME_TB_1, false, coa, ca, infos)
    }

    /// Build `M_ME_TE_1`: scaled measured value with a CP56Time2a time tag.
    pub fn measured_value_scaled_cp56time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueScaledInfo],
    ) -> Result<Asdu> {
        check_spontaneous_or_request(coa)?;
        Self::measured_value_scaled_with_type(params, TypeId::M_ME_TE_1, false, coa, ca, infos)
    }

    /// Decode `M_ME_NB_1`, `M_ME_TB_1` or `M_ME_TE_1`.
    pub fn get_measured_value_scaled(&self) -> Result<Vec<MeasuredValueScaledInfo>> {
        let seq = self.variable().is_sequence;
        let n = self.variable().number as usize;
        let mut r = self.reader();
        let mut out = Vec::with_capacity(n);
        let mut ioa = 0;
        for i in 0..n {
            ioa = r.next_obj_addr(i, seq, ioa)?;
            let value = r.scaled()?;
            let qds = QualityDescriptor(r.byte()?);
            let time = match self.type_id() {
                TypeId::M_ME_NB_1 => None,
                TypeId::M_ME_TB_1 => r.cp24time2a()?,
                TypeId::M_ME_TE_1 => r.cp56time2a()?,
                _ => return Err(Error::TypeIdNotMatch),
            };
            out.push(MeasuredValueScaledInfo {
                ioa,
                value,
                qds,
                time,
            });
        }
        Ok(out)
    }

    // -- measured value, short float -------------------------------------

    /// Build `M_ME_NC_1`, `M_ME_TC_1` or `M_ME_TF_1`: a short floating point
    /// measured value.
    ///
    /// Only the OV, NT and IV quality bits are transmitted by this family
    /// (mask `0xf1`), per the standard.
    /// See companion standard 101, subclasses 7.3.1.13, 7.3.1.14 and 7.3.1.28.
    pub fn measured_value_float_with_type(
        params: Params,
        type_id: TypeId,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueFloatInfo],
    ) -> Result<Asdu> {
        let mut u = start(params, type_id, is_sequence, coa, ca, infos.len())?;
        for (i, v) in infos.iter().enumerate() {
            let mut e = u.encoder();
            if writes_addr(is_sequence, i) {
                e.info_obj_addr(v.ioa)?;
            }
            e.f32(v.value).byte(v.qds.0 & 0xf1);
            match type_id {
                TypeId::M_ME_NC_1 => {}
                TypeId::M_ME_TC_1 => {
                    e.cp24time2a(v.time);
                }
                TypeId::M_ME_TF_1 => {
                    e.cp56time2a(v.time);
                }
                _ => return Err(Error::TypeIdNotMatch),
            }
        }
        Ok(u)
    }

    /// Build `M_ME_NC_1`: short floating point measured value without time tag.
    pub fn measured_value_float(
        params: Params,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueFloatInfo],
    ) -> Result<Asdu> {
        check_measured_cause(coa)?;
        Self::measured_value_float_with_type(params, TypeId::M_ME_NC_1, is_sequence, coa, ca, infos)
    }

    /// Build `M_ME_TC_1`: short float measured value with a CP24Time2a time tag.
    pub fn measured_value_float_cp24time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueFloatInfo],
    ) -> Result<Asdu> {
        check_spontaneous_or_request(coa)?;
        Self::measured_value_float_with_type(params, TypeId::M_ME_TC_1, false, coa, ca, infos)
    }

    /// Build `M_ME_TF_1`: short float measured value with a CP56Time2a time tag.
    pub fn measured_value_float_cp56time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[MeasuredValueFloatInfo],
    ) -> Result<Asdu> {
        check_spontaneous_or_request(coa)?;
        Self::measured_value_float_with_type(params, TypeId::M_ME_TF_1, false, coa, ca, infos)
    }

    /// Decode `M_ME_NC_1`, `M_ME_TC_1` or `M_ME_TF_1`.
    pub fn get_measured_value_float(&self) -> Result<Vec<MeasuredValueFloatInfo>> {
        let seq = self.variable().is_sequence;
        let n = self.variable().number as usize;
        let mut r = self.reader();
        let mut out = Vec::with_capacity(n);
        let mut ioa = 0;
        for i in 0..n {
            ioa = r.next_obj_addr(i, seq, ioa)?;
            let value = r.f32()?;
            let qds = QualityDescriptor(r.byte()? & 0xf1);
            let time = match self.type_id() {
                TypeId::M_ME_NC_1 => None,
                TypeId::M_ME_TC_1 => r.cp24time2a()?,
                TypeId::M_ME_TF_1 => r.cp56time2a()?,
                _ => return Err(Error::TypeIdNotMatch),
            };
            out.push(MeasuredValueFloatInfo {
                ioa,
                value,
                qds,
                time,
            });
        }
        Ok(out)
    }

    // -- integrated totals -----------------------------------------------

    /// Build `M_IT_NA_1`, `M_IT_TA_1` or `M_IT_TB_1`: integrated totals.
    ///
    /// See companion standard 101, subclasses 7.3.1.15, 7.3.1.16 and 7.3.1.29.
    pub fn integrated_totals_with_type(
        params: Params,
        type_id: TypeId,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BinaryCounterReadingInfo],
    ) -> Result<Asdu> {
        let mut u = start(params, type_id, is_sequence, coa, ca, infos.len())?;
        for (i, v) in infos.iter().enumerate() {
            let mut e = u.encoder();
            if writes_addr(is_sequence, i) {
                e.info_obj_addr(v.ioa)?;
            }
            e.binary_counter_reading(v.value);
            match type_id {
                TypeId::M_IT_NA_1 => {}
                TypeId::M_IT_TA_1 => {
                    e.cp24time2a(v.time);
                }
                TypeId::M_IT_TB_1 => {
                    e.cp56time2a(v.time);
                }
                _ => return Err(Error::TypeIdNotMatch),
            }
        }
        Ok(u)
    }

    /// Build `M_IT_NA_1`: integrated totals without time tag.
    ///
    /// Permitted causes: `Spontaneous` and the counter request causes.
    pub fn integrated_totals(
        params: Params,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BinaryCounterReadingInfo],
    ) -> Result<Asdu> {
        check_counter_cause(coa)?;
        Self::integrated_totals_with_type(params, TypeId::M_IT_NA_1, is_sequence, coa, ca, infos)
    }

    /// Build `M_IT_TA_1`: integrated totals with a CP24Time2a time tag.
    pub fn integrated_totals_cp24time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BinaryCounterReadingInfo],
    ) -> Result<Asdu> {
        check_counter_cause(coa)?;
        Self::integrated_totals_with_type(params, TypeId::M_IT_TA_1, false, coa, ca, infos)
    }

    /// Build `M_IT_TB_1`: integrated totals with a CP56Time2a time tag.
    pub fn integrated_totals_cp56time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[BinaryCounterReadingInfo],
    ) -> Result<Asdu> {
        check_counter_cause(coa)?;
        Self::integrated_totals_with_type(params, TypeId::M_IT_TB_1, false, coa, ca, infos)
    }

    /// Decode `M_IT_NA_1`, `M_IT_TA_1` or `M_IT_TB_1`.
    pub fn get_integrated_totals(&self) -> Result<Vec<BinaryCounterReadingInfo>> {
        let seq = self.variable().is_sequence;
        let n = self.variable().number as usize;
        let mut r = self.reader();
        let mut out = Vec::with_capacity(n);
        let mut ioa = 0;
        for i in 0..n {
            ioa = r.next_obj_addr(i, seq, ioa)?;
            let value = r.binary_counter_reading()?;
            let time = match self.type_id() {
                TypeId::M_IT_NA_1 => None,
                TypeId::M_IT_TA_1 => r.cp24time2a()?,
                TypeId::M_IT_TB_1 => r.cp56time2a()?,
                _ => return Err(Error::TypeIdNotMatch),
            };
            out.push(BinaryCounterReadingInfo { ioa, value, time });
        }
        Ok(out)
    }

    // -- protection equipment --------------------------------------------

    /// Build `M_EP_TA_1` or `M_EP_TD_1`: event of protection equipment.
    ///
    /// Only `Spontaneous` is permitted.
    /// See companion standard 101, subclasses 7.3.1.17 and 7.3.1.30.
    pub fn event_of_protection_equipment_with_type(
        params: Params,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[EventOfProtectionEquipmentInfo],
    ) -> Result<Asdu> {
        if coa.cause != Cause::SPONTANEOUS {
            return Err(Error::CmdCause);
        }
        let mut u = start(params, type_id, false, coa, ca, infos.len())?;
        for v in infos {
            let mut e = u.encoder();
            e.info_obj_addr(v.ioa)?;
            e.byte(v.event.value() | (v.qdp.0 & 0xf8)).cp16time2a(v.msec);
            match type_id {
                TypeId::M_EP_TA_1 => {
                    e.cp24time2a(v.time);
                }
                TypeId::M_EP_TD_1 => {
                    e.cp56time2a(v.time);
                }
                _ => return Err(Error::TypeIdNotMatch),
            }
        }
        Ok(u)
    }

    /// Build `M_EP_TA_1`: event of protection equipment with a CP24Time2a time tag.
    pub fn event_of_protection_equipment_cp24time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[EventOfProtectionEquipmentInfo],
    ) -> Result<Asdu> {
        Self::event_of_protection_equipment_with_type(params, TypeId::M_EP_TA_1, coa, ca, infos)
    }

    /// Build `M_EP_TD_1`: event of protection equipment with a CP56Time2a time tag.
    pub fn event_of_protection_equipment_cp56time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[EventOfProtectionEquipmentInfo],
    ) -> Result<Asdu> {
        Self::event_of_protection_equipment_with_type(params, TypeId::M_EP_TD_1, coa, ca, infos)
    }

    /// Decode `M_EP_TA_1` or `M_EP_TD_1`.
    pub fn get_event_of_protection_equipment(
        &self,
    ) -> Result<Vec<EventOfProtectionEquipmentInfo>> {
        let seq = self.variable().is_sequence;
        let n = self.variable().number as usize;
        let mut r = self.reader();
        let mut out = Vec::with_capacity(n);
        let mut ioa = 0;
        for i in 0..n {
            ioa = r.next_obj_addr(i, seq, ioa)?;
            let value = r.byte()?;
            let msec = r.cp16time2a()?;
            let time = match self.type_id() {
                TypeId::M_EP_TA_1 => r.cp24time2a()?,
                TypeId::M_EP_TD_1 => r.cp56time2a()?,
                _ => return Err(Error::TypeIdNotMatch),
            };
            out.push(EventOfProtectionEquipmentInfo {
                ioa,
                event: SingleEvent::parse(value),
                qdp: QualityDescriptorProtection(value & 0xf8),
                msec,
                time,
            });
        }
        Ok(out)
    }

    /// Build `M_EP_TB_1` or `M_EP_TE_1`: packed start events of protection
    /// equipment. Always exactly one information object.
    ///
    /// See companion standard 101, subclasses 7.3.1.18 and 7.3.1.31.
    pub fn packed_start_events_with_type(
        params: Params,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: PackedStartEventsOfProtectionEquipmentInfo,
    ) -> Result<Asdu> {
        if coa.cause != Cause::SPONTANEOUS {
            return Err(Error::CmdCause);
        }
        let mut u = start(params, type_id, false, coa, ca, 1)?;
        let mut e = u.encoder();
        e.info_obj_addr(info.ioa)?;
        e.byte(info.event.0)
            .byte(info.qdp.0 & 0xf8)
            .cp16time2a(info.msec);
        match type_id {
            TypeId::M_EP_TB_1 => {
                e.cp24time2a(info.time);
            }
            TypeId::M_EP_TE_1 => {
                e.cp56time2a(info.time);
            }
            _ => return Err(Error::TypeIdNotMatch),
        }
        Ok(u)
    }

    /// Build `M_EP_TB_1`: packed start events with a CP24Time2a time tag.
    pub fn packed_start_events_cp24time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: PackedStartEventsOfProtectionEquipmentInfo,
    ) -> Result<Asdu> {
        Self::packed_start_events_with_type(params, TypeId::M_EP_TB_1, coa, ca, info)
    }

    /// Build `M_EP_TE_1`: packed start events with a CP56Time2a time tag.
    pub fn packed_start_events_cp56time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: PackedStartEventsOfProtectionEquipmentInfo,
    ) -> Result<Asdu> {
        Self::packed_start_events_with_type(params, TypeId::M_EP_TE_1, coa, ca, info)
    }

    /// Decode `M_EP_TB_1` or `M_EP_TE_1`.
    pub fn get_packed_start_events(
        &self,
    ) -> Result<PackedStartEventsOfProtectionEquipmentInfo> {
        if self.variable().is_sequence || self.variable().number != 1 {
            return Err(Error::InfoObjIndexFit);
        }
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        let event = StartEvent(r.byte()?);
        let qdp = QualityDescriptorProtection(r.byte()? & 0xf8);
        let msec = r.cp16time2a()?;
        let time = match self.type_id() {
            TypeId::M_EP_TB_1 => r.cp24time2a()?,
            TypeId::M_EP_TE_1 => r.cp56time2a()?,
            _ => return Err(Error::TypeIdNotMatch),
        };
        Ok(PackedStartEventsOfProtectionEquipmentInfo {
            ioa,
            event,
            qdp,
            msec,
            time,
        })
    }

    /// Build `M_EP_TC_1` or `M_EP_TF_1`: packed output circuit information of
    /// protection equipment. Always exactly one information object.
    ///
    /// See companion standard 101, subclasses 7.3.1.19 and 7.3.1.32.
    pub fn packed_output_circuit_info_with_type(
        params: Params,
        type_id: TypeId,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: PackedOutputCircuitInfoInfo,
    ) -> Result<Asdu> {
        if coa.cause != Cause::SPONTANEOUS {
            return Err(Error::CmdCause);
        }
        let mut u = start(params, type_id, false, coa, ca, 1)?;
        let mut e = u.encoder();
        e.info_obj_addr(info.ioa)?;
        e.byte(info.oci.0)
            .byte(info.qdp.0 & 0xf8)
            .cp16time2a(info.msec);
        match type_id {
            TypeId::M_EP_TC_1 => {
                e.cp24time2a(info.time);
            }
            TypeId::M_EP_TF_1 => {
                e.cp56time2a(info.time);
            }
            _ => return Err(Error::TypeIdNotMatch),
        }
        Ok(u)
    }

    /// Build `M_EP_TC_1`: packed output circuit information, CP24Time2a.
    pub fn packed_output_circuit_info_cp24time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: PackedOutputCircuitInfoInfo,
    ) -> Result<Asdu> {
        Self::packed_output_circuit_info_with_type(params, TypeId::M_EP_TC_1, coa, ca, info)
    }

    /// Build `M_EP_TF_1`: packed output circuit information, CP56Time2a.
    pub fn packed_output_circuit_info_cp56time2a(
        params: Params,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        info: PackedOutputCircuitInfoInfo,
    ) -> Result<Asdu> {
        Self::packed_output_circuit_info_with_type(params, TypeId::M_EP_TF_1, coa, ca, info)
    }

    /// Decode `M_EP_TC_1` or `M_EP_TF_1`.
    pub fn get_packed_output_circuit_info(&self) -> Result<PackedOutputCircuitInfoInfo> {
        if self.variable().is_sequence || self.variable().number != 1 {
            return Err(Error::InfoObjIndexFit);
        }
        let mut r = self.reader();
        let ioa = r.info_obj_addr()?;
        let oci = OutputCircuitInfo(r.byte()?);
        let qdp = QualityDescriptorProtection(r.byte()? & 0xf8);
        let msec = r.cp16time2a()?;
        let time = match self.type_id() {
            TypeId::M_EP_TC_1 => r.cp24time2a()?,
            TypeId::M_EP_TF_1 => r.cp56time2a()?,
            _ => return Err(Error::TypeIdNotMatch),
        };
        Ok(PackedOutputCircuitInfoInfo {
            ioa,
            oci,
            qdp,
            msec,
            time,
        })
    }

    // -- packed single point with SCD ------------------------------------

    /// Build `M_PS_NA_1`: packed single-point information with status change
    /// detection.
    ///
    /// See companion standard 101, subclass 7.3.1.20.
    pub fn packed_single_point_with_scd(
        params: Params,
        is_sequence: bool,
        coa: CauseOfTransmission,
        ca: CommonAddr,
        infos: &[PackedSinglePointWithScdInfo],
    ) -> Result<Asdu> {
        check_untagged_cause(coa)?;
        let mut u = start(
            params,
            TypeId::M_PS_NA_1,
            is_sequence,
            coa,
            ca,
            infos.len(),
        )?;
        for (i, v) in infos.iter().enumerate() {
            let mut e = u.encoder();
            if writes_addr(is_sequence, i) {
                e.info_obj_addr(v.ioa)?;
            }
            e.scd(v.scd).byte(v.qds.0);
        }
        Ok(u)
    }

    /// Decode `M_PS_NA_1`.
    pub fn get_packed_single_point_with_scd(&self) -> Result<Vec<PackedSinglePointWithScdInfo>> {
        let seq = self.variable().is_sequence;
        let n = self.variable().number as usize;
        let mut r = self.reader();
        let mut out = Vec::with_capacity(n);
        let mut ioa = 0;
        for i in 0..n {
            ioa = r.next_obj_addr(i, seq, ioa)?;
            let scd = r.scd()?;
            let qds = QualityDescriptor(r.byte()?);
            out.push(PackedSinglePointWithScdInfo { ioa, scd, qds });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::params::{PARAMS_STANDARD_101, PARAMS_WIDE};
    use chrono::TimeZone as _;

    fn coa(c: Cause) -> CauseOfTransmission {
        CauseOfTransmission::new(c)
    }

    fn t() -> Option<DateTime<Utc>> {
        Some(Utc.with_ymd_and_hms(2019, 6, 5, 21, 17, 45).unwrap() + chrono::Duration::milliseconds(678))
    }

    #[test]
    fn single_encodes_value_and_quality_in_one_octet() {
        let a = Asdu::single(
            PARAMS_STANDARD_101,
            false,
            coa(Cause::SPONTANEOUS),
            1,
            &[SinglePointInfo {
                ioa: 0x1234,
                value: true,
                qds: QualityDescriptor::INVALID,
                time: None,
            }],
        )
        .unwrap();
        assert_eq!(a.info_obj, vec![0x34, 0x12, 0x81]);
        let got = a.get_single_point().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].ioa, 0x1234);
        assert!(got[0].value);
        assert_eq!(got[0].qds, QualityDescriptor::INVALID);
    }

    #[test]
    fn single_sequence_writes_only_the_first_address() {
        let infos: Vec<_> = (0..3)
            .map(|i| SinglePointInfo::new(100 + i, i % 2 == 0))
            .collect();
        let a = Asdu::single(PARAMS_WIDE, true, coa(Cause::SPONTANEOUS), 1, &infos).unwrap();
        // 3 address octets + 3 value octets.
        assert_eq!(a.info_obj.len(), 6);
        assert!(a.variable().is_sequence);
        assert_eq!(a.get_single_point().unwrap(), infos);
    }

    #[test]
    fn single_rejects_causes_outside_the_standard() {
        assert_eq!(
            Asdu::single(
                PARAMS_WIDE,
                false,
                coa(Cause::ACTIVATION),
                1,
                &[SinglePointInfo::new(1, true)]
            ),
            Err(Error::CmdCause)
        );
        assert_eq!(
            Asdu::single_cp56time2a(
                PARAMS_WIDE,
                coa(Cause::BACKGROUND),
                1,
                &[SinglePointInfo::new(1, true)]
            ),
            Err(Error::CmdCause)
        );
        assert_eq!(
            Asdu::single(PARAMS_WIDE, false, coa(Cause::SPONTANEOUS), 1, &[]),
            Err(Error::NotAnyObjInfo)
        );
    }

    #[test]
    fn time_tagged_families_round_trip() {
        let time = t();
        let a = Asdu::single_cp56time2a(
            PARAMS_WIDE,
            coa(Cause::SPONTANEOUS),
            1,
            &[SinglePointInfo {
                ioa: 100,
                value: true,
                qds: QualityDescriptor::GOOD,
                time,
            }],
        )
        .unwrap();
        assert_eq!(a.get_single_point().unwrap()[0].time, time);

        let a = Asdu::double_cp56time2a(
            PARAMS_WIDE,
            coa(Cause::SPONTANEOUS),
            1,
            &[DoublePointInfo {
                ioa: 101,
                value: DoublePoint::DeterminedOn,
                qds: QualityDescriptor::GOOD,
                time,
            }],
        )
        .unwrap();
        let got = a.get_double_point().unwrap();
        assert_eq!(got[0].value, DoublePoint::DeterminedOn);
        assert_eq!(got[0].time, time);
    }

    #[test]
    fn step_position_round_trips_with_transient() {
        let info = StepPositionInfo {
            ioa: 200,
            value: StepPosition {
                val: -17,
                has_transient: true,
            },
            qds: QualityDescriptor::BLOCKED,
            time: None,
        };
        let a = Asdu::step(PARAMS_WIDE, false, coa(Cause::SPONTANEOUS), 1, &[info]).unwrap();
        assert_eq!(a.get_step_position().unwrap()[0], info);
    }

    #[test]
    fn measured_float_masks_quality_to_ov_nt_iv() {
        let a = Asdu::measured_value_float(
            PARAMS_WIDE,
            false,
            coa(Cause::PERIODIC),
            1,
            &[MeasuredValueFloatInfo {
                ioa: 400,
                value: 22.5,
                // BLOCKED (0x10) and SUBSTITUTED (0x20) are not carried here.
                qds: QualityDescriptor(0xff),
                time: None,
            }],
        )
        .unwrap();
        let got = a.get_measured_value_float().unwrap();
        assert_eq!(got[0].value, 22.5);
        assert_eq!(got[0].qds, QualityDescriptor(0xf1));
    }

    #[test]
    fn measured_normal_without_quality_omits_the_octet() {
        let infos = [MeasuredValueNormalInfo {
            ioa: 300,
            value: Normalize(16384),
            qds: QualityDescriptor::INVALID,
            time: None,
        }];
        let a =
            Asdu::measured_value_normal_no_quality(PARAMS_WIDE, false, coa(Cause::PERIODIC), 1, &infos)
                .unwrap();
        // 3 address + 2 value octets only.
        assert_eq!(a.info_obj.len(), 5);
        let got = a.get_measured_value_normal().unwrap();
        assert_eq!(got[0].value.f64(), 0.5);
        assert_eq!(got[0].qds, QualityDescriptor::GOOD);
    }

    #[test]
    fn integrated_totals_accept_only_counter_causes() {
        let info = BinaryCounterReadingInfo {
            ioa: 500,
            value: BinaryCounterReading {
                counter_reading: 987_654,
                seq_number: 3,
                has_carry: true,
                is_adjusted: false,
                is_invalid: false,
            },
            time: None,
        };
        assert!(
            Asdu::integrated_totals(
                PARAMS_WIDE,
                false,
                coa(Cause::REQUEST_BY_GENERAL_COUNTER),
                1,
                &[info]
            )
            .is_ok()
        );
        assert_eq!(
            Asdu::integrated_totals(PARAMS_WIDE, false, coa(Cause::PERIODIC), 1, &[info]),
            Err(Error::CmdCause)
        );
        let a =
            Asdu::integrated_totals(PARAMS_WIDE, false, coa(Cause::SPONTANEOUS), 1, &[info]).unwrap();
        assert_eq!(a.get_integrated_totals().unwrap()[0], info);
    }

    #[test]
    fn protection_events_require_spontaneous() {
        let info = EventOfProtectionEquipmentInfo {
            ioa: 600,
            event: SingleEvent::DeterminedOn,
            qdp: QualityDescriptorProtection::INVALID,
            msec: 1234,
            time: t(),
        };
        assert_eq!(
            Asdu::event_of_protection_equipment_cp56time2a(
                PARAMS_WIDE,
                coa(Cause::PERIODIC),
                1,
                &[info]
            ),
            Err(Error::CmdCause)
        );
        let a = Asdu::event_of_protection_equipment_cp56time2a(
            PARAMS_WIDE,
            coa(Cause::SPONTANEOUS),
            1,
            &[info],
        )
        .unwrap();
        assert_eq!(a.get_event_of_protection_equipment().unwrap()[0], info);
    }

    #[test]
    fn packed_start_events_round_trip() {
        let info = PackedStartEventsOfProtectionEquipmentInfo {
            ioa: 700,
            event: StartEvent::GENERAL_START | StartEvent::START_L2,
            qdp: QualityDescriptorProtection::BLOCKED,
            msec: 42,
            time: t(),
        };
        let a =
            Asdu::packed_start_events_cp56time2a(PARAMS_WIDE, coa(Cause::SPONTANEOUS), 1, info)
                .unwrap();
        assert_eq!(a.get_packed_start_events().unwrap(), info);
    }

    #[test]
    fn packed_output_circuit_info_round_trips() {
        let info = PackedOutputCircuitInfoInfo {
            ioa: 701,
            oci: OutputCircuitInfo::GENERAL_COMMAND | OutputCircuitInfo::COMMAND_L3,
            qdp: QualityDescriptorProtection::GOOD,
            msec: 7,
            time: t(),
        };
        let a = Asdu::packed_output_circuit_info_cp56time2a(
            PARAMS_WIDE,
            coa(Cause::SPONTANEOUS),
            1,
            info,
        )
        .unwrap();
        assert_eq!(a.get_packed_output_circuit_info().unwrap(), info);
    }

    #[test]
    fn packed_single_point_with_scd_round_trips() {
        let infos = [
            PackedSinglePointWithScdInfo {
                ioa: 800,
                scd: StatusAndStatusChangeDetection(0xdead_beef),
                qds: QualityDescriptor::GOOD,
            },
            PackedSinglePointWithScdInfo {
                ioa: 801,
                scd: StatusAndStatusChangeDetection(0x0000_ffff),
                qds: QualityDescriptor::NOT_TOPICAL,
            },
        ];
        let a = Asdu::packed_single_point_with_scd(
            PARAMS_WIDE,
            false,
            coa(Cause::INTERROGATED_BY_STATION),
            1,
            &infos,
        )
        .unwrap();
        assert_eq!(a.get_packed_single_point_with_scd().unwrap(), infos);
        assert_eq!(infos[0].scd.status(), 0xbeef);
        assert_eq!(infos[0].scd.change_detection(), 0xdead);
    }

    #[test]
    fn every_family_survives_a_wire_round_trip() {
        let p = PARAMS_WIDE;
        let asdus = [
            Asdu::single(p, false, coa(Cause::SPONTANEOUS), 1, &[SinglePointInfo::new(1, true)])
                .unwrap(),
            Asdu::double(
                p,
                false,
                coa(Cause::SPONTANEOUS),
                1,
                &[DoublePointInfo {
                    ioa: 2,
                    value: DoublePoint::DeterminedOff,
                    ..Default::default()
                }],
            )
            .unwrap(),
            Asdu::step(p, false, coa(Cause::SPONTANEOUS), 1, &[StepPositionInfo::default()])
                .unwrap(),
            Asdu::bitstring32(
                p,
                false,
                coa(Cause::SPONTANEOUS),
                1,
                &[BitString32Info {
                    ioa: 4,
                    value: 0x1234_5678,
                    ..Default::default()
                }],
            )
            .unwrap(),
            Asdu::measured_value_normal(
                p,
                false,
                coa(Cause::PERIODIC),
                1,
                &[MeasuredValueNormalInfo::default()],
            )
            .unwrap(),
            Asdu::measured_value_scaled(
                p,
                false,
                coa(Cause::PERIODIC),
                1,
                &[MeasuredValueScaledInfo {
                    ioa: 6,
                    value: -1234,
                    ..Default::default()
                }],
            )
            .unwrap(),
            Asdu::measured_value_float(
                p,
                false,
                coa(Cause::PERIODIC),
                1,
                &[MeasuredValueFloatInfo {
                    ioa: 7,
                    value: 3.25,
                    ..Default::default()
                }],
            )
            .unwrap(),
            Asdu::integrated_totals(
                p,
                false,
                coa(Cause::SPONTANEOUS),
                1,
                &[BinaryCounterReadingInfo::default()],
            )
            .unwrap(),
        ];
        for a in asdus {
            let raw = a.marshal_binary().unwrap();
            let b = Asdu::unmarshal_binary(p, &raw).unwrap();
            assert_eq!(a, b, "round trip failed for {}", a.type_id());
        }
    }
}
