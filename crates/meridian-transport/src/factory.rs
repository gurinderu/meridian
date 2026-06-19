use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use crate::mcp::ToolRegistry;
use crate::pool::{IsolationKey, ProcessFactory};
use crate::process::{spawn, CliProcess};
use crate::spawn::SpawnConfig;

/// Spawns real `claude` CLI processes for the pool. Each isolation key maps to
/// a per-profile `CLAUDE_CONFIG_DIR` under `config_root`.
pub struct CliProcessFactory {
    exe: String,
    config_root: PathBuf,
    tools: Arc<dyn ToolRegistry>,
}

pub fn new(exe: impl Into<String>, config_root: PathBuf, tools: Arc<dyn ToolRegistry>) -> CliProcessFactory {
    CliProcessFactory { exe: exe.into(), config_root, tools }
}

impl ProcessFactory for CliProcessFactory {
    type Proc = CliProcess;

    async fn spawn(&self, key: &IsolationKey) -> std::io::Result<CliProcess> {
        let config_dir = self.config_root.join(&key.profile_id);
        std::fs::create_dir_all(&config_dir)?;
        let cfg = SpawnConfig {
            config_dir,
            model: None,
            mcp_config: None,
            include_partial_messages: true,
            resume: key.resume.clone(),
        };
        let base_env: HashMap<String, String> = std::env::vars().collect();
        spawn(&self.exe, &cfg, &base_env, self.tools.clone()).await
    }
}
