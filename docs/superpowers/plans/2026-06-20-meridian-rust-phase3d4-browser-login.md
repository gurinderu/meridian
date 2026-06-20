# Phase 3d-4 — Browser OAuth Login (`meridian profile add/login`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Complete the `meridian profile` CLI with browser-based account onboarding — `profile add <name>` (and `profile login <name>`), with an interactive `claude auth login` path and a `--headless` manual-OAuth (PKCE) path that writes credentials through the Phase 3d-3 store. This is the last piece of the Rust port vs the TS original.

**Architecture:** A new `oauth` module holds the pure, testable OAuth primitives — base64url, SHA-256 bytes, a PKCE/`OAuthSession` builder (authorize URL + verifier + state), and `parse_authorization_code`. The interactive flows live in the CLI: non-headless spawns `claude auth login` (which does the whole OAuth itself) into the profile's config dir; `--headless` prints the authorize URL, reads the pasted code from stdin, exchanges it via `curl` (reusing the Phase 3d-3 token-request helper), and writes the credentials with `create_platform_credential_store`. The profile is then persisted to `profiles.json` and its auth verified with `claude auth status`.

**Tech Stack:** Rust, `std::process::Command` (claude, curl), `std::fs` (`/dev/urandom`), serde_json, the Phase 3d-3 `token_refresh` module.

## Global Constraints

- **1:1 port** of the OAuth pieces of `src-original/src/proxy/profileCli.ts`. Exact constants:
  - `OAUTH_AUTHORIZE_URL = "https://claude.com/cai/oauth/authorize"`
  - `OAUTH_TOKEN_URL = "https://platform.claude.com/v1/oauth/token"` (already in `token_refresh.rs`)
  - `OAUTH_CLIENT_ID = "9d1c250a-e61b-44d9-88ed-5944d1962f5e"`
  - `OAUTH_REDIRECT_URI = "https://platform.claude.com/oauth/code/callback"`
  - `OAUTH_SCOPES = ["org:create_api_key","user:profile","user:inference","user:sessions:claude_code","user:mcp_servers","user:file_upload"]`
- **Authorize URL query (in this order):** `code=true`, `client_id`, `response_type=code`, `redirect_uri`, `scope` (space-joined), `code_challenge`, `code_challenge_method=S256`, `state`. URL-encode values.
- **PKCE:** `code_verifier = base64url(32 random bytes)`; `code_challenge = base64url(sha256(code_verifier_ascii))`; `state = base64url(32 random bytes)`. base64url = standard base64 with `-`/`_` and **no padding**.
- **Token exchange body** (authorization_code grant): `{"grant_type":"authorization_code","client_id":<id>,"code":<code>,"redirect_uri":<redirect>,"code_verifier":<verifier>,"state":<state>}`. POST to `OAUTH_TOKEN_URL` via `curl` with the body on **stdin** (never argv — `code`/`code_verifier` are secrets).
- **Credentials written:** `{"claudeAiOauth":{"accessToken":<access>,"refreshToken":<refresh>,"expiresAt":<expires_at or expires_in→ms or now+8h>,"scopes":<scope.split or OAUTH_SCOPES>}}` via `create_platform_credential_store(Some(profile_config_dir))` (Phase 3d-3) — which already writes 0o600 / keychain.
- **No new crates.** Randomness from `/dev/urandom` (read 32 bytes); base64url + sha256 hand-rolled (sha256 already exists in `token_refresh`).
- **Randomness/IO note:** the actual browser round-trip, the `claude auth login` spawn, and the token exchange are NOT unit-testable (interactive + network + real account). Test the PURE primitives exhaustively (base64url, PKCE challenge against the RFC 7636 vector, authorize-URL assembly, code parsing) and the deterministic CLI guards (invalid id, duplicate, unknown-profile-on-login). Mark nothing `#[ignore]` that can run; do not write a test that requires a browser.
- **No Claude attribution** on commits. Dense hand style; do NOT run `cargo fmt`. Gate: build + `cargo test` (default) + `cargo clippy --workspace --all-targets -- -D warnings`, all green.

---

## File Structure

