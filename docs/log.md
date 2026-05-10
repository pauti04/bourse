# log

## slice 0
Workspace, pinned toolchain, CI (fmt / clippy / test / doc), property tests
on `Price` and `SequenceGenerator`. Trait stub for `OrderBook` with ignored
placeholder tests for the invariants we'll prove later.

## slice 1 — next
Concrete `OrderBook`: `BTreeMap<Price, VecDeque<Order>>` per side, plus an
`OrderId` index for O(log n) cancel. First criterion bench on add/cancel
with full latency histogram. Wire `cargo bench --no-run` into CI.
