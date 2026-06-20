# Phase 3c — Profile Management Surface Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the profile-management surface on top of Phase 3b's request-path profiles: persistent `settings.json`, live disk re-discovery of `profiles.json`, the `GET /profiles/list` + `POST /profiles/active` routes, and a `meridian profile` CLI (`list` / `use` / `remove` / `add --oauth-token`).

**Architecture:** A new leaf `settings` module persists `activeProfile` to `~/.config/meridian/settings.json`. `ProfileStore` gains an *effective profile list* — its static startup config (from `MERIDIAN_PROFILES`) merged with a 5-second-TTL re-read of `profiles.json` on disk — so profiles added by the CLI are picked up without a restart. Setting the active profile persists to settings and evicts the session cache (sessions were started under the previous account's credentials). The CLI subcommands read/write `profiles.json` directly and drive the active switch through the running proxy's HTTP route.

**Tech Stack:** Rust, axum, serde/serde_json, clap (already in `bin/meridian-cli`), std::fs. Ports `src-original/src/proxy/settings.ts`, the disk-discovery / `getEffectiveProfiles` / `listProfiles` / `restoreActiveProfile` parts of `profiles.ts`, the `/profiles/list` + `/profiles/active` handlers in `server.ts`, and the management subset of `profileCli.ts`.

## Global Constraints

- **1:1 functional port** of the TS original's management surface. Match field names, file locations, file modes, and HTTP response shapes verbatim where given below.
- **DEFERRED to Phase 3d (do NOT build here):** the OAuth *browser-login* flow — `meridian profile add <name>` (PKCE authorize URL, token exchange, manual/headless OAuth, platform credential-store write) and `meridian profile login` — plus `claude auth status` enrichment of `/profiles/list` (the `email` / `subscriptionType` / `loggedIn` fields), the rate-limit store, `GET /v1/usage/quota`, and the `MERIDIAN_API_KEY` auth middleware that guards `/profiles/*` in the original. This slice ships `add --oauth-token` only (no browser, no credential store).
- **Settings file:** `~/.config/meridian/settings.json`, mode `0o600`, written as pretty JSON (2-space) + trailing newline. Reads return an empty settings object on missing/invalid file. Writes MERGE with existing keys (never clobber unknown keys). Key: `activeProfile` (string, optional).
- **Profiles file:** `~/.config/meridian/profiles.json` (already read by 3b's `load_profiles`), mode `0o600`, pretty JSON + trailing newline. Profile id charset: `^[A-Za-z0-9_-]+$`.
- **Disk-discovery TTL:** 5000 ms. Disk discovery is OFF by default (tests / programmatic `ProfileStore::new`) and ON only when the server starts without `MERIDIAN_PROFILES` set.
- **Active-profile precedence (unchanged from 3b):** request header `x-meridian-profile` > active profile > first effective profile > implicit `"default"`. The active profile only changes resolution when no header is sent.
- **On active-profile switch:** persist to settings AND evict the session cache (`SessionStore::clear`). The original also clears the rate-limit store — that store does not exist yet (3d), so omit it here.
- **No Claude/Claude Code attribution** on commits (per repo CLAUDE.md).
- **Formatting:** the repo has no rustfmt config/CI and uses a dense hand style (`main` itself fails `cargo fmt --check`). Match the surrounding style; do NOT run `cargo fmt` across the tree. Quality gate is `cargo build` + `cargo test` (default suite) + `cargo clippy --workspace --all-targets -- -D warnings`, all green. Tests tagged `#[ignore]` need a live `claude` CLI — do not run them in the gate.

---

## File Structure

- **Create** `crates/meridian/src/settings.rs` — `MeridianSettings`, `load_settings`, `save_settings`, `get_active_profile`, `set_active_profile`. Leaf module (no deps on server/session/profiles). Path override via a function arg so tests don't touch `$HOME`.
- **Modify** `crates/meridian/src/profiles.rs` — add the effective-profile list (config ⊕ disk-TTL), `disk_discovery` flag, `effective()`, `list()`, and make `resolve_id` / `overlay_for` / `resolved_type` / `find` operate on the effective list. `set_active` persists to settings when disk discovery is on. Add `restore_active`.
- **Modify** `crates/meridian/src/session.rs` — add `SessionStore::clear`.
- **Modify** `crates/meridian/src/server.rs` — add `GET /profiles/list` and `POST /profiles/active` routes + handlers; wire the cache eviction on switch.
- **Modify** `crates/meridian/src/lib.rs` — `pub mod settings;`.
- **Modify** `bin/meridian-cli/src/main.rs` — add the `profile` subcommand (`list` / `use <id>` / `remove <id>` / `add <id> --oauth-token [TOKEN]`); enable disk discovery + restore-active in the `serve` path when `MERIDIAN_PROFILES` is unset.
- **Create** `crates/meridian/src/profile_cli.rs` — pure helpers shared by the CLI: `is_valid_profile_id`, `load_profiles_json`, `save_profiles_json`, `dirs_to_remove_on_remove`. (Keeps `main.rs` thin and lets the logic be unit-tested.)

---

## Task 1: Settings module (`settings.rs`)

**Files:**
- Create: `crates/meridian/src/settings.rs`
- Modify: `crates/meridian/src/lib.rs` (add `pub mod settings;`)
- Test: `crates/meridian/tests/settings_test.rs`

**Interfaces:**
- Produces:
  - `pub struct MeridianSettings { pub active_profile: Option<String> }` (serde: `#[serde(rename = "activeProfile", default, skip_serializing_if = "Option::is_none")]`)
  - `pub fn settings_path() -> Option<PathBuf>` — `$HOME/.config/meridian/settings.json`
  - `pub fn load_settings_at(path: &Path) -> MeridianSettings` — empty on missing/invalid
  - `pub fn save_settings_at(path: &Path, updates: MeridianSettings) -> std::io::Result<()>` — merge + 0o600 + pretty + trailing `\n`
  - `pub fn get_active_profile() -> Option<String>` — convenience over `settings_path()`
  - `pub fn set_active_profile(id: &str)` — convenience; warns (tracing) on write failure, never panics

- [ ] **Step 1: Write the failing test**

```rust
// crates/meridian/tests/settings_test.rs
use meridian::settings::{load_settings_at, save_settings_at, MeridianSettings};

#[test]
fn save_then_load_roundtrips_active_profile() {
    let dir = std::env::temp_dir().join(format!("mer-settings-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");

    save_settings_at(&path, MeridianSettings { active_profile: Some("work".into()) }).unwrap();
    let s = load_settings_at(&path);
    assert_eq!(s.active_profile.as_deref(), Some("work"));

    // missing file -> empty
    let _ = std::fs::remove_file(&path);
    assert_eq!(load_settings_at(&path).active_profile, None);
    // invalid json -> empty (not a panic)
    std::fs::write(&path, b"{not json").unwrap();
    assert_eq!(load_settings_at(&path).active_profile, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_merges_and_does_not_clobber_unknown_keys() {
    let dir = std::env::temp_dir().join(format!("mer-settings-merge-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    // a pre-existing file with an unknown key
    std::fs::write(&path, b"{\n  \"theme\": \"dark\"\n}\n").unwrap();
    save_settings_at(&path, MeridianSettings { active_profile: Some("p1".into()) }).unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(raw.contains("\"theme\""), "unknown key must survive merge");
    assert!(raw.contains("\"activeProfile\""));
    assert!(raw.ends_with("\n"), "trailing newline");
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p meridian --test settings_test`
Expected: FAIL — `settings` module / functions don't exist.

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/meridian/src/settings.rs
//! Persistent server settings (~/.config/meridian/settings.json). Survives
//! restarts. Leaf module — no imports from server/session/profiles.
//! Port of src-original/src/proxy/settings.ts.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MeridianSettings {
    /// Last active profile ID — restored on proxy startup.
    #[serde(rename = "activeProfile", default, skip_serializing_if = "Option::is_none")]
    pub active_profile: Option<String>,
}

pub fn settings_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".config").join("meridian").join("settings.json"))
}

