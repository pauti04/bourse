//! bourse test client and load generator.
//!
//! Two measurements:
//!
//! 1. **RTT** — sequential round-trip latency. For each iteration:
//!    rest a Sell at price 100, then send a Buy at 100 and time
//!    until `Done(Filled)` for that Buy. No pipelining; reflects
//!    actual one-order end-to-end latency.
//! 2. **Throughput** — pipelined burst. Fire `n` alternating orders
//!    as fast as the socket accepts them; time wall clock to drain
//!    every server response.
//!
//! Usage:
//!
//! ```text
//! bourse-client [addr] [n_rtt] [n_throughput]
//! ```
//!
//! Defaults: `127.0.0.1:9000`, `10000` RTT iters, `100000` throughput
//! orders.

#![allow(
    clippy::print_stdout,
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "CLI tool prints stats; unwrap is fine for argv parsing in a demo binary"
)]

use std::time::Instant;

use bourse_core::matcher::{DoneReason, Event, NewOrder, OrderKind};
use bourse_core::types::{OrderId, Price, Qty, Side, Timestamp};
use bourse_protocol::{ClientMessage, ServerMessage, encode_client};
use bourse_server::read_one_server_message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn limit(id: u64, side: Side, price: i64, qty: u64) -> ClientMessage {
    ClientMessage::NewOrder(NewOrder {
        id: OrderId::new(id),
        side,
        qty: Qty::new(qty),
        kind: OrderKind::Limit {
            price: Price::from_raw(price),
        },
        timestamp: Timestamp::EPOCH,
    })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> std::io::Result<()> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:9000".into());
    let n_rtt: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    let n_throughput: usize = std::env::args()
        .nth(3)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);

    println!("connecting to {addr} ...");

    rtt_bench(&addr, n_rtt).await?;
    println!();
    throughput_bench(&addr, n_throughput).await?;

    Ok(())
}

/// Sequential RTT: rest a Sell, then time send-Buy → Done(taker filled).
async fn rtt_bench(addr: &str, n: usize) -> std::io::Result<()> {
    let stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    let (mut reader, mut writer) = stream.into_split();
    let mut buf = Vec::with_capacity(64);

    // 100 warmup iters so caches are hot and TCP/jit overhead amortises.
    let warmup = 100.min(n);
    for i in 0..warmup as u64 {
        rtt_one(&mut reader, &mut writer, &mut buf, i * 2 + 1, i * 2 + 2).await?;
    }

    let mut latencies = Vec::with_capacity(n);
    let base = (warmup as u64) * 2 + 1;
    for i in 0..n as u64 {
        let sell_id = base + i * 2;
        let buy_id = sell_id + 1;
        let lat = rtt_one(&mut reader, &mut writer, &mut buf, sell_id, buy_id).await?;
        latencies.push(lat);
    }

    print_histogram("RTT (sequential)", &mut latencies);
    Ok(())
}

async fn rtt_one(
    reader: &mut tokio::net::tcp::OwnedReadHalf,
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    buf: &mut Vec<u8>,
    sell_id: u64,
    buy_id: u64,
) -> std::io::Result<u64> {
    // Rest the Sell.
    buf.clear();
    encode_client(&limit(sell_id, Side::Sell, 100, 1), buf);
    writer.write_all(buf).await?;
    // Drain until we see Accepted(sell).
    drain_until(
        reader,
        |e| matches!(e, Event::Accepted { id, .. } if *id == OrderId::new(sell_id)),
    )
    .await?;

    // Send the Buy and time.
    buf.clear();
    encode_client(&limit(buy_id, Side::Buy, 100, 1), buf);
    let start = Instant::now();
    writer.write_all(buf).await?;
    drain_until(reader, |e| {
        matches!(
            e,
            Event::Done { id, reason: DoneReason::Filled, .. } if *id == OrderId::new(buy_id)
        )
    })
    .await?;
    Ok(start.elapsed().as_nanos() as u64)
}

async fn drain_until<R, F>(reader: &mut R, mut stop: F) -> std::io::Result<()>
where
    R: AsyncReadExt + Unpin,
    F: FnMut(&Event) -> bool,
{
    loop {
        match read_one_server_message(reader).await? {
            Some(ServerMessage::Execution(e)) => {
                if stop(&e) {
                    return Ok(());
                }
            }
            None => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "server closed connection",
                ));
            }
        }
    }
}

/// Pipelined burst: write `n` alternating Sell/Buy orders, then drain
/// every server response. Reports wall-clock throughput; per-order
/// latency under this regime is dominated by queueing and isn't useful
/// in isolation.
async fn throughput_bench(addr: &str, n: usize) -> std::io::Result<()> {
    let stream = TcpStream::connect(addr).await?;
    stream.set_nodelay(true)?;
    let (mut reader, mut writer) = stream.into_split();
    let mut buf = Vec::with_capacity(n * 32);

    // Build the whole burst into one big buffer up-front so we measure
    // the path, not the encoder.
    for i in 1..=n as u64 {
        let side = if i % 2 == 1 { Side::Sell } else { Side::Buy };
        encode_client(&limit(i, side, 100, 1), &mut buf);
    }

    // Drain task — counts every Done(Filled) from the Buy ids (even).
    let target_dones = n / 2;
    let recv_task = tokio::spawn(async move {
        let mut got = 0usize;
        while got < target_dones {
            match read_one_server_message(&mut reader).await? {
                Some(ServerMessage::Execution(Event::Done {
                    id,
                    reason: DoneReason::Filled,
                    ..
                })) if id.get() % 2 == 0 => {
                    got += 1;
                }
                Some(_) => {}
                None => break,
            }
        }
        Ok::<usize, std::io::Error>(got)
    });

    let start = Instant::now();
    writer.write_all(&buf).await?;
    let got = recv_task.await.unwrap()?;
    let elapsed = start.elapsed();

    println!("throughput (pipelined burst):");
    println!("  orders submitted:   {n}");
    println!("  Done(Filled) seen:  {got}");
    println!("  wall time:          {:.2?}", elapsed);
    println!(
        "  rate:               {:.0} orders/sec ({:.0} round-trips/sec)",
        (n as f64) / elapsed.as_secs_f64(),
        (got as f64) / elapsed.as_secs_f64()
    );
    Ok(())
}

fn print_histogram(label: &str, latencies: &mut [u64]) {
    latencies.sort_unstable();
    let n = latencies.len();
    let p = |q: f64| -> u64 {
        let idx = ((n as f64) * q) as usize;
        latencies.get(idx.min(n - 1)).copied().unwrap_or(0)
    };
    println!("{label}:");
    println!("  samples:    {n}");
    println!("  p50:        {} ns", p(0.50));
    println!("  p90:        {} ns", p(0.90));
    println!("  p99:        {} ns", p(0.99));
    println!("  p99.9:      {} ns", p(0.999));
    println!(
        "  max:        {} ns",
        latencies.last().copied().unwrap_or(0)
    );
}
