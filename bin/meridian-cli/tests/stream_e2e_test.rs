use std::sync::Arc;
use meridian::pooled_runner::pooled_runner;
use meridian::server::StreamRunner;
use tokio_stream::StreamExt;

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn stream_yields_deltas_and_stop() {
    let root = std::env::temp_dir().join(format!("meridian-stream-{}", std::process::id()));
    let runner = pooled_runner("claude".into(), root, 2, std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir())), std::sync::Arc::new(meridian::rate_limit::RateLimitStore::new()));
    let mut stream = runner.run_stream("sonnet".into(), None, "Reply with exactly: OK".into(), None);

    // With the keychain-realignment fix (CLAUDE_SECURESTORAGE_CONFIG_DIR="" in
    // the spawn env), auth succeeds under an isolated config dir, so the CLI
    // emits real --include-partial-messages partials. Assert we receive at
    // least one SSE event AND the stream terminates cleanly.
    let mut count = 0usize;
    let drained = tokio::time::timeout(std::time::Duration::from_secs(120), async {
        while let Some(_ev) = stream.next().await {
            count += 1;
            if count > 1000 { break; }
        }
    })
    .await;
    assert!(drained.is_ok(), "stream did not terminate within 120s (it hung)");
    assert!(count > 0, "expected at least one SSE event (partials should flow under the auth fix)");
}

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn http_stream_true_streams_sse() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use meridian::server::router;
    use meridian::session::SessionStore;
    let root = std::env::temp_dir().join(format!("meridian-httpstream-{}", std::process::id()));
    let profiles = std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir()));
    let rate_limit = Arc::new(meridian::rate_limit::RateLimitStore::new());
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), rate_limit.clone())), Arc::new(SessionStore::new()), profiles, rate_limit);
    let body = serde_json::json!({"model":"sonnet","stream":true,"messages":[{"role":"user","content":"Reply with exactly: OK"}]});
    let resp = app.oneshot(
        Request::post("/v1/messages").header("content-type","application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // With the keychain-realignment auth fix, partials flow under isolation,
    // so the SSE body carries real Anthropic stream events.
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("event: "), "no SSE event lines in body: {text}");
}
