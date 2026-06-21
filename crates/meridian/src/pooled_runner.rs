use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use meridian_transport::codec::CliMessage;
use meridian_transport::factory::{self};
use meridian_transport::mcp::ToolRegistry;
use meridian_transport::pool::{IsolationKey, Pool};
use meridian_transport::process::CliProcess;

use crate::error::ProxyError;
use crate::parked::ParkedStore;
use crate::profiles::ProfileStore;
use crate::rate_limit::RateLimitStore;
use crate::server::{TurnRequest, TurnResult, TurnRunner};

struct NoTools;
impl ToolRegistry for NoTools {
    fn list(&self) -> Vec<Value> { vec![] }
    fn call(&self, _n: &str, _a: &Value) -> Value { serde_json::json!({}) }
}

/// Profile id used when a request carries none (no profiles configured).
fn profile_id(req: &TurnRequest) -> String {
    req.profile.clone().unwrap_or_else(|| "default".into())
}

pub struct PooledRunner {
    pool: std::sync::Arc<Pool<factory::CliProcessFactory>>,
    exe: String,
    config_root: PathBuf,
    profiles: Arc<ProfileStore>,
    rate_limit: Arc<RateLimitStore>,
    parked: Arc<ParkedStore<CliProcess>>,
    max_parked: usize,
}

pub fn pooled_runner(exe: String, config_root: PathBuf, cap: usize, profiles: Arc<ProfileStore>, rate_limit: Arc<RateLimitStore>, max_parked: usize) -> PooledRunner {
    let f = factory::new_with_resolver(exe.clone(), config_root.clone(), Arc::new(NoTools), profiles.clone());
    PooledRunner {
        pool: std::sync::Arc::new(Pool::new(f, cap)),
        exe,
        config_root,
        profiles,
        rate_limit,
        parked: Arc::new(ParkedStore::new()),
        max_parked,
    }
}

impl PooledRunner {
    /// Expose the parked store (tests assert on `.len()`).
    pub fn parked(&self) -> Arc<ParkedStore<CliProcess>> {
        self.parked.clone()
    }

    /// Reap processes idle longer than `ttl` and shut them down.
    pub async fn reap_parked(&self, ttl: Duration) {
        for mut p in self.parked.reap(ttl) {
            p.shutdown().await;
        }
    }
}

impl TurnRunner for PooledRunner {
    async fn run_turn(&self, req: TurnRequest) -> Result<TurnResult, ProxyError> {
        if !req.tools.is_empty() {
            return self.run_passthrough(req).await;
        }
        let key_profile = profile_id(&req);

        // --- Warm path: try to reuse a parked process for this (profile, session) ---
        if let Some(sid) = &req.resume {
            if let Some(mut proc) = self.parked.take(&key_profile, sid) {
                if proc.is_alive() {
                    let result = run_one_turn(&mut proc, req.system.clone(), req.prompt.clone(), &self.rate_limit).await;
                    match result {
                        Ok(turn) => {
                            if let Some(new_sid) = &turn.session_id {
                                let evicted = self.parked.park(key_profile, new_sid.clone(), proc, self.max_parked);
                                for mut e in evicted { e.shutdown().await; }
                            } else {
                                proc.shutdown().await;
                            }
                            return Ok(turn);
                        }
                        Err(_) => {
                            proc.shutdown().await;
                            // fall through to cold path
                        }
                    }
                }
                // proc not alive: drop it (kill_on_drop handles cleanup), fall through
            }
        }

        // --- Cold path: spawn via pool ---
        let key = IsolationKey {
            profile_id: key_profile.clone(),
            resume: req.resume.clone(),
        };
        let mut lease = self
            .pool
            .acquire(&key)
            .await
            .map_err(|e| ProxyError::Upstream(format!("spawn failed: {e}")))?
            .ok_or_else(|| ProxyError::Upstream("pool at capacity".into()))?;

        let result = run_one_turn(lease.proc(), req.system, req.prompt, &self.rate_limit).await;
        match &result {
            Ok(turn) if turn.session_id.is_some() => {
                // Park the live process under the result session_id
                let sid = turn.session_id.clone().unwrap();
                if let Some(proc) = lease.take_proc() {
                    let evicted = self.parked.park(key_profile, sid, proc, self.max_parked);
                    for mut e in evicted { e.shutdown().await; }
                }
                // take_proc already freed the cap slot; lease Drop is a no-op (proc == None).
            }
            _ => {
                lease.proc().shutdown().await;
                lease.discard();
            }
        }
        result
    }
}

