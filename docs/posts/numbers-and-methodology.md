# bourse numbers, and how they were measured

The bourse README opens with a code block of round-trip latencies and
throughput numbers. This post says what those numbers actually
measure, what they don't, and where you should be skeptical.

## The headline numbers

From the [README][readme]:

```
in-process round-trip            ~225 ns
TCP round-trip (loopback) p50    ~45 µs
TCP round-trip (loopback) p99    ~109 µs
TCP throughput (pipelined)       ~118 k orders/sec
matcher walks 1000 levels        ~94 µs (≈10 M trades/sec)
WAL group commit speedup         187× to 245×
```

Each of those came out of a specific bench under specific conditions.
None of them is a synthetic ceiling; each is the result of real code
running.

## Where the bench code lives

| Number | Bench | Notes |
| --- | --- | --- |
| in-process round-trip | [`benches/engine.rs`][engine-bench] | gateway thread → SPSC → matcher thread → SPSC → consumer thread |
| TCP RTT | [`crates/bourse-client/src/main.rs`][client] | RTT mode: rest a Sell, time send-Buy → `Done(Filled)`, no pipelining |
| TCP throughput | same | throughput mode: encode `n` orders into one buffer, write once, drain all responses, wall-clock |
| matcher walks N levels | [`benches/matcher.rs`][matcher-bench] | `accept(Limit, fully fills against N resting makers)` |
| WAL group commit | [`benches/wal_commit.rs`][wal-bench] | fsync-per-record vs one-fsync-per-batch at N = 1, 8, 64, 256 |
| SPSC push+pop | [`benches/spsc.rs`][spsc-bench] | tight steady-state push then pop in one thread |

## Hardware

Two distinct boxes, both reported.

- **macOS dev box** — Apple M-series silicon, APFS, default macOS
  scheduler. fsync actually fsyncs through to physical media. Numbers
  here are stable run-to-run within ~5%.
- **Ubuntu CI runner** — `actions/runner-images` `ubuntu-latest` (AMD
  EPYC 7763 at the time of writing), shared infrastructure, `/tmp` is
  tmpfs. fsync to tmpfs is ~10× cheaper than to a real disk; that
  shows up clearly in `wal_commit`. Run-to-run variance on shared CI
  hardware is 2–10×, so absolute numbers off the runner are a sanity
  check, not a measurement.

The `bench numbers` CI job runs every bench in `--quick` mode on
`ubuntu-latest` and uploads `bench_numbers.md` as a downloadable
artifact on every PR.

## Mode and compiler

All numbers are release builds (`cargo bench` defaults to release).
The release profile is configured in the workspace `Cargo.toml`:

```toml
[profile.release]
codegen-units = 1
lto           = "fat"
panic         = "abort"
opt-level     = 3
debug         = "line-tables-only"
```

Single codegen unit + fat LTO matter for the bench numbers — without
them the matcher's hot path doesn't inline through the SPSC's
`try_pop`, and the round-trip number creeps up by ~30%.

## What "RTT" measures, exactly

For the TCP RTT bench (the load-gen client, sequential mode):

