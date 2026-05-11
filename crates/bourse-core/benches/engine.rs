//! End-to-end latency: gateway pushes a Command onto the SPSC, the
//! matcher thread processes it, and the bench thread spins until it
//! observes the corresponding `Done`.
//!
//! Two scenarios:
//! - empty book + Market: minimal matcher work; measures pure pipeline
//!   overhead (queue push, wake matcher, queue pop, push event back,
//!   pop event).
//! - full cross of a single resting Limit: same overhead plus one
//!   Trade and one maker `Done`.

#![allow(
    missing_docs,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "criterion macros expand to allocator/print/panic-using code"
)]

use std::hint::black_box;

use bourse_core::engine::{Command, Engine};
use bourse_core::matcher::{DoneReason, Event, NewOrder, OrderKind};
use bourse_core::types::{OrderId, Price, Qty, Side, Timestamp};
use criterion::{Criterion, criterion_group, criterion_main};

fn market(id: u64) -> Command {
    Command::New(NewOrder {
        id: OrderId::new(id),
        side: Side::Buy,
        qty: Qty::new(1),
        kind: OrderKind::Market,
        timestamp: Timestamp::EPOCH,
    })
}

fn limit_sell(id: u64, price: i64, qty: u64) -> Command {
    Command::New(NewOrder {
        id: OrderId::new(id),
        side: Side::Sell,
        qty: Qty::new(qty),
        kind: OrderKind::Limit {
            price: Price::from_raw(price),
        },
        timestamp: Timestamp::EPOCH,
    })
}

fn limit_buy(id: u64, price: i64, qty: u64) -> Command {
    Command::New(NewOrder {
        id: OrderId::new(id),
        side: Side::Buy,
        qty: Qty::new(qty),
        kind: OrderKind::Limit {
            price: Price::from_raw(price),
        },
        timestamp: Timestamp::EPOCH,
    })
}

fn push_blocking(engine: &mut Engine, mut c: Command) {
    while let Err(returned) = engine.input().try_push(c) {
        c = returned;
        std::hint::spin_loop();
    }
}

fn wait_for_done(engine: &mut Engine, expected: OrderId) {
    loop {
        if let Some(e) = engine.events().try_pop()
            && let Event::Done {
                id,
                reason: DoneReason::Filled | DoneReason::NoLiquidity | DoneReason::Expired,
                ..
            } = e
            && id == expected
        {
            return;
        }
        std::hint::spin_loop();
    }
}

fn bench_market_on_empty(c: &mut Criterion) {
    c.bench_function(
        "Engine round-trip: Market on empty book (Done(NoLiquidity))",
        |b| {
            let mut engine = Engine::start(1024, 1024);
            let mut id = 0u64;
            b.iter(|| {
                id += 1;
                let oid = OrderId::new(id);
                push_blocking(&mut engine, market(id));
                wait_for_done(&mut engine, black_box(oid));
            });
            let _ = engine.stop();
        },
    );
}

fn bench_limit_full_cross(c: &mut Criterion) {
    c.bench_function(
        "Engine round-trip: Limit fully fills against 1 resting maker",
        |b| {
            let mut engine = Engine::start(1024, 1024);
            let mut id = 1u64;
            b.iter(|| {
                let maker = id;
                id += 1;
                let taker = id;
                id += 1;
                push_blocking(&mut engine, limit_sell(maker, 100, 1));
                push_blocking(&mut engine, limit_buy(taker, 100, 1));
                wait_for_done(&mut engine, black_box(OrderId::new(taker)));
            });
            let _ = engine.stop();
        },
    );
}

criterion_group!(benches, bench_market_on_empty, bench_limit_full_cross);
criterion_main!(benches);
