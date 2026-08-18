// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Information elements: addresses, values, quality descriptors and qualifiers.
//!
//! See companion standard 101, subclass 7.2.6.

use std::fmt;

/// Information object address. See companion standard 101, subclass 7.2.5.
///
/// The width on the wire is controlled by `Params::info_obj_addr_size`:
/// width 1 gives `<1..=255>`, width 2 `<1..=65535>`, width 3 `<1..=16777215>`.
/// Zero means "irrelevant" and is reserved for system commands; real points
/// start at 1.
pub type InfoObjAddr = u32;

/// Zero: the information object address is irrelevant.
pub const INFO_OBJ_ADDR_IRRELEVANT: InfoObjAddr = 0;

/// Single-point information. See companion standard 101, subclass 7.2.6.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum SinglePoint {
    /// 0: off
    Off = 0,
    /// 1: on
    On = 1,
}

impl SinglePoint {
    /// Encode to the two-state value.
    pub const fn value(self) -> u8 {
        self as u8
    }

    /// Decode from the low bit of an octet.
    pub const fn parse(b: u8) -> Self {
        if b & 0x01 == 0x01 {
            SinglePoint::On
        } else {
            SinglePoint::Off
        }
    }
}

impl From<bool> for SinglePoint {
    fn from(v: bool) -> Self {
        if v {
            SinglePoint::On
        } else {
            SinglePoint::Off
        }
    }
}

impl From<SinglePoint> for bool {
    fn from(v: SinglePoint) -> Self {
        v == SinglePoint::On
    }
}

impl fmt::Display for SinglePoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SinglePoint::Off => "Off",
            SinglePoint::On => "On",
        })
    }
}

/// Double-point information. See companion standard 101, subclass 7.2.6.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum DoublePoint {
    /// 0: indeterminate or intermediate state
    #[default]
    IndeterminateOrIntermediate = 0,
    /// 1: determined state off
    DeterminedOff = 1,
    /// 2: determined state on
    DeterminedOn = 2,
    /// 3: indeterminate state
    Indeterminate = 3,
}

impl DoublePoint {
    /// Encode to the two-bit value.
    pub const fn value(self) -> u8 {
        self as u8
    }

    /// Decode from the low two bits of an octet.
    pub const fn parse(b: u8) -> Self {
        match b & 0x03 {
            0 => DoublePoint::IndeterminateOrIntermediate,
            1 => DoublePoint::DeterminedOff,
            2 => DoublePoint::DeterminedOn,
            _ => DoublePoint::Indeterminate,
        }
    }
}

impl fmt::Display for DoublePoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DoublePoint::IndeterminateOrIntermediate => "IndeterminateOrIntermediate",
            DoublePoint::DeterminedOff => "DeterminedOff",
            DoublePoint::DeterminedOn => "DeterminedOn",
            DoublePoint::Indeterminate => "Indeterminate",
        })
    }
}

/// Quality descriptor flags attached to measured values.
///
/// See companion standard 101, subclass 7.2.6.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct QualityDescriptor(pub u8);

impl QualityDescriptor {
    /// No flags: the value is good.
    pub const GOOD: QualityDescriptor = QualityDescriptor(0);
    /// Bit 0: the value is beyond a predefined range.
    pub const OVERFLOW: QualityDescriptor = QualityDescriptor(1 << 0);
    /// Bit 4: the value is blocked for transmission and holds its last value.
    pub const BLOCKED: QualityDescriptor = QualityDescriptor(1 << 4);
    /// Bit 5: the value was entered by an operator rather than acquired.
    pub const SUBSTITUTED: QualityDescriptor = QualityDescriptor(1 << 5);
    /// Bit 6: the most recent update was unsuccessful.
    pub const NOT_TOPICAL: QualityDescriptor = QualityDescriptor(1 << 6);
    /// Bit 7: the value was incorrectly acquired.
    pub const INVALID: QualityDescriptor = QualityDescriptor(1 << 7);

    /// True when no quality flag is set.
    pub const fn is_good(self) -> bool {
        self.0 == 0
    }

