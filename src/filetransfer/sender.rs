// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The controlled station (outstation) side of a monitor-direction transfer.

use std::sync::{Arc, Mutex};

use crate::asdu::{
    AckFileOrSectionInfo, AckFileOrSectionQualifier, AfqAction, Asdu, CallOrSelectFileInfo, Cause,
    CauseOfTransmission, CommonAddr, Connect, DirectoryInfo, FileError, FileReadyInfo, InfoObjAddr,
    LastSectionOrSegmentInfo, LastSectionQualifier, NameOfFile, Params, ScqAction, SectionReadyInfo,
    SegmentInfo, StatusOfFile, TypeId, file_checksum,
};
use crate::error::{Error, Result};
use crate::filetransfer::store::{Store, is_file_asdu, split_sections};

/// The default number of octets per section.
pub const DEFAULT_SECTION_SIZE: usize = 4096;

fn transfer() -> CauseOfTransmission {
    CauseOfTransmission::new(Cause::FILE_TRANSFER)
}

/// What one state transition produced: the ASDUs to transmit, and the error to
/// report to the caller.
///
/// The two are independent. A request for a file the store does not have is
/// answered on the wire with a negative acknowledgement *and* reported to the
/// application — the peer needs to be told, and so does the caller.
#[derive(Default)]
struct Reply {
    send: Vec<Asdu>,
    error: Option<Error>,
}

impl Reply {
    fn nothing() -> Reply {
        Reply::default()
    }

    fn send(asdus: Vec<Asdu>) -> Reply {
        Reply {
            send: asdus,
            error: None,
        }
    }

    fn failed(e: Error) -> Reply {
        Reply {
            send: Vec::new(),
            error: Some(e),
        }
    }

    /// Transmit `asdus`, and still report `e`.
    fn send_and_fail(asdus: Vec<Asdu>, e: Error) -> Reply {
        Reply {
            send: asdus,
            error: Some(e),
        }
    }
}

/// Turn a builder result into a reply, so an ASDU that will not encode is
/// reported rather than silently dropped.
impl From<Result<Vec<Asdu>>> for Reply {
    fn from(r: Result<Vec<Asdu>>) -> Reply {
        match r {
            Ok(a) => Reply::send(a),
            Err(e) => Reply::failed(e),
        }
    }
}

/// The transfer currently being served.
struct Active {
    ca: CommonAddr,
    ioa: InfoObjAddr,
    nof: NameOfFile,
    sections: Vec<Vec<u8>>,
    /// Index of the section being served.
    cur: usize,
}

/// The controlled station (outstation) side of a monitor-direction file
/// transfer: it announces files, answers directory calls, and serves the
/// sections and segments a controlling station asks for.
///
/// One transfer runs at a time, which is what the standard's procedure allows
/// per common address.
///
/// Feed it every ASDU your handler receives; it reports whether the ASDU
/// belonged to the file transfer service, and you continue with your own
/// dispatch when it did not:
///
/// ```no_run
/// # use std::sync::Arc;
/// # use rs_iec60870_5::asdu::{Asdu, Connect};
/// # use rs_iec60870_5::filetransfer::Sender;
/// # struct H { files: Arc<Sender> }
/// # #[async_trait::async_trait]
/// # impl rs_iec60870_5::cs104::ServerHandler for H {
/// async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
///     if self.files.handle(c, pack).await? {
///         return Ok(());
///     }
///     // ... the application's own types
///     Ok(())
/// }
/// # }
/// ```
pub struct Sender {
    store: Arc<dyn Store>,
    section_size: Mutex<usize>,
    active: Mutex<Option<Active>>,
}

impl Sender {
    /// A sender serving files out of `store`.
    pub fn new(store: Arc<dyn Store>) -> Sender {
        Sender {
            store,
            section_size: Mutex::new(DEFAULT_SECTION_SIZE),
            active: Mutex::new(None),
        }
    }

    /// Set the maximum number of octets per section.
    ///
    /// A section is the unit that carries a checksum, so smaller sections
    /// detect corruption earlier at the cost of more round trips. Zero is
    /// ignored.
    pub fn set_section_size(&self, n: usize) {
        if n > 0 {
            *self.section_size.lock().unwrap() = n;
        }
    }

