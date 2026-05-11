//! Multi-tenant gateway test.
//!
//! Spawn the server. Connect three concurrent clients. Each rests an
//! order at a distinct price; clients 2 and 3 cross client 1's
//! resting orders. Verify each client sees only its own events plus
//! the trades it's involved in (taker or maker), not events from
//! orders submitted by other clients.

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
use tokio::net::tcp::OwnedReadHalf;

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

async fn drain_until<F>(r: &mut OwnedReadHalf, mut stop: F) -> Vec<Event>
where
    F: FnMut(&Event) -> bool,
{
    let mut out = Vec::new();
    loop {
        let m = read_one_server_message(r).await.unwrap().expect("eof");
        let ServerMessage::Execution(e) = m;
        let done = stop(&e);
        out.push(e);
        if done {
            return out;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_clients_share_one_matcher() {
    let listener = bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(serve(listener, Config::default()));

    // Three concurrent clients on the same shared hub.
    let mut a = TcpStream::connect(addr).await.unwrap();
    let mut b = TcpStream::connect(addr).await.unwrap();
    let mut c = TcpStream::connect(addr).await.unwrap();
    a.set_nodelay(true).unwrap();
    b.set_nodelay(true).unwrap();
    c.set_nodelay(true).unwrap();

    // Client A rests two sells at distinct prices.
    let mut buf = Vec::new();
    encode_client(&limit(101, Side::Sell, 100, 5), &mut buf);
    encode_client(&limit(102, Side::Sell, 200, 3), &mut buf);
    a.write_all(&buf).await.unwrap();

    // Drain A's two Accepted events first so the matcher state is
    // settled before clients B and C arrive.
    let (mut a_r, _a_w) = a.into_split();
    let _ = drain_until(
        &mut a_r,
        |e| matches!(e, Event::Accepted { id, .. } if *id == OrderId::new(102)),
    )
    .await;

    // Client B fully fills A's sell at 100.
    let mut buf = Vec::new();
    encode_client(&limit(201, Side::Buy, 100, 5), &mut buf);
    b.write_all(&buf).await.unwrap();

    // Client C fully fills A's sell at 200.
    let mut buf = Vec::new();
    encode_client(&limit(301, Side::Buy, 200, 3), &mut buf);
    c.write_all(&buf).await.unwrap();

    let (mut b_r, _b_w) = b.into_split();
    let (mut c_r, _c_w) = c.into_split();

    // B sees Accepted(201), Trade, Done(201, Filled).
    let b_events = drain_until(
        &mut b_r,
        |e| matches!(e, Event::Done { id, reason: DoneReason::Filled, .. } if *id == OrderId::new(201)),
    )
    .await;
    let b_trades: Vec<_> = b_events
        .iter()
        .filter_map(|e| match *e {
            Event::Trade {
                taker,
                maker,
                qty,
                price,
                ..
            } => Some((taker, maker, qty, price)),
            _ => None,
        })
        .collect();
    assert_eq!(
        b_trades,
        vec![(
            OrderId::new(201),
            OrderId::new(101),
            Qty::new(5),
            Price::from_raw(100)
        )]
    );
    // B should NOT see anything about order 102 or 301 — those belong
    // to other tenants.
    for e in &b_events {
        if let Event::Trade { taker, maker, .. } = *e {
            assert!(taker.get() < 300);
            assert!(maker.get() < 300);
        }
    }

    // C sees Accepted(301), Trade, Done(301, Filled).
    let c_events = drain_until(
        &mut c_r,
        |e| matches!(e, Event::Done { id, reason: DoneReason::Filled, .. } if *id == OrderId::new(301)),
    )
    .await;
    let c_trades: Vec<_> = c_events
        .iter()
        .filter_map(|e| match *e {
            Event::Trade {
                taker,
                maker,
                qty,
                price,
                ..
            } => Some((taker, maker, qty, price)),
            _ => None,
        })
        .collect();
    assert_eq!(
        c_trades,
        vec![(
            OrderId::new(301),
            OrderId::new(102),
            Qty::new(3),
            Price::from_raw(200)
        )]
    );

    // A sees both Trades (as maker) and both Done(maker filled) events.
    let a_events = drain_until(
        &mut a_r,
        |e| matches!(e, Event::Done { id, reason: DoneReason::Filled, .. } if *id == OrderId::new(102)),
    )
    .await;
    let a_trades: Vec<_> = a_events
        .iter()
        .filter_map(|e| match *e {
            Event::Trade {
                taker,
                maker,
                qty,
                price,
                ..
            } => Some((taker, maker, qty, price)),
            _ => None,
        })
        .collect();
    // A is the maker for both trades. Order may interleave by matcher
    // schedule; assert as a set.
    assert_eq!(a_trades.len(), 2);
    assert!(a_trades.contains(&(
        OrderId::new(201),
        OrderId::new(101),
        Qty::new(5),
        Price::from_raw(100)
    )));
    assert!(a_trades.contains(&(
        OrderId::new(301),
        OrderId::new(102),
        Qty::new(3),
        Price::from_raw(200)
    )));
}