    /// True when every flag in `other` is set here.
    pub const fn contains(self, other: QualityDescriptor) -> bool {
        self.0 & other.0 == other.0
    }
}

impl std::ops::BitOr for QualityDescriptor {
    type Output = QualityDescriptor;
    fn bitor(self, rhs: QualityDescriptor) -> QualityDescriptor {
        QualityDescriptor(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for QualityDescriptor {
    fn bitor_assign(&mut self, rhs: QualityDescriptor) {
        self.0 |= rhs.0;
    }
}

impl std::ops::BitAnd for QualityDescriptor {
    type Output = QualityDescriptor;
    fn bitand(self, rhs: QualityDescriptor) -> QualityDescriptor {
        QualityDescriptor(self.0 & rhs.0)
    }
}

impl From<u8> for QualityDescriptor {
    fn from(v: u8) -> Self {
        QualityDescriptor(v)
    }
}

impl fmt::Display for QualityDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_good() {
            return f.write_str("Good");
        }
        let mut first = true;
        let mut put = |s: &str, f: &mut fmt::Formatter<'_>| -> fmt::Result {
            if !first {
                f.write_str(",")?;
            }
            first = false;
            f.write_str(s)
        };
        if self.contains(Self::OVERFLOW) {
            put("Overflow", f)?;
        }
        if self.contains(Self::BLOCKED) {
            put("Blocked", f)?;
        }
        if self.contains(Self::SUBSTITUTED) {
            put("Substituted", f)?;
        }
        if self.contains(Self::NOT_TOPICAL) {
            put("NotTopical", f)?;
        }
        if self.contains(Self::INVALID) {
            put("Invalid", f)?;
        }
        if first {
            // Only reserved bits are set.
            return write!(f, "QualityDescriptor({:x})", self.0);
        }
        Ok(())
    }
}

/// Quality descriptor for protection equipment.
///
/// See companion standard 101, subclass 7.2.6.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct QualityDescriptorProtection(pub u8);

impl QualityDescriptorProtection {
    /// No flags: the value is good.
    pub const GOOD: Self = QualityDescriptorProtection(0);
    /// Bit 3: the elapsed time was incorrectly acquired.
    pub const ELAPSED_TIME_INVALID: Self = QualityDescriptorProtection(1 << 3);
    /// Bit 4: the value is blocked for transmission.
    pub const BLOCKED: Self = QualityDescriptorProtection(1 << 4);
    /// Bit 5: the value was entered by an operator.
    pub const SUBSTITUTED: Self = QualityDescriptorProtection(1 << 5);
    /// Bit 6: the most recent update was unsuccessful.
    pub const NOT_TOPICAL: Self = QualityDescriptorProtection(1 << 6);
    /// Bit 7: the value was incorrectly acquired.
    pub const INVALID: Self = QualityDescriptorProtection(1 << 7);

    /// True when no quality flag is set.
    pub const fn is_good(self) -> bool {
        self.0 == 0
    }
}

impl std::ops::BitOr for QualityDescriptorProtection {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        QualityDescriptorProtection(self.0 | rhs.0)
    }
}

impl From<u8> for QualityDescriptorProtection {
    fn from(v: u8) -> Self {
        QualityDescriptorProtection(v)
    }
}

/// Measured value with transient state indication, for transformer tap
/// changers and other step positions.
///
/// See companion standard 101, subclass 7.2.6.5. `val` ranges `-64..=63`
/// (bits 0..=6, bit 6 being the sign); bit 7 marks the equipment in transient
/// state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StepPosition {
    /// Step position in `-64..=63`.
    pub val: i8,
    /// The equipment is in transient state.
    pub has_transient: bool,
}

impl StepPosition {
    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        let mut p = (self.val as u8) & 0x7f;
        if self.has_transient {
            p |= 0x80;
        }
        p
    }

    /// Decode from the wire octet, sign-extending the 7-bit value.
    pub const fn parse(b: u8) -> Self {
        let raw = b & 0x7f;
        // Sign-extend bit 6 into an i8.
        let val = if raw & 0x40 == 0 {
            raw as i8
        } else {
            (raw | 0x80) as i8
        };
        StepPosition {
            val,
            has_transient: (b & 0x80) != 0,
        }
    }
}

