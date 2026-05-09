//! Nanosecond-precision timestamps.

/// A timestamp in nanoseconds since the Unix epoch.
///
/// Stored as a signed 64-bit integer so that timestamp differences are
/// trivially representable. The signed `i64` range covers approximately
/// 1678-08-12 to 2262-04-11 in nanoseconds — sufficient for any realistic
/// run of the engine.
///
/// The engine treats timestamps as opaque ordering metadata; they do not
/// participate in price-time priority *between* orders ([`super::Sequence`]
/// does — see [`docs/correctness-guarantees.md`](../../../../../docs/correctness-guarantees.md)).
/// They are recorded in the WAL and in market-data updates so downstream
/// consumers can reconstruct latency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Timestamp(i64);

impl Timestamp {
    /// The Unix epoch.
    pub const EPOCH: Self = Self(0);

    /// Construct a `Timestamp` from nanoseconds since the Unix epoch.
    #[inline]
    #[must_use]
    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    /// Return the underlying nanoseconds-since-epoch value.
    #[inline]
    #[must_use]
    pub const fn nanos(self) -> i64 {
        self.0
    }
}
