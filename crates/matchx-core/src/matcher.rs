//! Matching engine.
//!
//! Single-threaded. Owns a [`Book`] and a [`SequenceGenerator`]. Callers
//! pass a `&mut Vec<Event>` so the matcher can append events without
//! allocating per call (the buffer lives across calls and is drained by
//! the publisher / WAL writer threads).
//!
//! Self-trade prevention: any incoming order whose id is already resting
//! in the book is rejected upfront. That's the simplest STP variant and
//! prevents self-trades by construction. Cancel-newest /
//! decrement-and-cancel STP modes are parked under v2.

use crate::order_book::Book;
use crate::types::{OrderId, Price, Qty, Sequence, SequenceGenerator, Side, Timestamp};

/// Variant of an incoming order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderKind {
    /// Cross what crosses; rest the rest at `price`.
    Limit {
        /// Limit price.
        price: Price,
    },
    /// Cross at any price; cancel the rest.
    Market,
    /// Like a Limit but cancel the rest instead of resting it.
    Ioc {
        /// Price ceiling (buy) / floor (sell).
        price: Price,
    },
}

/// New order presented to the matcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewOrder {
    /// Order id assigned by the gateway.
    pub id: OrderId,
    /// Side.
    pub side: Side,
    /// Total quantity.
    pub qty: Qty,
    /// Limit / Market / IOC.
    pub kind: OrderKind,
    /// Receive timestamp; opaque to the matcher.
    pub timestamp: Timestamp,
}

/// Why an order is no longer active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoneReason {
    /// All quantity matched.
    Filled,
    /// Explicit cancel.
    Cancelled,
    /// IOC remainder.
    Expired,
    /// Market with insufficient resting liquidity.
    NoLiquidity,
    /// Pre-acceptance reject (zero qty, duplicate id on rest).
    Rejected,
}

/// Event emitted by the matcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// Order entered the matcher. For a Limit that ends up resting, the
    /// `seq` is the time-priority seq used inside the book.
    Accepted {
        /// Order id.
        id: OrderId,
        /// Quantity as received.
        qty: Qty,
        /// Sequence number.
        seq: Sequence,
    },
    /// Trade between a taker and a maker.
    Trade {
        /// Aggressor.
        taker: OrderId,
        /// Resting order matched.
        maker: OrderId,
        /// Trade price (maker's).
        price: Price,
        /// Quantity.
        qty: Qty,
        /// Sequence number.
        seq: Sequence,
    },
    /// Order is no longer active.
    Done {
        /// Order id.
        id: OrderId,
        /// Quantity left when the order ended.
        leaves_qty: Qty,
        /// Why.
        reason: DoneReason,
        /// Sequence number.
        seq: Sequence,
    },
}

/// The matcher.
#[derive(Debug, Default)]
pub struct Matcher {
    book: Book,
    seq: SequenceGenerator,
}

impl Matcher {
    /// Empty matcher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrow the underlying book (for inspection / tests).
    pub fn book(&self) -> &Book {
        &self.book
    }

