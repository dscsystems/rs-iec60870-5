// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Verify that file sends under backpressure do not block the FT1.2 driver.
#![cfg(all(feature = "cs101", feature = "filetransfer"))]
#[path = "support/file_receiver.rs"]
mod file_receiver;
use file_receiver::Receiver;
use rs_iec60870_5::{asdu::*, cs101::*, filetransfer::*};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;
struct Outstation {
    sender: Mutex<Sender>,
}
#[async_trait::async_trait]
impl ServerHandler for Outstation {
    async fn asdu(&self, c: &dyn Connect, a: &Asdu) -> rs_iec60870_5::Result<()> {
        self.sender.lock().await.handle(c, a).await?;
        Ok(())
    }
}
struct Master {
    receiver: Arc<Mutex<Receiver>>,
}
#[async_trait::async_trait]
impl ClientHandler for Master {
    async fn asdu(&self, c: &dyn Connect, a: &Asdu, _: usize) -> rs_iec60870_5::Result<()> {
        self.receiver.lock().await.handle(c, a).await?;
        Ok(())
    }
}
#[tokio::test]
async fn file_transfer_with_a_one_asdu_class_buffer() {
    let data: Vec<u8> = (0..5000).map(|i| (i * 17 + 3) as u8).collect();
    let source = Arc::new(MemStore::new());
    source.insert(100, NameOfFile(2), data.clone());
    let cfg = Config {
        transport: TransportType::TcpServer,
        tcp: TcpConfig {
            address: "127.0.0.1:0".into(),
            ..Default::default()
        },
        timeout_send_link_msg: Duration::from_millis(1),
        max_send_queue_size: 1,
        ..Config::default()
    };
    let server = Server::new(Outstation {
        sender: Mutex::new(Sender::new(source)),
    })
    .with_config(cfg.clone())
    .unwrap();
    server.bind().await.unwrap();
    let addr = server.listen_addr().unwrap();
    server.start().unwrap();
    let receiver = Arc::new(Mutex::new(Receiver::new(None)));
    let client = Client::new(
        Master {
            receiver: receiver.clone(),
        },
        ClientOption::new()
            .with_config(Config {
                transport: TransportType::TcpClient,
                tcp: TcpConfig {
                    address: addr.to_string(),
                    ..Default::default()
                },
                ..cfg
            })
            .unwrap()
            .with_auto_reconnect(false),
    );
    client.start().unwrap();
    tokio::time::timeout(Duration::from_secs(5), client.wait_link_active())
        .await
        .unwrap();
    receiver
        .lock()
        .await
        .request_file(client.as_ref(), 1, 100, 2)
        .await
        .unwrap();
    let files = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let files = receiver.lock().await.take_completed();
            if !files.is_empty() {
                break files;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(files[0].data, data);
    client.close();
    server.close();
}