/// A 16-bit normalized value in `[-1, 1 - 2^-15]`.
///
/// See companion standard 101, subclass 7.2.6.6.
/// `f_normalized = 32768 * f_real / full_scale`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Normalize(pub i16);

impl Normalize {
    /// The value as a fraction in `[-1, 1 - 2^-15]`.
    pub fn f64(self) -> f64 {
        self.0 as f64 / 32768.0
    }

    /// Build from a fraction in `[-1, 1)`, saturating at the representable range.
    pub fn from_f64(v: f64) -> Self {
        Normalize((v * 32768.0).round().clamp(i16::MIN as f64, i16::MAX as f64) as i16)
    }
}

impl From<i16> for Normalize {
    fn from(v: i16) -> Self {
        Normalize(v)
    }
}

/// Binary counter reading. See companion standard 101, subclass 7.2.6.9.
///
/// ```text
/// | counter_reading  [bit0..bit31] |
/// | SQ [bit32..bit36] CY [bit37] CA [bit38] IV [bit39] |
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BinaryCounterReading {
    /// The accumulated count.
    pub counter_reading: i32,
    /// Sequence number, `0..=31`.
    pub seq_number: u8,
    /// CY: the counter overflowed during the integration period.
    pub has_carry: bool,
    /// CA: the counter was adjusted since the last reading.
    pub is_adjusted: bool,
    /// IV: the counter reading is invalid.
    pub is_invalid: bool,
}

/// Single event of protection equipment. See companion standard 101, subclass 7.2.6.10.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum SingleEvent {
    /// 0: indeterminate or intermediate state
    #[default]
    IndeterminateOrIntermediate = 0,
    /// 1: determined state off
    DeterminedOff = 1,
    /// 2: determined state on
    DeterminedOn = 2,
    /// 3: indeterminate state
    Indeterminate = 3,
}

impl SingleEvent {
    /// Encode to the two-bit value.
    pub const fn value(self) -> u8 {
        self as u8
    }

    /// Decode from the low two bits of an octet.
    pub const fn parse(b: u8) -> Self {
        match b & 0x03 {
            0 => SingleEvent::IndeterminateOrIntermediate,
            1 => SingleEvent::DeterminedOff,
            2 => SingleEvent::DeterminedOn,
            _ => SingleEvent::Indeterminate,
        }
    }
}

/// Start event of protection equipment, a bit field.
///
/// See companion standard 101, subclass 7.2.6.11.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct StartEvent(pub u8);

impl StartEvent {
    /// Bit 0: general start of operation.
    pub const GENERAL_START: StartEvent = StartEvent(1 << 0);
    /// Bit 1: start of operation phase L1.
    pub const START_L1: StartEvent = StartEvent(1 << 1);
    /// Bit 2: start of operation phase L2.
    pub const START_L2: StartEvent = StartEvent(1 << 2);
    /// Bit 3: start of operation phase L3.
    pub const START_L3: StartEvent = StartEvent(1 << 3);
    /// Bit 4: start of operation, earth current.
    pub const START_EARTH_CURRENT: StartEvent = StartEvent(1 << 4);
    /// Bit 5: start of operation in reverse direction.
    pub const START_REVERSE_DIRECTION: StartEvent = StartEvent(1 << 5);
}

impl std::ops::BitOr for StartEvent {
    type Output = StartEvent;
    fn bitor(self, rhs: StartEvent) -> StartEvent {
        StartEvent(self.0 | rhs.0)
    }
}

/// Output circuit information of protection equipment, a bit field.
///
/// See companion standard 101, subclass 7.2.6.12.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct OutputCircuitInfo(pub u8);

impl OutputCircuitInfo {
    /// Bit 0: general command output to the output circuit.
    pub const GENERAL_COMMAND: OutputCircuitInfo = OutputCircuitInfo(1 << 0);
    /// Bit 1: command output to the output circuit, phase L1.
    pub const COMMAND_L1: OutputCircuitInfo = OutputCircuitInfo(1 << 1);
    /// Bit 2: command output to the output circuit, phase L2.
    pub const COMMAND_L2: OutputCircuitInfo = OutputCircuitInfo(1 << 2);
    /// Bit 3: command output to the output circuit, phase L3.
    pub const COMMAND_L3: OutputCircuitInfo = OutputCircuitInfo(1 << 3);
}

