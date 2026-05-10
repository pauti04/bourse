//! matchx replay tool.
//!
//! Reconstructs the book from a snapshot + WAL tail and prints a
//! state summary (resting depth, best bid/ask, a deterministic state
//! hash). Two modes:
//!
//! ```text
//! matchx-replay --wal <path>                       # full WAL replay from empty
//! matchx-replay --snapshot <path> --wal <path>     # snapshot + tail
//! ```
//!
//! In snapshot+tail mode the snapshot's `wal_seq` marker selects
//! which WAL records to replay (skip those with `wal_seq <= marker`).

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "CLI tool prints a summary; unwrap is fine for argv parsing"
)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use matchx_core::matcher::Matcher;
use matchx_core::order_book::Book;
use matchx_core::snapshot;
use matchx_core::types::Sequence;
use matchx_core::wal::{WalRecord, for_each_record};

fn usage_and_exit() -> ! {
    eprintln!("usage:");
    eprintln!("  matchx-replay --wal <path>");
    eprintln!("  matchx-replay --snapshot <path> --wal <path>");
    std::process::exit(2);
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut snap: Option<PathBuf> = None;
    let mut wal: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--snapshot" => snap = args.next().map(PathBuf::from),
            "--wal" => wal = args.next().map(PathBuf::from),
            "-h" | "--help" => usage_and_exit(),
            other => {
                eprintln!("unknown arg: {other}");
                usage_and_exit()
            }
        }
    }
    let Some(wal_path) = wal else {
        usage_and_exit()
    };

    let start = Instant::now();
    let (mut matcher, skip_marker) = match snap {
        Some(p) => {
            let (book, matcher_seq) = snapshot::read(&p).expect("read snapshot");
            let wal_marker = snapshot::read_wal_marker(&p).expect("read snapshot marker");
            println!(
                "loaded snapshot {p:?}: matcher_seq={:?} wal_seq_marker={:?} resting={} bids/asks={:?}/{:?}",
                matcher_seq,
                wal_marker,
                book.len(),
                book.best_bid(),
                book.best_ask(),
            );
            (Matcher::with_book(book, matcher_seq), wal_marker)
        }
        None => (Matcher::new(), Sequence::ZERO),
    };

    let mut events = Vec::with_capacity(16);
    let mut replayed = 0usize;
    let mut last_seq = Sequence::ZERO;
    for_each_record(&wal_path, |seq, rec| {
        if seq <= skip_marker {
            return;
        }
        last_seq = seq;
        match rec {
            WalRecord::NewOrder(no) => matcher.accept(no, &mut events),
            WalRecord::Cancel(id) => matcher.cancel(id, &mut events),
        }
        replayed += 1;
    })
    .expect("replay wal");
    let elapsed = start.elapsed();

    let book = matcher.book();
    let hash = state_hash(book);
    println!();
    println!("recovery summary:");
    println!("  wal:                {wal_path:?}");
    println!("  records replayed:   {replayed}");
    println!("  last wal_seq:       {last_seq:?}");
    println!("  resting orders:     {}", book.len());
    println!("  best bid:           {:?}", book.best_bid());
    println!("  best ask:           {:?}", book.best_ask());
    println!("  state hash:         0x{hash:016x}");
    println!("  wall time:          {elapsed:.2?}");

    ExitCode::SUCCESS
}

/// Order-independent state hash: hash each resting order's tuple
/// independently and XOR the results, so the answer doesn't depend
/// on iteration order. Two books with the same resting orders
/// (same id / side / price / qty / seq) hash to the same number.
fn state_hash(book: &Book) -> u64 {
    let mut acc = 0u64;
    for o in book.iter_resting() {
        let mut h = DefaultHasher::new();
        (
            o.id.get(),
            o.side as u8,
            o.price.raw(),
            o.qty.get(),
            o.seq.get(),
        )
            .hash(&mut h);
        acc ^= h.finish();
    }
    acc
}
