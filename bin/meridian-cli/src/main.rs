use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use meridian::pooled_runner::pooled_runner;
use meridian::server::router;
use meridian::session::SessionStore;
use meridian::service::health_check;

const DEFAULT_PORT: u16 = 8787;

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
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
    },
    /// Install + start meridian as a background OS service (launchd/systemd user).
    Install {
        #[arg(long, default_value_t = DEFAULT_PORT)]
        port: u16,
    },
    /// Stop + remove the background OS service.
    Uninstall,
    /// Manage authentication profiles.
    Profile {
        #[command(subcommand)]
        action: ProfileCmd,
    },
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// List all configured profiles.
    List,
    /// Switch the active profile (tells the running proxy).
    Use {
        id: String,
    },
    /// Remove a profile from profiles.json.
    Remove {
        id: String,
    },
    /// Add a profile with an OAuth token.
    Add {
        id: String,
        /// Long-lived OAuth token (sk-ant-oat-…). Omit to read from stdin.
        #[arg(long = "oauth-token")]
        oauth_token: Option<String>,
    },
}

#[derive(clap::Args)]
struct ServeArgs {
    #[arg(long, default_value_t = DEFAULT_PORT)]
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
        None => serve(ServeArgs { port: DEFAULT_PORT, claude: "claude".into(), cap: 10 }).await,
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
        Some(Cmd::Profile { action }) => run_profile(action).await,
    }
}

async fn serve(args: ServeArgs) {
    tracing_subscriber::fmt::init();
    let config_root: PathBuf = std::env::temp_dir().join("meridian-config");
    let mut store = meridian::profiles::ProfileStore::from_env_or_disk(config_root.clone());
    if std::env::var("MERIDIAN_PROFILES").is_err() {
        store = store.with_disk_discovery();
    }
    let profiles = Arc::new(store);
    profiles.restore_active();
    let runner = Arc::new(pooled_runner(args.claude, config_root, args.cap, profiles.clone()));
    let sessions = Arc::new(SessionStore::new());
    let app = router(runner, sessions, profiles);
    let addr = format!("0.0.0.0:{}", args.port);
    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    tracing::info!("meridian listening on {addr}");
    axum::serve(listener, app).await.expect("serve");
}

async fn run_profile(action: ProfileCmd) {
    use meridian::profile_cli::{add_oauth_token, dirs_to_remove_on_remove,
        load_profiles_json_at, profiles_dir, profiles_json_path, save_profiles_json_at};

    match action {
        ProfileCmd::List => {
            let path = match profiles_json_path() {
                Some(p) => p,
                None => { eprintln!("Cannot determine home directory."); std::process::exit(1); }
            };
            let profiles = load_profiles_json_at(&path);
            if profiles.is_empty() {
                println!("No profiles configured.");
                println!("  Add one: meridian profile add <id> --oauth-token <token>");
            } else {
                for p in &profiles {
                    let kind = p.kind.map(|k| {
                        match k {
                            meridian::profiles::ProfileType::ClaudeMax => "claude-max",
                            meridian::profiles::ProfileType::Api => "api",
                            meridian::profiles::ProfileType::OauthToken => "oauth-token",
                        }
                    }).unwrap_or("claude-max");
                    println!("{} ({})", p.id, kind);
                }
            }
        }
        ProfileCmd::Add { id, oauth_token } => {
            // browser login not yet supported — only --oauth-token in this slice (Phase 3d)
            let token = match oauth_token {
                Some(t) => t,
                None => {
                    // Not an error — the user just didn't opt into the token
                    // path. Exit 0 so shell `&&`-chains / CI don't trip.
                    println!("Browser login is not yet supported (Phase 3d).");
                    println!("Please supply a token directly: meridian profile add {} --oauth-token <token>", id);
                    std::process::exit(0);
                }
            };
            let path = match profiles_json_path() {
                Some(p) => p,
                None => { eprintln!("Cannot determine home directory."); std::process::exit(1); }
            };
            match add_oauth_token(&path, &id, &token) {
                Ok(()) => println!("Profile \"{}\" added (OAuth token).", id),
                Err(e) => { eprintln!("error: {e}"); std::process::exit(1); }
            }
        }
        ProfileCmd::Remove { id } => {
            let path = match profiles_json_path() {
                Some(p) => p,
                None => { eprintln!("Cannot determine home directory."); std::process::exit(1); }
            };
            let pdir = match profiles_dir() {
                Some(d) => d,
                None => { eprintln!("Cannot determine home directory."); std::process::exit(1); }
            };
            let mut profiles = load_profiles_json_at(&path);
            let idx = profiles.iter().position(|p| p.id == id);
            let Some(idx) = idx else {
                eprintln!("error: Profile \"{id}\" not found.");
                std::process::exit(1);
            };
            let removed = profiles.remove(idx);
            let to_rm = dirs_to_remove_on_remove(&removed, &pdir);
            if let Err(e) = save_profiles_json_at(&path, &profiles) {
                eprintln!("error: failed to save profiles.json: {e}");
                std::process::exit(1);
            }
            for d in &to_rm {
                if d.exists() {
                    if let Err(e) = std::fs::remove_dir_all(d) {
                        eprintln!("warning: could not remove {}: {e}", d.display());
                    }
                }
            }
            println!("Profile \"{}\" removed.", id);
        }
        ProfileCmd::Use { id } => {
            let host = std::env::var("MERIDIAN_HOST").unwrap_or_else(|_| "127.0.0.1".into());
            let port: u16 = std::env::var("MERIDIAN_PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(DEFAULT_PORT);
            match post_profiles_active(&host, port, &id).await {
                Ok(()) => {
                    meridian::settings::set_active_profile(&id);
                    println!("Switched to profile: {id}");
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            }
        }
    }
}

/// POST /profiles/active {"profile":<id>} to the running proxy.
/// Reuses the same tokio TcpStream / raw HTTP/1.1 approach as health_check.
async fn post_profiles_active(host: &str, port: u16, id: &str) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect((host, port)).await
        .map_err(|e| format!("Cannot connect to meridian at {host}:{port}: {e}"))?;
    let body = format!("{{\"profile\":\"{}\"}}", id.replace('"', "\\\""));
    let req = format!(
        "POST /profiles/active HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).await
        .map_err(|e| format!("Write error: {e}"))?;
    let mut buf = Vec::new();
    let _ = stream.read_to_end(&mut buf).await;
    let raw = String::from_utf8_lossy(&buf);
    // Check status line
    let status_line = raw.lines().next().unwrap_or("");
    let mut parts = status_line.split_whitespace();
    parts.next(); // HTTP/x.y
    let status = parts.next().unwrap_or("0");
    if status == "200" {
        Ok(())
    } else {
        // Extract the body (after \r\n\r\n) for the error message
        let body_part = raw.find("\r\n\r\n")
            .map(|i| &raw[i+4..])
            .unwrap_or(&raw);
        Err(format!("Server returned {status}: {body_part}"))
    }
}
