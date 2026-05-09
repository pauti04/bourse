/// Unique order identifier, opaque to clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct OrderId(u64);

impl OrderId {
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
}
