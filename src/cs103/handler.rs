// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The application handler of the IEC 60870-5-103 primary station, and the
//! [`Link`] handle it uses to talk back to the devices.

use chrono::Utc;

use crate::cs103::asdu::{Asdu, IdentificationInfo, MeasurandsInfo, TimeTaggedInfo, TypeId};
use crate::cs103::elements::Dco;
use crate::error::Result;

/// What a handler can do with the link it was called from.
///
/// Implemented by [`Client`](crate::cs103::Client). Everything is queued and
/// transmitted by the protocol loop once the target device's link is free, so
/// none of these block.
pub trait Link: Send + Sync {
    /// Queue an ASDU for the device at `addr`.
    fn send_to(&self, a: Asdu, addr: u8) -> Result<()>;

    /// Whether at least one device's link layer is active.
    fn is_link_active(&self) -> bool;

    /// Send a time synchronization (ASDU 6) carrying the current time.
    ///
    /// The device confirms with a monitor-direction ASDU 6.
    fn time_sync(&self, addr: u8) -> Result<()> {
        self.send_to(Asdu::time_sync(addr, Utc::now()), addr)
    }

    /// Initiate a general interrogation (ASDU 7) with the given scan number.
    ///
    /// The device answers with ASDU 1 messages carrying cause 9 and terminates
    /// with an ASDU 8 carrying the same scan number.
    fn general_interrogation(&self, addr: u8, scn: u8) -> Result<()> {
        self.send_to(Asdu::general_interrogation(addr, scn), addr)
    }

    /// Send a general command (ASDU 20).
    ///
    /// The acknowledgement returns as an ASDU 1 with cause 20 (positive) or 21
    /// (negative) whose supplementary information carries `rii`.
    fn general_command(&self, addr: u8, fun: u8, inf: u8, dco: Dco, rii: u8) -> Result<()> {
        self.send_to(Asdu::general_command(addr, fun, inf, dco, rii), addr)
    }
}

/// Handles ASDUs received from protection equipment.
///
/// Dispatch by received type identification:
///
/// | Type | Method |
/// |------|--------|
/// | 1, 2 | [`time_tagged`](ClientHandler::time_tagged) — events, interrogation replies and command acknowledgements |
/// | 3, 9 | [`measurands`](ClientHandler::measurands) |
/// | 5 | [`identification`](ClientHandler::identification) |
/// | 8 | [`gi_termination`](ClientHandler::gi_termination) |
/// | other | [`asdu`](ClientHandler::asdu) — ASDU 4, 6, generic and disturbance types |
///
/// The device that produced an ASDU is identified by its common address,
/// `pack.common_addr`. Every method has a default implementation that does
/// nothing, so an application only writes the ones it needs.
#[async_trait::async_trait]
pub trait ClientHandler: Send + Sync + 'static {
    /// ASDU 1 and 2: time-tagged messages.
    ///
    /// The cause distinguishes what one means:
    /// [`Cause::SPONTANEOUS`](crate::cs103::Cause::SPONTANEOUS) is an event,
    /// [`Cause::GI`](crate::cs103::Cause::GI) an interrogation reply, and
    /// [`Cause::COMMAND_ACK_POS`](crate::cs103::Cause::COMMAND_ACK_POS) or
    /// [`Cause::COMMAND_ACK_NEG`](crate::cs103::Cause::COMMAND_ACK_NEG) a
    /// command result whose `info.sin` carries the RII of the original command.
    async fn time_tagged(&self, link: &dyn Link, pack: &Asdu, info: TimeTaggedInfo) -> Result<()> {
        let (_, _, _) = (link, pack, info);
        Ok(())
    }

    /// ASDU 3 and 9: measurands. `info.values[i].f64()` is the fraction of
    /// full scale.
    async fn measurands(&self, link: &dyn Link, pack: &Asdu, info: MeasurandsInfo) -> Result<()> {
        let (_, _, _) = (link, pack, info);
        Ok(())
    }

    /// ASDU 5: the identification message a device reports after a reset.
    async fn identification(
        &self,
        link: &dyn Link,
        pack: &Asdu,
        info: IdentificationInfo,
    ) -> Result<()> {
        let (_, _, _) = (link, pack, info);
        Ok(())
    }

    /// ASDU 8: end of general interrogation, with the scan number that completed.
    async fn gi_termination(&self, link: &dyn Link, pack: &Asdu, scn: u8) -> Result<()> {
        let (_, _, _) = (link, pack, scn);
        Ok(())
    }

    /// Everything else: ASDU 4, 6, the generic services and disturbance data.
    async fn asdu(&self, link: &dyn Link, pack: &Asdu) -> Result<()> {
        let (_, _) = (link, pack);
        Ok(())
    }

    /// Called for every received ASDU before routing, for logging or forwarding.
    async fn asdu_all(&self, link: &dyn Link, pack: &Asdu) -> Result<()> {
        let (_, _) = (link, pack);
        Ok(())
    }

    /// The line came alive: the first frame of this connection was received.
    async fn on_connect(&self, link: &dyn Link) {
        let _ = link;
    }

    /// A device's link became active, after its reset was confirmed.
    ///
    /// With [`Config::auto_init`](crate::cs103::Config::auto_init) the client
    /// has already queued a time synchronization and a general interrogation
    /// by the time this runs.
    async fn on_device_active(&self, link: &dyn Link, addr: u8) {
        let (_, _) = (link, addr);
    }

    /// The transport went down.
    async fn on_connection_lost(&self, link: &dyn Link) {
        let _ = link;
    }
}

