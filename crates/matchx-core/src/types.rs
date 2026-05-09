//! Core domain types: identifiers, side, price, quantity, sequence numbers,
//! and timestamps.
//!
//! All numeric types are integers — there are no floats anywhere in this
//! module by deliberate design. Binary floating-point cannot exactly
//! represent decimal prices and accumulated rounding violates the byte-
//! exact replay guarantee. See
//! [`docs/correctness-guarantees.md`](../../../../docs/correctness-guarantees.md).

mod order_id;
mod price;
mod qty;
mod sequence;
mod side;
mod timestamp;

pub use order_id::OrderId;
pub use price::{PRICE_SCALE, PRICE_SCALE_DIGITS, Price, PriceError};
pub use qty::Qty;
pub use sequence::{Sequence, SequenceGenerator};
pub use side::Side;
pub use timestamp::Timestamp;
