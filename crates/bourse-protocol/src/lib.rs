//! bourse wire protocol — binary codec, no I/O.
//!
//! Length-prefixed framing. Each frame starts with a 4-byte little-
//! endian length followed by a 1-byte protocol version, a 1-byte
//! message type, and a type-specific payload. Both the segment-level
//! version and per-message constants give us room to evolve.
//!
//! ## Frame
//!
//! ```text
//! [len u32 LE] [version u8] [type u8] [payload (len-2 bytes)]
//! ```
//!
//! ## Messages
//!
//! - Client → Server: `NewOrderSingle`, `OrderCancelRequest`.
//! - Server → Client: `Execution` (one per matcher [`Event`] —
//!   Accepted / Trade / Done).
//!
//! ## Why hand-rolled
//!
//! The wire schema is small (three message types, fixed-size
//! payloads), the matcher pumps tens of millions of events per second
//! per core, and a length+CRC framing built on top of fixed integer
//! reads is faster than any general-purpose serializer. Same reasoning
//! drove the WAL codec — see `bourse_core::wal`.

use bourse_core::matcher::{DoneReason, Event, NewOrder, OrderKind};
use bourse_core::types::{OrderId, Price, Qty, Sequence, Side, Timestamp};

const PROTOCOL_VERSION: u8 = 1;

const MSG_NEW_ORDER: u8 = 0x01;
const MSG_CANCEL: u8 = 0x02;
const MSG_EXECUTION: u8 = 0x10;

const SIDE_BUY: u8 = 1;
const SIDE_SELL: u8 = 2;

const KIND_LIMIT: u8 = 1;
const KIND_MARKET: u8 = 2;
const KIND_IOC: u8 = 3;
const KIND_POST_ONLY: u8 = 4;
const KIND_FOK: u8 = 5;

const EXEC_ACCEPTED: u8 = 1;
const EXEC_TRADE: u8 = 2;
const EXEC_DONE: u8 = 3;

const REASON_FILLED: u8 = 1;
const REASON_CANCELLED: u8 = 2;
const REASON_EXPIRED: u8 = 3;
const REASON_NO_LIQUIDITY: u8 = 4;
const REASON_REJECTED: u8 = 5;

/// Errors from decoding a wire frame.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ProtocolError {
    /// Frame body shorter than the schema requires.
    Truncated,
    /// Protocol version we don't know how to read.
    UnknownVersion(u8),
    /// Message type byte we don't recognise.
    UnknownMessage(u8),
    /// Side byte we don't recognise.
    UnknownSide(u8),
    /// Order-kind tag we don't recognise.
    UnknownKindTag(u8),
    /// Execution-report subtype byte we don't recognise.
    UnknownExecType(u8),
    /// Done-reason byte we don't recognise.
    UnknownReason(u8),
    /// Frame body length doesn't match the message schema.
    TrailingBytes,
}

impl core::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Truncated => f.write_str("frame body truncated"),
            Self::UnknownVersion(v) => write!(f, "protocol version {v} unknown"),
            Self::UnknownMessage(t) => write!(f, "message type {t} unknown"),
            Self::UnknownSide(s) => write!(f, "side byte {s} unknown"),
            Self::UnknownKindTag(t) => write!(f, "order kind tag {t} unknown"),
            Self::UnknownExecType(t) => write!(f, "exec type {t} unknown"),
            Self::UnknownReason(r) => write!(f, "done reason {r} unknown"),
            Self::TrailingBytes => f.write_str("frame body had trailing bytes"),
        }
    }
}

impl core::error::Error for ProtocolError {}

/// Messages the client sends to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientMessage {
    /// Submit a new order.
    NewOrder(NewOrder),
    /// Cancel a resting order by id.
    Cancel(OrderId),
}

/// Messages the server sends back. There's one execution event per
/// matcher [`Event`] emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMessage {
    /// One matcher event.
    Execution(Event),
}

/// Encode a `ClientMessage` into a length-prefixed frame appended to
/// `out`.
pub fn encode_client(msg: &ClientMessage, out: &mut Vec<u8>) {
    let frame_start = out.len();
    out.extend_from_slice(&[0u8; 4]); // length placeholder
    out.push(PROTOCOL_VERSION);
    match *msg {
        ClientMessage::NewOrder(no) => {
            out.push(MSG_NEW_ORDER);
            encode_new_order(&no, out);
        }
        ClientMessage::Cancel(id) => {
            out.push(MSG_CANCEL);
            out.extend_from_slice(&id.get().to_le_bytes());
        }
    }
    let body_len = out.len() - frame_start - 4;
    let len_bytes = u32::try_from(body_len).unwrap_or(u32::MAX).to_le_bytes();
    out[frame_start..frame_start + 4].copy_from_slice(&len_bytes);
}

