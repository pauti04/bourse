//! matchx server — accepts TCP connections and pipes orders through a
//! shared multi-tenant `Hub`.
//!
//! One `Hub` for the whole process: a single matcher thread fed by a
//! lock-free MPSC (`crossbeam_queue::ArrayQueue`) shared by every
//! connected client. Each TCP connection registers a tenant on the
//! hub and gets back `(Submitter, EventReceiver)`. The reader task
//! decodes `ClientMessage`s and calls `submitter.submit`; the writer
//! task drains the event receiver and frames events out as
//! `ServerMessage`s. When the reader returns (client disconnect /
//! decode error) the writer is aborted and the submitter dropped,
//! which sends a `Disconnect` to the matcher.

#![allow(
    clippy::expect_used,
    reason = "task spawn / join failures are unrecoverable; matches std::thread"
)]

use std::io;
use std::sync::Arc;

use matchx_core::hub::{Command, Hub, Submitter};
use matchx_core::matcher::Event;
use matchx_protocol::{ClientMessage, ServerMessage, decode_client, encode_server};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::UnboundedReceiver;

/// Server configuration.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Capacity of the shared MPSC inbox (rounded up internally).
    pub inbox_capacity: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            inbox_capacity: 8192,
        }
    }
}

/// Bind to `addr`. Returns a listener; pass it to [`serve`].
pub async fn bind(addr: &str) -> io::Result<TcpListener> {
    TcpListener::bind(addr).await
}

/// Run the accept loop on `listener` forever, sharing one matcher
/// across every accepted connection.
pub async fn serve(listener: TcpListener, cfg: Config) -> io::Result<()> {
    let hub = Arc::new(Hub::start(cfg.inbox_capacity));
    loop {
        let (stream, _peer) = listener.accept().await?;
        let hub = Arc::clone(&hub);
        tokio::spawn(handle_connection(stream, hub));
    }
}

async fn handle_connection(stream: TcpStream, hub: Arc<Hub>) {
    let _ = stream.set_nodelay(true);
    let (submitter, events) = hub.register();
    let (read_half, write_half) = stream.into_split();

    let reader = tokio::spawn(reader_loop(read_half, submitter));
    let writer = tokio::spawn(writer_loop(write_half, events));

    let _ = reader.await;
    writer.abort();
    let _ = writer.await;
}

async fn reader_loop(mut r: OwnedReadHalf, submitter: Submitter) -> io::Result<()> {
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
        submitter.submit(cmd);
    }
}

async fn writer_loop(
    mut w: OwnedWriteHalf,
    mut events: UnboundedReceiver<Event>,
) -> io::Result<()> {
    let mut buf = Vec::with_capacity(64);
    while let Some(e) = events.recv().await {
        buf.clear();
        encode_server(&ServerMessage::Execution(e), &mut buf);
        w.write_all(&buf).await?;
    }
    Ok(())
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
