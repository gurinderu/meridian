use std::sync::Arc;

use clap::Parser;
use meridian::pooled_runner::pooled_runner;
use meridian::server::router;

#[derive(Parser)]
struct Args {
    /// Port to bind.
    #[arg(long, default_value_t = 8787)]
    port: u16,
    /// Path to the `claude` executable.
    #[arg(long, default_value = "claude")]
    claude: String,
    /// Max concurrent pooled processes.
    #[arg(long, default_value_t = 10)]
    cap: usize,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let args = Args::parse();
    let config_root = std::env::temp_dir().join("meridian-config");
    let runner = Arc::new(pooled_runner(args.claude, config_root, args.cap));
    let sessions = std::sync::Arc::new(meridian::session::SessionStore::new());
    let app = router(runner, sessions);

    let addr = format!("0.0.0.0:{}", args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    tracing::info!("meridian listening on {addr}");
    axum::serve(listener, app).await.expect("serve");
}
