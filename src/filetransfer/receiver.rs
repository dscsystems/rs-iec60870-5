// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The controlling station (master) side of a monitor-direction transfer.

use std::sync::{Arc, Mutex};

use crate::asdu::{
    AckFileOrSectionInfo, AckFileOrSectionQualifier, AfqAction, Asdu, CallOrSelectFileInfo, Cause,
    CauseOfTransmission, CommonAddr, Connect, DirectoryInfo, FileError, INFO_OBJ_ADDR_IRRELEVANT,
    InfoObjAddr, LastSectionQualifier, NameOfFile, Params, ScqAction, SelectAndCallQualifier,
    TypeId, file_checksum,
};
use crate::error::{Error, Result};
use crate::filetransfer::store::{Entry, Store, is_file_asdu};

fn transfer() -> CauseOfTransmission {
    CauseOfTransmission::new(Cause::FILE_TRANSFER)
}

/// Called with each completed file.
pub type FileHandler = Box<dyn Fn(Entry, Vec<u8>) + Send + Sync>;
/// Called with each received directory listing.
pub type DirectoryHandler = Box<dyn Fn(CommonAddr, Vec<DirectoryInfo>) + Send + Sync>;

/// The transfer currently being assembled.
struct Active {
    ca: CommonAddr,
    ioa: InfoObjAddr,
    nof: NameOfFile,
    nos: u8,
    section_buf: Vec<u8>,
    file_buf: Vec<u8>,
}

/// What one state transition produced.
#[derive(Default)]
struct Reply {
    send: Vec<Asdu>,
    /// A file that is now complete, ready to be stored and handed to the
    /// callback.
    file: Option<(Entry, Vec<u8>)>,
    /// A directory listing to hand to the callback.
    directory: Option<(CommonAddr, Vec<DirectoryInfo>)>,
    error: Option<Error>,
}

impl Reply {
    fn nothing() -> Reply {
        Reply::default()
    }

    fn failed(e: Error) -> Reply {
        Reply {
            error: Some(e),
            ..Default::default()
        }
    }

    fn send(asdus: Vec<Asdu>) -> Reply {
        Reply {
            send: asdus,
            ..Default::default()
        }
    }
}

impl From<Result<Vec<Asdu>>> for Reply {
    fn from(r: Result<Vec<Asdu>>) -> Reply {
        match r {
            Ok(a) => Reply::send(a),
            Err(e) => Reply::failed(e),
        }
    }
}

/// The controlling station (master) side of a monitor-direction file transfer:
/// it calls the directory, selects files, requests their sections, verifies
/// each section checksum and assembles the result.
///
/// One transfer runs at a time.
///
/// Feed it every ASDU your handler receives; it reports whether the ASDU
/// belonged to the file transfer service, and you continue with your own
/// dispatch when it did not:
///
/// ```no_run
/// # use std::sync::Arc;
/// # use rs_iec60870_5::asdu::{Asdu, Connect};
/// # use rs_iec60870_5::filetransfer::Receiver;
/// # struct H { files: Arc<Receiver> }
/// # #[async_trait::async_trait]
/// # impl rs_iec60870_5::cs104::ClientHandler for H {
/// async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
///     if self.files.handle(c, pack).await? {
///         return Ok(());
///     }
///     // ... the application's own process data
///     Ok(())
/// }
/// # }
/// ```
pub struct Receiver {
    store: Option<Arc<dyn Store>>,
    auto_accept: Mutex<bool>,
    active: Mutex<Option<Active>>,
    on_file: Mutex<Option<FileHandler>>,
    on_directory: Mutex<Option<DirectoryHandler>>,
}

impl Receiver {
    /// A receiver storing completed files in `store`.
    pub fn new(store: Arc<dyn Store>) -> Receiver {
        Receiver::build(Some(store))
    }

    /// A receiver that only reports completed files through
    /// [`Receiver::set_file_handler`], without storing them.
    pub fn without_store() -> Receiver {
        Receiver::build(None)
    }

    fn build(store: Option<Arc<dyn Store>>) -> Receiver {
        Receiver {
            store,
            auto_accept: Mutex::new(true),
            active: Mutex::new(None),
            on_file: Mutex::new(None),
            on_directory: Mutex::new(None),
        }
    }

    /// Whether a file the outstation announces with `F_FR_NA_1` is selected
    /// automatically.
    ///
    /// On by default. Set it off to decide per file and call
    /// [`Receiver::request_file`] yourself.
    pub fn set_auto_accept(&self, on: bool) {
        *self.auto_accept.lock().unwrap() = on;
    }

    /// Set the callback invoked with each completed file.
    pub fn set_file_handler(&self, f: FileHandler) {
        *self.on_file.lock().unwrap() = Some(f);
    }

    /// Set the callback invoked with each received directory listing.
    pub fn set_directory_handler(&self, f: DirectoryHandler) {
        *self.on_directory.lock().unwrap() = Some(f);
    }