/// Encode a `ServerMessage` into a length-prefixed frame appended to
/// `out`.
pub fn encode_server(msg: &ServerMessage, out: &mut Vec<u8>) {
    let frame_start = out.len();
    out.extend_from_slice(&[0u8; 4]);
    out.push(PROTOCOL_VERSION);
    match *msg {
        ServerMessage::Execution(e) => {
            out.push(MSG_EXECUTION);
            encode_event(&e, out);
        }
    }
    let body_len = out.len() - frame_start - 4;
    let len_bytes = u32::try_from(body_len).unwrap_or(u32::MAX).to_le_bytes();
    out[frame_start..frame_start + 4].copy_from_slice(&len_bytes);
}

/// Decode a `ClientMessage` from a frame body (everything after the
/// length prefix). The body starts with the version byte.
pub fn decode_client(body: &[u8]) -> Result<ClientMessage, ProtocolError> {
    let mut c = Cursor::new(body);
    let version = c.read_u8()?;
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnknownVersion(version));
    }
    let tag = c.read_u8()?;
    let msg = match tag {
        MSG_NEW_ORDER => ClientMessage::NewOrder(decode_new_order(&mut c)?),
        MSG_CANCEL => ClientMessage::Cancel(OrderId::new(c.read_u64()?)),
        other => return Err(ProtocolError::UnknownMessage(other)),
    };
    if c.pos != c.bytes.len() {
        return Err(ProtocolError::TrailingBytes);
    }
    Ok(msg)
}

/// Decode a `ServerMessage` from a frame body.
pub fn decode_server(body: &[u8]) -> Result<ServerMessage, ProtocolError> {
    let mut c = Cursor::new(body);
    let version = c.read_u8()?;
    if version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnknownVersion(version));
    }
    let tag = c.read_u8()?;
    let msg = match tag {
        MSG_EXECUTION => ServerMessage::Execution(decode_event(&mut c)?),
        other => return Err(ProtocolError::UnknownMessage(other)),
    };
    if c.pos != c.bytes.len() {
        return Err(ProtocolError::TrailingBytes);
    }
    Ok(msg)
}

fn encode_new_order(no: &NewOrder, out: &mut Vec<u8>) {
    out.extend_from_slice(&no.id.get().to_le_bytes());
    out.push(match no.side {
        Side::Buy => SIDE_BUY,
        Side::Sell => SIDE_SELL,
    });
    out.extend_from_slice(&no.qty.get().to_le_bytes());
    match no.kind {
        OrderKind::Limit { price } => {
            out.push(KIND_LIMIT);
            out.extend_from_slice(&price.raw().to_le_bytes());
        }
        OrderKind::Market => {
            out.push(KIND_MARKET);
        }
        OrderKind::Ioc { price } => {
            out.push(KIND_IOC);
            out.extend_from_slice(&price.raw().to_le_bytes());
        }
        OrderKind::PostOnly { price } => {
            out.push(KIND_POST_ONLY);
            out.extend_from_slice(&price.raw().to_le_bytes());
        }
        OrderKind::Fok { price } => {
            out.push(KIND_FOK);
            out.extend_from_slice(&price.raw().to_le_bytes());
        }
    }
    out.extend_from_slice(&no.timestamp.nanos().to_le_bytes());
}

fn decode_new_order(c: &mut Cursor<'_>) -> Result<NewOrder, ProtocolError> {
    let id = OrderId::new(c.read_u64()?);
    let side = match c.read_u8()? {
        SIDE_BUY => Side::Buy,
        SIDE_SELL => Side::Sell,
        other => return Err(ProtocolError::UnknownSide(other)),
    };
    let qty = Qty::new(c.read_u64()?);
    let kind = match c.read_u8()? {
        KIND_LIMIT => OrderKind::Limit {
            price: Price::from_raw(c.read_i64()?),
        },
        KIND_MARKET => OrderKind::Market,
        KIND_IOC => OrderKind::Ioc {
            price: Price::from_raw(c.read_i64()?),
        },
        KIND_POST_ONLY => OrderKind::PostOnly {
            price: Price::from_raw(c.read_i64()?),
        },
        KIND_FOK => OrderKind::Fok {
            price: Price::from_raw(c.read_i64()?),
        },
        other => return Err(ProtocolError::UnknownKindTag(other)),
    };
    let timestamp = Timestamp::from_nanos(c.read_i64()?);
    Ok(NewOrder {
        id,
        side,
        qty,
        kind,
        timestamp,
    })
}

