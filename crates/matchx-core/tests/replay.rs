//! Slice 3 headline test.
//!
//! Generate 10k random commands. Run them through `Matcher_live` while
//! logging every input to a WAL. Open a fresh `Matcher_replayed`, feed
//! it the WAL. Assert that the two books and the two event streams are
//! byte-equal.

#![allow(
    missing_docs,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "integration test"
)]

use std::fs;
use std::path::PathBuf;

use matchx_core::matcher::{Event, Matcher, NewOrder, OrderKind};
use matchx_core::types::{OrderId, Price, Qty, Side, Timestamp};
use matchx_core::wal::{WalRecord, WalWriter, for_each_record};

/// Deterministic splitmix64 — small, no dep, reproducible across runs.
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
            // Keep prices in a narrow band so orders actually cross.
            let price = Price::from_raw(rng.range(95, 106) as i64);
            let kind = match rng.next() % 100 {
                0..=69 => OrderKind::Limit { price },
                70..=84 => OrderKind::Market,
                _ => OrderKind::Ioc { price },
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
    let dir = std::env::temp_dir().join("matchx-replay-test").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn replay_byte_equal_to_live_run() {
    let dir = temp_dir("replay_byte_equal_to_live_run");
    let wal_path = dir.join("seg-0.wal");
    let cmds = generate(0xDEAD_BEEF_CAFE_F00D, 10_000);

    // Live: write inputs to WAL, fsync, then drive the matcher.
    let mut wal = WalWriter::create(&wal_path).unwrap();
    let mut live = Matcher::new();
    let mut live_events: Vec<Event> = Vec::with_capacity(cmds.len() * 4);
    for c in &cmds {
        match *c {
            Cmd::New(no) => {
                wal.append(&WalRecord::NewOrder(no)).unwrap();
                wal.commit().unwrap();
                live.accept(no, &mut live_events);
            }
            Cmd::Cancel(id) => {
                wal.append(&WalRecord::Cancel(id)).unwrap();
                wal.commit().unwrap();
                live.cancel(id, &mut live_events);
            }
        }
    }
    drop(wal);

    // Replay: read WAL into a fresh matcher.
    let mut replayed = Matcher::new();
    let mut replayed_events: Vec<Event> = Vec::with_capacity(live_events.len());
    for_each_record(&wal_path, |_seq, rec| match rec {
        WalRecord::NewOrder(no) => replayed.accept(no, &mut replayed_events),
        WalRecord::Cancel(id) => replayed.cancel(id, &mut replayed_events),
    })
    .unwrap();

    assert_eq!(live.book(), replayed.book(), "book state diverged");
    assert_eq!(
        live_events,
        replayed_events,
        "event streams diverged ({} vs {} events)",
        live_events.len(),
        replayed_events.len()
    );

    // Sanity: this workload actually exercises matching.
    let trades = live_events
        .iter()
        .filter(|e| matches!(e, Event::Trade { .. }))
        .count();
    assert!(
        trades > 100,
        "expected >100 trades from 10k random orders, got {trades}"
    );
}
