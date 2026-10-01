// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Raw-peer regression checks for the IEC 104 state machine.
#![cfg(feature = "cs104")]
use rs_iec60870_5::{Error, Result, asdu::*, cs104::*};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
struct Handler;
#[async_trait::async_trait]
impl ServerHandler for Handler {}
async fn frame(stream: &mut TcpStream) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut header = [0; 2];
        stream.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0], 0x68);
        let mut f = vec![0; header[1] as usize + 2];
        f[..2].copy_from_slice(&header);
        stream.read_exact(&mut f[2..]).await.unwrap();
        f
    })
    .await
    .unwrap()
}
async fn start() -> (Arc<Server<Handler>>, tokio::task::JoinHandle<()>, TcpStream) {
    let server = Server::new(Handler);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let s = server.clone();
    let task = tokio::spawn(async move { s.serve(listener).await.unwrap() });
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(&[0x68, 4, 7, 0, 0, 0]).await.unwrap();
    assert_eq!(frame(&mut stream).await, [0x68, 4, 11, 0, 0, 0]);
    (server, task, stream)
}
async fn pending_stop(server: &Server<Handler>, stream: &mut TcpStream) -> u16 {
    server
        .send(
            Asdu::single(
                PARAMS_WIDE,
                false,
                CauseOfTransmission::new(Cause::SPONTANEOUS),
                1,
                &[SinglePointInfo::new(100, true)],
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let f = frame(stream).await;
    assert_eq!(f[2] & 1, 0);
    let ack = (u16::from_le_bytes([f[2], f[3]]) >> 1) + 1;
    stream.write_all(&[0x68, 4, 19, 0, 0, 0]).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), stream.read_u8())
            .await
            .is_err(),
        "STOPDT must await the I-frame acknowledgement"
    );
    ack
}
#[tokio::test]
async fn stopdt_waits_for_outstanding_data_and_resumes_without_losing_sequence() {
    let (server, task, mut stream) = start().await;
    let ack = pending_stop(&server, &mut stream).await;
    stream
        .write_all(&[0x68, 4, 1, 0, (ack << 1) as u8, (ack >> 7) as u8])
        .await
        .unwrap();
    assert_eq!(frame(&mut stream).await, [0x68, 4, 35, 0, 0, 0]);
    stream.write_all(&[0x68, 4, 7, 0, 0, 0]).await.unwrap();
    assert_eq!(frame(&mut stream).await, [0x68, 4, 11, 0, 0, 0]);
    server
        .send(
            Asdu::single(
                PARAMS_WIDE,
                false,
                CauseOfTransmission::new(Cause::SPONTANEOUS),
                1,
                &[SinglePointInfo::new(101, false)],
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let f = frame(&mut stream).await;
    assert_eq!(u16::from_le_bytes([f[2], f[3]]) >> 1, ack);
    server.close();
    task.await.unwrap();
}
#[tokio::test]
async fn startdt_supersedes_a_pending_stopdt() {
    let (server, task, mut stream) = start().await;
    let ack = pending_stop(&server, &mut stream).await;
    stream.write_all(&[0x68, 4, 7, 0, 0, 0]).await.unwrap();
    assert_eq!(frame(&mut stream).await, [0x68, 4, 11, 0, 0, 0]);
    stream
        .write_all(&[0x68, 4, 1, 0, (ack << 1) as u8, (ack >> 7) as u8])
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(150), stream.read_u8())
            .await
            .is_err()
    );
    server.close();
    task.await.unwrap();
}
#[tokio::test]
async fn malformed_u_frames_cannot_activate_a_station() {
    let server = Server::new(Handler);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let s = server.clone();
    let task = tokio::spawn(async move { s.serve(listener).await.unwrap() });
    for invalid in [
        [0x68, 4, 7, 1, 0, 0].to_vec(),
        [0x68, 5, 7, 0, 0, 0, 99].to_vec(),
        [0x68, 4, 15, 0, 0, 0].to_vec(),
    ] {
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(&invalid).await.unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read_u8())
            .await
            .unwrap();
        assert!(closed.is_err());
    }
    server.close();
    task.await.unwrap();
}
struct Queue {
    attempts: AtomicUsize,
    accepted: AtomicUsize,
    failures: usize,
}
#[async_trait::async_trait]
impl Connect for Queue {
    fn params(&self) -> Params {
        PARAMS_WIDE
    }
    async fn send(&self, _: Asdu) -> Result<()> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) < self.failures {
            return Err(Error::BufferFull);
        }
        self.accepted.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[tokio::test]
async fn waiting_retries_full_queues_and_obeys_its_deadline() {
    let a = Asdu::single(
        PARAMS_WIDE,
        false,
        CauseOfTransmission::new(Cause::SPONTANEOUS),
        1,
        &[SinglePointInfo::new(100, true)],
    )
    .unwrap();
    let q = Arc::new(Queue {
        attempts: AtomicUsize::new(0),
        accepted: AtomicUsize::new(0),
        failures: 3,
    });
    let w = Waiting::new(&q, tokio::time::Instant::now() + Duration::from_secs(1));
    w.send(a.clone()).await.unwrap();
    assert_eq!(q.accepted.load(Ordering::SeqCst), 1);
    let full = Queue {
        attempts: AtomicUsize::new(0),
        accepted: AtomicUsize::new(0),
        failures: usize::MAX,
    };
    assert_eq!(
        full.send_wait(a, tokio::time::Instant::now() + Duration::from_millis(20))
            .await,
        Err(Error::SendTimeout)
    );
    assert_eq!(full.accepted.load(Ordering::SeqCst), 0);
}
