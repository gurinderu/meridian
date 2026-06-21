use std::sync::Arc;
use meridian::pooled_runner::pooled_runner;
use meridian::server::StreamRunner;
use tokio_stream::StreamExt;

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn stream_yields_deltas_and_stop() {
    let root = std::env::temp_dir().join(format!("meridian-stream-{}", std::process::id()));
    let runner = pooled_runner("claude".into(), root, 2, std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir())), std::sync::Arc::new(meridian::rate_limit::RateLimitStore::new()), 8);
    let mut stream = runner.run_stream("sonnet".into(), None, "Reply with exactly: OK".into(), None, None, vec![], Arc::new(meridian::session::SessionStore::new()));

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
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), rate_limit.clone(), 8)), Arc::new(SessionStore::new()), profiles, rate_limit);
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

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn streaming_multi_turn_keeps_context() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use meridian::server::router;
    use meridian::session::SessionStore;
    let root = std::env::temp_dir().join(format!("meridian-streamctx-{}", std::process::id()));
    let profiles = std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir()));
    let rate_limit = Arc::new(meridian::rate_limit::RateLimitStore::new());
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), rate_limit.clone(), 8)), Arc::new(SessionStore::new()), profiles, rate_limit);
    // A multi-turn conversation: the codeword is only in turn 1. Pre-fix, the
    // streaming path sent only the last user message and would have NO context.
    let body = serde_json::json!({"model":"sonnet","stream":true,"messages":[
        {"role":"user","content":"Remember the codeword KUMQUAT83. Reply with just OK."},
        {"role":"assistant","content":"OK"},
        {"role":"user","content":"What was the exact codeword?"}]});
    let resp = app.oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(body.to_string())).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("KUMQUAT83"), "streaming reply must recall the codeword from flattened history; body: {text}");
}

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn streaming_resume_stores_and_continues() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use meridian::server::router;
    use meridian::session::SessionStore;
    let root = std::env::temp_dir().join(format!("meridian-streamresume-{}", std::process::id()));
    let profiles = std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir()));
    let rate_limit = Arc::new(meridian::rate_limit::RateLimitStore::new());
    let sessions = Arc::new(SessionStore::new());
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), rate_limit.clone(), 8)), sessions.clone(), profiles, rate_limit);

    // Turn 1 (streaming): set a codeword. After it completes, a session must be stored.
    let b1 = serde_json::json!({"model":"sonnet","stream":true,"messages":[
        {"role":"user","content":"Remember the codeword PERSIMMON5. Reply with just OK."}]});
    let r1 = app.clone().oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(b1.to_string())).unwrap()).await.unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(r1.into_body(), usize::MAX).await.unwrap(); // drain to completion
    // give the pump's post-stream store a beat (the SSE body is fully drained above,
    // but the store happens in the spawned task right before the channel closes).
    assert_eq!(sessions.len_for_test(), 1, "turn 1 must store a streaming session");

    // Turn 2 (streaming continuation): echoes turn 1 -> resolves resume -> delta send.
    let b2 = serde_json::json!({"model":"sonnet","stream":true,"messages":[
        {"role":"user","content":"Remember the codeword PERSIMMON5. Reply with just OK."},
        {"role":"assistant","content":"OK"},
        {"role":"user","content":"What was the exact codeword?"}]});
    let r2 = app.oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(b2.to_string())).unwrap()).await.unwrap();
    let text = String::from_utf8(axum::body::to_bytes(r2.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
    assert!(text.contains("PERSIMMON5"), "streaming continuation recalls the codeword via resume: {text}");
}
