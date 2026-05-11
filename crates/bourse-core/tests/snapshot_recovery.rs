//! Snapshot + WAL recovery test.
//!
//! Run 5000 random orders through a live matcher (logging every input
//! to a WAL). Snapshot the book at the midpoint. Continue with another
//! 5000 orders. Then recover from `(snapshot, WAL_tail)`: load the
//! snapshot, then replay only the WAL records past the snapshot's
//! sequence marker. Assert the reconstructed book equals the live
//! book — bit for bit.
//!
//! Also reports recovery wall-clock vs. full WAL replay, since the
//! charter cares about bounded recovery time.

#![allow(
    missing_docs,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    reason = "integration test that prints recovery timings"
)]

use std::fs;
use std::path::PathBuf;
use std::time::Instant;

use bourse_core::matcher::{Matcher, NewOrder, OrderKind};
use bourse_core::snapshot;
use bourse_core::types::{OrderId, Price, Qty, Side, Timestamp};
use bourse_core::wal::{WalRecord, WalWriter, for_each_record};

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo)
    }
}

#[derive(Debug, Clone, Copy)]
enum Cmd {
    New(NewOrder),
    Cancel(OrderId),
}

fn generate(seed: u64, n: usize) -> Vec<Cmd> {
    let mut rng = Rng::new(seed);
    let mut cmds = Vec::with_capacity(n);
    let mut next_id = 1u64;
    let mut active: Vec<u64> = Vec::new();
    for _ in 0..n {
        let r = rng.next() % 100;
        if r < 5 && !active.is_empty() {
            let i = (rng.next() as usize) % active.len();
            let id = active.swap_remove(i);
            cmds.push(Cmd::Cancel(OrderId::new(id)));
        } else {
            let id = next_id;
            next_id += 1;
            active.push(id);
            let side = if rng.next().is_multiple_of(2) {
                Side::Buy
            } else {
                Side::Sell
            };
            let qty = rng.range(1, 10);
            let price = Price::from_raw(rng.range(95, 106) as i64);
            let kind = match rng.next() % 100 {
                0..=54 => OrderKind::Limit { price },
                55..=69 => OrderKind::Market,
                70..=84 => OrderKind::Ioc { price },
                85..=92 => OrderKind::PostOnly { price },
                _ => OrderKind::Fok { price },
            };
            cmds.push(Cmd::New(NewOrder {
                id: OrderId::new(id),
                side,
                qty: Qty::new(qty),
                kind,
                timestamp: Timestamp::EPOCH,
            }));
        }
    }
    cmds
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("bourse-snap-recovery").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn apply(
    matcher: &mut Matcher,
    cmds: &[Cmd],
    wal: &mut WalWriter,
    events: &mut Vec<bourse_core::matcher::Event>,
) {
    for c in cmds {
        match *c {
            Cmd::New(no) => {
                let _ = wal.append(&WalRecord::NewOrder(no)).unwrap();
                wal.commit().unwrap();
                matcher.accept(no, events);
            }
            Cmd::Cancel(id) => {
                let _ = wal.append(&WalRecord::Cancel(id)).unwrap();
                wal.commit().unwrap();
                matcher.cancel(id, events);
            }
        }
    }
}

#[test]
fn snapshot_plus_wal_tail_reconstructs_live() {
    let dir = temp_dir("snapshot_plus_wal_tail_reconstructs_live");
    let wal_path = dir.join("seg-0.wal");
    let snap_path = dir.join("midpoint.snap");

    let cmds = generate(0xCAFE_F00D_DEAD_BEEF, 10_000);
    let (first, second) = cmds.split_at(5_000);

    let mut wal = WalWriter::create(&wal_path).unwrap();
    let mut live = Matcher::new();
    let mut events = Vec::with_capacity(8);

    // Phase 1: first half of commands.
    apply(&mut live, first, &mut wal, &mut events);

    // Snapshot at the midpoint. The matcher marker is the matcher's
    // next-seq, used to seed the recovered SequenceGenerator. The
    // WAL marker is `next_seq() - 1`, the last record durably
    // included — recovery skips records with `wal_seq <= wal_marker`.
    let matcher_marker = live.peek_seq();
    let wal_marker = bourse_core::types::Sequence::from_raw(wal.next_seq().get() - 1);
    snapshot::write(live.book(), matcher_marker, wal_marker, &snap_path).unwrap();

    // Phase 2: second half. WAL keeps growing.
    apply(&mut live, second, &mut wal, &mut events);
    drop(wal);

    // Recovery: snapshot first, then the WAL tail.
    let recovery_start = Instant::now();
    let (recovered_book, marker_at_load) = snapshot::read(&snap_path).unwrap();
    let mut recovered = Matcher::with_book(recovered_book, marker_at_load);
    // Skip-by-seq: each WAL record carries its `wal_seq`. Replay only
    // those past the snapshot's wal-seq marker. No out-of-band counting
    // — the snapshot file has the marker, the WAL has the seqs.
    let snap_wal_seq = snapshot::read_wal_marker(&snap_path).unwrap();
    let mut replayed = 0usize;
    for_each_record(&wal_path, |seq, rec| {
        if seq <= snap_wal_seq {
            return;
        }
        match rec {
            WalRecord::NewOrder(no) => recovered.accept(no, &mut events),
            WalRecord::Cancel(id) => recovered.cancel(id, &mut events),
        }
        replayed += 1;
    })
    .unwrap();
    let recovery_time = recovery_start.elapsed();

    assert_eq!(
        live.book(),
        recovered.book(),
        "recovered book diverged from live"
    );
    assert_eq!(replayed, second.len());

    // Compare against full WAL replay (no snapshot) on the same data,
    // for a recovery-time delta.
    let full_replay_start = Instant::now();
    let mut full = Matcher::new();
    let mut count = 0usize;
    for_each_record(&wal_path, |_seq, rec| {
        match rec {
            WalRecord::NewOrder(no) => full.accept(no, &mut events),
            WalRecord::Cancel(id) => full.cancel(id, &mut events),
        }
        count += 1;
    })
    .unwrap();
    let full_replay_time = full_replay_start.elapsed();
    assert_eq!(count, cmds.len());
    assert_eq!(live.book(), full.book());

    println!(
        "snapshot markers: matcher={:?} wal={:?}; recovery from \
         (snapshot + tail): {:?} ({} WAL records replayed); full WAL \
         replay: {:?} ({} records)",
        marker_at_load, snap_wal_seq, recovery_time, replayed, full_replay_time, count
    );
}
