//! Monotonic sequence numbers.

use core::sync::atomic::{AtomicU64, Ordering};

/// Sequence number issued by [`SequenceGenerator`].
///
/// Sequences uniquely order every event the engine emits and act as the
/// canonical tie-breaker when two events share a wall-clock timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Sequence(u64);

impl Sequence {
    /// Sentinel; never issued by [`SequenceGenerator`].
    pub const ZERO: Self = Self(0);

    /// Wrap a raw `u64`.
    #[inline]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Underlying value.
    #[inline]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Issues strictly-monotonic [`Sequence`] numbers starting at `1`.
///
/// In matchx the only writer is the matcher thread, so the increment runs
/// at `Relaxed` ordering. Downstream publication establishes the necessary
/// happens-before relationship for observers.
#[derive(Debug, Default)]
pub struct SequenceGenerator {
    next: AtomicU64,
}

impl SequenceGenerator {
    /// Fresh generator. First [`next`](Self::next) returns `1`.
    #[inline]
    pub const fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
        }
    }

    /// Generator that will issue `start` next. Used on recovery to
    /// resume from a snapshot's seq marker.
    #[inline]
    pub const fn starting_at(start: Sequence) -> Self {
        let raw = start.get();
        let raw = if raw == 0 { 1 } else { raw };
        Self {
            next: AtomicU64::new(raw),
        }
    }

    /// Next sequence number.
    #[inline]
    pub fn next(&self) -> Sequence {
        Sequence(self.next.fetch_add(1, Ordering::Relaxed))
    }

    /// What [`next`](Self::next) will return without advancing.
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
        #[test]
        fn next_strictly_monotonic(n in 1usize..1024) {
            let g = SequenceGenerator::new();
            let mut prev = g.next();
            prop_assert_eq!(prev.get(), 1);
            for _ in 1..n {
                let cur = g.next();
                prop_assert_eq!(cur.get(), prev.get() + 1);
                prev = cur;
            }
        }

        #[test]
        fn peek_idempotent(n in 0usize..1024) {
            let g = SequenceGenerator::new();
            for _ in 0..n { let _ = g.next(); }
            prop_assert_eq!(g.peek(), g.peek());
        }

        #[test]
        fn peek_predicts_next(n in 0usize..1024) {
            let g = SequenceGenerator::new();
            for _ in 0..n { let _ = g.next(); }
            prop_assert_eq!(g.peek(), g.next());
        }
    }
}
