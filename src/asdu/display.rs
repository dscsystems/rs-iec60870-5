// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! A compact, human-readable dump of an ASDU for logs and diagnostics.
//!
//! Decoding is non-destructive, so `to_string()` can be called at any point,
//! any number of times, and freely mixed with the `get_*` accessors.

use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};

use crate::asdu::codec::Asdu;
use crate::asdu::identifier::TypeId;
use crate::asdu::info::QualityDescriptor;
use crate::asdu::time::TimeTagFlags;

/// A time tag with its IV and SB flags, so an invalid or substituted time is
/// never read in a log as a good one.
fn tsf(t: Option<DateTime<Utc>>, flags: TimeTagFlags) -> String {
    let mut s = ts(t);
    if !s.is_empty() {
        if flags.invalid {
            s.push_str(" (IV)");
        }
        if flags.substituted {
            s.push_str(" (SB)");
        }
    }
    s
}

fn ts(t: Option<DateTime<Utc>>) -> String {
    match t {
        Some(t) => format!(" @{}", t.to_rfc3339_opts(SecondsFormat::AutoSi, true)),
        None => String::new(),
    }
}

fn qds(q: QualityDescriptor) -> String {
    if q.is_good() {
        String::new()
    } else {
        format!(" QDS=0x{:02x}", q.0)
    }
}

/// Format a list of items as ` items=N [a, b, c]`.
fn list<T>(f: &mut fmt::Formatter<'_>, items: &[T], mut one: impl FnMut(&mut fmt::Formatter<'_>, &T) -> fmt::Result) -> fmt::Result {
    write!(f, " items={}", items.len())?;
    for (i, it) in items.iter().enumerate() {
        f.write_str(if i == 0 { " [" } else { ", " })?;
        one(f, it)?;
    }
    if !items.is_empty() {
        f.write_str("]")?;
    }
    Ok(())
}

