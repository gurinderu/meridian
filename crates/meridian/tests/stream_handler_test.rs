use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use axum::response::sse::Event;
use meridian::error::ProxyError;
use meridian::server::{router, StreamRunner, TurnRunner};
use meridian::sse::{sse_event, SseStream};

struct FakeRunner;
impl TurnRunner for FakeRunner {
    async fn run_turn(&self, _m: String, _s: Option<String>, _p: String) -> Result<Value, ProxyError> {
        Ok(json!({"role":"assistant","content":[]}))
    }
}
impl StreamRunner for FakeRunner {
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String) -> SseStream {
        let (tx, rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(8);
        tokio::spawn(async move {
            let _ = tx.send(Ok(sse_event(&json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}})))).await;
            let _ = tx.send(Ok(sse_event(&json!({"type":"message_stop"})))).await;
        });
        ReceiverStream::new(rx)
    }
}

#[tokio::test]
async fn stream_true_returns_sse_with_events() {
    let app = router(Arc::new(FakeRunner));
    let body = json!({"model":"sonnet","stream":true,"messages":[{"role":"user","content":"hi"}]});
    let resp = app.oneshot(
        Request::post("/v1/messages").header("content-type","application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap().to_string();
    assert!(ct.starts_with("text/event-stream"), "expected SSE content-type, got {ct}");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("event: content_block_delta"), "body: {text}");
    assert!(text.contains("event: message_stop"));
    assert!(text.contains("text_delta"));
}

#[tokio::test]
async fn stream_false_still_returns_json() {
    let app = router(Arc::new(FakeRunner));
    let body = json!({"model":"sonnet","messages":[{"role":"user","content":"hi"}]});
    let resp = app.oneshot(
        Request::post("/v1/messages").header("content-type","application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp.headers().get("content-type").unwrap().to_str().unwrap().to_string();
    assert!(ct.starts_with("application/json"), "expected JSON, got {ct}");
}
