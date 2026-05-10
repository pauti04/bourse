//! Write-ahead log.
//!
//! Append-only segment files. Each record is length-prefixed and CRC32C-
//! protected, so corruption is detected on read and a partial trailing
//! write (from a crash mid-append) is tolerated as truncation rather than
//! corrupting the whole segment.
//!
//! Replay re-feeds the recorded inputs through a fresh `Matcher` and
//! produces the same book and the same event stream — bit for bit.
//! `tests/replay.rs` exercises this on 10k random orders.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::Path;

use crate::matcher::{NewOrder, OrderKind};
use crate::types::{OrderId, Price, Qty, Sequence, Side, Timestamp};

// "MXCW" as little-endian bytes — printable in `xxd` for sanity.
const MAGIC: u32 = 0x5743_584D;
/// Segment-header version. Bumped to 2 in slice 13 to indicate
/// per-record `wal_seq` tagging — `WalReader::read_record` surfaces
/// the seq alongside the record so recovery can skip-by-seq from a
/// snapshot marker without out-of-band coordination.
const SEGMENT_VERSION: u8 = 2;
const RECORD_VERSION: u8 = 1;

const REC_TYPE_NEW_ORDER: u8 = 1;
const REC_TYPE_CANCEL: u8 = 2;

const KIND_LIMIT: u8 = 1;
const KIND_MARKET: u8 = 2;
const KIND_IOC: u8 = 3;

const SIDE_BUY: u8 = 1;
const SIDE_SELL: u8 = 2;

/// Errors from the WAL.
#[derive(Debug)]
pub enum WalError {
    /// Underlying IO error.
    Io(io::Error),
    /// CRC32C mismatch — the record bytes are corrupt.
    CrcMismatch,
    /// Segment header magic doesn't match.
    BadMagic,
    /// Segment header version this build doesn't know how to read.
    UnknownSegmentVersion(u8),
    /// Record version this build doesn't know how to read.
    UnknownRecordVersion(u8),
    /// Record type byte we don't recognise.
    UnknownRecordType(u8),
    /// Order-kind tag we don't recognise.
    UnknownKindTag(u8),
    /// Side byte we don't recognise.
    UnknownSide(u8),
    /// Record body claims to be larger than `u32::MAX`.
    RecordTooLarge,
    /// Record body shorter than the schema requires (after CRC has
    /// already validated). Indicates a v2 record missing its 8-byte
    /// `wal_seq` prefix.
    Truncated,
}

impl core::fmt::Display for WalError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "wal io: {e}"),
            Self::CrcMismatch => f.write_str("wal record crc32c mismatch"),
            Self::BadMagic => f.write_str("wal segment bad magic"),
            Self::UnknownSegmentVersion(v) => write!(f, "wal segment version {v} unknown"),
            Self::UnknownRecordVersion(v) => write!(f, "wal record version {v} unknown"),
            Self::UnknownRecordType(t) => write!(f, "wal record type {t} unknown"),
            Self::UnknownKindTag(t) => write!(f, "wal order kind tag {t} unknown"),
            Self::UnknownSide(s) => write!(f, "wal side byte {s} unknown"),
            Self::RecordTooLarge => f.write_str("wal record exceeds u32::MAX bytes"),
            Self::Truncated => f.write_str("wal record body too short for v2 framing"),
        }
    }
}

impl core::error::Error for WalError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for WalError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// A replay-applicable WAL record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalRecord {
    /// A new order arriving at the matcher.
    NewOrder(NewOrder),
    /// A cancel by id.
    Cancel(OrderId),
}

/// Append-only WAL writer.
///
/// Each appended record is tagged with a monotonically-increasing
/// `wal_seq` (an internal counter, distinct from the matcher's seq),
/// so on replay the reader can hand back `(wal_seq, record)` and a
/// snapshot's marker can be compared directly to skip records.
///
/// `append` queues bytes through a `BufWriter`; `commit` flushes and
/// fsyncs. Call `commit` whenever you need durability — typically per
/// acked op.
#[derive(Debug)]
pub struct WalWriter {
    file: BufWriter<File>,
    next_seq: u64,
}