impl fmt::Display for Asdu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} IOA-Width={}",
            self.identifier,
            self.identifier.variable,
            self.params.info_obj_addr_size
        )?;

        if self.info_obj.is_empty() {
            return Ok(());
        }

        // A malformed payload must never make logging fail; fall back to a
        // byte-count summary when a decoder reports an error.
        let decoded = match self.type_id() {
            TypeId::M_SP_NA_1 | TypeId::M_SP_TA_1 | TypeId::M_SP_TB_1 => {
                self.get_single_point().map(|v| {
                    list(f, &v, |f, it| {
                        write!(f, "{}={}{}{}", it.ioa, it.value, qds(it.qds), tsf(it.time, it.time_flags))
                    })
                })
            }
            TypeId::M_DP_NA_1 | TypeId::M_DP_TA_1 | TypeId::M_DP_TB_1 => {
                self.get_double_point().map(|v| {
                    list(f, &v, |f, it| {
                        write!(
                            f,
                            "{}={}{}{}",
                            it.ioa,
                            it.value.value(),
                            qds(it.qds),
                            tsf(it.time, it.time_flags)
                        )
                    })
                })
            }
            TypeId::M_ST_NA_1 | TypeId::M_ST_TA_1 | TypeId::M_ST_TB_1 => {
                self.get_step_position().map(|v| {
                    list(f, &v, |f, it| {
                        write!(f, "{}=val({})", it.ioa, it.value.val)?;
                        if it.value.has_transient {
                            f.write_str(" transient")?;
                        }
                        write!(f, "{}{}", qds(it.qds), tsf(it.time, it.time_flags))
                    })
                })
            }
            TypeId::M_BO_NA_1 | TypeId::M_BO_TA_1 | TypeId::M_BO_TB_1 => {
                self.get_bitstring32().map(|v| {
                    list(f, &v, |f, it| {
                        write!(
                            f,
                            "{}=0x{:08x}{}{}",
                            it.ioa,
                            it.value,
                            qds(it.qds),
                            tsf(it.time, it.time_flags)
                        )
                    })
                })
            }
            TypeId::M_ME_NA_1 | TypeId::M_ME_TA_1 | TypeId::M_ME_TD_1 | TypeId::M_ME_ND_1 => {
                self.get_measured_value_normal().map(|v| {
                    list(f, &v, |f, it| {
                        write!(
                            f,
                            "{}={:.6}{}{}",
                            it.ioa,
                            it.value.f64(),
                            qds(it.qds),
                            tsf(it.time, it.time_flags)
                        )
                    })
                })
            }
            TypeId::M_ME_NB_1 | TypeId::M_ME_TB_1 | TypeId::M_ME_TE_1 => {
                self.get_measured_value_scaled().map(|v| {
                    list(f, &v, |f, it| {
                        write!(f, "{}={}{}{}", it.ioa, it.value, qds(it.qds), tsf(it.time, it.time_flags))
                    })
                })
            }
            TypeId::M_ME_NC_1 | TypeId::M_ME_TC_1 | TypeId::M_ME_TF_1 => {
                self.get_measured_value_float().map(|v| {
                    list(f, &v, |f, it| {
                        write!(f, "{}={}{}{}", it.ioa, it.value, qds(it.qds), tsf(it.time, it.time_flags))
                    })
                })
            }
            TypeId::M_IT_NA_1 | TypeId::M_IT_TA_1 | TypeId::M_IT_TB_1 => {
                self.get_integrated_totals().map(|v| {
                    list(f, &v, |f, it| {
                        let c = it.value;
                        write!(
                            f,
                            "{}=count({}) seq={}",
                            it.ioa, c.counter_reading, c.seq_number
                        )?;
                        if c.has_carry {
                            f.write_str(" carry")?;
                        }
                        if c.is_adjusted {
                            f.write_str(" adjusted")?;
                        }
                        if c.is_invalid {
                            f.write_str(" invalid")?;
                        }
                        f.write_str(&tsf(it.time, it.time_flags))
                    })
                })
            }
            TypeId::M_EP_TA_1 | TypeId::M_EP_TD_1 => {
                self.get_event_of_protection_equipment().map(|v| {
                    list(f, &v, |f, it| {
                        write!(
                            f,
                            "{}=event({}) QDP=0x{:02x} msec={}{}",
                            it.ioa,
                            it.event.value(),
                            it.qdp.0,
                            it.msec,
                            tsf(it.time, it.time_flags)
                        )
                    })
                })
            }
            TypeId::M_EP_TB_1 | TypeId::M_EP_TE_1 => self.get_packed_start_events().map(|it| {
                write!(
                    f,
                    " IOA={} start=0x{:02x} QDP=0x{:02x} msec={}{}",
                    it.ioa,
                    it.event.0,
                    it.qdp.0,
                    it.msec,
                    tsf(it.time, it.time_flags)
                )
            }),
            TypeId::M_EP_TC_1 | TypeId::M_EP_TF_1 => {
                self.get_packed_output_circuit_info().map(|it| {
                    write!(
                        f,
                        " IOA={} oci=0x{:02x} QDP=0x{:02x} msec={}{}",
                        it.ioa,
                        it.oci.0,
                        it.qdp.0,
                        it.msec,
                        tsf(it.time, it.time_flags)
                    )
                })
            }
            TypeId::M_PS_NA_1 => self.get_packed_single_point_with_scd().map(|v| {
                list(f, &v, |f, it| {
                    write!(
                        f,
                        "{}=SCD(0x{:08x}) QDS=0x{:02x}",
                        it.ioa, it.scd.0, it.qds.0
                    )
                })
            }),
            TypeId::M_EI_NA_1 => self.get_end_of_initialization().map(|(ioa, coi)| {
                write!(
                    f,
                    " IOA={ioa} cause={} localChange={}",
                    coi.cause.0, coi.is_local_change
                )
            }),
            TypeId::C_SC_NA_1 | TypeId::C_SC_TA_1 => self.get_single_cmd().map(|c| {
                write!(
                    f,
                    " IOA={} val={} QOC=0x{:02x}{}",
                    c.ioa,
                    c.value,
                    c.qoc.value(),
                    tsf(c.time, c.time_flags)
                )
            }),
            TypeId::C_DC_NA_1 | TypeId::C_DC_TA_1 => self.get_double_cmd().map(|c| {
                write!(
                    f,
                    " IOA={} val={} QOC=0x{:02x}{}",
                    c.ioa,
                    c.value.value(),
                    c.qoc.value(),
                    tsf(c.time, c.time_flags)
                )
            }),
            TypeId::C_RC_NA_1 | TypeId::C_RC_TA_1 => self.get_step_cmd().map(|c| {
                write!(
                    f,
                    " IOA={} val={} QOC=0x{:02x}{}",
                    c.ioa,
                    c.value.value(),
                    c.qoc.value(),
                    tsf(c.time, c.time_flags)
                )
            }),
            TypeId::C_SE_NA_1 | TypeId::C_SE_TA_1 => self.get_setpoint_normal_cmd().map(|c| {
                write!(
                    f,
                    " IOA={} val={:.6} QOS=0x{:02x}{}",
                    c.ioa,
                    c.value.f64(),
                    c.qos.value(),
                    tsf(c.time, c.time_flags)
                )
            }),
            TypeId::C_SE_NB_1 | TypeId::C_SE_TB_1 => self.get_setpoint_scaled_cmd().map(|c| {
                write!(
                    f,
                    " IOA={} val={} QOS=0x{:02x}{}",
                    c.ioa,
                    c.value,
                    c.qos.value(),
                    tsf(c.time, c.time_flags)
                )
            }),
            TypeId::C_SE_NC_1 | TypeId::C_SE_TC_1 => self.get_setpoint_float_cmd().map(|c| {
                write!(
                    f,
                    " IOA={} val={} QOS=0x{:02x}{}",
                    c.ioa,
                    c.value,
                    c.qos.value(),
                    tsf(c.time, c.time_flags)
                )
            }),
            TypeId::C_BO_NA_1 | TypeId::C_BO_TA_1 => self.get_bits_string32_cmd().map(|c| {
                write!(f, " IOA={} bits=0x{:08x}{}", c.ioa, c.value, tsf(c.time, c.time_flags))
            }),
            TypeId::P_ME_NA_1 => self.get_parameter_normal().map(|p| {
                write!(
                    f,
                    " IOA={} val={:.6} QPM=0x{:02x}",
                    p.ioa,
                    p.value.f64(),
                    p.qpm.value()
                )
            }),
            TypeId::P_ME_NB_1 => self.get_parameter_scaled().map(|p| {
                write!(
                    f,
                    " IOA={} val={} QPM=0x{:02x}",
                    p.ioa,
                    p.value,
                    p.qpm.value()
                )
            }),
            TypeId::P_ME_NC_1 => self.get_parameter_float().map(|p| {
                write!(
                    f,
                    " IOA={} val={} QPM=0x{:02x}",
                    p.ioa,
                    p.value,
                    p.qpm.value()
                )
            }),
            TypeId::P_AC_NA_1 => self
                .get_parameter_activation()
                .map(|p| write!(f, " IOA={} QPA={}", p.ioa, p.qpa.0)),
            TypeId::C_IC_NA_1 => self
                .get_interrogation_cmd()
                .map(|(ioa, qoi)| write!(f, " IOA={ioa} QOI={}", qoi.0)),
            TypeId::C_CI_NA_1 => self
                .get_counter_interrogation_cmd()
                .map(|(ioa, qcc)| write!(f, " IOA={ioa} QCC=0x{:02x}", qcc.value())),
            TypeId::C_CS_NA_1 => self
                .get_clock_synchronization_cmd()
                .map(|(ioa, t)| write!(f, " IOA={ioa}{}", ts(t))),
            TypeId::F_FR_NA_1 => self.get_file_ready().map(|i| {
                write!(
                    f,
                    " IOA={} NOF={} LOF={} FRQ=0x{:02x}",
                    i.ioa,
                    i.nof,
                    i.length_of_file,
                    i.frq.value()
                )
            }),
            TypeId::F_SR_NA_1 => self.get_section_ready().map(|i| {
                write!(
                    f,
                    " IOA={} NOF={} NOS={} LOS={} SRQ=0x{:02x}",
                    i.ioa,
                    i.nof,
                    i.nos,
                    i.length_of_section,
                    i.srq.value()
                )
            }),
            TypeId::F_SC_NA_1 => self.get_call_or_select_file().map(|i| {
                write!(
                    f,
                    " IOA={} NOF={} NOS={} SCQ=0x{:02x}",
                    i.ioa,
                    i.nof,
                    i.nos,
                    i.scq.value()
                )
            }),
            TypeId::F_LS_NA_1 => self.get_last_section_or_segment().map(|i| {
                write!(
                    f,
                    " IOA={} NOF={} NOS={} LSQ={} CHS=0x{:02x}",
                    i.ioa, i.nof, i.nos, i.lsq.0, i.chs
                )
            }),
            TypeId::F_AF_NA_1 => self.get_ack_file_or_section().map(|i| {
                write!(
                    f,
                    " IOA={} NOF={} NOS={} AFQ=0x{:02x}",
                    i.ioa,
                    i.nof,
                    i.nos,
                    i.afq.value()
                )
            }),
            // The segment payload itself is not dumped: a file transfer fills
            // the log with it, and its length is what a reader needs.
            TypeId::F_SG_NA_1 => self.get_file_segment().map(|i| {
                write!(
                    f,
                    " IOA={} NOF={} NOS={} segment={}B",
                    i.ioa,
                    i.nof,
                    i.nos,
                    i.segment.len()
                )
            }),
            TypeId::F_DR_TA_1 => self.get_file_directory().map(|infos| {
                list(f, &infos, |f, i| {
                    write!(
                        f,
                        "{}=NOF({}) {}B SOF=0x{:02x}{}",
                        i.ioa,
                        i.nof,
                        i.length_of_file,
                        i.sof.value(),
                        tsf(i.time, i.time_flags)
                    )
                })
            }),
            _ => {
                let n = self.variable().number.max(1);
                return write!(f, " items={n} payload={}B", self.info_obj.len());
            }
        };

        match decoded {
            Ok(r) => r,
            Err(_) => write!(f, " <undecodable payload={}B>", self.info_obj.len()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asdu::time::TimeTagFlags;
    use crate::asdu::identifier::{Cause, CauseOfTransmission};
    use crate::asdu::info::*;
    use crate::asdu::mproc::*;
    use crate::asdu::params::PARAMS_WIDE;

    #[test]
    fn single_point_dump_lists_every_object() {
        let a = Asdu::single(
            PARAMS_WIDE,
            false,
            CauseOfTransmission::new(Cause::SPONTANEOUS),
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
        let s = a.to_string();
        assert!(s.starts_with("TID<M_SP_NA_1> COT<Spontaneous> @1 VSQ<2> IOA-Width=3"));
        assert!(s.contains("items=2"));
        assert!(s.contains("100=true"));
        assert!(s.contains("101=false QDS=0x80"));
    }

    #[test]
    fn dump_is_non_destructive_and_repeatable() {
        let a = Asdu::single(
            PARAMS_WIDE,
            false,
            CauseOfTransmission::new(Cause::SPONTANEOUS),
            1,
            &[SinglePointInfo::new(100, true)],
        )
        .unwrap();
        let first = a.to_string();
        assert_eq!(a.get_single_point().unwrap().len(), 1);
        assert_eq!(a.to_string(), first);
        assert_eq!(a.get_single_point().unwrap().len(), 1);
    }

    #[test]
    fn unknown_types_fall_back_to_a_byte_summary() {
        let mut a = Asdu::new_empty(PARAMS_WIDE);
        a.identifier.type_id = TypeId(200);
        a.identifier.coa = CauseOfTransmission::new(Cause::SPONTANEOUS);
        a.identifier.common_addr = 1;
        a.identifier.variable.number = 1;
        a.info_obj = vec![1, 2, 3];
        assert!(a.to_string().ends_with("items=1 payload=3B"));
    }

    #[test]
    fn a_truncated_payload_never_makes_formatting_fail() {
        let mut a = Asdu::new_empty(PARAMS_WIDE);
        a.identifier.type_id = TypeId::M_SP_NA_1;
        a.identifier.coa = CauseOfTransmission::new(Cause::SPONTANEOUS);
        a.identifier.common_addr = 1;
        a.identifier.variable.number = 4; // claims 4 objects
        a.info_obj = vec![1, 2, 3]; // but carries less than one
        assert!(a.to_string().contains("<undecodable"));
    }
}