impl std::ops::BitOr for OutputCircuitInfo {
    type Output = OutputCircuitInfo;
    fn bitor(self, rhs: OutputCircuitInfo) -> OutputCircuitInfo {
        OutputCircuitInfo(self.0 | rhs.0)
    }
}

/// The fixed test bit pattern carried by test commands.
///
/// See companion standard 101, subclass 7.2.6.14.
pub const FBP_TEST_WORD: u16 = 0x55aa;

/// Single command value. See companion standard 101, subclass 7.2.6.15.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum SingleCommand {
    /// 0: switch off
    #[default]
    Off = 0,
    /// 1: switch on
    On = 1,
}

/// Double command value. See companion standard 101, subclass 7.2.6.16.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum DoubleCommand {
    /// 0: not permitted
    #[default]
    NotAllowed0 = 0,
    /// 1: switch off
    Off = 1,
    /// 2: switch on
    On = 2,
    /// 3: not permitted
    NotAllowed3 = 3,
}

impl DoubleCommand {
    /// Encode to the two-bit value.
    pub const fn value(self) -> u8 {
        self as u8
    }

    /// Decode from the low two bits of an octet.
    pub const fn parse(b: u8) -> Self {
        match b & 0x03 {
            0 => DoubleCommand::NotAllowed0,
            1 => DoubleCommand::Off,
            2 => DoubleCommand::On,
            _ => DoubleCommand::NotAllowed3,
        }
    }
}

/// Regulating step command value. See companion standard 101, subclass 7.2.6.17.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(u8)]
pub enum StepCommand {
    /// 0: not permitted
    #[default]
    NotAllowed0 = 0,
    /// 1: next step lower
    StepDown = 1,
    /// 2: next step higher
    StepUp = 2,
    /// 3: not permitted
    NotAllowed3 = 3,
}

impl StepCommand {
    /// Encode to the two-bit value.
    pub const fn value(self) -> u8 {
        self as u8
    }

    /// Decode from the low two bits of an octet.
    pub const fn parse(b: u8) -> Self {
        match b & 0x03 {
            0 => StepCommand::NotAllowed0,
            1 => StepCommand::StepDown,
            2 => StepCommand::StepUp,
            _ => StepCommand::NotAllowed3,
        }
    }
}

/// Cause of initialization. See companion standard 101, subclass 7.2.6.21.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct CoiCause(pub u8);

impl CoiCause {
    /// 0: local power on
    pub const LOCAL_POWER_ON: CoiCause = CoiCause(0);
    /// 1: local manual reset
    pub const LOCAL_HAND_RESET: CoiCause = CoiCause(1);
    /// 2: remote reset
    pub const REMOTE_RESET: CoiCause = CoiCause(2);
}

/// Cause of initialization with the "local parameters changed" flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CauseOfInitial {
    /// Why the station initialized.
    pub cause: CoiCause,
    /// Bit 7: initialization occurred after a change of local parameters.
    pub is_local_change: bool,
}

impl CauseOfInitial {
    /// Parse the wire octet.
    pub const fn parse(b: u8) -> Self {
        CauseOfInitial {
            cause: CoiCause(b & 0x7f),
            is_local_change: b & 0x80 == 0x80,
        }
    }

    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        if self.is_local_change {
            self.cause.0 | 0x80
        } else {
            self.cause.0
        }
    }
}

/// Qualifier of interrogation. See companion standard 101, subclass 7.2.6.22.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct QualifierOfInterrogation(pub u8);

impl QualifierOfInterrogation {
    /// 0: not used
    pub const UNUSED: Self = QualifierOfInterrogation(0);
    /// 20: station interrogation (global)
    pub const STATION: Self = QualifierOfInterrogation(20);
    /// 21: interrogation of group 1
    pub const GROUP1: Self = QualifierOfInterrogation(21);
    /// 36: interrogation of group 16
    pub const GROUP16: Self = QualifierOfInterrogation(36);

