//! Order book trait and (forthcoming) implementations.
//!
//! In v1 the order book is a **single-writer** data structure maintained by
//! the matching thread. The "lock-free" boundary in the architecture is
//! the SPSC input queue feeding the matcher and the broadcast outbound
//! queues — that is where multi-threaded contention exists, and that is
//! where `unsafe` will be justified once benchmarks demand it. A
//! lock-free book would require epoch reclamation and memory-ordering
//! proofs for no measurable benefit when only one thread mutates it. See
//! [`docs/architecture.md`](../../../../docs/architecture.md).

use crate::types::{OrderId, Price, Qty, Side};

/// Operations an order book must support.
///
/// This is a placeholder for the bootstrap slice — the methods fix the API
/// surface but no implementation is provided yet. The order-book slice
/// will provide a concrete implementation.
pub trait OrderBook {
    /// Add a resting order to the book.
    fn add_order(&mut self, id: OrderId, side: Side, price: Price, qty: Qty);

    /// Cancel a resting order. Returns `true` if the order existed.
    fn cancel_order(&mut self, id: OrderId) -> bool;

    /// Best bid (highest buy price), if any.
    fn best_bid(&self) -> Option<Price>;

    /// Best ask (lowest sell price), if any.
    fn best_ask(&self) -> Option<Price>;
}

#[cfg(test)]
mod invariants {
    //! Order-book invariants the matcher and WAL slices must satisfy.
    //!
    //! Each test below names an invariant from
    //! [`docs/correctness-guarantees.md`](../../../../docs/correctness-guarantees.md)
    //! and is `#[ignore]`d until the relevant slice lands. Failing-fast
    //! bodies ensure that un-ignoring an unimplemented test surfaces
    //! immediately rather than silently passing.

    #![allow(
        clippy::assertions_on_constants,
        reason = "intentional placeholder failures for forthcoming slices"
    )]

    /// Within a price level, the order that arrived first matches first.
    /// Tie-breaking is by issued sequence number, never by wall-clock.
    #[test]
    #[ignore = "TODO(matcher slice): implement price-time priority test"]
    fn price_time_priority_preserved() {
        assert!(false, "TODO(matcher slice): implement price-time priority");
    }

    /// The sum of executed quantities equals the matched quantity. No
    /// over-fill, no under-fill.
    #[test]
    #[ignore = "TODO(matcher slice): implement fill conservation test"]
    fn sum_of_fills_equals_matched_quantity() {
        assert!(false, "TODO(matcher slice): implement fill conservation");
    }

    /// No order has negative quantity. (`Qty` is `u64`, but resting state
    /// transitions must also never decrement below zero.)
    #[test]
    #[ignore = "TODO(matcher slice): implement non-negativity test"]
    fn no_negative_quantities() {
        assert!(false, "TODO(matcher slice): implement non-negativity check");
    }

    /// Every emitted event carries a strictly-monotonic sequence number
    /// satisfying `s_{i+1} = s_i + 1`. No skips, no duplicates.
    #[test]
    #[ignore = "TODO(matcher slice): implement monotonic-sequence test"]
    fn emitted_sequence_numbers_are_strictly_monotonic() {
        assert!(
            false,
            "TODO(matcher slice): implement strict-monotonic sequence test"
        );
    }

    /// Replaying the WAL from a snapshot through to the tail produces a
    /// book whose state hash equals the live book's, byte-for-byte.
    #[test]
    #[ignore = "TODO(WAL slice): implement replay-equality test"]
    fn wal_replay_produces_byte_equal_state() {
        assert!(false, "TODO(WAL slice): implement WAL replay equality");
    }
}
