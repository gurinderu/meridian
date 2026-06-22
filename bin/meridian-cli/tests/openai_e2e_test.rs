use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use meridian::pooled_runner::pooled_runner;
use meridian::server::router;
use meridian::session::SessionStore;

/// Accumulate `choices[].delta.content` from OpenAI-format SSE chunks.
/// Raw substring-matching the body is fragile — text may be split across chunks.
fn openai_sse_text(body: &str) -> String {
    let mut out = String::new();
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else { continue };
        let trimmed = data.trim();
        if trimmed == "[DONE]" { continue }
        let Ok(v) = serde_json::from_str::<Value>(trimmed) else { continue };
        if let Some(text) = v["choices"][0]["delta"]["content"].as_str() {
            out.push_str(text);
        }
    }
    out
}

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn openai_chat_completions_end_to_end() {
    let root = std::env::temp_dir().join(format!("meridian-oai-{}", std::process::id()));
    let profiles = std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir()));
    let rate_limit = Arc::new(meridian::rate_limit::RateLimitStore::new());
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), rate_limit.clone(), 8)), Arc::new(SessionStore::new()), profiles, rate_limit);
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

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn openai_chat_completions_streaming_end_to_end() {
    let root = std::env::temp_dir().join(format!("meridian-oai-stream-{}", std::process::id()));
    let profiles = std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir()));
    let rate_limit = Arc::new(meridian::rate_limit::RateLimitStore::new());
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), rate_limit.clone(), 8)), Arc::new(SessionStore::new()), profiles, rate_limit);
    let body = serde_json::json!({"model":"sonnet","stream":true,"messages":[{"role":"user","content":"Reply with exactly: OK"}]});
    let resp = app.oneshot(
        Request::post("/v1/chat/completions").header("content-type","application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(text.contains("chat.completion.chunk"), "no chunks in body: {text}");
    assert!(text.contains("data: [DONE]"), "missing [DONE]: {text}");
}

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn openai_non_stream_multi_turn_resume_and_session_stored() {
    let root = std::env::temp_dir().join(format!("meridian-oai-resume-{}", std::process::id()));
    let profiles = std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir()));
    let rate_limit = Arc::new(meridian::rate_limit::RateLimitStore::new());
    let sessions = Arc::new(SessionStore::new());
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), rate_limit.clone(), 8)), sessions.clone(), profiles, rate_limit);

    // Turn 1: set a codeword and record the reply.
    let b1 = json!({"model":"sonnet","messages":[
        {"role":"user","content":"Remember codeword VELVET42. Reply with exactly: OK"}]});
    let r1 = app.clone().oneshot(Request::post("/v1/chat/completions").header("content-type","application/json")
        .body(Body::from(b1.to_string())).unwrap()).await.unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let v1: Value = serde_json::from_slice(&axum::body::to_bytes(r1.into_body(), usize::MAX).await.unwrap()).unwrap();
    let reply1 = v1["choices"][0]["message"]["content"].as_str().unwrap_or("").to_string();
    assert!(sessions.len_for_test() >= 1, "turn 1 must store a session");

    // Turn 2: echo turn-1 reply and ask for the codeword — must resume and recall.
    let b2 = json!({"model":"sonnet","messages":[
        {"role":"user","content":"Remember codeword VELVET42. Reply with exactly: OK"},
        {"role":"assistant","content":reply1},
        {"role":"user","content":"What was the codeword? Reply with ONLY the codeword."}]});
    let r2 = app.oneshot(Request::post("/v1/chat/completions").header("content-type","application/json")
        .body(Body::from(b2.to_string())).unwrap()).await.unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let v2: Value = serde_json::from_slice(&axum::body::to_bytes(r2.into_body(), usize::MAX).await.unwrap()).unwrap();
    let reply2 = v2["choices"][0]["message"]["content"].as_str().unwrap_or("");
    assert!(reply2.to_uppercase().contains("VELVET"), "resume lost context; got {reply2:?}");
}

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn openai_stream_multi_turn_resume_recalls_context() {
    let root = std::env::temp_dir().join(format!("meridian-oai-streamresume-{}", std::process::id()));
    let profiles = std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir()));
    let rate_limit = Arc::new(meridian::rate_limit::RateLimitStore::new());
    let sessions = Arc::new(SessionStore::new());
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), rate_limit.clone(), 8)), sessions.clone(), profiles, rate_limit);

    // Turn 1 (streaming): set a codeword, drain to completion so session is stored.
    let b1 = json!({"model":"sonnet","stream":true,"messages":[
        {"role":"user","content":"Remember codeword CRIMSON9. Reply with just OK."}]});
    let r1 = app.clone().oneshot(Request::post("/v1/chat/completions").header("content-type","application/json")
        .body(Body::from(b1.to_string())).unwrap()).await.unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(r1.into_body(), usize::MAX).await.unwrap(); // drain
    assert_eq!(sessions.len_for_test(), 1, "turn 1 must store a streaming session");

    // Turn 2 (streaming): echoes turn 1 assistant reply "OK", asks for the codeword.
    let b2 = json!({"model":"sonnet","stream":true,"messages":[
        {"role":"user","content":"Remember codeword CRIMSON9. Reply with just OK."},
        {"role":"assistant","content":"OK"},
        {"role":"user","content":"What was the exact codeword?"}]});
    let r2 = app.oneshot(Request::post("/v1/chat/completions").header("content-type","application/json")
        .body(Body::from(b2.to_string())).unwrap()).await.unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let body = String::from_utf8(axum::body::to_bytes(r2.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
    let text = openai_sse_text(&body);
    assert!(text.to_uppercase().contains("CRIMSON"), "streaming resume lost context; accumulated: {text:?}");
}
