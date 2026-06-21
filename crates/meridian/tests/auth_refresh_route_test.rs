// crates/meridian/tests/auth_refresh_route_test.rs
use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use meridian::profiles::{ProfileConfig, ProfileStore, ProfileType};
use meridian::rate_limit::RateLimitStore;
use meridian::server::router_with_auth;
use meridian::session::SessionStore;

#[derive(Clone)] struct NoRun;
impl meridian::server::TurnRunner for NoRun {
    async fn run_turn(&self, _r: meridian::server::TurnRequest)
        -> Result<meridian::server::TurnResult, meridian::error::ProxyError> {
        Err(meridian::error::ProxyError::Internal("unused".into()))
    }
}
impl meridian::server::StreamRunner for NoRun {
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String, _pr: Option<String>, _resume: Option<String>, _messages: Vec<serde_json::Value>, _sessions: std::sync::Arc<meridian::session::SessionStore>)
        -> meridian::sse::EventStream {
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        tokio_stream::wrappers::ReceiverStream::new(rx)
    }
}

fn app(profiles: Vec<ProfileConfig>) -> axum::Router {
    router_with_auth(Arc::new(NoRun), Arc::new(SessionStore::new()),
        Arc::new(ProfileStore::new(profiles, std::env::temp_dir())),
        Arc::new(RateLimitStore::new()), None)
}

#[tokio::test]
async fn refresh_for_profile_pointing_at_empty_dir_fails_500() {
    // claude-max profile whose config dir has no credentials -> refresh fails.
    let dir = std::env::temp_dir().join(format!("mer-norefresh-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let p = ProfileConfig { id: "work".into(), kind: Some(ProfileType::ClaudeMax),
        claude_config_dir: Some(dir.to_string_lossy().into()), api_key: None, base_url: None, oauth_token: None };
    let app = app(vec![p]);
    let r = app.oneshot(Request::post("/auth/refresh")
        .header("x-meridian-profile","work").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(v["success"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn refresh_for_oauth_token_profile_is_failure() {
    // oauth-token profiles supply the token via env; no on-disk creds to refresh.
    let p = ProfileConfig { id: "ci".into(), kind: Some(ProfileType::OauthToken),
        claude_config_dir: None, api_key: None, base_url: None, oauth_token: Some("t".into()) };
    let app = app(vec![p]);
    let r = app.oneshot(Request::post("/auth/refresh")
        .header("x-meridian-profile","ci").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn refresh_for_api_profile_is_failure() {
    let p = ProfileConfig { id: "apikey".into(), kind: Some(ProfileType::Api),
        claude_config_dir: None, api_key: Some("k".into()), base_url: None, oauth_token: None };
    let app = app(vec![p]);
    let r = app.oneshot(Request::post("/auth/refresh")
        .header("x-meridian-profile","apikey").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
