use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;

use crate::error::ProxyError;
use crate::rate_limit::RateLimitStore;

pub struct TurnRequest {
    pub model: String,
    pub system: Option<String>,
    pub prompt: String,
    pub resume: Option<String>,
    pub tools: Vec<serde_json::Value>,
    /// Resolved profile id selecting the Claude account for this turn.
    pub profile: Option<String>,
}

pub struct TurnResult {
    pub message: serde_json::Value,
    pub session_id: Option<String>,
    pub captured_tools: Vec<serde_json::Value>,
}

/// Runs one prompt to completion and returns the Anthropic `message` object.
pub trait TurnRunner: Send + Sync {
    fn run_turn(
        &self,
        req: TurnRequest,
    ) -> impl std::future::Future<Output = Result<TurnResult, ProxyError>> + Send;
}

/// Runs one prompt and streams raw Anthropic stream-event Values as they arrive.
pub trait StreamRunner: Send + Sync {
    fn run_stream(
        &self,
        model: String,
        system: Option<String>,
        prompt: String,
        profile: Option<String>,
    ) -> crate::sse::EventStream;
}

pub struct AppState<R> {
    pub runner: Arc<R>,
    pub sessions: Arc<crate::session::SessionStore>,
    pub profiles: Arc<crate::profiles::ProfileStore>,
    pub rate_limit: Arc<RateLimitStore>,
}

impl<R> Clone for AppState<R> {
    fn clone(&self) -> Self {
        AppState {
            runner: self.runner.clone(),
            sessions: self.sessions.clone(),
            profiles: self.profiles.clone(),
            rate_limit: self.rate_limit.clone(),
        }
    }
}

pub fn router<R: TurnRunner + StreamRunner + 'static>(
    runner: Arc<R>,
    sessions: Arc<crate::session::SessionStore>,
    profiles: Arc<crate::profiles::ProfileStore>,
    rate_limit: Arc<RateLimitStore>,
) -> Router {
    router_with_auth(runner, sessions, profiles, rate_limit, crate::auth::configured_key())
}

/// Like `router`, but with an explicit API key instead of reading
/// `MERIDIAN_API_KEY` from the environment. `None` disables auth. Lets tests
/// exercise the auth middleware deterministically without touching env vars.
pub fn router_with_auth<R: TurnRunner + StreamRunner + 'static>(
    runner: Arc<R>,
    sessions: Arc<crate::session::SessionStore>,
    profiles: Arc<crate::profiles::ProfileStore>,
    rate_limit: Arc<RateLimitStore>,
    api_key: Option<String>,
) -> Router {
    // /health is always open; everything else sits behind the (optional) key.
    let protected = Router::new()
        .route("/v1/messages", post(messages::<R>))
        .route("/v1/chat/completions", post(chat_completions::<R>))
        .route("/v1/models", get(models))
        .route("/v1/usage/quota", get(usage_quota::<R>))
        .route("/profiles/list", get(profiles_list::<R>))
        .route("/profiles/active", post(profiles_active::<R>))
        .route("/auth/refresh", post(auth_refresh::<R>))
        .route_layer(axum::middleware::from_fn_with_state(api_key, crate::auth::require_auth))
        .with_state(AppState { runner, sessions, profiles, rate_limit });
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .merge(protected)
}

