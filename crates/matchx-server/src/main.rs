//! matchx server entry point. Binds to `127.0.0.1:9000` by default; pass
//! a single arg to override (`matchx-server 0.0.0.0:9000`).
//!
//! Logging is `tracing` + `tracing-subscriber` formatter. Set log level
//! via `RUST_LOG` (e.g. `RUST_LOG=info,matchx_server=debug`).

use matchx_server::{Config, bind, serve};
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
    serve(listener, Config::default()).await
}