impl WalWriter {
    /// Create a new segment at `path`. Fails if the file already
    /// exists. Writes the segment header and fsyncs it before
    /// returning. The first appended record gets `wal_seq = 1`.
    pub fn create(path: &Path) -> Result<Self, WalError> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        let mut w = BufWriter::new(file);
        w.write_all(&MAGIC.to_le_bytes())?;
        w.write_all(&[SEGMENT_VERSION, 0, 0, 0])?;
        w.flush()?;
        w.get_ref().sync_all()?;
        Ok(Self {
            file: w,
            next_seq: 1,
        })
    }

    /// Append a record. Bytes go into the OS page cache; not durable
    /// until [`commit`](Self::commit). Returns the `wal_seq` assigned
    /// to this record.
    pub fn append(&mut self, record: &WalRecord) -> Result<Sequence, WalError> {
        let seq = self.next_seq;
        let mut payload = Vec::with_capacity(56);
        payload.extend_from_slice(&seq.to_le_bytes());
        encode_record(record, &mut payload);
        let len = u32::try_from(payload.len()).map_err(|_| WalError::RecordTooLarge)?;
        let crc = crc32c::crc32c(&payload);
        self.file.write_all(&len.to_le_bytes())?;
        self.file.write_all(&crc.to_le_bytes())?;
        self.file.write_all(&payload)?;
        self.next_seq += 1;
        Ok(Sequence::from_raw(seq))
    }

    /// Flush the buffer to the OS and fsync the file.
    pub fn commit(&mut self) -> Result<(), WalError> {
        self.file.flush()?;
        self.file.get_ref().sync_all()?;
        Ok(())
    }

    /// What `wal_seq` the *next* `append` will assign. Useful at
    /// snapshot time: the snapshot marker is `next_seq() - 1`, the
    /// last record durably included in the snapshot's logical state.
    pub fn next_seq(&self) -> Sequence {
        Sequence::from_raw(self.next_seq)
    }
}

/// Reads records back from a WAL segment.
#[derive(Debug)]
pub struct WalReader {
    file: BufReader<File>,
}

impl WalReader {
    /// Open `path` for reading. Validates the segment header.
    pub fn open(path: &Path) -> Result<Self, WalError> {
        let file = File::open(path)?;
        let mut r = BufReader::new(file);
        let mut hdr = [0u8; 8];
        r.read_exact(&mut hdr)?;
        let magic = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        if magic != MAGIC {
            return Err(WalError::BadMagic);
        }
        let segver = hdr[4];
        if segver != SEGMENT_VERSION {
            return Err(WalError::UnknownSegmentVersion(segver));
        }
        Ok(Self { file: r })
    }

    /// Next record, or `Ok(None)` at clean EOF or when the trailing
    /// record was truncated (partial write from a crash). CRC failures
    /// in the middle of the file are surfaced as `Err(CrcMismatch)`.
    /// Returns `(wal_seq, record)`.
    pub fn read_record(&mut self) -> Result<Option<(Sequence, WalRecord)>, WalError> {
        let mut header = [0u8; 8];
        match self.file.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        let len = u32::from_le_bytes([header[0], header[1], header[2], header[3]]) as usize;
        let crc = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);

        let mut payload = vec![0u8; len];
        match self.file.read_exact(&mut payload) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        if crc32c::crc32c(&payload) != crc {
            return Err(WalError::CrcMismatch);
        }
        if payload.len() < 8 {
            return Err(WalError::Truncated);
        }
        let mut seq_buf = [0u8; 8];
        seq_buf.copy_from_slice(&payload[..8]);
        let seq = Sequence::from_raw(u64::from_le_bytes(seq_buf));
        let rec = decode_record(&payload[8..])?;
        Ok(Some((seq, rec)))
    }
}

/// Run `f` on every `(wal_seq, record)` in the WAL at `path`.
pub fn for_each_record<F>(path: &Path, mut f: F) -> Result<(), WalError>
where
    F: FnMut(Sequence, WalRecord),
{
    let mut r = WalReader::open(path)?;
    while let Some((seq, rec)) = r.read_record()? {
        f(seq, rec);
    }
    Ok(())
}

fn encode_record(rec: &WalRecord, out: &mut Vec<u8>) {
    out.push(RECORD_VERSION);
    match *rec {
        WalRecord::NewOrder(no) => {
            out.push(REC_TYPE_NEW_ORDER);
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
            }
            out.extend_from_slice(&no.timestamp.nanos().to_le_bytes());
        }
        WalRecord::Cancel(id) => {
            out.push(REC_TYPE_CANCEL);
            out.extend_from_slice(&id.get().to_le_bytes());
        }
    }
}

