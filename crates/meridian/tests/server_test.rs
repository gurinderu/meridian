use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt; // oneshot
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use meridian::error::ProxyError;
use meridian::server::{router, StreamRunner, TurnRunner};
use meridian::sse::SseStream;

struct FakeRunner;
impl TurnRunner for FakeRunner {
    async fn run_turn(&self, model: String, system: Option<String>, prompt: String) -> Result<Value, ProxyError> {
        Ok(json!({
            "id":"msg_test","type":"message","role":"assistant","model":model,
            "content":[{"type":"text","text":format!("sys={};p={}", system.unwrap_or_default(), prompt)}],
            "stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}
        }))
    }
}
impl StreamRunner for FakeRunner {
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String) -> SseStream {
        let (_tx, rx) = mpsc::channel(1);
        ReceiverStream::new(rx)
    }
}

#[tokio::test]
async fn messages_endpoint_returns_assistant_message() {
    let app = router(Arc::new(FakeRunner));
    let body = json!({"model":"opus","system":"be brief","messages":[{"role":"user","content":"hi"}]});
    let resp = app.oneshot(
        Request::post("/v1/messages")
            .header("content-type","application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["role"], "assistant");
    assert_eq!(v["model"], "opus");
    assert_eq!(v["content"][0]["text"], "sys=be brief;p=hi");
}

#[tokio::test]
async fn empty_messages_is_400() {
    let app = router(Arc::new(FakeRunner));
    let body = json!({"messages":[]});
    let resp = app.oneshot(
        Request::post("/v1/messages")
            .header("content-type","application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
