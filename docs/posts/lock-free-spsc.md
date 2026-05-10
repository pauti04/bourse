# Designing the matchx lock-free SPSC queue

The matcher in [matchx][matchx] runs on a single dedicated OS thread.
Orders arrive from the gateway thread; events leave to the publisher
thread. The matcher itself is single-writer — we don't need any locking
*inside* it. The contention lives at the boundaries: gateway → matcher
and matcher → publisher.

That's exactly the shape of a single-producer, single-consumer
(**SPSC**) queue: one writer, one reader, and we want a hand-off that
costs as close to zero CPU cycles as possible. This post walks through
the design we ended up with: a bounded ring buffer with cache-padded
indices, "cached views" of the other side, an Acquire/Release pair for
memory ordering, and Miri-checked tests in CI.

The code is in [`crates/matchx-core/src/spsc.rs`][spsc]. It's about 200
lines including tests.

## The naïve version

A ring buffer needs three pieces: a fixed-size `buffer`, a `head` index
the consumer advances, and a `tail` index the producer advances. The
queue is empty when `head == tail` and full when `tail - head ==
capacity`. With a power-of-two capacity, `idx & mask` wraps for free.

A first-pass implementation looks something like:

```rust
struct Queue<T> {
    buffer: Box<[UnsafeCell<MaybeUninit<T>>]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    capacity: usize,
    mask: usize,
}
```

Producer pushes by reading `tail`, writing the slot at `tail & mask`,
storing `tail + 1`. Consumer pops symmetrically.

This is correct but slow. Every `try_push` and every `try_pop` reads
both atomics. The two indices end up on the same cache line; every
push invalidates the line in the consumer's cache, every pop
invalidates it in the producer's cache. Cache lines bounce between
cores and the queue starves on coherence traffic before doing any
useful work.

## Padding the indices

Step one is to put `head` and `tail` on different cache lines so they
don't share coherence traffic. On modern x86-64 and ARMv8 cache lines
are 64 bytes (sometimes 128 on Apple silicon), so we wrap each atomic
in a 64-byte-aligned struct:

```rust
#[repr(align(64))]
struct CachePadded<T>(T);

struct Inner<T> {
    tail: CachePadded<AtomicUsize>,  // producer writes; consumer reads
    head: CachePadded<AtomicUsize>,  // consumer writes; producer reads
    buffer: Box<[Slot<T>]>,
    mask: usize,
    capacity: usize,
}
```

Now a producer-side write to `tail` invalidates the cache line holding
`tail` but not the one holding `head`. The consumer can keep its read
of `head` (which it owns) hot in L1 and only pays the coherence cost
when it needs to *publish* a new `head` value to the producer.

## Caching the other side

The bigger win comes from realising that the producer doesn't need an
exact, up-to-the-microsecond view of `head`. It only needs to know
"is the queue full?" And if the cached view says "no, you have lots of
room," the answer can't be wrong by much — the consumer can only have
*added* room since we last looked.

Same on the consumer side. If the cached view of `tail` says "yes,
there are items waiting," the consumer can pop without re-reading
`tail`.

The producer keeps a private `cached_head` field. It only refreshes
that field from the atomic when the cached value indicates "queue
full":

```rust
pub fn try_push(&mut self, value: T) -> Result<(), T> {
    let tail = self.inner.tail.load(Ordering::Relaxed);

    // Quick path: cached head says we have room. Refresh from the
    // atomic only when the cache says full — keeps the consumer's
    // cache line out of the hot path most of the time.
    if tail.wrapping_sub(self.cached_head) >= self.inner.capacity {
        self.cached_head = self.inner.head.load(Ordering::Acquire);
        if tail.wrapping_sub(self.cached_head) >= self.inner.capacity {
            return Err(value);
        }
    }
    // ... write the slot, publish new tail
}
```

Under steady load, the cache check is the only thing the hot path
does. The producer never reads the consumer's cache line (and so never
forces a cross-core migration of it) until the queue actually fills up
— at which point we *want* to stall and check.

The consumer side mirrors this: it caches `tail` and only re-reads
when it thinks the queue is empty.

## Memory ordering

The synchronisation between producer and consumer is the standard
Acquire/Release pair, and reasoning about it is short enough to write
out:

- The producer writes the slot at `buffer[tail & mask]`.
- The producer then stores `tail + 1` with `Release`.
- The consumer loads `tail` with `Acquire`.
- That Acquire load synchronises-with the Release store, which means
  every memory access that was program-order-before the store on the
  producer's thread happens-before every access program-order-after
  the load on the consumer's thread. The slot write is sequenced
  before the store, so the slot read is sequenced after the slot
  write. The consumer is guaranteed to see the value the producer
  wrote.

Symmetric on the consumer side: the consumer reads the slot, then
stores `head + 1` with `Release` to publish "this slot is now free."
The producer's Acquire load on `head` (which only happens when the
cached value indicates full) synchronises with that Release.

The producer's load of its own `tail` and the consumer's load of its
own `head` are `Relaxed` — each side is the only writer of its own
index, so it can't observe an earlier value than what it just wrote.

