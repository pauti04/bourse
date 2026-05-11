//! End-to-end test: spawn the server on an ephemeral port, connect a
//! client over loopback TCP, exchange a few orders, and verify the
//! server's `ServerMessage` stream matches what the matcher should
//! emit.

#![allow(
    missing_docs,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "integration test"
)]

use bourse_core::matcher::{DoneReason, Event, NewOrder, OrderKind};
use bourse_core::types::{OrderId, Price, Qty, Side, Timestamp};
use bourse_protocol::{ClientMessage, ServerMessage, encode_client};
use bourse_server::{Config, bind, read_one_server_message, serve};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

fn limit(id: u64, side: Side, price: i64, qty: u64) -> ClientMessage {
    ClientMessage::NewOrder(NewOrder {
        id: OrderId::new(id),
        side,
        qty: Qty::new(qty),
        kind: OrderKind::Limit {
            price: Price::from_raw(price),
        },
        timestamp: Timestamp::EPOCH,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_round_trip() {
    let listener = bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, Config::default()));

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.set_nodelay(true).unwrap();

    // Sell rests at 100 qty 5; buy crosses at 100 qty 5 → full fill.
    let mut buf = Vec::with_capacity(128);
    encode_client(&limit(1, Side::Sell, 100, 5), &mut buf);
    encode_client(&limit(2, Side::Buy, 100, 5), &mut buf);
    stream.write_all(&buf).await.unwrap();

    let (mut r, _w) = stream.into_split();
    let mut events = Vec::new();
    // Expected: Accepted(1), Accepted(2), Trade(2,1,100,5),
    // Done(1, Filled), Done(2, Filled). Five frames.
    for _ in 0..5 {
        let m = read_one_server_message(&mut r)
            .await
            .unwrap()
            .expect("eof before all events arrived");
        let ServerMessage::Execution(e) = m;
        events.push(e);
    }

    // The two Accepteds can race in either order with the matcher
    // thread vs. the reader, but the post-Accepted events for id 2
    // (Trade and Done) must follow id-2's Accepted in seq order. The
    // simplest assertion: there is exactly one Trade with the
    // expected fields, and a Done(Filled) for both ids.
    let trades: Vec<_> = events
        .iter()
        .filter_map(|e| match *e {
            Event::Trade {
                taker, maker, qty, ..
            } => Some((taker, maker, qty)),
            _ => None,
        })
        .collect();
    assert_eq!(
        trades,
        vec![(OrderId::new(2), OrderId::new(1), Qty::new(5))]
    );

    let dones: Vec<_> = events
        .iter()
        .filter_map(|e| match *e {
            Event::Done { id, reason, .. } => Some((id, reason)),
            _ => None,
        })
        .collect();
    assert!(dones.contains(&(OrderId::new(1), DoneReason::Filled)));
    assert!(dones.contains(&(OrderId::new(2), DoneReason::Filled)));

    let accepteds = events
        .iter()
        .filter(|e| matches!(e, Event::Accepted { .. }))
        .count();
    assert_eq!(accepteds, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_market_on_empty() {
    let listener = bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, Config::default()));

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.set_nodelay(true).unwrap();

    let market = ClientMessage::NewOrder(NewOrder {
        id: OrderId::new(1),
        side: Side::Buy,
        qty: Qty::new(5),
        kind: OrderKind::Market,
        timestamp: Timestamp::EPOCH,
    });
    let mut buf = Vec::new();
    encode_client(&market, &mut buf);
    stream.write_all(&buf).await.unwrap();

    let (mut r, _w) = stream.into_split();
    // Accepted, Done(NoLiquidity).
    let m1 = read_one_server_message(&mut r)
        .await
        .unwrap()
        .expect("first");
    let m2 = read_one_server_message(&mut r)
        .await
        .unwrap()
        .expect("second");
    assert!(matches!(
        m1,
        ServerMessage::Execution(Event::Accepted { .. })
    ));
    assert!(matches!(
        m2,
        ServerMessage::Execution(Event::Done {
            reason: DoneReason::NoLiquidity,
            ..
        })
    ));
}
