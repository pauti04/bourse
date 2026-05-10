//! matchx matching engine. No I/O — inputs and outputs are queues
//! the rest of the workspace owns.

pub mod engine;
pub mod hub;
pub mod matcher;
pub mod order_book;
pub mod snapshot;
pub mod spsc;
pub mod types;
pub mod wal;
