//! Bench `Matcher::accept`:
//! - The no-cross path (incoming Limit just rests).
//! - The crossing path, where the taker walks N price levels.

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

use bourse_core::matcher::{Event, Matcher, NewOrder, OrderKind};
use bourse_core::types::{OrderId, Price, Qty, Side, Timestamp};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

fn matcher_with_asks(n: usize, base_price: i64) -> Matcher {
    let mut m = Matcher::new();
    let mut ev = Vec::with_capacity(8);
    for i in 0..n {
        ev.clear();
        m.accept(
            NewOrder {
                id: OrderId::new(i as u64),
                side: Side::Sell,
                qty: Qty::new(1),
                kind: OrderKind::Limit {
                    price: Price::from_raw(base_price + i as i64),
                },
                timestamp: Timestamp::EPOCH,
            },
            &mut ev,
        );
    }
    m
}

fn bench_accept_no_cross(c: &mut Criterion) {
    let mut g = c.benchmark_group("Matcher::accept (Limit, no cross)");
    for &depth in &[0usize, 100, 1_000] {
        g.bench_with_input(BenchmarkId::from_parameter(depth), &depth, |b, &depth| {
            let mut events: Vec<Event> = Vec::with_capacity(8);
            b.iter_batched_ref(
                || matcher_with_asks(depth, 200),
                |m| {
                    events.clear();
                    m.accept(
                        black_box(NewOrder {
                            id: OrderId::new(2_000_000),
                            side: Side::Buy,
                            qty: Qty::new(1),
                            kind: OrderKind::Limit {
                                price: Price::from_raw(100),
                            },
                            timestamp: Timestamp::EPOCH,
                        }),
                        &mut events,
                    );
                },
                BatchSize::SmallInput,
            );
        });
    }
    g.finish();
}

fn bench_accept_walks_n_levels(c: &mut Criterion) {
    let mut g = c.benchmark_group("Matcher::accept (Limit walks N levels, full fill)");
    for &n in &[1usize, 10, 100, 1_000] {
        g.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let mut events: Vec<Event> = Vec::with_capacity(n * 4);
            b.iter_batched_ref(
                || matcher_with_asks(n, 100),
                |m| {
                    events.clear();
                    m.accept(
                        black_box(NewOrder {
                            id: OrderId::new(2_000_000),
                            side: Side::Buy,
                            qty: Qty::new(n as u64),
                            kind: OrderKind::Limit {
                                price: Price::from_raw(100 + n as i64),
                            },
                            timestamp: Timestamp::EPOCH,
                        }),
                        &mut events,
                    );
                },
                BatchSize::SmallInput,
            );
        });
    }
    g.finish();
}

criterion_group!(benches, bench_accept_no_cross, bench_accept_walks_n_levels);
criterion_main!(benches);
