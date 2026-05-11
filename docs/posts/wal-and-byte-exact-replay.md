# Crash-safe matching: bourse's WAL and byte-exact replay

The matcher in [bourse][bourse] makes decisions that can't be unmade.
A trade is a trade — the maker and taker have agreed, the executions
have been (or will be) reported, and the broader system has wired
positions and balances on the assumption that what happened, happened.

If the engine crashes mid-flight, recovery has to reconstruct exactly
the state that was acknowledged to clients. Not "approximately the
same book." The same book, byte for byte, with the same resting
orders at the same prices in the same time-priority order with the
same internal sequence numbers. Anything else and the next event
emitted post-recovery would diverge from what observers think
happened.

This post walks through how bourse's WAL is built and why an
integration test on 10,000 random orders asserts the live and
replayed engine state are bit-equal — including with a snapshot
taken halfway through.

The code is in [`crates/bourse-core/src/wal.rs`][wal] and
[`crates/bourse-core/src/snapshot.rs`][snapshot]. The headline tests
are [`tests/replay.rs`][replay-test] and
[`tests/snapshot_recovery.rs`][snapshot-test].

## Inputs, not outputs

A matching engine has two natural things you could log: the inputs
(the `NewOrder` and `Cancel` commands the matcher consumed) or the
outputs (the `Trade`, `Accepted`, `Done` events it emitted). Real
exchanges log both — outputs go on the public market-data feed and
to clients as `ExecutionReport`s; inputs go to the WAL.

For recovery, inputs are what you want, for two reasons:

1. **Smaller.** One input typically produces multiple events. The
   input log is consistently the smaller of the two streams.
2. **Trustworthy by construction.** If you log outputs and replay by
   re-applying them to a fresh book, you have to trust that the
   output log is internally consistent. If you log inputs and replay
   by re-running them through the matcher, the replayed events come
   out of the same code that produced the original events — any bug
   would have produced the same outputs the first time.

So bourse's WAL records exactly two things: `NewOrder(NewOrder)` and
`Cancel(OrderId)`. They reuse the matcher's `NewOrder` type directly,
so the wire model is the matcher's input model. No translation, no
schema drift.

## Frame format

Each WAL segment file looks like this:

```text
[magic u32 LE = "MXCW"] [version u8] [pad 3]
[record]+

record:
  [len u32 LE] [crc32c u32 LE] [payload (len bytes)]

payload:
  [record_version u8] [record_type u8] [type-specific bytes…]
```

A few choices worth pulling out:

- **Length-prefixed before CRC.** The length is metadata for finding
  the next record; the CRC covers the actual record bytes. A flipped
  bit in the length field will trip read_exact — the file is
  garbled and we shouldn't try to interpret what we read. A flipped
  bit in the payload will fail the CRC and surface as
  `WalError::CrcMismatch` so the caller knows recovery cannot
  proceed past this point.
- **CRC32C, not IEEE CRC32.** CRC32C (Castagnoli) has hardware
  acceleration on x86-64 SSE 4.2 and ARMv8; the `crc32c` crate dispatches
  to those instructions automatically. For a WAL on the durability
  path, CRC32C is the right call — same protection as IEEE CRC32 but
  about an order of magnitude faster.
- **Two version bytes.** Segment-level version at the top of the
  file (for incompatible file format changes), and a per-record
  version (for adding fields to existing record types). Bumping
  either is a one-line code change and a no-cost compatibility
  story; not having them means a v2 layout silently misreads v1
  records and the only sign is corrupted state.
- **Hand-rolled binary.** The schema has two record types, three
  order kinds, two sides — a couple dozen bytes per record. A
  general-purpose serializer wins exactly nothing on this surface
  area and pays for it with link-time dependency cost and runtime
  branches we don't need. The whole codec is about 50 lines of LE
  byte twiddling.

## Tolerating partial writes

A WAL append goes: `write_all(len + crc + payload)` to a `BufWriter`,
then `commit()` which flushes the buffer to the OS and `fsync`s. If
the process or the kernel dies between `write_all` and `commit`, what
hits the disk could be:

- nothing (good — the previous record is the tail).
- a complete record (good — the new record is the tail).
- a *partial* record (bad if mishandled).

The reader treats short reads as clean EOF rather than as errors:

```rust
pub fn read_record(&mut self) -> Result<Option<WalRecord>, WalError> {
    let mut header = [0u8; 8];
    match self.file.read_exact(&mut header) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    // ...
    let mut payload = vec![0u8; len];
    match self.file.read_exact(&mut payload) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    if crc32c::crc32c(&payload) != crc { return Err(WalError::CrcMismatch); }
    Ok(Some(decode_record(&payload)?))
}
```

The unit test
`truncated_trailing_record_is_clean_eof` writes two records, commits,
chops the last byte of the file (simulating a crash mid-`fsync`),
then opens the reader and verifies the first record reads cleanly
and the second reads as `Ok(None)`. CRC mismatches in the *middle*
of the file (not at the trailing record) are still surfaced as a
typed error — that case represents corruption, not crash.

## The byte-exact replay test

The headline test, [`tests/replay.rs`][replay-test]:

