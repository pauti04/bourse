/// Nanoseconds since the Unix epoch.
///
/// Signed so timestamp differences are trivially representable. The `i64`
/// range covers ~1678-08-12 to ~2262-04-11 in nanoseconds — fine.
///
/// Timestamps are recorded for downstream latency analysis; ordering
/// between orders is by [`super::Sequence`], not wall-clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    /// Unix epoch.
    pub const EPOCH: Self = Self(0);

    /// Wrap a nanosecond count.
    #[inline]
    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    /// Underlying nanoseconds.
    #[inline]
    pub const fn nanos(self) -> i64 {
        self.0
    }
}
