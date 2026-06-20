use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use meridian::pooled_runner::pooled_runner;
use meridian::profiles::{ProfileConfig, ProfileStore, ProfileType};
use meridian::server::router;
use meridian::session::SessionStore;

fn req(profile: Option<&str>) -> Request<Body> {
    let mut b = Request::post("/v1/messages").header("content-type", "application/json");
    if let Some(p) = profile { b = b.header("x-meridian-profile", p); }
    b.body(Body::from(json!({"model":"sonnet","messages":[{"role":"user","content":"Reply with exactly: OK"}]}).to_string())).unwrap()
}

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn api_profile_repoints_subprocess() {
    let root = std::env::temp_dir().join(format!("meridian-prof-{}", std::process::id()));
    // "broken" api profile -> unreachable base url; proves the overlay reaches the subprocess.
    let broken = ProfileConfig {
        id: "broken".into(), kind: Some(ProfileType::Api),
        claude_config_dir: None, api_key: Some("sk-bogus".into()),
        base_url: Some("http://127.0.0.1:1/".into()), oauth_token: None,
    };
    let profiles = Arc::new(ProfileStore::new(vec![broken], root.clone()));
    let rate_limit = Arc::new(meridian::rate_limit::RateLimitStore::new());
    let runner = Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), rate_limit.clone()));
    let app = router(runner, Arc::new(SessionStore::new()), profiles, rate_limit);

    // The broken profile must fail (overlay reached the subprocess and re-pointed it).
    let r_broken = app.oneshot(req(Some("broken"))).await.unwrap();
    assert_ne!(r_broken.status(), StatusCode::OK, "broken api profile should fail to reach upstream");

    // A second store with NO profiles falls back to host creds and succeeds.
    let host_root = std::env::temp_dir().join(format!("meridian-host-{}", std::process::id()));
    let host_store = Arc::new(ProfileStore::new(vec![], host_root.clone()));
    let host_rate_limit = Arc::new(meridian::rate_limit::RateLimitStore::new());
    let host_runner = Arc::new(pooled_runner("claude".into(), host_root, 2, host_store.clone(), host_rate_limit.clone()));
    let host_app = router(host_runner, Arc::new(SessionStore::new()), host_store, host_rate_limit);
    let r_host = host_app.oneshot(req(None)).await.unwrap();
    assert_eq!(r_host.status(), StatusCode::OK, "no-profile path should use host creds and succeed");
    let body = axum::body::to_bytes(r_host.into_body(), usize::MAX).await.unwrap();
    let v: Value = serde_json::from_slice(&body).unwrap();
    assert!(v["content"][0]["text"].as_str().unwrap_or("").to_uppercase().contains("OK"));
}
