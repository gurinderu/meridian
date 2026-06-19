use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use meridian_transport::codec::CliMessage;
use meridian_transport::factory::{self};
use meridian_transport::mcp::ToolRegistry;
use meridian_transport::pool::{IsolationKey, Pool};

use crate::error::ProxyError;
use crate::server::TurnRunner;

struct NoTools;
impl ToolRegistry for NoTools {
    fn list(&self) -> Vec<Value> { vec![] }
    fn call(&self, _n: &str, _a: &Value) -> Value { serde_json::json!({}) }
}

pub struct PooledRunner {
    pool: Pool<factory::CliProcessFactory>,
}

pub fn pooled_runner(exe: String, config_root: PathBuf, cap: usize) -> PooledRunner {
    let f = factory::new(exe, config_root, Arc::new(NoTools));
    PooledRunner { pool: Pool::new(f, cap) }
}

impl TurnRunner for PooledRunner {
    async fn run_turn(&self, _model: String, system: Option<String>, prompt: String) -> Result<Value, ProxyError> {
        let key = IsolationKey { profile_id: "default".into(), cwd: "/".into(), options_hash: 0 };
        let mut lease = self
            .pool
            .acquire(&key)
            .await
            .map_err(|e| ProxyError::Upstream(format!("spawn failed: {e}")))?
            .ok_or_else(|| ProxyError::Upstream("pool at capacity".into()))?;

        let result = run_one_turn(lease.proc(), system, prompt).await;
        lease.proc().shutdown().await;
        lease.discard(); // a shut-down process must not return to the warm idle set
        result
    }
}

/// Drives one prompt to completion on an already-acquired process. The caller
/// is responsible for shutting the process down and discarding the lease.
async fn run_one_turn(
    proc: &mut meridian_transport::process::CliProcess,
    system: Option<String>,
    prompt: String,
) -> Result<Value, ProxyError> {
    let full = match system {
        Some(s) if !s.is_empty() => format!("{s}\n\n{prompt}"),
        _ => prompt,
    };
    proc.send_user_turn(&full)
        .await
        .map_err(|e| ProxyError::Upstream(format!("write failed: {e}")))?;

    let collect = async {
        let mut last_message: Option<Value> = None;
        let mut stop_reason: Option<String> = None;
        while let Some(ev) = proc.next_event().await {
            match ev {
                CliMessage::Assistant { message, .. } => last_message = Some(message),
                CliMessage::Result { raw, .. } => {
                    stop_reason = raw.get("stop_reason").and_then(Value::as_str).map(str::to_string);
                    break;
                }
                _ => {}
            }
        }
        (last_message, stop_reason)
    };

    let (last_message, stop_reason) =
        tokio::time::timeout(std::time::Duration::from_secs(300), collect)
            .await
            .map_err(|_| ProxyError::Upstream("upstream timeout".into()))?;

    let mut msg = last_message
        .ok_or_else(|| ProxyError::Upstream("no assistant message produced".into()))?;
    if let (Some(obj), Some(sr)) = (msg.as_object_mut(), stop_reason) {
        obj.insert("stop_reason".into(), Value::String(sr));
    }
    Ok(msg)
}
