use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
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

/// Runs one prompt and streams raw Anthropic stream-event Values as they arrive.
pub trait StreamRunner: Send + Sync {
    fn run_stream(
        &self,
        model: String,
        system: Option<String>,
        prompt: String,
    ) -> crate::sse::EventStream;
}

pub fn router<R: TurnRunner + StreamRunner + 'static>(runner: Arc<R>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/v1/messages", post(messages::<R>))
        .route("/v1/chat/completions", post(chat_completions::<R>))
        .route("/v1/models", get(models))
        .with_state(runner)
}

async fn messages<R: TurnRunner + StreamRunner + 'static>(
    State(runner): State<Arc<R>>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let messages = match body.get("messages").and_then(Value::as_array).filter(|a| !a.is_empty()) {
        Some(m) => m,
        None => return ProxyError::BadRequest("messages must be a non-empty array".into()).into_response(),
    };
    let model = body.get("model").and_then(Value::as_str).unwrap_or("sonnet").to_string();
    let system = extract_system(&body);
    let prompt = match extract_last_user_text(messages) {
        Some(p) => p,
        None => return ProxyError::BadRequest("no user message text found".into()).into_response(),
    };

    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        use tokio_stream::StreamExt;
        let events = runner.run_stream(model, system, prompt);
        let sse = events.map(|v| Ok::<_, std::convert::Infallible>(crate::sse::sse_event(&v)));
        return axum::response::sse::Sse::new(sse).into_response();
    }
    match runner.run_turn(model, system, prompt).await {
        Ok(msg) => Json(msg).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn chat_completions<R: TurnRunner + StreamRunner + 'static>(
    State(runner): State<Arc<R>>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let (model, system, prompt) = match crate::openai::openai_to_canonical(&body) {
        Ok(t) => t,
        Err(e) => return ProxyError::BadRequest(e).into_response(),
    };

    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        use tokio_stream::StreamExt;
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<axum::response::sse::Event, std::convert::Infallible>>(64);
        let mut events = runner.run_stream(model.clone(), system, prompt);
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

    match runner.run_turn(model.clone(), system, prompt).await {
        Ok(msg) => Json(crate::openai::anthropic_to_openai(&msg, &model)).into_response(),
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
