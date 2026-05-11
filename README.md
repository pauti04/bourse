# bourse

[![CI](https://github.com/pauti04/bourse/actions/workflows/ci.yml/badge.svg)](https://github.com/pauti04/bourse/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.95-orange.svg)](rust-toolchain.toml)
[![Site](https://img.shields.io/badge/site-pauti04.github.io%2Fbourse-brightgreen.svg)](https://pauti04.github.io/bourse/)

A limit order book matching engine in Rust. Single-instrument,
price-time priority, length-prefixed binary protocol over TCP,
write-ahead log with byte-exact replay, and a lock-free SPSC queue
between the gateway and the matcher.

> **About this project.** This is a learning portfolio piece I built
> to understand how a real matching engine is put together — not a
> production system. The goal was to make decisions an interviewer
> would push on (allocator behavior, memory ordering, durability
> semantics, benchmarking discipline) and *measure* them rather than
> hand-wave. What's here works and is tested; what's not built is
> documented honestly in [`docs/v2-ideas.md`](docs/v2-ideas.md) and
> in the limitations called out throughout.

## Headline numbers

End-to-end on M-series silicon, single matcher thread, multi-tenant
Hub, release build:

```
in-process round-trip            ~225 ns
TCP round-trip (loopback) p50    ~78 µs
TCP round-trip (loopback) p99    ~307 µs
TCP throughput (pipelined)       ~88 k orders/sec
matcher walks 1000 levels        ~94 µs (≈10 M trades/sec)
WAL group commit speedup         187× to 245×
```

The TCP RTT shifted from earlier `~45 µs` to `~78 µs` when the server
gained the multi-tenant Hub (slice 18) — the MPSC + per-tenant
routing adds a small constant overhead vs the earlier per-connection
SPSC. The trade-off: the v1 "one connection per matcher" limit is
gone.

## Live demo

A real captured run from `cargo run --release -p bourse-client`
against a local `bourse-server`:

```
$ bourse-server 127.0.0.1:9000 &
INFO bourse-server listening addr=127.0.0.1:9000
INFO hub started, accepting connections inbox_capacity=8192

$ bourse-client 127.0.0.1:9000 2000 20000
connecting to 127.0.0.1:9000 ...

RTT (sequential):
  samples:    2000
  p50:        78542 ns
  p90:        112125 ns
  p99:        307334 ns
  p99.9:      782500 ns
  max:        1093959 ns

throughput (pipelined burst):
  orders submitted:   20000
  Done(Filled) seen:  10000
  wall time:          228.27ms
  rate:               87616 orders/sec (43808 round-trips/sec)
```

[Full capture](docs/demo-output.txt). To re-run: see
[Quickstart](#quickstart).

## What I learned while building this

- **Memory ordering isn't intuition.** Writing the SPSC queue and
  walking through the Acquire/Release happens-before argument by
  hand was the first time I felt I actually understood what the
  C++20 memory model is *doing* rather than just citing it. Miri
  catching subtle ordering bugs locally — before they ever became
  shipped data-race weirdness — is the strongest tooling lesson.
- **"Zero alloc on the hot path" is a claim that needs a meter.**
  Argued it, didn't prove it for many slices; finally built the
  custom-allocator harness in
  [`tests/no_alloc.rs`](crates/bourse-core/tests/no_alloc.rs) and
  the gap between "I think this is alloc-free" and "the steady-
  state cross loop is 0/1000" was instructive.
- **Property tests find real bugs.** The matcher's lifecycle
  proptest caught two real correctness bugs while I was *writing*
  it — duplicate-id `Done` collisions and `Book::cancel` lying
  about `leaves_qty`. Both are visible in the slice 2 commit.
- **Benchmarks lie if you don't define them carefully.** My first
  TCP load-gen reported `p50 = 275 ms` because I'd built a
  closed-loop measurement that double-counted queueing delay. The
  [methodology post](docs/posts/numbers-and-methodology.md) walks
  through what each headline number actually measures and why.
- **Versioning everything from day one is cheap.** Both the WAL and
  the snapshot file format carry a version byte from the very first
  byte. Slice 13 bumped both because we needed to add `wal_seq`
  tagging — the bump cost a one-line code change.

## What this demonstrates

- **Lock-free SPSC ring buffer** (cache-padded head/tail, cached views,
  Acquire/Release pair) **validated by Miri** in CI on every push.
  See [`crates/bourse-core/src/spsc.rs`](crates/bourse-core/src/spsc.rs)
  and the [write-up](docs/posts/lock-free-spsc.md).
- **Hot-path zero-allocation, machine-verified.** A custom
  global-allocator harness counts every `alloc`/`realloc` call. The
  steady-state `Limit`-cross path measures **0 allocs per 1000 pairs**
  on macOS and well under one alloc-per-call on Ubuntu CI. See
  [`crates/bourse-core/tests/no_alloc.rs`](crates/bourse-core/tests/no_alloc.rs).
- **Byte-exact WAL replay.** 10 000 random orders run through a live
  matcher with `fsync` per command; a fresh matcher replays the WAL;
  the live and replayed books *and* event streams are equal sequence
  for sequence. See
  [`crates/bourse-core/tests/replay.rs`](crates/bourse-core/tests/replay.rs)
  and the [write-up](docs/posts/wal-and-byte-exact-replay.md).
- **Snapshot recovery.** Mid-stream snapshot at sequence N; recovery
  loads the snapshot, skips WAL records with `wal_seq <= N`, replays
  the tail. Result is byte-equal to the live engine. See
  [`crates/bourse-core/tests/snapshot_recovery.rs`](crates/bourse-core/tests/snapshot_recovery.rs).
- **WAL group commit benchmark** demonstrating a measured 187–245×
  throughput improvement vs `fsync`-per-record at batch=256, with
  the ratio holding across both macOS and Linux (CI artifact).

## Architecture

```mermaid
flowchart LR
    subgraph gateways["tokio gateway tasks (per connection)"]
        c1[client A reader]
        c2[client B reader]
        c3[client N reader]
    end

    mpsc[("MPSC inbox<br/>crossbeam ArrayQueue")]
    matcher{{"matcher thread<br/>single-writer<br/>0 alloc on hot path"}}

    subgraph fanout["per-tenant SPSC outbound"]
        e1[client A writer]
        e2[client B writer]
        e3[client N writer]
    end

    wal[(WAL<br/>CRC32C frames<br/>fsync-on-commit)]
    snap[(snapshot<br/>marker + book)]

    c1 & c2 & c3 -->|"Command<br/>NewOrder, Cancel"| mpsc
    mpsc --> matcher
    matcher -->|"Event<br/>Accepted, Trade, Done"| e1
    matcher --> e2
    matcher --> e3
    matcher -.->|append + fsync| wal
    wal -.->|periodic| snap
```

The matcher itself runs on one dedicated thread — single-writer, no
contention to design around. The lock-free primitives are the queues
at the boundaries: a multi-producer single-consumer queue at ingress
(many gateways feeding one matcher) and per-tenant single-producer
single-consumer queues at egress. The SPSC at egress is where
`unsafe`, the `// SAFETY:` proofs, and Miri validation live. The
matching path uses fixed-point integer arithmetic only — no floats,
no allocation in steady state.

The WAL is the durability boundary: every state-changing op is fsynced
before the corresponding `ExecutionReport` is sent to the client.
Recovery loads the latest snapshot plus the WAL tail and reconstructs
state byte-for-byte.

## Layout

| Crate              | Purpose                                                   |
| ------------------ | --------------------------------------------------------- |
| `bourse-core`      | Matching engine library. Types, order book, matcher, WAL, snapshot, lock-free SPSC. |
| `bourse-protocol`  | FIX-inspired binary wire protocol codec.                  |
| `bourse-server`    | tokio TCP gateway; one engine per connection (v1).        |
| `bourse-client`    | Test client + load generator with RTT histogram.          |
| `bourse-replay`    | Recovery binary: rebuild book from snapshot + WAL tail.   |
| `bourse-bench`     | Cross-crate `criterion` benches.                          |

## Quickstart

```bash
# Pinned toolchain — rustup picks 1.95.0 from rust-toolchain.toml.
rustup show

cargo test --workspace                   # unit + property + integration
cargo bench --workspace --no-run         # confirm benches build
cargo bench -p bourse-core               # actually run them
```

End-to-end TCP demo:

```bash
# Terminal 1
cargo run --release -p bourse-server -- 127.0.0.1:9000

# Terminal 2: 5000 RTT samples + 50000-order throughput burst
cargo run --release -p bourse-client -- 127.0.0.1:9000 5000 50000
```

Recovery from a WAL (with optional snapshot) printing a state hash:

```bash
cargo run --release -p bourse-replay -- --wal path/to/wal
cargo run --release -p bourse-replay -- --snapshot path/to/snap --wal path/to/wal
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

[lifecycle]: crates/bourse-core/src/matcher.rs
[alloc]: crates/bourse-core/tests/no_alloc.rs

## Documentation

- [Architecture](docs/architecture.md)
- [Correctness guarantees](docs/correctness-guarantees.md)
- [Development log](docs/log.md) (slice-by-slice)
- [v2 ideas (out of scope)](docs/v2-ideas.md)

### Long-form write-ups

- [Designing the bourse lock-free SPSC queue](docs/posts/lock-free-spsc.md)
- [Crash-safe matching: WAL and byte-exact replay](docs/posts/wal-and-byte-exact-replay.md)
- [bourse numbers, and how they were measured](docs/posts/numbers-and-methodology.md) —
  what each headline number actually measures, what the bench code is,
  and what we don't claim.

### CI

Every push runs: `cargo fmt --check`, `cargo clippy --all-targets -D
warnings`, `cargo test --workspace`, `cargo doc --no-deps`,
`cargo bench --no-run`, **Miri** on the lock-free modules, and a
**bench numbers** job on `ubuntu-latest` that uploads
`bench_numbers.md` as a downloadable artifact.

## License

MIT — see [LICENSE](LICENSE).