    /// Whether a transfer is currently running.
    pub fn in_progress(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    /// Clear any transfer in progress without notifying the peer.
    pub fn abort(&self) {
        *self.active.lock().unwrap() = None;
    }

    /// Call the directory of `ca`: `F_SC_NA_1` with the default qualifier.
    pub async fn request_directory(&self, conn: &dyn Connect, ca: CommonAddr) -> Result<()> {
        conn.send(Asdu::call_or_select_file(
            conn.params(),
            CauseOfTransmission::new(Cause::REQUEST),
            ca,
            CallOrSelectFileInfo {
                ioa: INFO_OBJ_ADDR_IRRELEVANT,
                nof: NameOfFile::DEFAULT,
                nos: 0,
                scq: SelectAndCallQualifier {
                    action: ScqAction::DEFAULT,
                    error: FileError::NONE,
                },
            },
        )?)
        .await
    }

    /// Select a file for transfer: `F_SC_NA_1` with the select-file qualifier.
    ///
    /// The outstation answers with the first section, and the transfer then
    /// runs on its own until the file is complete.
    pub async fn request_file(
        &self,
        conn: &dyn Connect,
        ca: CommonAddr,
        ioa: InfoObjAddr,
        nof: NameOfFile,
    ) -> Result<()> {
        {
            let mut active = self.active.lock().unwrap();
            if active.is_some() {
                return Err(Error::TransferBusy);
            }
            *active = Some(Active {
                ca,
                ioa,
                nof,
                nos: 0,
                section_buf: Vec::new(),
                file_buf: Vec::new(),
            });
        }
        let r = conn
            .send(Asdu::call_or_select_file(
                conn.params(),
                transfer(),
                ca,
                CallOrSelectFileInfo {
                    ioa,
                    nof,
                    nos: 0,
                    scq: SelectAndCallQualifier {
                        action: ScqAction::SELECT_FILE,
                        error: FileError::NONE,
                    },
                },
            )?)
            .await;
        if r.is_err() {
            // The transfer never started, so it must not be left marked busy.
            self.abort();
        }
        r
    }

    /// Process one received ASDU.
    ///
    /// Returns `true` when the ASDU belonged to the file transfer service. A
    /// `false` return means the caller should continue with its own dispatch.
    pub async fn handle(&self, conn: &dyn Connect, a: &Asdu) -> Result<bool> {
        if !is_file_asdu(a.type_id()) {
            return Ok(false);
        }
        let reply = self.step(conn.params(), a);

        // Storing, the callbacks and the sends all run with the lock released.
        let mut err = reply.error;
        if let Some((entry, data)) = reply.file {
            if let Some(store) = &self.store
                && let Err(e) = store.write(entry.ioa, entry.nof, data.clone()).await
                && err.is_none()
            {
                err = Some(e);
            }
            if let Some(cb) = self.on_file.lock().unwrap().as_ref() {
                cb(entry, data);
            }
        }
        if let Some((ca, dir)) = reply.directory
            && let Some(cb) = self.on_directory.lock().unwrap().as_ref()
        {
            cb(ca, dir);
        }
        for asdu in reply.send {
            if let Err(e) = conn.send(asdu).await {
                self.abort();
                return Err(e);
            }
        }
        match err {
            Some(e) => Err(e),
            None => Ok(true),
        }
    }

    fn step(&self, params: Params, a: &Asdu) -> Reply {
        match a.type_id() {
            TypeId::F_DR_TA_1 => match a.get_file_directory() {
                Ok(dir) => Reply {
                    directory: Some((a.common_addr(), dir)),
                    ..Default::default()
                },
                Err(e) => Reply::failed(e),
            },
            TypeId::F_FR_NA_1 => self.on_file_ready(params, a),
            TypeId::F_SR_NA_1 => self.on_section_ready(params, a),
            TypeId::F_SG_NA_1 => self.on_segment(a),
            TypeId::F_LS_NA_1 => self.on_last_section_or_segment(params, a),
            // Control-direction ASDUs: this component produces them, a
            // controlling station does not receive them.
            TypeId::F_AF_NA_1 | TypeId::F_SC_NA_1 => Reply::failed(Error::FileServiceUnsupported),
            _ => Reply::nothing(),
        }
    }

    fn on_file_ready(&self, params: Params, a: &Asdu) -> Reply {
        let info = match a.get_file_ready() {
            Ok(i) => i,
            Err(e) => return Reply::failed(e),
        };
        if info.frq.is_negative {
            // The outstation cannot serve this file after all.
            self.abort();
            return Reply::failed(Error::FileNotFound);
        }

        let mut active = self.active.lock().unwrap();
        if !*self.auto_accept.lock().unwrap() || active.is_some() {
            return Reply::nothing();
        }
        *active = Some(Active {
            ca: a.common_addr(),
            ioa: info.ioa,
            nof: info.nof,
            nos: 0,
            section_buf: Vec::new(),
            file_buf: Vec::new(),
        });
        let t = active.as_ref().expect("just set");
        Asdu::call_or_select_file(
            params,
            transfer(),
            t.ca,
            CallOrSelectFileInfo {
                ioa: t.ioa,
                nof: t.nof,
                nos: 0,
                scq: SelectAndCallQualifier {
                    action: ScqAction::SELECT_FILE,
                    error: FileError::NONE,
                },
            },
        )
        .map(|a| vec![a])
        .into()
    }

    fn on_section_ready(&self, params: Params, a: &Asdu) -> Reply {
        let info = match a.get_section_ready() {
            Ok(i) => i,
            Err(e) => return Reply::failed(e),
        };
        let mut active = self.active.lock().unwrap();
        let Some(t) = active.as_mut().filter(|t| t.nof == info.nof) else {
            return Reply::failed(Error::NoTransfer);
        };
        if info.srq.is_not_ready {
            *active = None;
            return Reply::failed(Error::NoTransfer);
        }
        t.nos = info.nos;
        t.section_buf.clear();

        Asdu::call_or_select_file(
            params,
            transfer(),
            t.ca,
            CallOrSelectFileInfo {
                ioa: t.ioa,
                nof: t.nof,
                nos: t.nos,
                scq: SelectAndCallQualifier {
                    action: ScqAction::REQUEST_SECTION,
                    error: FileError::NONE,
                },
            },
        )
        .map(|a| vec![a])
        .into()
    }

    fn on_segment(&self, a: &Asdu) -> Reply {
        let info = match a.get_file_segment() {
            Ok(i) => i,
            Err(e) => return Reply::failed(e),
        };
        let mut active = self.active.lock().unwrap();
        let Some(t) = active
            .as_mut()
            .filter(|t| t.nof == info.nof && t.nos == info.nos)
        else {
            return Reply::failed(Error::NoTransfer);
        };
        t.section_buf.extend_from_slice(&info.segment);
        Reply::nothing()
    }

    fn on_last_section_or_segment(&self, params: Params, a: &Asdu) -> Reply {
        let info = match a.get_last_section_or_segment() {
            Ok(i) => i,
            Err(e) => return Reply::failed(e),
        };
        let mut active = self.active.lock().unwrap();
        let Some(t) = active.as_mut().filter(|t| t.nof == info.nof) else {
            return Reply::failed(Error::NoTransfer);
        };
        let (ca, ioa, nof) = (t.ca, t.ioa, t.nof);

        let ack = |nos, action| {
            Asdu::ack_file_or_section(
                params,
                transfer(),
                ca,
                AckFileOrSectionInfo {
                    ioa,
                    nof,
                    nos,
                    afq: AckFileOrSectionQualifier {
                        action,
                        error: FileError::NONE,
                    },
                },
            )
        };

        if info.lsq.is_end_of_file() {
            // End of the file: acknowledge and deliver.
            let data = std::mem::take(&mut t.file_buf);
            let entry = Entry {
                ioa,
                nof,
                size: data.len() as u32,
                time: None,
                is_directory: false,
            };
            *active = None;
            return match ack(info.nos, AfqAction::POS_ACK_FILE) {
                Ok(a) => Reply {
                    send: vec![a],
                    file: Some((entry, data)),
                    ..Default::default()
                },
                Err(e) => Reply::failed(e),
            };
        }

        match info.lsq {
            LastSectionQualifier::SECTION_WITHOUT_DEACTIVATE
            | LastSectionQualifier::SECTION_WITH_DEACTIVATE => {
                // End of a section: verify its checksum before acknowledging.
                // A section that does not match is rejected rather than
                // appended, so a corrupted file is never assembled silently.
                if file_checksum(&t.section_buf) != info.chs {
                    let nos = t.nos;
                    t.section_buf.clear();
                    let neg = Asdu::ack_file_or_section(
                        params,
                        transfer(),
                        ca,
                        AckFileOrSectionInfo {
                            ioa,
                            nof,
                            nos,
                            afq: AckFileOrSectionQualifier {
                                action: AfqAction::NEG_ACK_SECTION,
                                error: FileError::CHECKSUM_FAILED,
                            },
                        },
                    );
                    return match neg {
                        Ok(a) => Reply {
                            send: vec![a],
                            error: Some(Error::FileChecksum),
                            ..Default::default()
                        },
                        Err(e) => Reply::failed(e),
                    };
                }
                let section = std::mem::take(&mut t.section_buf);
                t.file_buf.extend_from_slice(&section);
                let nos = t.nos;
                ack(nos, AfqAction::POS_ACK_SECTION).map(|a| vec![a]).into()
            }
            _ => Reply::nothing(),
        }
    }
}
