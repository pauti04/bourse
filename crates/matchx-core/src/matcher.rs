//! Matching logic.
//!
//! Bootstrap stub. Implementation lands in the matcher slice (after the
//! order-book data structure is implemented). The matcher runs on a single
//! dedicated OS thread, consuming inbound orders from a lock-free SPSC
//! queue and emitting executions and market-data deltas onto outbound
//! queues. See [`docs/architecture.md`](../../../../docs/architecture.md).
