# correctness

Within a price level, the order that arrived first matches first. Tie-
breaking is by issued sequence number, never by wall-clock timestamp.

Sum of fills equals matched quantity — no over-fill, no under-fill. No
order or resting level ever goes negative; `Qty` is `u64`. Prices may be
negative (some markets allow it) and arithmetic on `Price` is total over
the full `i64` range.

Every emitted event carries a sequence number `s`, with consecutive events
satisfying `s_{i+1} = s_i + 1`. No skips, no duplicates.

Replaying the WAL from a snapshot through to the tail produces a book whose
state hash is byte-equal to the live book's. An integration test enforces
this on a randomised stream of ≥10k orders.

No floats anywhere on the price/quantity path — fixed-point `i64`. No
silent overflow — saturate or `Result`. Every state-changing op is durable
in the WAL before it is acknowledged.