async fn messages<R: TurnRunner + StreamRunner + 'static>(
    State(state): State<AppState<R>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let messages = match body.get("messages").and_then(Value::as_array).filter(|a| !a.is_empty()) {
        Some(m) => m,
        None => return ProxyError::BadRequest("messages must be a non-empty array".into()).into_response(),
    };
    let model = body.get("model").and_then(Value::as_str).unwrap_or("sonnet").to_string();
    let system = extract_system(&body);
    let requested = headers.get("x-meridian-profile").and_then(|v| v.to_str().ok());
    let profile = Some(state.profiles.resolve_id(requested));

    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        // Streaming has no resume/session continuity yet, so it must carry the
        // FULL conversation (flatten) — sending only the last user message
        // dropped all prior context on multi-turn streaming chats.
        if !messages.iter().any(|m| m.get("role").and_then(Value::as_str) == Some("user")) {
            return ProxyError::BadRequest("no user message found".into()).into_response();
        }
        let prompt = flatten_conversation(messages);
        use tokio_stream::StreamExt;
        let events = state.runner.run_stream(model, system, prompt, profile);
        let sse = events.map(|v| Ok::<_, std::convert::Infallible>(crate::sse::sse_event(&v)));
        return axum::response::sse::Sse::new(sse).into_response();
    }

    // Non-streaming: resume-aware path
    let last_user_idx = messages.iter().rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"));
    let last_user_idx = match last_user_idx {
        Some(i) => i,
        None => return ProxyError::BadRequest("no user message found".into()).into_response(),
    };
    let prefix = &messages[..last_user_idx];
    let resume = state.sessions.get(&crate::session::fingerprint(prefix));

    // On resume, send only the last user message; otherwise flatten the whole conversation.
    // If the last user message carries tool_result blocks, unwrap them as the prompt first.
    let last_user = &messages[last_user_idx];
    let prompt = if let Some(unwrapped) = crate::tools::unwrap_tool_results(last_user) {
        unwrapped
    } else if resume.is_some() {
        crate::session::message_text_pub(last_user)
    } else {
        flatten_conversation(messages)
    };

    let mcp_tools = body
        .get("tools")
        .and_then(Value::as_array)
        .map(|ts| crate::tools::anthropic_tools_to_mcp_defs(ts))
        .unwrap_or_default();

    match state.runner.run_turn(TurnRequest { model: model.clone(), system, prompt, resume, tools: mcp_tools, profile }).await {
        Ok(r) => {
            let response = if r.captured_tools.is_empty() {
                r.message.clone()
            } else {
                let blocks: Vec<Value> = r.captured_tools.iter().map(|c| serde_json::json!({
                    "type": "tool_use",
                    "id": c.get("id").cloned().unwrap_or(Value::Null),
                    "name": crate::tools::strip_oc_prefix(c.get("name").and_then(Value::as_str).unwrap_or("")),
                    "input": c.get("input").cloned().unwrap_or_else(|| serde_json::json!({})),
                })).collect();
                serde_json::json!({
                    "id": "msg_meridian", "type": "message", "role": "assistant", "model": model,
                    "content": blocks, "stop_reason": "tool_use", "stop_sequence": Value::Null,
                    "usage": r.message.get("usage").cloned().unwrap_or_else(|| serde_json::json!({}))
                })
            };
            if let Some(sid) = r.session_id {
                // Store under the fingerprint of the conversation INCLUDING our reply,
                // so the client's next turn (which echoes our reply) hits this session.
                let reply_text = r.message.get("content").and_then(Value::as_array)
                    .map(|b| b.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect::<Vec<_>>().concat())
                    .unwrap_or_default();
                let mut convo: Vec<Value> = messages.to_vec();
                convo.push(serde_json::json!({"role":"assistant","content":reply_text}));
                state.sessions.insert(crate::session::fingerprint(&convo), sid);
            }
            Json(response).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn chat_completions<R: TurnRunner + StreamRunner + 'static>(
    State(state): State<AppState<R>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let (model, system, prompt) = match crate::openai::openai_to_canonical(&body) {
        Ok(t) => t,
        Err(e) => return ProxyError::BadRequest(e).into_response(),
    };
    let requested = headers.get("x-meridian-profile").and_then(|v| v.to_str().ok());
    let profile = Some(state.profiles.resolve_id(requested));

    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        use tokio_stream::StreamExt;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::response::sse::Event, std::convert::Infallible>>(64);
        let mut events = state.runner.run_stream(model.clone(), system, prompt, profile);
        let model2 = model.clone();
        tokio::spawn(async move {
            let mut chunker = crate::openai::new_chunker(&model2);
            while let Some(ev) = events.next().await {
                for c in chunker.push(&ev) {
                    if tx.send(Ok(axum::response::sse::Event::default().data(c.to_string()))).await.is_err() {
                        return;
                    }
                }
            }
            let _ = tx.send(Ok(axum::response::sse::Event::default().data("[DONE]"))).await;
        });
        return axum::response::sse::Sse::new(tokio_stream::wrappers::ReceiverStream::new(rx)).into_response();
    }

    let mcp_tools = body
        .get("tools")
        .and_then(Value::as_array)
        .map(|ts| crate::openai::openai_tools_to_mcp_defs(ts))
        .unwrap_or_default();

    match state.runner.run_turn(TurnRequest { model: model.clone(), system, prompt, resume: None, tools: mcp_tools, profile }).await {
        Ok(r) => {
            let resp = if r.captured_tools.is_empty() {
                crate::openai::anthropic_to_openai(&r.message, &model)
            } else {
                crate::openai::tool_calls_completion(&r.captured_tools, &model)
            };
            Json(resp).into_response()
        }
        Err(e) => e.into_response(),
    }
}

async fn models() -> axum::response::Response {
    Json(crate::openai::model_list()).into_response()
}

