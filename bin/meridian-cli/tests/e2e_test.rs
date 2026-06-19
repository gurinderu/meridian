// Live end-to-end: starts the router with the real pooled runner, posts an
// Anthropic /v1/messages request, asserts a well-formed assistant message.
use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use meridian::pooled_runner::pooled_runner;
use meridian::server::router;
use meridian::session::SessionStore;

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn end_to_end_messages() {
    let root = std::env::temp_dir().join(format!("meridian-e2e-{}", std::process::id()));
    let profiles = std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir()));
    let runner = Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone()));
    let sessions = Arc::new(SessionStore::new());
    let app = router(runner, sessions, profiles);
    let body = json!({"model":"sonnet","messages":[{"role":"user","content":"Reply with exactly: OK"}]});
    let resp = app.oneshot(
        Request::post("/v1/messages")
            .header("content-type","application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["role"], "assistant");
    assert!(v["content"][0]["text"].as_str().is_some());
}
