use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;

use crate::error::ProxyError;

pub struct TurnRequest {
    pub model: String,
    pub system: Option<String>,
    pub prompt: String,
    pub resume: Option<String>,
    pub tools: Vec<serde_json::Value>,
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
    ) -> crate::sse::EventStream;
}

pub struct AppState<R> {
    pub runner: Arc<R>,
    pub sessions: Arc<crate::session::SessionStore>,
}

impl<R> Clone for AppState<R> {
    fn clone(&self) -> Self {
        AppState { runner: self.runner.clone(), sessions: self.sessions.clone() }
    }
}

pub fn router<R: TurnRunner + StreamRunner + 'static>(
    runner: Arc<R>,
    sessions: Arc<crate::session::SessionStore>,
) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/messages", post(messages::<R>))
        .route("/v1/chat/completions", post(chat_completions::<R>))
        .route("/v1/models", get(models))
        .with_state(AppState { runner, sessions })
}

async fn messages<R: TurnRunner + StreamRunner + 'static>(
    State(state): State<AppState<R>>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let messages = match body.get("messages").and_then(Value::as_array).filter(|a| !a.is_empty()) {
        Some(m) => m,
        None => return ProxyError::BadRequest("messages must be a non-empty array".into()).into_response(),
    };
    let model = body.get("model").and_then(Value::as_str).unwrap_or("sonnet").to_string();
    let system = extract_system(&body);

    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        let prompt = match extract_last_user_text(messages) {
            Some(p) => p,
            None => return ProxyError::BadRequest("no user message text found".into()).into_response(),
        };
        use tokio_stream::StreamExt;
        let events = state.runner.run_stream(model, system, prompt);
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
    let prompt = if resume.is_some() {
        crate::session::message_text_pub(&messages[last_user_idx])
    } else {
        messages.iter()
            .map(|m| format!("{}: {}", m.get("role").and_then(Value::as_str).unwrap_or(""), crate::session::message_text_pub(m)))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let mcp_tools = body
        .get("tools")
        .and_then(Value::as_array)
        .map(|ts| crate::tools::anthropic_tools_to_mcp_defs(ts))
        .unwrap_or_default();

    match state.runner.run_turn(TurnRequest { model: model.clone(), system, prompt, resume, tools: mcp_tools }).await {
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
    Json(body): Json<Value>,
) -> axum::response::Response {
    let (model, system, prompt) = match crate::openai::openai_to_canonical(&body) {
        Ok(t) => t,
        Err(e) => return ProxyError::BadRequest(e).into_response(),
    };

    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        use tokio_stream::StreamExt;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::response::sse::Event, std::convert::Infallible>>(64);
        let mut events = state.runner.run_stream(model.clone(), system, prompt);
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

    match state.runner.run_turn(TurnRequest { model: model.clone(), system, prompt, resume: None, tools: Vec::new() }).await {
        Ok(r) => Json(crate::openai::anthropic_to_openai(&r.message, &model)).into_response(),
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

fn extract_last_user_text(messages: &[Value]) -> Option<String> {
    let last_user = messages
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))?;
    match last_user.get("content") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(parts)) => {
            let text = parts
                .iter()
                .filter_map(|p| p.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            Some(text)
        }
        _ => None,
    }
}
