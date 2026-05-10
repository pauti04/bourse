# matchx

A limit order book matching engine in Rust. Single instrument,
price-time priority, write-ahead log with byte-exact replay, lock-free
single-producer single-consumer queues at the boundaries.

WIP — see [`docs/log.md`](docs/log.md) for what's done and what's next.

## Numbers

End-to-end **over loopback TCP** (M-series silicon, macOS, release
build, single connection, single matcher thread):

```
RTT (sequential, send→Done(Filled)) over 5000 iters
  p50   ~45 µs
  p90   ~64 µs
  p99   ~109 µs
  p99.9 ~150 µs

throughput (pipelined burst, 50k orders)
  ~118k orders/sec
  ~59k round-trips/sec
```

In-process (no TCP) the matcher is much tighter — the lock-free
pipeline alone runs ~225 ns per round trip:

```
SPSC → matcher → SPSC, Market on empty                      → ~225 ns
SPSC → matcher → SPSC, Limit fully fills against 1 maker    → ~424 ns
matcher only, Limit walks 1000 price levels and fully fills → ~94 µs (≈10M trades/s)
```

The TCP cost (≈45 µs) is dominated by the kernel network stack,
not the matcher. A kernel-bypass NIC would shave most of it.

## What's built

| Subsystem | Status |
| --- | --- |
| Core types (`Price` fixed-point i64, `OrderId`, `Sequence`, `Side`, `Qty`, `Timestamp`) | ✅ slice 0 |
| In-memory order book (`BTreeMap` per side, `HashMap` index for cancel) | ✅ slice 1 |
| Matcher (Limit / Market / IOC; partial fills; lifecycle proptest) | ✅ slice 2 |
| Write-ahead log (CRC32C-framed records, fsync-on-commit, **byte-exact replay** test on 10k random orders) | ✅ slice 3 |
| Lock-free SPSC ring buffer (Acquire/Release with `// SAFETY:` proofs, **Miri-validated in CI**) | ✅ slice 4 |
| End-to-end engine (matcher on a dedicated thread, SPSC queues at the boundaries) | ✅ slice 5 |
| Hand-rolled binary wire protocol codec | ✅ slice 6 |
| TCP server + tokio-based gateway (`matchx-server`) | ✅ slice 7 |
| Load-gen client with RTT + throughput histogram (`matchx-client`) | ✅ slice 8 |
| Snapshots + recovery time bench | planned |
| Multi-tenant matching (MPSC at the gateway) | v2 |

## Architecture

```
   gateway thread (tokio)            matcher thread (dedicated)
        │                                  ▲
        │  Command::New{...}               │
        │  Command::Cancel{id}             │  poll
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

The matcher runs on a single dedicated thread — no contention to design
around inside it. The lock-free primitives are the SPSC queues at the
boundaries; that's where the `unsafe`, `// SAFETY:` proofs, and Miri
validation live.

## Quickstart

```bash
# Pinned toolchain — rustup picks 1.95.0 from rust-toolchain.toml.
rustup show

cargo test --workspace                 # unit + property + integration
cargo bench --workspace --no-run       # confirm benches build
cargo bench -p matchx-core             # actually run them
```

To run the headline replay test by name:

```bash
cargo test -p matchx-core --test replay
```

To run the end-to-end TCP demo:

```bash
# Terminal 1
cargo run --release -p matchx-server -- 127.0.0.1:9000

# Terminal 2: 5000 RTT samples + 50000-order throughput burst
cargo run --release -p matchx-client -- 127.0.0.1:9000 5000 50000
```

## Documentation

- [Architecture](docs/architecture.md)
- [Correctness guarantees](docs/correctness-guarantees.md)
- [Development log](docs/log.md)
- [v2 ideas (out of scope)](docs/v2-ideas.md)

## License

MIT — see [LICENSE](LICENSE).
