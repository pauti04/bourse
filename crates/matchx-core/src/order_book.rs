//! Order book.

use crate::types::{OrderId, Price, Qty, Side};

/// What an order book must support. Concrete impls live in submodules
/// once they exist; for now the trait fixes the API.
pub trait OrderBook {
    /// Add a resting order.
    fn add_order(&mut self, id: OrderId, side: Side, price: Price, qty: Qty);
    /// Remove a resting order. Returns `true` if it existed.
    fn cancel_order(&mut self, id: OrderId) -> bool;
    /// Highest buy price, if any.
    fn best_bid(&self) -> Option<Price>;
    /// Lowest sell price, if any.
    fn best_ask(&self) -> Option<Price>;
}

#[cfg(test)]
mod invariants {
    //! Placeholders for invariants the matcher and WAL slices must hold.

    #![allow(clippy::assertions_on_constants)]

    #[test]
    #[ignore = "matcher slice"]
    fn price_time_priority() {
        assert!(false, "todo");
    }

    #[test]
    #[ignore = "matcher slice"]
    fn fill_conservation() {
        assert!(false, "todo");
    }

    #[test]
    #[ignore = "matcher slice"]
    fn no_negative_qty() {
        assert!(false, "todo");
    }

    #[test]
    #[ignore = "matcher slice"]
    fn monotonic_sequence() {
        assert!(false, "todo");
    }

    #[test]
    #[ignore = "wal slice"]
    fn wal_replay_byte_equal() {
        assert!(false, "todo");
    }
}
