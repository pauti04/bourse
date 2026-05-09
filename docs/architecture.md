# Architecture

> Bootstrap version. Expanded as subsystems land. Keep this file in sync
> with `crates/`.

## Process model

A single OS process. Three classes of threads:

1. **Gateway threads (tokio).** Accept TCP connections, decode protocol
   messages with `matchx-protocol`, push them onto a lock-free SPSC input
   queue.
2. **Matcher thread (single, dedicated, pinnable).** Pops messages from the
   input queue, mutates the order book, emits `ExecutionReport`s and
   market-data deltas onto outbound queues.
3. **Publisher / WAL writer thread(s).** Drain outbound queues, durably
   write WAL records (fsync-on-commit), publish UDP-multicast updates.

Acknowledgement to the client is sent only after the corresponding WAL
record has been fsynced — the WAL is the authoritative record of what
happened and is replayable byte-for-byte.

## Why the order book itself is *not* lock-free

A natural-sounding choice for a "high-performance matching engine" would
be a lock-free order book. We deliberately reject it for v1:

- Only one thread (the matcher) ever mutates the book. Multi-writer
  contention does not exist.
- A lock-free book would require epoch-based memory reclamation and
  detailed memory-ordering proofs — significant complexity for **zero**
  measurable benefit when the contended boundaries lie elsewhere.
- The contended boundaries are the SPSC input queue and the outbound
  broadcast queues. Those are the primitives where `unsafe` will be
  justified once benchmarks demand it.

A multi-instrument variant (v2) where multiple matcher threads share a
single book per instrument doesn't change this picture — each book still
has a single writer. Multiple instruments means multiple matchers, each
owning its own book.

## Hot-path discipline

The matcher's loop body is the hot path. It must:

- Allocate zero bytes in steady state (all capacities pre-sized at startup).
- Issue no syscalls.
- Take no locks.
- Use only fixed-point integer arithmetic — never floats.

Tracing on the hot path is restricted to events that compile out under
release-mode subscribers, or that live behind a feature flag. An
allocation-counting harness (planned for the matcher slice) makes the
"no alloc on the hot path" claim machine-checkable.

## Snapshot strategy (final design lands in the WAL slice)

Leading proposal:

1. Periodically the matcher emits a **snapshot marker** sequence number
   onto the WAL stream. The marker is just an event of type `SnapMark`.
2. A sidecar **snapshotter thread** observes markers and serializes the
   book state by replaying the WAL from the previous snapshot up to the
   marker into a side buffer, then writing a new snapshot file alongside
   the WAL.
3. The matcher thread itself never stalls for a snapshot — it never
   blocks on snapshot I/O, it only emits the marker event.

Recovery is then: load the latest snapshot, replay the WAL from that
snapshot's marker forward, exit with the resulting state hash.

## Versioning

Every persistent or wire artifact begins with a **version byte**:

- WAL records: 1-byte version, then the framed record.
- Snapshot files: 1-byte version, then a length-prefixed body.
- Wire protocol frames: 1-byte version negotiated at session start.

This is the only way we ship a v1 we'll be able to evolve without
breaking replay of old WALs. We do not "add it later".
