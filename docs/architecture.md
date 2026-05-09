# architecture

A single OS process. Three concerns:

- Gateway threads (tokio) decode the binary protocol and push messages onto
  a lock-free SPSC queue.
- A single dedicated thread, the matcher, pops from that queue, mutates the
  order book, and emits executions and book deltas onto outbound queues.
- Publisher / WAL writer threads drain those outbound queues, fsync the
  WAL, and publish UDP multicast.

The order book itself isn't lock-free. Only the matcher writes to it, so
there's no contention to design around. The lock-free primitives in this
system are the SPSC input queue and the broadcast outbound queues — that's
where `unsafe` and Miri will earn their keep.

The WAL is the durability boundary: every state-changing op is fsynced
before the corresponding `ExecutionReport` is sent to the client. Replay
loads the latest snapshot, replays the WAL tail, and an integration test
asserts that the reconstructed state hashes to the same bytes as the live
book.

Versioning: every wire frame and every WAL record begins with a version
byte. New fields ship as a version bump, never a silent layout change.
