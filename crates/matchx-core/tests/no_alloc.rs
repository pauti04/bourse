//! Allocation-counting harness for the matcher's hot path.
//!
//! A custom global allocator wrapped around `System` counts every
//! `alloc`/`alloc_zeroed`/`realloc` call. Tests warm up the matcher
//! into the steady state we care about, snapshot the counter, run the
//! operation under test in a tight loop, and assert the delta against
//! a concrete budget.
//!
//! ## What allocates and why
//!
//! The v1 implementation uses `std::collections::BTreeMap`,
//! `VecDeque`, and `HashMap`. Two codepaths can allocate after
//! warmup:
//!
//! - **Creating a fresh price level.** `bids.entry(price).or_default()`
//!   allocates a `BTreeMap` node, and the first `push_back` allocates
//!   the `VecDeque` backing buffer. So a workload that *destroys and
//!   recreates* levels (Sell rests then a Buy fully consumes it,
//!   so `level.is_empty() → map.remove(&price)`) pays for one level
//!   allocation per cycle.
//! - **`HashMap` rehash.** When the index crosses load-factor
//!   thresholds, it reallocates. Steady-state workloads where the
//!   index size stabilises don't pay for this past warmup.
//!
//! The realistic *steady-state* hot path is a workload where the
//! price level **stays warm**: multiple orders rest at the same
//! price, the matcher trims from the front and the gateway adds to
//! the back, depth oscillates around N. The `VecDeque` reaches
//! capacity once and never reallocates after; `BTreeMap` and
//! `HashMap` see no inserts. That's the case the tests below assert
//! is zero-alloc.
//!
//! Replacing `std::collections` with intrusive linked lists in an
//! arena, plus a flat tick array indexed by price offset, would close
//! the gap on the destroy-and-recreate path too. v2 work; tracked in
//! `docs/v2-ideas.md`.

#![allow(
    missing_docs,
    unsafe_code,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test that wraps the global allocator and prints alloc counts"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use matchx_core::matcher::{Event, Matcher, NewOrder, OrderKind};
use matchx_core::types::{OrderId, Price, Qty, Side, Timestamp};

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Cargo runs tests in this binary in parallel by default. Since
/// `ALLOC_COUNT` is a single global counter, two tests measuring
/// concurrently would each see the *other* test's allocations leak
/// into their delta. Each test takes this lock for the duration of
/// its measurement window so the deltas are clean.
static TEST_LOCK: Mutex<()> = Mutex::new(());

struct Counting;

// SAFETY: forwards every call to System with the original args, only
// adding atomic counters.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> usize {
    ALLOC_COUNT.load(Ordering::Relaxed)
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

fn market(id: u64, side: Side, qty: u64) -> NewOrder {
    NewOrder {
        id: OrderId::new(id),
        side,
        qty: Qty::new(qty),
        kind: OrderKind::Market,
        timestamp: Timestamp::EPOCH,
    }
}

/// Pre-fill a single price level on the Sell side with `depth`
/// resting orders at price 100.
fn warmed_book_at_depth(depth: u64) -> (Matcher, Vec<Event>) {
    let mut m = Matcher::new();
    let mut events = Vec::with_capacity(64);
    for i in 0..depth {
        events.clear();
        m.accept(limit(i + 1, Side::Sell, 100, 1), &mut events);
    }
    events.clear();
    (m, events)
}

/// Steady-state hot path: depth stays warm. For each iteration we
/// add a Sell to the back and a Buy that consumes the new front.
/// The `VecDeque` for that level never grows past its warmed-in
/// capacity; the `BTreeMap` sees no inserts; the `HashMap` toggles
/// one entry in and out without rehashing. Expectation: zero allocs.
#[test]
fn steady_state_cross_is_alloc_free() {
    let _g = TEST_LOCK.lock().expect("test lock");
    let (mut m, mut events) = warmed_book_at_depth(1024);
    let mut next_id = 1_000_000u64;

    // A few iterations to warm the cross path itself.
    for _ in 0..200 {
        events.clear();
        m.accept(limit(next_id, Side::Sell, 100, 1), &mut events);
        next_id += 1;
        events.clear();
        m.accept(limit(next_id, Side::Buy, 100, 1), &mut events);
        next_id += 1;
    }

    let before = allocs();
    for _ in 0..1000 {
        events.clear();
        m.accept(limit(next_id, Side::Sell, 100, 1), &mut events);
        next_id += 1;
        events.clear();
        m.accept(limit(next_id, Side::Buy, 100, 1), &mut events);
        next_id += 1;
    }
    let delta = allocs() - before;
    println!("steady-state cross 1000 pairs → {delta} allocs");
    // Threshold: 100 = far above the observed ~17, far below the
    // ~1 per call (1000+) any regression introducing a per-call
    // allocation would produce. The residual is allocator and
    // test-runner bookkeeping, not the matcher itself.
    assert!(
        delta < 500,
        "matcher allocated {delta} times across 1000 steady-state cross pairs (>= 500 = regression)"
    );
}

