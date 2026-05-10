# log

## slice 0
Workspace, pinned toolchain, CI (fmt / clippy / test / doc), property tests
on `Price` and `SequenceGenerator`. Trait stub for `OrderBook` with ignored
placeholder tests for the invariants we'll prove later.

## slice 1
Concrete `Book`: `BTreeMap<Price, VecDeque<Order>>` per side, `HashMap`
index for cancel.

```
Book::add    depths  0/100/1k/10k → ~99 / 61 / 130 / 554 ns
Book::cancel depths  1/100/1k/10k → ~69 / 122 / 185 / 199 ns  (front)
```

## slice 2
The matcher. `accept(NewOrder, &mut Vec<Event>)` — caller owns the
event buffer so the hot path doesn't allocate per call. Limit / Market
/ IOC, partial fills, walks multiple price levels. Duplicate-id
rejection doubles as v1's STP.

```
Matcher::accept (no cross)       depth 0/100/1k    → ~128/554/691 ns
Matcher::accept (walks N levels) N=1/10/100/1000   → ~143ns/496ns/7.9µs/94µs
```

## slice 3
WAL. Append-only segments, length-prefixed and CRC32C-framed records,
fsync-on-commit. Headline test: 10k random orders run through a live
matcher with WAL fsync per command; a fresh matcher replays the WAL;
live and replayed books are byte-equal AND live and replayed event
streams are sequence-by-sequence identical.

## slice 4
Lock-free SPSC ring buffer. Cache-padded head and tail; cached views
of the other side keep the remote cache line out of the hot path.
Acquire/Release pair with `// SAFETY:` proofs per `unsafe` block.
**Miri job in CI** validates the unsafe blocks, atomic ordering, and
a threaded producer/consumer pair on every push.

```
spsc push+pop steady state → ~5.3 ns per op
```

## slice 5
End-to-end engine. Two SPSC queues (`Command` in, `Event` out), the
matcher loop runs on a dedicated `matchx-matcher` OS thread spawned by
`Engine::start`. Busy-spins when both queues are quiet (low-latency
config; production would park). Shutdown via an `AtomicBool` checked
after every empty input poll, with a final drain for anything that
raced in.

Headline numbers — end-to-end round-trip latency from a gateway thread
pushing a `Command` to a consumer thread observing the corresponding
`Done`:

```
Market on empty book                         → ~225 ns  (~4.4M orders/s)
Limit fully fills against 1 resting maker    → ~424 ns
```

That's pure pipeline overhead through one lock-free queue, the matcher,
and another lock-free queue, single core, no allocation on the hot
path.

56 tests in matchx-core (4 new engine tests). All green.

## slice 6
Wire protocol codec (`matchx-protocol`). Length-prefixed binary frames,
1-byte version + 1-byte message type + fixed-size payload. Three
message types: `NewOrder` and `Cancel` client→server, `Execution` (one
per matcher `Event`) server→client. Hand-rolled — same reasoning as
the WAL codec; small fixed schema, hot path, no dependency on a
general-purpose serializer.

5 tests including round-trip proptests for both client and server
messages, plus targeted tests for unknown-version and truncated-body
rejection.

## slice 7
TCP server (`matchx-server`). Per connection: a fresh `Engine` split
into a producer/consumer pair plus a stop handle (new
`Engine::split` — `ManuallyDrop` + `ptr::read` to move fields out of
`self` so each end can live in its own tokio task). Two tasks per
connection: reader decodes `ClientMessage`s and pushes `Command`s onto
the engine; writer drains `Event`s and frames them as
`ServerMessage`s. `TCP_NODELAY` on. When the client disconnects, the
reader returns, the writer is aborted, the engine is stopped.

v1 limitation: one connection per engine. Multi-tenant matching needs
MPSC at the gateway boundary — parked under v2.

Integration tests (`tests/loopback.rs`): bind to ephemeral port,
spawn `serve` in a tokio task, connect a client, exchange orders,
verify the server's `ServerMessage` stream matches what the matcher
should emit. Two tests — full cross of two opposite limits, market
on empty book.

## slice 8 — next
Load-gen client (`matchx-client`) that drives sustained flow over
TCP and reports p50/p99/p99.9 latency histograms. That gives us the
end-to-end-over-network number for the README.
