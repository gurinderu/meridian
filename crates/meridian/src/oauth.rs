//! OAuth primitives for browser-based profile login (PKCE + authorize URL +
//! code parsing + token exchange).
//!
//! Port of `src-original/src/proxy/profileCli.ts` — createManualOAuthSession,
//! parseAuthorizationCodeInput, and the token-exchange path.

use crate::token_refresh::{self, CredentialsFile, OAuthCredentials};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const OAUTH_AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const OAUTH_REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const OAUTH_SCOPES: &[&str] = &[
    "org:create_api_key",
    "user:profile",
    "user:inference",
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
];

// ---------------------------------------------------------------------------
// base64url — RFC 4648 §5, no padding
// ---------------------------------------------------------------------------

/// Standard base64 alphabet → url-safe base64 alphabet, no padding.
/// Maps `+` → `-`, `/` → `_`, strips `=`.
pub fn base64url(bytes: &[u8]) -> String {
    // hand-rolled to avoid any new crate
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = if i+1 < bytes.len() { bytes[i+1] as u32 } else { 0 };
        let b2 = if i+2 < bytes.len() { bytes[i+2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        let rem = bytes.len() - i;
        out.push(ALPHA[((n >> 18) & 0x3f) as usize] as char);
        out.push(ALPHA[((n >> 12) & 0x3f) as usize] as char);
        if rem > 1 { out.push(ALPHA[((n >> 6) & 0x3f) as usize] as char); }
        if rem > 2 { out.push(ALPHA[(n & 0x3f) as usize] as char); }
        i += 3;
    }
    out
}

// ---------------------------------------------------------------------------
// PKCE
// ---------------------------------------------------------------------------

/// PKCE code challenge: `base64url(sha256(verifier_ascii_bytes))`.
/// Verified against RFC 7636 Appendix B.
pub fn code_challenge_for(verifier: &str) -> String {
    base64url(&token_refresh::sha256_bytes(verifier.as_bytes()))
}

// ---------------------------------------------------------------------------
// Authorize URL
// ---------------------------------------------------------------------------

/// Percent-encode a string for use as a URL query value.
/// Encodes everything outside the unreserved set (A-Za-z0-9 - _ . ~).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.as_bytes() {
        let c = *byte;
        if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b'.' || c == b'~' {
            out.push(c as char);
        } else {
            out.push('%');
            out.push(char::from_digit((c >> 4) as u32, 16).unwrap().to_ascii_uppercase());
            out.push(char::from_digit((c & 0xf) as u32, 16).unwrap().to_ascii_uppercase());
        }
    }
    out
}

