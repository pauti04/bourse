# matchx

A limit order book matching engine in Rust. Single instrument,
price-time priority, write-ahead log with byte-exact replay, lock-free
single-producer single-consumer queues at the boundaries.

WIP — see [`docs/log.md`](docs/log.md) for what's done and what's next.

## Numbers

End-to-end round-trip latency on M-series silicon, single core, no
allocation on the hot path:

```
Market on empty book                         → ~225 ns
Limit fully fills against 1 resting maker    → ~424 ns
Limit walks 1000 price levels and fully fills → ~94 µs (≈10M trades/s)
```

The path measured is gateway thread → SPSC queue → matcher thread →
SPSC queue → consumer thread. Numbers are from `criterion --quick`;
fuller histograms (p50/p99/p99.9 under sustained load) come with the
TCP slice.

## What's built

| Subsystem | Status |
| --- | --- |
| Core types (`Price` fixed-point i64, `OrderId`, `Sequence`, `Side`, `Qty`, `Timestamp`) | ✅ slice 0 |
| In-memory order book (`BTreeMap` per side, `HashMap` index for cancel) | ✅ slice 1 |
| Matcher (Limit / Market / IOC; partial fills; lifecycle proptest) | ✅ slice 2 |
| Write-ahead log (CRC32C-framed records, fsync-on-commit, **byte-exact replay** test on 10k random orders) | ✅ slice 3 |
| Lock-free SPSC ring buffer (Acquire/Release with `// SAFETY:` proofs, **Miri-validated in CI**) | ✅ slice 4 |
| End-to-end engine (matcher on a dedicated thread, SPSC queues at the boundaries) | ✅ slice 5 |
| TCP gateway + binary wire protocol | next |
| Snapshots + recovery time bench | planned |

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

## Documentation

- [Architecture](docs/architecture.md)
- [Correctness guarantees](docs/correctness-guarantees.md)
- [Development log](docs/log.md)
- [v2 ideas (out of scope)](docs/v2-ideas.md)

## License

MIT — see [LICENSE](LICENSE).
