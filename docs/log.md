# log

## slice 0
Workspace, pinned toolchain, CI (fmt / clippy / test / doc), property tests
on `Price` and `SequenceGenerator`. Trait stub for `OrderBook` with ignored
placeholder tests for the invariants we'll prove later.

## slice 1
Concrete `Book`: `BTreeMap<Price, VecDeque<Order>>` per side, `HashMap`
index for cancel. `add` / `cancel` / `best_bid` / `best_ask` plus
`level_qty` / `level_len` for introspection. Returns `bool` — an error
enum can come back when a caller actually needs the reason.

Tests: 9 unit, 4 proptest (best-price extrema, index consistency, no
empty levels left behind, level membership matches the index), plus the
time-priority surrogate.

First criterion bench. Quick numbers on M-series silicon:

```
Book::add    depths  0/100/1k/10k → ~99 / 61 / 130 / 554 ns
Book::cancel depths  1/100/1k/10k → ~69 / 122 / 185 / 199 ns  (front)
```

Cancel is flat with depth because `VecDeque::remove(0)` is O(1); the
worst case is middle cancel, which slice 2's bench will exercise.

Wired `cargo bench --no-run` into CI so the bench harness compiles on
every push.

## slice 2 — next
The matcher. Cross incoming orders against the book; emit
`ExecutionReport`s. IOC handling. Un-ignore `fill_conservation` and
`monotonic_sequence`. Add an allocation-counting harness so the matcher's
hot path can be asserted zero-alloc.