    /// Interrogation of group `n`, for `n` in `1..=16`.
    pub const fn group(n: u8) -> Option<Self> {
        if n == 0 || n > 16 {
            return None;
        }
        Some(QualifierOfInterrogation(20 + n))
    }
}

/// Counter interrogation request, bits 0..=5.
///
/// See companion standard 101, subclass 7.2.6.23.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct QccRequest(pub u8);

impl QccRequest {
    /// 0: not used
    pub const UNUSED: QccRequest = QccRequest(0);
    /// 1: request counter group 1
    pub const GROUP1: QccRequest = QccRequest(1);
    /// 2: request counter group 2
    pub const GROUP2: QccRequest = QccRequest(2);
    /// 3: request counter group 3
    pub const GROUP3: QccRequest = QccRequest(3);
    /// 4: request counter group 4
    pub const GROUP4: QccRequest = QccRequest(4);
    /// 5: general request counter
    pub const TOTAL: QccRequest = QccRequest(5);
}

/// Counter freeze behaviour, bits 6..=7.
///
/// See companion standard 101, subclass 7.2.6.23.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct QccFreeze(pub u8);

impl QccFreeze {
    /// 0x00: read without freeze or reset
    pub const READ: QccFreeze = QccFreeze(0x00);
    /// 0x40: freeze without reset, the frozen value is the accumulated total
    pub const FREEZE_NO_RESET: QccFreeze = QccFreeze(0x40);
    /// 0x80: freeze with reset, the frozen value is the increment
    pub const FREEZE_RESET: QccFreeze = QccFreeze(0x80);
    /// 0xc0: counter reset
    pub const RESET: QccFreeze = QccFreeze(0xc0);
}

/// Qualifier of counter interrogation command.
///
/// See companion standard 101, subclass 7.2.6.23.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct QualifierCountCall {
    /// Which counter group is requested.
    pub request: QccRequest,
    /// Whether and how the counters are frozen.
    pub freeze: QccFreeze,
}

impl QualifierCountCall {
    /// Parse the wire octet.
    pub const fn parse(b: u8) -> Self {
        QualifierCountCall {
            request: QccRequest(b & 0x3f),
            freeze: QccFreeze(b & 0xc0),
        }
    }

    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        (self.request.0 & 0x3f) | (self.freeze.0 & 0xc0)
    }
}

/// Category of a measured value parameter, bits 0..=5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct QpmCategory(pub u8);

impl QpmCategory {
    /// 0: not used
    pub const UNUSED: QpmCategory = QpmCategory(0);
    /// 1: threshold value
    pub const THRESHOLD: QpmCategory = QpmCategory(1);
    /// 2: smoothing factor (filter time constant)
    pub const SMOOTHING: QpmCategory = QpmCategory(2);
    /// 3: low limit for transmission of measured values
    pub const LOW_LIMIT: QpmCategory = QpmCategory(3);
    /// 4: high limit for transmission of measured values
    pub const HIGH_LIMIT: QpmCategory = QpmCategory(4);
}

/// Qualifier of parameter of measured values.
///
/// See companion standard 101, subclass 7.2.6.24.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct QualifierOfParameterMv {
    /// Which parameter this is.
    pub category: QpmCategory,
    /// Bit 6: the parameter was changed locally.
    pub is_change: bool,
    /// Bit 7: the parameter is not in operation.
    pub is_in_operation: bool,
}

impl QualifierOfParameterMv {
    /// Parse the wire octet.
    pub const fn parse(b: u8) -> Self {
        QualifierOfParameterMv {
            category: QpmCategory(b & 0x3f),
            is_change: b & 0x40 == 0x40,
            is_in_operation: b & 0x80 == 0x80,
        }
    }

    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        let mut v = self.category.0 & 0x3f;
        if self.is_change {
            v |= 0x40;
        }
        if self.is_in_operation {
            v |= 0x80;
        }
        v
    }
}

/// Qualifier of parameter activation. See companion standard 101, subclass 7.2.6.25.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct QualifierOfParameterAct(pub u8);

