# Phase 3d-3 — OAuth Credential Store + Token Refresh Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Port the credential store + OAuth token refresh from `src-original/src/proxy/tokenRefresh.ts`, and expose a manual `POST /auth/refresh` route. This unblocks Phase 3d-4 (browser login writes credentials through this store) and lets operators refresh an `oauth-token`/profile credential without restarting.

**Architecture:** A `token_refresh` module with a `CredentialStore` trait and two backends — macOS Keychain (via `/usr/bin/security`) and a `.credentials.json` file (Linux + any non-default profile dir). `refresh_oauth_token` reads the refresh token, exchanges it at Anthropic's OAuth endpoint **by shelling out to `curl`** (body on stdin — no new HTTPS crate, no token in argv), and writes the new tokens back in the encoding Claude Code expects. `POST /auth/refresh` resolves the request's profile, builds its store, refreshes, and clears the rate-limit snapshot on success.

**Tech Stack:** Rust, axum, serde/serde_json, `std::process::Command` (curl + security), tokio.

## Global Constraints

- **1:1 port** of `tokenRefresh.ts` for the in-scope functions. Match the OAuth constants and the on-disk format EXACTLY:
  - `OAUTH_TOKEN_URL = "https://platform.claude.com/v1/oauth/token"`, `OAUTH_CLIENT_ID = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"`.
  - Keychain service base: `"Claude Code-credentials"`. Default dir `~/.claude` → bare name; any other dir → `"Claude Code-credentials-<sha256(abs_path) hex [..8]>"`.
  - File path for a profile dir: `<abs(dir)>/.credentials.json`; default file: `~/.claude/.credentials.json`.
  - Credentials JSON shape: `{ "claudeAiOauth": { "accessToken", "refreshToken", "expiresAt"(epoch ms), "scopes"?, "subscriptionType"?, "rateLimitTier"? }, ...unknown keys preserved }`.
  - **`serialize_credentials` MUST be compact** (no whitespace) — Claude Code's parser treats pretty-printed JSON as logged-out (issue #452). Pin with a test.
  - Keychain value may be raw JSON or hex-encoded JSON (Claude Code writes hex after `claude login`). Detect on read, preserve the same encoding on write (per-service).
