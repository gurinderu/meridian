use std::sync::{Arc, Mutex};
use axum::body::Body;
use axum::http::Request;
use serde_json::{json, Value};
use tower::ServiceExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use meridian::error::ProxyError;
use meridian::server::{router, StreamRunner, TurnRequest, TurnResult, TurnRunner};
use meridian::session::SessionStore;
use meridian::sse::EventStream;

#[derive(Default)]
struct RecRunner { prompt: Arc<Mutex<String>> }
impl TurnRunner for RecRunner {
    async fn run_turn(&self, req: TurnRequest) -> Result<TurnResult, ProxyError> {
        *self.prompt.lock().unwrap() = req.prompt.clone();
        Ok(TurnResult {
            message: json!({"role":"assistant","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":1,"output_tokens":1}}),
            session_id: None, captured_tools: vec![],
        })
    }
}
impl StreamRunner for RecRunner {
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String) -> EventStream {
        let (_tx, rx) = mpsc::channel::<Value>(1); ReceiverStream::new(rx)
    }
}

#[tokio::test]
async fn tool_result_request_sends_unwrapped_result_as_prompt() {
    let runner = Arc::new(RecRunner::default());
    let app = router(runner.clone(), Arc::new(SessionStore::new()));
    let body = json!({"model":"opus","tools":[{"name":"get_weather"}],"messages":[
        {"role":"user","content":"weather in Paris?"},
        {"role":"assistant","content":[{"type":"tool_use","id":"tu_9","name":"get_weather","input":{"city":"Paris"}}]},
        {"role":"user","content":[{"type":"tool_result","tool_use_id":"tu_9","content":"18C sunny"}]}
    ]});
    let _ = app.oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let p = runner.prompt.lock().unwrap().clone();
    assert!(p.contains("18C sunny"), "prompt carries the unwrapped tool result: {p}");
    assert!(p.contains("tu_9"), "prompt references the tool_use_id: {p}");
}
