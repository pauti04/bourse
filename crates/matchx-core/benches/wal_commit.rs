//! Bench WAL throughput under two commit cadences.
//!
//! - **fsync-per-record** — every `append` is followed by `commit`.
//!   This is the simplest correctness model: the client gets an ack
//!   only after the record is durably on disk, but it pays the full
//!   fsync cost per order. Throughput is bounded by the disk's fsync
//!   latency.
//! - **batched commit** — append `N` records, then call `commit` once.
//!   The whole batch becomes durable together, so per-record
//!   throughput approaches the buffered-write rate. The latency cost
//!   is paid by the *last* record in the batch; the engine ack policy
//!   would acknowledge all `N` after the single fsync.
//!
//! Both write to a temp file in the OS-default temp dir. Linux CI
//! puts that on tmpfs, so the absolute numbers there are
//! representative of an in-memory disk; macOS APFS will show the
//! real fsync cost. Either way the *ratio* is what matters.

#![allow(
    missing_docs,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "criterion macros expand to allocator/print/panic-using code"
)]

use std::fs;
use std::path::PathBuf;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use matchx_core::matcher::{NewOrder, OrderKind};
use matchx_core::types::{OrderId, Price, Qty, Side, Timestamp};
use matchx_core::wal::{WalRecord, WalWriter};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("matchx-wal-bench").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn fresh_wal(dir: &std::path::Path, suffix: &str) -> WalWriter {
    let mut path = dir.to_path_buf();
    path.push(format!("seg-{suffix}.wal"));
    let _ = fs::remove_file(&path);
    WalWriter::create(&path).unwrap()
}

fn rec(id: u64) -> WalRecord {
    WalRecord::NewOrder(NewOrder {
        id: OrderId::new(id),
        side: Side::Buy,
        qty: Qty::new(1),
        kind: OrderKind::Limit {
            price: Price::from_raw(100),
        },
        timestamp: Timestamp::EPOCH,
    })
}

fn bench_fsync_per_record(c: &mut Criterion) {
    let dir = temp_dir("fsync_per_record");
    let mut g = c.benchmark_group("WAL fsync-per-record (one record per commit)");
    for &batch in &[1usize, 8, 64, 256] {
        g.bench_with_input(BenchmarkId::from_parameter(batch), &batch, |b, &batch| {
            let mut counter = 0u64;
            b.iter_batched_ref(
                || fresh_wal(&dir, &format!("per-rec-{batch}")),
                |w| {
                    for _ in 0..batch {
                        counter += 1;
                        w.append(&rec(counter)).unwrap();
                        w.commit().unwrap();
                    }
                },
                BatchSize::SmallInput,
            );
        });
    }
    g.finish();
}

fn bench_group_commit(c: &mut Criterion) {
    let dir = temp_dir("group_commit");
    let mut g = c.benchmark_group("WAL group commit (one commit per batch)");
    for &batch in &[1usize, 8, 64, 256] {
        g.bench_with_input(BenchmarkId::from_parameter(batch), &batch, |b, &batch| {
            let mut counter = 0u64;
            b.iter_batched_ref(
                || fresh_wal(&dir, &format!("group-{batch}")),
                |w| {
                    for _ in 0..batch {
                        counter += 1;
                        w.append(&rec(counter)).unwrap();
                    }
                    w.commit().unwrap();
                },
                BatchSize::SmallInput,
            );
        });
    }
    g.finish();
}

criterion_group!(benches, bench_fsync_per_record, bench_group_commit);
criterion_main!(benches);
