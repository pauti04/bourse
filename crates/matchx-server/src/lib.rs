//! matchx server — accepts TCP connections and pipes orders through
//! the engine, one engine per connection.
//!
//! Per connection: a fresh [`Engine`], split into a producer/consumer
//! pair plus a handle. Two tokio tasks — a reader that decodes
//! [`ClientMessage`]s off the wire and pushes [`Command`]s onto the
//! engine, and a writer that drains [`Event`]s off the engine and
//! frames them out as [`ServerMessage`]s. When the client disconnects,
//! the reader returns; the writer is aborted; the engine is stopped.
//!
//! v1 limitation: one connection at a time per server-bound engine.
//! Multi-tenant matching needs MPSC at the gateway boundary, which is
//! parked under v2.

#![allow(
    clippy::expect_used,
    reason = "task spawn / join failures are unrecoverable; matches std::thread's contract"
)]

use std::io;

use matchx_core::engine::{Command, Engine};
use matchx_core::matcher::Event;
use matchx_core::spsc::{Consumer, Producer};
use matchx_protocol::{ClientMessage, ServerMessage, decode_client, encode_server};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Server configuration.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Capacity of the SPSC command queue (rounded up to power of two).
    pub command_capacity: usize,
    /// Capacity of the SPSC event queue (rounded up to power of two).
    pub event_capacity: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            command_capacity: 4096,
            event_capacity: 4096,
        }
    }
}

/// Bind to `addr` and start serving. Returns the listener (so callers
/// can read `local_addr` for tests using port 0) plus a future that
/// runs the accept loop.
pub async fn bind(addr: &str) -> io::Result<TcpListener> {
    TcpListener::bind(addr).await
}

/// Run the accept loop on `listener` forever, spawning a connection
/// handler per accepted socket. Returns only on accept-loop error.
pub async fn serve(listener: TcpListener, cfg: Config) -> io::Result<()> {
    loop {
        let (stream, _peer) = listener.accept().await?;
        tokio::spawn(handle_connection(stream, cfg));
    }
}

async fn handle_connection(stream: TcpStream, cfg: Config) {
    let _ = stream.set_nodelay(true);
    let engine = Engine::start(cfg.command_capacity, cfg.event_capacity);
    let (input, events, handle) = engine.split();

    let (read_half, write_half) = stream.into_split();
    let reader = tokio::spawn(reader_loop(read_half, input));
    let writer = tokio::spawn(writer_loop(write_half, events));

    // Wait for the reader to finish (client disconnected or sent garbage).
    let _ = reader.await;
    // Tell the writer to stop draining and exit.
    writer.abort();
    let _ = writer.await;
    let _ = handle.stop();
}

async fn reader_loop<R>(mut r: R, mut input: Producer<Command>) -> io::Result<()>
where
    R: AsyncReadExt + Unpin,
{
    let mut len_buf = [0u8; 4];
    let mut body = Vec::with_capacity(64);
    loop {
        match r.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        body.clear();
        body.resize(len, 0);
        r.read_exact(&mut body).await?;
        let cmd = match decode_client(&body) {
            Ok(ClientMessage::NewOrder(no)) => Command::New(no),
            Ok(ClientMessage::Cancel(id)) => Command::Cancel(id),
            Err(e) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("decode: {e}"),
                ));
            }
        };
        let mut c = cmd;
        while let Err(returned) = input.try_push(c) {
            c = returned;
            tokio::task::yield_now().await;
        }
    }
}

async fn writer_loop<W>(mut w: W, mut events: Consumer<Event>) -> io::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let mut buf = Vec::with_capacity(64);
    loop {
        if let Some(e) = events.try_pop() {
            buf.clear();
            encode_server(&ServerMessage::Execution(e), &mut buf);
            w.write_all(&buf).await?;
            // Don't flush per event — TCP_NODELAY is on, so the kernel
            // will send promptly, and skipping the explicit flush keeps
            // small bursts coalesced.
        } else {
            tokio::task::yield_now().await;
        }
    }
}

/// For tests: read a single framed `ServerMessage` from `r`. Returns
/// `Ok(None)` on clean EOF.
pub async fn read_one_server_message<R>(r: &mut R) -> io::Result<Option<ServerMessage>>
where
    R: AsyncReadExt + Unpin,
{
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    matchx_protocol::decode_server(&body)
        .map(Some)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{e}")))
}
