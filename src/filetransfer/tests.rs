// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! The sender and the receiver driven against each other, so the procedures
//! are tested as a pair rather than each against its own idea of the other.

use std::sync::{Arc, Mutex};

use crate::asdu::{
    Asdu, Cause, CauseOfTransmission, CommonAddr, Connect, NameOfFile, PARAMS_WIDE, Params, TypeId,
};
use crate::error::{Error, Result};
use crate::filetransfer::{MemStore, Receiver, Sender, Store};

const CA: CommonAddr = 1;
const IOA: u32 = 100;
const NOF: NameOfFile = NameOfFile::DISTURBANCE_DATA;

/// A [`Connect`] that records what was sent instead of transmitting it.
#[derive(Default)]
struct Recorder {
    sent: Mutex<Vec<Asdu>>,
    /// When set, every send fails with this error.
    fail: Mutex<Option<Error>>,
}

#[async_trait::async_trait]
impl Connect for Recorder {
    fn params(&self) -> Params {
        PARAMS_WIDE
    }
    async fn send(&self, a: Asdu) -> Result<()> {
        if let Some(e) = self.fail.lock().unwrap().clone() {
            return Err(e);
        }
        self.sent.lock().unwrap().push(a);
        Ok(())
    }
}

impl Recorder {
    fn take(&self) -> Vec<Asdu> {
        std::mem::take(&mut self.sent.lock().unwrap())
    }
}

/// Run a whole transfer by shuttling ASDUs between the two components until
/// neither has anything more to say. Returns the number of ASDUs exchanged.
async fn pump(sender: &Sender, s_link: &Recorder, receiver: &Receiver, r_link: &Recorder) -> usize {
    let mut exchanged = 0;
    for _ in 0..1000 {
        let to_receiver = s_link.take();
        let to_sender = r_link.take();
        if to_receiver.is_empty() && to_sender.is_empty() {
            return exchanged;
        }
        exchanged += to_receiver.len() + to_sender.len();
        for a in to_receiver {
            receiver.handle(r_link, &a).await.expect("receiver");
        }
        for a in to_sender {
            sender.handle(s_link, &a).await.expect("sender");
        }
    }
    panic!("the transfer did not settle");
}

fn parts(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8).collect()
}

/// A sender and receiver wired to their own recorders, with `data` in the
/// sender's store.
fn pair(data: Vec<u8>) -> (Sender, Recorder, Receiver, Recorder, Arc<MemStore>) {
    let src = Arc::new(MemStore::new());
    src.insert(IOA, NOF, data);
    let dst = Arc::new(MemStore::new());
    (
        Sender::new(src),
        Recorder::default(),
        Receiver::new(dst.clone()),
        Recorder::default(),
        dst,
    )
}

#[tokio::test]
async fn a_file_survives_a_whole_transfer() {
    // Sizes that straddle the section and segment boundaries: a file smaller
    // than one segment, one that fills several, and one that spans sections.
    for size in [0usize, 1, 100, 232, 233, 500, 4096, 4097, 10_000] {
        let data = parts(size);
        let (sender, s_link, receiver, r_link, dst) = pair(data.clone());
        sender.set_section_size(4096);

        receiver
            .request_file(&r_link, CA, IOA, NOF)
            .await
            .expect("select");
        pump(&sender, &s_link, &receiver, &r_link).await;

        assert_eq!(
            dst.read(IOA, NOF).await.unwrap(),
            data,
            "a {size} octet file did not arrive intact"
        );
        assert!(!sender.in_progress(), "{size}: the sender is still busy");
        assert!(!receiver.in_progress(), "{size}: the receiver is still busy");
    }
}

