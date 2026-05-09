//! Fixed-point price representation.
//!
//! Prices are stored as `i64` with **8 implicit fractional digits**: a raw
//! value of `123_45000000` represents the price `123.45`. The chosen scale
//! matches widely-deployed crypto matching engines (e.g. Coinbase) and
//! gives a representable range of approximately ±9.22 × 10^10 in price
//! units — far beyond any realistic instrument price.
//!
//! No floating-point types are used anywhere in this module. Binary
//! floating-point cannot exactly encode most decimal prices and introduces
//! non-deterministic rounding under repeated arithmetic, which is
//! incompatible with byte-exact WAL replay.
//!
//! Arithmetic on [`Price`] **saturates** rather than wrapping or
//! panicking. Saturation preserves a total order under arbitrary inputs,
//! which is required for property tests of book invariants.

use core::fmt;

/// Number of fractional decimal digits encoded in a [`Price`].
pub const PRICE_SCALE_DIGITS: u32 = 8;

/// Multiplier corresponding to [`PRICE_SCALE_DIGITS`]: `10^8`.
pub const PRICE_SCALE: i64 = 100_000_000;

/// Errors that can arise when constructing a [`Price`].
#[derive(Debug, thiserror::Error, PartialEq, Eq, Clone, Copy)]
#[non_exhaustive]
pub enum PriceError {
    /// The provided value lies outside the representable range.
    #[error("price value out of representable range")]
    OutOfRange,
}

/// A fixed-point price with 8 fractional digits, stored as `i64`.
///
/// Construction:
/// - [`Price::from_raw`] takes the already-scaled integer.
/// - [`Price::from_units`] takes whole price units and is multiplied by
///   [`PRICE_SCALE`]; returns [`PriceError::OutOfRange`] on overflow.
///
/// Arithmetic uses saturating semantics — see module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Price(i64);

impl Price {
    /// The smallest representable price.
    pub const MIN: Self = Self(i64::MIN);
    /// The largest representable price.
    pub const MAX: Self = Self(i64::MAX);
    /// The zero price.
    pub const ZERO: Self = Self(0);

    /// Construct a `Price` from its raw (already-scaled) representation.
    #[inline]
    #[must_use]
    pub const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }

    /// Construct a `Price` from a number of whole units. Returns
    /// [`PriceError::OutOfRange`] if the multiplication overflows.
    #[inline]
    pub const fn from_units(units: i64) -> Result<Self, PriceError> {
        match units.checked_mul(PRICE_SCALE) {
            Some(raw) => Ok(Self(raw)),
            None => Err(PriceError::OutOfRange),
        }
    }

    /// Return the raw scaled `i64` representation.
    #[inline]
    #[must_use]
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Saturating addition. Overflow saturates to [`Price::MIN`] /
    /// [`Price::MAX`] rather than wrapping or panicking.
    #[inline]
    #[must_use]
    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    /// Saturating subtraction. Underflow saturates to [`Price::MIN`] /
    /// [`Price::MAX`].
    #[inline]
    #[must_use]
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
        /// Saturation must keep the result within the representable range
        /// for **all** input pairs. This is the safety property we rely on
        /// in book-invariant property tests.
        #[test]
        fn saturating_add_stays_in_range(a in any::<i64>(), b in any::<i64>()) {
            let p = Price::from_raw(a).saturating_add(Price::from_raw(b));
            prop_assert!(p >= Price::MIN);
            prop_assert!(p <= Price::MAX);
        }

        /// Saturating addition on `Price` must be bit-exact equivalent to
        /// `i64::saturating_add` on the raw representations.
        #[test]
        fn saturating_add_matches_i64(a in any::<i64>(), b in any::<i64>()) {
            let lhs = Price::from_raw(a).saturating_add(Price::from_raw(b)).raw();
            let rhs = a.saturating_add(b);
            prop_assert_eq!(lhs, rhs);
        }

        /// Saturating subtraction on `Price` must be bit-exact equivalent
        /// to `i64::saturating_sub` on the raw representations.
        #[test]
        fn saturating_sub_matches_i64(a in any::<i64>(), b in any::<i64>()) {
            let lhs = Price::from_raw(a).saturating_sub(Price::from_raw(b)).raw();
            let rhs = a.saturating_sub(b);
            prop_assert_eq!(lhs, rhs);
        }

        /// Total ordering: for every pair of prices, exactly one of
        /// `<`, `=`, `>` holds.
        #[test]
        fn ordering_is_total(a in any::<i64>(), b in any::<i64>()) {
            let p = Price::from_raw(a);
            let q = Price::from_raw(b);
            let lt = u8::from(p <  q);
            let eq = u8::from(p == q);
            let gt = u8::from(p >  q);
            prop_assert_eq!(lt + eq + gt, 1);
        }

        /// `Price` ordering is preserved across all bit patterns: it
        /// matches the ordering of the raw `i64` representations.
        #[test]
        fn ordering_matches_raw_i64(a in any::<i64>(), b in any::<i64>()) {
            prop_assert_eq!(
                Price::from_raw(a).cmp(&Price::from_raw(b)),
                a.cmp(&b)
            );
        }
    }

    #[test]
    fn from_units_overflow_is_reported() {
        // `i64::MAX / PRICE_SCALE + 1` units overflows.
        let overflowing = i64::MAX / PRICE_SCALE + 1;
        assert_eq!(Price::from_units(overflowing), Err(PriceError::OutOfRange));
    }

    #[test]
    fn display_formats_with_eight_fractional_digits() {
        // Raw 12_345_000_000 == 123.45 at 8-digit scale.
        assert_eq!(
            format!("{}", Price::from_raw(12_345_000_000)),
            "123.45000000"
        );
        assert_eq!(format!("{}", Price::from_raw(-1)), "-0.00000001");
        assert_eq!(format!("{}", Price::ZERO), "0.00000000");
    }
}