impl PooledRunner {
    async fn run_passthrough(&self, req: TurnRequest) -> Result<TurnResult, ProxyError> {
        use std::collections::HashMap;
        let pid = profile_id(&req);
        let config_dir = self.config_root.join(meridian_transport::factory::safe_profile_segment(&pid));
        let _ = std::fs::create_dir_all(&config_dir);
        let cfg = meridian_transport::spawn::SpawnConfig {
            config_dir,
            model: None,
            mcp_config: Some(serde_json::json!({"mcpServers":{"oc":{"type":"sdk","name":"oc"}}})),
            include_partial_messages: false,
            resume: req.resume.clone(),
            max_turns: Some(3),
            // Overlay may itself override CLAUDE_CONFIG_DIR (oauth-token profiles).
            env_overlay: { use meridian_transport::factory::EnvResolver; self.profiles.overlay(&pid) },
        };
        let tools = Arc::new(meridian_transport::passthrough::new(req.tools.clone()));
        let base: HashMap<String, String> = std::env::vars().collect();
        let mut proc = meridian_transport::process::spawn(&self.exe, &cfg, &base, tools.clone())
            .await
            .map_err(|e| ProxyError::Upstream(format!("spawn failed: {e}")))?;
        let result = run_one_turn(&mut proc, req.system, req.prompt, &self.rate_limit).await;
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
    #[allow(clippy::too_many_arguments)]
    fn run_stream(&self, _model: String, system: Option<String>, prompt: String, profile: Option<String>, resume: Option<String>, messages: Vec<Value>, sessions: Arc<crate::session::SessionStore>) -> EventStream {
        let (tx, rx) = mpsc::channel::<Value>(64);
        let pool = self.pool.clone();
        let rate_limit = self.rate_limit.clone();
        let parked = self.parked.clone();
        let max_parked = self.max_parked;
        tokio::spawn(async move {
            let pid = profile.unwrap_or_else(|| "default".into());
            let full = match system {
                Some(s) if !s.is_empty() => format!("{s}\n\n{prompt}"),
                _ => prompt,
            };

            // --- Warm path: try to reuse a parked process for this (profile, session) ---
            if let Some(ref sid) = resume {
                if let Some(mut proc) = parked.take(&pid, sid) {
                    if proc.is_alive() {
                        if let Err(e) = proc.send_user_turn(&full).await {
                            let _ = tx.send(error_event(&format!("write failed: {e}"))).await;
                            proc.shutdown().await;
                            // fall through to cold path below
                        } else {
                            let mut session_id: Option<String> = None;
                            let mut reply_text = String::new();
                            let mut pump_error = false;
                            let pump = async {
                                while let Some(ev) = proc.next_event().await {
                                    match ev {
                                        CliMessage::Init { session_id: new_sid, .. } => {
                                            session_id = Some(new_sid);
                                        }
                                        CliMessage::StreamEvent { event, .. } => {
                                            if event.get("type").and_then(Value::as_str) == Some("content_block_delta") {
                                                if let Some(text) = event
                                                    .get("delta")
                                                    .and_then(|d| d.get("text"))
                                                    .and_then(Value::as_str)
                                                {
                                                    reply_text.push_str(text);
                                                }
                                            }
                                            if tx.send(event).await.is_err() { break; }
                                        }
                                        CliMessage::Result { raw, result, .. } => {
                                            if let Some(msg) = upstream_error_from_result(&raw, result) {
                                                let _ = tx.send(error_event(&msg)).await;
                                                pump_error = true;
                                            }
                                            break;
                                        }
                                        CliMessage::RateLimitEvent { info, .. } => rate_limit.record(&info),
                                        _ => {}
                                    }
                                }
                            };
                            let timed_out = tokio::time::timeout(std::time::Duration::from_secs(300), pump).await.is_err();
                            if timed_out {
                                let _ = tx.send(error_event("upstream timeout")).await;
                            }
                            // Store session before disposition.
                            if let Some(new_sid) = session_id {
                                if !messages.is_empty() {
                                    let mut convo = messages;
                                    convo.push(json!({"role":"assistant","content":reply_text}));
                                    sessions.insert(crate::session::fingerprint(&convo), new_sid.clone());
                                }
                                if !pump_error && !timed_out && proc.is_alive() {
                                    let evicted = parked.park(pid, new_sid, proc, max_parked);
                                    for mut e in evicted { e.shutdown().await; }
                                } else {
                                    proc.shutdown().await;
                                }
                            } else {
                                proc.shutdown().await;
                            }
                            return;
                        }
                    }
                    // proc not alive: drop it, fall through to cold path
                }
            }

            // --- Cold path: acquire from pool + spawn ---
            let key = IsolationKey { profile_id: pid.clone(), resume: resume.clone() };
            let mut lease = match pool.acquire(&key).await {
                Ok(Some(l)) => l,
                Ok(None) => { let _ = tx.send(error_event("pool at capacity")).await; return; }
                Err(e) => { let _ = tx.send(error_event(&format!("spawn failed: {e}"))).await; return; }
            };
            if let Err(e) = lease.proc().send_user_turn(&full).await {
                let _ = tx.send(error_event(&format!("write failed: {e}"))).await;
                lease.proc().shutdown().await;
                lease.discard();
                return;
            }
            let mut session_id: Option<String> = None;
            let mut reply_text = String::new();
            let mut pump_error = false;
            let pump = async {
                while let Some(ev) = lease.proc().next_event().await {
                    match ev {
                        CliMessage::Init { session_id: sid, .. } => {
                            session_id = Some(sid);
                        }
                        CliMessage::StreamEvent { event, .. } => {
                            // Accumulate assistant text for session store.
                            if event.get("type").and_then(Value::as_str) == Some("content_block_delta") {
                                if let Some(text) = event
                                    .get("delta")
                                    .and_then(|d| d.get("text"))
                                    .and_then(Value::as_str)
                                {
                                    reply_text.push_str(text);
                                }
                            }
                            let disconnected = tx.send(event).await.is_err();
                            if disconnected { break; }
                        }
                        CliMessage::Result { raw, result, .. } => {
                            if let Some(msg) = upstream_error_from_result(&raw, result) {
                                let _ = tx.send(error_event(&msg)).await;
                                pump_error = true;
                            }
                            break;
                        }
                        CliMessage::RateLimitEvent { info, .. } => rate_limit.record(&info),
                        _ => {}
                    }
                }
            };
            let timed_out = tokio::time::timeout(std::time::Duration::from_secs(300), pump).await.is_err();
            if timed_out {
                let _ = tx.send(error_event("upstream timeout")).await;
            }
            // Store session BEFORE shutdown so the next turn can resume.
            if let Some(sid) = session_id {
                if !messages.is_empty() {
                    let mut convo = messages;
                    convo.push(json!({"role":"assistant","content":reply_text}));
                    sessions.insert(crate::session::fingerprint(&convo), sid.clone());
                }
                // Park if the stream completed cleanly and the proc is alive.
                if !pump_error && !timed_out && lease.proc().is_alive() {
                    if let Some(proc) = lease.take_proc() {
                        let evicted = parked.park(pid, sid, proc, max_parked);
                        for mut e in evicted { e.shutdown().await; }
                    }
                } else {
                    lease.proc().shutdown().await;
                    lease.discard();
                }
            } else {
                lease.proc().shutdown().await;
                lease.discard();
            }
        });
        ReceiverStream::new(rx)
    }
}

fn error_event(message: &str) -> Value {
    json!({"type":"error","error":{"type":"api_error","message":message}})
}

/// Map a CLI `result` event to an upstream error, if it signals one. The CLI
/// reports auth / network / rate-limit failures with `is_error: true` and the
/// human-readable text in `result`; without this the error text would leak
/// through as an ordinary 200 turn. Returns `None` for a healthy result.
fn upstream_error_from_result(raw: &Value, result: Option<String>) -> Option<String> {
    if raw.get("is_error").and_then(Value::as_bool) == Some(true) {
        Some(result.unwrap_or_else(|| "upstream error".into()))
    } else {
        None
    }
}

/// Drives one prompt to completion on an already-acquired process. The caller
/// is responsible for shutting the process down and discarding the lease.
async fn run_one_turn(
    proc: &mut meridian_transport::process::CliProcess,
    system: Option<String>,
    prompt: String,
    rate_limit: &RateLimitStore,
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
        let mut upstream_error: Option<String> = None;
        while let Some(ev) = proc.next_event().await {
            match ev {
                CliMessage::Init { session_id: sid, .. } => session_id = Some(sid),
                CliMessage::Assistant { message, .. } => last_message = Some(message),
                CliMessage::Result { raw, result, .. } => {
                    stop_reason = raw.get("stop_reason").and_then(Value::as_str).map(str::to_string);
                    upstream_error = upstream_error_from_result(&raw, result);
                    break;
                }
                CliMessage::RateLimitEvent { info, .. } => rate_limit.record(&info),
                _ => {}
            }
        }
        (last_message, stop_reason, session_id, upstream_error)
    };