    /// Whether a transfer is currently running.
    pub fn in_progress(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    /// Clear any transfer in progress without notifying the peer.
    pub fn abort(&self) {
        *self.active.lock().unwrap() = None;
    }

    /// Announce that a file is ready for transfer, with `F_FR_NA_1`.
    ///
    /// A controlling station typically answers by selecting the file, which
    /// starts the transfer.
    pub async fn offer(
        &self,
        conn: &dyn Connect,
        ca: CommonAddr,
        ioa: InfoObjAddr,
        nof: NameOfFile,
    ) -> Result<()> {
        let data = self.store.read(ioa, nof).await?;
        conn.send(Asdu::file_ready(
            conn.params(),
            transfer(),
            ca,
            FileReadyInfo {
                ioa,
                nof,
                length_of_file: data.len() as u32,
                frq: Default::default(),
            },
        )?)
        .await
    }

    /// Process one received ASDU.
    ///
    /// Returns `true` when the ASDU belonged to the file transfer service. A
    /// `false` return means the caller should continue with its own dispatch.
    pub async fn handle(&self, conn: &dyn Connect, a: &Asdu) -> Result<bool> {
        if !is_file_asdu(a.type_id()) {
            return Ok(false);
        }
        let reply = match a.type_id() {
            TypeId::F_SC_NA_1 => self.on_call_or_select(conn.params(), a).await,
            TypeId::F_AF_NA_1 => self.on_ack(conn.params(), a),
            // Monitor-direction ASDUs: this component produces them, a
            // controlled station does not receive them.
            _ => Reply::failed(Error::FileServiceUnsupported),
        };

        // The ASDUs go out with the lock released, so that a transport whose
        // send is synchronous cannot re-enter this state machine while it is
        // held.
        for asdu in reply.send {
            if let Err(e) = conn.send(asdu).await {
                self.abort();
                return Err(e);
            }
        }
        match reply.error {
            Some(e) => Err(e),
            None => Ok(true),
        }
    }

    async fn on_call_or_select(&self, params: Params, a: &Asdu) -> Reply {
        let req = match a.get_call_or_select_file() {
            Ok(r) => r,
            Err(e) => return Reply::failed(e),
        };
        let ca = a.common_addr();

        match req.scq.action {
            ScqAction::DEFAULT => self.directory(params, ca).await,

            ScqAction::SELECT_FILE | ScqAction::REQUEST_FILE => {
                let data = match self.store.read(req.ioa, req.nof).await {
                    Ok(d) => d,
                    Err(e) => {
                        return self.neg_ack_reply(
                            params,
                            ca,
                            &req,
                            AfqAction::NEG_ACK_FILE,
                            FileError::UNEXPECTED_NAME_OF_FILE,
                            e,
                        );
                    }
                };
                let section_size = *self.section_size.lock().unwrap();
                let mut active = self.active.lock().unwrap();
                *active = Some(Active {
                    ca,
                    ioa: req.ioa,
                    nof: req.nof,
                    sections: split_sections(&data, section_size),
                    cur: 0,
                });
                Self::section_ready(params, active.as_ref().expect("just set")).into()
            }

            ScqAction::SELECT_SECTION | ScqAction::REQUEST_SECTION => {
                let requesting_data = req.scq.action == ScqAction::REQUEST_SECTION;
                let mut active = self.active.lock().unwrap();

                let Some(t) = active.as_mut().filter(|t| t.nof == req.nof) else {
                    drop(active);
                    return self.neg_ack_reply(
                        params,
                        ca,
                        &req,
                        AfqAction::NEG_ACK_FILE,
                        FileError::UNEXPECTED_NAME_OF_FILE,
                        Error::NoTransfer,
                    );
                };

                // Section 0 on a data request means "the one already
                // announced"; anywhere else the number must name a section.
                if !(requesting_data && req.nos == 0) {
                    let Some(idx) = section_index(req.nos, t.sections.len()) else {
                        drop(active);
                        return self.neg_ack_reply(
                            params,
                            ca,
                            &req,
                            AfqAction::NEG_ACK_SECTION,
                            FileError::UNEXPECTED_NAME_OF_SECTION,
                            Error::NoTransfer,
                        );
                    };
                    t.cur = idx;
                }

                if requesting_data {
                    Self::section_data(params, t).into()
                } else {
                    Self::section_ready(params, t).into()
                }
            }

            ScqAction::DEACTIVATE_FILE | ScqAction::DEACTIVATE_SECTION => {
                *self.active.lock().unwrap() = None;
                Reply::nothing()
            }

            ScqAction::DELETE_FILE => {
                if let Err(e) = self.store.delete(req.ioa, req.nof).await {
                    return self.neg_ack_reply(
                        params,
                        ca,
                        &req,
                        AfqAction::NEG_ACK_FILE,
                        FileError::UNEXPECTED_NAME_OF_FILE,
                        e,
                    );
                }
                let mut active = self.active.lock().unwrap();
                if active
                    .as_ref()
                    .is_some_and(|t| t.ioa == req.ioa && t.nof == req.nof)
                {
                    *active = None;
                }
                Reply::nothing()
            }

            _ => Reply::nothing(),
        }
    }

    fn on_ack(&self, params: Params, a: &Asdu) -> Reply {
        let ack = match a.get_ack_file_or_section() {
            Ok(r) => r,
            Err(e) => return Reply::failed(e),
        };
        let mut active = self.active.lock().unwrap();
        let Some(t) = active.as_mut() else {
            return Reply::failed(Error::NoTransfer);
        };

        match ack.afq.action {
            AfqAction::POS_ACK_SECTION => {
                t.cur += 1;
                if t.cur < t.sections.len() {
                    Self::section_ready(params, t).into()
                } else {
                    // Every section is acknowledged: close the file.
                    Self::last_section(params, t).into()
                }
            }
            // The controlling station rejected the section, typically a
            // checksum mismatch: serve it again.
            AfqAction::NEG_ACK_SECTION => Self::section_data(params, t).into(),
            AfqAction::POS_ACK_FILE | AfqAction::NEG_ACK_FILE => {
                *active = None;
                Reply::nothing()
            }
            _ => Reply::nothing(),
        }
    }

    /// The directory reply, `F_DR_TA_1`.
    async fn directory(&self, params: Params, ca: CommonAddr) -> Reply {
        let entries = match self.store.list().await {
            Ok(e) => e,
            Err(e) => return Reply::failed(e),
        };
        if entries.is_empty() {
            // The standard has no empty-directory ASDU, so stay silent rather
            // than sending a malformed one.
            return Reply::nothing();
        }
        let last = entries.len() - 1;
        let infos: Vec<DirectoryInfo> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| DirectoryInfo {
                ioa: e.ioa,
                nof: e.nof,
                length_of_file: e.size,
                sof: StatusOfFile {
                    is_last_file_of_directory: i == last,
                    is_directory: e.is_directory,
                    ..Default::default()
                },
                time: e.time.or_else(|| Some(chrono::Utc::now())),
            })
            .collect();
        Asdu::file_directory(params, CauseOfTransmission::new(Cause::REQUEST), ca, &infos)
            .map(|a| vec![a])
            .into()
    }

