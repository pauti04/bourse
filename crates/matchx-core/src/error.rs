//! Crate-level error type.

/// All errors emitted by `matchx-core`.
///
/// Marked `#[non_exhaustive]` so future slices can add variants without
/// breaking downstream `match` exhaustiveness.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Placeholder variant exposed by the bootstrap slice; will be replaced
    /// by concrete variants as subsystems land.
    #[error("unimplemented matchx-core operation")]
    Unimplemented,
}
