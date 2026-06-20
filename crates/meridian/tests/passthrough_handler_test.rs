use std::sync::{Arc, Mutex};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use meridian::error::ProxyError;
use meridian::server::{router, StreamRunner, TurnRequest, TurnResult, TurnRunner};
use meridian::session::SessionStore;
use meridian::sse::EventStream;

#[derive(Default)]
struct ToolRunner { saw_tools: Arc<Mutex<usize>> }
impl TurnRunner for ToolRunner {
    async fn run_turn(&self, req: TurnRequest) -> Result<TurnResult, ProxyError> {
        *self.saw_tools.lock().unwrap() = req.tools.len();
        Ok(TurnResult {
            message: json!({"role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":1}}),
            session_id: None,
            captured_tools: vec![json!({"id":"tu_1","name":"mcp__oc__edit_file","input":{"path":"foo.txt"}})],
        })
    }
}
impl StreamRunner for ToolRunner {
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String, _profile: Option<String>) -> EventStream {
        let (_tx, rx) = mpsc::channel::<Value>(1);
        ReceiverStream::new(rx)
    }
}

#[tokio::test]
async fn surfaces_tool_use_with_stop_reason() {
    let runner = Arc::new(ToolRunner::default());
    let app = router(runner.clone(), Arc::new(SessionStore::new()), Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), "/cfg".into())), Arc::new(meridian::rate_limit::RateLimitStore::new()));
    let body = json!({
        "model":"opus",
        "tools":[{"name":"edit_file","description":"Edit","input_schema":{"type":"object"}}],
        "messages":[{"role":"user","content":"edit foo.txt"}]
    });
    let resp = app.oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(body.to_string())).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(*runner.saw_tools.lock().unwrap(), 1, "tools were parsed + passed to the runner");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["stop_reason"], "tool_use");
    assert_eq!(v["content"][0]["type"], "tool_use");
    assert_eq!(v["content"][0]["name"], "edit_file", "mcp__oc__ prefix stripped");
    assert_eq!(v["content"][0]["id"], "tu_1");
    assert_eq!(v["content"][0]["input"]["path"], "foo.txt");
}
