# v2 ideas

Anything tempting that does not belong in v1 lands here. Do not implement
without first promoting to a v2 charter.

## Order types
- FOK (fill-or-kill).
- Post-only (rejects if the order would cross).
- Hidden / iceberg (display quantity vs. total quantity).
- Stop / stop-limit.

## Engine
- Multi-instrument (per-instrument matcher threads, per-symbol or shared WAL).
- Self-trade prevention (cancel-newest, cancel-oldest, decrement-and-cancel).
- Pre-trade risk checks: max order size, max notional, fat-finger limits.
- Modify-in-place (currently cancel + new).
- Market-by-order vs. market-by-price feed variants.
- Configurable price scale (currently hard-coded 8 fractional digits).

## Performance / infrastructure
- Custom allocator + huge pages.
- CPU pinning, `SCHED_FIFO`, isolated cores.
- Kernel-bypass NIC paths (DPDK, XDP, Solarflare TCPDirect).
- Snapshot via copy-on-write or persistent data structure rather than
  WAL replay.
- Hardware timestamping for ingress / egress instrumentation.
