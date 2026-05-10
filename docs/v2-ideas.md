# v2 ideas

Things I want but won't ship in v1.

Order types: FOK, post-only, hidden, iceberg, stop / stop-limit.

Engine: multi-instrument (one matcher per symbol), self-trade prevention,
pre-trade risk (max size / notional / fat-finger), modify-in-place,
market-by-order vs. market-by-price feed variants, configurable price scale.

Performance: kernel-bypass NIC (DPDK / XDP / TCPDirect), custom allocator
+ huge pages, pinned `SCHED_FIFO` matcher thread on an isolated core,
hardware timestamping at ingress / egress, snapshots via persistent data
structure rather than WAL replay.