#[tokio::test]
async fn a_file_spanning_many_sections_and_segments_survives() {
    // Small sections force the section ready / request / acknowledge cycle to
    // run many times, and a small file still has to come back byte for byte.
    let data = parts(5000);
    let (sender, s_link, receiver, r_link, dst) = pair(data.clone());
    sender.set_section_size(64);

    receiver.request_file(&r_link, CA, IOA, NOF).await.unwrap();
    let exchanged = pump(&sender, &s_link, &receiver, &r_link).await;

    assert_eq!(dst.read(IOA, NOF).await.unwrap(), data);
    // 5000 / 64 = 79 sections, each with its own round trip.
    assert!(exchanged > 79 * 3, "only {exchanged} ASDUs exchanged");
}

#[tokio::test]
async fn the_file_handler_receives_the_completed_file() {
    let data = parts(300);
    let (sender, s_link, receiver, r_link, _) = pair(data.clone());

    /// The information object address and content the handler was given.
    type Delivered = Arc<Mutex<Option<(u32, Vec<u8>)>>>;

    let got: Delivered = Arc::new(Mutex::new(None));
    let sink = got.clone();
    receiver.set_file_handler(Box::new(move |entry, d| {
        *sink.lock().unwrap() = Some((entry.ioa, d));
    }));

    receiver.request_file(&r_link, CA, IOA, NOF).await.unwrap();
    pump(&sender, &s_link, &receiver, &r_link).await;

    let (ioa, received) = got.lock().unwrap().clone().expect("the handler fired");
    assert_eq!(ioa, IOA);
    assert_eq!(received, data);
}

#[tokio::test]
async fn an_announced_file_is_selected_automatically() {
    let data = parts(400);
    let (sender, s_link, receiver, r_link, dst) = pair(data.clone());

    // The outstation offers the file rather than the master asking for it.
    sender.offer(&s_link, CA, IOA, NOF).await.unwrap();
    pump(&sender, &s_link, &receiver, &r_link).await;

    assert_eq!(dst.read(IOA, NOF).await.unwrap(), data);
}

#[tokio::test]
async fn auto_accept_can_be_turned_off() {
    let (sender, s_link, receiver, r_link, dst) = pair(parts(400));
    receiver.set_auto_accept(false);

    sender.offer(&s_link, CA, IOA, NOF).await.unwrap();
    pump(&sender, &s_link, &receiver, &r_link).await;

    assert!(!receiver.in_progress(), "the offer was not taken up");
    assert_eq!(dst.read(IOA, NOF).await, Err(Error::FileNotFound));
}

#[tokio::test]
async fn a_directory_call_lists_what_the_store_holds() {
    let src = Arc::new(MemStore::new());
    src.insert(100, NameOfFile::DISTURBANCE_DATA, parts(10));
    src.insert(200, NameOfFile::TRANSPARENT, parts(20));
    let sender = Sender::new(src);
    let s_link = Recorder::default();

    let receiver = Receiver::without_store();
    let r_link = Recorder::default();
    let listing = Arc::new(Mutex::new(Vec::new()));
    let sink = listing.clone();
    receiver.set_directory_handler(Box::new(move |_, dir| {
        *sink.lock().unwrap() = dir;
    }));

    receiver.request_directory(&r_link, CA).await.unwrap();
    pump(&sender, &s_link, &receiver, &r_link).await;

    let dir = listing.lock().unwrap().clone();
    assert_eq!(
        dir.iter().map(|e| (e.ioa, e.length_of_file)).collect::<Vec<_>>(),
        vec![(100, 10), (200, 20)]
    );
    assert!(
        dir.last().unwrap().sof.is_last_file_of_directory,
        "the last entry must carry LFD, or the master cannot tell where the listing ends"
    );
    assert!(!dir[0].sof.is_last_file_of_directory);
}

