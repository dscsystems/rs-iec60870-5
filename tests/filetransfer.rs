// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Complete procedure including checksum rejection, retry and directory batching.
use rs_iec60870_5::{Error, Result, asdu::*, filetransfer::*};
use std::sync::{Arc, Mutex};
struct Wire {
    params: Params,
    frames: Mutex<Vec<Asdu>>,
}
#[async_trait::async_trait]
impl Connect for Wire {
    fn params(&self) -> Params {
        self.params
    }
    async fn send(&self, a: Asdu) -> Result<()> {
        let raw = a.marshal_binary()?;
        self.frames
            .lock()
            .unwrap()
            .push(Asdu::unmarshal_binary(self.params, &raw)?);
        Ok(())
    }
}
impl Wire {
    fn new(params: Params) -> Self {
        Self {
            params,
            frames: Mutex::new(Vec::new()),
        }
    }
    fn take(&self) -> Vec<Asdu> {
        std::mem::take(&mut *self.frames.lock().unwrap())
    }
}
async fn transfer(params: Params, size: usize, corrupt: bool, offer: bool) {
    let source = Arc::new(MemStore::new());
    let target = Arc::new(MemStore::new());
    let data: Vec<u8> = (0..size).map(|i| (i * 17 + 3) as u8).collect();
    source.write(100, 2, &data).unwrap();
    let mut sender = Sender::new(source);
    sender.set_section_size(256).unwrap();
    let mut receiver = Receiver::new(Some(target.clone()));
    let sw = Wire::new(params);
    let rw = Wire::new(params);
    if offer {
        sender.offer(&sw, 1, 100, 2).await.unwrap();
    } else {
        receiver.request_file(&rw, 1, 100, 2).await.unwrap();
    }
    let mut corrupted = false;
    let mut retry = false;
    for _ in 0..1000 {
        for mut a in sw.take() {
            if corrupt && !corrupted && a.type_id() == TypeId::F_SG_NA_1 {
                *a.info_obj.last_mut().unwrap() ^= 1;
                corrupted = true;
            }
            receiver.handle(&rw, &a).await.unwrap();
        }
        for a in rw.take() {
            if a.type_id() == TypeId::F_AF_NA_1 && a.get_ack_file_or_section().unwrap().afq == 0x24
            {
                retry = true;
            }
            sender.handle(&sw, &a).await.unwrap();
        }
        if !sender.in_progress() && !receiver.in_progress() {
            let files = receiver.take_completed();
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].data, data);
            assert_eq!(target.read(100, 2).unwrap(), data);
            assert_eq!(retry, corrupt);
            return;
        }
    }
    panic!("file transfer did not finish");
}
#[tokio::test]
async fn empty_multi_section_offered_and_corrupted_files() {
    for params in [PARAMS_WIDE, PARAMS_STANDARD_101, PARAMS_NARROW] {
        transfer(params, 0, false, false).await;
        transfer(params, 5000, false, false).await;
        transfer(params, 600, true, false).await;
        transfer(params, 600, false, true).await;
    }
}
#[tokio::test]
async fn directory_batches_preserve_the_final_entry_flag() {
    let store = Arc::new(MemStore::new());
    for ioa in 1..=40 {
        store.write(ioa, 2, &[1, 2, 3]).unwrap();
    }
    let mut sender = Sender::new(store);
    let mut receiver = Receiver::new(None);
    let sw = Wire::new(PARAMS_WIDE);
    let rw = Wire::new(PARAMS_WIDE);
    receiver.request_directory(&rw, 1).await.unwrap();
    sender.handle(&sw, &rw.take()[0]).await.unwrap();
    let frames = sw.take();
    assert_eq!(frames.len(), 3);
    for a in frames {
        receiver.handle(&rw, &a).await.unwrap();
    }
    let entries = receiver.take_directory();
    assert_eq!(entries.len(), 40);
    assert_eq!(entries[39].sof & 32, 32);
    assert!(entries[..39].iter().all(|e| e.sof & 32 == 0));
}
#[tokio::test]
async fn busy_missing_file_and_foreign_file_are_detected() {
    let sw = Wire::new(PARAMS_WIDE);
    let rw = Wire::new(PARAMS_WIDE);
    let mut receiver = Receiver::new(None);
    let mut sender = Sender::new(Arc::new(MemStore::new()));
    receiver.request_file(&rw, 1, 100, 2).await.unwrap();
    assert!(matches!(
        receiver.request_file(&rw, 1, 101, 2).await,
        Err(Error::FileTransfer(_))
    ));
    sender.handle(&sw, &rw.take()[0]).await.unwrap();
    assert!(receiver.handle(&rw, &sw.take()[0]).await.is_err());
    assert!(!receiver.in_progress());
    receiver.request_file(&rw, 1, 100, 2).await.unwrap();
    let foreign = Asdu::section_ready(
        PARAMS_WIDE,
        CauseOfTransmission::new(Cause::FILE_TRANSFER),
        2,
        SectionReadyInfo {
            ioa: 100,
            nof: 2,
            nos: 1,
            length_of_section: 10,
            srq: 0,
        },
    )
    .unwrap();
    assert!(receiver.handle(&rw, &foreign).await.is_err());
    assert!(rw.take().len() == 1);
}
#[test]
fn segment_length_count_and_trailing_data_are_checked() {
    let a = Asdu::file_segment(
        PARAMS_WIDE,
        CauseOfTransmission::new(Cause::FILE_TRANSFER),
        1,
        SegmentInfo {
            ioa: 100,
            nof: 2,
            nos: 1,
            segment: vec![7; 236],
        },
    )
    .unwrap();
    let wire = a.marshal_binary().unwrap();
    assert_eq!(wire.len(), 249);
    assert_eq!(
        Asdu::unmarshal_binary(PARAMS_WIDE, &wire)
            .unwrap()
            .get_file_segment()
            .unwrap()
            .segment,
        vec![7; 236]
    );
    let mut malformed = wire.clone();
    malformed[12] = 235;
    assert_eq!(
        Asdu::unmarshal_binary(PARAMS_WIDE, &malformed),
        Err(Error::InfoObjSizeMismatch)
    );
    let mut malformed = wire.clone();
    malformed[1] = 2;
    assert_eq!(
        Asdu::unmarshal_binary(PARAMS_WIDE, &malformed),
        Err(Error::InfoObjIndexFit)
    );
    let mut malformed = wire;
    malformed.pop();
    assert_eq!(
        Asdu::unmarshal_binary(PARAMS_WIDE, &malformed),
        Err(Error::UnexpectedEof)
    );
}
#[test]
fn malformed_outbound_asdus_are_refused_and_legacy_trailing_is_opt_in() {
    let a = Asdu::single(
        PARAMS_WIDE,
        false,
        CauseOfTransmission::new(Cause::SPONTANEOUS),
        1,
        &[SinglePointInfo::new(100, true)],
    )
    .unwrap();
    let mut trailing = a.marshal_binary().unwrap();
    trailing.push(99);
    assert_eq!(
        Asdu::unmarshal_binary(PARAMS_WIDE, &trailing),
        Err(Error::InfoObjSizeMismatch)
    );
    let legacy = Params {
        allow_trailing_octets: true,
        ..PARAMS_WIDE
    };
    assert_eq!(
        Asdu::unmarshal_binary(legacy, &trailing).unwrap().info_obj,
        a.info_obj
    );
    for n in [0, 2, 128] {
        let mut bad = a.clone();
        bad.identifier.variable.number = n;
        assert!(bad.marshal_binary().is_err());
    }
    let mut bad = a.clone();
    bad.info_obj.pop();
    assert_eq!(bad.marshal_binary(), Err(Error::UnexpectedEof));
    let mut bad = a.clone();
    bad.info_obj.push(99);
    assert_eq!(bad.marshal_binary(), Err(Error::InfoObjSizeMismatch));
    let mut private = a;
    private.identifier.type_id = TypeId(200);
    assert!(private.marshal_binary().is_ok());
}
