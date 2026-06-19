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
async fn openai_tool_call_surfaces() {
    let root = std::env::temp_dir().join(format!("meridian-oai-tools-{}", std::process::id()));
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2)), Arc::new(SessionStore::new()));
    let body = json!({
        "model":"sonnet",
        "tools":[{"type":"function","function":{"name":"get_weather","description":"Get the weather for a city","parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}}],
        "messages":[{"role":"user","content":"What is the weather in Paris? Use the get_weather tool."}]
    });
    let resp = app.oneshot(Request::post("/v1/chat/completions").header("content-type","application/json")
        .body(Body::from(body.to_string())).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["choices"][0]["finish_reason"], "tool_calls", "expected tool_calls; got {v}");
    assert_eq!(v["choices"][0]["message"]["tool_calls"][0]["function"]["name"], "get_weather");
}
