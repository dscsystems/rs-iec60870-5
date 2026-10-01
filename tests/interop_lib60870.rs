// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Live IEC 104 and file codec checks against lib60870-C's public API.
#![cfg(all(feature = "cs104", feature = "filetransfer"))]
mod common;
use common::*;
#[path = "support/file_receiver.rs"]
mod file_receiver;
use file_receiver::Receiver;
use rs_iec60870_5::{asdu::*, cs104::*, filetransfer::*};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use tokio::{net::TcpListener, sync::Mutex};

fn lib_bin() -> Option<&'static PathBuf> {
    static BIN: OnceLock<Option<PathBuf>> = OnceLock::new();
    BIN.get_or_init(|| {
        let Some(root) = std::env::var_os("LIB60870_ROOT") else {
            assert!(
                std::env::var_os("IEC60870_INTEROP_REQUIRED").is_none(),
                "LIB60870_ROOT required"
            );
            eprintln!("SKIP: set LIB60870_ROOT to a built lib60870-C checkout");
            return None;
        };
        let root = PathBuf::from(root);
        let bin = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/lib60870-peer");
        let result = std::process::Command::new("cc")
            .args([
                "-std=c11",
                "-Wall",
                "-Wextra",
                "-Wno-unused-parameter",
                "-pthread",
            ])
            .arg("-I")
            .arg(root.join("src/inc/api"))
            .arg("-I")
            .arg(root.join("src/hal/inc"))
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/interop/lib60870/peer.c"))
            .arg(root.join("build/liblib60870.a"))
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("C compiler");
        assert!(
            result.status.success(),
            "lib60870 peer build failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        Some(bin)
    })
    .as_ref()
}
fn fixture() -> Vec<u8> {
    (0..600).map(|i| (i * 17 + 3) as u8).collect()
}
struct Master {
    log: Arc<Mutex<Vec<Asdu>>>,
    receiver: Arc<Mutex<Receiver>>,
}
#[async_trait::async_trait]
impl ClientHandler for Master {
    async fn asdu_all(
        &self,
        _: &dyn Connect,
        a: &Asdu,
        _: &ClientContext,
    ) -> rs_iec60870_5::Result<()> {
        self.log.lock().await.push(a.clone());
        Ok(())
    }
    async fn asdu(&self, c: &dyn Connect, a: &Asdu) -> rs_iec60870_5::Result<()> {
        self.receiver.lock().await.handle(c, a).await?;
        Ok(())
    }
}

