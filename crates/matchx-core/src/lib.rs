//! `matchx-core` — the matching-engine library.
//!
//! No I/O is performed in this crate. Inputs arrive via lock-free queues
//! from `matchx-server`; outputs (executions, market-data deltas, WAL
//! records) are emitted into outbound queues consumed by other crates.
//!
//! See [`docs/architecture.md`](../../../docs/architecture.md) for the
//! system-level picture and
//! [`docs/correctness-guarantees.md`](../../../docs/correctness-guarantees.md)
//! for the invariants this crate maintains.

pub mod error;
pub mod matcher;
pub mod order_book;
pub mod types;
pub mod wal;
