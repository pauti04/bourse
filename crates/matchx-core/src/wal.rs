//! Write-ahead log.
//!
//! Bootstrap stub. Implementation lands in the WAL slice. Design:
//! append-only segment files, fsync-on-commit, **CRC32C** (Castagnoli, with
//! hardware acceleration on x86-64 and ARMv8) per record, periodic
//! snapshots. The WAL is the durability boundary: every state-changing op
//! is durable in the log before it is acknowledged to the client.
