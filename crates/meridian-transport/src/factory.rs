use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::mcp::ToolRegistry;
use crate::pool::{IsolationKey, ProcessFactory};
use crate::process::{spawn, CliProcess};
use crate::spawn::SpawnConfig;

/// Resolves the per-profile env overlay for a spawn. Implemented in the
/// `meridian` crate by the profile store; `NoEnv` is the no-profiles default.
/// Lives in transport so the factory can build the subprocess env without
/// depending on the `meridian` crate (which would be a dependency cycle).
pub trait EnvResolver: Send + Sync {
    fn overlay(&self, profile_id: &str) -> HashMap<String, String>;
}

/// No-op resolver — yields an empty overlay for every profile. The default
/// when no profiles are configured, preserving single-account behavior.
pub struct NoEnv;
impl EnvResolver for NoEnv {
    fn overlay(&self, _profile_id: &str) -> HashMap<String, String> {
        HashMap::new()
    }
}

/// Spawns real `claude` CLI processes for the pool. Each isolation key maps to
/// a per-profile `CLAUDE_CONFIG_DIR` under `config_root`, plus the profile's
/// env overlay from `resolver`.
pub struct CliProcessFactory {
    exe: String,
    config_root: PathBuf,
    tools: Arc<dyn ToolRegistry>,
    resolver: Arc<dyn EnvResolver>,
}

pub fn new(exe: impl Into<String>, config_root: PathBuf, tools: Arc<dyn ToolRegistry>) -> CliProcessFactory {
    new_with_resolver(exe, config_root, tools, Arc::new(NoEnv))
}

pub fn new_with_resolver(
    exe: impl Into<String>,
    config_root: PathBuf,
    tools: Arc<dyn ToolRegistry>,
    resolver: Arc<dyn EnvResolver>,
) -> CliProcessFactory {
    CliProcessFactory { exe: exe.into(), config_root, tools, resolver }
}

/// Sanitize a profile id into a single safe path segment so it can never escape
/// `config_root` (path traversal). Profile ids SHOULD be `[A-Za-z0-9_-]` (the
/// CLI enforces it on `profile add`), but ids loaded from a hand-edited
/// profiles.json / MERIDIAN_PROFILES are not validated — a `../x` or absolute id
/// would otherwise reach `config_root.join(..)` and escape. A well-formed id
/// maps to itself; any other char becomes `_`; empty becomes "default".
pub fn safe_profile_segment(profile_id: &str) -> String {
    let mut out: String = profile_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    if out.is_empty() {
        out.push_str("default");
    }
    out
}

impl ProcessFactory for CliProcessFactory {
    type Proc = CliProcess;

    async fn spawn(&self, key: &IsolationKey) -> std::io::Result<CliProcess> {
        let config_dir = self.config_root.join(safe_profile_segment(&key.profile_id));
        std::fs::create_dir_all(&config_dir)?;
        let cfg = SpawnConfig {
            config_dir,
            model: None,
            mcp_config: None,
            include_partial_messages: true,
            resume: key.resume.clone(),
            max_turns: None,
            env_overlay: self.resolver.overlay(&key.profile_id),
        };
        let base_env: HashMap<String, String> = std::env::vars().collect();
        spawn(&self.exe, &cfg, &base_env, self.tools.clone()).await
    }
}