impl QualifierOfParameterAct {
    /// 0: not used
    pub const UNUSED: Self = QualifierOfParameterAct(0);
    /// 1: (de)activation of the previously loaded parameters (IOA = 0)
    pub const DEACT_PREV_LOADED_PARAMETER: Self = QualifierOfParameterAct(1);
    /// 2: (de)activation of the parameters of the addressed object
    pub const DEACT_OBJECT_PARAMETER: Self = QualifierOfParameterAct(2);
    /// 3: (de)activation of cyclic or periodic transmission of the addressed object
    pub const DEACT_OBJECT_TRANSMISSION: Self = QualifierOfParameterAct(3);
}

/// Command qualifier value, bits 2..=6 of the qualifier octet.
///
/// See companion standard 101, subclass 7.2.6.26.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct QocQual(pub u8);

impl QocQual {
    /// 0: no additional definition
    pub const NO_ADDITIONAL_DEFINITION: QocQual = QocQual(0);
    /// 1: short pulse duration, determined by a parameter in the outstation
    pub const SHORT_PULSE_DURATION: QocQual = QocQual(1);
    /// 2: long pulse duration, determined by a parameter in the outstation
    pub const LONG_PULSE_DURATION: QocQual = QocQual(2);
    /// 3: persistent output
    pub const PERSISTENT_OUTPUT: QocQual = QocQual(3);
}

impl fmt::Display for QocQual {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            QocQual::NO_ADDITIONAL_DEFINITION => f.write_str("NoAdditionalDefinition"),
            QocQual::SHORT_PULSE_DURATION => f.write_str("ShortPulseDuration"),
            QocQual::LONG_PULSE_DURATION => f.write_str("LongPulseDuration"),
            QocQual::PERSISTENT_OUTPUT => f.write_str("PersistentOutput"),
            QocQual(v) => write!(f, "QOCQual({v})"),
        }
    }
}

/// Qualifier of command. See companion standard 101, subclass 7.2.6.26 and
/// section 5, subclass 6.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct QualifierOfCommand {
    /// Pulse behaviour of the output.
    pub qual: QocQual,
    /// S/E bit: `true` selects, `false` executes.
    pub in_select: bool,
}

impl QualifierOfCommand {
    /// Parse the wire octet (bits 2..=7 of the command octet).
    pub const fn parse(b: u8) -> Self {
        QualifierOfCommand {
            qual: QocQual((b >> 2) & 0x1f),
            in_select: b & 0x80 == 0x80,
        }
    }

    /// Encode to the wire octet, leaving the value bits clear.
    pub const fn value(self) -> u8 {
        let mut v = (self.qual.0 & 0x1f) << 2;
        if self.in_select {
            v |= 0x80;
        }
        v
    }
}

impl fmt::Display for QualifierOfCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mode = if self.in_select { "Select" } else { "Execute" };
        write!(f, "{mode}:{}", self.qual)
    }
}

/// Qualifier of reset process command. See companion standard 101, subclass 7.2.6.27.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct QualifierOfResetProcessCmd(pub u8);

impl QualifierOfResetProcessCmd {
    /// 0: not used
    pub const UNUSED: Self = QualifierOfResetProcessCmd(0);
    /// 1: general reset of the process
    pub const GENERAL_RESET: Self = QualifierOfResetProcessCmd(1);
    /// 2: reset of pending time-tagged information in the event buffer
    pub const RESET_PENDING_INFO_WITH_TIME_TAG: Self = QualifierOfResetProcessCmd(2);
}

/// Set-point command qualifier value, bits 0..=6.
///
/// See companion standard 101, subclass 7.2.6.39.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct QosQual(pub u8);

/// Qualifier of a set-point command. See section 5, subclass 6.8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct QualifierOfSetpointCmd {
    /// Implementation-defined qualifier value.
    pub qual: QosQual,
    /// S/E bit: `true` selects, `false` executes.
    pub in_select: bool,
}