/// Build the authorize URL. Params in the exact order required by the spec:
/// code=true, client_id, response_type=code, redirect_uri, scope, code_challenge,
/// code_challenge_method=S256, state.
pub fn build_authorize_url(code_challenge: &str, state: &str) -> String {
    let scope = OAUTH_SCOPES.join(" ");
    let params = [
        ("code", "true".to_string()),
        ("client_id", OAUTH_CLIENT_ID.to_string()),
        ("response_type", "code".to_string()),
        ("redirect_uri", OAUTH_REDIRECT_URI.to_string()),
        ("scope", scope),
        ("code_challenge", code_challenge.to_string()),
        ("code_challenge_method", "S256".to_string()),
        ("state", state.to_string()),
    ];
    let qs: String = params.iter()
        .map(|(k, v)| format!("{}={}", k, percent_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{OAUTH_AUTHORIZE_URL}?{qs}")
}

// ---------------------------------------------------------------------------
// OAuthSession
// ---------------------------------------------------------------------------

pub struct OAuthSession {
    pub authorize_url: String,
    pub code_verifier: String,
    pub state: String,
}

/// Build a new PKCE session by reading 32 bytes from `/dev/urandom` twice.
/// No external randomness crate required.
pub fn new_oauth_session() -> std::io::Result<OAuthSession> {
    let mut buf = [0u8; 32];
    let mut f = std::fs::File::open("/dev/urandom")?;
    use std::io::Read;
    f.read_exact(&mut buf)?;
    let code_verifier = base64url(&buf);
    f.read_exact(&mut buf)?;
    let state = base64url(&buf);
    let challenge = code_challenge_for(&code_verifier);
    let authorize_url = build_authorize_url(&challenge, &state);
    Ok(OAuthSession { authorize_url, code_verifier, state })
}

// ---------------------------------------------------------------------------
// parse_authorization_code
// ---------------------------------------------------------------------------

pub struct ParsedCode {
    pub code: String,
    pub state: Option<String>,
}

/// Parse the authorization code from:
/// - a full callback URL (`?code=&state=`)
/// - a bare `code#state` string
/// - a bare code
///
/// Returns None for empty/whitespace input.
pub fn parse_authorization_code(input: &str) -> Option<ParsedCode> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Try URL parse: if it has a '?' it's a callback URL.
    if trimmed.contains('?') {
        // manual query-string parse (no url crate)
        let qs_start = trimmed.find('?')? + 1;
        let qs = &trimmed[qs_start..];
        let mut code: Option<String> = None;
        let mut state: Option<String> = None;
        for pair in qs.split('&') {
            if let Some(idx) = pair.find('=') {
                let key = &pair[..idx];
                let val = &pair[idx+1..];
                if key == "code" { code = Some(val.to_string()); }
                if key == "state" { state = Some(val.to_string()); }
            }
        }
        return code.map(|c| ParsedCode { code: c, state });
    }

    // bare "code#state"
    if trimmed.contains('#') {
        let mut parts = trimmed.splitn(2, '#');
        let code = parts.next()?.trim();
        let st = parts.next().map(|s| s.trim()).filter(|s| !s.is_empty()).map(String::from);
        if code.is_empty() { return None; }
        return Some(ParsedCode { code: code.to_string(), state: st });
    }

    // bare code
    Some(ParsedCode { code: trimmed.to_string(), state: None })
}

// ---------------------------------------------------------------------------
// exchange_code
// ---------------------------------------------------------------------------

/// POST the authorization code to the token endpoint and return the raw JSON.
pub fn exchange_code(code: &str, verifier: &str, state: &str) -> Option<serde_json::Value> {
    let body = serde_json::json!({
        "grant_type": "authorization_code",
        "client_id": OAUTH_CLIENT_ID,
        "code": code,
        "redirect_uri": OAUTH_REDIRECT_URI,
        "code_verifier": verifier,
        "state": state,
    }).to_string();
    let resp = token_refresh::oauth_token_request(&body)?;
    serde_json::from_str(&resp).ok()
}

// ---------------------------------------------------------------------------
// build_credentials_file
// ---------------------------------------------------------------------------

/// Map the token-endpoint JSON response to a `CredentialsFile` ready for
/// `create_platform_credential_store(...).write(...)`.
/// Returns None if `access_token` or `refresh_token` is missing.
pub fn build_credentials_file(token: &serde_json::Value, now_ms: i64) -> Option<CredentialsFile> {
    let access_token = token["access_token"].as_str()?.to_string();
    let refresh_token = token["refresh_token"].as_str()?.to_string();
    let expires_at = token["expires_at"].as_i64().unwrap_or_else(|| {
        token["expires_in"].as_i64()
            .map(|s| now_ms + s * 1000)
            .unwrap_or(now_ms + 8 * 60 * 60 * 1000)
    });
    let scopes: Option<Vec<String>> = token["scope"].as_str()
        .map(|s| s.split(' ').filter(|p| !p.is_empty()).map(String::from).collect())
        .or_else(|| Some(OAUTH_SCOPES.iter().map(|s| s.to_string()).collect()));
    Some(CredentialsFile {
        claude_ai_oauth: OAuthCredentials {
            access_token,
            refresh_token,
            expires_at,
            scopes,
            subscription_type: None,
            rate_limit_tier: None,
            extra: serde_json::Map::new(),
        },
        extra: serde_json::Map::new(),
    })
}

// ---------------------------------------------------------------------------
// auth_status_logged_in
// ---------------------------------------------------------------------------

/// Parse the JSON output of `claude auth status` and return `loggedIn`.
pub fn auth_status_logged_in(json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()
        .and_then(|v| v["loggedIn"].as_bool())
        .unwrap_or(false)
}
