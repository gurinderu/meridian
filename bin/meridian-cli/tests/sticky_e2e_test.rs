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
