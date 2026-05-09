//! Fixed-point price.
//!
//! Prices are `i64` scaled by `10^8`: a raw value of `12_345_000_000` means
//! `123.45`. The scale matches Coinbase's wire format and gives ~±9.2e10 in
//! price units, which is plenty. No floats — IEEE-754 can't exactly represent
//! most decimal prices, and accumulated rounding would break the byte-exact
//! WAL replay we rely on.
//!
//! Arithmetic saturates rather than wrapping or panicking, so total ordering
//! is preserved over the whole `i64` range. That matters for the property
//! tests of book invariants.

use core::fmt;

/// Number of fractional decimal digits in a [`Price`].
pub const PRICE_SCALE_DIGITS: u32 = 8;

/// `10^PRICE_SCALE_DIGITS`.
pub const PRICE_SCALE: i64 = 100_000_000;

/// Errors from constructing a [`Price`].
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum PriceError {
    /// Value lies outside the representable range.
    OutOfRange,
}

impl fmt::Display for PriceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("price value out of representable range")
    }
}

impl core::error::Error for PriceError {}

/// Fixed-point price with 8 fractional digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Price(i64);

impl Price {
    /// Smallest representable price.
    pub const MIN: Self = Self(i64::MIN);
    /// Largest representable price.
    pub const MAX: Self = Self(i64::MAX);
    /// Zero.
    pub const ZERO: Self = Self(0);

    /// Wrap an already-scaled `i64`.
    #[inline]
    pub const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }

    /// Build from whole units, multiplied by [`PRICE_SCALE`]. Returns
    /// [`PriceError::OutOfRange`] on overflow.
    #[inline]
    pub const fn from_units(units: i64) -> Result<Self, PriceError> {
        match units.checked_mul(PRICE_SCALE) {
            Some(raw) => Ok(Self(raw)),
            None => Err(PriceError::OutOfRange),
        }
    }

    /// Underlying scaled `i64`.
    #[inline]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Saturating add — overflow saturates to `MIN` / `MAX`.
    #[inline]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    /// Saturating sub — underflow saturates to `MIN` / `MAX`.
    #[inline]
    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }
}

impl fmt::Display for Price {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let abs = self.0.unsigned_abs();
        let scale = PRICE_SCALE as u64;
        let whole = abs / scale;
        let frac = abs % scale;
        let sign = if self.0 < 0 { "-" } else { "" };
        write!(
            f,
            "{sign}{whole}.{frac:0width$}",
            width = PRICE_SCALE_DIGITS as usize
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn saturating_add_in_range(a in any::<i64>(), b in any::<i64>()) {
            let p = Price::from_raw(a).saturating_add(Price::from_raw(b));
            prop_assert!(p >= Price::MIN && p <= Price::MAX);
        }

        #[test]
        fn saturating_add_matches_i64(a in any::<i64>(), b in any::<i64>()) {
            prop_assert_eq!(
                Price::from_raw(a).saturating_add(Price::from_raw(b)).raw(),
                a.saturating_add(b),
            );
        }

        #[test]
        fn saturating_sub_matches_i64(a in any::<i64>(), b in any::<i64>()) {
            prop_assert_eq!(
                Price::from_raw(a).saturating_sub(Price::from_raw(b)).raw(),
                a.saturating_sub(b),
            );
        }

        #[test]
        fn ordering_total(a in any::<i64>(), b in any::<i64>()) {
            let p = Price::from_raw(a);
            let q = Price::from_raw(b);
            let n = u8::from(p < q) + u8::from(p == q) + u8::from(p > q);
            prop_assert_eq!(n, 1);
        }

        #[test]
        fn ordering_matches_raw(a in any::<i64>(), b in any::<i64>()) {
            prop_assert_eq!(Price::from_raw(a).cmp(&Price::from_raw(b)), a.cmp(&b));
        }
    }

    #[test]
    fn from_units_overflow() {
        let n = i64::MAX / PRICE_SCALE + 1;
        assert_eq!(Price::from_units(n), Err(PriceError::OutOfRange));
    }

    #[test]
    fn display() {
        assert_eq!(
            format!("{}", Price::from_raw(12_345_000_000)),
            "123.45000000"
        );
        assert_eq!(format!("{}", Price::from_raw(-1)), "-0.00000001");
        assert_eq!(format!("{}", Price::ZERO), "0.00000000");
    }
}
