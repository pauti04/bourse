//! Basic matcher example — drives `Matcher` directly without TCP.
//!
//! Runnable: `cargo run --release -p matchx-core --example basic_match`
//!
//! Submits a Sell order at price 100, then a Buy at 100 that fills it,
//! then a Buy at 99 that doesn't cross. Prints every event the matcher
//! emits.

#![allow(clippy::print_stdout, reason = "example binary prints to stdout")]

use matchx_core::matcher::{Event, Matcher, NewOrder, OrderKind};
use matchx_core::types::{OrderId, Price, Qty, Side, Timestamp};

fn main() {
    let mut m = Matcher::new();
    let mut events: Vec<Event> = Vec::with_capacity(8);

    println!("== submit Sell 5 @ 100 (should rest) ==");
    submit(
        &mut m,
        &mut events,
        NewOrder {
            id: OrderId::new(1),
            side: Side::Sell,
            qty: Qty::new(5),
            kind: OrderKind::Limit {
                price: Price::from_raw(100),
            },
            timestamp: Timestamp::EPOCH,
        },
    );

    println!("\n== submit Buy 5 @ 100 (should cross and fill) ==");
    submit(
        &mut m,
        &mut events,
        NewOrder {
            id: OrderId::new(2),
            side: Side::Buy,
            qty: Qty::new(5),
            kind: OrderKind::Limit {
                price: Price::from_raw(100),
            },
            timestamp: Timestamp::EPOCH,
        },
    );

    println!("\n== submit Buy 3 @ 99 (should rest, no cross) ==");
    submit(
        &mut m,
        &mut events,
        NewOrder {
            id: OrderId::new(3),
            side: Side::Buy,
            qty: Qty::new(3),
            kind: OrderKind::Limit {
                price: Price::from_raw(99),
            },
            timestamp: Timestamp::EPOCH,
        },
    );

    println!("\n== final book ==");
    println!("  best bid: {:?}", m.book().best_bid());
    println!("  best ask: {:?}", m.book().best_ask());
    println!("  resting:  {} orders", m.book().len());
}

fn submit(m: &mut Matcher, events: &mut Vec<Event>, order: NewOrder) {
    events.clear();
    m.accept(order, events);
    for e in events.iter() {
        println!("  emitted: {e:?}");
    }
}
