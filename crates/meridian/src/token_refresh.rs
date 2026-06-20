//! OAuth credential store + token refresh.
//!
//! Port of `src-original/src/proxy/tokenRefresh.ts` (non-scheduler subset).
//!
//! Two backends:
//!   macOS  — system Keychain via /usr/bin/security (no prompt, pre-authorised)
//!   Linux  — <dir>/.credentials.json
//!
//! OAuth POST shells out to `curl` with the request body on stdin — the
//! refresh token is never placed in argv.
//!
//! Background scheduler (start/stop_background_refresh) is deferred to 3d-3b.
//! Single-flight here serialises concurrent callers per refresh_key with a
//! Mutex<()> rather than the TS shared-promise dedup; the distinction doesn't
//! matter while only /auth/refresh drives refreshes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const OAUTH_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const OAUTH_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

// ---------------------------------------------------------------------------
// Pure SHA-256  (FIPS-180-4, no deps)
// ---------------------------------------------------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
    0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
    0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
    0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
    0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Pure Rust SHA-256 producing a lowercase hex string. No dependencies.
/// Must return `ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad`
/// for input `b"abc"`.
pub fn sha256_hex(input: &[u8]) -> String {
    let mut h = [
        0x6a09e667_u32, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    let bit_len = (input.len() as u64) * 8;
    let mut msg: Vec<u8> = input.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0x00);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([block[i*4], block[i*4+1], block[i*4+2], block[i*4+3]]);
        }
        for i in 16..64 {
            let s0 = w[i-15].rotate_right(7) ^ w[i-15].rotate_right(18) ^ (w[i-15] >> 3);
            let s1 = w[i-2].rotate_right(17) ^ w[i-2].rotate_right(19) ^ (w[i-2] >> 10);
            w[i] = w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] =
            [h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]];
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let tmp1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let tmp2 = s0.wrapping_add(maj);
            hh = g; g = f; f = e;
            e = d.wrapping_add(tmp1);
            d = c; c = b; b = a;
            a = tmp1.wrapping_add(tmp2);
        }
        h[0] = h[0].wrapping_add(a); h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c); h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e); h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g); h[7] = h[7].wrapping_add(hh);
    }

    h.iter().map(|v| format!("{v:08x}")).collect()
}

// ---------------------------------------------------------------------------
// Credential types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OAuthCredentials {
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "refreshToken")]
    pub refresh_token: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: i64,
    #[serde(rename = "scopes", skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    #[serde(rename = "subscriptionType", skip_serializing_if = "Option::is_none")]
    pub subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier", skip_serializing_if = "Option::is_none")]
    pub rate_limit_tier: Option<String>,
    /// Preserve any unknown keys inside claudeAiOauth.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    pub claude_ai_oauth: OAuthCredentials,
    /// Preserve any unknown top-level keys.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Serialize credentials to compact JSON (no whitespace).
/// MUST be compact — Claude Code's parser treats pretty-printed JSON as
/// logged-out (issue #452).
pub fn serialize_credentials(c: &CredentialsFile) -> String {
    serde_json::to_string(c).unwrap()
}

// ---------------------------------------------------------------------------
// Config dir → keychain service / file path
// ---------------------------------------------------------------------------

fn default_claude_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
    PathBuf::from(home).join(".claude")
}

fn canonicalize_or_lexical(dir: &str) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir))
}

/// Map `claudeConfigDir` to the macOS Keychain service name Claude Code uses.
/// Default `~/.claude` → bare `"Claude Code-credentials"`.
/// Any other dir → `"Claude Code-credentials-<sha256(absPath)[..8]>"`.
pub fn config_dir_to_keychain_service(dir: &str) -> String {
    let abs = canonicalize_or_lexical(dir);
    let default = canonicalize_or_lexical(default_claude_dir().to_str().unwrap_or(""));
    if abs == default {
        return KEYCHAIN_SERVICE.to_string();
    }
    let abs_str = abs.to_string_lossy();
    format!("{}-{}", KEYCHAIN_SERVICE, &sha256_hex(abs_str.as_bytes())[..8])
}

/// Map `claudeConfigDir` to the file-based credentials path.
pub fn config_dir_to_credentials_file(dir: &str) -> PathBuf {
    PathBuf::from(dir).join(".credentials.json")
}

// ---------------------------------------------------------------------------
// CredentialStore trait
// ---------------------------------------------------------------------------

pub trait CredentialStore: Send + Sync {
    /// Stable key for single-flight refresh deduplication per store instance.
    fn refresh_key(&self) -> String;
    fn read(&self) -> Option<CredentialsFile>;
    fn write(&self, c: &CredentialsFile) -> bool;
}

