# matchx

A high-performance, crash-safe limit order book matching engine in Rust.
Single-instrument, price-time priority, FIX-inspired binary protocol over
TCP, UDP-multicast market data, and a write-ahead log with byte-exact replay.

This is **slice 0 / bootstrap** — the workspace, CI, and core type system are
in place; matching, protocol, and I/O land in subsequent slices.

## Quickstart

```bash
# 1. The toolchain is pinned; rustup will fetch the right version automatically.
rustup show

# 2. Build everything.
cargo build --workspace

# 3. Run all tests (unit + property tests).
cargo test --workspace

# 4. Lint.
cargo clippy --workspace --all-targets -- -D warnings

# 5. Render the docs.
cargo doc --workspace --no-deps --open
```

## Layout

| Crate              | Purpose                                           |
| ------------------ | ------------------------------------------------- |
| `matchx-core`      | Matching engine library (no I/O).                 |
| `matchx-protocol`  | FIX-inspired binary wire protocol codec (no I/O). |
| `matchx-server`    | TCP order entry + UDP multicast market data.      |
| `matchx-client`    | Test client and load generator.                   |
| `matchx-replay`    | Reconstructs the book from a snapshot + WAL tail. |
| `matchx-bench`     | `criterion` benchmarks.                           |

## Documentation

- [Architecture](docs/architecture.md)
- [Correctness guarantees](docs/correctness-guarantees.md)
- [Wire protocol](docs/wire-protocol.md)
- [Order types](docs/order-types.md)
- [Dependency justification](docs/dependencies.md)
- [Development log](docs/log.md)
- [v2 ideas (out of scope)](docs/v2-ideas.md)

```bash
cargo test --workspace
```

MIT.
