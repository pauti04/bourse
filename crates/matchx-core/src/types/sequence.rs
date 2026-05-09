//! Monotonic sequence numbers and the generator that issues them.

use core::sync::atomic::{AtomicU64, Ordering};

/// A monotonic sequence number.
///
/// Sequence numbers are issued by [`SequenceGenerator`] and uniquely order
/// every event emitted by the engine. They are the canonical tie-breaker
/// for events with identical timestamps and serve as the index for WAL
/// records and market-data updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Sequence(u64);

impl Sequence {
    /// The zero sequence sentinel; never issued by [`SequenceGenerator`].
    pub const ZERO: Self = Self(0);

    /// Construct a sequence number from a raw `u64`.
    #[inline]
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Return the raw `u64` representation.
    #[inline]
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Issues strictly-monotonic [`Sequence`] numbers starting at `1`.
///
/// The generator is internally an [`AtomicU64`]. In the matchx
/// architecture it is owned and written by a single thread (the matching
/// thread), so the increment uses [`Ordering::Relaxed`] — no inter-thread
/// synchronization is required for the increment itself; downstream
/// publication establishes the necessary happens-before relationship.
#[derive(Debug, Default)]
pub struct SequenceGenerator {
    next: AtomicU64,
}

impl SequenceGenerator {
    /// Construct a new generator. The first value returned by
    /// [`Self::next`] will be `Sequence(1)`.
    #[inline]
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
        }
    }

    /// Return the next sequence number, monotonically incrementing the
    /// internal counter.
    #[inline]
    pub fn next(&self) -> Sequence {
        Sequence(self.next.fetch_add(1, Ordering::Relaxed))
    }

    /// Return the value that the next call to [`Self::next`] will produce,
    /// without advancing the counter.
    #[inline]
    pub fn peek(&self) -> Sequence {
        Sequence(self.next.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// `next()` is strictly monotonic with stride 1, starting at 1.
        #[test]
        fn next_is_strictly_monotonic_stride_one(n in 1usize..1024) {
            let r#gen = SequenceGenerator::new();
            let first = r#gen.next();
            prop_assert_eq!(first.get(), 1);
            let mut prev = first;
            for _ in 1..n {
                let cur = r#gen.next();
                prop_assert_eq!(cur.get(), prev.get() + 1);
                prev = cur;
            }
        }

        /// `peek()` is idempotent and does not advance the counter.
        #[test]
        fn peek_does_not_advance(n in 0usize..1024) {
            let r#gen = SequenceGenerator::new();
            for _ in 0..n {
                let _ = r#gen.next();
            }
            let p1 = r#gen.peek();
            let p2 = r#gen.peek();
            prop_assert_eq!(p1, p2);
        }

        /// `peek()` returns the value the next `next()` will issue.
        #[test]
        fn peek_predicts_next(n in 0usize..1024) {
            let r#gen = SequenceGenerator::new();
            for _ in 0..n {
                let _ = r#gen.next();
            }
            let predicted = r#gen.peek();
            let actual = r#gen.next();
            prop_assert_eq!(predicted, actual);
        }
    }
}
