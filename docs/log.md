# log

## slice 0
Workspace, pinned toolchain, CI (fmt / clippy / test / doc), property tests
on `Price` and `SequenceGenerator`. Trait stub for `OrderBook` with ignored
placeholder tests for the invariants we'll prove later.

## slice 1
Concrete `Book`: `BTreeMap<Price, VecDeque<Order>>` per side, `HashMap`
index for cancel. `add` / `cancel` / `best_bid` / `best_ask` plus
`level_qty` / `level_len`.

```
Book::add    depths  0/100/1k/10k → ~99 / 61 / 130 / 554 ns
Book::cancel depths  1/100/1k/10k → ~69 / 122 / 185 / 199 ns  (front)
```

`cargo bench --no-run` wired into CI.

## slice 2
The matcher. `Matcher::accept(NewOrder, &mut Vec<Event>)` — caller owns
the event buffer so the hot path doesn't allocate per call. Limit /
Market / IOC, partial fills, walks multiple price levels. Duplicate-id
rejection doubles as v1's STP.

The lifecycle proptest (per-id state machine) caught two real bugs
during writing — duplicate-id Done collisions and `Book::cancel` lying
about `leaves_qty`. Both fixed.

```
Matcher::accept (no cross)       depth 0/100/1k    → ~128/554/691 ns
Matcher::accept (walks N levels) N=1/10/100/1000   → ~143ns/496ns/7.9µs/94µs
```

≈10M trades/s ceiling on the crossing path with this structure.

## slice 3
WAL. Append-only segments, length-prefixed and CRC32C-framed records,
fsync-on-commit. `WalReader` tolerates a truncated trailing record
(crash mid-fsync) as clean EOF and surfaces CRC mismatch as a typed
error. Versioned: 4-byte magic + 1-byte segment version at file start;
every record carries its own version byte.

Headline integration test (`tests/replay.rs`): 10k random orders run
through a live matcher with WAL fsync per command; a fresh matcher
replays the WAL; live and replayed books are byte-equal AND live and
replayed event streams are sequence-by-sequence identical. ~5500 trades
on the test workload.

## slice 4
Lock-free single-producer single-consumer ring buffer. Cache-padded
head and tail (separate cache lines so producer and consumer atomics
don't bounce). Each side caches the other's index and only re-reads
from the atomic when the cache says full / empty — that keeps the
remote cache line out of the hot path most of the time.

Standard Acquire/Release pair: producer writes the slot, then publishes
tail with `Release`; consumer reads tail with `Acquire` (which
establishes happens-before with the slot write) before reading the
slot. Symmetric on head for the consumer publishing "this slot is now
free". `unsafe` is gated by `#[allow(unsafe_code)]` at module level;
each block carries a `// SAFETY:` comment naming the invariant it
relies on.

**Miri job in CI** (`cargo +nightly miri test --package matchx-core
--lib spsc`) exercises the unsafe blocks, atomic ordering, and a
threaded producer/consumer pair (smaller N under cfg(miri) so the run
finishes in seconds rather than hours). Catches data races and
provenance violations.

7 unit tests including a 100k-element threaded test (with Miri-aware
N). Bench: push+pop batch of 256 in steady state ≈ 5.3 ns per
operation on M-series silicon.

## slice 5 — next
Wire the SPSC queue into the matcher: gateway stub pushes `NewOrder` /
`Cancel` commands onto the queue; matcher polls in a tight loop on a
dedicated thread, drains commands and emits events. End-to-end
latency bench: command queued → corresponding event observable.
