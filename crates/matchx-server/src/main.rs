//! matchx server entry point. Binds to `127.0.0.1:9000` by default; pass
//! a single arg to override (`matchx-server 0.0.0.0:9000`).
//!
//! Logging is `tracing` + `tracing-subscriber` formatter. Set log level
//! via `RUST_LOG` (e.g. `RUST_LOG=info,matchx_server=debug`).
//!
//! Stops cleanly on SIGINT (Ctrl-C) or SIGTERM: stops accepting new
//! connections, drains in-flight ones up to `Config::shutdown_grace`,
//! then exits.

#![allow(
    clippy::expect_used,
    reason = "signal handler install is unrecoverable; matches std::thread"
)]

use matchx_server::{Config, bind, serve_until};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:9000".into());

    let listener = bind(&addr).await?;
    tracing::info!(%addr, "matchx-server listening");
    serve_until(listener, Config::default(), shutdown_signal()).await
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    let mut sigint = signal(SignalKind::interrupt()).expect("install SIGINT handler");
    tokio::select! {
        _ = sigterm.recv() => tracing::info!("received SIGTERM"),
        _ = sigint.recv()  => tracing::info!("received SIGINT (Ctrl-C)"),
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("received Ctrl-C");
}