// ---------------------------------------------------------------------------
// FileStore backend
// ---------------------------------------------------------------------------

pub struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub fn new(path: PathBuf) -> FileStore {
        FileStore { path }
    }
}

impl CredentialStore for FileStore {
    fn refresh_key(&self) -> String {
        format!("file:{}", self.path.display())
    }

    fn read(&self) -> Option<CredentialsFile> {
        if !self.path.exists() {
            return None;
        }
        match std::fs::read_to_string(&self.path) {
            Ok(raw) => match serde_json::from_str(&raw) {
                Ok(c) => Some(c),
                Err(e) => {
                    tracing::warn!("token_refresh.file_read_parse_failed path={} err={e}", self.path.display());
                    None
                }
            },
            Err(e) => {
                tracing::warn!("token_refresh.file_read_failed path={} err={e}", self.path.display());
                None
            }
        }
    }

    fn write(&self, c: &CredentialsFile) -> bool {
        if let Some(parent) = self.path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::warn!("token_refresh.file_write_mkdir_failed path={} err={e}", self.path.display());
                return false;
            }
        }
        match std::fs::write(&self.path, serialize_credentials(c)) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("token_refresh.file_write_failed path={} err={e}", self.path.display());
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// KeychainStore backend (macOS)
// ---------------------------------------------------------------------------

/// Per-service encoding flag: true = the value was read as hex-encoded JSON.
/// Track so we write back in the same encoding Claude Code expects.
static KEYCHAIN_WAS_HEX: std::sync::OnceLock<Mutex<HashMap<String, bool>>> = std::sync::OnceLock::new();

fn keychain_was_hex() -> &'static Mutex<HashMap<String, bool>> {
    KEYCHAIN_WAS_HEX.get_or_init(|| Mutex::new(HashMap::new()))
}

fn username() -> String {
    std::env::var("USER").unwrap_or_else(|_| "unknown".to_string())
}

fn parse_keychain_value(raw: &str) -> Option<(CredentialsFile, bool)> {
    let trimmed = raw.trim();
    // Try raw JSON first.
    if let Ok(c) = serde_json::from_str::<CredentialsFile>(trimmed) {
        return Some((c, false));
    }
    // Try hex-decoded JSON (Claude Code's format after `claude login`).
    let decoded = (0..trimmed.len())
        .step_by(2)
        .filter_map(|i| {
            if i + 1 < trimmed.len() {
                u8::from_str_radix(&trimmed[i..i+2], 16).ok()
            } else {
                None
            }
        })
        .collect::<Vec<u8>>();
    if decoded.is_empty() {
        return None;
    }
    match serde_json::from_slice::<CredentialsFile>(&decoded) {
        Ok(c) => Some((c, true)),
        Err(_) => None,
    }
}

pub struct KeychainStore {
    pub service: String,
}

impl CredentialStore for KeychainStore {
    fn refresh_key(&self) -> String {
        format!("keychain:{}", self.service)
    }

    fn read(&self) -> Option<CredentialsFile> {
        let out = std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", &self.service, "-a", &username(), "-w"])
            .output()
            .ok()?;
        if !out.status.success() {
            tracing::warn!("token_refresh.keychain_read_failed service={}", self.service);
            return None;
        }
        let raw = String::from_utf8_lossy(&out.stdout);
        match parse_keychain_value(&raw) {
            Some((creds, was_hex)) => {
                keychain_was_hex().lock().unwrap().insert(self.service.clone(), was_hex);
                Some(creds)
            }
            None => {
                tracing::warn!("token_refresh.keychain_parse_failed service={}", self.service);
                None
            }
        }
    }

