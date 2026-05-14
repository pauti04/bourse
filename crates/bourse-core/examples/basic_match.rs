//! Basic matcher example — drives `Matcher` directly without TCP.
//!
//! Runnable: `cargo run --release -p bourse-core --example basic_match`
//!
//! Walks every order kind:
//!   1. Sell 5 @ 100 (rests; Accepted only)
//!   2. Buy 5 @ 100 (crosses; Accepted, Trade, Done(Filled) ×2)
//!   3. Buy 3 @ 99  (rests, no cross)
//!   4. Sell 4 @ 99 PostOnly  (would cross resting buy at 99 → Rejected)
//!   5. Sell 2 @ 100 PostOnly (does not cross → rests)
//!   6. Buy 10 @ 100 FOK      (only 2 available → Rejected, book unchanged)
//!   7. Buy 2 @ 100 FOK       (exactly 2 available → fully fills)

#![allow(clippy::print_stdout, reason = "example binary prints to stdout")]

use bourse_core::matcher::{Event, Matcher, NewOrder, OrderKind};
use bourse_core::types::{OrderId, Price, Qty, Side, Timestamp};

fn main() {
    let mut m = Matcher::new();
    let mut events: Vec<Event> = Vec::with_capacity(8);

    println!("== 1. Sell 5 @ 100 (should rest) ==");
    submit(&mut m, &mut events, limit(1, Side::Sell, 100, 5));

    println!("\n== 2. Buy 5 @ 100 (should cross and fill) ==");
    submit(&mut m, &mut events, limit(2, Side::Buy, 100, 5));

    println!("\n== 3. Buy 3 @ 99 (should rest, no cross) ==");
    submit(&mut m, &mut events, limit(3, Side::Buy, 99, 3));

    println!("\n== 4. Sell 4 @ 99 PostOnly (would cross bid at 99 → Rejected) ==");
    submit(&mut m, &mut events, post_only(4, Side::Sell, 99, 4));

    println!("\n== 5. Sell 2 @ 100 PostOnly (no cross → rests) ==");
    submit(&mut m, &mut events, post_only(5, Side::Sell, 100, 2));

    println!("\n== 6. Buy 10 @ 100 FOK (only 2 @ <=100 → Rejected, book intact) ==");
    submit(&mut m, &mut events, fok(6, Side::Buy, 100, 10));

    println!("\n== 7. Buy 2 @ 100 FOK (exactly 2 @ 100 → fully fills) ==");
    submit(&mut m, &mut events, fok(7, Side::Buy, 100, 2));

    println!("\n== final book ==");
    println!("  best bid: {:?}", m.book().best_bid());
    println!("  best ask: {:?}", m.book().best_ask());
    println!("  resting:  {} orders", m.book().len());
}

fn limit(id: u64, side: Side, price: i64, qty: u64) -> NewOrder {
    NewOrder {
        id: OrderId::new(id),
        side,
        qty: Qty::new(qty),
        kind: OrderKind::Limit {
            price: Price::from_raw(price),
        },
        timestamp: Timestamp::EPOCH,
    }
}

fn post_only(id: u64, side: Side, price: i64, qty: u64) -> NewOrder {
    NewOrder {
        id: OrderId::new(id),
        side,
        qty: Qty::new(qty),
        kind: OrderKind::PostOnly {
            price: Price::from_raw(price),
        },
        timestamp: Timestamp::EPOCH,
    }
}

fn fok(id: u64, side: Side, price: i64, qty: u64) -> NewOrder {
    NewOrder {
        id: OrderId::new(id),
        side,
        qty: Qty::new(qty),
        kind: OrderKind::Fok {
            price: Price::from_raw(price),
        },
        timestamp: Timestamp::EPOCH,
    }
}

fn submit(m: &mut Matcher, events: &mut Vec<Event>, order: NewOrder) {
    events.clear();
    m.accept(order, events);
    for e in events.iter() {
        println!("  emitted: {e:?}");
    }
}
