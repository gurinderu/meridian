use std::sync::{Arc, Mutex};
use axum::body::Body;
use axum::http::Request;
use serde_json::{json, Value};
use tower::ServiceExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use meridian::error::ProxyError;
use meridian::profiles::{ProfileConfig, ProfileStore, ProfileType};
use meridian::server::{router, StreamRunner, TurnRequest, TurnResult, TurnRunner};
use meridian::session::SessionStore;
use meridian::sse::EventStream;

#[derive(Default)]
struct RecordingRunner { last_profile: Arc<Mutex<Option<String>>> }
impl TurnRunner for RecordingRunner {
    async fn run_turn(&self, req: TurnRequest) -> Result<TurnResult, ProxyError> {
        *self.last_profile.lock().unwrap() = req.profile.clone();
        Ok(TurnResult {
            message: json!({"role":"assistant","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}),
            session_id: None, captured_tools: vec![],
        })
    }
}
impl StreamRunner for RecordingRunner {
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String, _profile: Option<String>, _resume: Option<String>, _messages: Vec<serde_json::Value>, _sessions: std::sync::Arc<meridian::session::SessionStore>) -> EventStream {
        let (_tx, rx) = mpsc::channel::<Value>(1);
        ReceiverStream::new(rx)
    }
}

fn store() -> Arc<ProfileStore> {
    let mk = |id: &str, kind| ProfileConfig { id: id.into(), kind: Some(kind), claude_config_dir: None, api_key: Some("k".into()), base_url: None, oauth_token: None };
    Arc::new(ProfileStore::new(vec![mk("personal", ProfileType::Api), mk("work", ProfileType::Api)], "/cfg".into()))
}

async fn post_with(app: axum::Router, header: Option<&str>) {
    let mut b = Request::post("/v1/messages").header("content-type", "application/json");
    if let Some(h) = header { b = b.header("x-meridian-profile", h); }
    let _ = app.oneshot(b.body(Body::from(json!({"model":"sonnet","messages":[{"role":"user","content":"hi"}]}).to_string())).unwrap()).await.unwrap();
}

#[tokio::test]
async fn header_selects_profile_else_first() {
    let runner = Arc::new(RecordingRunner::default());
    let app = || router(runner.clone(), Arc::new(SessionStore::new()), store(), Arc::new(meridian::rate_limit::RateLimitStore::new()));

    post_with(app(), Some("work")).await;
    assert_eq!(*runner.last_profile.lock().unwrap(), Some("work".into()), "header selects profile");

    post_with(app(), None).await;
    assert_eq!(*runner.last_profile.lock().unwrap(), Some("personal".into()), "no header -> first profile");

    post_with(app(), Some("ghost")).await;
    assert_eq!(*runner.last_profile.lock().unwrap(), Some("personal".into()), "unknown header -> first profile");
}