fn encode_event(e: &Event, out: &mut Vec<u8>) {
    match *e {
        Event::Accepted { id, qty, seq } => {
            out.push(EXEC_ACCEPTED);
            out.extend_from_slice(&id.get().to_le_bytes());
            out.extend_from_slice(&qty.get().to_le_bytes());
            out.extend_from_slice(&seq.get().to_le_bytes());
        }
        Event::Trade {
            taker,
            maker,
            price,
            qty,
            seq,
        } => {
            out.push(EXEC_TRADE);
            out.extend_from_slice(&taker.get().to_le_bytes());
            out.extend_from_slice(&maker.get().to_le_bytes());
            out.extend_from_slice(&price.raw().to_le_bytes());
            out.extend_from_slice(&qty.get().to_le_bytes());
            out.extend_from_slice(&seq.get().to_le_bytes());
        }
        Event::Done {
            id,
            leaves_qty,
            reason,
            seq,
        } => {
            out.push(EXEC_DONE);
            out.extend_from_slice(&id.get().to_le_bytes());
            out.extend_from_slice(&leaves_qty.get().to_le_bytes());
            out.push(match reason {
                DoneReason::Filled => REASON_FILLED,
                DoneReason::Cancelled => REASON_CANCELLED,
                DoneReason::Expired => REASON_EXPIRED,
                DoneReason::NoLiquidity => REASON_NO_LIQUIDITY,
                DoneReason::Rejected => REASON_REJECTED,
            });
            out.extend_from_slice(&seq.get().to_le_bytes());
        }
    }
}