impl QualifierOfSetpointCmd {
    /// Parse the wire octet.
    pub const fn parse(b: u8) -> Self {
        QualifierOfSetpointCmd {
            qual: QosQual(b & 0x7f),
            in_select: b & 0x80 == 0x80,
        }
    }

    /// Encode to the wire octet.
    pub const fn value(self) -> u8 {
        let mut v = self.qual.0 & 0x7f;
        if self.in_select {
            v |= 0x80;
        }
        v
    }
}

impl fmt::Display for QualifierOfSetpointCmd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mode = if self.in_select { "Select" } else { "Execute" };
        write!(f, "{mode}:QOSQual({})", self.qual.0)
    }
}

/// Status and status change detection, 32 bits.
///
/// See companion standard 101, subclass 7.2.6.40. The low 16 bits are the
/// current status of 16 single points, the high 16 bits mark which of them
/// changed since the last transmission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct StatusAndStatusChangeDetection(pub u32);

impl StatusAndStatusChangeDetection {
    /// The 16 status bits (ST).
    pub const fn status(self) -> u16 {
        self.0 as u16
    }

    /// The 16 change-detection bits (CD).
    pub const fn change_detection(self) -> u16 {
        (self.0 >> 16) as u16
    }
}

impl From<u32> for StatusAndStatusChangeDetection {
    fn from(v: u32) -> Self {
        StatusAndStatusChangeDetection(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_position_sign_extends_the_seven_bit_field() {
        for v in -64i8..=63 {
            for transient in [false, true] {
                let sp = StepPosition {
                    val: v,
                    has_transient: transient,
                };
                assert_eq!(StepPosition::parse(sp.value()), sp, "val {v}");
            }
        }
        assert_eq!(StepPosition::parse(0x40).val, -64);
        assert_eq!(StepPosition::parse(0x3f).val, 63);
        assert!(StepPosition::parse(0x80).has_transient);
    }

    #[test]
    fn normalize_maps_the_full_scale_range() {
        assert_eq!(Normalize(0).f64(), 0.0);
        assert_eq!(Normalize(32767).f64(), 32767.0 / 32768.0);
        assert_eq!(Normalize(-32768).f64(), -1.0);
        assert_eq!(Normalize::from_f64(-1.0), Normalize(-32768));
        assert_eq!(Normalize::from_f64(0.5), Normalize(16384));
    }

    #[test]
    fn qualifier_of_command_round_trips() {
        for qual in 0u8..32 {
            for sel in [false, true] {
                let q = QualifierOfCommand {
                    qual: QocQual(qual),
                    in_select: sel,
                };
                assert_eq!(QualifierOfCommand::parse(q.value()), q);
            }
        }
    }

    #[test]
    fn qualifier_of_setpoint_round_trips() {
        for b in 0u8..=255 {
            assert_eq!(QualifierOfSetpointCmd::parse(b).value(), b);
        }
    }

    #[test]
    fn counter_call_qualifier_round_trips() {
        for b in 0u8..=255 {
            assert_eq!(QualifierCountCall::parse(b).value(), b);
        }
    }

    #[test]
    fn quality_descriptor_formats_flags() {
        assert_eq!(QualityDescriptor::GOOD.to_string(), "Good");
        assert_eq!(QualityDescriptor::INVALID.to_string(), "Invalid");
        assert_eq!(
            (QualityDescriptor::OVERFLOW | QualityDescriptor::INVALID).to_string(),
            "Overflow,Invalid"
        );
        assert_eq!(QualityDescriptor(0x02).to_string(), "QualityDescriptor(2)");
    }

    #[test]
    fn cause_of_initial_round_trips() {
        for b in 0u8..=255 {
            assert_eq!(CauseOfInitial::parse(b).value(), b);
        }
    }

    #[test]
    fn interrogation_groups_are_bounded() {
        assert_eq!(
            QualifierOfInterrogation::group(1),
            Some(QualifierOfInterrogation::GROUP1)
        );
        assert_eq!(
            QualifierOfInterrogation::group(16),
            Some(QualifierOfInterrogation::GROUP16)
        );
        assert_eq!(QualifierOfInterrogation::group(17), None);
        assert_eq!(QualifierOfInterrogation::group(0), None);
    }
}
