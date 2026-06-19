use std::collections::HashMap;
use std::path::PathBuf;
use serde_json::{json, Value};

pub struct SpawnConfig {
    pub config_dir: PathBuf,
    pub model: Option<String>,
    pub mcp_config: Option<serde_json::Value>,
    pub include_partial_messages: bool,
    pub resume: Option<String>,
    pub max_turns: Option<u32>,
    /// Per-profile env vars overlaid on the subprocess environment. Applied
    /// LAST in `build_env` so a profile can override both the host-var strip
    /// (e.g. `ANTHROPIC_API_KEY` for an api profile) and the base
    /// `CLAUDE_CONFIG_DIR` (e.g. an oauth-token profile's isolated dir).
    pub env_overlay: HashMap<String, String>,
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
    if let Some(r) = &cfg.resume {
        a.push("--resume".into());
        a.push(r.clone());
    }
    if let Some(n) = cfg.max_turns {
        a.push("--max-turns".into());
        a.push(n.to_string());
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
    // isolated config dir.
    //
    // BUT: the realignment makes the isolated config dir resolve the HOST's
    // default OAuth token. That is correct for the default / claude-max path,
    // where we want host auth. When a profile supplies its OWN credentials
    // (ANTHROPIC_API_KEY for api profiles, CLAUDE_CODE_OAUTH_TOKEN for
    // oauth-token profiles), we want the profile's config dir kept ISOLATED from
    // the host keychain so only the profile's auth is in play. So skip the
    // realignment whenever the overlay carries explicit auth. (Empirically, in
    // streaming mode the CLI honors the overlay's ANTHROPIC_BASE_URL/key and does
    // NOT fall back to host OAuth on failure — an unreachable base_url surfaces as
    // an `is_error` result, which pooled_runner maps to a non-2xx upstream error.)
    let overlay_has_auth = cfg.env_overlay.contains_key("ANTHROPIC_API_KEY")
        || cfg.env_overlay.contains_key("CLAUDE_CODE_OAUTH_TOKEN");
    if !overlay_has_auth {
        env.insert("CLAUDE_SECURESTORAGE_CONFIG_DIR".into(), String::new());
    }
    // Profile overlay wins last: survives the strip above and overrides the
    // base CLAUDE_CONFIG_DIR for oauth-token profiles.
    for (k, v) in &cfg.env_overlay {
        env.insert(k.clone(), v.clone());
    }
    env
}

/// Build the `initialize` control_request for a registry, or `None` when it
/// wants neither in-process MCP servers nor a PreToolUse hook.
pub fn build_initialize(tools: &dyn crate::mcp::ToolRegistry) -> Option<Value> {
    let servers = tools.sdk_mcp_servers();
    let wants_hook = tools.wants_pre_tool_use_hook();
    if servers.is_empty() && !wants_hook {
        return None;
    }
    let mut req = json!({ "subtype": "initialize" });
    if !servers.is_empty() {
        req["sdkMcpServers"] = json!(servers);
    }
    if wants_hook {
        req["hooks"] = json!({
            "PreToolUse": [{ "matcher": "", "hookCallbackIds": ["pre-tool-use"], "timeout": 60 }]
        });
    }
    Some(json!({ "type": "control_request", "request_id": "meridian-init", "request": req }))
}
