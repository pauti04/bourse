//! Globally-unique order identifier.

/// A unique identifier for an order.
///
/// `OrderId` is opaque to clients; the engine assigns IDs at acceptance
/// time and references them in subsequent `ExecutionReport` and
/// `OrderCancelReject` messages. IDs are only required to be unique for
/// the lifetime of the running engine instance and across replays from a
/// given snapshot — the WAL records them verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct OrderId(u64);

impl OrderId {
    /// Construct an `OrderId` from a raw `u64`. The caller is responsible
    /// for ensuring uniqueness within the engine instance.
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
}
