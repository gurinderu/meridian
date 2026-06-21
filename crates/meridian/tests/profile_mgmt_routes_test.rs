use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use meridian::profiles::{ProfileConfig, ProfileStore, ProfileType};
use meridian::server::router;
use meridian::session::SessionStore;
use meridian::sse::EventStream;
use meridian::error::ProxyError;

// Minimal runner stub: never actually spawns anything (these routes don't run turns).
#[derive(Clone)] struct NoRun;
impl meridian::server::TurnRunner for NoRun {
    async fn run_turn(&self, _r: meridian::server::TurnRequest)
        -> Result<meridian::server::TurnResult, ProxyError> {
        Err(ProxyError::Internal("unused".into()))
    }
}
impl meridian::server::StreamRunner for NoRun {
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String, _pr: Option<String>, _resume: Option<String>, _messages: Vec<serde_json::Value>, _sessions: std::sync::Arc<meridian::session::SessionStore>)
        -> EventStream {
        let (_tx, rx) = mpsc::channel::<Value>(1);
        ReceiverStream::new(rx)
    }
}

fn app(profiles: Vec<ProfileConfig>) -> axum::Router {
    let store = Arc::new(ProfileStore::new(profiles, std::env::temp_dir()));
    router(Arc::new(NoRun), Arc::new(SessionStore::new()), store, Arc::new(meridian::rate_limit::RateLimitStore::new()))
}

fn pc(id: &str, kind: ProfileType) -> ProfileConfig {
    ProfileConfig { id: id.into(), kind: Some(kind), claude_config_dir: Some("/x".into()),
        api_key: None, base_url: None, oauth_token: None }
}

#[tokio::test]
async fn list_returns_profiles_and_active() {
    let app = app(vec![pc("a", ProfileType::ClaudeMax), pc("b", ProfileType::Api)]);
    let r = app.oneshot(Request::get("/profiles/list").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(v["profiles"].as_array().unwrap().len(), 2);
    assert_eq!(v["profiles"][0]["id"], "a");
    assert_eq!(v["profiles"][1]["type"], "api");
    assert_eq!(v["activeProfile"], "a"); // first is active by default
}

#[tokio::test]
async fn active_switches_known_profile_and_rejects_unknown() {
    let app = app(vec![pc("a", ProfileType::ClaudeMax), pc("b", ProfileType::ClaudeMax)]);
    // unknown -> 400
    let bad = Request::post("/profiles/active").header("content-type","application/json")
        .body(Body::from(r#"{"profile":"nope"}"#)).unwrap();
    let rb = app.clone().oneshot(bad).await.unwrap();
    assert_eq!(rb.status(), StatusCode::BAD_REQUEST);
    // known -> 200 success
    let ok = Request::post("/profiles/active").header("content-type","application/json")
        .body(Body::from(r#"{"profile":"b"}"#)).unwrap();
    let ro = app.clone().oneshot(ok).await.unwrap();
    assert_eq!(ro.status(), StatusCode::OK);
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(ro.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(v["success"], true);
    assert_eq!(v["activeProfile"], "b");
    // now list reports b active
    let rl = app.oneshot(Request::get("/profiles/list").body(Body::empty()).unwrap()).await.unwrap();
    let lv: Value = serde_json::from_slice(&axum::body::to_bytes(rl.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(lv["activeProfile"], "b");
}

#[tokio::test]
async fn active_with_no_profiles_is_400() {
    let app = app(vec![]);
    let r = app.oneshot(Request::post("/profiles/active").header("content-type","application/json")
        .body(Body::from(r#"{"profile":"x"}"#)).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
}
