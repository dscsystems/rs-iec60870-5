// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The pluggable backing store of the file transfer components, plus an
//! in-memory implementation.

use std::collections::HashMap;
use std::sync::RwLock;

use chrono::{DateTime, Utc};

use crate::asdu::{InfoObjAddr, NameOfFile, TypeId};
use crate::error::{Error, Result};

/// One file held by a [`Store`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Entry {
    /// The information object address the file is associated with.
    pub ioa: InfoObjAddr,
    /// The name of the file. Values below 5 are predefined by the standard —
    /// see [`NameOfFile::DISTURBANCE_DATA`] and its neighbours.
    pub nof: NameOfFile,
    /// The file length in octets.
    pub size: u32,
    /// Creation or acquisition time, reported in a directory.
    pub time: Option<DateTime<Utc>>,
    /// This entry names a subdirectory rather than a file.
    pub is_directory: bool,
}

/// Where the file transfer components keep their files.
///
/// Implementations must be safe for concurrent use. [`MemStore`] is the
/// in-memory one; any other backing — a directory on disk, a database — can
/// implement this trait.
#[async_trait::async_trait]
pub trait Store: Send + Sync + 'static {
    /// The directory of available files.
    async fn list(&self) -> Result<Vec<Entry>>;

    /// The whole content of one file.
    ///
    /// Returns [`Error::FileNotFound`] when the file is unknown.
    async fn read(&self, ioa: InfoObjAddr, nof: NameOfFile) -> Result<Vec<u8>>;

    /// Store a received file, replacing any previous content.
    async fn write(&self, ioa: InfoObjAddr, nof: NameOfFile, data: Vec<u8>) -> Result<()>;

    /// Remove a file.
    ///
    /// Returns [`Error::FileNotFound`] when the file is unknown.
    async fn delete(&self, ioa: InfoObjAddr, nof: NameOfFile) -> Result<()>;
}

#[derive(PartialEq, Eq, Hash, Clone, Copy)]
struct FileKey {
    ioa: InfoObjAddr,
    nof: NameOfFile,
}

struct MemFile {
    data: Vec<u8>,
    time: DateTime<Utc>,
}

/// An in-memory [`Store`], safe for concurrent use.
#[derive(Default)]
pub struct MemStore {
    files: RwLock<HashMap<FileKey, MemFile>>,
}

impl MemStore {
    /// An empty in-memory store.
    pub fn new() -> MemStore {
        MemStore::default()
    }

    /// Insert or replace a file, stamped with the current time.
    ///
    /// The synchronous counterpart of [`Store::write`], for seeding a store
    /// before it is handed to a [`Sender`](crate::filetransfer::Sender).
    pub fn insert(&self, ioa: InfoObjAddr, nof: NameOfFile, data: Vec<u8>) {
        self.files.write().unwrap().insert(
            FileKey { ioa, nof },
            MemFile {
                data,
                time: Utc::now(),
            },
        );
    }

    /// How many files the store holds.
    pub fn len(&self) -> usize {
        self.files.read().unwrap().len()
    }

    /// Whether the store holds no files.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait::async_trait]
impl Store for MemStore {
    /// Ordered by information object address, then by name of file, so a
    /// directory listing is stable across calls.
    async fn list(&self) -> Result<Vec<Entry>> {
        let files = self.files.read().unwrap();
        let mut out: Vec<Entry> = files
            .iter()
            .map(|(k, v)| Entry {
                ioa: k.ioa,
                nof: k.nof,
                size: v.data.len() as u32,
                time: Some(v.time),
                is_directory: false,
            })
            .collect();
        out.sort_by_key(|e| (e.ioa, e.nof.0));
        Ok(out)
    }

    async fn read(&self, ioa: InfoObjAddr, nof: NameOfFile) -> Result<Vec<u8>> {
        self.files
            .read()
            .unwrap()
            .get(&FileKey { ioa, nof })
            .map(|f| f.data.clone())
            .ok_or(Error::FileNotFound)
    }

    async fn write(&self, ioa: InfoObjAddr, nof: NameOfFile, data: Vec<u8>) -> Result<()> {
        self.insert(ioa, nof, data);
        Ok(())
    }

