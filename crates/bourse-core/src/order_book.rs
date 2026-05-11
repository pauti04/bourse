//! In-memory single-writer order book.
//!
//! `BTreeMap<Price, VecDeque<Order>>` per side, plus an `OrderId` index for
//! O(log n) cancel. Cancel is O(log levels + L) where L is the number of
//! orders at that price level — VecDeque scans linearly. For realistic
//! depths L is small; an intrusive linked-list version is on the list when
//! benches show it matters.

use std::collections::{BTreeMap, HashMap, VecDeque};

use crate::types::{OrderId, Price, Qty, Sequence, Side};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Order {
    id: OrderId,
    qty: Qty,
    seq: Sequence,
}

type Level = VecDeque<Order>;

/// Limit order book. Single-writer, not thread-safe.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Book {
    bids: BTreeMap<Price, Level>,
    asks: BTreeMap<Price, Level>,
    index: HashMap<OrderId, (Side, Price)>,
}

impl Book {
    /// Empty book.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a resting order. Returns `false` if the id is already in the book
    /// or `qty` is zero — both indicate a gateway bug.
    pub fn add(&mut self, id: OrderId, side: Side, price: Price, qty: Qty, seq: Sequence) -> bool {
        if qty == Qty::ZERO || self.index.contains_key(&id) {
            return false;
        }
        let level = match side {
            Side::Buy => self.bids.entry(price).or_default(),
            Side::Sell => self.asks.entry(price).or_default(),
        };
        level.push_back(Order { id, qty, seq });
        self.index.insert(id, (side, price));
        true
    }

    /// Cancel by id. Returns the resting quantity that was removed, or
    /// `None` if the id isn't in the book — common when a client cancels
    /// an order the matcher already filled.
    pub fn cancel(&mut self, id: OrderId) -> Option<Qty> {
        let (side, price) = self.index.remove(&id)?;
        let map = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        let level = map.get_mut(&price)?;
        let pos = level.iter().position(|o| o.id == id)?;
        let qty = level.remove(pos)?.qty;
        if level.is_empty() {
            map.remove(&price);
        }
        Some(qty)
    }

    /// Highest buy price, if any.
    pub fn best_bid(&self) -> Option<Price> {
        self.bids.last_key_value().map(|(p, _)| *p)
    }

    /// Lowest sell price, if any.
    pub fn best_ask(&self) -> Option<Price> {
        self.asks.first_key_value().map(|(p, _)| *p)
    }

    /// Aggregate quantity resting at `(side, price)`. Zero if absent.
    pub fn level_qty(&self, side: Side, price: Price) -> Qty {
        let map = match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        };
        map.get(&price)
            .map(|l| l.iter().fold(Qty::ZERO, |a, o| a.saturating_add(o.qty)))
            .unwrap_or(Qty::ZERO)
    }

    /// Number of resting orders at `(side, price)`.
    pub fn level_len(&self, side: Side, price: Price) -> usize {
        let map = match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        };
        map.get(&price).map(VecDeque::len).unwrap_or(0)
    }

    /// Total resting orders.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// True if there are no resting orders.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// True if `id` rests in the book.
    pub fn contains(&self, id: OrderId) -> bool {
        self.index.contains_key(&id)
    }

    /// Sum the resting quantity on `side` whose price is at or better
    /// than `limit_price`, capped at `cap`. Stops as soon as the running
    /// sum reaches `cap`, so the worst case scans only enough levels to
    /// satisfy the FOK pre-check.
    ///
    /// "Better than" means: from the perspective of a taker that would
    /// cross with this side. A taker buying with `limit_price = P` walks
    /// asks ascending and accepts `ask <= P`. A taker selling with
    /// `limit_price = P` walks bids descending and accepts `bid >= P`.
    pub fn fillable_qty_at(&self, side: Side, limit_price: Price, cap: Qty) -> Qty {
        let mut sum = Qty::ZERO;
        match side {
            Side::Sell => {
                for (&ask_price, level) in self.asks.iter() {
                    if ask_price > limit_price {
                        break;
                    }
                    for o in level {
                        sum = sum.saturating_add(o.qty);
                        if sum >= cap {
                            return sum;
                        }
                    }
                }
            }
            Side::Buy => {
                for (&bid_price, level) in self.bids.iter().rev() {
                    if bid_price < limit_price {
                        break;
                    }
                    for o in level {
                        sum = sum.saturating_add(o.qty);
                        if sum >= cap {
                            return sum;
                        }
                    }
                }
            }
        }
        sum
    }

    /// Iterate every resting order. Yields bids first (price-ascending,
    /// time-priority within a level), then asks (same), so reinsertion in
    /// the same order reconstructs the level structure exactly.
    pub fn iter_resting(&self) -> impl Iterator<Item = RestingOrder> + '_ {
        let bids = self.bids.iter().flat_map(|(price, level)| {
            let p = *price;
            level.iter().map(move |o| RestingOrder {
                id: o.id,
                side: Side::Buy,
                price: p,
                qty: o.qty,
                seq: o.seq,
            })
        });
        let asks = self.asks.iter().flat_map(|(price, level)| {
            let p = *price;
            level.iter().map(move |o| RestingOrder {
                id: o.id,
                side: Side::Sell,
                price: p,
                qty: o.qty,
                seq: o.seq,
            })
        });
        bids.chain(asks)
    }
}

