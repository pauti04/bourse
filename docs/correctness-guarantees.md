# Correctness guarantees

These invariants are non-negotiable. Each is enforced by tests; the
property-test placeholders in
[`crates/matchx-core/src/order_book.rs`](../crates/matchx-core/src/order_book.rs)
and the WAL slice's replay test name them explicitly.

1. **Price-time priority.** Within a price level, the order that arrived
   first matches first. Ordering is by issued sequence number, never by
   wall-clock timestamp.
2. **Fill conservation.** The sum of executed quantities equals the
   matched quantity. No over-fill, no under-fill.
3. **Non-negativity.** No order has negative quantity, and resting state
   transitions never decrement a level below zero. (`Qty` is `u64`.)
   Prices may be negative in principle — some markets allow it — and the
   ordering / arithmetic on `Price` is total over the full `i64` range.
4. **Strict monotonicity of emitted sequence numbers.** Every emitted
   event carries a sequence number `s`, and consecutive events satisfy
   `s_{i+1} = s_i + 1`. No skips, no duplicates.
5. **WAL replay equality.** Replaying the WAL from a snapshot through to
   the tail produces a book whose state hash equals the live book's,
   byte-for-byte.
6. **No floats in price/quantity arithmetic.** Binary floating-point
   cannot exactly represent decimal prices, and accumulated rounding
   violates byte-exact replay. All price arithmetic uses fixed-point
   `i64`. No `f32` or `f64` may appear in `matchx-core` outside (someday)
   reporting code that is explicitly out of the WAL path.
7. **No silent overflow.** All integer arithmetic on engine types either
   saturates (with documented intent) or returns a `Result`. We never
   wrap, and we never panic on arithmetic.
8. **Durability before acknowledgement.** Every state-changing operation
   is durable (fsynced) in the WAL **before** the corresponding
   `ExecutionReport` or `OrderCancelReject` is sent to the client.