fn decode_event(c: &mut Cursor<'_>) -> Result<Event, ProtocolError> {
    let tag = c.read_u8()?;
    match tag {
        EXEC_ACCEPTED => Ok(Event::Accepted {
            id: OrderId::new(c.read_u64()?),
            qty: Qty::new(c.read_u64()?),
            seq: Sequence::from_raw(c.read_u64()?),
        }),
        EXEC_TRADE => Ok(Event::Trade {
            taker: OrderId::new(c.read_u64()?),
            maker: OrderId::new(c.read_u64()?),
            price: Price::from_raw(c.read_i64()?),
            qty: Qty::new(c.read_u64()?),
            seq: Sequence::from_raw(c.read_u64()?),
        }),
        EXEC_DONE => {
            let id = OrderId::new(c.read_u64()?);
            let leaves_qty = Qty::new(c.read_u64()?);
            let reason = match c.read_u8()? {
                REASON_FILLED => DoneReason::Filled,
                REASON_CANCELLED => DoneReason::Cancelled,
                REASON_EXPIRED => DoneReason::Expired,
                REASON_NO_LIQUIDITY => DoneReason::NoLiquidity,
                REASON_REJECTED => DoneReason::Rejected,
                other => return Err(ProtocolError::UnknownReason(other)),
            };
            let seq = Sequence::from_raw(c.read_u64()?);
            Ok(Event::Done {
                id,
                leaves_qty,
                reason,
                seq,
            })
        }
        other => Err(ProtocolError::UnknownExecType(other)),
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    fn read_u8(&mut self) -> Result<u8, ProtocolError> {
        let b = *self.bytes.get(self.pos).ok_or(ProtocolError::Truncated)?;
        self.pos += 1;
        Ok(b)
    }
    fn read_u64(&mut self) -> Result<u64, ProtocolError> {
        let end = self.pos + 8;
        let bytes = self
            .bytes
            .get(self.pos..end)
            .ok_or(ProtocolError::Truncated)?;
        let mut buf = [0u8; 8];
        buf.copy_from_slice(bytes);
        self.pos = end;
        Ok(u64::from_le_bytes(buf))
    }
    fn read_i64(&mut self) -> Result<i64, ProtocolError> {
        Ok(self.read_u64()? as i64)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "test setup"
    )]

    use super::*;
    use proptest::prelude::*;

    fn arb_kind() -> impl Strategy<Value = OrderKind> {
        prop_oneof![
            any::<i64>().prop_map(|p| OrderKind::Limit {
                price: Price::from_raw(p)
            }),
            Just(OrderKind::Market),
            any::<i64>().prop_map(|p| OrderKind::Ioc {
                price: Price::from_raw(p)
            }),
            any::<i64>().prop_map(|p| OrderKind::PostOnly {
                price: Price::from_raw(p)
            }),
            any::<i64>().prop_map(|p| OrderKind::Fok {
                price: Price::from_raw(p)
            }),
        ]
    }

    fn arb_new_order() -> impl Strategy<Value = NewOrder> {
        (
            any::<u64>(),
            any::<bool>(),
            any::<u64>(),
            arb_kind(),
            any::<i64>(),
        )
            .prop_map(|(id, buy, qty, kind, ts)| NewOrder {
                id: OrderId::new(id),
                side: if buy { Side::Buy } else { Side::Sell },
                qty: Qty::new(qty),
                kind,
                timestamp: Timestamp::from_nanos(ts),
            })
    }

    fn arb_client() -> impl Strategy<Value = ClientMessage> {
        prop_oneof![
            arb_new_order().prop_map(ClientMessage::NewOrder),
            any::<u64>().prop_map(|n| ClientMessage::Cancel(OrderId::new(n))),
        ]
    }

    fn arb_event() -> impl Strategy<Value = Event> {
        prop_oneof![
            (any::<u64>(), any::<u64>(), any::<u64>()).prop_map(|(id, qty, seq)| Event::Accepted {
                id: OrderId::new(id),
                qty: Qty::new(qty),
                seq: Sequence::from_raw(seq),
            }),
            (
                any::<u64>(),
                any::<u64>(),
                any::<i64>(),
                any::<u64>(),
                any::<u64>()
            )
                .prop_map(|(t, m, p, q, s)| Event::Trade {
                    taker: OrderId::new(t),
                    maker: OrderId::new(m),
                    price: Price::from_raw(p),
                    qty: Qty::new(q),
                    seq: Sequence::from_raw(s),
                }),
            (
                any::<u64>(),
                any::<u64>(),
                prop_oneof![
                    Just(DoneReason::Filled),
                    Just(DoneReason::Cancelled),
                    Just(DoneReason::Expired),
                    Just(DoneReason::NoLiquidity),
                    Just(DoneReason::Rejected),
                ],
                any::<u64>(),
            )
                .prop_map(|(id, l, r, s)| Event::Done {
                    id: OrderId::new(id),
                    leaves_qty: Qty::new(l),
                    reason: r,
                    seq: Sequence::from_raw(s),
                }),
        ]
    }

    fn frame_body(buf: &[u8]) -> &[u8] {
        let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        &buf[4..4 + len]
    }

    proptest! {
        #[test]
        fn client_roundtrip(msg in arb_client()) {
            let mut buf = Vec::new();
            encode_client(&msg, &mut buf);
            let body = frame_body(&buf);
            prop_assert_eq!(decode_client(body).unwrap(), msg);
        }

        #[test]
        fn server_roundtrip(e in arb_event()) {
            let mut buf = Vec::new();
            encode_server(&ServerMessage::Execution(e), &mut buf);
            let body = frame_body(&buf);
            prop_assert_eq!(decode_server(body).unwrap(), ServerMessage::Execution(e));
        }
    }

    #[test]
    fn unknown_version_rejected() {
        let msg = ClientMessage::Cancel(OrderId::new(1));
        let mut buf = Vec::new();
        encode_client(&msg, &mut buf);
        // Body starts at offset 4; bump the version byte.
        buf[4] = 0xFF;
        let body = frame_body(&buf);
        assert_eq!(
            decode_client(body),
            Err(ProtocolError::UnknownVersion(0xFF))
        );
    }

    #[test]
    fn truncated_body_rejected() {
        let body = [PROTOCOL_VERSION, MSG_NEW_ORDER]; // missing payload
        assert_eq!(decode_client(&body), Err(ProtocolError::Truncated));
    }

    #[test]
    fn frame_layout_is_length_prefixed() {
        let msg = ClientMessage::Cancel(OrderId::new(0xDEAD_BEEF));
        let mut buf = Vec::new();
        encode_client(&msg, &mut buf);
        // 4 bytes len + 1 byte version + 1 byte type + 8 bytes id = 14
        // total bytes; len = 1 + 1 + 8 = 10.
        assert_eq!(buf.len(), 14);
        assert_eq!(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]), 10);
    }
}
