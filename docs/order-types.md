# Order types (v1)

| Type   | Behavior                                                           |
| ------ | ------------------------------------------------------------------ |
| Limit  | Rests on the book at the specified price; matches against the opposite side at prices at-or-better-than the limit. |
| Market | Matches against the opposite side at any price; never rests. Unfilled quantity is cancelled. |
| IOC    | Like a limit order, but unfilled quantity is cancelled instead of resting. |

**Out of scope for v1** (see [v2-ideas.md](v2-ideas.md)): FOK,
post-only, hidden, iceberg, stop / stop-limit, self-trade prevention.

## Cancellation

v1 supports **full cancel only**. Modify is implemented as **cancel +
new** — the client must reissue the order with new parameters. Modify-in-
place is parked for v2; it complicates priority semantics
(does a price-or-quantity change forfeit time priority?) and is not worth
the complexity in a v1 demo.
