# Wire protocol

> Bootstrap placeholder. Full layout lands in the protocol slice.

## Framing

Each TCP frame is **length-prefixed**:

```
+--------+--------+----------------+
| len:u32| ver:u8 | payload (len-1 bytes)
+--------+--------+----------------+
```

- `len` is the byte length of `ver + payload` in network byte order.
- `ver` is the protocol version. Negotiated at session start; the engine
  rejects mismatched versions before processing any payload.

## Message types (v1)

Field semantics follow [FIX 4.4](https://www.fixtrading.org/standards/fix-4-4/)
where reasonable. The encoding is **binary** (closer in spirit to SBE than
to FIX tag=value), chosen for parsing throughput and zero-allocation
decode. Detailed byte layouts land in the protocol slice.

| Message            | Direction         | Purpose                                  |
| ------------------ | ----------------- | ---------------------------------------- |
| `NewOrderSingle`   | client → server   | Submit a new order.                      |
| `OrderCancelRequest` | client → server | Cancel a resting order by `OrderId`.    |
| `ExecutionReport`  | server → client   | Acknowledgement, partial fill, full fill, cancel ack. |
| `OrderCancelReject` | server → client  | Cancel of an unknown / already-filled order. |

## Side encoding

Per `Side` in `matchx-core::types`:

| Side  | Wire byte |
| ----- | --------- |
| Buy   | `0x01`    |
| Sell  | `0x02`    |

## Market-data feed (UDP multicast)

Separate from the order-entry session. Sequence-numbered incremental
updates with periodic full-book snapshots so a joining consumer can catch
up. Layout TBD in the protocol slice.
