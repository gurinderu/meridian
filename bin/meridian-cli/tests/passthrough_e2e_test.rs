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
async fn client_tool_surfaces_as_tool_use() {
    let root = std::env::temp_dir().join(format!("meridian-pt-e2e-{}", std::process::id()));
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2, std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir())))), Arc::new(SessionStore::new()), std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir())));
    let body = json!({
        "model":"sonnet",
        "tools":[{"name":"edit_file","description":"Edit a file","input_schema":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}],
        "messages":[{"role":"user","content":"Use the edit_file tool to set foo.txt to 'hi'. You MUST call the tool."}]
    });
    let resp = app.oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(body.to_string())).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["stop_reason"], "tool_use", "expected tool_use; got {v}");
    assert!(v["content"].as_array().unwrap().iter().any(|b|
        b["type"]=="tool_use" && b["name"]=="edit_file"), "no edit_file tool_use block in {v}");
}
