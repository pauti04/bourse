/// Order quantity. Always non-negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Qty(u64);

impl Qty {
    /// Zero.
    pub const ZERO: Self = Self(0);

    /// Wrap a raw `u64`.
    #[inline]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// Underlying value.
    #[inline]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Saturating add.
    #[inline]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    /// Saturating sub — underflow saturates to zero.
    #[inline]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}