    /// Process a new order. Events are appended to `out` in emission
    /// order: `Accepted`, then any `Trade`s (with a `Done(Filled)` for
    /// each maker exhausted), then a final `Done` if the taker is no
    /// longer active.
    pub fn accept(&mut self, order: NewOrder, out: &mut Vec<Event>) {
        // Pre-acceptance gate: zero qty or duplicate id is rejected
        // outright. Duplicate-id rejection is also v1's STP — without it,
        // a taker could trade against its own resting order.
        if order.qty == Qty::ZERO || self.book.contains(order.id) {
            out.push(Event::Done {
                id: order.id,
                leaves_qty: order.qty,
                reason: DoneReason::Rejected,
                seq: self.seq.next(),
            });
            return;
        }

        // Receive ack. This is also the seq used for time-priority if
        // the order ends up resting (Limit with leftover).
        let receive_seq = self.seq.next();
        out.push(Event::Accepted {
            id: order.id,
            qty: order.qty,
            seq: receive_seq,
        });

        let opposite = order.side.opposite();
        let mut remaining = order.qty;

        while remaining > Qty::ZERO {
            let best = match opposite {
                Side::Buy => self.book.best_bid(),
                Side::Sell => self.book.best_ask(),
            };
            let Some(best_price) = best else { break };

            let crosses = match order.kind {
                OrderKind::Market => true,
                OrderKind::Limit { price } | OrderKind::Ioc { price } => match order.side {
                    Side::Buy => price >= best_price,
                    Side::Sell => price <= best_price,
                },
            };
            if !crosses {
                break;
            }

            let Some(take) = self.book.take_front(opposite, best_price, remaining) else {
                break;
            };

            out.push(Event::Trade {
                taker: order.id,
                maker: take.maker,
                price: best_price,
                qty: take.taken,
                seq: self.seq.next(),
            });

            if take.remaining == Qty::ZERO {
                out.push(Event::Done {
                    id: take.maker,
                    leaves_qty: Qty::ZERO,
                    reason: DoneReason::Filled,
                    seq: self.seq.next(),
                });
            }

            remaining = remaining.saturating_sub(take.taken);
        }

        if remaining == Qty::ZERO {
            out.push(Event::Done {
                id: order.id,
                leaves_qty: Qty::ZERO,
                reason: DoneReason::Filled,
                seq: self.seq.next(),
            });
            return;
        }

        match order.kind {
            OrderKind::Limit { price } => {
                // Duplicate id was caught upfront; book.add cannot fail here.
                let _ = self
                    .book
                    .add(order.id, order.side, price, remaining, receive_seq);
                // No "now resting" event — Accepted + Trades imply the
                // remaining qty is in the book under `receive_seq`.
            }
            OrderKind::Market => {
                out.push(Event::Done {
                    id: order.id,
                    leaves_qty: remaining,
                    reason: DoneReason::NoLiquidity,
                    seq: self.seq.next(),
                });
            }
            OrderKind::Ioc { .. } => {
                out.push(Event::Done {
                    id: order.id,
                    leaves_qty: remaining,
                    reason: DoneReason::Expired,
                    seq: self.seq.next(),
                });
            }
        }
    }

    /// Cancel a resting order by id. Emits a `Done(Cancelled)` carrying
    /// the resting quantity if the order existed, nothing otherwise.
    pub fn cancel(&mut self, id: OrderId, out: &mut Vec<Event>) {
        if let Some(leaves) = self.book.cancel(id) {
            out.push(Event::Done {
                id,
                leaves_qty: leaves,
                reason: DoneReason::Cancelled,
                seq: self.seq.next(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::panic,
        clippy::expect_used,
        reason = "tests assert via panic on setup"
    )]

    use super::*;

    fn id(n: u64) -> OrderId {
        OrderId::new(n)
    }
    fn p(n: i64) -> Price {
        Price::from_raw(n)
    }
    fn q(n: u64) -> Qty {
        Qty::new(n)
    }
    fn limit(id_: u64, side: Side, price: i64, qty: u64) -> NewOrder {
        NewOrder {
            id: id(id_),
            side,
            qty: q(qty),
            kind: OrderKind::Limit { price: p(price) },
            timestamp: Timestamp::EPOCH,
        }
    }
    fn market(id_: u64, side: Side, qty: u64) -> NewOrder {
        NewOrder {
            id: id(id_),
            side,
            qty: q(qty),
            kind: OrderKind::Market,
            timestamp: Timestamp::EPOCH,
        }
    }
    fn ioc(id_: u64, side: Side, price: i64, qty: u64) -> NewOrder {
        NewOrder {
            id: id(id_),
            side,
            qty: q(qty),
            kind: OrderKind::Ioc { price: p(price) },
            timestamp: Timestamp::EPOCH,
        }
    }

    #[test]
    fn limit_rests_when_no_cross() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.accept(limit(1, Side::Buy, 100, 5), &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], Event::Accepted { .. }));
        assert_eq!(m.book().best_bid(), Some(p(100)));
        assert_eq!(m.book().level_qty(Side::Buy, p(100)), q(5));
    }

