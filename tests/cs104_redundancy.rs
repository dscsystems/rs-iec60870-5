// Copyright (c) 2026 Ricardo Olsen / DSC Systems. All rights reserved.
// Source-available under the DSC Systems Source-Available License; see LICENSE.

//! Redundancy groups over real TCP (IEC 60870-5-104, clause 10): a main and a
//! standby master on one outstation, switchover, the event buffer that covers
//! an outage, group admission by address, and the connection limit.

#![cfg(feature = "cs104")]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::Mutex;

use rs_iec60870_5::asdu::*;
use rs_iec60870_5::cs104::{
    Client, ClientHandler, ClientOption, RedundancyGroup, Server, ServerHandler, ServerMode,
};

struct Outstation;

#[async_trait::async_trait]
impl ServerHandler for Outstation {}

/// Records the IOAs of the single points a master receives, in order.
#[derive(Default)]
struct Received(Mutex<Vec<u32>>);

struct Master(Arc<Received>);

#[async_trait::async_trait]
impl ClientHandler for Master {
    async fn asdu(&self, _c: &dyn Connect, pack: &Asdu) -> rs_iec60870_5::Result<()> {
        if pack.type_id() == TypeId::M_SP_NA_1 {
            let mut got = self.0.0.lock().await;
            got.extend(pack.get_single_point()?.iter().map(|p| p.ioa));
        }
        Ok(())
    }
}

async fn listen(srv: &Arc<Server<Outstation>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let srv = Arc::clone(srv);
    tokio::spawn(async move { srv.serve(listener).await });
    addr
}

fn master(addr: &str, auto_start: bool) -> (Arc<Client<Master>>, Arc<Received>) {
    let got = Arc::new(Received::default());
    let option = ClientOption::new()
        .with_server(addr)
        .unwrap()
        .with_auto_start_dt(auto_start);
    let cli = Client::new(Master(Arc::clone(&got)), option);
    cli.start().unwrap();
    (cli, got)
}

async fn eventually(label: &str, mut f: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for: {label}");
}

async fn publish(srv: &Arc<Server<Outstation>>, ioa: u32) -> rs_iec60870_5::Result<()> {
    srv.send_single(
        false,
        CauseOfTransmission::new(Cause::SPONTANEOUS),
        1,
        &[SinglePointInfo::new(ioa, true)],
    )
    .await
}

async fn received(r: &Received) -> Vec<u32> {
    r.0.lock().await.clone()
}

#[tokio::test]
async fn only_the_started_master_of_a_group_is_sent_data_and_a_switchover_moves_it() {
    let srv = Server::new(Outstation).with_mode(ServerMode::SingleRedundancyGroup);
    let addr = listen(&srv).await;

    let (main, main_got) = master(&addr, true);
    let (standby, standby_got) = master(&addr, false);
    tokio::time::timeout(Duration::from_secs(5), main.wait_active())
        .await
        .expect("main did not start");
    eventually("both connected", async || srv.session_count() == 2).await;

    publish(&srv, 1).await.unwrap();
    eventually("main received 1", async || received(&main_got).await == [1]).await;

    // The control centre switches over by starting the standby.
    standby.connection().await.unwrap().send_start_dt();
    tokio::time::timeout(Duration::from_secs(5), standby.wait_active())
        .await
        .expect("standby did not start");

    publish(&srv, 2).await.unwrap();
    eventually("standby received 2", async || received(&standby_got).await == [2]).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        received(&main_got).await,
        [1],
        "the previous master is out of data transfer and must not be sent more"
    );

    main.close();
    standby.close();
}

