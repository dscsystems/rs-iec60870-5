// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The client handler that feeds the UI, and the decoding of monitor-direction
//! ASDUs into displayable points.

use chrono::{DateTime, Utc};
use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{ClientContext, ClientHandler};

use crate::event::{Event, Sender};

/// One row of the monitored-points table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Point {
    pub ioa: InfoObjAddr,
    pub type_name: String,
    pub value: String,
    pub quality: String,
    pub cause: String,
    pub time: String,
    /// How many times this address has been reported, filled in by the UI.
    pub count: u64,
}

/// Sends every received ASDU to the UI: a summary line for the log, and the
/// decoded points for the table.
pub struct TuiHandler {
    pub tx: Sender,
}

#[async_trait::async_trait]
impl ClientHandler for TuiHandler {
    /// Every inbound ASDU passes here before routing, which is exactly what a
    /// protocol explorer wants to see.
    async fn asdu_all(
        &self,
        _c: &dyn Connect,
        pack: &Asdu,
        _ctx: &ClientContext,
    ) -> rs_iec60870_5::Result<()> {
        let _ = self.tx.send(Event::Log(format!("[rx] {pack}")));
        let points = extract_points(pack);
        if !points.is_empty() {
            let _ = self.tx.send(Event::Points(points));
        }
        Ok(())
    }

    // The connection callbacks drive the status indicator in the header, and
    // gate the request keys: nothing may be sent before StartDT is confirmed.

    async fn on_connect(&self, _c: &dyn Connect) {
        let _ = self.tx.send(Event::status(
            true,
            false,
            "TCP connected, sending STARTDT",
        ));
    }

    async fn on_activated(&self, _c: &dyn Connect) {
        let _ = self.tx.send(Event::status(
            true,
            true,
            "data transfer active (STARTDT confirmed)",
        ));
    }

    async fn on_deactivated(&self, _c: &dyn Connect) {
        let _ = self.tx.send(Event::status(
            true,
            false,
            "data transfer stopped (STOPDT confirmed)",
        ));
    }

    async fn on_connection_lost(&self, _c: &dyn Connect) {
        let _ = self.tx.send(Event::status(false, false, "connection lost"));
    }
}

/// Strip the `TID<...>` wrapper from a type identification.
fn type_name(t: TypeId) -> String {
    t.name().map(str::to_string).unwrap_or_else(|| t.0.to_string())
}

/// Render a cause of transmission with its test and negative flags.
fn cause_name(coa: CauseOfTransmission) -> String {
    let mut s = coa.cause.to_string();
    if coa.is_negative {
        s.push_str(",neg");
    }
    if coa.is_test {
        s.push_str(",test");
    }
    s
}

