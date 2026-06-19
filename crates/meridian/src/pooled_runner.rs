use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use meridian_transport::codec::CliMessage;
use meridian_transport::factory::{self};
use meridian_transport::mcp::ToolRegistry;
use meridian_transport::pool::{IsolationKey, Pool};

use crate::error::ProxyError;
use crate::server::{TurnRequest, TurnResult, TurnRunner};

struct NoTools;
impl ToolRegistry for NoTools {
    fn list(&self) -> Vec<Value> { vec![] }
    fn call(&self, _n: &str, _a: &Value) -> Value { serde_json::json!({}) }
}

pub struct PooledRunner {
    pool: std::sync::Arc<Pool<factory::CliProcessFactory>>,
    exe: String,
    config_root: PathBuf,
}

pub fn pooled_runner(exe: String, config_root: PathBuf, cap: usize) -> PooledRunner {
    let f = factory::new(exe.clone(), config_root.clone(), Arc::new(NoTools));
    PooledRunner { pool: std::sync::Arc::new(Pool::new(f, cap)), exe, config_root }
}

impl TurnRunner for PooledRunner {
    async fn run_turn(&self, req: TurnRequest) -> Result<TurnResult, ProxyError> {
        if !req.tools.is_empty() {
            return self.run_passthrough(req).await;
        }
        let key = IsolationKey {
            profile_id: "default".into(),
            cwd: "/".into(),
            options_hash: 0,
            resume: req.resume.clone(),
        };
        let mut lease = self
            .pool
            .acquire(&key)
            .await
            .map_err(|e| ProxyError::Upstream(format!("spawn failed: {e}")))?
            .ok_or_else(|| ProxyError::Upstream("pool at capacity".into()))?;

        let result = run_one_turn(lease.proc(), req.system, req.prompt).await;
        lease.proc().shutdown().await;
        lease.discard(); // a shut-down process must not return to the warm idle set
        result
    }
}

impl PooledRunner {
    async fn run_passthrough(&self, req: TurnRequest) -> Result<TurnResult, ProxyError> {
        use std::collections::HashMap;
        let config_dir = self.config_root.join("default");
        let _ = std::fs::create_dir_all(&config_dir);
        let cfg = meridian_transport::spawn::SpawnConfig {
            config_dir,
            model: None,
            mcp_config: Some(serde_json::json!({"mcpServers":{"oc":{"type":"sdk","name":"oc"}}})),
            include_partial_messages: false,
            resume: req.resume.clone(),
            max_turns: Some(3),
        };
        let tools = Arc::new(meridian_transport::passthrough::new(req.tools.clone()));
        let base: HashMap<String, String> = std::env::vars().collect();
        let mut proc = meridian_transport::process::spawn(&self.exe, &cfg, &base, tools.clone())
            .await
            .map_err(|e| ProxyError::Upstream(format!("spawn failed: {e}")))?;
        let result = run_one_turn(&mut proc, req.system, req.prompt).await;
        proc.shutdown().await;
        match result {
            Ok(mut tr) => {
                tr.captured_tools = tools.captured();
                Ok(tr)
            }
            Err(e) => Err(e),
        }
    }
}

use crate::server::StreamRunner;
use crate::sse::EventStream;
use serde_json::json;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

impl StreamRunner for PooledRunner {
    fn run_stream(&self, _model: String, system: Option<String>, prompt: String) -> EventStream {
        let (tx, rx) = mpsc::channel::<Value>(64);
        let pool = self.pool.clone();
        tokio::spawn(async move {
            let key = IsolationKey { profile_id: "default".into(), cwd: "/".into(), options_hash: 0, resume: None };
            let mut lease = match pool.acquire(&key).await {
                Ok(Some(l)) => l,
                Ok(None) => { let _ = tx.send(error_event("pool at capacity")).await; return; }
                Err(e) => { let _ = tx.send(error_event(&format!("spawn failed: {e}"))).await; return; }
            };
            let full = match system {
                Some(s) if !s.is_empty() => format!("{s}\n\n{prompt}"),
                _ => prompt,
            };
            if let Err(e) = lease.proc().send_user_turn(&full).await {
                let _ = tx.send(error_event(&format!("write failed: {e}"))).await;
                lease.proc().shutdown().await;
                lease.discard();
                return;
            }
            let pump = async {
                while let Some(ev) = lease.proc().next_event().await {
                    match ev {
                        CliMessage::StreamEvent { event, .. } => {
                            let disconnected = tx.send(event).await.is_err();
                            if disconnected { break; } // client disconnected
                        }
                        CliMessage::Result { .. } => break,
                        _ => {}
                    }
                }
            };
            if tokio::time::timeout(std::time::Duration::from_secs(300), pump).await.is_err() {
                let _ = tx.send(error_event("upstream timeout")).await;
            }
            lease.proc().shutdown().await;
            lease.discard();
        });
        ReceiverStream::new(rx)
    }
}

fn error_event(message: &str) -> Value {
    json!({"type":"error","error":{"type":"api_error","message":message}})
}

/// Drives one prompt to completion on an already-acquired process. The caller
/// is responsible for shutting the process down and discarding the lease.
async fn run_one_turn(
    proc: &mut meridian_transport::process::CliProcess,
    system: Option<String>,
    prompt: String,
) -> Result<TurnResult, ProxyError> {
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
        let mut session_id: Option<String> = None;
        while let Some(ev) = proc.next_event().await {
            match ev {
                CliMessage::Init { session_id: sid, .. } => session_id = Some(sid),
                CliMessage::Assistant { message, .. } => last_message = Some(message),
                CliMessage::Result { raw, .. } => {
                    stop_reason = raw.get("stop_reason").and_then(Value::as_str).map(str::to_string);
                    break;
                }
                _ => {}
            }
        }
        (last_message, stop_reason, session_id)
    };

    let (last_message, stop_reason, session_id) =
        tokio::time::timeout(std::time::Duration::from_secs(300), collect)
            .await
            .map_err(|_| ProxyError::Upstream("upstream timeout".into()))?;

    let mut msg = last_message
        .ok_or_else(|| ProxyError::Upstream("no assistant message produced".into()))?;
    if let (Some(obj), Some(sr)) = (msg.as_object_mut(), stop_reason) {
        obj.insert("stop_reason".into(), Value::String(sr));
    }
    Ok(TurnResult { message: msg, session_id, captured_tools: Vec::new() })
}
