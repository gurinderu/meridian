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
struct RecordingRunner { last_resume: Arc<Mutex<Option<Option<String>>>>, last_prompt: Arc<Mutex<String>> }
impl TurnRunner for RecordingRunner {
    async fn run_turn(&self, req: TurnRequest) -> Result<TurnResult, ProxyError> {
        *self.last_resume.lock().unwrap() = Some(req.resume.clone());
        *self.last_prompt.lock().unwrap() = req.prompt.clone();
        Ok(TurnResult {
            message: json!({"role":"assistant","content":[{"type":"text","text":"reply"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}),
            session_id: Some("sess-A".into()),
        })
    }
}
impl StreamRunner for RecordingRunner {
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String) -> EventStream {
        let (_tx, rx) = mpsc::channel::<Value>(1);
        ReceiverStream::new(rx)
    }
}

async fn post(app: axum::Router, body: Value) {
    let _ = app.oneshot(
        Request::post("/v1/messages").header("content-type","application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
}

#[tokio::test]
async fn second_turn_resumes_and_sends_only_new_user_message() {
    let runner = Arc::new(RecordingRunner::default());
    let sessions = Arc::new(SessionStore::new());
    let app = || router(runner.clone(), sessions.clone());

    // Turn 1: single user message -> fresh (resume None), stores sess-A under the post-turn fingerprint.
    post(app(), json!({"model":"opus","messages":[{"role":"user","content":"u1"}]})).await;
    assert_eq!(*runner.last_resume.lock().unwrap(), Some(None), "turn 1 is fresh");

    // Turn 2: client echoes our assistant reply + a new user msg -> should resume sess-A and send only u2.
    post(app(), json!({"model":"opus","messages":[
        {"role":"user","content":"u1"},
        {"role":"assistant","content":"reply"},
        {"role":"user","content":"u2"}
    ]})).await;
    assert_eq!(*runner.last_resume.lock().unwrap(), Some(Some("sess-A".into())), "turn 2 resumes sess-A");
    assert_eq!(*runner.last_prompt.lock().unwrap(), "u2", "turn 2 sends only the new user message");
}