    #[test]
    fn limit_full_fill_on_exact_cross() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.accept(limit(1, Side::Sell, 100, 5), &mut out);
        out.clear();
        m.accept(limit(2, Side::Buy, 100, 5), &mut out);

        // Accepted, Trade, Done(maker filled), Done(taker filled)
        assert_eq!(out.len(), 4);
        assert!(matches!(out[0], Event::Accepted { id: i, qty: x, .. } if i == id(2) && x == q(5)));
        assert!(matches!(
            out[1],
            Event::Trade { taker: t, maker: mk, price: pr, qty: x, .. }
            if t == id(2) && mk == id(1) && pr == p(100) && x == q(5)
        ));
        assert!(
            matches!(out[2], Event::Done { id: i, reason: DoneReason::Filled, .. } if i == id(1))
        );
        assert!(
            matches!(out[3], Event::Done { id: i, reason: DoneReason::Filled, .. } if i == id(2))
        );
        assert!(m.book().is_empty());
    }

    #[test]
    fn limit_partial_fill_then_rests() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.accept(limit(1, Side::Sell, 100, 3), &mut out);
        out.clear();
        m.accept(limit(2, Side::Buy, 100, 5), &mut out);

        // Accepted(2), Trade(2,1,3), Done(1 filled), and that's it —
        // remaining 2 rests under receive_seq.
        assert_eq!(out.len(), 3);
        assert!(matches!(
            out[1],
            Event::Trade { qty: x, .. } if x == q(3)
        ));
        assert_eq!(m.book().best_bid(), Some(p(100)));
        assert_eq!(m.book().level_qty(Side::Buy, p(100)), q(2));
    }

    #[test]
    fn limit_walks_the_book() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.accept(limit(1, Side::Sell, 100, 2), &mut out);
        m.accept(limit(2, Side::Sell, 101, 2), &mut out);
        m.accept(limit(3, Side::Sell, 102, 2), &mut out);
        out.clear();

        // Buy 5 @ 101 — should fill 2@100, 2@101, then rest 1 @ 101.
        m.accept(limit(4, Side::Buy, 101, 5), &mut out);

        let trades: Vec<_> = out
            .iter()
            .filter_map(|e| match e {
                Event::Trade { price, qty, .. } => Some((*price, *qty)),
                _ => None,
            })
            .collect();
        assert_eq!(trades, vec![(p(100), q(2)), (p(101), q(2))]);
        assert_eq!(m.book().best_bid(), Some(p(101)));
        assert_eq!(m.book().level_qty(Side::Buy, p(101)), q(1));
        assert_eq!(m.book().best_ask(), Some(p(102)));
    }

    #[test]
    fn limit_does_not_cross_through_limit_price() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.accept(limit(1, Side::Sell, 102, 5), &mut out);
        out.clear();

        // Buy at 100 against ask at 102 — no cross.
        m.accept(limit(2, Side::Buy, 100, 5), &mut out);
        // Just Accepted, then rests.
        assert_eq!(out.len(), 1);
        assert_eq!(m.book().best_bid(), Some(p(100)));
        assert_eq!(m.book().best_ask(), Some(p(102)));
    }

    #[test]
    fn market_buys_all_available_then_no_liquidity() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.accept(limit(1, Side::Sell, 100, 2), &mut out);
        m.accept(limit(2, Side::Sell, 200, 1), &mut out);
        out.clear();

        m.accept(market(3, Side::Buy, 10), &mut out);
        // Accepted, Trade@100/2, Done(1), Trade@200/1, Done(2), Done(3 NoLiquidity, leaves=7)
        let last = out.last().expect("done");
        assert!(matches!(
            last,
            Event::Done { reason: DoneReason::NoLiquidity, leaves_qty, .. } if *leaves_qty == q(7)
        ));
        assert!(m.book().is_empty());
    }

    #[test]
    fn market_on_empty_side_rejects_with_no_liquidity() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.accept(market(1, Side::Buy, 5), &mut out);
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], Event::Accepted { .. }));
        assert!(matches!(
            out[1],
            Event::Done { reason: DoneReason::NoLiquidity, leaves_qty, .. } if leaves_qty == q(5)
        ));
    }

    #[test]
    fn ioc_remainder_expires_not_rests() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.accept(limit(1, Side::Sell, 100, 2), &mut out);
        out.clear();

        m.accept(ioc(2, Side::Buy, 100, 5), &mut out);
        // Trade 2@100, then IOC remainder of 3 expires.
        let last = out.last().expect("done");
        assert!(matches!(
            last,
            Event::Done { reason: DoneReason::Expired, leaves_qty, .. } if *leaves_qty == q(3)
        ));
        assert!(m.book().is_empty()); // nothing rests
    }

    #[test]
    fn ioc_full_fill_emits_done_filled() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.accept(limit(1, Side::Sell, 100, 5), &mut out);
        out.clear();

        m.accept(ioc(2, Side::Buy, 100, 5), &mut out);
        let last = out.last().expect("done");
        assert!(matches!(
            last,
            Event::Done {
                reason: DoneReason::Filled,
                ..
            }
        ));
    }

    #[test]
    fn zero_qty_rejected() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.accept(limit(1, Side::Buy, 100, 0), &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            Event::Done {
                reason: DoneReason::Rejected,
                ..
            }
        ));
    }

    #[test]
    fn cancel_resting_order() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.accept(limit(1, Side::Buy, 100, 5), &mut out);
        out.clear();

        m.cancel(id(1), &mut out);
        assert_eq!(out.len(), 1);
        assert!(
            matches!(out[0], Event::Done { id: i, reason: DoneReason::Cancelled, .. } if i == id(1))
        );
        assert!(m.book().is_empty());
    }

    #[test]
    fn cancel_unknown_emits_nothing() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.cancel(id(99), &mut out);
        assert!(out.is_empty());
    }

    /// Duplicate id is rejected upfront — that's v1's STP. A taker whose id
    /// matches a resting order produces a single Done(Rejected) and never
    /// trades; the resting order is untouched.
    #[test]
    fn duplicate_id_rejected_no_self_trade() {
        let mut m = Matcher::new();
        let mut out = vec![];
        m.accept(limit(1, Side::Sell, 100, 5), &mut out);
        out.clear();

        m.accept(limit(1, Side::Buy, 100, 5), &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            Event::Done { id: i, reason: DoneReason::Rejected, .. } if i == id(1)
        ));
        assert_eq!(m.book().best_ask(), Some(p(100)));
        assert_eq!(m.book().level_qty(Side::Sell, p(100)), q(5));
    }
}

