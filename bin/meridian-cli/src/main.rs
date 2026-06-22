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
    /// Add a profile. With --oauth-token, stores a long-lived token; otherwise
    /// runs a browser OAuth login (claude auth login, or --headless paste-code).
    Add {
        id: String,
        /// Long-lived OAuth token (sk-ant-oat-…). Omit to read from stdin.
        #[arg(long = "oauth-token")]
        oauth_token: Option<String>,
        /// Headless login: print an OAuth URL and prompt for the returned code
        /// instead of opening a browser. Ignored when --oauth-token is given.
        #[arg(long)]
        headless: bool,
    },
    /// Re-authenticate an existing browser-login (claude-max) profile.
    Login {
        id: String,
        #[arg(long)]
        headless: bool,
    },
}

#[derive(clap::Args)]
struct ServeArgs {
    #[arg(long, default_value_t = DEFAULT_PORT)]
    port: u16,
    /// Address to bind. Defaults to loopback; pass 0.0.0.0 to expose on the
    /// network (do that only behind MERIDIAN_API_KEY or an isolated host).
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value = "claude")]
    claude: String,
    #[arg(long, default_value_t = 10)]
    cap: usize,
    #[arg(long, default_value_t = 8)]
    max_parked: usize,
    #[arg(long = "park-ttl-secs", default_value_t = 300)]
    park_ttl_secs: u64,
    /// Cap the summed resident memory (MB) of parked processes; over it, the
    /// reaper evicts oldest-first. 0 = disabled (count + TTL caps only).
    /// Linux-only (reads /proc); a no-op elsewhere.
    #[arg(long = "max-parked-mem-mb", default_value_t = 0)]
    max_parked_mem_mb: u64,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        None => serve(ServeArgs { port: DEFAULT_PORT, host: "127.0.0.1".into(), claude: "claude".into(), cap: 10, max_parked: 8, park_ttl_secs: 300, max_parked_mem_mb: 0 }).await,
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
    // Per-user runtime config root (per-profile CLAUDE_CONFIG_DIRs live here).
    // Use ~/.config/meridian — NOT a world-writable /tmp path — and so the
    // oauth-token isolation dir (config_root/profiles/<id>) coincides with the
    // dir `meridian profile add` writes credentials to.
    let config_root: PathBuf = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".config").join("meridian"))
        .unwrap_or_else(|_| std::env::temp_dir().join("meridian-config"));
    let mut store = meridian::profiles::ProfileStore::from_env_or_disk(config_root.clone());
    if std::env::var("MERIDIAN_PROFILES").is_err() {
        store = store.with_disk_discovery();
    }
    let profiles = Arc::new(store);
    profiles.restore_active();
    let rate_limit = std::sync::Arc::new(meridian::rate_limit::RateLimitStore::new());
    let runner = Arc::new(pooled_runner(args.claude, config_root, args.cap, profiles.clone(), rate_limit.clone(), args.max_parked));
    {
        let runner = runner.clone();
        let ttl = std::time::Duration::from_secs(args.park_ttl_secs);
        let tick = std::time::Duration::from_secs(args.park_ttl_secs.clamp(5, 60));
        let mem_budget = args.max_parked_mem_mb.saturating_mul(1024 * 1024);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tick).await;
                runner.reap_parked(ttl).await;
                if mem_budget > 0 {
                    runner.reap_parked_over_mem(mem_budget).await;
                }
            }
        });
    }
    let sessions = Arc::new(SessionStore::new());
    let app = router(runner, sessions, profiles, rate_limit);
    // Keep the default account's OAuth refresh token warm even when the proxy
    // is idle (Anthropic invalidates a refresh token left unused past expiry).
    const FIVE_MIN_MS: i64 = 5 * 60 * 1000;
    meridian::token_refresh::start_background_refresh(None, FIVE_MIN_MS, FIVE_MIN_MS);
    let addr = format!("{}:{}", args.host, args.port);
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
        ProfileCmd::Login { id, headless } => {
            let path = match profiles_json_path() {
                Some(p) => p,
                None => { eprintln!("Cannot determine home directory."); std::process::exit(1); }
            };
            let profiles = load_profiles_json_at(&path);
            let Some(p) = profiles.iter().find(|p| p.id == id) else {
                eprintln!("error: Profile \"{id}\" not found. Run: meridian profile add {id}");
                std::process::exit(1);
            };
            if p.oauth_token.is_some() || p.kind == Some(meridian::profiles::ProfileType::OauthToken) {
                eprintln!("error: Profile \"{id}\" uses an OAuth token; `claude auth login` does not apply.");
                std::process::exit(1);
            }
            let dir = match p.claude_config_dir.clone() {
                Some(d) => std::path::PathBuf::from(d),
                None => match profiles_dir() { Some(d) => d.join(&id), None => { eprintln!("Cannot determine home directory."); std::process::exit(1); } },
            };
            if !perform_login(&dir, headless) {
                eprintln!("error: Login failed.");
                std::process::exit(1);
            }
            println!("Profile \"{id}\" re-authenticated.");
        }
        ProfileCmd::Add { id, oauth_token, headless } => {
            let path = match profiles_json_path() {
                Some(p) => p,
                None => { eprintln!("Cannot determine home directory."); std::process::exit(1); }
            };
            // No token → browser OAuth login (claude auth login or --headless).
            let Some(token) = oauth_token else {
                if !meridian::profile_cli::is_valid_profile_id(&id) {
                    eprintln!("error: Invalid profile ID. Use only letters, numbers, hyphens, underscores.");
                    std::process::exit(1);
                }
                let mut profiles = load_profiles_json_at(&path);
                if profiles.iter().any(|p| p.id == id) {
                    eprintln!("error: Profile \"{id}\" already exists.");
                    std::process::exit(1);
                }
                let dir = match profiles_dir() { Some(d) => d.join(&id), None => { eprintln!("Cannot determine home directory."); std::process::exit(1); } };
                let _ = std::fs::create_dir_all(&dir);
                if !perform_login(&dir, headless) {
                    eprintln!("error: Login did not complete. Try again: meridian profile add {id}");
                    std::process::exit(1);
                }
                profiles.push(meridian::profiles::ProfileConfig {
                    id: id.clone(),
                    kind: Some(meridian::profiles::ProfileType::ClaudeMax),
                    claude_config_dir: Some(dir.to_string_lossy().into_owned()),
                    api_key: None, base_url: None, oauth_token: None,
                });
                if let Err(e) = save_profiles_json_at(&path, &profiles) {
                    eprintln!("error: failed to save profiles.json: {e}");
                    std::process::exit(1);
                }
                println!("Profile \"{id}\" created.");
                return;
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

/// Run a browser OAuth login into `config_dir` (interactive `claude auth login`
/// or the headless paste-code flow), then verify it with `claude auth status`.
/// Returns true when the dir ends up authenticated.
fn perform_login(config_dir: &std::path::Path, headless: bool) -> bool {
    if headless {
        if let Err(e) = headless_oauth_login(config_dir) {
            eprintln!("error: {e}");
            return false;
        }
    } else {
        // `claude auth login` does the whole browser OAuth itself; inherit stdio
        // so the user interacts with it directly.
        match std::process::Command::new("claude")
            .args(["auth", "login"])
            .env("CLAUDE_CONFIG_DIR", config_dir)
            .status()
        {
            Ok(s) if s.success() => {}
            _ => { eprintln!("error: `claude auth login` failed."); return false; }
        }
    }
    claude_auth_status(config_dir)
}

/// True when `claude auth status` reports loggedIn for this config dir.
fn claude_auth_status(config_dir: &std::path::Path) -> bool {
    match std::process::Command::new("claude")
        .args(["auth", "status"])
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .output()
    {
        Ok(o) if o.status.success() => {
            meridian::oauth::auth_status_logged_in(&String::from_utf8_lossy(&o.stdout))
        }
        _ => false,
    }
}

/// Headless OAuth: print the authorize URL, read the pasted code from stdin,
/// exchange it, and write the credentials into `config_dir`'s store.
fn headless_oauth_login(config_dir: &std::path::Path) -> Result<(), String> {
    use std::io::Write;
    let session = meridian::oauth::new_oauth_session().map_err(|e| format!("could not start OAuth session: {e}"))?;
    println!("Open this URL in a browser, sign in, then paste the code shown:");
    println!();
    println!("{}", session.authorize_url);
    println!();
    print!("Paste code: ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| format!("read failed: {e}"))?;
    let parsed = meridian::oauth::parse_authorization_code(&line).ok_or("no authorization code received")?;
    // Require the returned state and bind it to our session (CSRF protection).
    // The claude.com callback always returns `code#state`, so this does not
    // break the real flow — it only rejects a stateless paste, which is exactly
    // the case worth refusing. (Deliberately stricter than the TS original,
    // whose state check was optional — a latent weakness.)
    let received_state = parsed.state.as_deref()
        .ok_or("OAuth response missing the required state parameter")?;
    if received_state != session.state {
        return Err("OAuth state mismatch — please retry the login".into());
    }
    let token = meridian::oauth::exchange_code(&parsed.code, &session.code_verifier, &session.state)
        .ok_or("OAuth token exchange failed")?;
    let creds = meridian::oauth::build_credentials_file(&token, meridian::token_refresh::now_ms())
        .ok_or("OAuth response did not include the required tokens")?;
    let store = meridian::token_refresh::create_platform_credential_store(config_dir.to_str());
    if !store.write(&creds) {
        return Err("failed to write credentials".into());
    }
    Ok(())
}

/// POST /profiles/active {"profile":<id>} to the running proxy.
/// Reuses the same tokio TcpStream / raw HTTP/1.1 approach as health_check.
async fn post_profiles_active(host: &str, port: u16, id: &str) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let dur = std::time::Duration::from_secs(5);
    let mut stream = tokio::time::timeout(dur, tokio::net::TcpStream::connect((host, port))).await
        .map_err(|_| format!("Timed out connecting to meridian at {host}:{port}"))?
        .map_err(|e| format!("Cannot connect to meridian at {host}:{port}: {e}"))?;
    let body = format!("{{\"profile\":\"{}\"}}", id.replace('"', "\\\""));
    let req = format!(
        "POST /profiles/active HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(req.as_bytes()).await
        .map_err(|e| format!("Write error: {e}"))?;
    let mut buf = Vec::new();
    // Bound the read — a non-responsive peer must not hang `profile use` forever.
    tokio::time::timeout(dur, stream.read_to_end(&mut buf)).await
        .map_err(|_| "Timed out waiting for meridian's response".to_string())?
        .map_err(|e| format!("Read error: {e}"))?;
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