    let (last_message, stop_reason, session_id, upstream_error) =
        tokio::time::timeout(std::time::Duration::from_secs(300), collect)
            .await
            .map_err(|_| ProxyError::Upstream("upstream timeout".into()))?;

    if let Some(e) = upstream_error {
        return Err(ProxyError::Upstream(e));
    }

    let mut msg = last_message
        .ok_or_else(|| ProxyError::Upstream("no assistant message produced".into()))?;
    if let (Some(obj), Some(sr)) = (msg.as_object_mut(), stop_reason) {
        obj.insert("stop_reason".into(), Value::String(sr));
    }
    Ok(TurnResult { message: msg, session_id, captured_tools: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::upstream_error_from_result;
    use serde_json::json;

    #[test]
    fn is_error_result_becomes_upstream_error() {
        let raw = json!({"type":"result","subtype":"success","is_error":true,
                         "result":"API Error: Unable to connect"});
        assert_eq!(
            upstream_error_from_result(&raw, Some("API Error: Unable to connect".into())),
            Some("API Error: Unable to connect".into())
        );
    }

    #[test]
    fn is_error_without_text_falls_back_to_generic() {
        let raw = json!({"is_error":true});
        assert_eq!(upstream_error_from_result(&raw, None), Some("upstream error".into()));
    }

    #[test]
    fn healthy_result_is_not_an_error() {
        let raw = json!({"type":"result","subtype":"success","is_error":false});
        assert_eq!(upstream_error_from_result(&raw, Some("ignored".into())), None);
        // missing is_error also means success
        assert_eq!(upstream_error_from_result(&json!({}), None), None);
    }
}