    async fn delete(&self, ioa: InfoObjAddr, nof: NameOfFile) -> Result<()> {
        self.files
            .write()
            .unwrap()
            .remove(&FileKey { ioa, nof })
            .map(|_| ())
            .ok_or(Error::FileNotFound)
    }
}

/// Whether the type identification belongs to the file transfer set these
/// components handle.
pub(crate) fn is_file_asdu(t: TypeId) -> bool {
    matches!(
        t,
        TypeId::F_FR_NA_1
            | TypeId::F_SR_NA_1
            | TypeId::F_SC_NA_1
            | TypeId::F_LS_NA_1
            | TypeId::F_AF_NA_1
            | TypeId::F_SG_NA_1
            | TypeId::F_DR_TA_1
    )
}

/// The most sections one file can have.
///
/// The name of section (NOS) is a single octet and section numbers start at 1,
/// so 255 is the last one a peer can name. A file cut into more than that
/// cannot be addressed: the numbering wraps, and the receiver ends up asking
/// for a section it has already had — a transfer that never finishes.
pub(crate) const MAX_SECTIONS: usize = 255;

/// Cut `data` into sections of at most `section_size` octets, in at most
/// [`MAX_SECTIONS`] sections.
///
/// The configured size is a *preference*: when the file needs more sections
/// than NOS can address, the sections are grown just enough to fit. That
/// trades the earlier corruption detection of small sections for a transfer
/// that can complete at all, which is the better failure mode — and a section
/// may be up to 16 MB, so the growth is always available.
///
/// An empty file yields a single empty section, so that it still goes through
/// the section ready / segment / last section exchange the standard defines
/// rather than being silently skipped.
pub(crate) fn split_sections(data: &[u8], section_size: usize) -> Vec<Vec<u8>> {
    if section_size == 0 || data.is_empty() {
        return vec![data.to_vec()];
    }
    // Round up, so the last section is the short one rather than a 256th.
    let needed = data.len().div_ceil(MAX_SECTIONS);
    let size = section_size.max(needed);
    data.chunks(size).map(|c| c.to_vec()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_are_cut_to_the_configured_size() {
        assert_eq!(split_sections(&[1, 2, 3, 4, 5], 2), vec![
            vec![1, 2],
            vec![3, 4],
            vec![5]
        ]);
        assert_eq!(split_sections(&[1, 2, 3, 4], 2), vec![vec![1, 2], vec![3, 4]]);
        assert_eq!(split_sections(&[1, 2, 3], 10), vec![vec![1, 2, 3]]);
    }

    #[test]
    fn a_file_is_never_cut_into_more_sections_than_nos_can_name() {
        // NOS is one octet, so section 256 would be numbered 0 and the
        // receiver would ask for a section it has already had — a transfer
        // that never finishes.
        for (len, requested) in [
            (20_000usize, 64usize),
            (100_000, 1),
            (1_000_000, 4096),
            (256, 1),
            (255, 1),
        ] {
            let data = vec![7u8; len];
            let sections = split_sections(&data, requested);
            assert!(
                sections.len() <= MAX_SECTIONS,
                "{len} octets in {requested} octet sections gave {} sections",
                sections.len()
            );
            // And no octet is lost or duplicated in the process.
            assert_eq!(sections.concat(), data, "{len}/{requested}");
        }
    }

    #[test]
    fn the_configured_section_size_is_honoured_when_it_fits() {
        // Growing the sections is a last resort, not the normal path.
        let data = vec![0u8; 1000];
        assert_eq!(split_sections(&data, 100).len(), 10);
        assert!(split_sections(&data, 100).iter().all(|s| s.len() == 100));
    }

    #[test]
    fn an_empty_file_still_yields_one_section() {
        // Otherwise it would never be offered, and the peer would wait for a
        // transfer that never starts.
        assert_eq!(split_sections(&[], 512), vec![Vec::<u8>::new()]);
        assert_eq!(split_sections(&[1, 2], 0), vec![vec![1, 2]]);
    }

    #[test]
    fn the_file_type_set_is_exactly_the_seven_implemented() {
        for t in [
            TypeId::F_FR_NA_1,
            TypeId::F_SR_NA_1,
            TypeId::F_SC_NA_1,
            TypeId::F_LS_NA_1,
            TypeId::F_AF_NA_1,
            TypeId::F_SG_NA_1,
            TypeId::F_DR_TA_1,
        ] {
            assert!(is_file_asdu(t), "{t:?}");
        }
        // F_SC_NB_1 <127> (query log) is not implemented, and process data
        // must fall through to the application's own dispatch.
        for t in [TypeId::F_SC_NB_1, TypeId::M_SP_NA_1, TypeId::C_IC_NA_1] {
            assert!(!is_file_asdu(t), "{t:?}");
        }
    }

    #[tokio::test]
    async fn the_memory_store_round_trips_a_file() {
        let s = MemStore::new();
        assert!(s.is_empty());
        s.write(100, NameOfFile::DISTURBANCE_DATA, vec![1, 2, 3])
            .await
            .unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(
            s.read(100, NameOfFile::DISTURBANCE_DATA).await.unwrap(),
            vec![1, 2, 3]
        );

        assert_eq!(
            s.read(101, NameOfFile::DISTURBANCE_DATA).await,
            Err(Error::FileNotFound)
        );
        assert_eq!(
            s.delete(101, NameOfFile::DISTURBANCE_DATA).await,
            Err(Error::FileNotFound)
        );

        s.delete(100, NameOfFile::DISTURBANCE_DATA).await.unwrap();
        assert!(s.is_empty());
    }

    #[tokio::test]
    async fn the_directory_listing_is_ordered_and_sized() {
        let s = MemStore::new();
        s.insert(200, NameOfFile::TRANSPARENT, vec![0; 10]);
        s.insert(100, NameOfFile::SEQUENCES_OF_EVENTS, vec![0; 5]);
        s.insert(100, NameOfFile::DISTURBANCE_DATA, vec![0; 7]);

        let list = s.list().await.unwrap();
        assert_eq!(
            list.iter().map(|e| (e.ioa, e.nof.0, e.size)).collect::<Vec<_>>(),
            vec![(100, 2, 7), (100, 3, 5), (200, 1, 10)]
        );
        assert!(list.iter().all(|e| e.time.is_some()));
    }
}