/// Read settings. Returns default (empty) on a missing or invalid file.
pub fn load_settings_at(path: &Path) -> MeridianSettings {
    let Ok(raw) = std::fs::read_to_string(path) else { return MeridianSettings::default() };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Merge `updates` into the existing file (preserving unknown keys) and write
/// back with mode 0o600, pretty JSON, trailing newline.
pub fn save_settings_at(path: &Path, updates: MeridianSettings) -> std::io::Result<()> {
    // Start from whatever is on disk as a generic object so unknown keys survive.
    let mut obj: Map<String, Value> = std::fs::read_to_string(path)
        .ok()
        .and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_default();
    if let Value::Object(up) = serde_json::to_value(&updates).unwrap_or(Value::Null) {
        for (k, v) in up {
            obj.insert(k, v);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = serde_json::to_string_pretty(&Value::Object(obj))?;
    body.push('\n');
    write_private(path, body.as_bytes())
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

pub fn get_active_profile() -> Option<String> {
    settings_path().map(|p| load_settings_at(&p)).and_then(|s| s.active_profile)
}

pub fn set_active_profile(id: &str) {
    let Some(path) = settings_path() else { return };
    if let Err(e) = save_settings_at(&path, MeridianSettings { active_profile: Some(id.to_string()) }) {
        tracing::warn!("failed to persist active profile to {}: {e}", path.display());
    }
}
```

Add to `crates/meridian/src/lib.rs`: `pub mod settings;`

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p meridian --test settings_test`
Expected: PASS (2 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/meridian/src/settings.rs crates/meridian/src/lib.rs crates/meridian/tests/settings_test.rs
git commit -m "feat(meridian): settings.json persistence (activeProfile, 0o600, merge)"
```

---

## Task 2: Effective profile list + disk discovery + restore/persist (`profiles.rs`)

**Files:**
- Modify: `crates/meridian/src/profiles.rs`
- Test: `crates/meridian/tests/profiles_effective_test.rs`

**Interfaces:**
- Consumes: `MeridianSettings` / `get_active_profile` / `set_active_profile` from Task 1 (only in the disk-discovery-on path).
- Produces (new/changed on `ProfileStore`):
  - field `disk_discovery: bool` + `disk_cache: Mutex<Option<(std::time::Instant, Vec<ProfileConfig>)>>`
  - `pub fn with_disk_discovery(mut self) -> Self` — turns it on (builder; default off)
  - `pub fn effective(&self) -> Vec<ProfileConfig>` — config ⊕ disk(TTL), config wins by id
  - `pub fn list(&self) -> Vec<ProfileSummary>` where `pub struct ProfileSummary { pub id: String, pub kind: ProfileType, pub is_active: bool }`
  - `pub fn restore_active(&self)` — load persisted active (disk-discovery only), validate it exists, set it
  - `set_active` now also persists via `set_active_profile` when disk discovery is on
  - `resolve_id` / `overlay_for` / `resolved_type` operate over `effective()` (not the static `profiles` field)

**Notes for the implementer:**
- The static startup set comes from `MERIDIAN_PROFILES`/`profiles.json` via existing `load_profiles` and is stored in the existing field (rename `profiles` → `config_profiles` for clarity). When disk discovery is OFF, `effective()` == `config_profiles` (preserves all 3b behavior and tests).
- Disk source = `~/.config/meridian/profiles.json`. Re-read with a 5s TTL cache (`DISK_CACHE_TTL_MS = 5_000`). On read error, warn and treat as empty (don't poison the cache with a panic). Cache the parsed Vec + an `Instant`.
- `effective()` = `config_profiles` followed by disk profiles whose id is not already in `config_profiles` (config/env wins).
- `restore_active`: if active already set, return. If disk discovery off, return. Read `get_active_profile()`; if `Some(saved)` and (`effective()` is empty OR contains `saved`), set it; else `tracing::warn!` and leave unset.

- [ ] **Step 1: Write the failing test**

```rust
// crates/meridian/tests/profiles_effective_test.rs
use meridian::profiles::{ProfileConfig, ProfileStore, ProfileType};

fn cfg(id: &str) -> ProfileConfig {
    ProfileConfig { id: id.into(), kind: Some(ProfileType::ClaudeMax),
        claude_config_dir: Some(format!("/cfg/{id}")), api_key: None, base_url: None, oauth_token: None }
}

#[test]
fn disk_discovery_off_uses_only_config_profiles() {
    let store = ProfileStore::new(vec![cfg("a")], std::env::temp_dir());
    let ids: Vec<_> = store.effective().into_iter().map(|p| p.id).collect();
    assert_eq!(ids, vec!["a"]);
    // list() reports active flag against precedence (first profile here)
    let l = store.list();
    assert_eq!(l.len(), 1);
    assert!(l[0].is_active);
}

#[test]
fn config_wins_over_disk_by_id() {
    // disk discovery merge logic is exercised via the pure merge helper
    let from_config = vec![cfg("shared"), cfg("only-config")];
    let from_disk = vec![cfg("shared"), cfg("only-disk")];
    let merged = meridian::profiles::merge_effective(&from_config, from_disk);
    let ids: Vec<_> = merged.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, vec!["shared", "only-config", "only-disk"]);
    // the "shared" entry is the config one (its config dir)
    assert_eq!(merged[0].claude_config_dir.as_deref(), Some("/cfg/shared"));
}

#[test]
fn resolve_and_overlay_use_effective_list() {
    let store = ProfileStore::new(vec![cfg("a"), cfg("b")], std::env::temp_dir());
    assert_eq!(store.resolve_id(Some("b")), "b");
    assert_eq!(store.resolve_id(None), "a"); // first
    use meridian_transport::factory::EnvResolver;
    assert_eq!(store.overlay("b").get("CLAUDE_CONFIG_DIR").map(String::as_str), Some("/cfg/b"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p meridian --test profiles_effective_test`
Expected: FAIL — `effective`, `list`, `merge_effective` don't exist.

- [ ] **Step 3: Write minimal implementation**

In `crates/meridian/src/profiles.rs`:

1. Rename the struct field `profiles` → `config_profiles` and update `ProfileStore::new` / `from_env_or_disk` / `find`. Add fields:

```rust
const DISK_CACHE_TTL_MS: u128 = 5_000;

pub struct ProfileStore {
    config_profiles: Vec<ProfileConfig>,
    config_root: PathBuf,
    active: Mutex<Option<String>>,
    disk_discovery: bool,
    disk_cache: Mutex<Option<(std::time::Instant, Vec<ProfileConfig>)>>,
}

#[derive(Debug, Clone)]
pub struct ProfileSummary {
    pub id: String,
    pub kind: ProfileType,
    pub is_active: bool,
}
```

2. Update `new` and add the builder + helpers:

```rust
impl ProfileStore {
    pub fn new(profiles: Vec<ProfileConfig>, config_root: PathBuf) -> Self {
        ProfileStore { config_profiles: profiles, config_root, active: Mutex::new(None),
            disk_discovery: false, disk_cache: Mutex::new(None) }
    }

    pub fn from_env_or_disk(config_root: PathBuf) -> Self {
        Self::new(load_profiles().unwrap_or_default(), config_root)
    }

    /// Turn on live re-discovery of ~/.config/meridian/profiles.json (5s TTL).
    pub fn with_disk_discovery(mut self) -> Self { self.disk_discovery = true; self }

    fn disk_profiles(&self) -> Vec<ProfileConfig> {
        let mut g = self.disk_cache.lock().unwrap();
        if let Some((at, ref cached)) = *g {
            if at.elapsed().as_millis() < DISK_CACHE_TTL_MS { return cached.clone(); }
        }
        let fresh = profiles_json_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|raw| match serde_json::from_str::<Vec<ProfileConfig>>(&raw) {
                Ok(v) => Some(v),
                Err(e) => { tracing::warn!("profiles.json is not valid JSON: {e}; ignoring"); None }
            })
            .unwrap_or_default();
        *g = Some((std::time::Instant::now(), fresh.clone()));
        fresh
    }

    pub fn effective(&self) -> Vec<ProfileConfig> {
        if !self.disk_discovery { return self.config_profiles.clone(); }
        merge_effective(&self.config_profiles, self.disk_profiles())
    }

    pub fn list(&self) -> Vec<ProfileSummary> {
        let eff = self.effective();
        if eff.is_empty() { return vec![]; }
        let active = self.active().unwrap_or_else(|| eff[0].id.clone());
        eff.iter().map(|p| ProfileSummary {
            id: p.id.clone(),
            kind: self.resolved_type_of(p),
            is_active: p.id == active,
        }).collect()
    }

    pub fn restore_active(&self) {
        if self.active().is_some() { return; }
        if !self.disk_discovery { return; }
        let Some(saved) = crate::settings::get_active_profile() else { return };
        let eff = self.effective();
        if eff.is_empty() || eff.iter().any(|p| p.id == saved) {
            *self.active.lock().unwrap() = Some(saved);
        } else {
            tracing::warn!("saved active profile \"{saved}\" not found; using default");
        }
    }
}

/// Config profiles followed by disk profiles whose id is not already present.
pub fn merge_effective(from_config: &[ProfileConfig], from_disk: Vec<ProfileConfig>) -> Vec<ProfileConfig> {
    let ids: std::collections::HashSet<&str> = from_config.iter().map(|p| p.id.as_str()).collect();
    let mut out = from_config.to_vec();
    out.extend(from_disk.into_iter().filter(|p| !ids.contains(p.id.as_str())));
    out
}

fn profiles_json_path() -> Option<PathBuf> {
    dirs_config_meridian().map(|d| d.join("profiles.json"))
}
```

3. Make resolution operate on the effective list. Replace `find`, `resolve_id`, `resolved_type`, `overlay_for` so they read `self.effective()`:

```rust
    fn find_in<'a>(eff: &'a [ProfileConfig], id: &str) -> Option<&'a ProfileConfig> {
        eff.iter().find(|p| p.id == id)
    }

    pub fn resolve_id(&self, requested: Option<&str>) -> String {
        let eff = self.effective();
        if eff.is_empty() { return DEFAULT_PROFILE_ID.to_string(); }
        let first = eff[0].id.clone();
        let candidate = requested.map(str::to_string)
            .or_else(|| self.active())
            .unwrap_or_else(|| first.clone());
        if Self::find_in(&eff, &candidate).is_some() { candidate }
        else { tracing::warn!("unknown profile \"{candidate}\"; using first profile \"{first}\""); first }
    }

    fn resolved_type_of(&self, p: &ProfileConfig) -> ProfileType {
        if p.oauth_token.is_some() || p.kind == Some(ProfileType::OauthToken) { ProfileType::OauthToken }
        else { p.kind.unwrap_or(ProfileType::ClaudeMax) }
    }

    pub fn resolved_type(&self, id: &str) -> ProfileType {
        let eff = self.effective();
        match Self::find_in(&eff, id) {
            Some(p) => self.resolved_type_of(p),
            None => ProfileType::ClaudeMax,
        }
    }

    fn overlay_for(&self, id: &str) -> HashMap<String, String> {
        let eff = self.effective();
        let Some(p) = Self::find_in(&eff, id) else { return HashMap::new() };
        let mut env = HashMap::new();
        match self.resolved_type_of(p) {
            // ... body unchanged from 3b (oauth-token / api / claude-max arms) ...
        }
        env
    }
```

(Keep the existing body of the three `match` arms exactly as in 3b.)

4. `set_active` persists when disk discovery is on:

```rust
    pub fn set_active(&self, id: String) {
        if self.disk_discovery { crate::settings::set_active_profile(&id); }
        *self.active.lock().unwrap() = Some(id);
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p meridian --test profiles_effective_test` then the existing `cargo test -p meridian --test profiles_test` and `--test profile_routing_test`.
Expected: PASS — new tests pass AND all 3b profile tests still pass (disk discovery off by default preserves behavior).

- [ ] **Step 5: Commit**

```bash
git add crates/meridian/src/profiles.rs crates/meridian/tests/profiles_effective_test.rs
git commit -m "feat(meridian): effective profile list (config + disk TTL), list(), restore/persist active"
```

---

## Task 3: `SessionStore::clear` + route wiring prerequisites (`session.rs`)

**Files:**
- Modify: `crates/meridian/src/session.rs`
- Test: `crates/meridian/tests/session_test.rs` (append)

**Interfaces:**
- Produces: `pub fn clear(&self)` on `SessionStore` — drops all cached sessions.

- [ ] **Step 1: Write the failing test (append to session_test.rs)**

```rust
#[test]
fn clear_evicts_all_sessions() {
    use meridian::session::{SessionStore, fingerprint};
    let store = SessionStore::new();
    let fp = fingerprint(&[]);
    store.put(fp.clone(), "sess-1".into());
    assert!(store.get(&fp).is_some());
    store.clear();
    assert!(store.get(&fp).is_none());
}
```

(If the store's insert method is not named `put`, use the actual name — check `session.rs` and adjust this test before running. The point is: insert one, `clear()`, assert it's gone.)

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p meridian --test session_test clear_evicts_all_sessions`
Expected: FAIL — `clear` does not exist.

- [ ] **Step 3: Write minimal implementation**

In `crates/meridian/src/session.rs`, add to the `impl SessionStore` (the store wraps a `Mutex`/`RwLock` over a map — clear it):

```rust
    /// Evict every cached session. Called when the active profile changes:
    /// sessions were started under the previous account's credentials and
    /// must not be resumed under a different identity.
    pub fn clear(&self) {
        self.inner.lock().unwrap().clear();
    }
```

(Adjust `self.inner.lock().unwrap()` to the actual field/lock used by `SessionStore`.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p meridian --test session_test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/meridian/src/session.rs crates/meridian/tests/session_test.rs
git commit -m "feat(meridian): SessionStore::clear (evict-all on profile switch)"
```

---

## Task 4: `/profiles/list` + `/profiles/active` routes (`server.rs`)

**Files:**
- Modify: `crates/meridian/src/server.rs`
- Test: `crates/meridian/tests/profile_mgmt_routes_test.rs`

**Interfaces:**
- Consumes: `ProfileStore::list` / `effective` / `set_active` (Task 2), `SessionStore::clear` (Task 3). `AppState` already holds `profiles` and `sessions`.
- Produces routes:
  - `GET /profiles/list` → `200 {"profiles":[{"id","type","isActive"}...],"activeProfile":"<id>"}`. `type` serializes kebab-case (`claude-max` / `api` / `oauth-token`) to match 3b's `ProfileType` serde. (Auth-status enrichment — email/subscriptionType/loggedIn — is DEFERRED to 3d; do not add those fields.)
  - `POST /profiles/active` body `{"profile":"<id>"}`:
    - invalid/missing JSON → `400 {"error":"..."}`
    - no effective profiles → `400 {"error":"No profiles configured"}`
    - unknown id → `400 {"error":"Unknown profile: <id>. Available: <ids>"}`
    - success → `set_active`, `sessions.clear()`, `200 {"success":true,"activeProfile":"<id>"}`

**Note:** the original guards `/profiles/*` behind `requireAuth` (MERIDIAN_API_KEY). That middleware is 3d — do NOT add it here; these routes are unauthenticated in this slice (same posture as the other routes today).

- [ ] **Step 1: Write the failing test**

```rust
// crates/meridian/tests/profile_mgmt_routes_test.rs
use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;
use meridian::profiles::{ProfileConfig, ProfileStore, ProfileType};
use meridian::server::router;
use meridian::session::SessionStore;

// Minimal runner stub: never actually spawns anything (these routes don't run turns).
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
        Box::pin(tokio_stream::empty())
    }
}

fn app(profiles: Vec<ProfileConfig>) -> axum::Router {
    let store = Arc::new(ProfileStore::new(profiles, std::env::temp_dir()));
    router(Arc::new(NoRun), Arc::new(SessionStore::new()), store)
}

fn pc(id: &str, kind: ProfileType) -> ProfileConfig {
    ProfileConfig { id: id.into(), kind: Some(kind), claude_config_dir: Some("/x".into()),
        api_key: None, base_url: None, oauth_token: None }
}

#[tokio::test]
async fn list_returns_profiles_and_active() {
    let app = app(vec![pc("a", ProfileType::ClaudeMax), pc("b", ProfileType::Api)]);
    let r = app.oneshot(Request::get("/profiles/list").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(v["profiles"].as_array().unwrap().len(), 2);
    assert_eq!(v["profiles"][0]["id"], "a");
    assert_eq!(v["profiles"][1]["type"], "api");
    assert_eq!(v["activeProfile"], "a"); // first is active by default
}

#[tokio::test]
async fn active_switches_known_profile_and_rejects_unknown() {
    let app = app(vec![pc("a", ProfileType::ClaudeMax), pc("b", ProfileType::ClaudeMax)]);
    // unknown -> 400
    let bad = Request::post("/profiles/active").header("content-type","application/json")
        .body(Body::from(r#"{"profile":"nope"}"#)).unwrap();
    let rb = app.clone().oneshot(bad).await.unwrap();
    assert_eq!(rb.status(), StatusCode::BAD_REQUEST);
    // known -> 200 success
    let ok = Request::post("/profiles/active").header("content-type","application/json")
        .body(Body::from(r#"{"profile":"b"}"#)).unwrap();
    let ro = app.clone().oneshot(ok).await.unwrap();
    assert_eq!(ro.status(), StatusCode::OK);
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(ro.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(v["success"], true);
    assert_eq!(v["activeProfile"], "b");
    // now list reports b active
    let rl = app.oneshot(Request::get("/profiles/list").body(Body::empty()).unwrap()).await.unwrap();
    let lv: Value = serde_json::from_slice(&axum::body::to_bytes(rl.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(lv["activeProfile"], "b");
}

#[tokio::test]
async fn active_with_no_profiles_is_400() {
    let app = app(vec![]);
    let r = app.oneshot(Request::post("/profiles/active").header("content-type","application/json")
        .body(Body::from(r#"{"profile":"x"}"#)).unwrap()).await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
}
```

> Interface check: confirm `router(...)` and the `TurnRunner`/`StreamRunner`/`TurnRequest`/`TurnResult`/`EventStream`/`ProxyError` paths match the actual signatures in `server.rs`/`sse.rs`/`error.rs` before writing the impl; adjust the stub to whatever the traits require. If `EventStream` is not a `Pin<Box<dyn Stream>>`, build the empty stream with the type its alias expects.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p meridian --test profile_mgmt_routes_test`
Expected: FAIL — routes return 404.

- [ ] **Step 3: Write minimal implementation**

Add the two routes in `router(...)` where the other routes are registered, and the handlers. `AppState<R>` already carries `profiles: Arc<ProfileStore>` and `sessions: Arc<SessionStore>`.

```rust
// in router(): .route("/profiles/list", get(profiles_list))
//               .route("/profiles/active", post(profiles_active))

async fn profiles_list<R: TurnRunner + StreamRunner + Clone + Send + Sync + 'static>(
    State(state): State<AppState<R>>,
) -> axum::response::Response {
    let list = state.profiles.list();
    let active = state.profiles.resolve_id(None);
    let profiles: Vec<Value> = list.into_iter().map(|p| serde_json::json!({
        "id": p.id,
        "type": p.kind,        // serde kebab-case via ProfileType
        "isActive": p.is_active,
    })).collect();
    axum::Json(serde_json::json!({ "profiles": profiles, "activeProfile": active })).into_response()
}

async fn profiles_active<R: TurnRunner + StreamRunner + Clone + Send + Sync + 'static>(
    State(state): State<AppState<R>>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let parsed: Result<Value, _> = serde_json::from_slice(&body);
    let profile = match parsed.ok().as_ref().and_then(|v| v.get("profile")).and_then(Value::as_str) {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => return ProxyError::BadRequest("Missing 'profile' in request body".into()).into_response(),
    };
    let eff = state.profiles.effective();
    if eff.is_empty() {
        return ProxyError::BadRequest("No profiles configured".into()).into_response();
    }
    if !eff.iter().any(|p| p.id == profile) {
        let avail = eff.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(", ");
        return ProxyError::BadRequest(format!("Unknown profile: {profile}. Available: {avail}")).into_response();
    }
    state.profiles.set_active(profile.clone());
    state.sessions.clear(); // sessions started under the old account can't be resumed
    axum::Json(serde_json::json!({ "success": true, "activeProfile": profile })).into_response()
}
```

> Use whatever `State`/handler-generic pattern the existing handlers in `server.rs` use (the existing `messages`/`chat_completions` handlers show the exact `State<AppState<R>>` bound and imports). If body extraction in the existing handlers uses `Json<Value>`, prefer matching that; the manual `Bytes` parse above is only to return the precise 400 on malformed JSON — keep it if the original's "Invalid JSON" 400 matters, otherwise reuse the existing extractor.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p meridian --test profile_mgmt_routes_test`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/meridian/src/server.rs crates/meridian/tests/profile_mgmt_routes_test.rs
git commit -m "feat(meridian): GET /profiles/list + POST /profiles/active routes"
```

---

## Task 5: `meridian profile` CLI (`profile_cli.rs` + `main.rs`)

**Files:**
- Create: `crates/meridian/src/profile_cli.rs`
- Modify: `crates/meridian/src/lib.rs` (`pub mod profile_cli;`)
- Modify: `bin/meridian-cli/src/main.rs`
- Test: `crates/meridian/tests/profile_cli_test.rs`

**Interfaces:**
- Produces (pure, unit-tested helpers in `profile_cli.rs`):
  - `pub fn is_valid_profile_id(id: &str) -> bool` — `^[A-Za-z0-9_-]+$`, non-empty
  - `pub fn profiles_json_path() -> Option<PathBuf>` and `pub fn profiles_dir() -> Option<PathBuf>` (`~/.config/meridian/profiles.json`, `~/.config/meridian/profiles`)
  - `pub fn load_profiles_json_at(path: &Path) -> Vec<ProfileConfig>` (empty on missing/invalid + warn)
  - `pub fn save_profiles_json_at(path: &Path, profiles: &[ProfileConfig]) -> std::io::Result<()>` (0o600, pretty, trailing `\n`, create parent)
  - `pub fn dirs_to_remove_on_remove(p: &ProfileConfig, profiles_dir: &Path) -> Vec<PathBuf>` — port of `dirsToRemoveOnProfileRemove`
  - `pub fn add_oauth_token(path: &Path, id: &str, token: &str) -> Result<(), String>` — validate id, reject duplicate, append `{id, type:"oauth-token", oauthToken}`, save
- `main.rs`: clap subcommand `profile { list | use <id> | remove <id> | add <id> --oauth-token [TOKEN] }`.

**CLI behavior (match the original's intent; plain output, no ANSI needed):**
- `profile list` — print each id + type from `profiles.json`; if none, print `No profiles configured.` and a hint.
- `profile add <id> --oauth-token [TOKEN]` — if TOKEN omitted, read one line from stdin (no echo not required for this slice; a plain line read is acceptable). Validate id, reject duplicate, persist, print `Profile "<id>" added (OAuth token).` (Browser `add` without `--oauth-token` → print that it is not yet supported and point to `--oauth-token`; the full browser flow is Phase 3d.)
- `profile remove <id>` — load, find (error to stderr + exit 1 if missing), compute `dirs_to_remove_on_remove`, splice out, save, `rmSync` each dir if it exists, print `Profile "<id>" removed.`
- `profile use <id>` — POST `http://<host>:<port>/profiles/active` `{"profile":id}`; on `success` also `settings::set_active_profile(id)` and print `Switched to profile: <id>`; on error print the server error / connection error to stderr + exit 1. Host/port from `MERIDIAN_HOST`/`MERIDIAN_PORT` (fallback `127.0.0.1` / the serve default port used elsewhere in `main.rs`).
- In the `serve` path: when `MERIDIAN_PROFILES` is unset, build the `ProfileStore` `.with_disk_discovery()` and call `restore_active()` before serving, so CLI-added profiles + the persisted active profile are honored.

- [ ] **Step 1: Write the failing test (pure helpers)**

```rust
// crates/meridian/tests/profile_cli_test.rs
use meridian::profile_cli::{is_valid_profile_id, load_profiles_json_at, save_profiles_json_at,
    add_oauth_token, dirs_to_remove_on_remove};
use meridian::profiles::{ProfileConfig, ProfileType};

#[test]
fn id_validation() {
    assert!(is_valid_profile_id("work-1_x"));
    assert!(!is_valid_profile_id(""));
    assert!(!is_valid_profile_id("has space"));
    assert!(!is_valid_profile_id("dots.bad"));
}

#[test]
fn add_oauth_token_persists_and_rejects_dupes() {
    let dir = std::env::temp_dir().join(format!("mer-pcli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("profiles.json");
    add_oauth_token(&path, "ci", "sk-ant-oat-xxx").unwrap();
    let loaded = load_profiles_json_at(&path);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "ci");
    assert_eq!(loaded[0].oauth_token.as_deref(), Some("sk-ant-oat-xxx"));
    // duplicate rejected
    assert!(add_oauth_token(&path, "ci", "other").is_err());
    // invalid id rejected
    assert!(add_oauth_token(&path, "bad id", "t").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn remove_dirs_for_oauth_and_browser_profiles() {
    let pdir = std::path::Path::new("/root/profiles");
    // oauth-token: isolation dir profiles/<id>
    let oauth = ProfileConfig { id: "ci".into(), kind: Some(ProfileType::OauthToken),
        claude_config_dir: None, api_key: None, base_url: None, oauth_token: Some("t".into()) };
    assert_eq!(dirs_to_remove_on_remove(&oauth, pdir), vec![pdir.join("ci")]);
    // browser profile with config dir under profiles_dir
    let browser = ProfileConfig { id: "work".into(), kind: Some(ProfileType::ClaudeMax),
        claude_config_dir: Some("/root/profiles/work".into()), api_key: None, base_url: None, oauth_token: None };
    assert_eq!(dirs_to_remove_on_remove(&browser, pdir), vec![std::path::PathBuf::from("/root/profiles/work")]);
    // config dir OUTSIDE profiles_dir is not removed
    let imported = ProfileConfig { id: "home".into(), kind: Some(ProfileType::ClaudeMax),
        claude_config_dir: Some("/home/u/.claude".into()), api_key: None, base_url: None, oauth_token: None };
    assert!(dirs_to_remove_on_remove(&imported, pdir).is_empty());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p meridian --test profile_cli_test`
Expected: FAIL — `profile_cli` module doesn't exist.

- [ ] **Step 3: Write minimal implementation**

```rust
// crates/meridian/src/profile_cli.rs
//! Pure helpers for the `meridian profile` CLI. Reads/writes
//! ~/.config/meridian/profiles.json. Port of the management subset of
//! src-original/src/proxy/profileCli.ts (browser OAuth login is Phase 3d).

use std::path::{Path, PathBuf};
use crate::profiles::ProfileConfig;

pub fn is_valid_profile_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn config_meridian() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config").join("meridian"))
}
pub fn profiles_json_path() -> Option<PathBuf> { config_meridian().map(|d| d.join("profiles.json")) }
pub fn profiles_dir() -> Option<PathBuf> { config_meridian().map(|d| d.join("profiles")) }

pub fn load_profiles_json_at(path: &Path) -> Vec<ProfileConfig> {
    let Ok(raw) = std::fs::read_to_string(path) else { return vec![] };
    match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => { tracing::warn!("failed to read {}: {e}", path.display()); vec![] }
    }
}

pub fn save_profiles_json_at(path: &Path, profiles: &[ProfileConfig]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    let mut body = serde_json::to_string_pretty(profiles)?;
    body.push('\n');
    write_private(path, body.as_bytes())
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(bytes)
}
#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> { std::fs::write(path, bytes) }

/// Directories to delete when a profile is removed (port of dirsToRemoveOnProfileRemove).
pub fn dirs_to_remove_on_remove(p: &ProfileConfig, profiles_dir: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(cd) = &p.claude_config_dir {
        if Path::new(cd).starts_with(profiles_dir) { dirs.push(PathBuf::from(cd)); }
    }
    let is_oauth = p.oauth_token.is_some()
        || p.kind == Some(crate::profiles::ProfileType::OauthToken);
    if is_oauth {
        let iso = profiles_dir.join(&p.id);
        if !dirs.contains(&iso) { dirs.push(iso); }
    }
    dirs
}

pub fn add_oauth_token(path: &Path, id: &str, token: &str) -> Result<(), String> {
    if !is_valid_profile_id(id) {
        return Err("Invalid profile ID. Use only letters, numbers, hyphens, underscores.".into());
    }
    let mut profiles = load_profiles_json_at(path);
    if profiles.iter().any(|p| p.id == id) {
        return Err(format!("Profile \"{id}\" already exists."));
    }
    if token.trim().is_empty() { return Err("Empty token. Aborted.".into()); }
    profiles.push(ProfileConfig {
        id: id.to_string(),
        kind: Some(crate::profiles::ProfileType::OauthToken),
        claude_config_dir: None, api_key: None, base_url: None,
        oauth_token: Some(token.trim().to_string()),
    });
    save_profiles_json_at(path, &profiles).map_err(|e| e.to_string())
}
```

Add `pub mod profile_cli;` to `crates/meridian/src/lib.rs`.

Then wire clap in `bin/meridian-cli/src/main.rs` — add a `Profile` subcommand with the four actions, calling the helpers above; `use` drives an HTTP POST (reqwest is already a dep for e2e? if not, use a tiny `std::net::TcpStream` HTTP/1.1 request or the existing HTTP client used elsewhere in the bin — check `main.rs`/`service.rs` for the pattern `health_check` uses and reuse it). For the `serve` path, gate `.with_disk_discovery()` + `restore_active()` on `std::env::var("MERIDIAN_PROFILES").is_err()`.

> Implementer: check how `service.rs::health_check` performs its HTTP request and reuse that mechanism for `profile use` (POST /profiles/active). Do not add a new HTTP client dependency if one is already used.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p meridian --test profile_cli_test`, then build the bin: `cargo build -p meridian-cli`.
Expected: PASS + clean build.

- [ ] **Step 5: Smoke-test the CLI manually (non-gating)**

```bash
MERIDIAN_PROFILES= ./target/debug/meridian-cli profile add ci --oauth-token sk-test-123
./target/debug/meridian-cli profile list   # shows: ci (oauth-token)
./target/debug/meridian-cli profile remove ci
```

- [ ] **Step 6: Commit**

```bash
git add crates/meridian/src/profile_cli.rs crates/meridian/src/lib.rs bin/meridian-cli/src/main.rs crates/meridian/tests/profile_cli_test.rs
git commit -m "feat(meridian-cli): meridian profile list/use/remove/add --oauth-token + serve disk-discovery"
```

---

## Self-Review

1. **Spec coverage:** settings.json persistence (T1) ✓; disk-discovery TTL + effective list + restore/persist active (T2) ✓; session-cache eviction on switch (T3) ✓; `/profiles/list` + `/profiles/active` (T4) ✓; `meridian profile` CLI subset (T5) ✓. Deferred items (browser OAuth login, auth-status enrichment, rate-limit/quota, MERIDIAN_API_KEY auth) are explicitly out of scope per Global Constraints.
2. **Placeholders:** none — every step carries real code or an explicit "check the existing signature and match it" instruction where the exact local type must be confirmed.
3. **Type consistency:** `ProfileType` serde kebab-case is reused for the `/profiles/list` `type` field; `ProfileConfig` field names (`claude_config_dir`, `oauth_token`, `kind`) match 3b; `ProfileStore::effective`/`list`/`set_active`/`resolve_id` are the single source of the effective list used by both routes and 3b resolution.
4. **Risk note for the executor:** Task 2 changes `ProfileStore` internals that 3b depends on — run the FULL `cargo test -p meridian` after T2 (not just the new test) to confirm 3b's `profiles_test` / `profile_routing_test` still pass. The default-off disk discovery is what preserves 3b behavior.
