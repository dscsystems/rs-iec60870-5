// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! A minimal IEC 60870-5-103 protection device, for testing masters.
//!
//! The crate implements the master side only — as does go-iecp5 — so this
//! simulator is what both are driven against. It speaks the FT1.2 secondary
//! procedure over a TCP-encapsulated link and answers the standard 103
//! start-up: identification after reset, a time-sync mirror, a general
//! interrogation reply terminated by ASDU 8, command acknowledgements carrying
//! the RII, and cyclic measurands collected by class 2 polls.

#![allow(dead_code)] // each test binary uses a different subset

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use rs_iec60870_5::asdu::VariableStruct;
use rs_iec60870_5::cs101::{ControlField, Frame, prim_fc, read_frame, sec_fc};
use rs_iec60870_5::cs103::{Asdu, Cause, Dco, Dpi, Measurand, TypeId, cp32time2a, fun, inf};

/// What the master did, in the order it did it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Request status of link (FC 9).
    StatusRequest,
    /// Reset of the communication unit (FC 0).
    ResetCu,
    /// Reset of the frame count bit (FC 7).
    ResetFcb,
    /// Request user data class 1 (FC 10).
    Class1Poll,
    /// Request user data class 2 (FC 11).
    Class2Poll,
    /// Confirmed user data (FC 3) carrying an ASDU.
    UserData(Box<Asdu>),
    /// A frame repeated with the same frame count bit.
    RepeatedFrame,
}

/// A running device simulator.
pub struct RelaySim {
    /// Everything the master sent, in order.
    pub events: Arc<Mutex<Vec<Event>>>,
    /// The device's link and common address.
    pub addr: u8,
}

impl RelaySim {
    /// Bind an ephemeral port and serve one master.
    ///
    /// Returns the address to dial and the simulator's event log.
    pub async fn spawn(addr: u8) -> (SocketAddr, Arc<RelaySim>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let local = listener.local_addr().expect("local addr");

        let sim = Arc::new(RelaySim {
            events: Arc::new(Mutex::new(Vec::new())),
            addr,
        });

        {
            let sim = Arc::clone(&sim);
            tokio::spawn(async move {
                // Serve successive masters, so a test may reconnect.
                while let Ok((tcp, _)) = listener.accept().await {
                    sim.serve(tcp).await;
                }
            });
        }
        (local, sim)
    }

    /// Every event of a given shape seen so far.
    pub async fn events(&self) -> Vec<Event> {
        self.events.lock().await.clone()
    }

    /// The ASDUs the master sent, in order.
    pub async fn received_asdus(&self) -> Vec<Asdu> {
        self.events
            .lock()
            .await
            .iter()
            .filter_map(|e| match e {
                Event::UserData(a) => Some((**a).clone()),
                _ => None,
            })
            .collect()
    }