fn bool_str(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// A time tag for the table, marked when its IV or SB flag is set so an
/// unsynchronized or substituted time is never mistaken for a good one.
fn ts(t: Option<DateTime<Utc>>, flags: TimeTagFlags) -> String {
    let Some(t) = t else { return String::new() };
    let mut s = t.format("%H:%M:%S%.3f").to_string();
    if flags.invalid {
        s.push_str(" IV");
    }
    if flags.substituted {
        s.push_str(" SB");
    }
    s
}

/// Decode the monitor-direction information objects of an ASDU into points.
///
/// Control and system ASDUs carry no monitored data and yield nothing; they
/// still show up in the log panel. A malformed payload also yields nothing
/// rather than failing the whole update.
pub fn extract_points(a: &Asdu) -> Vec<Point> {
    let tid = type_name(a.type_id());
    let cause = cause_name(a.coa());

    let mk = |ioa: InfoObjAddr,
              value: String,
              quality: String,
              (time, flags): (Option<DateTime<Utc>>, TimeTagFlags)| Point {
        ioa,
        type_name: tid.clone(),
        value,
        quality,
        cause: cause.clone(),
        time: ts(time, flags),
        count: 0,
    };

    match a.type_id() {
        TypeId::M_SP_NA_1 | TypeId::M_SP_TA_1 | TypeId::M_SP_TB_1 => a
            .get_single_point()
            .map(|v| {
                v.into_iter()
                    .map(|p| mk(p.ioa, bool_str(p.value).into(), p.qds.to_string(), (p.time, p.time_flags)))
                    .collect()
            })
            .unwrap_or_default(),

        TypeId::M_DP_NA_1 | TypeId::M_DP_TA_1 | TypeId::M_DP_TB_1 => a
            .get_double_point()
            .map(|v| {
                v.into_iter()
                    .map(|p| mk(p.ioa, p.value.to_string(), p.qds.to_string(), (p.time, p.time_flags)))
                    .collect()
            })
            .unwrap_or_default(),

        TypeId::M_ST_NA_1 | TypeId::M_ST_TA_1 | TypeId::M_ST_TB_1 => a
            .get_step_position()
            .map(|v| {
                v.into_iter()
                    .map(|p| {
                        let mut value = p.value.val.to_string();
                        if p.value.has_transient {
                            value.push_str(" (transient)");
                        }
                        mk(p.ioa, value, p.qds.to_string(), (p.time, p.time_flags))
                    })
                    .collect()
            })
            .unwrap_or_default(),

        TypeId::M_BO_NA_1 | TypeId::M_BO_TA_1 | TypeId::M_BO_TB_1 => a
            .get_bitstring32()
            .map(|v| {
                v.into_iter()
                    .map(|p| {
                        mk(
                            p.ioa,
                            format!("0x{:08X}", p.value),
                            p.qds.to_string(),
                            (p.time, p.time_flags),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),

        TypeId::M_ME_NA_1 | TypeId::M_ME_TA_1 | TypeId::M_ME_TD_1 | TypeId::M_ME_ND_1 => a
            .get_measured_value_normal()
            .map(|v| {
                v.into_iter()
                    .map(|p| {
                        mk(
                            p.ioa,
                            format!("{:.5}", p.value.f64()),
                            p.qds.to_string(),
                            (p.time, p.time_flags),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),

        TypeId::M_ME_NB_1 | TypeId::M_ME_TB_1 | TypeId::M_ME_TE_1 => a
            .get_measured_value_scaled()
            .map(|v| {
                v.into_iter()
                    .map(|p| mk(p.ioa, p.value.to_string(), p.qds.to_string(), (p.time, p.time_flags)))
                    .collect()
            })
            .unwrap_or_default(),

        TypeId::M_ME_NC_1 | TypeId::M_ME_TC_1 | TypeId::M_ME_TF_1 => a
            .get_measured_value_float()
            .map(|v| {
                v.into_iter()
                    .map(|p| mk(p.ioa, p.value.to_string(), p.qds.to_string(), (p.time, p.time_flags)))
                    .collect()
            })
            .unwrap_or_default(),

        TypeId::M_IT_NA_1 | TypeId::M_IT_TA_1 | TypeId::M_IT_TB_1 => a
            .get_integrated_totals()
            .map(|v| {
                v.into_iter()
                    .map(|p| {
                        let value =
                            format!("{} (seq {})", p.value.counter_reading, p.value.seq_number);
                        // The counter flags are their own quality set, not a QDS.
                        let mut flags = Vec::new();
                        if p.value.is_invalid {
                            flags.push("IV");
                        }
                        if p.value.has_carry {
                            flags.push("CY");
                        }
                        if p.value.is_adjusted {
                            flags.push("CA");
                        }
                        mk(p.ioa, value, flags.join(","), (p.time, p.time_flags))
                    })
                    .collect()
            })
            .unwrap_or_default(),

        TypeId::M_PS_NA_1 => a
            .get_packed_single_point_with_scd()
            .map(|v| {
                v.into_iter()
                    .map(|p| {
                        mk(
                            p.ioa,
                            format!("ST 0x{:04X} CD 0x{:04X}", p.scd.status(), p.scd.change_detection()),
                            p.qds.to_string(),
                            (None, TimeTagFlags::GOOD),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default(),

        // Control, parameter and system types carry no monitored points.
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coa(c: Cause) -> CauseOfTransmission {
        CauseOfTransmission::new(c)
    }

    #[test]
    fn single_points_decode_into_rows() {
        let a = Asdu::single(
            PARAMS_WIDE,
            false,
            coa(Cause::INTERROGATED_BY_STATION),
            1,
            &[
                SinglePointInfo::new(100, true),
                SinglePointInfo {
                    ioa: 101,
                    value: false,
                    qds: QualityDescriptor::INVALID,
                    time: None,
                    time_flags: TimeTagFlags::GOOD,
                },
            ],
        )
        .unwrap();

        let points = extract_points(&a);
        assert_eq!(points.len(), 2);
        assert_eq!(points[0].ioa, 100);
        assert_eq!(points[0].value, "on");
        assert_eq!(points[0].quality, "Good");
        assert_eq!(points[0].type_name, "M_SP_NA_1");
        assert_eq!(points[0].cause, "InterrogatedByStation");
        assert_eq!(points[1].value, "off");
        assert_eq!(points[1].quality, "Invalid");
    }

    #[test]
    fn time_tagged_points_carry_a_timestamp() {
        let t = chrono::Utc::now();
        let a = Asdu::single_cp56time2a(
            PARAMS_WIDE,
            coa(Cause::SPONTANEOUS),
            1,
            &[SinglePointInfo {
                ioa: 100,
                value: true,
                qds: QualityDescriptor::GOOD,
                time: Some(t),
                time_flags: TimeTagFlags::GOOD,
            }],
        )
        .unwrap();

        let points = extract_points(&a);
        assert!(!points[0].time.is_empty(), "a CP56Time2a tag must show");
        assert_eq!(points[0].type_name, "M_SP_TB_1");
    }

    #[test]
    fn every_monitor_family_yields_a_row() {
        let p = PARAMS_WIDE;
        let cases: Vec<(Asdu, &str)> = vec![
            (
                Asdu::double(
                    p,
                    false,
                    coa(Cause::SPONTANEOUS),
                    1,
                    &[DoublePointInfo {
                        ioa: 1,
                        value: DoublePoint::DeterminedOn,
                        ..Default::default()
                    }],
                )
                .unwrap(),
                "DeterminedOn",
            ),
            (
                Asdu::step(
                    p,
                    false,
                    coa(Cause::SPONTANEOUS),
                    1,
                    &[StepPositionInfo {
                        ioa: 2,
                        value: StepPosition {
                            val: -17,
                            has_transient: true,
                        },
                        ..Default::default()
                    }],
                )
                .unwrap(),
                "-17 (transient)",
            ),
            (
                Asdu::bitstring32(
                    p,
                    false,
                    coa(Cause::SPONTANEOUS),
                    1,
                    &[BitString32Info {
                        ioa: 3,
                        value: 0xdead_beef,
                        ..Default::default()
                    }],
                )
                .unwrap(),
                "0xDEADBEEF",
            ),
            (
                Asdu::measured_value_normal(
                    p,
                    false,
                    coa(Cause::PERIODIC),
                    1,
                    &[MeasuredValueNormalInfo {
                        ioa: 4,
                        value: Normalize(16384),
                        ..Default::default()
                    }],
                )
                .unwrap(),
                "0.50000",
            ),
            (
                Asdu::measured_value_scaled(
                    p,
                    false,
                    coa(Cause::PERIODIC),
                    1,
                    &[MeasuredValueScaledInfo {
                        ioa: 5,
                        value: -1234,
                        ..Default::default()
                    }],
                )
                .unwrap(),
                "-1234",
            ),
            (
                Asdu::measured_value_float(
                    p,
                    false,
                    coa(Cause::PERIODIC),
                    1,
                    &[MeasuredValueFloatInfo {
                        ioa: 6,
                        value: 22.5,
                        ..Default::default()
                    }],
                )
                .unwrap(),
                "22.5",
            ),
        ];

        for (asdu, expected) in cases {
            let points = extract_points(&asdu);
            assert_eq!(points.len(), 1, "no row for {}", asdu.type_id());
            assert_eq!(points[0].value, expected, "wrong value for {}", asdu.type_id());
        }
    }

    #[test]
    fn counter_flags_are_shown_as_quality() {
        let a = Asdu::integrated_totals(
            PARAMS_WIDE,
            false,
            coa(Cause::SPONTANEOUS),
            1,
            &[BinaryCounterReadingInfo {
                ioa: 500,
                value: BinaryCounterReading {
                    counter_reading: 4242,
                    seq_number: 3,
                    has_carry: true,
                    is_adjusted: false,
                    is_invalid: true,
                },
                time: None,
                time_flags: TimeTagFlags::GOOD,
            }],
        )
        .unwrap();

        let points = extract_points(&a);
        assert_eq!(points[0].value, "4242 (seq 3)");
        assert_eq!(points[0].quality, "IV,CY");
    }

    #[test]
    fn control_and_system_types_yield_no_points() {
        let a = Asdu::interrogation_cmd(
            PARAMS_WIDE,
            coa(Cause::ACTIVATION),
            1,
            QualifierOfInterrogation::STATION,
        )
        .unwrap();
        assert!(extract_points(&a).is_empty());

        let a = Asdu::single_cmd(
            PARAMS_WIDE,
            TypeId::C_SC_NA_1,
            coa(Cause::ACTIVATION),
            1,
            SingleCommandInfo::default(),
        )
        .unwrap();
        assert!(extract_points(&a).is_empty());
    }

    #[test]
    fn a_negative_confirmation_is_visible_in_the_cause() {
        let a = Asdu::single(
            PARAMS_WIDE,
            false,
            coa(Cause::SPONTANEOUS).negative(),
            1,
            &[SinglePointInfo::new(1, true)],
        )
        .unwrap();
        assert_eq!(extract_points(&a)[0].cause, "Spontaneous,neg");
    }

    #[test]
    fn a_truncated_payload_yields_nothing_rather_than_panicking() {
        let mut a = Asdu::new_empty(PARAMS_WIDE);
        a.identifier.type_id = TypeId::M_SP_NA_1;
        a.identifier.coa = coa(Cause::SPONTANEOUS);
        a.identifier.common_addr = 1;
        a.identifier.variable.number = 4; // claims four objects
        a.info_obj = vec![1, 2, 3]; // carries less than one
        assert!(extract_points(&a).is_empty());
    }
}
