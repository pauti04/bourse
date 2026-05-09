# matchx development log

Each slice ends with: tests passing, `cargo fmt --check` clean, CI green,
and the next slice noted below.

---

## Slice 0 — Bootstrap (in progress, awaiting GitHub remote)

Date: 2026-05-09.

**Shipped:**
- Workspace skeleton: root `Cargo.toml`, `rust-toolchain.toml` (pinned to
  Rust 1.95.0), `.cargo/config.toml`, `.gitignore`, MIT `LICENSE`,
  placeholder `README.md`.
- Six member crates with stub `lib.rs` / `main.rs`:
  `matchx-core`, `matchx-protocol`, `matchx-server`, `matchx-client`,
  `matchx-replay`, `matchx-bench`.
- `[workspace.lints]` encoding the charter's non-negotiables: `unsafe_code`,
  `missing_docs`, `unwrap_used`, `expect_used`, `panic`, `print_stdout`,
  `print_stderr`, `dbg_macro`, `todo`, `unimplemented`, `unreachable`,
  `mem_forget` all denied.
- `matchx-core::types`: `OrderId`, `Sequence` + `SequenceGenerator`, `Side`,
  `Price` (i64 fixed-point, 8 fractional digits), `Qty`, `Timestamp`. All
  newtypes with rustdoc and `const` constructors / accessors.
- Property tests (proptest):
  - `Price::saturating_add` stays in range and matches `i64::saturating_add`.
  - `Price::saturating_sub` matches `i64::saturating_sub`.
  - `Price` ordering is total and matches raw `i64` ordering.
  - `SequenceGenerator::next` is strictly monotonic with stride 1, starts at 1.
  - `peek` is idempotent and predicts `next`.
- `OrderBook` trait stub with no-op method signatures.
- Property-test placeholders for v1 invariants (price-time priority, fill
  conservation, non-negativity, monotonic-seq, WAL replay equality), each
  `#[ignore]`d with a TODO naming the slice that owns it.
- GitHub Actions CI: `fmt --check`, `clippy --all-targets -- -D warnings`,
  `test --workspace`, `doc --workspace --no-deps`.
- Docs scaffolding: `architecture.md`, `correctness-guarantees.md`,
  `wire-protocol.md`, `order-types.md`, `dependencies.md`, `v2-ideas.md`.

**Deferred from this slice (with reason):**
- **Miri CI job.** No lock-free or `unsafe` code yet — Miri would have
  nothing to validate. Wire up alongside the SPSC queue slice.
- **`cargo bench --no-run` in CI.** No benches yet — would only add minutes.
  Wire up in the order-book slice when the first criterion bench lands.
- **Allocation-counting test harness.** No hot path yet to instrument.
  Wire up in the matcher slice.

**Open items / things the user owns:**
- Create the GitHub repo (`gh repo create matchx --public`) and add the
  remote. The bootstrap commit is on a `bootstrap` branch ready to push
  and PR against `main`.
- Update the `repository` field in the root `Cargo.toml`
  (`https://github.com/REPLACE-ME/matchx`) once the repo URL is known.

**Next slice (proposed): Slice 1 — Order book, single-writer, in-memory.**
- Concrete `OrderBook` impl. Probable shape: `BTreeMap<Price, VecDeque<Order>>`
  per side, plus an `OrderId -> (Price, Side, position)` index for O(log n)
  cancel.
- Un-ignore: `price_time_priority_preserved`, `no_negative_quantities`.
- Add unit tests: `add_order` / `cancel_order` round-trip; `best_bid` /
  `best_ask` after add/cancel; cancel of unknown id returns `false`.
- No matching logic yet (no crossing). Pure data-structure slice.
- First `criterion` bench: `add_order` and `cancel_order` latency
  histograms. Wire `cargo bench --no-run` into CI as part of this slice.
