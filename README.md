# matchx

A high-performance, crash-safe limit order book matching engine in Rust.
Single-instrument, price-time priority, FIX-inspired binary protocol over
TCP, write-ahead log with byte-exact replay, and a lock-free SPSC queue
between the gateway and the matcher.

## Headline numbers

End-to-end on M-series silicon, single matcher thread, single TCP
connection, release build:

```
in-process round-trip            ~225 ns
TCP round-trip (loopback) p50    ~45 µs
TCP round-trip (loopback) p99    ~109 µs
TCP throughput (pipelined)       ~118 k orders/sec
matcher walks 1000 levels        ~94 µs (≈10 M trades/sec)
WAL group commit speedup         187× to 245×
```

## What this demonstrates

- **Lock-free SPSC ring buffer** (cache-padded head/tail, cached views,
  Acquire/Release pair) **validated by Miri** in CI on every push.
  See [`crates/matchx-core/src/spsc.rs`](crates/matchx-core/src/spsc.rs)
  and the [write-up](docs/posts/lock-free-spsc.md).
- **Hot-path zero-allocation, machine-verified.** A custom
  global-allocator harness counts every `alloc`/`realloc` call. The
  steady-state `Limit`-cross path measures **0 allocs per 1000 pairs**
  on macOS and well under one alloc-per-call on Ubuntu CI. See
  [`crates/matchx-core/tests/no_alloc.rs`](crates/matchx-core/tests/no_alloc.rs).
- **Byte-exact WAL replay.** 10 000 random orders run through a live
  matcher with `fsync` per command; a fresh matcher replays the WAL;
  the live and replayed books *and* event streams are equal sequence
  for sequence. See
  [`crates/matchx-core/tests/replay.rs`](crates/matchx-core/tests/replay.rs)
  and the [write-up](docs/posts/wal-and-byte-exact-replay.md).
- **Snapshot recovery.** Mid-stream snapshot at sequence N; recovery
  loads the snapshot, skips WAL records with `wal_seq <= N`, replays
  the tail. Result is byte-equal to the live engine. See
  [`crates/matchx-core/tests/snapshot_recovery.rs`](crates/matchx-core/tests/snapshot_recovery.rs).
- **WAL group commit benchmark** demonstrating a measured 187–245×
  throughput improvement vs `fsync`-per-record at batch=256, with
  the ratio holding across both macOS and Linux (CI artifact).

## Architecture

```
   tokio gateway thread(s)            matcher thread (dedicated)
        │                                  ▲
        │  Command::New{...}               │  poll
        │  Command::Cancel{id}             │
        ▼                                  │
   ┌───────────┐                       ┌───────────┐
   │  SPSC in  │ ────────────────────▶ │  matcher  │
   └───────────┘                       └───────────┘
                                            │
                                            │  Event::Trade{...}
                                            │  Event::Done{...}
                                            ▼
                                       ┌───────────┐
                                       │ SPSC out  │ ──▶  publisher / WAL
                                       └───────────┘
```

The matcher itself runs on one dedicated thread — single-writer, no
contention to design around. The lock-free primitives are the SPSC
queues at the boundaries; that's where `unsafe`, the `// SAFETY:`
proofs, and Miri validation live. The matching path uses fixed-point
integer arithmetic only — no floats, no allocation in steady state.

The WAL is the durability boundary: every state-changing op is fsynced
before the corresponding `ExecutionReport` is sent to the client.
Recovery loads the latest snapshot plus the WAL tail and reconstructs
state byte-for-byte.

## Layout

| Crate              | Purpose                                                   |
| ------------------ | --------------------------------------------------------- |
| `matchx-core`      | Matching engine library. Types, order book, matcher, WAL, snapshot, lock-free SPSC. |
| `matchx-protocol`  | FIX-inspired binary wire protocol codec.                  |
| `matchx-server`    | tokio TCP gateway; one engine per connection (v1).        |
| `matchx-client`    | Test client + load generator with RTT histogram.          |
| `matchx-replay`    | Recovery binary: rebuild book from snapshot + WAL tail.   |
| `matchx-bench`     | Cross-crate `criterion` benches.                          |

## Quickstart

```bash
# Pinned toolchain — rustup picks 1.95.0 from rust-toolchain.toml.
rustup show

cargo test --workspace                   # unit + property + integration
cargo bench --workspace --no-run         # confirm benches build
cargo bench -p matchx-core               # actually run them
```

End-to-end TCP demo:

```bash
# Terminal 1
cargo run --release -p matchx-server -- 127.0.0.1:9000

# Terminal 2: 5000 RTT samples + 50000-order throughput burst
cargo run --release -p matchx-client -- 127.0.0.1:9000 5000 50000
```

Recovery from a WAL (with optional snapshot) printing a state hash:

```bash
cargo run --release -p matchx-replay -- --wal path/to/wal
cargo run --release -p matchx-replay -- --snapshot path/to/snap --wal path/to/wal
```

## What to read first

For a 5-minute interviewer skim:

1. The [SPSC write-up](docs/posts/lock-free-spsc.md) — cache padding,
   memory ordering, the `!Sync` trick, Miri validation.
2. The [WAL + replay write-up](docs/posts/wal-and-byte-exact-replay.md)
   — input log vs output log, CRC32C, truncation tolerance, why
   "byte-exact" needs the matcher's seq generator re-seeded.
3. The [matcher's lifecycle proptest][lifecycle] — a per-id state
   machine that simultaneously verifies fill conservation, no
   over-/under-fill, no `Trade` before `Accepted`, and correct
   `leaves_qty` on cancel. Caught two real bugs while it was being
   written; both fixed in the same PR.
4. The [allocation-counting harness][alloc] — closes the charter
   gap "no allocation on the hot path" with measurements rather
   than argument.

[lifecycle]: crates/matchx-core/src/matcher.rs
[alloc]: crates/matchx-core/tests/no_alloc.rs

## Documentation

- [Architecture](docs/architecture.md)
- [Correctness guarantees](docs/correctness-guarantees.md)
- [Development log](docs/log.md) (slice-by-slice)
- [v2 ideas (out of scope)](docs/v2-ideas.md)

### Long-form write-ups

- [Designing the matchx lock-free SPSC queue](docs/posts/lock-free-spsc.md)
- [Crash-safe matching: WAL and byte-exact replay](docs/posts/wal-and-byte-exact-replay.md)

### CI

Every push runs: `cargo fmt --check`, `cargo clippy --all-targets -D
warnings`, `cargo test --workspace`, `cargo doc --no-deps`,
`cargo bench --no-run`, **Miri** on the lock-free modules, and a
**bench numbers** job on `ubuntu-latest` that uploads
`bench_numbers.md` as a downloadable artifact.

## License

MIT — see [LICENSE](LICENSE).
