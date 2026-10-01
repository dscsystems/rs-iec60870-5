// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The IEC 60870-5-101/-104 application layer.
//!
//! An ASDU (Application Service Data Unit) is one application message:
//!
//! ```text
//! ASDU = data unit identifier + information objects
//!        | type ID | variable structure | cause of transmission | common address | information objects... |
//! bytes |    1    |         1          |        1..2           |      1..2      |          n             |
//! ```
//!
//! * [`Params`] fixes the octet widths of the identifier fields and **must be
//!   identical on both peers**. Use [`PARAMS_WIDE`] for IEC 104 and
//!   [`PARAMS_STANDARD_101`] for IEC 101.
//! * [`Asdu`] carries the identifier plus the serialised information objects.
//!   Its `Asdu::single`-style constructors build one, and its `get_*` methods
//!   decode one — non-destructively, so they may be called in any order and
//!   any number of times.
//! * [`Connect`] is what every transport endpoint implements; [`ConnectExt`]
//!   adds a build-and-send method per ASDU family.
//!
//! # Example
//!
//! ```
//! use rs_iec60870_5::asdu::*;
//!
//! # fn main() -> rs_iec60870_5::Result<()> {
//! let a = Asdu::measured_value_float(
//!     PARAMS_WIDE,
//!     false,
//!     CauseOfTransmission::new(Cause::PERIODIC),
//!     1,
//!     &[MeasuredValueFloatInfo { ioa: 400, value: 22.5, ..Default::default() }],
//! )?;
//!
//! let wire = a.marshal_binary()?;
//! let back = Asdu::unmarshal_binary(PARAMS_WIDE, &wire)?;
//! assert_eq!(back.get_measured_value_float()?[0].value, 22.5);
//! # Ok(())
//! # }
//! ```

mod codec;
mod connect;
mod cpara;
mod cproc;
mod csys;
mod display;
mod filet;
mod identifier;
mod info;
mod mproc;
mod params;
pub mod time;

pub use codec::{ASDU_SIZE_MAX, Asdu, Encoder, InfoObjReader};
pub use connect::{Connect, ConnectExt};
pub use cpara::{
    ParameterActivationInfo, ParameterFloatInfo, ParameterNormalInfo, ParameterScaledInfo,
};
pub use cproc::{
    BitsString32CommandInfo, DoubleCommandInfo, SetpointCommandFloatInfo,
    SetpointCommandNormalInfo, SetpointCommandScaledInfo, SingleCommandInfo, StepCommandInfo,
};
pub use filet::{
    AckFileOrSectionInfo, AckFileOrSectionQualifier, AfqAction, CallOrSelectFileInfo,
    DirectoryInfo, FileError, FileReadyInfo, FileReadyQualifier, LENGTH_OF_FILE_MAX,
    LastSectionOrSegmentInfo, LastSectionQualifier, NameOfFile, ScqAction, SectionReadyInfo,
    SectionReadyQualifier, SegmentInfo, SelectAndCallQualifier, StatusOfFile, file_checksum,
};
pub use identifier::{
    Cause, CauseOfTransmission, CommonAddr, GLOBAL_COMMON_ADDR, INVALID_COMMON_ADDR, Identifier,
    OriginAddr, TypeId, VariableStruct,
};
pub use info::{
    BinaryCounterReading, CauseOfInitial, CoiCause, DoubleCommand, DoublePoint,
    INFO_OBJ_ADDR_IRRELEVANT, InfoObjAddr, Normalize, OutputCircuitInfo, QccFreeze, QccRequest,
    QocQual, QosQual, QpmCategory, QualifierCountCall, QualifierOfCommand,
    QualifierOfInterrogation, QualifierOfParameterAct, QualifierOfParameterMv,
    QualifierOfResetProcessCmd, QualifierOfSetpointCmd, QualityDescriptor,
    QualityDescriptorProtection, SingleCommand, SingleEvent, SinglePoint, StartEvent, StepCommand,
    StepPosition, StatusAndStatusChangeDetection, FBP_TEST_WORD,
};
pub use mproc::{
    BinaryCounterReadingInfo, BitString32Info, DoublePointInfo, EventOfProtectionEquipmentInfo,
    MeasuredValueFloatInfo, MeasuredValueNormalInfo, MeasuredValueScaledInfo,
    PackedOutputCircuitInfoInfo, PackedSinglePointWithScdInfo,
    PackedStartEventsOfProtectionEquipmentInfo, SinglePointInfo, StepPositionInfo,
};
pub use time::TimeTagFlags;
pub use params::{
    PARAMS_NARROW, PARAMS_STANDARD_101, PARAMS_STANDARD_104, PARAMS_WIDE, Params, TimeZone,
};