    fn write(&self, c: &CredentialsFile) -> bool {
        let json = serialize_credentials(c);
        let was_hex = *keychain_was_hex().lock().unwrap().get(&self.service).unwrap_or(&false);
        let value = if was_hex {
            json.bytes().map(|b| format!("{b:02x}")).collect::<String>()
        } else {
            json
        };
        let out = std::process::Command::new("/usr/bin/security")
            .args([
                "add-generic-password", "-U",
                "-s", &self.service,
                "-a", &username(),
                "-w", &value,
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => true,
            Ok(_) => {
                tracing::warn!("token_refresh.keychain_write_failed service={}", self.service);
                false
            }
            Err(e) => {
                tracing::warn!("token_refresh.keychain_write_failed service={} err={e}", self.service);
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Platform store factory
// ---------------------------------------------------------------------------

/// Returns the appropriate credential store for the current platform.
/// macOS → KeychainStore; other → FileStore.
/// `None` means default `~/.claude`.
pub fn create_platform_credential_store(claude_config_dir: Option<&str>) -> Box<dyn CredentialStore> {
    create_platform_credential_store_impl(claude_config_dir)
}

#[cfg(target_os = "macos")]
fn create_platform_credential_store_impl(claude_config_dir: Option<&str>) -> Box<dyn CredentialStore> {
    let service = match claude_config_dir {
        Some(dir) => config_dir_to_keychain_service(dir),
        None => KEYCHAIN_SERVICE.to_string(),
    };
    Box::new(KeychainStore { service })
}

#[cfg(not(target_os = "macos"))]
fn create_platform_credential_store_impl(claude_config_dir: Option<&str>) -> Box<dyn CredentialStore> {
    let path = match claude_config_dir {
        Some(dir) => config_dir_to_credentials_file(dir),
        None => {
            let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
            PathBuf::from(home).join(".claude").join(".credentials.json")
        }
    };
    Box::new(FileStore { path })
}

// ---------------------------------------------------------------------------
// now_ms helper
// ---------------------------------------------------------------------------

/// Current time as milliseconds since UNIX epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Single-flight per refresh_key
// ---------------------------------------------------------------------------

static INFLIGHT: std::sync::OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = std::sync::OnceLock::new();

fn inflight() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    INFLIGHT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn per_key_lock(key: &str) -> Arc<Mutex<()>> {
    let mut map = inflight().lock().unwrap();
    map.entry(key.to_string()).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
}

// ---------------------------------------------------------------------------
// curl OAuth POST helper
// ---------------------------------------------------------------------------

fn oauth_post(body: &str) -> Option<String> {
    use std::io::Write;
    let mut child = std::process::Command::new("curl")
        .args([
            "--silent", "--show-error", "--max-time", "15",
            "-H", "Content-Type: application/json",
            "--data", "@-",
            OAUTH_TOKEN_URL,
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(body.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        tracing::warn!("token_refresh.curl_failed");
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// refresh_oauth_token
// ---------------------------------------------------------------------------

/// Refresh the Claude Code OAuth access token.
///
/// Reads the stored refresh token, exchanges it at Anthropic's OAuth endpoint
/// via curl (body on stdin), and writes the updated credentials back.
///
/// Concurrent calls for the same `refresh_key` are serialised with a per-key
/// Mutex so only one network round-trip fires at a time.
pub fn refresh_oauth_token(store: &dyn CredentialStore) -> bool {
    let key = store.refresh_key();
    let lock = per_key_lock(&key);
    let _guard = lock.lock().unwrap();

    let creds = match store.read() {
        Some(c) => c,
        None => {
            tracing::warn!("token_refresh.no_credentials");
            return false;
        }
    };

    let refresh_token = &creds.claude_ai_oauth.refresh_token;
    if refresh_token.is_empty() {
        tracing::warn!("token_refresh.no_refresh_token");
        return false;
    }

    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": OAUTH_CLIENT_ID,
        "refresh_token": refresh_token,
    }).to_string();

    let resp_str = match oauth_post(&body) {
        Some(s) => s,
        None => return false,
    };

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
        refresh_token: Option<String>,
        expires_in: Option<i64>,
        expires_at: Option<i64>,
    }

    let token_data: TokenResponse = match serde_json::from_str(&resp_str) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("token_refresh.parse_failed err={e}");
            return false;
        }
    };

    let now = now_ms();
    let expires_at = token_data.expires_at
        .unwrap_or_else(|| token_data.expires_in
            .map(|s| now + s * 1000)
            .unwrap_or(now + 8 * 60 * 60 * 1000));

    let mut updated = creds;
    updated.claude_ai_oauth.access_token = token_data.access_token;
    if let Some(rt) = token_data.refresh_token {
        updated.claude_ai_oauth.refresh_token = rt;
    }
    updated.claude_ai_oauth.expires_at = expires_at;

    if !store.write(&updated) {
        return false;
    }

    tracing::info!("token_refresh.success expires_at={expires_at}");
    true
}

// ---------------------------------------------------------------------------
// ensure_fresh_token
// ---------------------------------------------------------------------------

/// Returns true when the stored token is still valid (expires_at - now > buffer_ms),
/// or when a refresh succeeds. False on any failure (no credentials, missing
/// expiresAt, refresh failed).
pub fn ensure_fresh_token(store: &dyn CredentialStore, buffer_ms: i64) -> bool {
    let creds = match store.read() {
        Some(c) => c,
        None => return false,
    };
    let expires_at = creds.claude_ai_oauth.expires_at;
    if expires_at == 0 {
        return false;
    }
    if expires_at - now_ms() > buffer_ms {
        return true;
    }
    refresh_oauth_token(store)
}
