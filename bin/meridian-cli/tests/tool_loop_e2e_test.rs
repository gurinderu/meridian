use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use meridian::pooled_runner::pooled_runner;
use meridian::server::router;
use meridian::session::SessionStore;

async fn post(app: &axum::Router, body: Value) -> Value {
    let r = app.clone().oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(body.to_string())).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let b = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&b).unwrap()
}

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn full_tool_loop_returns_final_answer() {
    let root = std::env::temp_dir().join(format!("meridian-loop-{}", std::process::id()));
    let profiles = std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir()));
    let rate_limit = Arc::new(meridian::rate_limit::RateLimitStore::new());
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), rate_limit.clone())), Arc::new(SessionStore::new()), profiles, rate_limit);
    let weather_tool = json!({"name":"get_weather","description":"Get the weather for a city","input_schema":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}});

    // Turn 1: force a tool call.
    let v1 = post(&app, json!({"model":"sonnet","tools":[weather_tool],
        "messages":[{"role":"user","content":"What is the weather in Paris? Use the get_weather tool."}]})).await;
    assert_eq!(v1["stop_reason"], "tool_use", "turn 1 should surface a tool_use; got {v1}");
    let tu = v1["content"].as_array().unwrap().iter().find(|b| b["type"]=="tool_use").unwrap().clone();
    let tu_id = tu["id"].as_str().unwrap();

    // Turn 2: echo the tool_use + send a tool_result -> resume -> final answer.
    let v2 = post(&app, json!({"model":"sonnet","tools":[weather_tool],"messages":[
        {"role":"user","content":"What is the weather in Paris? Use the get_weather tool."},
        {"role":"assistant","content":[tu]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":tu_id,"content":"18C and sunny"}]}
    ]})).await;
    let text = v2["content"].as_array().unwrap().iter()
        .filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().concat();
    assert!(text.to_lowercase().contains("sunny") || text.contains("18"),
        "turn 2 should incorporate the tool result; got {v2}");
}
