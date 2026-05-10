# log

## slice 0
Workspace, pinned toolchain, CI (fmt / clippy / test / doc), property tests
on `Price` and `SequenceGenerator`. Trait stub for `OrderBook` with ignored
placeholder tests for the invariants we'll prove later.

## slice 1
Concrete `Book`: `BTreeMap<Price, VecDeque<Order>>` per side, `HashMap`
index for cancel. `add` / `cancel` / `best_bid` / `best_ask` plus
`level_qty` / `level_len`. `cancel` returns `Option<Qty>` so the matcher
can carry the resting qty through to the cancel ack.

Tests: 9 unit, 4 proptest. First criterion bench. Quick numbers on
M-series silicon:

```
Book::add    depths  0/100/1k/10k → ~99 / 61 / 130 / 554 ns
Book::cancel depths  1/100/1k/10k → ~69 / 122 / 185 / 199 ns  (front)
```

Cancel is flat with depth because `VecDeque::remove(0)` is O(1); the
worst case is middle cancel, which a later bench will exercise.

`cargo bench --no-run` wired into CI.

## slice 2
The matcher. `Matcher::accept(NewOrder, &mut Vec<Event>)` — caller owns
the event buffer so the hot path doesn't allocate per call. Covers
Limit / Market / IOC, partial fills, walking multiple price levels,
duplicate-id rejection (which is also v1's STP — duplicates can't
self-trade because they're never accepted).

Events: `Accepted { id, qty, seq }`, `Trade { taker, maker, price, qty,
seq }`, `Done { id, leaves_qty, reason, seq }` with reasons Filled /
Cancelled / Expired (IOC) / NoLiquidity (Market) / Rejected.

Added `Book::take_front` so the matcher consumes liquidity in one call.

Tests: 14 unit covering each order type and edge case, 2 proptests:
**monotonic_sequence** (every emitted seq = prev + 1) and
**lifecycle_consistent** — a per-id state machine that simultaneously
verifies fill conservation, no over- / under-fill, no Trade before
Accepted, and correct `leaves_qty` on cancel. The lifecycle test caught
two real bugs while I was writing it (duplicate-id Done collisions, and
`leaves_qty=0` on cancel of any-qty order).

Bench: `Matcher::accept` no-cross at depths 0/100/1k → ~128/554/691 ns.
Walking N levels and fully filling: 1/10/100/1000 levels → ~143ns/496ns/
7.9µs/94µs (≈10M trades/s ceiling on this path).

## slice 3 — next
WAL. Append-only segments, fsync-on-commit, **CRC32C** per record,
periodic snapshots. Replay tool reads (snapshot + WAL tail) and exits
with a state hash; integration test asserts replay hash == live hash on
a randomised stream of ≥10k orders. Adds the `Replay` placeholder test
that's been ignored since slice 0.
