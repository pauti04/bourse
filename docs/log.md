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

`cargo bench --no-run` wired into CI.

## slice 2
The matcher. `Matcher::accept(NewOrder, &mut Vec<Event>)` — caller owns
the event buffer so the hot path doesn't allocate per call. Limit /
Market / IOC, partial fills, walks multiple price levels. Duplicate-id
rejection doubles as v1's STP.

Events: `Accepted`, `Trade`, `Done` with reasons Filled / Cancelled /
Expired (IOC) / NoLiquidity (Market) / Rejected.

Added `Book::take_front` so the matcher consumes liquidity in one call.

The lifecycle proptest (per-id state machine) caught two real bugs
during writing — duplicate-id Done collisions and `Book::cancel`
returning `bool` and so lying about `leaves_qty`. Both fixed.

```
Matcher::accept (no cross)       depth 0/100/1k    → ~128/554/691 ns
Matcher::accept (walks N levels) N=1/10/100/1000   → ~143ns/496ns/7.9µs/94µs
```

≈10M trades/s ceiling on the crossing path with this structure.

## slice 3
Write-ahead log. Append-only segments, length-prefixed and CRC32C-framed
records, fsync-on-commit. Replay re-feeds the recorded inputs through a
fresh `Matcher` and produces the same book and the same event stream
**byte-for-byte**.

The headline integration test (`tests/replay.rs`) generates 10k random
orders, runs them through a live matcher while logging every input to a
WAL with fsync per command, then opens a fresh matcher and replays the
WAL. Asserts:
- Live book == replayed book (`Book` derives `PartialEq`).
- Live event stream == replayed event stream (sequence-by-sequence).

`Book::cancel` already returned the resting qty; that lets the cancel
event carry correct `leaves_qty` through replay. CRC mismatch is
surfaced as a typed error; truncated trailing records are tolerated as
clean EOF (so a crash mid-fsync doesn't poison the segment). Bad magic
and unknown versions are rejected.

Tests: 4 unit + 1 codec proptest + the 10k integration test. 46 tests
in matchx-core total. Dep added: `crc32c = "0.6"` (hardware-accelerated
CRC32C on x86-64 SSE 4.2 and ARMv8 — the standard choice over IEEE
CRC32 for WALs).

## slice 4 — next
Snapshots. Periodic serialization of book state alongside a sequence
marker, so recovery doesn't have to replay the entire WAL. Add a
snapshotter thread design (matcher emits a marker; sidecar walks the
WAL up to the marker and writes the snapshot file). Recovery tool
(`matchx-replay` binary) wired up: load latest snapshot, replay WAL
since.
