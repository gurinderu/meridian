use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub enum ProxyError {
    BadRequest(String),
    Upstream(String),
    Internal(String),
}

impl ProxyError {
    pub fn status(&self) -> u16 {
        match self {
            ProxyError::BadRequest(_) => 400,
            ProxyError::Upstream(_) => 502,
            ProxyError::Internal(_) => 500,
        }
    }

    fn parts(&self) -> (&'static str, &str) {
        match self {
            ProxyError::BadRequest(m) => ("invalid_request_error", m),
            ProxyError::Upstream(m) => ("api_error", m),
            ProxyError::Internal(m) => ("internal_error", m),
        }
    }
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        let (kind, msg) = self.parts();
        let code = StatusCode::from_u16(self.status()).unwrap();
        let body = Json(json!({"type":"error","error":{"type":kind,"message":msg}}));
        (code, body).into_response()
    }
}
