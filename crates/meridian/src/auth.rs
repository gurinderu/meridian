//! Optional API-key authentication.
//!
//! When `MERIDIAN_API_KEY` is set to a non-empty value, protected routes
//! require a matching key via the `x-api-key` header or
//! `Authorization: Bearer <key>`. When unset, all routes are open (default,
//! backward compatible). Port of `src-original/src/proxy/auth.ts`.
//!
//! The comparison is constant-time over the secret's length so a timing side
//! channel cannot reveal the key. We avoid pulling in a crypto crate (the
//! original HMAC-then-timingSafeEqual dance) — for a localhost proxy key this
//! length-folded compare is sufficient and dependency-free.

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// True when `MERIDIAN_API_KEY` is set to a non-empty value.
pub fn auth_enabled() -> bool {
    configured_key().is_some()
}

/// The configured key from the environment, or `None` when unset/empty.
/// Read once at router build (see `router`), not per request: changing
/// `MERIDIAN_API_KEY` on a running server has no effect until restart.
pub fn configured_key() -> Option<String> {
    std::env::var("MERIDIAN_API_KEY").ok().filter(|s| !s.is_empty())
}

/// Constant-time comparison of a provided key against the secret. The loop
/// length is the secret's length (constant for a deployment), so timing does
/// not depend on the provided value's content; a length mismatch is folded in
/// without an early return.
pub fn constant_time_eq(provided: &[u8], secret: &[u8]) -> bool {
    // The length XOR MUST be seeded before the loop: it is what guards against a
    // false-accept when `provided` is shorter than `secret` (the loop's
    // unwrap_or(0) would otherwise let a NUL-padded short input slip through if a
    // secret byte were also 0x00). Any length difference makes `diff` non-zero,
    // so `diff == 0` is unreachable unless the byte sequences are identical.
    let mut diff = (provided.len() ^ secret.len()) as u32;
    for (i, &s) in secret.iter().enumerate() {
        let p = provided.get(i).copied().unwrap_or(0);
        diff |= (p ^ s) as u32;
    }
    diff == 0
}

/// Extract the API key from the request headers: `x-api-key` first, then
/// `Authorization: Bearer <key>`. An empty `x-api-key` falls through.
pub fn extract_key<'a>(x_api_key: Option<&'a str>, authorization: Option<&'a str>) -> Option<&'a str> {
    if let Some(k) = x_api_key.filter(|s| !s.is_empty()) {
        return Some(k);
    }
    authorization.and_then(|a| a.strip_prefix("Bearer "))
}

/// Decide whether a request is authorized. `configured` is `None`/empty when
/// auth is disabled — then every request is allowed.
pub fn is_authorized(configured: Option<&str>, x_api_key: Option<&str>, authorization: Option<&str>) -> bool {
    let Some(key) = configured.filter(|s| !s.is_empty()) else {
        return true;
    };
    match extract_key(x_api_key, authorization) {
        Some(provided) => constant_time_eq(provided.as_bytes(), key.as_bytes()),
        None => false,
    }
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({
            "type": "error",
            "error": { "type": "authentication_error", "message": "Invalid or missing API key" }
        })),
    )
        .into_response()
}

/// axum middleware (via `from_fn_with_state`): rejects requests without a valid
/// key. The captured `api_key` is the key snapshot taken at router build; when
/// `None` the middleware is a pass-through.
pub async fn require_auth(State(api_key): State<Option<String>>, req: Request<Body>, next: Next) -> Response {
    let headers = req.headers();
    let x = headers.get("x-api-key").and_then(|v| v.to_str().ok());
    let a = headers.get("authorization").and_then(|v| v.to_str().ok());
    if is_authorized(api_key.as_deref(), x, a) {
        next.run(req).await
    } else {
        unauthorized()
    }
}
