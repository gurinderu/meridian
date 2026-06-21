use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use meridian::pooled_runner::pooled_runner;
use meridian::profiles::ProfileStore;
use meridian::rate_limit::RateLimitStore;
use meridian::server::router;
use meridian::session::SessionStore;

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn continuation_reuses_a_parked_process() {
    let root = std::env::temp_dir().join(format!("mer-sticky-{}", std::process::id()));
    let profiles = Arc::new(ProfileStore::new(vec![], root.clone()));
    let runner = Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), Arc::new(RateLimitStore::new()), 8));
    let sessions = Arc::new(SessionStore::new());
    let app = router(runner.clone(), sessions, profiles, Arc::new(RateLimitStore::new()));

    // Turn 1: set a codeword (no prior context).
    let r1 = app.clone().oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(json!({"model":"sonnet","messages":[
            {"role":"user","content":"Remember the codeword TANGERINE19. Reply with just OK."}]}).to_string())).unwrap())
        .await.unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    // a process is now parked
    assert_eq!(runner.parked().len(), 1, "turn 1 should park its process");

    // Turn 2: continuation (includes turn-1 user + assistant) -> resume -> warm reuse.
    let r2 = app.oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(json!({"model":"sonnet","messages":[
            {"role":"user","content":"Remember the codeword TANGERINE19. Reply with just OK."},
            {"role":"assistant","content":"OK"},
            {"role":"user","content":"What was the exact codeword?"}]}).to_string())).unwrap())
        .await.unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(r2.into_body(), usize::MAX).await.unwrap()).unwrap();
    let text = v["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("TANGERINE19"), "continuation recalled the codeword: {text}");
}

/// Concatenate the assistant text from an SSE body's `content_block_delta`
/// events. A raw `body.contains("WORD")` is fragile — the CLI splits text
/// across deltas, so the codeword may land in separate chunks.
fn sse_text(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else { continue };
        let Ok(v) = serde_json::from_str::<Value>(data.trim()) else { continue };
        if v.get("type").and_then(|t| t.as_str()) == Some("content_block_delta") {
            if let Some(t) = v.get("delta").and_then(|d| d.get("text")).and_then(|t| t.as_str()) {
                out.push_str(t);
            }
        }
    }
    out
}

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn streaming_continuation_reuses_a_parked_process() {
    let root = std::env::temp_dir().join(format!("mer-streamsticky-{}", std::process::id()));
    let profiles = Arc::new(ProfileStore::new(vec![], root.clone()));
    let runner = Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), Arc::new(RateLimitStore::new()), 8));
    let sessions = Arc::new(SessionStore::new());
    let app = router(runner.clone(), sessions, profiles, Arc::new(RateLimitStore::new()));

    // Turn 1 (streaming): set a codeword; drain SSE body to completion so the
    // pump task finishes and parks its process.
    let b1 = json!({"model":"sonnet","stream":true,"messages":[
        {"role":"user","content":"Remember the codeword TANGERINE19. Reply with just OK."}]});
    let r1 = app.clone().oneshot(
        Request::post("/v1/messages").header("content-type","application/json")
            .body(Body::from(b1.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(r1.into_body(), usize::MAX).await.unwrap(); // drain to completion
    // After drain, the pump task has stored the session and parked the proc.
    assert_eq!(runner.parked().len(), 1, "turn 1 (streaming) must park its process");

    // Turn 2 (streaming continuation): send prefix matching turn 1 -> resume
    // resolved -> warm reuse; the reply must recall the codeword.
    let b2 = json!({"model":"sonnet","stream":true,"messages":[
        {"role":"user","content":"Remember the codeword TANGERINE19. Reply with just OK."},
        {"role":"assistant","content":"OK"},
        {"role":"user","content":"What was the exact codeword?"}]});
    let r2 = app.oneshot(
        Request::post("/v1/messages").header("content-type","application/json")
            .body(Body::from(b2.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(r2.into_body(), usize::MAX).await.unwrap();
    let body_str = String::from_utf8(bytes.to_vec()).unwrap();
    let text = sse_text(&body_str);
    assert!(text.contains("TANGERINE19"), "streaming continuation recalled the codeword via warm reuse: {text}");
}