/// One resting order's worth of state — enough to round-trip through a
/// snapshot file and back into a `Book` via [`Book::add`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestingOrder {
    /// Order id.
    pub id: OrderId,
    /// Side.
    pub side: Side,
    /// Price.
    pub price: Price,
    /// Resting quantity.
    pub qty: Qty,
    /// Time-priority sequence.
    pub seq: Sequence,
}

impl Book {
    /// Take up to `want` quantity from the front of the `(side, price)`
    /// level. Returns `None` if no order rests there or `want` is zero.
    /// The maker order is reduced in place; if exhausted it is removed
    /// from the book and from the index. The matcher uses this to consume
    /// liquidity.
    pub fn take_front(&mut self, side: Side, price: Price, want: Qty) -> Option<Take> {
        if want == Qty::ZERO {
            return None;
        }
        let map = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };
        let level = map.get_mut(&price)?;
        let front = level.front_mut()?;

        let taken = if want.get() >= front.qty.get() {
            front.qty
        } else {
            want
        };
        let maker = front.id;
        let remaining = front.qty.saturating_sub(taken);

        if remaining == Qty::ZERO {
            level.pop_front();
            self.index.remove(&maker);
            if level.is_empty() {
                map.remove(&price);
            }
        } else {
            front.qty = remaining;
        }

        Some(Take {
            maker,
            taken,
            remaining,
        })
    }
}

/// Result of [`Book::take_front`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Take {
    /// The maker order whose front we consumed.
    pub maker: OrderId,
    /// Quantity actually taken (may be less than requested).
    pub taken: Qty,
    /// Quantity remaining on the maker after this take. Zero means the
    /// maker was exhausted and removed from the book.
    pub remaining: Qty,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> OrderId {
        OrderId::new(n)
    }
    fn s(n: u64) -> Sequence {
        Sequence::from_raw(n)
    }
    fn p(n: i64) -> Price {
        Price::from_raw(n)
    }
    fn q(n: u64) -> Qty {
        Qty::new(n)
    }

    #[test]
    fn empty() {
        let b = Book::new();
        assert_eq!(b.best_bid(), None);
        assert_eq!(b.best_ask(), None);
        assert!(b.is_empty());
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn one_buy_sets_best_bid() {
        let mut b = Book::new();
        assert!(b.add(id(1), Side::Buy, p(100), q(5), s(1)));
        assert_eq!(b.best_bid(), Some(p(100)));
        assert_eq!(b.best_ask(), None);
        assert_eq!(b.len(), 1);
        assert_eq!(b.level_qty(Side::Buy, p(100)), q(5));
    }

    #[test]
    fn one_sell_sets_best_ask() {
        let mut b = Book::new();
        assert!(b.add(id(1), Side::Sell, p(101), q(5), s(1)));
        assert_eq!(b.best_ask(), Some(p(101)));
        assert_eq!(b.best_bid(), None);
    }

    #[test]
    fn best_bid_is_max_buy_price() {
        let mut b = Book::new();
        b.add(id(1), Side::Buy, p(100), q(1), s(1));
        b.add(id(2), Side::Buy, p(101), q(1), s(2));
        b.add(id(3), Side::Buy, p(99), q(1), s(3));
        assert_eq!(b.best_bid(), Some(p(101)));
    }

    #[test]
    fn best_ask_is_min_sell_price() {
        let mut b = Book::new();
        b.add(id(1), Side::Sell, p(100), q(1), s(1));
        b.add(id(2), Side::Sell, p(99), q(1), s(2));
        b.add(id(3), Side::Sell, p(101), q(1), s(3));
        assert_eq!(b.best_ask(), Some(p(99)));
    }

    #[test]
    fn cancel_unknown_returns_false() {
        let mut b = Book::new();
        b.add(id(1), Side::Buy, p(100), q(1), s(1));
        assert!(b.cancel(id(999)).is_none());
        assert_eq!(b.len(), 1);
    }

    #[test]
    fn cancel_removes_order_and_collapses_empty_level() {
        let mut b = Book::new();
        b.add(id(1), Side::Buy, p(100), q(1), s(1));
        b.add(id(2), Side::Buy, p(101), q(1), s(2));
        assert_eq!(b.cancel(id(2)), Some(q(1)));
        assert_eq!(b.best_bid(), Some(p(100)));
        assert_eq!(b.level_len(Side::Buy, p(101)), 0);
    }

    #[test]
    fn duplicate_id_rejected() {
        let mut b = Book::new();
        assert!(b.add(id(1), Side::Buy, p(100), q(1), s(1)));
        assert!(!b.add(id(1), Side::Sell, p(101), q(1), s(2)));
        assert_eq!(b.len(), 1);
        assert_eq!(b.best_ask(), None);
    }

    #[test]
    fn zero_qty_rejected() {
        let mut b = Book::new();
        assert!(!b.add(id(1), Side::Buy, p(100), Qty::ZERO, s(1)));
        assert!(b.is_empty());
    }

    /// Within a price level, the front of the deque is the earliest insertion.
    /// The matcher will consume from the front, so this is the price-time
    /// priority guarantee for the data structure.
    #[test]
    fn time_priority_within_level() {
        let mut b = Book::new();
        b.add(id(1), Side::Buy, p(100), q(1), s(1));
        b.add(id(2), Side::Buy, p(100), q(1), s(2));
        b.add(id(3), Side::Buy, p(100), q(1), s(3));

        let front = |b: &Book| b.bids.get(&p(100)).and_then(|l| l.front()).map(|o| o.id);
        assert_eq!(front(&b), Some(id(1)));
        b.cancel(id(1));
        assert_eq!(front(&b), Some(id(2)));
        b.cancel(id(2));
        assert_eq!(front(&b), Some(id(3)));
        b.cancel(id(3));
        assert_eq!(front(&b), None);
    }
}