fn extract_system(body: &Value) -> Option<String> {
    match body.get("system") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(blocks)) => {
            let joined = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            (!joined.is_empty()).then_some(joined)
        }
        _ => None,
    }
}

async fn profiles_list<R: TurnRunner + StreamRunner + 'static>(
    State(state): State<AppState<R>>,
) -> axum::response::Response {
    let list = state.profiles.list();
    let active = state.profiles.resolve_id(None);
    let profiles: Vec<Value> = list.into_iter().map(|p| serde_json::json!({
        "id": p.id,
        "type": p.kind,
        "isActive": p.is_active,
    })).collect();
    Json(serde_json::json!({ "profiles": profiles, "activeProfile": active })).into_response()
}

async fn profiles_active<R: TurnRunner + StreamRunner + 'static>(
    State(state): State<AppState<R>>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let parsed: Result<Value, _> = serde_json::from_slice(&body);
    let profile = match parsed.ok().as_ref().and_then(|v| v.get("profile")).and_then(Value::as_str) {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => return ProxyError::BadRequest("Missing 'profile' in request body".into()).into_response(),
    };
    let eff = state.profiles.effective();
    if eff.is_empty() {
        return ProxyError::BadRequest("No profiles configured".into()).into_response();
    }
    if !eff.iter().any(|p| p.id == profile) {
        let avail = eff.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", ");
        return ProxyError::BadRequest(format!("Unknown profile: {profile}. Available: {avail}")).into_response();
    }
    state.profiles.set_active(profile.clone());
    state.sessions.clear();
    state.rate_limit.clear();
    Json(serde_json::json!({ "success": true, "activeProfile": profile })).into_response()
}

async fn usage_quota<R: TurnRunner + StreamRunner + 'static>(
    State(state): State<AppState<R>>,
    axum::extract::RawQuery(q): axum::extract::RawQuery,
) -> axum::response::Response {
    // No percent-decoding: profile ids are restricted to [A-Za-z0-9_-] slugs
    // (is_valid_profile_id), so a raw match is exact for any real id.
    let requested = q.as_deref().and_then(|qs| qs.split('&')
        .find_map(|kv| kv.strip_prefix("profile=")))
        .map(|s| s.to_string());
    let profile = state.profiles.resolve_id(requested.as_deref());
    // get_all() already excludes the internal "default" bucket, so the count is
    // exactly buckets.len() — derive it from the same snapshot rather than a
    // second lock (which could disagree by one under a concurrent record()).
    let buckets = state.rate_limit.get_all();
    let count = buckets.len();
    axum::Json(serde_json::json!({
        "profile": profile,
        "buckets": buckets,
        "extraUsage": serde_json::Value::Null,
        "sources": { "oauth": serde_json::Value::Null, "sdk": { "entryCount": count } },
    })).into_response()
}

async fn auth_refresh<R: TurnRunner + StreamRunner + 'static>(
    State(state): State<AppState<R>>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let requested = headers.get("x-meridian-profile").and_then(|v| v.to_str().ok());
    let id = state.profiles.resolve_id(requested);
    // Only claude-max / default profiles have an on-disk credential store to
    // refresh; api + oauth-token profiles carry their auth via env.
    let kind = state.profiles.resolved_type(&id);
    let dir = state.profiles.config_dir_for(&id);
    let refreshable = matches!(kind, crate::profiles::ProfileType::ClaudeMax);
    let ok = if refreshable {
        tokio::task::spawn_blocking(move || {
            let store = crate::token_refresh::create_platform_credential_store(dir.as_deref());
            crate::token_refresh::refresh_oauth_token(store.as_ref())
        }).await.unwrap_or_else(|e| { tracing::error!("token refresh task failed: {e}"); false })
    } else { false };
    if ok {
        state.rate_limit.clear();
        axum::Json(serde_json::json!({"success":true,"message":"OAuth token refreshed successfully","profile":id})).into_response()
    } else {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR,
         axum::Json(serde_json::json!({"success":false,"message":"Token refresh failed. If the problem persists, run 'claude login'."}))).into_response()
    }
}

/// Flatten a whole conversation into one `role: text\n…` prompt — the
/// self-contained form sent to a fresh (non-resumed) `claude` so it has the full
/// context. Used by the non-stream no-resume path AND the streaming path (which
/// has no resume/session continuity yet, so it must carry the full history).
fn flatten_conversation(messages: &[Value]) -> String {
    messages
        .iter()
        .map(|m| format!("{}: {}", m.get("role").and_then(Value::as_str).unwrap_or(""), crate::session::message_text_pub(m)))
        .collect::<Vec<_>>()
        .join("\n")
}
