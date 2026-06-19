use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use meridian::error::ProxyError;
use meridian::server::{router, StreamRunner, TurnRunner};
use meridian::sse::EventStream;

struct FakeRunner;
impl TurnRunner for FakeRunner {
    async fn run_turn(&self, req: meridian::server::TurnRequest) -> Result<meridian::server::TurnResult, ProxyError> {
        let message = json!({
            "id":"msg_x","role":"assistant","model":req.model,
            "content":[{"type":"text","text":format!("echo:{}", req.prompt)}],
            "stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}
        });
        Ok(meridian::server::TurnResult { message, session_id: None })
    }
}
impl StreamRunner for FakeRunner {
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String) -> EventStream {
        let (_tx, rx) = mpsc::channel::<serde_json::Value>(1);
        ReceiverStream::new(rx)
    }
}

#[tokio::test]
async fn chat_completions_returns_openai_shape() {
    let app = router(Arc::new(FakeRunner));
    let body = json!({"model":"opus","messages":[{"role":"user","content":"hi"}]});
    let resp = app.oneshot(
        Request::post("/v1/chat/completions").header("content-type","application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["object"], "chat.completion");
    assert_eq!(v["choices"][0]["message"]["content"], "echo:hi");
    assert_eq!(v["choices"][0]["finish_reason"], "stop");
}

#[tokio::test]
async fn chat_completions_empty_messages_is_400() {
    let app = router(Arc::new(FakeRunner));
    let resp = app.oneshot(
        Request::post("/v1/chat/completions").header("content-type","application/json")
            .body(Body::from(json!({"messages":[]}).to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn models_endpoint_lists_models() {
    let app = router(Arc::new(FakeRunner));
    let resp = app.oneshot(Request::get("/v1/models").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["object"], "list");
    assert!(v["data"].as_array().unwrap().iter().any(|m| m["id"] == "opus"));
}
