use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use meridian::error::ProxyError;
use meridian::server::{router, StreamRunner, TurnRunner};
use meridian::session::SessionStore;
use meridian::sse::EventStream;

struct FakeRunner;
impl TurnRunner for FakeRunner {
    async fn run_turn(&self, _req: meridian::server::TurnRequest) -> Result<meridian::server::TurnResult, ProxyError> {
        let message = json!({"role":"assistant","content":[]});
        Ok(meridian::server::TurnResult { message, session_id: None, captured_tools: Vec::new() })
    }
}
impl StreamRunner for FakeRunner {
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String, _profile: Option<String>) -> EventStream {
        let (tx, rx) = mpsc::channel::<Value>(8);
        tokio::spawn(async move {
            let _ = tx.send(json!({"type":"message_start","message":{"id":"msg_1"}})).await;
            let _ = tx.send(json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}})).await;
            let _ = tx.send(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})).await;
            let _ = tx.send(json!({"type":"message_stop"})).await;
        });
        ReceiverStream::new(rx)
    }
}

#[tokio::test]
async fn openai_stream_emits_chunks_and_done() {
    let app = router(Arc::new(FakeRunner), Arc::new(SessionStore::new()), Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), "/cfg".into())), Arc::new(meridian::rate_limit::RateLimitStore::new()));
    let body = json!({"model":"opus","stream":true,"messages":[{"role":"user","content":"hi"}]});
    let resp = app.oneshot(
        Request::post("/v1/chat/completions").header("content-type","application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap().to_string();
    assert!(ct.starts_with("text/event-stream"), "got {ct}");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("chat.completion.chunk"), "body: {text}");
    assert!(text.contains("\"content\":\"hi\""));
    assert!(text.contains("\"finish_reason\":\"stop\""));
    assert!(text.contains("data: [DONE]"));
}

#[tokio::test]
async fn openai_stream_false_still_json() {
    let app = router(Arc::new(FakeRunner), Arc::new(SessionStore::new()), Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), "/cfg".into())), Arc::new(meridian::rate_limit::RateLimitStore::new()));
    let body = json!({"model":"opus","messages":[{"role":"user","content":"hi"}]});
    let resp = app.oneshot(
        Request::post("/v1/chat/completions").header("content-type","application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap().to_string();
    assert!(ct.starts_with("application/json"), "got {ct}");
}