/// Route one received ASDU to the matching [`ClientHandler`] method.
pub(crate) async fn dispatch<H: ClientHandler>(handler: &H, link: &dyn Link, pack: &Asdu) {
    tracing::debug!(asdu = %pack, "RX ASDU");
    if let Err(e) = handler.asdu_all(link, pack).await {
        tracing::warn!(error = %e, "asdu_all handler failed");
    }

    let r = async {
        match pack.type_id {
            TypeId::TIME_TAGGED | TypeId::TIME_TAGGED_REL => {
                let info = pack.get_time_tagged()?;
                handler.time_tagged(link, pack, info).await
            }
            TypeId::MEASURANDS_I | TypeId::MEASURANDS_II => {
                let info = pack.get_measurands()?;
                handler.measurands(link, pack, info).await
            }
            TypeId::IDENTIFICATION => {
                let info = pack.get_identification()?;
                handler.identification(link, pack, info).await
            }
            TypeId::GI_TERMINATION => {
                let scn = pack.get_gi_termination()?;
                handler.gi_termination(link, pack, scn).await
            }
            _ => handler.asdu(link, pack).await,
        }
    }
    .await;

    if let Err(e) = r {
        tracing::warn!(error = %e, "client handler failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cs103::asdu::Cause;
    use crate::cs103::elements::{fun, inf};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder {
        queued: Mutex<Vec<(Asdu, u8)>>,
    }

    impl Link for Recorder {
        fn send_to(&self, a: Asdu, addr: u8) -> Result<()> {
            self.queued.lock().unwrap().push((a, addr));
            Ok(())
        }

        fn is_link_active(&self) -> bool {
            true
        }
    }

    #[test]
    fn the_link_helpers_build_the_right_control_direction_asdus() {
        let link = Recorder::default();
        link.time_sync(3).unwrap();
        link.general_interrogation(3, 7).unwrap();
        link.general_command(3, fun::OVERCURRENT_PROTECTION, inf::LED_RESET, Dco::On, 9)
            .unwrap();

        let queued = link.queued.lock().unwrap();
        assert_eq!(queued.len(), 3);
        assert!(queued.iter().all(|(_, addr)| *addr == 3));

        assert_eq!(queued[0].0.type_id, TypeId::TIME_SYNC);
        assert_eq!(queued[1].0.type_id, TypeId::GENERAL_INTERROGATION);
        assert_eq!(queued[1].0.get_general_interrogation().unwrap(), 7);

        let cmd = queued[2].0.get_general_command().unwrap();
        assert_eq!(cmd.dco, Dco::On);
        assert_eq!(cmd.rii, 9);
    }

    /// Records which handler method each ASDU reached.
    #[derive(Default)]
    struct Routed {
        seen: Mutex<Vec<&'static str>>,
    }

    #[async_trait::async_trait]
    impl ClientHandler for Routed {
        async fn time_tagged(&self, _: &dyn Link, _: &Asdu, _: TimeTaggedInfo) -> Result<()> {
            self.seen.lock().unwrap().push("time_tagged");
            Ok(())
        }
        async fn measurands(&self, _: &dyn Link, _: &Asdu, _: MeasurandsInfo) -> Result<()> {
            self.seen.lock().unwrap().push("measurands");
            Ok(())
        }
        async fn identification(&self, _: &dyn Link, _: &Asdu, _: IdentificationInfo) -> Result<()> {
            self.seen.lock().unwrap().push("identification");
            Ok(())
        }
        async fn gi_termination(&self, _: &dyn Link, _: &Asdu, _: u8) -> Result<()> {
            self.seen.lock().unwrap().push("gi_termination");
            Ok(())
        }
        async fn asdu(&self, _: &dyn Link, _: &Asdu) -> Result<()> {
            self.seen.lock().unwrap().push("asdu");
            Ok(())
        }
        async fn asdu_all(&self, _: &dyn Link, _: &Asdu) -> Result<()> {
            self.seen.lock().unwrap().push("all");
            Ok(())
        }
    }

    fn vsq() -> crate::asdu::VariableStruct {
        crate::asdu::VariableStruct {
            number: 1,
            is_sequence: true,
        }
    }

    #[tokio::test]
    async fn every_type_reaches_its_dedicated_method() {
        let handler = Routed::default();
        let link = Recorder::default();

        let mut tt = Asdu::new(TypeId::TIME_TAGGED, vsq(), Cause::SPONTANEOUS, 3);
        tt.append(&[fun::OVERCURRENT_PROTECTION, inf::GENERAL_TRIP, 2]);
        tt.append(&crate::cs103::elements::cp32time2a(
            Some(Utc::now()),
            crate::asdu::TimeZone::Utc,
        ));
        tt.append(&[0]);

        let mut me = Asdu::new(TypeId::MEASURANDS_I, vsq(), Cause::CYCLIC, 3);
        me.append(&[fun::OVERCURRENT_PROTECTION, inf::MEASURAND_I, 0, 0]);

        let mut id = Asdu::new(TypeId::IDENTIFICATION, vsq(), Cause::RESET_CU, 3);
        id.append(&[fun::GLOBAL, inf::RESET_CU, 4]);
        id.append(b"RELAY");

        let mut gi = Asdu::new(TypeId::GI_TERMINATION, vsq(), Cause::GI_TERMINATION, 3);
        gi.append(&[fun::GLOBAL, 0, 1]);

        let other = Asdu::time_sync(3, Utc::now());

        for a in [tt, me, id, gi, other] {
            dispatch(&handler, &link, &a).await;
        }

        assert_eq!(
            *handler.seen.lock().unwrap(),
            vec![
                "all",
                "time_tagged",
                "all",
                "measurands",
                "all",
                "identification",
                "all",
                "gi_termination",
                "all",
                "asdu",
            ],
            "asdu_all runs before every dispatch"
        );
    }

    #[tokio::test]
    async fn a_malformed_payload_is_logged_rather_than_dispatched() {
        let handler = Routed::default();
        let link = Recorder::default();

        // Claims to be a time-tagged message but is far too short to decode.
        let mut broken = Asdu::new(TypeId::TIME_TAGGED, vsq(), Cause::SPONTANEOUS, 3);
        broken.append(&[1, 2]);
        dispatch(&handler, &link, &broken).await;

        assert_eq!(
            *handler.seen.lock().unwrap(),
            vec!["all"],
            "only asdu_all runs; the decode error stops the dedicated dispatch"
        );
    }
}
