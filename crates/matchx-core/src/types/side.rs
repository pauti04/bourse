//! Order side: buy or sell.

/// The side of the book an order rests on or trades against.
///
/// Encoded as a one-byte enum to make wire and on-disk representations
/// trivial — see [`docs/wire-protocol.md`](../../../../../docs/wire-protocol.md)
/// for the byte mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Side {
    /// A bid: willing to buy at or below a given price.
    Buy = 1,
    /// An offer: willing to sell at or above a given price.
    Sell = 2,
}

impl Side {
    /// Return the opposite side.
    #[inline]
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_is_involutive() {
        assert_eq!(Side::Buy.opposite().opposite(), Side::Buy);
        assert_eq!(Side::Sell.opposite().opposite(), Side::Sell);
    }
}
