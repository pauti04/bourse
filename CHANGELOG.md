# Changelog

Slice-by-slice. Each entry maps to a single squash-merged commit on
`main` (one PR per slice). Numbering follows the original PR numbers
in the queue.

## Unreleased / 0.1.0

### Slice 0 — Bootstrap
- Workspace skeleton (six member crates), pinned `rust-toolchain.toml`
  to 1.95.0, MIT LICENSE, `.cargo/config.toml`, `.gitignore`.
- `[workspace.lints]` denying every charter-forbidden construct
  (`unsafe_code`, `missing_docs`, `unwrap_used`, `expect_used`,
  `panic`, `print_stdout/stderr`, `dbg_macro`, `todo`,
  `unimplemented`, `unreachable`).
- `matchx-core::types`: `OrderId`, `Sequence` + `SequenceGenerator`,
  `Side`, `Price` (i64 fixed-point, 8 fractional digits), `Qty`,
  `Timestamp`. Property tests for `Price` arithmetic / ordering and
  `SequenceGenerator` strict monotonicity.
- GitHub Actions CI: fmt / clippy / test / doc.

### Slice 1 — In-memory order book
- `Book` struct: `BTreeMap<Price, VecDeque<Order>>` per side,
  `HashMap<OrderId, (Side, Price)>` index for O(log n) cancel.
- 9 unit tests + 4 proptests. First criterion bench (add / cancel
  at depths 0/100/1k/10k).
- `cargo bench --no-run` wired into CI.

### Slice 2 — Matcher
- `Matcher::accept(NewOrder, &mut Vec<Event>)` — caller owns the
  event buffer so the hot path doesn't allocate per call.
  Limit / Market / IOC, partial fills, walks multiple price levels.
  Duplicate-id rejection doubles as v1's STP.
- Lifecycle proptest (per-id state machine) caught two real
  correctness bugs while it was being written; both fixed.
- Bench: `accept` no-cross at depths 0/100/1k → ~128 / 554 / 691 ns.

### Slice 3 — WAL + byte-exact replay
- Append-only segments, length-prefixed and CRC32C-framed records,
  fsync-on-commit. Truncated trailing record handled as clean EOF.
- Headline integration test: 10 k random orders run live and
  replayed; live and replayed books AND event streams are byte-equal.

### Slice 4 — Lock-free SPSC + Miri CI
- Cache-padded head / tail, cached views of the other side's index,
  Acquire/Release pair with `// SAFETY:` proofs per `unsafe` block.
- Miri job in CI on every push.
- Bench: ~5.3 ns per push+pop in steady state.

### Slice 5 — End-to-end engine
- `Engine::start` spawns the matcher on a dedicated OS thread; SPSC
  queues at the boundaries. Round-trip latency ~225 ns (Market on
  empty), ~424 ns (Limit fully fills against 1 maker).

### Slice 6 — Wire protocol codec
- Hand-rolled binary: length-prefixed frames, 1-byte version + 1-byte
  message type + fixed-size payload. `NewOrder`, `Cancel`,
  `Execution(Event)`. Round-trip proptests.

### Slice 7 — TCP server
- tokio-based listener; `Engine::split` so reader/writer tokio tasks
  each own one half of the SPSC. `TCP_NODELAY` on. Loopback
  integration tests.

### Slice 8 — Load-gen client + RTT histogram
- `matchx-client` with two modes: sequential RTT (warmup + 10 k
  measurement) and pipelined throughput burst.
- Numbers (M-series, loopback): RTT p50 ~45 µs, p99 ~109 µs;
  throughput ~118 k orders/sec.

### Slice 9 — Lock-free SPSC write-up
- `docs/posts/lock-free-spsc.md`. Cache padding, cached views,
  Acquire/Release reasoning, the `!Sync` trick, Miri validation,
  numbers.

### Slice 10 — Snapshots + byte-exact recovery
- Atomic temp-then-rename snapshot writer, versioned format.
  `Matcher::with_book` plus `SequenceGenerator::starting_at` so
  resting orders added during WAL-tail replay get the same seq
  values they had on the live engine.
- Headline integration test: live → snapshot → live → recover →
  byte-equal.

### Slice 11 — WAL + replay write-up
- `docs/posts/wal-and-byte-exact-replay.md`. Input log vs output
  log, CRC32C, truncation tolerance, snapshots, why "byte-exact"
  needs the matcher's seq generator re-seeded.

### Slice 12 — Allocation-counting harness
- Custom `GlobalAlloc` wrapping `System` counts every alloc /
  realloc call. Steady-state cross loop machine-verified to be
  0 allocs / 1000 pairs (release) on every CI push.

### Slice 13 — WAL records seq-tagged + snapshot v2
- WAL format bumped to v2: each record carries an 8-byte `wal_seq`.
  Snapshot format bumped to v2: stores both `matcher_seq` and
  `wal_seq`. Recovery is now self-contained — no out-of-band
  coordination from the test harness.

### Slice 14 — WAL group commit bench
- `benches/wal_commit.rs`: at batch=256, group commit is 245× faster
  than fsync-per-record on macOS APFS, 187× on Ubuntu CI tmpfs.

### Slice 15 — Linux bench numbers as CI artifact
- New CI job uploads `bench_numbers.md` on every PR with criterion
  output from every bench, runner CPU info, and a noise caveat.

### Slice 16 — `matchx-replay` binary
- The placeholder stub from slice 0 is now a real recovery binary:
  `matchx-replay --snapshot <p> --wal <p>` loads the snapshot,
  skips WAL records by `wal_seq`, replays the rest, prints a state
  hash.

### Slice 17 — README hero rewrite
- Numbers in a code block at the top, ASCII architecture diagram,
  "what to read first" section linking the lifecycle proptest and
  the alloc-counting harness.

### Slice 18 — Multi-tenant gateway via lock-free MPSC
- New `Hub` (`crossbeam_queue::ArrayQueue` MPSC) + per-tenant
  outbound `tokio::sync::mpsc`. `matchx-server` switched to share
  one matcher across many TCP connections. New `multi_tenant.rs`
  integration test with three concurrent clients. Removes the v1
  "one connection per matcher" limit.

### Slice 19 — Numbers + methodology write-up
- `docs/posts/numbers-and-methodology.md`. What each headline number
  measures, where the bench code lives, what we don't claim.

### Slice 20 — Honest student framing on the README
- "About this project" callout, "What I learned while building this"
  bullets pinned to specific code, removed "high-performance,
  crash-safe" oversell from the tagline.

### Slice 21 — GitHub Pages site
- `docs/index.md` landing + `docs/_config.yml`. Site live at
  <https://pauti04.github.io/matchx/>; the three write-ups render
  at `/posts/<slug>.html`.

### Slice 22 — CI badges + this CHANGELOG
- README gains build / license / Rust / site badges.
- `CHANGELOG.md` (this file) maps every commit on `main` to its
  slice.
