use std::sync::Arc;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;

use crate::error::ProxyError;

/// Runs one prompt to completion and returns the Anthropic `message` object.
pub trait TurnRunner: Send + Sync {
    fn run_turn(
        &self,
        model: String,
        system: Option<String>,
        prompt: String,
    ) -> impl std::future::Future<Output = Result<Value, ProxyError>> + Send;
}

/// Runs one prompt and streams Anthropic SSE events as they arrive.
pub trait StreamRunner: Send + Sync {
    fn run_stream(
        &self,
        model: String,
        system: Option<String>,
        prompt: String,
    ) -> crate::sse::SseStream;
}

pub fn router<R: TurnRunner + 'static>(runner: Arc<R>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/messages", post(messages::<R>))
        .with_state(runner)
}

async fn messages<R: TurnRunner + 'static>(
    State(runner): State<Arc<R>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, ProxyError> {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| ProxyError::BadRequest("messages must be a non-empty array".into()))?;

    let model = body.get("model").and_then(Value::as_str).unwrap_or("sonnet").to_string();
    let system = extract_system(&body);
    let prompt = extract_last_user_text(messages)
        .ok_or_else(|| ProxyError::BadRequest("no user message text found".into()))?;

    let msg = runner.run_turn(model, system, prompt).await?;
    Ok(Json(msg))
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
