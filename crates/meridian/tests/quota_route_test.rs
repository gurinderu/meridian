use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use meridian::profiles::ProfileStore;
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

#[tokio::test]
async fn quota_empty_is_200_with_empty_buckets() {
    let rl = Arc::new(RateLimitStore::new());
    let app = router_with_auth(Arc::new(NoRun), Arc::new(SessionStore::new()),
        Arc::new(ProfileStore::new(vec![], std::env::temp_dir())), rl, None);
    let r = app.oneshot(Request::get("/v1/usage/quota").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(v["buckets"].as_array().unwrap().len(), 0);
    assert_eq!(v["extraUsage"], Value::Null);
    assert_eq!(v["sources"]["oauth"], Value::Null);
    assert_eq!(v["sources"]["sdk"]["entryCount"], 0);
}

#[tokio::test]
async fn quota_reflects_recorded_buckets() {
    let rl = Arc::new(RateLimitStore::new());
    rl.record(&serde_json::json!({"status":"allowed","rateLimitType":"five_hour","utilization":0.42}));
    let app = router_with_auth(Arc::new(NoRun), Arc::new(SessionStore::new()),
        Arc::new(ProfileStore::new(vec![], std::env::temp_dir())), rl, None);
    let r = app.oneshot(Request::get("/v1/usage/quota").body(Body::empty()).unwrap()).await.unwrap();
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(v["sources"]["sdk"]["entryCount"], 1);
    assert_eq!(v["buckets"][0]["type"], "five_hour");
    assert_eq!(v["buckets"][0]["utilization"], 0.42);
}