#[tokio::test]
async fn an_empty_store_answers_a_directory_call_with_silence() {
    // The standard has no empty-directory ASDU, and one with zero objects
    // would be malformed.
    let sender = Sender::new(Arc::new(MemStore::new()));
    let s_link = Recorder::default();
    let call = Asdu::call_or_select_file(
        PARAMS_WIDE,
        CauseOfTransmission::new(Cause::REQUEST),
        CA,
        Default::default(),
    )
    .unwrap();

    assert!(sender.handle(&s_link, &call).await.unwrap());
    assert!(s_link.take().is_empty());
}

#[tokio::test]
async fn a_request_for_a_file_that_is_not_there_is_refused_on_the_wire() {
    let sender = Sender::new(Arc::new(MemStore::new()));
    let s_link = Recorder::default();

    let select = Asdu::call_or_select_file(
        PARAMS_WIDE,
        CauseOfTransmission::new(Cause::FILE_TRANSFER),
        CA,
        crate::asdu::CallOrSelectFileInfo {
            ioa: IOA,
            nof: NOF,
            nos: 0,
            scq: crate::asdu::SelectAndCallQualifier {
                action: crate::asdu::ScqAction::SELECT_FILE,
                error: crate::asdu::FileError::NONE,
            },
        },
    )
    .unwrap();

    // The peer is told, and the caller is told.
    assert_eq!(
        sender.handle(&s_link, &select).await,
        Err(Error::FileNotFound)
    );
    let sent = s_link.take();
    assert_eq!(sent.len(), 1);
    let ack = sent[0].get_ack_file_or_section().unwrap();
    assert_eq!(ack.afq.action, crate::asdu::AfqAction::NEG_ACK_FILE);
    assert_eq!(
        ack.afq.error,
        crate::asdu::FileError::UNEXPECTED_NAME_OF_FILE
    );
    assert!(!sender.in_progress());
}

#[tokio::test]
async fn a_corrupted_section_is_rejected_and_served_again() {
    let data = parts(300);
    let (sender, s_link, receiver, r_link, dst) = pair(data.clone());
    sender.set_section_size(128);

    receiver.request_file(&r_link, CA, IOA, NOF).await.unwrap();

    // Drive the exchange by hand so one segment can be corrupted in flight.
    let mut corrupted = false;
    for _ in 0..1000 {
        let to_receiver = s_link.take();
        let to_sender = r_link.take();
        if to_receiver.is_empty() && to_sender.is_empty() {
            break;
        }
        for mut a in to_receiver {
            if !corrupted && a.type_id() == TypeId::F_SG_NA_1 {
                // Flip an octet of the payload: the section checksum will not
                // match, and the receiver must not append it to the file.
                let last = a.info_obj.len() - 1;
                a.info_obj[last] ^= 0xff;
                corrupted = true;
                assert_eq!(
                    receiver.handle(&r_link, &a).await,
                    Ok(true),
                    "a segment is buffered, not verified"
                );
                continue;
            }
            // The end of the corrupted section is where it is detected.
            let r = receiver.handle(&r_link, &a).await;
            if a.type_id() == TypeId::F_LS_NA_1 && r == Err(Error::FileChecksum) {
                continue;
            }
            r.expect("receiver");
        }
        for a in to_sender {
            sender.handle(&s_link, &a).await.expect("sender");
        }
    }

    assert!(corrupted, "no segment was corrupted, the test proved nothing");
    assert_eq!(
        dst.read(IOA, NOF).await.unwrap(),
        data,
        "the retransmitted section must repair the file"
    );
}

