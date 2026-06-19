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
struct ToolRunner { saw: Arc<Mutex<usize>> }
impl TurnRunner for ToolRunner {
    async fn run_turn(&self, req: TurnRequest) -> Result<TurnResult, ProxyError> {
        *self.saw.lock().unwrap() = req.tools.len();
        Ok(TurnResult {
            message: json!({"role":"assistant","content":[],"usage":{"input_tokens":1,"output_tokens":1}}),
            session_id: None,
            captured_tools: vec![json!({"id":"call_1","name":"mcp__oc__get_weather","input":{"city":"Paris"}})],
        })
    }
}
impl StreamRunner for ToolRunner {
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String, _profile: Option<String>) -> EventStream {
        let (_tx, rx) = mpsc::channel::<Value>(1); ReceiverStream::new(rx)
    }
}

#[tokio::test]
async fn chat_completions_surfaces_tool_calls() {
    let runner = Arc::new(ToolRunner::default());
    let app = router(runner.clone(), Arc::new(SessionStore::new()), Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), "/cfg".into())));
    let body = json!({
        "model":"opus",
        "tools":[{"type":"function","function":{"name":"get_weather","parameters":{"type":"object"}}}],
        "messages":[{"role":"user","content":"weather in Paris?"}]
    });
    let resp = app.oneshot(Request::post("/v1/chat/completions").header("content-type","application/json")
        .body(Body::from(body.to_string())).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(*runner.saw.lock().unwrap(), 1, "tools parsed + passed to runner");
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(v["choices"][0]["message"]["tool_calls"][0]["function"]["name"], "get_weather");
    let args = v["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"].as_str().unwrap();
    assert_eq!(serde_json::from_str::<Value>(args).unwrap()["city"], "Paris");
}