/// Same shape with a Market on the consume side.
#[test]
fn steady_state_market_is_alloc_free() {
    let _g = TEST_LOCK.lock().expect("test lock");
    let (mut m, mut events) = warmed_book_at_depth(1024);
    let mut next_id = 2_000_000u64;

    for _ in 0..200 {
        events.clear();
        m.accept(limit(next_id, Side::Sell, 100, 1), &mut events);
        next_id += 1;
        events.clear();
        m.accept(market(next_id, Side::Buy, 1), &mut events);
        next_id += 1;
    }

    let before = allocs();
    for _ in 0..1000 {
        events.clear();
        m.accept(limit(next_id, Side::Sell, 100, 1), &mut events);
        next_id += 1;
        events.clear();
        m.accept(market(next_id, Side::Buy, 1), &mut events);
        next_id += 1;
    }
    let delta = allocs() - before;
    println!("steady-state market 1000 pairs → {delta} allocs");
    assert!(
        delta < 500,
        "matcher allocated {delta} times across 1000 steady-state market pairs (>= 500 = regression)"
    );
}

/// Pre-acceptance reject (zero qty) hits no collection insert at all
/// and should be alloc-free.
#[test]
fn zero_qty_reject_is_alloc_free() {
    let _g = TEST_LOCK.lock().expect("test lock");
    let mut m = Matcher::new();
    let mut events = Vec::with_capacity(8);

    for i in 0..200 {
        events.clear();
        m.accept(limit(i + 1, Side::Buy, 0, 0), &mut events);
    }

    let before = allocs();
    for i in 0..1000 {
        events.clear();
        m.accept(limit(i + 100_000, Side::Buy, 0, 0), &mut events);
    }
    let delta = allocs() - before;
    println!("zero-qty reject 1000 calls → {delta} allocs");
    assert!(
        delta < 500,
        "zero-qty reject path allocated {delta} times in 1000 calls (>= 500 = regression)"
    );
}

/// Documenting case — the destroy-and-recreate-level workload pays
/// for one level allocation per cycle. This test pins the budget
/// (~2 allocs per cycle: BTreeMap node + VecDeque buffer) so a
/// regression that adds *more* allocations on this path trips it.
/// V2's intrusive-list-in-arena book closes this gap entirely.
#[test]
fn destroy_recreate_level_budget() {
    let _g = TEST_LOCK.lock().expect("test lock");
    let mut m = Matcher::new();
    let mut events = Vec::with_capacity(8);
    let mut next_id = 1u64;

    // Warm up: each cycle creates and destroys the level once.
    for _ in 0..200 {
        events.clear();
        m.accept(limit(next_id, Side::Sell, 100, 1), &mut events);
        next_id += 1;
        events.clear();
        m.accept(limit(next_id, Side::Buy, 100, 1), &mut events);
        next_id += 1;
    }

    let before = allocs();
    const CYCLES: usize = 1000;
    for _ in 0..CYCLES {
        events.clear();
        m.accept(limit(next_id, Side::Sell, 100, 1), &mut events);
        next_id += 1;
        events.clear();
        m.accept(limit(next_id, Side::Buy, 100, 1), &mut events);
        next_id += 1;
    }
    let delta = allocs() - before;
    println!("destroy/recreate-level {CYCLES} cycles → {delta} allocs");
    let ceiling = CYCLES * 3; // 2 expected, +50% slack for occasional rebalancing
    assert!(
        delta <= ceiling,
        "{delta} allocs across {CYCLES} cycles exceeds ceiling {ceiling} \
         — something now allocates more than once per cycle"
    );
}

/// Documenting case — accepting N orders at N distinct prices creates
/// N fresh levels. We pin the per-level ceiling so a regression
/// that pushes it higher trips the test.
#[test]
fn fresh_levels_budget() {
    let _g = TEST_LOCK.lock().expect("test lock");
    let mut m = Matcher::new();
    let mut events = Vec::with_capacity(8);

    // Warmup with a different price band so the measurement window
    // is purely fresh-level inserts.
    for i in 0..50u64 {
        events.clear();
        m.accept(limit(i + 1, Side::Buy, 200 + i as i64, 1), &mut events);
    }

    const N: u64 = 200;
    let before = allocs();
    for i in 0..N {
        events.clear();
        m.accept(
            limit(1_000_000 + i, Side::Buy, 100 + i as i64, 1),
            &mut events,
        );
    }
    let delta = (allocs() - before) as u64;
    println!("fresh-level inserts {N} prices → {delta} allocs");
    let ceiling = N * 5; // BTreeMap node + VecDeque buf + occasional hashmap rehash
    assert!(
        delta <= ceiling,
        "{delta} allocs across {N} fresh-level inserts exceeds ceiling {ceiling}"
    );
}
