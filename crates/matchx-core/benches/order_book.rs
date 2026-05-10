//! Bench `Book::add` and `Book::cancel` at increasing book depths.
//!
//! Cancel is exercised at the *front* of a single price level — that's the
//! VecDeque worst case (linear shift), so the numbers reflect the upper
//! bound on cancel cost.

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

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use matchx_core::order_book::Book;
use matchx_core::types::{OrderId, Price, Qty, Sequence, Side};

const PRICE: i64 = 100_000_000; // 1.00000000

fn prefill_single_level(n: usize) -> Book {
    let mut book = Book::new();
    for i in 0..n {
        book.add(
            OrderId::new(i as u64),
            Side::Buy,
            Price::from_raw(PRICE),
            Qty::new(1),
            Sequence::from_raw(i as u64 + 1),
        );
    }
    book
}

fn bench_add(c: &mut Criterion) {
    let mut g = c.benchmark_group("Book::add (single level)");
    for &depth in &[0usize, 100, 1_000, 10_000] {
        g.bench_with_input(BenchmarkId::from_parameter(depth), &depth, |b, &depth| {
            b.iter_batched_ref(
                || prefill_single_level(depth),
                |book| {
                    book.add(
                        black_box(OrderId::new(depth as u64 + 1)),
                        Side::Buy,
                        Price::from_raw(PRICE),
                        Qty::new(1),
                        Sequence::from_raw(depth as u64 + 1),
                    );
                },
                BatchSize::SmallInput,
            );
        });
    }
    g.finish();
}

fn bench_cancel_front(c: &mut Criterion) {
    let mut g = c.benchmark_group("Book::cancel (front of single level)");
    for &depth in &[1usize, 100, 1_000, 10_000] {
        g.bench_with_input(BenchmarkId::from_parameter(depth), &depth, |b, &depth| {
            b.iter_batched_ref(
                || prefill_single_level(depth),
                |book| {
                    book.cancel(black_box(OrderId::new(0)));
                },
                BatchSize::SmallInput,
            );
        });
    }
    g.finish();
}

criterion_group!(benches, bench_add, bench_cancel_front);
criterion_main!(benches);
