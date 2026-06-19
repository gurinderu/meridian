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
async fn two_turn_conversation_resumes_context() {
    let root = std::env::temp_dir().join(format!("meridian-resume-{}", std::process::id()));
    let profiles = std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir()));
    let runner = Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone()));
    let sessions = Arc::new(SessionStore::new());
    let app = || router(runner.clone(), sessions.clone(), profiles.clone());

    // Turn 1: state a codeword.
    let r1 = app().oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(json!({"model":"sonnet","messages":[{"role":"user","content":"Remember codeword ZORBLAX-7. Reply with exactly: OK"}]}).to_string())).unwrap()).await.unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let b1 = axum::body::to_bytes(r1.into_body(), usize::MAX).await.unwrap();
    let v1: Value = serde_json::from_slice(&b1).unwrap();
    let reply1 = v1["content"][0]["text"].as_str().unwrap_or("").to_string();

    // Turn 2: echo turn-1 reply + ask for the codeword -> must resume and remember.
    let r2 = app().oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(json!({"model":"sonnet","messages":[
            {"role":"user","content":"Remember codeword ZORBLAX-7. Reply with exactly: OK"},
            {"role":"assistant","content":reply1},
            {"role":"user","content":"What was the codeword? Reply with ONLY the codeword."}
        ]}).to_string())).unwrap()).await.unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let b2 = axum::body::to_bytes(r2.into_body(), usize::MAX).await.unwrap();
    let v2: Value = serde_json::from_slice(&b2).unwrap();
    let reply2 = v2["content"][0]["text"].as_str().unwrap_or("");
    assert!(reply2.to_uppercase().contains("ZORBLAX"), "resume lost context; got {reply2:?}");
}