#[tokio::test]
async fn a_checksum_mismatch_is_reported_and_negatively_acknowledged() {
    let (_, _, receiver, r_link, _) = pair(parts(10));
    receiver.request_file(&r_link, CA, IOA, NOF).await.unwrap();
    r_link.take();

    // Announce a section, then close it with a checksum that matches nothing.
    let sr = Asdu::section_ready(
        PARAMS_WIDE,
        CauseOfTransmission::new(Cause::FILE_TRANSFER),
        CA,
        crate::asdu::SectionReadyInfo {
            ioa: IOA,
            nof: NOF,
            nos: 1,
            length_of_section: 1,
            srq: Default::default(),
        },
    )
    .unwrap();
    receiver.handle(&r_link, &sr).await.unwrap();
    r_link.take();

    let ls = Asdu::last_section_or_segment(
        PARAMS_WIDE,
        CauseOfTransmission::new(Cause::FILE_TRANSFER),
        CA,
        crate::asdu::LastSectionOrSegmentInfo {
            ioa: IOA,
            nof: NOF,
            nos: 1,
            lsq: crate::asdu::LastSectionQualifier::SECTION_WITHOUT_DEACTIVATE,
            chs: 0x5a,
        },
    )
    .unwrap();
    assert_eq!(receiver.handle(&r_link, &ls).await, Err(Error::FileChecksum));

    let sent = r_link.take();
    assert_eq!(sent.len(), 1);
    let ack = sent[0].get_ack_file_or_section().unwrap();
    assert_eq!(ack.afq.action, crate::asdu::AfqAction::NEG_ACK_SECTION);
    assert_eq!(ack.afq.error, crate::asdu::FileError::CHECKSUM_FAILED);
}

#[tokio::test]
async fn a_second_transfer_is_refused_while_one_is_running() {
    let (_, _, receiver, r_link, _) = pair(parts(10));
    receiver.request_file(&r_link, CA, IOA, NOF).await.unwrap();
    assert!(receiver.in_progress());
    assert_eq!(
        receiver.request_file(&r_link, CA, IOA, NOF).await,
        Err(Error::TransferBusy)
    );

    receiver.abort();
    assert!(!receiver.in_progress());
    assert!(receiver.request_file(&r_link, CA, IOA, NOF).await.is_ok());
}

#[tokio::test]
async fn a_failed_select_does_not_leave_the_receiver_busy() {
    let (_, _, receiver, r_link, _) = pair(parts(10));
    *r_link.fail.lock().unwrap() = Some(Error::UseClosedConnection);

    assert_eq!(
        receiver.request_file(&r_link, CA, IOA, NOF).await,
        Err(Error::UseClosedConnection)
    );
    assert!(
        !receiver.in_progress(),
        "a transfer that never started must not block the next one"
    );
}

#[tokio::test]
async fn a_negative_file_ready_is_reported() {
    let (_, _, receiver, r_link, _) = pair(parts(10));
    let fr = Asdu::file_ready(
        PARAMS_WIDE,
        CauseOfTransmission::new(Cause::FILE_TRANSFER),
        CA,
        crate::asdu::FileReadyInfo {
            ioa: IOA,
            nof: NOF,
            length_of_file: 0,
            frq: crate::asdu::FileReadyQualifier {
                qual: 0,
                is_negative: true,
            },
        },
    )
    .unwrap();
    assert_eq!(receiver.handle(&r_link, &fr).await, Err(Error::FileNotFound));
    assert!(!receiver.in_progress());
}

#[tokio::test]
async fn each_side_refuses_the_other_side_types() {
    // The sender produces monitor-direction ASDUs and must not act on them;
    // the receiver likewise for the control direction. Getting this wrong
    // would let a peer drive the wrong half of the state machine.
    let (sender, s_link, receiver, r_link, _) = pair(parts(10));
    let coa = CauseOfTransmission::new(Cause::FILE_TRANSFER);

    let monitor = Asdu::section_ready(PARAMS_WIDE, coa, CA, Default::default()).unwrap();
    assert_eq!(
        sender.handle(&s_link, &monitor).await,
        Err(Error::FileServiceUnsupported)
    );

    let control =
        Asdu::ack_file_or_section(PARAMS_WIDE, coa, CA, Default::default()).unwrap();
    assert_eq!(
        receiver.handle(&r_link, &control).await,
        Err(Error::FileServiceUnsupported)
    );
}