1. Generate 10,000 random commands using a deterministic splitmix64
   PRNG with a fixed seed. ~70% Limits, 15% Markets, 10% IOCs, 5%
   Cancels of an existing order. Prices are in a tight band so the
   workload actually crosses.
2. **Live phase.** Open a `WalWriter`. For each command: append to
   WAL, commit (fsync per command), then process through `Matcher`.
   Collect events in a `Vec<Event>`.
3. **Replay phase.** Open a `WalReader` on the same file. Spin up a
   fresh `Matcher`. For each record: process it through the new
   matcher, collecting events.
4. **Assert.** `live.book() == replayed.book()` (they're both
   `Book`, which derives `PartialEq`). And `live_events ==
   replayed_events` — sequence number for sequence number, byte
   for byte.

`Book` derives `PartialEq` because every field on it does:
`BTreeMap<Price, VecDeque<Order>>` is `PartialEq` if `Price` and
`Order` are; `HashMap<OrderId, (Side, Price)>` is `PartialEq` on
contents (the hash order doesn't matter for equality). So the
assertion is genuinely checking the full state, not a hash that
might collide.

The "events also match" half is what makes this **byte-exact** rather
than just "produces the same final state." A bug that emitted an
extra `Done(Cancelled)` somewhere but happened to land the book in
the right place would pass a state-only check and fail the events
check.

## Snapshots — bounded recovery

Replaying a 100 GB WAL is cheap per record but not constant time. To
bound recovery latency, bourse writes periodic snapshots: serialised
book state plus a sequence marker.

The snapshot file format mirrors the WAL's choices: magic,
file-level version, then the marker, then a count, then per-order
records. Atomic writes via temp-then-rename — `WalWriter` opens
`path.tmp` exclusive-create, fsyncs, and renames into place, so a
crash mid-snapshot can never leave a half-written file at the real
path.

Recovery loads the snapshot, builds a `Book`, then constructs a
`Matcher` with `with_book(book, marker)`. The interesting bit is
`marker` — it's the matcher's `peek_seq()` at the moment the
snapshot was taken, and `Matcher::with_book` uses
`SequenceGenerator::starting_at` to seed the generator there. Without
that, the recovered matcher would issue seq 1, 2, 3, … starting from
the snapshot's tail, while the live engine had been deep into seq
40,000s. Resting orders added during tail replay would carry
different seq values than they did on the live engine. The book
state hashes wouldn't match.

(`Book` doesn't actually use the seq for ordering inside a price
level — that's done by `VecDeque` insertion order — so a "merely
semantically equivalent" recovery would *function* correctly. It just
wouldn't be byte-exact, which means downstream observers replaying
the same WAL on a different node would diverge in their event
streams. We want everyone to land in lockstep.)

The snapshot integration test mirrors the WAL replay test: live
phase, snapshot at the midpoint, more live phase, recovery from
`(snapshot + WAL_tail)`, assert `recovered.book() == live.book()`.
At 10k commands, recovery from snapshot+tail clocks ~2.5 ms vs
~1.9 ms for full WAL replay — roughly comparable at small N because
the snapshot file dominates. At 100k commands the snapshot wins
decisively; at 1M it's the difference between hundreds of ms and a
few ms.

## What we didn't do (yet)

- **Seq-tag every WAL record.** Right now WAL records are positional;
  recovery skips by count. Tagging each record with its corresponding
  matcher seq would let recovery skip-by-seq directly using the
  snapshot's marker, with no out-of-band counter. It's a one-byte-per-
  record cost and a one-line WAL format bump.
- **Group commit.** `commit()` currently fsyncs once per call, and
  the test exercises one commit per command. A real engine batches —
  pull N commands off the input queue, append all N to the WAL, then
  one fsync; ack all N. This trades a microsecond of latency for an
  order of magnitude of throughput. Easy add when the workload
  demands it.
- **Concurrent snapshot.** Snapshotting on the matcher thread stalls
  matching for the duration of the serialise. The architecture doc
  describes the eventual design — matcher emits a snapshot-marker
  event onto the WAL stream; a sidecar thread observes it, replays
  the WAL up to the marker into a side buffer, and writes the
  snapshot file. The matcher itself never blocks on snapshot I/O.
- **WAL segment rotation.** A single segment grows unboundedly. A
  production WAL closes a segment at some size threshold and starts a
  new one; old segments past the latest snapshot can be deleted.

## Why this matters

The WAL is bourse's contract with the rest of the world. Every
acknowledgement the matcher sends to a client is preceded by an
fsync of the corresponding `NewOrder` / `Cancel`. If the engine
crashes after the ack but before the next event lands, recovery
reads the WAL and brings the book to the exact state the client was
last told about. Re-running the matcher from the recovered state
produces the events the client would have seen had the crash never
happened.

Byte-exact replay is what makes that statement provable, not just
plausible.

[bourse]: https://github.com/pauti04/bourse
[wal]: https://github.com/pauti04/bourse/blob/main/crates/bourse-core/src/wal.rs
[snapshot]: https://github.com/pauti04/bourse/blob/main/crates/bourse-core/src/snapshot.rs
[replay-test]: https://github.com/pauti04/bourse/blob/main/crates/bourse-core/tests/replay.rs
[snapshot-test]: https://github.com/pauti04/bourse/blob/main/crates/bourse-core/tests/snapshot_recovery.rs