#[tokio::test]
async fn rust_master_with_lib60870_outstation() {
    let Some(bin) = lib_bin() else { return };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    drop(listener);
    let peer = GoPeer::spawn(bin, &["server", &port]).await;
    let addr = peer.wait_ready().await;
    let log = Arc::new(Mutex::new(Vec::new()));
    let receiver = Arc::new(Mutex::new(Receiver::new(None)));
    let client = Client::new(
        Master {
            log: log.clone(),
            receiver: receiver.clone(),
        },
        ClientOption::new()
            .with_server(&addr)
            .unwrap()
            .with_auto_reconnect(false),
    );
    client.start().unwrap();
    tokio::time::timeout(WAIT, client.wait_active())
        .await
        .unwrap();
    client
        .send_interrogation_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION),
            1,
            QualifierOfInterrogation::STATION,
        )
        .await
        .unwrap();
    client
        .send_single_cmd(
            TypeId::C_SC_NA_1,
            CauseOfTransmission::new(Cause::ACTIVATION),
            1,
            SingleCommandInfo {
                ioa: 500,
                value: true,
                qoc: QualifierOfCommand {
                    in_select: true,
                    ..Default::default()
                },
                time: None,
                time_flags: TimeTagFlags::GOOD,
            },
        )
        .await
        .unwrap();
    let command = peer
        .wait("select command", |v| v["event"] == "command")
        .await;
    assert_eq!(command["ioa"], 500);
    assert_eq!(command["value"], 1);
    assert_eq!(command["select"], 1);
    eventually("GI termination", || async {
        log.lock()
            .await
            .iter()
            .any(|a| a.type_id() == TypeId::C_IC_NA_1 && a.coa().cause == Cause::ACTIVATION_TERM)
    })
    .await;
    let asdus = log.lock().await;
    assert_eq!(
        asdus
            .iter()
            .find(|a| a.type_id() == TypeId::M_SP_NA_1)
            .unwrap()
            .get_single_point()
            .unwrap()[0],
        SinglePointInfo::new(100, true)
    );
    let mv = asdus
        .iter()
        .find(|a| a.type_id() == TypeId::M_ME_NC_1)
        .unwrap()
        .get_measured_value_float()
        .unwrap()[0];
    assert_eq!(mv.value, 22.5);
    assert_eq!(mv.qds, QualityDescriptor::INVALID);
    drop(asdus);
    receiver
        .lock()
        .await
        .request_directory(client.as_ref(), 1)
        .await
        .unwrap();
    eventually("directory", || async {
        log.lock()
            .await
            .iter()
            .any(|a| a.type_id() == TypeId::F_DR_TA_1)
    })
    .await;
    let directory = receiver.lock().await.take_directory();
    assert_eq!(directory.len(), 1);
    assert_eq!(directory[0].length_of_file, 600);
    receiver
        .lock()
        .await
        .request_file(client.as_ref(), 1, 100, 2)
        .await
        .unwrap();
    peer.wait("file acknowledgement", |v| v["event"] == "file_ack")
        .await;
    let completed = receiver.lock().await.take_completed();
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].data, fixture());
    client.close();
    peer.kill().await;
}
struct Outstation {
    sender: Mutex<Sender>,
}
#[async_trait::async_trait]
impl ServerHandler for Outstation {
    async fn interrogation(
        &self,
        c: &dyn Connect,
        a: &Asdu,
        _: QualifierOfInterrogation,
    ) -> rs_iec60870_5::Result<()> {
        c.send(a.reply_mirror(Cause::ACTIVATION_CON)).await?;
        c.send_single(
            false,
            CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION),
            1,
            &[SinglePointInfo::new(100, true)],
        )
        .await?;
        c.send(a.reply_mirror(Cause::ACTIVATION_TERM)).await
    }
    async fn asdu(&self, c: &dyn Connect, a: &Asdu) -> rs_iec60870_5::Result<()> {
        if self.sender.lock().await.handle(c, a).await? {
            return Ok(());
        }
        if a.type_id() == TypeId::C_SC_NA_1 {
            assert_eq!(a.orig_addr(), 7);
            assert_eq!(a.get_single_cmd()?.ioa, 500);
            c.send(a.reply_mirror(Cause::ACTIVATION_CON)).await?;
            return c.send(a.reply_mirror(Cause::ACTIVATION_TERM)).await;
        }
        Ok(())
    }
}
#[tokio::test]
async fn lib60870_master_with_rust_outstation() {
    let Some(bin) = lib_bin() else { return };
    let store = Arc::new(MemStore::new());
    store.insert(100, NameOfFile(2), fixture());
    let sender = Sender::new(store);
    sender.set_section_size(256);
    let server = Server::new(Outstation {
        sender: Mutex::new(sender),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port().to_string();
    let serving = server.clone();
    let task = tokio::spawn(async move { serving.serve(listener).await.unwrap() });
    let peer = GoPeer::spawn(bin, &["client", &port]).await;
    peer.wait("GI termination", |v| v["type"] == 100 && v["cause"] == 10)
        .await;
    let confirm = peer
        .wait("command confirmation", |v| {
            v["type"] == 45 && v["cause"] == 7
        })
        .await;
    assert_eq!(confirm["oa"], 7);
    assert_eq!(confirm["payload"], serde_json::json!([244, 1, 0, 129]));
    peer.wait("directory", |v| v["type"] == 126).await;
    let file = peer.wait("complete file", |v| v["event"] == "file").await;
    assert_eq!(file["size"], 600);
    peer.kill().await;
    server.close();
    task.await.unwrap();
}
#[test]
fn file_codecs_match_lib60870_byte_for_byte() {
    let Some(bin) = lib_bin() else { return };
    let output = std::process::Command::new(bin)
        .arg("vectors")
        .output()
        .unwrap();
    assert!(output.status.success());
    let time = chrono::DateTime::from_timestamp_millis(1787056496789).unwrap();
    let p = PARAMS_WIDE;
    let c = CauseOfTransmission::new(Cause::FILE_TRANSFER);
    let expected = [
        Asdu::file_ready(
            p,
            c,
            1,
            FileReadyInfo {
                ioa: 100,
                nof: NameOfFile(2),
                length_of_file: 600,
                frq: FileReadyQualifier::parse(0),
            },
        )
        .unwrap(),
        Asdu::section_ready(
            p,
            c,
            1,
            SectionReadyInfo {
                ioa: 100,
                nof: NameOfFile(2),
                nos: 1,
                length_of_section: 600,
                srq: SectionReadyQualifier::parse(0),
            },
        )
        .unwrap(),
        Asdu::call_or_select_file(
            p,
            c,
            1,
            CallOrSelectFileInfo {
                ioa: 100,
                nof: NameOfFile(2),
                nos: 1,
                scq: SelectAndCallQualifier::parse(6),
            },
        )
        .unwrap(),
        Asdu::last_section_or_segment(
            p,
            c,
            1,
            LastSectionOrSegmentInfo {
                ioa: 100,
                nof: NameOfFile(2),
                nos: 1,
                lsq: LastSectionQualifier(3),
                chs: 42,
            },
        )
        .unwrap(),
        Asdu::ack_file_or_section(
            p,
            c,
            1,
            AckFileOrSectionInfo {
                ioa: 100,
                nof: NameOfFile(2),
                nos: 1,
                afq: AckFileOrSectionQualifier::parse(3),
            },
        )
        .unwrap(),
        Asdu::file_segment(
            p,
            c,
            1,
            &SegmentInfo {
                ioa: 100,
                nof: NameOfFile(2),
                nos: 1,
                segment: fixture()[..236].to_vec(),
            },
        )
        .unwrap(),
        Asdu::file_directory(
            p,
            CauseOfTransmission::new(Cause::REQUEST),
            1,
            &[DirectoryInfo {
                ioa: 100,
                nof: NameOfFile(2),
                length_of_file: 600,
                sof: StatusOfFile::parse(32),
                time: Some(time),
                time_flags: TimeTagFlags::GOOD,
            }],
        )
        .unwrap(),
    ];
    let events: Vec<serde_json::Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(events.len(), 7);
    for (event, mut a) in events.into_iter().zip(expected) {
        a.identifier.orig_addr = 7;
        assert_eq!(event["type"], a.type_id().0);
        assert_eq!(event["payload"], serde_json::json!(a.info_obj));
        let raw = a.marshal_binary().unwrap();
        assert_eq!(Asdu::unmarshal_binary(p, &raw).unwrap(), a);
    }
}

#[tokio::test]
async fn file_transfer_with_current_go_outstation() {
    let Some(bins) = go_bins() else { return };
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);
    let peer = GoPeer::spawn(&bins.join("gofile"), &["server", &addr]).await;
    peer.wait_ready().await;
    let receiver = Arc::new(Mutex::new(Receiver::new(None)));
    let log = Arc::new(Mutex::new(Vec::new()));
    let client = Client::new(
        Master {
            log: log.clone(),
            receiver: receiver.clone(),
        },
        ClientOption::new()
            .with_server(&addr)
            .unwrap()
            .with_auto_reconnect(false),
    );
    client.start().unwrap();
    tokio::time::timeout(WAIT, client.wait_active())
        .await
        .unwrap();
    receiver
        .lock()
        .await
        .request_directory(client.as_ref(), 1)
        .await
        .unwrap();
    eventually("Go directory", || async {
        !receiver.lock().await.take_directory().is_empty()
    })
    .await;
    receiver
        .lock()
        .await
        .request_file(client.as_ref(), 1, 100, 2)
        .await
        .unwrap();
    peer.wait("Go file acknowledgement", |v| v["event"] == "file_ack")
        .await;
    assert_eq!(receiver.lock().await.take_completed()[0].data, fixture());
    client.close();
    peer.kill().await;
}
#[tokio::test]
async fn file_transfer_with_current_go_master() {
    let Some(bins) = go_bins() else { return };
    let store = Arc::new(MemStore::new());
    store.insert(100, NameOfFile(2), fixture());
    let sender = Sender::new(store);
    sender.set_section_size(256);
    let server = Server::new(Outstation {
        sender: Mutex::new(sender),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let serving = server.clone();
    let task = tokio::spawn(async move { serving.serve(listener).await.unwrap() });
    let peer = GoPeer::spawn(&bins.join("gofile"), &["client", &addr]).await;
    peer.wait("Go directory", |v| {
        v["event"] == "directory" && v["count"] == 1
    })
    .await;
    let file = peer
        .wait("Go completed file", |v| {
            v["event"] == "file" && v["size"] == 600
        })
        .await;
    assert_eq!(file["data"], serde_json::json!(fixture()));
    peer.kill().await;
    server.close();
    task.await.unwrap();
}
#[cfg(all(unix, feature = "serial"))]
async fn serial_bridge() -> (GoPeer, String, String) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/interop/lib60870/pty_bridge.py");
    let peer = GoPeer::spawn(Path::new("python3"), &[path.to_str().unwrap()]).await;
    let ready = peer.wait("PTY pair", |v| v["event"] == "ready").await;
    (
        peer,
        ready["addr"].as_str().unwrap().into(),
        ready["other"].as_str().unwrap().into(),
    )
}
#[cfg(all(unix, feature = "serial"))]
fn serial_config(path: &str) -> rs_iec60870_5::cs101::Config {
    use rs_iec60870_5::cs101::*;
    Config {
        serial: SerialConfig {
            parity: Parity::None,
            ..SerialConfig::new(path, 9600)
        },
        timeout_send_link_msg: std::time::Duration::from_millis(10),
        ..Config::default()
    }
}
#[cfg(all(unix, feature = "serial"))]
struct SerialMaster {
    log: Arc<Mutex<Vec<Asdu>>>,
}
#[cfg(all(unix, feature = "serial"))]
#[async_trait::async_trait]
impl rs_iec60870_5::cs101::ClientHandler for SerialMaster {
    async fn asdu_all(&self, _: &dyn Connect, a: &Asdu, _: usize) -> rs_iec60870_5::Result<()> {
        self.log.lock().await.push(a.clone());
        Ok(())
    }
}
#[cfg(all(unix, feature = "serial"))]
#[tokio::test]
async fn rust_101_primary_with_lib60870_secondary() {
    let Some(bin) = lib_bin() else { return };
    let (bridge, left, right) = serial_bridge().await;
    let peer = GoPeer::spawn(bin, &["server101", &right]).await;
    peer.wait_ready().await;
    let log = Arc::new(Mutex::new(Vec::new()));
    let client = rs_iec60870_5::cs101::Client::new(
        SerialMaster { log: log.clone() },
        rs_iec60870_5::cs101::ClientOption::new()
            .with_config(serial_config(&left))
            .unwrap()
            .with_auto_reconnect(false),
    );
    client.start().unwrap();
    tokio::time::timeout(WAIT, client.wait_link_active())
        .await
        .unwrap();
    client
        .send_interrogation_cmd(
            CauseOfTransmission::new(Cause::ACTIVATION),
            1,
            QualifierOfInterrogation::STATION,
        )
        .await
        .unwrap();
    client
        .send_single_cmd(
            TypeId::C_SC_NA_1,
            CauseOfTransmission::new(Cause::ACTIVATION),
            1,
            SingleCommandInfo {
                ioa: 500,
                value: true,
                qoc: QualifierOfCommand {
                    in_select: true,
                    ..Default::default()
                },
                time: None,
                time_flags: TimeTagFlags::GOOD,
            },
        )
        .await
        .unwrap();
    peer.wait("101 select command", |v| {
        v["event"] == "command" && v["select"] == 1
    })
    .await;
    eventually("101 GI termination", || async {
        log.lock()
            .await
            .iter()
            .any(|a| a.type_id() == TypeId::C_IC_NA_1 && a.coa().cause == Cause::ACTIVATION_TERM)
    })
    .await;
    let log = log.lock().await;
    let mv = log
        .iter()
        .find(|a| a.type_id() == TypeId::M_ME_NC_1)
        .unwrap()
        .get_measured_value_float()
        .unwrap()[0];
    assert_eq!(mv.value, 22.5);
    assert_eq!(mv.qds, QualityDescriptor::INVALID);
    drop(log);
    client.close();
    peer.kill().await;
    bridge.kill().await;
}
#[cfg(all(unix, feature = "serial"))]
struct SerialOutstation;
#[cfg(all(unix, feature = "serial"))]
#[async_trait::async_trait]
impl rs_iec60870_5::cs101::ServerHandler for SerialOutstation {
    async fn interrogation(
        &self,
        c: &dyn Connect,
        a: &Asdu,
        _: QualifierOfInterrogation,
    ) -> rs_iec60870_5::Result<()> {
        c.send(a.reply_mirror(Cause::ACTIVATION_CON)).await?;
        c.send_single(
            false,
            CauseOfTransmission::new(Cause::INTERROGATED_BY_STATION),
            1,
            &[SinglePointInfo::new(100, true)],
        )
        .await?;
        c.send(a.reply_mirror(Cause::ACTIVATION_TERM)).await
    }
    async fn asdu(&self, c: &dyn Connect, a: &Asdu) -> rs_iec60870_5::Result<()> {
        assert_eq!(a.get_single_cmd()?.ioa, 500);
        c.send(a.reply_mirror(Cause::ACTIVATION_CON)).await
    }
}
#[cfg(all(unix, feature = "serial"))]
#[tokio::test]
async fn lib60870_101_primary_with_rust_secondary() {
    let Some(bin) = lib_bin() else { return };
    let (bridge, left, right) = serial_bridge().await;
    let server = rs_iec60870_5::cs101::Server::new(SerialOutstation)
        .with_config(serial_config(&left))
        .unwrap();
    server.start().unwrap();
    let peer = GoPeer::spawn(bin, &["client101", &right]).await;
    peer.wait("101 GI termination", |v| {
        v["type"] == 100 && v["cause"] == 10
    })
    .await;
    let command = peer
        .wait("101 command confirmation", |v| {
            v["type"] == 45 && v["cause"] == 7
        })
        .await;
    assert_eq!(command["payload"], serde_json::json!([244, 1, 129]));
    peer.kill().await;
    server.close();
    bridge.kill().await;
}