fn decode_record(payload: &[u8]) -> Result<WalRecord, WalError> {
    let mut c = Cursor::new(payload);
    let version = c.read_u8()?;
    if version != RECORD_VERSION {
        return Err(WalError::UnknownRecordVersion(version));
    }
    let tag = c.read_u8()?;
    match tag {
        REC_TYPE_NEW_ORDER => {
            let id = OrderId::new(c.read_u64()?);
            let side = match c.read_u8()? {
                SIDE_BUY => Side::Buy,
                SIDE_SELL => Side::Sell,
                other => return Err(WalError::UnknownSide(other)),
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
                other => return Err(WalError::UnknownKindTag(other)),
            };
            let timestamp = Timestamp::from_nanos(c.read_i64()?);
            Ok(WalRecord::NewOrder(NewOrder {
                id,
                side,
                qty,
                kind,
                timestamp,
            }))
        }
        REC_TYPE_CANCEL => {
            let id = OrderId::new(c.read_u64()?);
            Ok(WalRecord::Cancel(id))
        }
        other => Err(WalError::UnknownRecordType(other)),
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
    fn short(&self) -> WalError {
        WalError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "wal record payload shorter than expected",
        ))
    }
    fn read_u8(&mut self) -> Result<u8, WalError> {
        let b = *self.bytes.get(self.pos).ok_or_else(|| self.short())?;
        self.pos += 1;
        Ok(b)
    }
    fn read_u64(&mut self) -> Result<u64, WalError> {
        let end = self.pos + 8;
        let bytes = self.bytes.get(self.pos..end).ok_or_else(|| self.short())?;
        let mut buf = [0u8; 8];
        buf.copy_from_slice(bytes);
        self.pos = end;
        Ok(u64::from_le_bytes(buf))
    }
    fn read_i64(&mut self) -> Result<i64, WalError> {
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
    use std::fs;
    use std::path::PathBuf;

    fn arb_kind() -> impl Strategy<Value = OrderKind> {
        prop_oneof![
            any::<i64>().prop_map(|p| OrderKind::Limit {
                price: Price::from_raw(p)
            }),
            Just(OrderKind::Market),
            any::<i64>().prop_map(|p| OrderKind::Ioc {
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

    fn arb_record() -> impl Strategy<Value = WalRecord> {
        prop_oneof![
            arb_new_order().prop_map(WalRecord::NewOrder),
            any::<u64>().prop_map(|n| WalRecord::Cancel(OrderId::new(n))),
        ]
    }

    proptest! {
        #[test]
        fn codec_roundtrip(rec in arb_record()) {
            let mut buf = Vec::new();
            encode_record(&rec, &mut buf);
            let decoded = decode_record(&buf).expect("decode");
            prop_assert_eq!(rec, decoded);
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("matchx-wal-tests").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_then_read_one_record() {
        let dir = temp_dir("write_then_read_one_record");
        let path = dir.join("wal");
        let mut w = WalWriter::create(&path).unwrap();
        let rec = WalRecord::Cancel(OrderId::new(42));
        let assigned = w.append(&rec).unwrap();
        w.commit().unwrap();
        drop(w);
        assert_eq!(assigned, Sequence::from_raw(1));

        let mut r = WalReader::open(&path).unwrap();
        assert_eq!(r.read_record().unwrap(), Some((Sequence::from_raw(1), rec)));
        assert_eq!(r.read_record().unwrap(), None);
    }

    #[test]
    fn assigned_seqs_are_consecutive() {
        let dir = temp_dir("assigned_seqs_are_consecutive");
        let path = dir.join("wal");
        let mut w = WalWriter::create(&path).unwrap();
        for i in 1..=10 {
            let s = w.append(&WalRecord::Cancel(OrderId::new(i))).unwrap();
            assert_eq!(s, Sequence::from_raw(i));
        }
        assert_eq!(w.next_seq(), Sequence::from_raw(11));
    }

    #[test]
    fn crc_mismatch_is_detected() {
        let dir = temp_dir("crc_mismatch_is_detected");
        let path = dir.join("wal");
        let mut w = WalWriter::create(&path).unwrap();
        w.append(&WalRecord::Cancel(OrderId::new(7))).unwrap();
        w.commit().unwrap();
        drop(w);

        // Flip a bit deep in the payload to corrupt it without touching
        // the segment header.
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        fs::write(&path, &bytes).unwrap();

        let mut r = WalReader::open(&path).unwrap();
        match r.read_record() {
            Err(WalError::CrcMismatch) => {}
            other => panic!("expected CrcMismatch, got {other:?}"),
        }
    }

    #[test]
    fn truncated_trailing_record_is_clean_eof() {
        let dir = temp_dir("truncated_trailing_record_is_clean_eof");
        let path = dir.join("wal");
        let mut w = WalWriter::create(&path).unwrap();
        w.append(&WalRecord::Cancel(OrderId::new(1))).unwrap();
        w.append(&WalRecord::Cancel(OrderId::new(2))).unwrap();
        w.commit().unwrap();
        drop(w);

        // Chop the last byte — simulates a crash mid-append on the
        // second record.
        let mut bytes = fs::read(&path).unwrap();
        bytes.pop();
        fs::write(&path, &bytes).unwrap();

        let mut r = WalReader::open(&path).unwrap();
        // First record reads cleanly.
        assert_eq!(
            r.read_record().unwrap(),
            Some((Sequence::from_raw(1), WalRecord::Cancel(OrderId::new(1))))
        );
        // Second record's body is short → treat as clean EOF.
        assert_eq!(r.read_record().unwrap(), None);
    }

    #[test]
    fn bad_magic_rejected() {
        let dir = temp_dir("bad_magic_rejected");
        let path = dir.join("wal");
        fs::write(&path, [0xFF; 8]).unwrap();
        match WalReader::open(&path) {
            Err(WalError::BadMagic) => {}
            other => panic!("expected BadMagic, got {other:?}"),
        }
    }
}
