//! Single-threaded SPSC push/pop micro-bench.
//!
//! Threaded throughput lives in the test under `cfg(not(miri))`; this
//! bench is the per-call cost of `try_push` and `try_pop` in tight
//! steady-state, with cached head/tail hot.

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

use criterion::{Criterion, criterion_group, criterion_main};
use matchx_core::spsc::channel;

fn push_then_pop_batch(c: &mut Criterion) {
    c.bench_function("Spsc push+pop batch=256, cap=1024", |b| {
        let (mut tx, mut rx) = channel::<u64>(1024);
        b.iter(|| {
            for i in 0..256u64 {
                tx.try_push(black_box(i)).unwrap();
            }
            for _ in 0..256u64 {
                let _ = black_box(rx.try_pop());
            }
        });
    });
}

criterion_group!(benches, push_then_pop_batch);
criterion_main!(benches);