Each `unsafe` block in the implementation has a `// SAFETY:` comment
naming the invariant it relies on. For the producer's slot write:

```rust
let slot = &self.inner.buffer[tail & self.inner.mask];
// SAFETY: `tail - head < capacity` here, so this slot is logically
// free. The consumer reads slot[i] only after seeing tail >= i+1
// via an Acquire load, so it cannot be touching this slot until we
// publish the new tail below.
unsafe {
    slot.cell.get().write(MaybeUninit::new(value));
}
self.inner.tail.store(tail.wrapping_add(1), Ordering::Release);
```

## `Send` and `Sync` discipline

The producer and consumer halves of the queue should be `Send` (you
move them onto their dedicated threads) but `!Sync` (you must not
share either half with another thread of the same role). That's
expressed with `PhantomData<Cell<()>>`:

```rust
pub struct Producer<T> {
    inner: Arc<Inner<T>>,
    cached_head: usize,
    /// Make the type !Sync without affecting Send.
    _not_sync: PhantomData<Cell<()>>,
}
```

`Cell<()>` is `Send` but not `Sync`; the `PhantomData` propagates that
to the wrapping struct without affecting the actual layout.

`Inner<T>: Sync` is granted via a manual `unsafe impl`, with `T: Send`
as the bound — every slot is owned by exactly one side at a time, so
moving a `T` from producer to consumer is sound under the `Send`
contract.

## Validating with Miri

Memory-ordering bugs are notoriously hard to find — they manifest only
under specific interleavings, often only on weakly-ordered hardware
(ARM), and often only at high core counts. The tests pass on x86 and
the implementation looks right, and you ship it, and three months
later somebody reproduces a corruption on an Apple silicon box.

Miri catches a lot of this at compile-test time. It interprets the
program one instruction at a time and tracks pointer provenance and
the C++20 memory model. A missing `Release` on the producer or
`Acquire` on the consumer shows up as a data race in the very first
threaded test.

CI runs Miri on the SPSC module on every push:

```yaml
miri:
  name: miri (lock-free modules)
  runs-on: ubuntu-latest
  env:
    MIRIFLAGS: -Zmiri-strict-provenance
  steps:
    - uses: actions/checkout@v4
    - run: rustup toolchain install nightly --component miri
    - uses: Swatinem/rust-cache@v2
      with:
        key: miri
    - run: cargo +nightly miri test --package matchx-core --lib spsc
```

Miri is much slower than native execution — easily 100× — so the
threaded test dials its iteration count down under `cfg(miri)`:

```rust
#[cfg(not(miri))]
const N: u32 = 100_000;
#[cfg(miri)]
const N: u32 = 200;
```

Two hundred round-trips through a producer/consumer pair are still
plenty for Miri to catch any racing access. The job runs in about a
minute on GitHub's Ubuntu runners.

## Numbers

Single-threaded `try_push` immediately followed by `try_pop` in tight
steady state, on M-series silicon:

```
spsc push+pop steady state → ~5.3 ns per op
```

End-to-end through the matcher engine (which has an SPSC on the input
side and another on the output side):

```
SPSC → matcher → SPSC, Market on empty                      → ~225 ns
SPSC → matcher → SPSC, Limit fully fills against 1 maker    → ~424 ns
```

The 225 ns is the lower bound on a round trip through this design.
With this queue the matcher pumps roughly 4 million order-events per
second per core; the limiting factor at higher rates is the matcher's
own work, not the queue.

## What we didn't do

- **Multi-producer.** The matchx gateway is currently single-connection
  per engine, so SPSC is enough. A real exchange wants MPSC at the
  ingress; that's parked under v2 and would replace this queue at the
  gateway boundary.
- **Wait-strategies.** The current matcher loop busy-spins when the
  input queue is empty. Production workloads might prefer a
  parking-then-spinning hybrid. The queue itself is wait-free; the
  parking would live in the consumer loop.
- **Bounded with overwrite.** We chose `try_push` returns `Err(value)`
  on full. Some designs prefer drop-oldest semantics. For a matching
  engine you want the producer to back off — losing orders silently is
  worse than back-pressure.
- **`get_unchecked`.** Indexing `buffer[i & mask]` is bounds-checked.
  In the hot path the optimiser usually elides the check, but a future
  pass could reach for `get_unchecked` with a SAFETY proof. Not worth
  the unsafe noise until benchmarks show it matters.

## Why this matters for matchx

The whole reason matchx can quote a ~225 ns end-to-end round trip is
that nothing on the hot path takes a lock or allocates. The SPSC
queues are how we hold that invariant at the inter-thread boundaries.
Everything else — the matcher, the WAL, the protocol codec — gets to
assume that "deliver this thing to the next stage" is a few atomic
operations and a slot write, not a syscall and a wakeup.

If you want to read the code, it's [`spsc.rs`][spsc]. The module is
short on purpose: most of the engineering is in the comments and
SAFETY proofs, not the lines of code.

[matchx]: https://github.com/pauti04/matchx
[spsc]: https://github.com/pauti04/matchx/blob/main/crates/matchx-core/src/spsc.rs