- **Create** `crates/meridian/src/oauth.rs` — `base64url`, `sha256` re-export, `code_challenge_for`, `OAuthSession { authorize_url, code_verifier, state }`, `new_oauth_session()` (reads `/dev/urandom`), `parse_authorization_code(&str) -> Option<ParsedCode{code, state}>`, `exchange_code(...) -> Option<TokenResponse>`, `build_credentials_file(...)`.
- **Modify** `crates/meridian/src/token_refresh.rs` — make the curl helper reusable: `pub fn oauth_token_request(body: &str) -> Option<String>`. Add `pub fn sha256_bytes(input: &[u8]) -> [u8; 32]` and make `sha256_hex` call it.
- **Modify** `crates/meridian/src/lib.rs` — `pub mod oauth;`.
- **Modify** `bin/meridian-cli/src/main.rs` — implement `profile add <name> [--headless]` (replace the current "not supported" stub for the no-`--oauth-token` path) and add `profile login <name> [--headless]`.
- **Test** `crates/meridian/tests/oauth_test.rs`.

---

## Task 1: `oauth` module (pure primitives + exchange)

**Files:** Create `crates/meridian/src/oauth.rs`; Modify `crates/meridian/src/token_refresh.rs`, `crates/meridian/src/lib.rs`; Test `crates/meridian/tests/oauth_test.rs`.

**Interfaces (Produces):**
- `pub fn base64url(bytes: &[u8]) -> String` (no padding, `-`/`_`)
- `pub fn code_challenge_for(verifier: &str) -> String` (= `base64url(sha256_bytes(verifier.as_bytes()))`)
- `pub struct OAuthSession { pub authorize_url: String, pub code_verifier: String, pub state: String }`
- `pub fn build_authorize_url(code_challenge: &str, state: &str) -> String`
- `pub fn new_oauth_session() -> std::io::Result<OAuthSession>` (32 bytes from `/dev/urandom` each for verifier+state)
- `pub struct ParsedCode { pub code: String, pub state: Option<String> }`
- `pub fn parse_authorization_code(input: &str) -> Option<ParsedCode>`
- `pub fn exchange_code(code: &str, verifier: &str, state: &str) -> Option<serde_json::Value>` (curl POST → parsed JSON)
- `pub fn build_credentials_file(token: &serde_json::Value, now_ms: i64) -> Option<token_refresh::CredentialsFile>` (maps access_token/refresh_token/expires_* → CredentialsFile; None if access/refresh missing)

In `token_refresh.rs`: `pub fn sha256_bytes(input: &[u8]) -> [u8;32]` (refactor `sha256_hex` to format its output) and `pub fn oauth_token_request(body: &str) -> Option<String>` (the existing curl helper, made public).

- [ ] **Step 1: Write the failing tests**

```rust
// crates/meridian/tests/oauth_test.rs
use meridian::oauth::*;

#[test]
fn base64url_no_padding_and_urlsafe() {
    assert_eq!(base64url(b"abc"), "YWJj");          // no padding needed
    assert_eq!(base64url(b"ab"), "YWI");            // padding stripped
    assert_eq!(base64url(&[0xfb, 0xff]), "-_8");    // - and _ (not + /)
}

#[test]
fn pkce_challenge_matches_rfc7636_vector() {
    // RFC 7636 Appendix B
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    assert_eq!(code_challenge_for(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
}

#[test]
fn authorize_url_has_required_params() {
    let u = build_authorize_url("CHAL", "STATE");
    assert!(u.starts_with("https://claude.com/cai/oauth/authorize?"));
    for needle in ["code=true","client_id=9d1c250a","response_type=code",
                   "code_challenge=CHAL","code_challenge_method=S256","state=STATE"] {
        assert!(u.contains(needle), "missing {needle} in {u}");
    }
    assert!(u.contains("redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback"));
    assert!(u.contains("scope=org%3Acreate_api_key")); // space-joined, url-encoded
}

#[test]
fn parse_code_from_raw_url_and_bare() {
    // pasted callback URL
    let p = parse_authorization_code("https://x/callback?code=AUTHCODE&state=ST").unwrap();
    assert_eq!(p.code, "AUTHCODE");
    assert_eq!(p.state.as_deref(), Some("ST"));
    // bare "code#state" form
    let p2 = parse_authorization_code("RAWCODE#ST2").unwrap();
    assert_eq!(p2.code, "RAWCODE");
    assert_eq!(p2.state.as_deref(), Some("ST2"));
    // bare code only
    let p3 = parse_authorization_code("JUSTCODE").unwrap();
    assert_eq!(p3.code, "JUSTCODE");
    // empty -> None
    assert!(parse_authorization_code("   ").is_none());
}

#[test]
fn new_session_is_well_formed() {
    let s = new_oauth_session().unwrap();
    assert!(!s.code_verifier.is_empty() && !s.state.is_empty());
    assert!(s.authorize_url.contains(&format!("state={}", s.state)));
    // challenge in the URL is derived from the verifier
    assert!(s.authorize_url.contains(&format!("code_challenge={}", code_challenge_for(&s.code_verifier))));
}

#[test]
fn build_credentials_maps_tokens() {
    let tok = serde_json::json!({"access_token":"AC","refresh_token":"RF","expires_in":3600,"scope":"a b"});
    let c = build_credentials_file(&tok, 1_000_000).unwrap();
    assert_eq!(c.claude_ai_oauth.access_token, "AC");
    assert_eq!(c.claude_ai_oauth.refresh_token, "RF");
    assert_eq!(c.claude_ai_oauth.expires_at, 1_000_000 + 3600*1000);
    // missing access/refresh -> None
    assert!(build_credentials_file(&serde_json::json!({"access_token":"x"}), 0).is_none());
}
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p meridian --test oauth_test`. Expected: FAIL (module absent).