1. Client opens one TCP connection, `set_nodelay(true)`.
2. Warmup: 100 iterations of (rest a Sell, send a Buy, drain until the
   Buy's `Done(Filled)`). Caches and TCP slow-start are out of the way
   by the end.
3. Measurement: 10 000 more iterations. For each one:
   1. Encode and `write_all` a `Sell` order. Drain server frames until
      `Accepted(sell)` arrives.
   2. **Start the timer.** Encode and `write_all` the `Buy`. Drain
      until `Done(Filled, buy)`.
   3. Stop the timer; record nanoseconds.
4. Sort the latencies; report p50, p90, p99, p99.9, max.

What the timer brackets: `Buy` framing, `write_all`, kernel TCP path,
server reader, decode, hub MPSC push, matcher dispatch, matcher
`accept`, hub event publish, server writer, kernel TCP path, client
read, decode, comparison. Everything end-to-end **except** the Sell
setup leg.

What it does *not* measure: queueing under heavy load. The client
sends one Buy at a time, waits, sends another. That's the right way
to measure single-order RTT; it's the wrong way to measure tail
latency under sustained pressure. For tail-under-load you'd want
open-loop measurement at a fixed offered rate; we don't have that
yet.

## Why throughput is reported separately

The same client also has a "throughput" mode: encode `n` orders into
one big buffer, `write_all` once, then drain every response on a
separate task. Wall clock divided by `n` gives the apparent
throughput; it's about **118 k orders/sec** on macOS loopback.

The per-order latencies in this mode are *meaningless in isolation*
because they include time spent waiting in the kernel TCP buffer
behind earlier orders. A naïve measurement would report something like
"p50 = 275 ms" on a 100 k-order burst, which sounds catastrophic but
is just queueing. Latency under load needs the open-loop measurement
above; throughput is the wall-clock number; don't conflate them.

## What the in-process round-trip measures

The engine bench (`benches/engine.rs`) runs entirely inside one
process: a gateway thread `try_push`es onto a single SPSC; the matcher
thread on a dedicated OS thread `try_pop`s, runs `accept`, and pushes
events onto another SPSC; the bench thread spins on `try_pop` until
the corresponding `Done` is observed.

This is a clean read on the engine's *internal* cost: SPSC push, hand-
off to another core, matcher work, SPSC publish, hand-off back, SPSC
pop. ~225 ns on M-series, ~227 ns on EPYC 7763. The two converge
because once the syscalls and the kernel network stack are out of the
way, the engine's hot path is bound by the same things on either
chip — atomic operations on shared cache lines and the matcher's own
arithmetic.

The TCP path adds ~45 µs to that because of kernel TCP. A kernel-bypass
NIC (DPDK, XDP, Solarflare TCPDirect) would close most of that gap;
that's parked under v2 in `docs/v2-ideas.md`.

## What the matcher's "10 M trades/sec" actually means

The matcher bench measures `Matcher::accept(Limit, fully fills against
N resting makers)`:

- N = 1: ~143 ns per accept (one trade emitted)
- N = 10: ~496 ns (~50 ns per trade)
- N = 100: ~7.9 µs (~80 ns per trade)
- N = 1000: ~94 µs (~94 ns per trade)

The "10 M trades/sec ceiling" is `1 / 94 ns ≈ 10.6 M`. That's the
matcher's per-trade work *only* — no I/O, no queueing, no protocol
encoding, no logging. It's the upper bound on what the matcher
itself can produce; reality is slower because everything else has
to happen too.

## Why "245× speedup" needs context

The WAL group commit bench compares two cadences at four batch sizes
(1, 8, 64, 256):

```
fsync per record:   ~3.4 / ~25.9 / ~203.7 / ~951.0 ms   (macOS APFS)
group commit:       ~3.5 / ~3.8  / ~3.6   / ~3.9   ms
```

At batch = 256, group commit's 3.9 ms is 245× faster than
fsync-per-record's 951 ms. **But** that's because the fsync-per-record
column scales roughly linearly with the batch (each record pays the
~3.5 ms macOS-APFS fsync cost), while group commit pays one fsync per
batch regardless. The ratio grows with batch size; at batch = 1 the
two are identical (one fsync, one record).

On Ubuntu CI tmpfs the absolute numbers are 10× smaller (tmpfs fsync
is in-memory) and the ratio is ~187× at batch = 256 — same shape, same
story, scaled differently because the disk story is different.

What this tells you: **a real engine wants group commit**. The
matcher's input queue should naturally batch — drain N commands,
append all, fsync once, ack all. The latency cost is one fsync paid by
the *last* record in the batch; per-record throughput drops by the
batch size.

## What we don't claim

A few things bourse's numbers explicitly do *not* claim:

- **Production-tuned latency tail.** No SCHED_FIFO, no isolated cores,
  no NUMA pinning, no huge pages, no kernel-bypass NIC. Default
  scheduler, default allocator, default Linux. Real exchanges go
  considerably further; that work is parked under v2.
- **Sustained-load tail latency.** All published p99 / p99.9 numbers
  are from the criterion benches (which are bound-by-design — they
  don't hit the system with sustained pressure) or the load-gen
  client's sequential RTT mode (which is by construction lightly
  loaded). For a real "p99.9 under sustained 100 k orders/sec for
  one hour" measurement you'd want a different harness.
- **Multi-instrument.** Single matcher, single instrument. The Hub is
  multi-tenant for connections, not for instruments. A multi-instrument
  engine would shard by symbol, one matcher thread per shard.
- **Real fault-injection.** The byte-exact replay test is run on
  randomly-generated workloads, not on workloads constructed to find
  reordering bugs in matcher state transitions. That's the next round
  of testing if this codebase ever ships.

## What you should believe

The numbers are reproducible. The benches are checked in; the
runners are documented; the methodology above describes exactly what
each one measures. Run them on your own hardware and compare. The
ratios — group commit vs fsync-per-record, depth-N vs depth-1, in-
process vs TCP — are what you should pay attention to; they're what
holds across platforms. The absolute values move with hardware and
that's the way it should be.

[readme]: ../../README.md
[engine-bench]: ../../crates/bourse-core/benches/engine.rs
[matcher-bench]: ../../crates/bourse-core/benches/matcher.rs
[spsc-bench]: ../../crates/bourse-core/benches/spsc.rs
[wal-bench]: ../../crates/bourse-core/benches/wal_commit.rs
[client]: ../../crates/bourse-client/src/main.rs