#[tokio::test]
async fn ordinary_process_data_falls_through_to_the_application() {
    let (sender, s_link, receiver, r_link, _) = pair(parts(10));
    let a = Asdu::single(
        PARAMS_WIDE,
        false,
        CauseOfTransmission::new(Cause::SPONTANEOUS),
        CA,
        &[crate::asdu::SinglePointInfo::new(1, true)],
    )
    .unwrap();

    assert_eq!(sender.handle(&s_link, &a).await, Ok(false));
    assert_eq!(receiver.handle(&r_link, &a).await, Ok(false));
    assert!(s_link.take().is_empty());
    assert!(r_link.take().is_empty());
}

#[tokio::test]
async fn deleting_a_file_clears_a_transfer_of_that_file() {
    let src = Arc::new(MemStore::new());
    src.insert(IOA, NOF, parts(100));
    let sender = Sender::new(src.clone());
    let s_link = Recorder::default();

    let select = |action| {
        Asdu::call_or_select_file(
            PARAMS_WIDE,
            CauseOfTransmission::new(Cause::FILE_TRANSFER),
            CA,
            crate::asdu::CallOrSelectFileInfo {
                ioa: IOA,
                nof: NOF,
                nos: 0,
                scq: crate::asdu::SelectAndCallQualifier {
                    action,
                    error: crate::asdu::FileError::NONE,
                },
            },
        )
        .unwrap()
    };

    sender
        .handle(&s_link, &select(crate::asdu::ScqAction::SELECT_FILE))
        .await
        .unwrap();
    assert!(sender.in_progress());

    sender
        .handle(&s_link, &select(crate::asdu::ScqAction::DELETE_FILE))
        .await
        .unwrap();
    assert!(!sender.in_progress(), "the file being served went away");
    assert_eq!(src.read(IOA, NOF).await, Err(Error::FileNotFound));
}

#[tokio::test]
async fn deactivating_a_file_ends_the_transfer() {
    let (sender, s_link, _, _, _) = pair(parts(100));
    let call = |action| {
        Asdu::call_or_select_file(
            PARAMS_WIDE,
            CauseOfTransmission::new(Cause::FILE_TRANSFER),
            CA,
            crate::asdu::CallOrSelectFileInfo {
                ioa: IOA,
                nof: NOF,
                nos: 0,
                scq: crate::asdu::SelectAndCallQualifier {
                    action,
                    error: crate::asdu::FileError::NONE,
                },
            },
        )
        .unwrap()
    };

    sender
        .handle(&s_link, &call(crate::asdu::ScqAction::SELECT_FILE))
        .await
        .unwrap();
    assert!(sender.in_progress());

    sender
        .handle(&s_link, &call(crate::asdu::ScqAction::DEACTIVATE_FILE))
        .await
        .unwrap();
    assert!(!sender.in_progress());
    assert!(
        s_link.take().iter().all(|a| a.type_id() != TypeId::F_SG_NA_1),
        "nothing more may be served after a deactivation"
    );
}

#[tokio::test]
async fn a_section_number_that_names_nothing_is_refused() {
    let (sender, s_link, _, _, _) = pair(parts(100));
    let call = |action, nos| {
        Asdu::call_or_select_file(
            PARAMS_WIDE,
            CauseOfTransmission::new(Cause::FILE_TRANSFER),
            CA,
            crate::asdu::CallOrSelectFileInfo {
                ioa: IOA,
                nof: NOF,
                nos,
                scq: crate::asdu::SelectAndCallQualifier {
                    action,
                    error: crate::asdu::FileError::NONE,
                },
            },
        )
        .unwrap()
    };

    sender
        .handle(&s_link, &call(crate::asdu::ScqAction::SELECT_FILE, 0))
        .await
        .unwrap();
    s_link.take();

    // 100 octets is one section, so section 9 does not exist.
    assert_eq!(
        sender
            .handle(&s_link, &call(crate::asdu::ScqAction::SELECT_SECTION, 9))
            .await,
        Err(Error::NoTransfer)
    );
    let sent = s_link.take();
    assert_eq!(sent.len(), 1);
    let ack = sent[0].get_ack_file_or_section().unwrap();
    assert_eq!(ack.afq.action, crate::asdu::AfqAction::NEG_ACK_SECTION);
    assert_eq!(
        ack.afq.error,
        crate::asdu::FileError::UNEXPECTED_NAME_OF_SECTION
    );
}

