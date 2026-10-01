// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Monitor-direction file transfer for IEC 101/104 (§7.4.11).
//!
//! Delegate file ASDUs from an ASDU handler to [`Sender::handle`] or
//! [`Receiver::handle`]. One transfer runs per service instance; use a separate
//! instance per connection. Call `abort` when the connection is lost.
use crate::{Error, Result, asdu::*};
use chrono::{DateTime, Utc};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Default checksum section size in octets.
pub const DEFAULT_SECTION_SIZE: usize = 4096;
/// Metadata for a stored or received file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Information object address.
    pub ioa: InfoObjAddr,
    /// Name of file.
    pub nof: NameOfFile,
    /// File size in octets.
    pub size: u32,
    /// Acquisition time.
    pub time: Option<DateTime<Utc>>,
    /// Whether this entry names a directory.
    pub is_directory: bool,
}
/// Concurrent backing store for file transfers.
pub trait Store: Send + Sync {
    /// List available entries.
    fn list(&self) -> Result<Vec<Entry>>;
    /// Read owned file bytes, or return a file-not-found error.
    fn read(&self, ioa: InfoObjAddr, nof: NameOfFile) -> Result<Vec<u8>>;
    /// Store received file bytes.
    fn write(&self, ioa: InfoObjAddr, nof: NameOfFile, data: &[u8]) -> Result<()>;
    /// Delete an entry.
    fn delete(&self, ioa: InfoObjAddr, nof: NameOfFile) -> Result<()>;
}
type Files = BTreeMap<(InfoObjAddr, NameOfFile), (Vec<u8>, DateTime<Utc>)>;
/// In-memory store; reads and writes copy the data.
#[derive(Default)]
pub struct MemStore {
    files: Mutex<Files>,
}
impl MemStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self::default()
    }
}
impl Store for MemStore {
    fn list(&self) -> Result<Vec<Entry>> {
        Ok(self
            .files
            .lock()
            .unwrap()
            .iter()
            .map(|(&(ioa, nof), (data, time))| Entry {
                ioa,
                nof,
                size: data.len() as u32,
                time: Some(*time),
                is_directory: false,
            })
            .collect())
    }
    fn read(&self, ioa: InfoObjAddr, nof: NameOfFile) -> Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(&(ioa, nof))
            .map(|f| f.0.clone())
            .ok_or(Error::FileTransfer("file not found"))
    }
    fn write(&self, ioa: InfoObjAddr, nof: NameOfFile, data: &[u8]) -> Result<()> {
        if data.len() > LENGTH_OF_FILE_MAX as usize {
            return Err(Error::LengthOutOfRange);
        }
        self.files
            .lock()
            .unwrap()
            .insert((ioa, nof), (data.to_vec(), Utc::now()));
        Ok(())
    }
    fn delete(&self, ioa: InfoObjAddr, nof: NameOfFile) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            .remove(&(ioa, nof))
            .map(|_| ())
            .ok_or(Error::FileTransfer("file not found"))
    }
}
fn file_type(t: TypeId) -> bool {
    (120..=126).contains(&t.0)
}
fn cause() -> CauseOfTransmission {
    CauseOfTransmission::new(Cause::FILE_TRANSFER)
}
fn fault(msg: &'static str) -> Error {
    Error::FileTransfer(msg)
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    ca: CommonAddr,
    ioa: InfoObjAddr,
    nof: NameOfFile,
}
impl Key {
    fn matches(self, a: &Asdu, ioa: InfoObjAddr, nof: NameOfFile) -> bool {
        self == Self {
            ca: a.common_addr(),
            ioa,
            nof,
        }
    }
}
struct Sending {
    key: Key,
    sections: Vec<Vec<u8>>,
    current: usize,
}
/// Controlled-station file server. Sends wait up to 30 seconds for queue room.
pub struct Sender {
    store: Arc<dyn Store>,
    section_size: usize,
    active: Option<Sending>,
}
impl Sender {
    /// Serve the supplied store.
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self {
            store,
            section_size: DEFAULT_SECTION_SIZE,
            active: None,
        }
    }
    /// Choose a section size (1..=0xffffff). At most 255 sections fit a file.
    pub fn set_section_size(&mut self, n: usize) -> Result<()> {
        if n == 0 || n > LENGTH_OF_FILE_MAX as usize {
            return Err(Error::Param);
        }
        self.section_size = n;
        Ok(())
    }
    /// Whether a file is selected.
    pub fn in_progress(&self) -> bool {
        self.active.is_some()
    }
    /// Clear state without notifying the peer.
    pub fn abort(&mut self) {
        self.active = None;
    }
    /// Announce a file available for automatic selection.
    pub async fn offer(
        &self,
        c: &dyn Connect,
        ca: CommonAddr,
        ioa: InfoObjAddr,
        nof: NameOfFile,
    ) -> Result<()> {
        let data = self.store.read(ioa, nof)?;
        if data.len() > LENGTH_OF_FILE_MAX as usize {
            return Err(Error::LengthOutOfRange);
        }
        c.send_file_ready(
            cause(),
            ca,
            FileReadyInfo {
                ioa,
                nof,
                length_of_file: data.len() as u32,
                frq: 0,
            },
        )
        .await
    }
    fn ready(&self, p: Params) -> Result<Asdu> {
        let s = self
            .active
            .as_ref()
            .ok_or(fault("no transfer in progress"))?;
        Asdu::section_ready(
            p,
            cause(),
            s.key.ca,
            SectionReadyInfo {
                ioa: s.key.ioa,
                nof: s.key.nof,
                nos: (s.current + 1) as u8,
                length_of_section: s.sections[s.current].len() as u32,
                srq: 0,
            },
        )
    }
    fn data(&self, p: Params) -> Result<Vec<Asdu>> {
        let s = self
            .active
            .as_ref()
            .ok_or(fault("no transfer in progress"))?;
        let section = s
            .sections
            .get(s.current)
            .ok_or(fault("unexpected section"))?;
        let mut out = Vec::new();
        for chunk in section.chunks(p.max_segment_size()) {
            out.push(Asdu::file_segment(
                p,
                cause(),
                s.key.ca,
                SegmentInfo {
                    ioa: s.key.ioa,
                    nof: s.key.nof,
                    nos: (s.current + 1) as u8,
                    segment: chunk.to_vec(),
                },
            )?);
        }
        out.push(Asdu::last_section_or_segment(
            p,
            cause(),
            s.key.ca,
            LastSectionOrSegmentInfo {
                ioa: s.key.ioa,
                nof: s.key.nof,
                nos: (s.current + 1) as u8,
                lsq: 3,
                chs: file_checksum(section),
            },
        )?);
        Ok(out)
    }
    /// Process an ASDU; `false` means it is outside the file-transfer service.
    pub async fn handle(&mut self, c: &dyn Connect, a: &Asdu) -> Result<bool> {
        if !file_type(a.type_id()) {
            return Ok(false);
        }
        let actions = self.step(c.params(), a)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        for out in actions {
            if let Err(e) = c.send_wait(out, deadline).await {
                self.abort();
                return Err(e);
            }
        }
        Ok(true)
    }
    fn step(&mut self, p: Params, a: &Asdu) -> Result<Vec<Asdu>> {
        p.valid()?;
        match a.type_id() {
            TypeId::F_SC_NA_1 => {
                let r = a.get_call_or_select_file()?;
                match r.scq & 15 {
                    0 => {
                        let entries = self.store.list()?;
                        let count = entries.len();
                        let infos: Vec<_> = entries
                            .into_iter()
                            .enumerate()
                            .map(|(i, e)| DirectoryInfo {
                                ioa: e.ioa,
                                nof: e.nof,
                                length_of_file: e.size,
                                sof: if i + 1 == count { 32 } else { 0 }
                                    | if e.is_directory { 64 } else { 0 },
                                time: e.time,
                            })
                            .collect();
                        let max = (ASDU_SIZE_MAX - p.identifier_size())
                            / (p.info_obj_addr_size as usize + 13);
                        infos
                            .chunks(max)
                            .map(|chunk| {
                                Asdu::file_directory(
                                    p,
                                    CauseOfTransmission::new(Cause::REQUEST),
                                    a.common_addr(),
                                    chunk,
                                )
                            })
                            .collect()
                    }
                    1 | 2 => {
                        if self.active.is_some() {
                            return Err(fault("transfer busy"));
                        }
                        let data = match self.store.read(r.ioa, r.nof) {
                            Ok(data) => data,
                            Err(_) => {
                                return Ok(vec![Asdu::ack_file_or_section(
                                    p,
                                    cause(),
                                    a.common_addr(),
                                    AckFileOrSectionInfo {
                                        ioa: r.ioa,
                                        nof: r.nof,
                                        nos: r.nos,
                                        afq: 0x42,
                                    },
                                )?]);
                            }
                        };
                        if data.len() > LENGTH_OF_FILE_MAX as usize {
                            return Err(Error::LengthOutOfRange);
                        }
                        let sections = if data.is_empty() {
                            vec![Vec::new()]
                        } else {
                            data.chunks(self.section_size)
                                .map(|c| c.to_vec())
                                .collect::<Vec<_>>()
                        };
                        if sections.len() > 255 {
                            return Err(Error::LengthOutOfRange);
                        }
                        self.active = Some(Sending {
                            key: Key {
                                ca: a.common_addr(),
                                ioa: r.ioa,
                                nof: r.nof,
                            },
                            sections,
                            current: 0,
                        });
                        Ok(vec![self.ready(p)?])
                    }
                    3 | 7 => {
                        self.abort();
                        Ok(vec![])
                    }
                    4 => {
                        self.store.delete(r.ioa, r.nof)?;
                        if self
                            .active
                            .as_ref()
                            .is_some_and(|s| s.key.matches(a, r.ioa, r.nof))
                        {
                            self.abort();
                        }
                        Ok(vec![])
                    }
                    5 | 6 => {
                        let s = self
                            .active
                            .as_mut()
                            .ok_or(fault("no transfer in progress"))?;
                        if !s.key.matches(a, r.ioa, r.nof) {
                            return Err(fault("unexpected file"));
                        }
                        if r.nos != 0 {
                            if r.nos as usize > s.sections.len() {
                                return Err(fault("unexpected section"));
                            }
                            s.current = r.nos as usize - 1;
                        }
                        if r.scq & 15 == 5 {
                            Ok(vec![self.ready(p)?])
                        } else {
                            self.data(p)
                        }
                    }
                    _ => Err(fault("unsupported service")),
                }
            }
            TypeId::F_AF_NA_1 => {
                let r = a.get_ack_file_or_section()?;
                let s = self
                    .active
                    .as_mut()
                    .ok_or(fault("no transfer in progress"))?;
                if !s.key.matches(a, r.ioa, r.nof) {
                    return Err(fault("unexpected file"));
                }
                match r.afq & 15 {
                    1 | 2 => {
                        self.abort();
                        Ok(vec![])
                    }
                    3 => {
                        if r.nos as usize != s.current + 1 {
                            return Err(fault("unexpected section"));
                        }
                        s.current += 1;
                        if s.current < s.sections.len() {
                            Ok(vec![self.ready(p)?])
                        } else {
                            Ok(vec![Asdu::last_section_or_segment(
                                p,
                                cause(),
                                s.key.ca,
                                LastSectionOrSegmentInfo {
                                    ioa: s.key.ioa,
                                    nof: s.key.nof,
                                    nos: s.sections.len() as u8,
                                    lsq: 1,
                                    chs: 0,
                                },
                            )?])
                        }
                    }
                    4 => {
                        if r.nos as usize != s.current + 1 {
                            return Err(fault("unexpected section"));
                        }
                        self.data(p)
                    }
                    _ => Err(fault("unsupported service")),
                }
            }
            _ => Err(fault("unsupported service")),
        }
    }
}
struct Receiving {
    key: Key,
    nos: u8,
    section: Vec<u8>,
    file: Vec<u8>,
    section_length: usize,
    awaiting_section: bool,
}
/// Completed transfer, returned by [`Receiver::take_completed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completed {
    /// File metadata.
    pub entry: Entry,
    /// Verified file bytes.
    pub data: Vec<u8>,
}
/// Controlling-station receiver with checksum verification and section retries.
pub struct Receiver {
    store: Option<Arc<dyn Store>>,
    auto_accept: bool,
    active: Option<Receiving>,
    completed: Vec<Completed>,
    directory: Vec<DirectoryInfo>,
}
impl Receiver {
    /// Construct a receiver; a store is optional.
    pub fn new(store: Option<Arc<dyn Store>>) -> Self {
        Self {
            store,
            auto_accept: true,
            active: None,
            completed: Vec::new(),
            directory: Vec::new(),
        }
    }
    /// Enable automatic acceptance of file-ready announcements (default true).
    pub fn set_auto_accept(&mut self, enabled: bool) {
        self.auto_accept = enabled;
    }
    /// Whether a transfer is selected.
    pub fn in_progress(&self) -> bool {
        self.active.is_some()
    }
    /// Clear an incomplete transfer.
    pub fn abort(&mut self) {
        self.active = None;
    }
    /// Drain completed files.
    pub fn take_completed(&mut self) -> Vec<Completed> {
        std::mem::take(&mut self.completed)
    }
    /// Drain received directory entries.
    pub fn take_directory(&mut self) -> Vec<DirectoryInfo> {
        std::mem::take(&mut self.directory)
    }
    /// Call a station's directory.
    pub async fn request_directory(&mut self, c: &dyn Connect, ca: CommonAddr) -> Result<()> {
        self.directory.clear();
        c.send_call_or_select_file(
            CauseOfTransmission::new(Cause::REQUEST),
            ca,
            CallOrSelectFileInfo::default(),
        )
        .await
    }
    /// Select a file; subsequent ASDUs drive the remaining procedure.
    pub async fn request_file(
        &mut self,
        c: &dyn Connect,
        ca: CommonAddr,
        ioa: InfoObjAddr,
        nof: NameOfFile,
    ) -> Result<()> {
        if self.active.is_some() {
            return Err(fault("transfer busy"));
        }
        self.select(Key { ca, ioa, nof });
        if let Err(e) = c
            .send_call_or_select_file(
                cause(),
                ca,
                CallOrSelectFileInfo {
                    ioa,
                    nof,
                    nos: 0,
                    scq: 1,
                },
            )
            .await
        {
            self.abort();
            return Err(e);
        }
        Ok(())
    }
    fn select(&mut self, key: Key) {
        self.active = Some(Receiving {
            key,
            nos: 0,
            section: Vec::new(),
            file: Vec::new(),
            section_length: 0,
            awaiting_section: false,
        });
    }
    /// Consume a file ASDU and queue the next request/acknowledgement.
    pub async fn handle(&mut self, c: &dyn Connect, a: &Asdu) -> Result<bool> {
        if !file_type(a.type_id()) {
            return Ok(false);
        }
        let actions = self.step(c.params(), a)?;
        for out in actions {
            if let Err(e) = c
                .send_wait(out, tokio::time::Instant::now() + Duration::from_secs(30))
                .await
            {
                self.abort();
                return Err(e);
            }
        }
        Ok(true)
    }
    fn step(&mut self, p: Params, a: &Asdu) -> Result<Vec<Asdu>> {
        if a.type_id() == TypeId::F_DR_TA_1 {
            self.directory.extend(a.get_file_directory()?);
            return Ok(vec![]);
        }
        if a.type_id() == TypeId::F_FR_NA_1 {
            let r = a.get_file_ready()?;
            if r.frq & 128 != 0 {
                self.abort();
                return Err(fault("file not ready"));
            }
            if self.auto_accept && self.active.is_none() {
                self.select(Key {
                    ca: a.common_addr(),
                    ioa: r.ioa,
                    nof: r.nof,
                });
                return Ok(vec![Asdu::call_or_select_file(
                    p,
                    cause(),
                    a.common_addr(),
                    CallOrSelectFileInfo {
                        ioa: r.ioa,
                        nof: r.nof,
                        nos: 0,
                        scq: 1,
                    },
                )?]);
            }
            return Ok(vec![]);
        }
        let s = self
            .active
            .as_mut()
            .ok_or(fault("no transfer in progress"))?;
        let ack = |nos, afq| {
            Asdu::ack_file_or_section(
                p,
                cause(),
                s.key.ca,
                AckFileOrSectionInfo {
                    ioa: s.key.ioa,
                    nof: s.key.nof,
                    nos,
                    afq,
                },
            )
        };
        match a.type_id() {
            TypeId::F_SR_NA_1 => {
                let r = a.get_section_ready()?;
                if !s.key.matches(a, r.ioa, r.nof) || r.nos == 0 {
                    return Err(fault("unexpected file or section"));
                }
                if r.srq & 128 != 0 {
                    self.abort();
                    return Err(fault("section not ready"));
                }
                s.nos = r.nos;
                s.section.clear();
                s.section_length = r.length_of_section as usize;
                s.awaiting_section = true;
                Ok(vec![Asdu::call_or_select_file(
                    p,
                    cause(),
                    s.key.ca,
                    CallOrSelectFileInfo {
                        ioa: s.key.ioa,
                        nof: s.key.nof,
                        nos: s.nos,
                        scq: 6,
                    },
                )?])
            }
            TypeId::F_SG_NA_1 => {
                let r = a.get_file_segment()?;
                if !s.key.matches(a, r.ioa, r.nof) || r.nos != s.nos || !s.awaiting_section {
                    return Err(fault("unexpected file or section"));
                }
                if s.section.len() + r.segment.len() > s.section_length {
                    return Err(fault("section length exceeded"));
                }
                s.section.extend(r.segment);
                Ok(vec![])
            }
            TypeId::F_LS_NA_1 => {
                let r = a.get_last_section_or_segment()?;
                if !s.key.matches(a, r.ioa, r.nof) || r.nos != s.nos {
                    return Err(fault("unexpected file or section"));
                }
                match r.lsq {
                    3 | 4 => {
                        if !s.awaiting_section {
                            return Err(fault("unexpected section end"));
                        }
                        if file_checksum(&s.section) != r.chs || s.section.len() != s.section_length
                        {
                            s.section.clear();
                            return Ok(vec![ack(s.nos, 0x24)?]);
                        }
                        if s.file.len() + s.section.len() > LENGTH_OF_FILE_MAX as usize {
                            return Err(Error::LengthOutOfRange);
                        }
                        s.file.append(&mut s.section);
                        s.awaiting_section = false;
                        Ok(vec![ack(s.nos, 3)?])
                    }
                    1 | 2 => {
                        if s.awaiting_section {
                            return Err(fault("file ended before section acknowledgement"));
                        }
                        let reply = ack(r.nos, 1)?;
                        let s = self.active.take().unwrap();
                        if let Some(store) = &self.store {
                            store.write(s.key.ioa, s.key.nof, &s.file)?;
                        }
                        self.completed.push(Completed {
                            entry: Entry {
                                ioa: s.key.ioa,
                                nof: s.key.nof,
                                size: s.file.len() as u32,
                                time: None,
                                is_directory: false,
                            },
                            data: s.file,
                        });
                        Ok(vec![reply])
                    }
                    _ => Err(fault("unsupported service")),
                }
            }
            TypeId::F_AF_NA_1 => {
                let r = a.get_ack_file_or_section()?;
                if !s.key.matches(a, r.ioa, r.nof) {
                    return Err(fault("unexpected file"));
                }
                self.abort();
                Err(fault("negative acknowledgement"))
            }
            _ => Err(fault("unsupported service")),
        }
    }
}