- [ ] **Step 3: Implement.** `base64url`: standard alphabet, map +→-, /→_, drop `=`. `code_challenge_for`: `base64url(&token_refresh::sha256_bytes(verifier.as_bytes()))`. `build_authorize_url`: assemble with percent-encoding (hand-roll a small `percent_encode` for the unreserved set, or encode the known-fixed redirect/scope literals — simplest: a tiny encoder that escapes everything outside `A-Za-z0-9-_.~`). `new_oauth_session`: read 32 bytes twice from `/dev/urandom`, base64url them, derive challenge, build URL. `parse_authorization_code`: try `url`-less manual parse — if it contains `?`/`&`, split query for `code`/`state`; else split on `#` for `code#state`; else the whole trimmed string is the code (empty → None). `exchange_code`: build the JSON body, `token_refresh::oauth_token_request(&body)`, parse JSON. `build_credentials_file`: pull access_token/refresh_token (None if either missing), `expires_at = expires_at ?? now + expires_in*1000 ?? now + 8h`, scopes from `scope.split(' ')` else the default list.

`token_refresh.rs`: refactor `sha256_hex` to `sha256_bytes(input) -> [u8;32]` + a hex wrapper; make the curl helper `pub fn oauth_token_request`.

Add `pub mod oauth;` to lib.rs.

- [ ] **Step 4: Run to verify pass** — `cargo test -p meridian --test oauth_test` + existing `token_refresh_test` (the sha256 refactor must keep `sha256("abc")` correct) green; clippy clean.
- [ ] **Step 5: Commit**

```bash
git add crates/meridian/src/oauth.rs crates/meridian/src/token_refresh.rs crates/meridian/src/lib.rs crates/meridian/tests/oauth_test.rs
git commit -m "feat(meridian): OAuth login primitives (PKCE, authorize URL, code parse, exchange)"
```

---

## Task 2: `profile add` / `profile login` CLI flows

**Files:** Modify `bin/meridian-cli/src/main.rs`; (reuse `crates/meridian/src/profile_cli.rs` helpers). Test: extend `crates/meridian/tests/profile_cli_test.rs` for any new pure helper.

**Interfaces:** Consumes `oauth::{new_oauth_session, parse_authorization_code, exchange_code, build_credentials_file}`, `token_refresh::create_platform_credential_store`, and the existing `profile_cli` helpers (`is_valid_profile_id`, `load_profiles_json_at`, `save_profiles_json_at`, `profiles_dir`).

**Behavior (port of `profileAdd` / `profileLogin`, plain output, no ANSI required):**
- `meridian profile add <name>` (no `--oauth-token`):
  1. Validate id (`is_valid_profile_id`) → error+exit 1 if bad; reject duplicate.
  2. **Headless** (`--headless`): `let s = new_oauth_session()?`; print the authorize URL + instructions; read a line from stdin (the pasted code); `parse_authorization_code` → validate `state` matches (if present); `exchange_code` → `build_credentials_file` → `create_platform_credential_store(Some(profile_dir)).write(&creds)`. On any failure print an error + exit 1.
  3. **Non-headless** (default): mkdir `<profiles_dir>/<name>`; spawn `claude auth login` with `CLAUDE_CONFIG_DIR=<dir>` and inherited stdio (`Command::status()`); if exit≠0 → error+exit 1.
  4. Verify with `claude auth status` (`CLAUDE_CONFIG_DIR=<dir>`, JSON `{loggedIn,...}`) — on success, push `{id, claudeConfigDir: <dir>}` to `profiles.json` and save; print success. (The "import existing ~/.claude" offer from the TS is OPTIONAL — implement only if trivial; otherwise skip and note it.)
