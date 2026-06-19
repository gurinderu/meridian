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
    env
}
