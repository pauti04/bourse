//! bourse replay tool.
//!
//! Reconstructs the book from a snapshot + WAL tail and prints a
//! state summary (resting depth, best bid/ask, a deterministic state
//! hash). Two modes:
//!
//! ```text
//! bourse-replay --wal <path>                       # full WAL replay from empty
//! bourse-replay --snapshot <path> --wal <path>     # snapshot + tail
//! ```
//!
//! In snapshot+tail mode the snapshot's `wal_seq` marker selects
//! which WAL records to replay (skip those with `wal_seq <= marker`).
//!
//! Logging via `tracing`. Set `RUST_LOG` to control verbosity.

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

use bourse_core::matcher::Matcher;
use bourse_core::order_book::Book;
use bourse_core::snapshot;
use bourse_core::types::Sequence;
use bourse_core::wal::{WalRecord, for_each_record};
use tracing_subscriber::EnvFilter;

fn usage_and_exit() -> ! {
    eprintln!("usage:");
    eprintln!("  bourse-replay --wal <path>");
    eprintln!("  bourse-replay --snapshot <path> --wal <path>");
    std::process::exit(2);
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

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
            tracing::info!(
                path = %p.display(),
                ?matcher_seq,
                ?wal_marker,
                resting = book.len(),
                best_bid = ?book.best_bid(),
                best_ask = ?book.best_ask(),
                "snapshot loaded"
            );
            (Matcher::with_book(book, matcher_seq), wal_marker)
        }
        None => {
            tracing::info!("no snapshot — starting from empty matcher");
            (Matcher::new(), Sequence::ZERO)
        }
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
        if replayed.is_multiple_of(50_000) {
            tracing::info!(records_replayed = replayed, ?last_seq, "progress");
        }
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
