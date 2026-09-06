// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! File transfer driven over a real IEC 104 connection: a [`Sender`] inside a
//! [`Server`] handler and a [`Receiver`] inside a [`Client`] handler, talking
//! over loopback TCP.
//!
//! The unit tests drive the two components against each other directly. This
//! puts the whole stack between them — APCI framing, the k/w windows, the
//! send queues — because that is where a transfer of several hundred ASDUs
//! actually has to survive.

#![cfg(all(feature = "cs104", feature = "filetransfer"))]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use std::sync::Mutex;

use tokio::net::TcpListener;

use rs_iec60870_5::asdu::{Asdu, Connect, NameOfFile};
use rs_iec60870_5::cs104::{Client, ClientHandler, ClientOption, Server, ServerHandler};
use rs_iec60870_5::filetransfer::{MemStore, Receiver, Sender};

const CA: u16 = 1;
const IOA: u32 = 100;
const NOF: NameOfFile = NameOfFile::DISTURBANCE_DATA;

/// The outstation: serves whatever its store holds.
struct Outstation {
    files: Arc<Sender>,
}

#[async_trait::async_trait]
impl ServerHandler for Outstation {
    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        if self.files.handle(c, pack).await? {
            return Ok(());
        }
        Ok(())
    }
}

/// The master: assembles files into its own store.
struct Master {
    files: Arc<Receiver>,
}

#[async_trait::async_trait]
impl ClientHandler for Master {
    async fn asdu(&self, c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        if self.files.handle(c, pack).await? {
            return Ok(());
        }
        Ok(())
    }
}

/// A disturbance record with enough structure that a swapped or dropped
/// segment shows up as a mismatch rather than passing unnoticed.
fn record(size: usize) -> Vec<u8> {
    (0..size).map(|i| ((i * 7 + i / 251) % 256) as u8).collect()
}

struct Rig {
    _srv: Arc<Server<Outstation>>,
    cli: Arc<Client<Master>>,
    received: Arc<MemStore>,
    done: Arc<AtomicBool>,
    got: Arc<Mutex<Vec<u8>>>,
}

async fn rig(data: Vec<u8>, section_size: usize) -> Rig {
    let source = Arc::new(MemStore::new());
    source.insert(IOA, NOF, data);
    let sender = Arc::new(Sender::new(source));
    sender.set_section_size(section_size);

    let srv = Server::new(Outstation {
        files: Arc::clone(&sender),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    {
        let srv = Arc::clone(&srv);
        tokio::spawn(async move { srv.serve(listener).await });
    }

    let received = Arc::new(MemStore::new());
    let receiver = Arc::new(Receiver::new(received.clone()));
    let done = Arc::new(AtomicBool::new(false));
    let got = Arc::new(Mutex::new(Vec::new()));
    {
        let (done, got) = (Arc::clone(&done), Arc::clone(&got));
        receiver.set_file_handler(Box::new(move |_, data| {
            *got.lock().unwrap() = data;
            done.store(true, Ordering::SeqCst);
        }));
    }

    let option = ClientOption::new().with_server(&addr.to_string()).unwrap();
    let cli = Client::new(
        Master {
            files: Arc::clone(&receiver),
        },
        option,
    );
    cli.start().unwrap();
    tokio::time::timeout(Duration::from_secs(5), cli.wait_active())
        .await
        .expect("the client did not activate");

    // The receiver drives the transfer from the client side.
    let conn: &dyn Connect = cli.as_ref();
    receiver
        .request_file(conn, CA, IOA, NOF)
        .await
        .expect("select the file");

    Rig {
        _srv: srv,
        cli,
        received,
        done,
        got,
    }
}

async fn wait_done(done: &AtomicBool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !done.load(Ordering::SeqCst) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the transfer did not complete"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn a_file_crosses_a_real_connection_intact() {
    let data = record(20_000);
    let r = rig(data.clone(), 4096).await;
    wait_done(&r.done).await;

    assert_eq!(*r.got.lock().unwrap(), data, "the delivered file differs");
    use rs_iec60870_5::filetransfer::Store;
    assert_eq!(
        r.received.read(IOA, NOF).await.unwrap(),
        data,
        "the stored file differs"
    );
    r.cli.close();
}

#[tokio::test]
async fn a_transfer_of_many_small_sections_survives_the_send_window() {
    // Small sections over a 20 kB file run the section handshake hundreds of
    // times and push well over a thousand ASDUs through the link, so the k
    // window fills repeatedly: the transfer only completes if the flow
    // control and the section handshake interleave correctly.
    let data = record(20_000);
    let r = rig(data.clone(), 64).await;
    wait_done(&r.done).await;

    assert_eq!(*r.got.lock().unwrap(), data);
    r.cli.close();
}

#[tokio::test]
async fn a_file_needing_more_sections_than_nos_can_name_still_completes() {
    // NOS is one octet. Asking for 64 octet sections on a 100 kB file would
    // want 1563 of them; numbered in a byte they wrap, and the receiver ends
    // up re-requesting a section it has already had — a transfer that runs
    // for ever. The sections are grown to fit instead.
    let data = record(100_000);
    let r = rig(data.clone(), 64).await;
    wait_done(&r.done).await;

    assert_eq!(*r.got.lock().unwrap(), data);
    r.cli.close();
}

#[tokio::test]
async fn an_empty_file_completes_rather_than_hanging() {
    let r = rig(Vec::new(), 4096).await;
    wait_done(&r.done).await;

    assert!(r.got.lock().unwrap().is_empty());
    r.cli.close();
}