    /// Wait until `pred` holds over the event log, or fail the test.
    pub async fn wait(&self, what: &str, pred: impl Fn(&[Event]) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while tokio::time::Instant::now() < deadline {
            if pred(&self.events.lock().await) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let seen = self.events.lock().await.clone();
        panic!("timed out waiting for {what}; the device saw: {seen:#?}");
    }

    async fn log(&self, e: Event) {
        self.events.lock().await.push(e);
    }

    async fn serve(&self, tcp: tokio::net::TcpStream) {
        let (mut reader, mut writer) = tokio::io::split(tcp);

        let mut fcb_expected = true;
        let mut last_resp: Option<Frame> = None;
        let mut class1: VecDeque<Asdu> = VecDeque::new();
        let mut class2: VecDeque<Asdu> = VecDeque::new();
        // Something cyclic is always available, as a real relay would have.
        class2.push_back(self.measurands());

        loop {
            let frame = match read_frame(&mut reader, 1).await {
                Ok(f) => f,
                Err(rs_iec60870_5::Error::Frame(_)) => continue, // resynchronise
                Err(_) => return,                            // peer closed
            };

            let Some(ctrl) = frame.control() else { continue };
            if frame.link_addr() != Some(self.addr as u16) {
                continue; // addressed to another device on the line
            }
            if !ctrl.prm {
                continue; // 103 is unbalanced; a master never sends PRM=0
            }

            // Duplicate detection over every FCV frame, per IEC 60870-5-2.
            if ctrl.fcv {
                if ctrl.fcb != fcb_expected {
                    self.log(Event::RepeatedFrame).await;
                    if let Some(f) = last_resp.clone() {
                        Self::write(&mut writer, &f).await;
                    }
                    continue;
                }
                fcb_expected = !fcb_expected;
            }

            let response = match ctrl.fun {
                prim_fc::REQ_STATUS => {
                    self.log(Event::StatusRequest).await;
                    self.fixed(sec_fc::RESP_STATUS, !class1.is_empty())
                }

                prim_fc::RESET_LINK | 7 => {
                    self.log(if ctrl.fun == 7 {
                        Event::ResetFcb
                    } else {
                        Event::ResetCu
                    })
                    .await;
                    fcb_expected = true;
                    // After a reset the device reports what it is.
                    class1.push_back(self.identification());
                    self.fixed(sec_fc::CONF_ACK, true)
                }

                prim_fc::REQ_DATA1 => {
                    self.log(Event::Class1Poll).await;
                    self.class_response(class1.pop_front(), !class1.is_empty())
                }

                prim_fc::REQ_DATA2 => {
                    self.log(Event::Class2Poll).await;
                    // Keep one cyclic value permanently available.
                    let next = class2.pop_front();
                    if class2.is_empty() {
                        class2.push_back(self.measurands());
                    }
                    self.class_response(next, !class1.is_empty())
                }

                prim_fc::USER_DATA_CONF => {
                    if let Some(raw) = frame.asdu() {
                        if let Ok(a) = Asdu::unmarshal_binary(raw) {
                            self.react(&a, &mut class1);
                            self.log(Event::UserData(Box::new(a))).await;
                        }
                    }
                    self.fixed(sec_fc::CONF_ACK, !class1.is_empty())
                }

                _ => self.fixed(sec_fc::RESP_LINK_NI, false),
            };

            last_resp = Some(response.clone());
            Self::write(&mut writer, &response).await;
        }
    }

    async fn write(writer: &mut (impl AsyncWriteExt + Unpin), frame: &Frame) {
        if let Ok(raw) = frame.marshal(1) {
            let _ = writer.write_all(&raw).await;
        }
    }

    fn fixed(&self, fun: u8, acd: bool) -> Frame {
        Frame::Fixed {
            control: ControlField::secondary(fun, acd, false),
            link_addr: self.addr as u16,
        }
    }

    /// Answer a class poll with data, or with "requested data not available".
    fn class_response(&self, next: Option<Asdu>, acd: bool) -> Frame {
        match next.and_then(|a| a.marshal_binary().ok()) {
            Some(raw) => Frame::Variable {
                control: ControlField::secondary(sec_fc::USER_DATA_CONF, acd, false),
                link_addr: self.addr as u16,
                asdu: raw,
            },
            None => self.fixed(sec_fc::USER_DATA_NO_REP, acd),
        }
    }

    /// Queue whatever the received control-direction ASDU calls for.
    fn react(&self, a: &Asdu, class1: &mut VecDeque<Asdu>) {
        match a.type_id {
            // Time synchronization is mirrored back.
            TypeId::TIME_SYNC => {
                let mut m = a.clone();
                m.coa = Cause::TIME_SYNC;
                class1.push_back(m);
            }

            // A general interrogation returns the process image, then ASDU 8.
            TypeId::GENERAL_INTERROGATION => {
                let scn = a.get_general_interrogation().unwrap_or(0);
                class1.push_back(self.gi_reply(inf::GENERAL_TRIP, Dpi::Off, scn));
                class1.push_back(self.gi_reply(inf::AUTO_RECLOSER_ACTIVE, Dpi::On, scn));
                class1.push_back(self.gi_termination(scn));
            }

            // A general command is acknowledged with the RII echoed in SIN.
            TypeId::GENERAL_COMMAND => {
                if let Ok(cmd) = a.get_general_command() {
                    let dpi = if cmd.dco == Dco::On {
                        Dpi::On
                    } else {
                        Dpi::Off
                    };
                    class1.push_back(self.command_ack(cmd.fun, cmd.inf, dpi, cmd.rii));
                }
            }

            _ => {}
        }
    }

    fn vsq(n: u8) -> VariableStruct {
        VariableStruct {
            number: n,
            is_sequence: true,
        }
    }

    /// ASDU 5, reported after a reset.
    fn identification(&self) -> Asdu {
        let mut a = Asdu::new(
            TypeId::IDENTIFICATION,
            Self::vsq(1),
            Cause::RESET_CU,
            self.addr,
        );
        a.append(&[fun::GLOBAL, inf::RESET_CU, 4]);
        a.append(b"DSCRELAY");
        a
    }

    /// ASDU 3, the cyclic measurand set.
    fn measurands(&self) -> Asdu {
        let values = [
            Measurand {
                val: 2048,
                ..Default::default()
            },
            Measurand {
                val: -1024,
                ..Default::default()
            },
        ];
        let mut a = Asdu::new(
            TypeId::MEASURANDS_I,
            Self::vsq(values.len() as u8),
            Cause::CYCLIC,
            self.addr,
        );
        a.append(&[fun::OVERCURRENT_PROTECTION, inf::MEASURAND_IV]);
        for m in &values {
            a.append(&m.value().to_le_bytes());
        }
        a
    }

    /// ASDU 1 with cause 9: one object of the interrogation reply.
    fn gi_reply(&self, information_number: u8, dpi: Dpi, scn: u8) -> Asdu {
        self.time_tagged(
            Cause::GI,
            fun::OVERCURRENT_PROTECTION,
            information_number,
            dpi,
            scn,
        )
    }

    /// ASDU 1 with cause 20: a positive command acknowledgement.
    fn command_ack(&self, function: u8, information_number: u8, dpi: Dpi, rii: u8) -> Asdu {
        self.time_tagged(
            Cause::COMMAND_ACK_POS,
            function,
            information_number,
            dpi,
            rii,
        )
    }

    fn time_tagged(
        &self,
        cause: Cause,
        function: u8,
        information_number: u8,
        dpi: Dpi,
        sin: u8,
    ) -> Asdu {
        let mut a = Asdu::new(TypeId::TIME_TAGGED, Self::vsq(1), cause, self.addr);
        a.append(&[function, information_number, dpi.value()]);
        a.append(&cp32time2a(
            Some(chrono::Utc::now()),
            rs_iec60870_5::asdu::TimeZone::Utc,
        ));
        a.append(&[sin]);
        a
    }

    /// ASDU 8: end of the general interrogation with the given scan number.
    fn gi_termination(&self, scn: u8) -> Asdu {
        let mut a = Asdu::new(
            TypeId::GI_TERMINATION,
            Self::vsq(1),
            Cause::GI_TERMINATION,
            self.addr,
        );
        a.append(&[fun::GLOBAL, 0, scn]);
        a
    }
}