#[tokio::test]
async fn a_segment_for_another_transfer_is_not_appended() {
    let (_, _, receiver, r_link, _) = pair(parts(10));
    receiver.request_file(&r_link, CA, IOA, NOF).await.unwrap();
    r_link.take();

    // A segment naming a different file must not join this one's buffer.
    let stray = Asdu::file_segment(
        PARAMS_WIDE,
        CauseOfTransmission::new(Cause::FILE_TRANSFER),
        CA,
        &crate::asdu::SegmentInfo {
            ioa: IOA,
            nof: NameOfFile::TRANSPARENT,
            nos: 1,
            segment: vec![0xff; 8],
        },
    )
    .unwrap();
    assert_eq!(
        receiver.handle(&r_link, &stray).await,
        Err(Error::NoTransfer)
    );
}

#[tokio::test]
async fn the_segment_size_never_exceeds_what_the_parameters_allow() {
    // Every segment must fit the ASDU, whatever the address widths.
    for params in [
        crate::asdu::PARAMS_NARROW,
        crate::asdu::PARAMS_STANDARD_101,
        PARAMS_WIDE,
    ] {
        struct P(Params, Mutex<Vec<Asdu>>);
        #[async_trait::async_trait]
        impl Connect for P {
            fn params(&self) -> Params {
                self.0
            }
            async fn send(&self, a: Asdu) -> Result<()> {
                self.1.lock().unwrap().push(a);
                Ok(())
            }
        }

        let src = Arc::new(MemStore::new());
        src.insert(1, NOF, parts(2000));
        let sender = Sender::new(src);
        let link = P(params, Mutex::new(Vec::new()));

        let select = Asdu::call_or_select_file(
            params,
            CauseOfTransmission::new(Cause::FILE_TRANSFER),
            CA,
            crate::asdu::CallOrSelectFileInfo {
                ioa: 1,
                nof: NOF,
                nos: 0,
                scq: crate::asdu::SelectAndCallQualifier {
                    action: crate::asdu::ScqAction::REQUEST_FILE,
                    error: crate::asdu::FileError::NONE,
                },
            },
        )
        .unwrap();
        sender.handle(&link, &select).await.unwrap();

        let request = Asdu::call_or_select_file(
            params,
            CauseOfTransmission::new(Cause::FILE_TRANSFER),
            CA,
            crate::asdu::CallOrSelectFileInfo {
                ioa: 1,
                nof: NOF,
                nos: 1,
                scq: crate::asdu::SelectAndCallQualifier {
                    action: crate::asdu::ScqAction::REQUEST_SECTION,
                    error: crate::asdu::FileError::NONE,
                },
            },
        )
        .unwrap();
        sender.handle(&link, &request).await.unwrap();

        let sent = std::mem::take(&mut *link.1.lock().unwrap());
        let segments: Vec<_> = sent
            .iter()
            .filter(|a| a.type_id() == TypeId::F_SG_NA_1)
            .collect();
        assert!(!segments.is_empty(), "{params:?}");
        for a in segments {
            let seg = a.get_file_segment().unwrap();
            assert!(
                seg.segment.len() <= params.max_segment_size(),
                "{params:?}: a {} octet segment exceeds the {} the parameters allow",
                seg.segment.len(),
                params.max_segment_size()
            );
            // And it must still be a legal ASDU on the wire.
            a.marshal_binary().expect("segment must encode");
        }
    }
}