#[cfg(test)]
mod proptests {
    #![allow(clippy::expect_used, reason = "test setup")]

    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum Op {
        Limit {
            id: u64,
            buy: bool,
            price: i64,
            qty: u64,
        },
        Market {
            id: u64,
            buy: bool,
            qty: u64,
        },
        Ioc {
            id: u64,
            buy: bool,
            price: i64,
            qty: u64,
        },
        Cancel {
            id: u64,
        },
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            6 => (1u64..200, any::<bool>(), 90i64..110, 1u64..20)
                .prop_map(|(id, buy, price, qty)| Op::Limit { id, buy, price, qty }),
            2 => (1u64..200, any::<bool>(), 1u64..20)
                .prop_map(|(id, buy, qty)| Op::Market { id, buy, qty }),
            2 => (1u64..200, any::<bool>(), 90i64..110, 1u64..20)
                .prop_map(|(id, buy, price, qty)| Op::Ioc { id, buy, price, qty }),
            3 => (1u64..200).prop_map(|id| Op::Cancel { id }),
        ]
    }

    fn side(buy: bool) -> Side {
        if buy { Side::Buy } else { Side::Sell }
    }

    fn run(ops: &[Op]) -> Vec<Event> {
        let mut m = Matcher::new();
        let mut out = vec![];
        for o in ops {
            match *o {
                Op::Limit {
                    id,
                    buy,
                    price,
                    qty,
                } => m.accept(
                    NewOrder {
                        id: OrderId::new(id),
                        side: side(buy),
                        qty: Qty::new(qty),
                        kind: OrderKind::Limit {
                            price: Price::from_raw(price),
                        },
                        timestamp: Timestamp::EPOCH,
                    },
                    &mut out,
                ),
                Op::Market { id, buy, qty } => m.accept(
                    NewOrder {
                        id: OrderId::new(id),
                        side: side(buy),
                        qty: Qty::new(qty),
                        kind: OrderKind::Market,
                        timestamp: Timestamp::EPOCH,
                    },
                    &mut out,
                ),
                Op::Ioc {
                    id,
                    buy,
                    price,
                    qty,
                } => m.accept(
                    NewOrder {
                        id: OrderId::new(id),
                        side: side(buy),
                        qty: Qty::new(qty),
                        kind: OrderKind::Ioc {
                            price: Price::from_raw(price),
                        },
                        timestamp: Timestamp::EPOCH,
                    },
                    &mut out,
                ),
                Op::Cancel { id } => m.cancel(OrderId::new(id), &mut out),
            }
        }
        out
    }

    fn seq_of(e: &Event) -> Sequence {
        match *e {
            Event::Accepted { seq, .. } | Event::Trade { seq, .. } | Event::Done { seq, .. } => seq,
        }
    }

    proptest! {
        /// Every emitted event has seq exactly one greater than the prior.
        #[test]
        fn monotonic_sequence(ops in vec(op(), 0..200)) {
            let events = run(&ops);
            for w in events.windows(2) {
                let a = seq_of(&w[0]).get();
                let b = seq_of(&w[1]).get();
                prop_assert_eq!(b, a + 1, "seq jump between {:?} and {:?}", w[0], w[1]);
            }
        }

        /// Per-id state machine: an `Accepted(id, qty)` opens a lifecycle for
        /// that id; subsequent `Trade`s decrement open qty for both taker
        /// and maker; `Done(id, leaves)` closes the lifecycle and the
        /// recorded open qty must equal `leaves`. `Done(Rejected)` events
        /// are pre-acceptance rejects and do not affect open state.
        ///
        /// This single invariant covers fill conservation (no over- or
        /// under-fill), Accepted-before-Trade ordering, no Trade for an
        /// unknown maker, and correct `leaves_qty` on cancel.
        #[test]
        fn lifecycle_consistent(ops in vec(op(), 0..200)) {
            let events = run(&ops);
            let mut open: std::collections::HashMap<OrderId, u64> = std::collections::HashMap::new();
            for evt in &events {
                match *evt {
                    Event::Accepted { id, qty, .. } => {
                        prop_assert!(!open.contains_key(&id),
                            "Accepted while already open: {:?}", id);
                        open.insert(id, qty.get());
                    }
                    Event::Trade { taker, maker, qty, .. } => {
                        let t = open.get_mut(&taker).expect("taker open");
                        prop_assert!(*t >= qty.get(), "taker over-fill {:?}", taker);
                        *t -= qty.get();
                        let m = open.get_mut(&maker).expect("maker open");
                        prop_assert!(*m >= qty.get(), "maker over-fill {:?}", maker);
                        *m -= qty.get();
                    }
                    Event::Done { reason: DoneReason::Rejected, .. } => {
                        // Pre-acceptance reject; never opened.
                    }
                    Event::Done { id, leaves_qty, .. } => {
                        let rem = open.remove(&id).expect("Done for unknown id");
                        prop_assert_eq!(rem, leaves_qty.get(),
                            "leaves_qty mismatch for {:?}", id);
                    }
                }
            }
        }
    }
}
