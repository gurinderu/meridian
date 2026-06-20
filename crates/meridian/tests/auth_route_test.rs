use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;
use meridian::profiles::ProfileStore;
use meridian::server::router_with_auth;
use meridian::session::SessionStore;

// /v1/models needs no runner state, but router_with_auth is generic over R, so
// provide a never-called stub runner.
#[derive(Clone)] struct NoRun;
impl meridian::server::TurnRunner for NoRun {
    async fn run_turn(&self, _r: meridian::server::TurnRequest)
        -> Result<meridian::server::TurnResult, meridian::error::ProxyError> {
        Err(meridian::error::ProxyError::Internal("unused".into()))
    }
}
impl meridian::server::StreamRunner for NoRun {
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String, _pr: Option<String>)
        -> meridian::sse::EventStream {
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        tokio_stream::wrappers::ReceiverStream::new(rx)
    }
}

fn app(key: Option<&str>) -> axum::Router {
    router_with_auth(
        Arc::new(NoRun),
        Arc::new(SessionStore::new()),
        Arc::new(ProfileStore::new(vec![], std::env::temp_dir())),
        key.map(str::to_string),
    )
}

async fn status(app: axum::Router, req: Request<Body>) -> StatusCode {
    app.oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn no_key_configured_leaves_routes_open() {
    let s = status(app(None), Request::get("/v1/models").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn configured_key_rejects_missing_and_wrong_accepts_valid() {
    // missing -> 401
    assert_eq!(
        status(app(Some("secret")), Request::get("/v1/models").body(Body::empty()).unwrap()).await,
        StatusCode::UNAUTHORIZED);
    // wrong -> 401
    assert_eq!(
        status(app(Some("secret")), Request::get("/v1/models").header("x-api-key","nope").body(Body::empty()).unwrap()).await,
        StatusCode::UNAUTHORIZED);
    // valid x-api-key -> 200
    assert_eq!(
        status(app(Some("secret")), Request::get("/v1/models").header("x-api-key","secret").body(Body::empty()).unwrap()).await,
        StatusCode::OK);
    // valid Bearer -> 200
    assert_eq!(
        status(app(Some("secret")), Request::get("/v1/models").header("authorization","Bearer secret").body(Body::empty()).unwrap()).await,
        StatusCode::OK);
}

#[tokio::test]
async fn health_is_open_even_with_auth() {
    assert_eq!(
        status(app(Some("secret")), Request::get("/health").body(Body::empty()).unwrap()).await,
        StatusCode::OK);
}
