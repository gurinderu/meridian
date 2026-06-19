use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use meridian::pooled_runner::pooled_runner;
use meridian::server::router;

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn openai_chat_completions_end_to_end() {
    let root = std::env::temp_dir().join(format!("meridian-oai-{}", std::process::id()));
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2)));
    let body = json!({"model":"sonnet","messages":[{"role":"user","content":"Reply with exactly: OK"}]});
    let resp = app.oneshot(
        Request::post("/v1/chat/completions").header("content-type","application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["object"], "chat.completion");
    assert!(v["choices"][0]["message"]["content"].as_str().is_some());
}
