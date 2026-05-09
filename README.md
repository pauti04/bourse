# matchx

A limit order book matching engine in Rust. Single instrument,
price-time priority, length-prefixed binary protocol over TCP, UDP
multicast market data, write-ahead log with byte-exact replay.

WIP. See `docs/log.md` for what's done and what's next.

```bash
cargo test --workspace
```

MIT.
