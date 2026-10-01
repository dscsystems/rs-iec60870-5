// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Collect callback results for assertions without holding application locks.
#![allow(dead_code)]
use rs_iec60870_5::{
    Result,
    asdu::*,
    filetransfer::{self, Store},
};
use std::sync::{Arc, Mutex};

pub struct Completed {
    pub data: Vec<u8>,
}
pub struct Receiver {
    inner: filetransfer::Receiver,
    completed: Arc<Mutex<Vec<Completed>>>,
    directory: Arc<Mutex<Vec<DirectoryInfo>>>,
}
impl Receiver {
    pub fn new(store: Option<Arc<dyn Store>>) -> Self {
        let inner = match store {
            Some(store) => filetransfer::Receiver::new(store),
            None => filetransfer::Receiver::without_store(),
        };
        let completed = Arc::new(Mutex::new(Vec::new()));
        let collected = completed.clone();
        inner.set_file_handler(Box::new(move |_, data| {
            collected.lock().unwrap().push(Completed { data });
        }));
        let directory = Arc::new(Mutex::new(Vec::new()));
        let collected = directory.clone();
        inner.set_directory_handler(Box::new(move |_, entries| {
            collected.lock().unwrap().extend(entries);
        }));
        Self {
            inner,
            completed,
            directory,
        }
    }
    pub fn take_completed(&self) -> Vec<Completed> {
        std::mem::take(&mut *self.completed.lock().unwrap())
    }
    pub fn take_directory(&self) -> Vec<DirectoryInfo> {
        std::mem::take(&mut *self.directory.lock().unwrap())
    }
    pub async fn request_file(
        &self,
        conn: &dyn Connect,
        ca: CommonAddr,
        ioa: InfoObjAddr,
        nof: u16,
    ) -> Result<()> {
        self.inner
            .request_file(conn, ca, ioa, NameOfFile(nof))
            .await
    }
    pub async fn request_directory(&self, conn: &dyn Connect, ca: CommonAddr) -> Result<()> {
        self.inner.request_directory(conn, ca).await
    }
    pub async fn handle(&self, conn: &dyn Connect, a: &Asdu) -> Result<bool> {
        self.inner.handle(conn, a).await
    }
    pub fn abort(&self) {
        self.inner.abort();
    }
    #[allow(dead_code)]
    pub fn in_progress(&self) -> bool {
        self.inner.in_progress()
    }
}