#[tokio::test]
async fn events_raised_during_an_outage_are_replayed_when_a_master_starts() {
    let srv = Server::new(Outstation)
        .with_mode(ServerMode::SingleRedundancyGroup)
        .with_event_buffer(10);
    let addr = listen(&srv).await;

    // Nobody connected: the group keeps the events rather than losing them.
    for ioa in 1..=3 {
        publish(&srv, ioa).await.unwrap();
    }
    assert_eq!(srv.buffered_count(), 3);

    let (cli, got) = master(&addr, true);
    eventually("the backlog arrives in order", async || {
        received(&got).await == [1, 2, 3]
    })
    .await;
    assert_eq!(srv.buffered_count(), 0);

    // And live data follows it.
    publish(&srv, 4).await.unwrap();
    eventually("live data follows", async || received(&got).await == [1, 2, 3, 4]).await;
    cli.close();
}

#[tokio::test]
async fn a_full_event_buffer_refuses_rather_than_overwrites() {
    let srv = Server::new(Outstation)
        .with_mode(ServerMode::SingleRedundancyGroup)
        .with_event_buffer(2);
    publish(&srv, 1).await.unwrap();
    publish(&srv, 2).await.unwrap();
    assert_eq!(
        publish(&srv, 3).await,
        Err(rs_iec60870_5::Error::SendQueueFull),
        "the loss must be reported"
    );
    assert_eq!(srv.buffered_count(), 2);
}

#[tokio::test]
async fn a_master_outside_every_group_is_refused() {
    let elsewhere = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    let srv = Server::new(Outstation).with_mode(ServerMode::MultipleRedundancyGroups(vec![
        RedundancyGroup::new("control-centre").with_client(elsewhere),
    ]));
    let addr = listen(&srv).await;

    // The test connects from 127.0.0.1, which no group admits.
    let (cli, _) = master(&addr, true);
    assert!(
        tokio::time::timeout(Duration::from_millis(800), cli.wait_active())
            .await
            .is_err(),
        "a refused master must not reach data transfer"
    );
    assert_eq!(srv.session_count(), 0);
    cli.close();
}

#[tokio::test]
async fn groups_by_address_each_have_their_own_started_master() {
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let srv = Server::new(Outstation).with_mode(ServerMode::MultipleRedundancyGroups(vec![
        RedundancyGroup::new("local").with_client(loopback),
        RedundancyGroup::new("rest"),
    ]));
    let addr = listen(&srv).await;

    // Two masters from the same address share the "local" group: only one is
    // in data transfer, exactly as in a single group.
    let (a, a_got) = master(&addr, true);
    tokio::time::timeout(Duration::from_secs(5), a.wait_active())
        .await
        .unwrap();
    let (b, b_got) = master(&addr, false);
    eventually("both connected", async || srv.session_count() == 2).await;

    publish(&srv, 7).await.unwrap();
    eventually("a received it", async || received(&a_got).await == [7]).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(received(&b_got).await.is_empty());
    a.close();
    b.close();
}

#[tokio::test]
async fn masters_beyond_the_connection_limit_are_refused() {
    let srv = Server::new(Outstation).with_max_connections(1);
    let addr = listen(&srv).await;

    let (first, _) = master(&addr, true);
    tokio::time::timeout(Duration::from_secs(5), first.wait_active())
        .await
        .unwrap();

    let (second, _) = master(&addr, true);
    assert!(
        tokio::time::timeout(Duration::from_millis(800), second.wait_active())
            .await
            .is_err(),
        "the second master got past the limit"
    );
    assert_eq!(srv.session_count(), 1);
    first.close();
    second.close();
}

#[tokio::test]
async fn by_default_every_started_master_receives_every_broadcast() {
    // Independent masters — not a redundant pair — each get all the data.
    let srv = Server::new(Outstation);
    let addr = listen(&srv).await;
    let (a, a_got) = master(&addr, true);
    let (b, b_got) = master(&addr, true);
    for c in [&a, &b] {
        tokio::time::timeout(Duration::from_secs(5), c.wait_active())
            .await
            .unwrap();
    }
    eventually("both connected", async || srv.session_count() == 2).await;

    publish(&srv, 5).await.unwrap();
    eventually("both received it", async || {
        received(&a_got).await == [5] && received(&b_got).await == [5]
    })
    .await;
    a.close();
    b.close();
}