- `meridian profile login <name>`: load profiles; not found → error+exit 1; oauth-token profile → error (claude auth login doesn't apply); else re-run the same login (headless or `claude auth login`) against the profile's existing `claudeConfigDir`.
- The `claude` executable path: reuse however the rest of the bin resolves it (the `serve` command takes `--claude`; for these subcommands default to `"claude"` on PATH, overridable by `MERIDIAN_CLAUDE_PATH` if that env is already honored elsewhere — check and match).

> Most of this task is process orchestration that cannot be unit-tested (interactive login). Keep the testable logic in small pure helpers where possible (e.g. a `fn auth_status_logged_in(json: &str) -> bool` parsing `claude auth status` output — unit-test that). Do NOT add a test that spawns `claude` or hits the network. The id-validation and duplicate-rejection paths are already covered by `profile_cli_test`; add a test only for any NEW pure helper you introduce.

- [ ] **Step 1: Write a failing test for the one new pure helper**

```rust
// append to crates/meridian/tests/profile_cli_test.rs (or oauth_test.rs)
#[test]
fn auth_status_parsing() {
    use meridian::oauth::auth_status_logged_in;
    assert!(auth_status_logged_in(r#"{"loggedIn":true,"email":"a@b.c"}"#));
    assert!(!auth_status_logged_in(r#"{"loggedIn":false}"#));
    assert!(!auth_status_logged_in("not json"));
}
```

(Place `pub fn auth_status_logged_in(json: &str) -> bool` in `oauth.rs`: parse JSON, `obj["loggedIn"].as_bool().unwrap_or(false)`.)

- [ ] **Step 2: Run to verify failure.**
- [ ] **Step 3: Implement** the helper + the two CLI flows in `main.rs`. Read the current `ProfileCmd::Add` arm (it currently has the `--oauth-token`-only path with a "not yet supported" branch) and the clap `ProfileCmd` enum; add a `--headless` flag to `Add` and a new `Login { id, headless }` variant. Keep the existing `--oauth-token` path unchanged.
- [ ] **Step 4: Run to verify pass** — `cargo test -p meridian` green; `cargo build -p meridian-cli`; clippy clean.
- [ ] **Step 5: Manual smoke (non-gating, document only):** `meridian profile add test-acct` should open the `claude auth login` flow; `--headless` should print an authorize URL and prompt for a code. (Cannot be automated; note the manual result.)
- [ ] **Step 6: Commit**

```bash
git add bin/meridian-cli/src/main.rs crates/meridian/src/oauth.rs crates/meridian/tests/*.rs
git commit -m "feat(meridian-cli): profile add/login browser OAuth (claude auth login + --headless PKCE)"
```

---

## Self-Review

1. **Coverage:** PKCE + authorize URL + code parse + exchange + credential mapping (T1) ✓; `add`/`login` interactive + headless flows + auth-status verify (T2) ✓. The `--oauth-token` add path (3c) is untouched.
2. **Placeholders:** none for the testable surface; interactive/network paths are inherently manual and are structured as thin orchestration over tested primitives.
3. **Type consistency:** `oauth` consumes `token_refresh::{sha256_bytes, oauth_token_request, create_platform_credential_store, CredentialsFile}`; `build_credentials_file` returns the same `CredentialsFile` the store writes; the CLI uses `new_oauth_session`/`parse_authorization_code`/`exchange_code`/`build_credentials_file` exactly as defined in T1.
4. **Risk notes:** (a) the sha256 refactor in T1 must not change `sha256("abc")` — the token_refresh test pins it; run it. (b) base64url must be no-padding + url-safe or the PKCE challenge mismatches and login fails — the RFC 7636 vector test guards this. (c) secrets (code, verifier, tokens) only via stdin/credential-store, never argv/logs. (d) the browser e2e is manual — do not fabricate a passing automated test for it.