#[cfg(test)]
mod proptests {
    #![allow(clippy::expect_used, reason = "test setup; failures must surface")]

    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum Op {
        Add {
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
            8 => (1u64..200, any::<bool>(), -1000i64..1000, 1u64..100)
                .prop_map(|(id, buy, price, qty)| Op::Add { id, buy, price, qty }),
            2 => (1u64..200).prop_map(|id| Op::Cancel { id }),
        ]
    }

    fn run(ops: &[Op]) -> Book {
        let mut book = Book::new();
        let mut seq = 1u64;
        for o in ops {
            match *o {
                Op::Add {
                    id,
                    buy,
                    price,
                    qty,
                } => {
                    let side = if buy { Side::Buy } else { Side::Sell };
                    book.add(
                        OrderId::new(id),
                        side,
                        Price::from_raw(price),
                        Qty::new(qty),
                        Sequence::from_raw(seq),
                    );
                    seq += 1;
                }
                Op::Cancel { id } => {
                    book.cancel(OrderId::new(id));
                }
            }
        }
        book
    }

    proptest! {
        #[test]
        fn best_prices_are_extrema(ops in vec(op(), 0..200)) {
            let book = run(&ops);
            if let Some(bb) = book.best_bid() {
                for &px in book.bids.keys() {
                    prop_assert!(bb >= px);
                }
            }
            if let Some(ba) = book.best_ask() {
                for &px in book.asks.keys() {
                    prop_assert!(ba <= px);
                }
            }
        }

        /// `len` must equal the total number of resting orders across both maps.
        #[test]
        fn index_size_matches_orders(ops in vec(op(), 0..200)) {
            let book = run(&ops);
            let count: usize = book.bids.values().chain(book.asks.values()).map(VecDeque::len).sum();
            prop_assert_eq!(book.len(), count);
        }

        /// No empty `VecDeque` is ever left in either map.
        #[test]
        fn no_empty_levels(ops in vec(op(), 0..200)) {
            let book = run(&ops);
            for l in book.bids.values() { prop_assert!(!l.is_empty()); }
            for l in book.asks.values() { prop_assert!(!l.is_empty()); }
        }

        /// Every id in the index resolves to a level that contains it.
        #[test]
        fn index_consistent(ops in vec(op(), 0..200)) {
            let book = run(&ops);
            for (id, &(side, price)) in &book.index {
                let map = match side { Side::Buy => &book.bids, Side::Sell => &book.asks };
                let level = map.get(&price).expect("indexed level exists");
                prop_assert!(level.iter().any(|o| o.id == *id));
            }
        }
    }
}
