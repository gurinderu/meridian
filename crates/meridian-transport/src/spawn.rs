use std::collections::HashMap;
use std::path::PathBuf;

pub struct SpawnConfig {
    pub config_dir: PathBuf,
    pub model: Option<String>,
    pub mcp_config: Option<serde_json::Value>,
    pub include_partial_messages: bool,
}

/// Confirmed base flags (live CLI + spike). Isolation is via env/SDK options,
/// NOT --strict-mcp-config (see Task 2 findings).
pub fn build_args(cfg: &SpawnConfig) -> Vec<String> {
    let mut a: Vec<String> = vec![
        "--output-format", "stream-json",
        "--verbose",
        "--input-format", "stream-json",
        "--permission-mode", "bypassPermissions",
    ].into_iter().map(String::from).collect();
    if cfg.include_partial_messages {
        a.push("--include-partial-messages".into());
    }
    if let Some(m) = &cfg.model {
        a.push("--model".into());
        a.push(m.clone());
    }
    if let Some(mcp) = &cfg.mcp_config {
        a.push("--mcp-config".into());
        a.push(mcp.to_string());
    }
    a
}

const STRIP: &[&str] = &["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "NODE_OPTIONS"];

pub fn build_env(cfg: &SpawnConfig, base: &HashMap<String, String>) -> HashMap<String, String> {
    let mut env: HashMap<String, String> = base
        .iter()
        .filter(|(k, _)| !STRIP.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    env.insert("CLAUDE_CONFIG_DIR".into(), cfg.config_dir.to_string_lossy().into_owned());
    // Setting CLAUDE_CONFIG_DIR alone makes the CLI derive a per-config-dir
    // macOS keychain key (a `-<hash>` suffix), so the default OAuth token is
    // not found -> 401 -> the API call fails and NO stream_event partials are
    // emitted (reverse-engineered from cli.js `m1()`). Setting
    // CLAUDE_SECURESTORAGE_CONFIG_DIR="" realigns the keychain key to the
    // default entry, so auth succeeds and streaming partials flow under an
    // isolated config dir. (Per-profile auth uses CLAUDE_CODE_OAUTH_TOKEN
    // instead; that bypasses the keychain entirely — a profiles-phase concern.)
    env.insert("CLAUDE_SECURESTORAGE_CONFIG_DIR".into(), String::new());
    env
}
