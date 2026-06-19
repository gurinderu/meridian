use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use meridian::pooled_runner::pooled_runner;
use meridian::server::router;
use meridian::session::SessionStore;
use meridian::service::health_check;

#[derive(Parser)]
#[command(name = "meridian", about = "Local proxy exposing Claude Code as the Anthropic + OpenAI APIs")]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the proxy server (default).
    Serve(ServeArgs),
    /// Check whether a running server answers /health.
    Status {
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
    /// Install + start meridian as a background OS service (launchd/systemd user).
    Install {
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
    /// Stop + remove the background OS service.
    Uninstall,
}

#[derive(clap::Args)]
struct ServeArgs {
    #[arg(long, default_value_t = 8787)]
    port: u16,
    #[arg(long, default_value = "claude")]
    claude: String,
    #[arg(long, default_value_t = 10)]
    cap: usize,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        None => serve(ServeArgs { port: 8787, claude: "claude".into(), cap: 10 }).await,
        Some(Cmd::Serve(a)) => serve(a).await,
        Some(Cmd::Status { port }) => {
            if health_check(port).await {
                println!("meridian: up (127.0.0.1:{port})");
            } else {
                println!("meridian: down (127.0.0.1:{port})");
                std::process::exit(1);
            }
        }
        Some(Cmd::Install { port }) => match meridian::service::install(port) {
            Ok(path) => println!("meridian: installed background service -> {path}\n  check: meridian status --port {port}"),
            Err(e) => { eprintln!("meridian: install failed: {e}"); std::process::exit(1); }
        },
        Some(Cmd::Uninstall) => {
            let _ = meridian::service::uninstall();
            println!("meridian: background service removed");
        }
    }
}

async fn serve(args: ServeArgs) {
    tracing_subscriber::fmt::init();
    let config_root: PathBuf = std::env::temp_dir().join("meridian-config");
    let runner = Arc::new(pooled_runner(args.claude, config_root, args.cap));
    let sessions = Arc::new(SessionStore::new());
    let app = router(runner, sessions);
    let addr = format!("0.0.0.0:{}", args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    tracing::info!("meridian listening on {addr}");
    axum::serve(listener, app).await.expect("serve");
}
