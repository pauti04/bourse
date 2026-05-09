//! Order quantity. Always non-negative.

/// Order quantity, expressed as an unsigned integer.
///
/// Quantity is dimensionless — interpretation depends on the instrument
/// (lot size, contract size, satoshis, base-currency units, etc.). The
/// matching engine treats it as opaque integer arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Qty(u64);

impl Qty {
    /// The zero-quantity sentinel.
    pub const ZERO: Self = Self(0);

    /// Construct a `Qty` from a raw `u64`.
    #[inline]
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the raw `u64` representation.
    #[inline]
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Saturating addition.
    #[inline]
    #[must_use]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    /// Saturating subtraction. Underflow saturates to zero.
    #[inline]
    #[must_use]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}