- **No new crates.** The OAuth POST uses `curl --silent --show-error --max-time 15 -H 'Content-Type: application/json' --data @- <url>` with the JSON body written to curl's **stdin** (never argv — the refresh token is secret). SHA-256 for the keychain service hash: there is no crate; compute it by shelling to a hash tool is brittle — instead implement a tiny pure-Rust SHA-256 (provided in Task 1) so the keychain service name matches Claude Code's. (~50 lines, no deps.)
- **DEFERRED to a later micro-slice (3d-3b), do NOT build here:** the background refresh scheduler (`start/stop_background_refresh`, the self-rescheduling generation-counter timer), the per-request proactive `ensure_fresh_token` wiring into the turn/stream loops, and the reactive refresh-on-401 path. The `claude` CLI refreshes its own credentials on the claude-max path; this slice delivers the store + a manual `/auth/refresh`. `ensure_fresh_token` itself IS implemented here (pure logic, tested) but not yet wired into the hot path.
- **Secrets:** never log accessToken/refreshToken; never pass the refresh token or keychain value as a process argument where avoidable (curl body via stdin). The macOS `security add-generic-password -w <value>` passes the value in argv — this is a faithful 1:1 port of Claude Code's own behavior and the write is local; keep it.
- **No Claude attribution** on commits. Match the dense hand style; do NOT run `cargo fmt`. Gate: `cargo build` + `cargo test` (default suite) + `cargo clippy --workspace --all-targets -- -D warnings`, all green. Skip `#[ignore]` tests (the live OAuth round-trip can't run without real credentials — mark any such test `#[ignore]`).

---

## File Structure

- **Create** `crates/meridian/src/token_refresh.rs` — types, `serialize_credentials`, `config_dir_to_keychain_service`, `config_dir_to_credentials_file`, `sha256_hex` (pure), `CredentialStore` trait, `FileStore`, `KeychainStore`, `create_platform_credential_store`, `refresh_oauth_token` (single-flight), `ensure_fresh_token`.
- **Modify** `crates/meridian/src/lib.rs` — `pub mod token_refresh;`.
- **Modify** `crates/meridian/src/server.rs` — `POST /auth/refresh` handler + route (protected); resolve profile → store → refresh → clear rate-limit on success.
- **Test** `crates/meridian/tests/token_refresh_test.rs`, `crates/meridian/tests/auth_refresh_route_test.rs`.

---

## Task 1: `token_refresh` module

**Files:**
- Create: `crates/meridian/src/token_refresh.rs`; Modify: `crates/meridian/src/lib.rs`
- Test: `crates/meridian/tests/token_refresh_test.rs`

**Interfaces (Produces):**
- `pub struct OAuthCredentials { access_token, refresh_token, expires_at: i64, scopes: Option<Vec<String>>, .. }` with serde renames (`accessToken`/`refreshToken`/`expiresAt`/`scopes`) and `#[serde(flatten)] extra: Map` on the file wrapper to preserve unknown keys.
- `pub struct CredentialsFile { claude_ai_oauth: OAuthCredentials (rename "claudeAiOauth"), #[serde(flatten)] extra: serde_json::Map<String,Value> }`
- `pub fn serialize_credentials(c: &CredentialsFile) -> String` (compact)
- `pub fn sha256_hex(input: &[u8]) -> String` (pure SHA-256 → lowercase hex)
- `pub fn config_dir_to_keychain_service(dir: &str) -> String`
- `pub fn config_dir_to_credentials_file(dir: &str) -> PathBuf`
- `pub trait CredentialStore: Send + Sync { fn refresh_key(&self) -> String; fn read(&self) -> Option<CredentialsFile>; fn write(&self, c: &CredentialsFile) -> bool; }`
- `pub struct FileStore { path: PathBuf }`, `pub struct KeychainStore { service: String }`
- `pub fn create_platform_credential_store(claude_config_dir: Option<&str>) -> Box<dyn CredentialStore>`
- `pub fn refresh_oauth_token(store: &dyn CredentialStore) -> bool` (single-flight per refresh_key)
- `pub fn ensure_fresh_token(store: &dyn CredentialStore, buffer_ms: i64) -> bool`

**Notes:** `read`/`write`/`refresh` are synchronous (shell out with `std::process::Command`); call them from handlers via `tokio::task::spawn_blocking`. `now_ms()` via `SystemTime`. Single-flight: a module-global `Mutex<HashMap<String, Arc<std::sync::Mutex<()>>>>` keyed by `refresh_key`; `refresh_oauth_token` holds the per-key lock across the read→curl→write so concurrent refreshes for the same store serialize (prevents the double-network/write race; not the shared-result dedup of the TS, which is unnecessary while only `/auth/refresh` calls it — note this in a comment).

- [ ] **Step 1: Write the failing tests**

```rust
// crates/meridian/tests/token_refresh_test.rs
use meridian::token_refresh::*;

#[test]
fn serialize_is_compact() {
    let c: CredentialsFile = serde_json::from_str(
        r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":111}}"#).unwrap();
    let s = serialize_credentials(&c);
    assert!(!s.contains('\n') && !s.contains("  "), "must be compact (issue #452): {s}");
    assert!(s.contains("\"claudeAiOauth\"") && s.contains("\"accessToken\":\"a\""));
}

#[test]
fn unknown_keys_preserved_on_roundtrip() {
    let c: CredentialsFile = serde_json::from_str(
        r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r","expiresAt":1},"other":42}"#).unwrap();
    assert!(serialize_credentials(&c).contains("\"other\":42"));
}

#[test]
fn sha256_known_vector() {
    // echo -n abc | sha256sum
    assert_eq!(sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
}

#[test]
fn keychain_service_default_vs_profile() {
    let home = std::env::var("HOME").unwrap();
    let default_dir = format!("{home}/.claude");
    assert_eq!(config_dir_to_keychain_service(&default_dir), "Claude Code-credentials");
    let other = config_dir_to_keychain_service("/some/profile/dir");
    assert!(other.starts_with("Claude Code-credentials-") && other.len() == "Claude Code-credentials-".len() + 8);
}

#[test]
fn credentials_file_path_for_profile() {
    assert_eq!(config_dir_to_credentials_file("/p/dir"),
        std::path::PathBuf::from("/p/dir/.credentials.json"));
}

#[test]
fn file_store_roundtrip_and_ensure_fresh() {
    let dir = std::env::temp_dir().join(format!("mer-tok-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = create_platform_credential_store(Some(dir.to_str().unwrap()));
    // create_platform_credential_store on macOS returns a KeychainStore for a
    // profile dir — so test the FileStore directly instead for determinism:
    let path = dir.join(".credentials.json");
    let fs = file_store_for_test(&path);
    let future = (now_ms_for_test() + 10 * 60 * 1000) as i64;
    let creds: CredentialsFile = serde_json::from_str(&format!(
        r#"{{"claudeAiOauth":{{"accessToken":"a","refreshToken":"r","expiresAt":{future}}}}}"#)).unwrap();
    assert!(fs.write(&creds));
    let back = fs.read().unwrap();
    assert_eq!(back.claude_ai_oauth.access_token, "a");
    // ensure_fresh_token: token valid for >buffer -> true WITHOUT any network.
    assert!(ensure_fresh_token(fs.as_ref(), 5 * 60 * 1000));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = store; // silence unused on non-macos
}
```

> The test references two test-only helpers — `file_store_for_test(&Path) -> Box<dyn CredentialStore>` and `now_ms_for_test() -> i64`. Export these (or make `FileStore::new` + a `now_ms` pub) so the test is deterministic regardless of platform (we must not depend on the macOS keychain in unit tests). Implementer: expose `pub fn FileStore::new(path: PathBuf) -> FileStore` and a `pub fn now_ms() -> i64`, and rewrite the test's two helper calls to use them.

- [ ] **Step 2: Run to verify failure** — `cargo test -p meridian --test token_refresh_test`. Expected: FAIL (module absent).

- [ ] **Step 3: Implement** `crates/meridian/src/token_refresh.rs`. Key pieces:

- Pure SHA-256 (`sha256_hex`) — standard FIPS-180 implementation over `&[u8]`, no deps.
- Types with serde renames + `#[serde(flatten)] extra` to preserve unknown keys; `serialize_credentials` = `serde_json::to_string(c).unwrap()` (compact).
- `config_dir_to_keychain_service`: `let abs = std::fs::canonicalize(dir).unwrap_or_else(|_| PathBuf::from(dir));` compare against canonical `~/.claude`; if equal → base name; else `format!("Claude Code-credentials-{}", &sha256_hex(abs_str.as_bytes())[..8])`. (Match TS `resolve()` semantics; if canonicalize fails because the dir doesn't exist, fall back to a lexical absolutize — document the minor divergence.)
- `FileStore`: read = parse file (None on missing/invalid, warn); write = `create_dir_all(parent)` + write compact (NOT pretty), return bool.
- `KeychainStore` (macOS): read via `Command::new("/usr/bin/security").args(["find-generic-password","-s",&service,"-a",&username,"-w"])`, parse raw-or-hex (track per-service hex flag in a module `Mutex<HashMap<String,bool>>`); write via `add-generic-password -U -s <service> -a <user> -w <value>` re-encoding to hex if read-as-hex. `username` from `std::env::var("USER")` (fallback to `whoami`-free: `users` crate not allowed → use `$USER`).
- `create_platform_credential_store(Some(dir))`: macOS → `KeychainStore{ service: config_dir_to_keychain_service(dir) }`; else → `FileStore{ path: config_dir_to_credentials_file(dir) }`. `None` → default `~/.claude` keychain (macOS) / `~/.claude/.credentials.json`.
- `refresh_oauth_token(store)`: single-flight per `store.refresh_key()`; read creds → POST via curl (body `{"grant_type":"refresh_token","client_id":OAUTH_CLIENT_ID,"refresh_token":<rt>}` on stdin) → parse `{access_token, refresh_token?, expires_in?, expires_at?}` → `expires_at = expires_at ?? (now + expires_in*1000) ?? (now + 8h)` → update creds (keep old refresh_token if response omits it) → `store.write`. Return false on any failure (log without secrets).
- `ensure_fresh_token(store, buffer_ms)`: read; `expires_at` missing → false; `expires_at - now > buffer_ms` → true; else `refresh_oauth_token(store)`.

curl call helper:
```rust
fn oauth_post(body: &str) -> Option<String> {
    use std::io::Write;
    let mut child = std::process::Command::new("curl")
        .args(["--silent","--show-error","--max-time","15","-H","Content-Type: application/json","--data","@-", OAUTH_TOKEN_URL])
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped())
        .spawn().ok()?;
    child.stdin.take()?.write_all(body.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() { tracing::warn!("oauth refresh curl failed"); return None; }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}
```

Add `pub mod token_refresh;` to lib.rs.

- [ ] **Step 4: Run to verify pass** — `cargo test -p meridian --test token_refresh_test` green; `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] **Step 5: Commit**

```bash
git add crates/meridian/src/token_refresh.rs crates/meridian/src/lib.rs crates/meridian/tests/token_refresh_test.rs
git commit -m "feat(meridian): OAuth credential store + token refresh (keychain/file, curl, single-flight)"
```

---

## Task 2: `POST /auth/refresh` route

**Files:**
- Modify: `crates/meridian/src/server.rs`
- Test: `crates/meridian/tests/auth_refresh_route_test.rs`

**Interfaces:**
- Consumes: `token_refresh::{create_platform_credential_store, refresh_oauth_token}`, `ProfileStore::resolve_id` + the resolved profile's `claude_config_dir`, `RateLimitStore::clear` (already on AppState).
- Route `POST /auth/refresh` (on the protected router):
  - Resolve profile from `x-meridian-profile` header (else active/first/default) → its `claude_config_dir` (None for api/default).
  - Build the store via `create_platform_credential_store(dir.as_deref())`, run `refresh_oauth_token` on `spawn_blocking`.
  - Success → `state.rate_limit.clear()`, `200 {"success":true,"message":"OAuth token refreshed successfully","profile":"<id>"}`.
  - Failure → `500 {"success":false,"message":"Token refresh failed. If the problem persists, run 'claude login'."}`.
  - For an `api`/`oauth-token` profile that has no credential store target (no `claude_config_dir` and not the default account): `oauth-token` profiles supply the token via env and have no on-disk creds to refresh → treat as failure (`success:false`) like the TS `store ? ... : false` when no store applies. Build the store only for claude-max/default (a `claude_config_dir` or the default account); for api/oauth-token profiles, return the 500 failure body without attempting a refresh.

- [ ] **Step 1: Write the failing test**

```rust
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
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String, _pr: Option<String>)
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
```

> These tests avoid the network: an empty/nonexistent creds dir makes `refresh_oauth_token` return false at the "no credentials" step before any curl call. They are NOT `#[ignore]` — they exercise the route + the no-creds failure path deterministically.

- [ ] **Step 2: Run to verify failure** — route 404 / handler missing.

- [ ] **Step 3: Implement** the handler + route in `server.rs` (register on the protected router, near the other routes):

```rust
async fn auth_refresh<R: TurnRunner + StreamRunner + 'static>(
    State(state): State<AppState<R>>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    let requested = headers.get("x-meridian-profile").and_then(|v| v.to_str().ok());
    let id = state.profiles.resolve_id(requested);
    // Only claude-max / default profiles have an on-disk credential store to
    // refresh; api + oauth-token profiles carry their auth via env.
    let kind = state.profiles.resolved_type(&id);
    let dir = state.profiles.config_dir_for(&id); // add this getter on ProfileStore
    let refreshable = matches!(kind, crate::profiles::ProfileType::ClaudeMax);
    let ok = if refreshable {
        tokio::task::spawn_blocking(move || {
            let store = crate::token_refresh::create_platform_credential_store(dir.as_deref());
            crate::token_refresh::refresh_oauth_token(store.as_ref())
        }).await.unwrap_or(false)
    } else { false };
    if ok {
        state.rate_limit.clear();
        axum::Json(serde_json::json!({"success":true,"message":"OAuth token refreshed successfully","profile":id})).into_response()
    } else {
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR,
         axum::Json(serde_json::json!({"success":false,"message":"Token refresh failed. If the problem persists, run 'claude login'."}))).into_response()
    }
}
```

Add a `pub fn config_dir_for(&self, id: &str) -> Option<String>` to `ProfileStore` (returns the effective profile's `claude_config_dir`). Register `.route("/auth/refresh", post(auth_refresh::<R>))` on the protected router.

- [ ] **Step 4: Run to verify pass** — `cargo test -p meridian --test auth_refresh_route_test`, full `cargo test`, clippy clean.
- [ ] **Step 5: Commit**

```bash
git add crates/meridian/src/server.rs crates/meridian/src/profiles.rs crates/meridian/tests/auth_refresh_route_test.rs
git commit -m "feat(meridian): POST /auth/refresh (per-profile credential store refresh)"
```

---

## Self-Review

1. **Coverage:** credential store keychain+file (T1) ✓; compact serialize + unknown-key preservation (T1) ✓; config→service/file mappers + sha256 (T1) ✓; refresh via curl + single-flight (T1) ✓; ensure_fresh_token logic (T1) ✓; /auth/refresh route + clear-on-success (T2) ✓. Background scheduler + per-request/401 wiring explicitly deferred (Global Constraints → 3d-3b).
2. **Placeholders:** none. The live OAuth round-trip is untestable without real creds — covered structurally; the no-creds failure path IS tested deterministically.
3. **Type consistency:** `CredentialStore` trait methods used identically in T1 tests and the T2 handler; `create_platform_credential_store(Option<&str>)` signature matches both; `config_dir_for` is the new getter T2 needs.
4. **Risk notes for executor:** (a) the pure SHA-256 must match a known vector (test pins `sha256("abc")`) — the keychain service name is wrong otherwise. (b) curl must receive the body on stdin, never argv. (c) unit tests MUST use the file backend (not the real keychain) for determinism. (d) `spawn_blocking` for the sync store/curl calls so the async runtime isn't blocked.