    /// Announce the current section, `F_SR_NA_1`.
    fn section_ready(params: Params, t: &Active) -> Result<Vec<Asdu>> {
        Ok(vec![Asdu::section_ready(
            params,
            transfer(),
            t.ca,
            SectionReadyInfo {
                ioa: t.ioa,
                nof: t.nof,
                nos: (t.cur + 1) as u8,
                length_of_section: t.sections[t.cur].len() as u32,
                srq: Default::default(),
            },
        )?])
    }

    /// Transmit the current section as segments, followed by the last-segment
    /// marker carrying the section checksum.
    fn section_data(params: Params, t: &Active) -> Result<Vec<Asdu>> {
        let section = &t.sections[t.cur];
        let nos = (t.cur + 1) as u8;
        let max = params.max_segment_size();
        if max == 0 {
            return Err(Error::Param);
        }

        let mut out = Vec::with_capacity(section.len() / max + 2);
        // An empty section still sends no segments and only the marker, which
        // is what its zero checksum describes.
        for chunk in section.chunks(max) {
            out.push(Asdu::file_segment(
                params,
                transfer(),
                t.ca,
                &SegmentInfo {
                    ioa: t.ioa,
                    nof: t.nof,
                    nos,
                    segment: chunk.to_vec(),
                },
            )?);
        }
        out.push(Asdu::last_section_or_segment(
            params,
            transfer(),
            t.ca,
            LastSectionOrSegmentInfo {
                ioa: t.ioa,
                nof: t.nof,
                nos,
                lsq: LastSectionQualifier::SECTION_WITHOUT_DEACTIVATE,
                chs: file_checksum(section),
            },
        )?);
        Ok(out)
    }

    /// Close the file: `F_LS_NA_1` with an end-of-file qualifier.
    fn last_section(params: Params, t: &Active) -> Result<Vec<Asdu>> {
        Ok(vec![Asdu::last_section_or_segment(
            params,
            transfer(),
            t.ca,
            LastSectionOrSegmentInfo {
                ioa: t.ioa,
                nof: t.nof,
                nos: t.sections.len() as u8,
                lsq: LastSectionQualifier::FILE_WITHOUT_DEACTIVATE,
                chs: 0,
            },
        )?])
    }

    /// Tell the peer why the request failed, and report `e` to the caller too.
    fn neg_ack_reply(
        &self,
        params: Params,
        ca: CommonAddr,
        req: &CallOrSelectFileInfo,
        action: AfqAction,
        error: FileError,
        e: Error,
    ) -> Reply {
        let neg = Asdu::ack_file_or_section(
            params,
            transfer(),
            ca,
            AckFileOrSectionInfo {
                ioa: req.ioa,
                nof: req.nof,
                nos: req.nos,
                afq: AckFileOrSectionQualifier { action, error },
            },
        );
        match neg {
            Ok(a) => Reply::send_and_fail(vec![a], e),
            // The acknowledgement itself will not encode; the original failure
            // is still the one worth reporting.
            Err(_) => Reply::failed(e),
        }
    }
}

/// Translate a one-based section number into an index, or `None` when it names
/// no section of this file.
fn section_index(nos: u8, len: usize) -> Option<usize> {
    let idx = (nos as usize).checked_sub(1)?;
    (idx < len).then_some(idx)
}
