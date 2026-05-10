//! matchx server entry point. Binds to `127.0.0.1:9000` by default; pass
//! a single arg to override (`matchx-server 0.0.0.0:9000`).

use matchx_server::{Config, bind, serve};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let addr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:9000".into());
    let listener = bind(&addr).await?;
    serve(listener, Config::default()).await
}
