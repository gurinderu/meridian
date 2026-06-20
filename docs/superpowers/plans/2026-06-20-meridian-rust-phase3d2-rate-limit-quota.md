# Phase 3d-2 — Rate-Limit Store + `/v1/usage/quota` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Capture the `rate_limit_event` messages the spawned `claude` CLI emits in its stream-json output into an in-memory per-bucket store, and expose the latest snapshot at `GET /v1/usage/quota` (SDK-sourced buckets only).

**Architecture:** A new `CliMessage::RateLimitEvent` codec variant. A shared `Arc<RateLimitStore>` (interior-mutable, keyed by `rateLimitType`) is created once at serve start, handed to both the `PooledRunner` (which records events as they stream through its turn/stream loops) and the axum `AppState` (which the quota route reads). On profile switch the store is cleared (a different account = different quota).

**Tech Stack:** Rust, axum, serde_json, std::time, std::sync::Mutex.

## Global Constraints

- **Verified live:** the CLI emits `{"type":"rate_limit_event","rate_limit_info":{status, resetsAt, rateLimitType, utilization?, overageStatus?, overageResetsAt?, overageDisabledReason?, isUsingOverage?, surpassedThreshold?, ...}, uuid, session_id}` lines in `--output-format stream-json` mode. Parse defensively — every inner field is optional except treat the whole `rate_limit_info` object as opaque-ish (keep raw access).
- **Scope = SDK-sourced buckets only.** The OAuth-usage-API overlay (`fetchOAuthUsage`, the `extraUsage` field, `sources.oauth`) is DEFERRED to after 3d-3 (it needs the credential store). This slice returns `extraUsage: null` and `sources.oauth: null`, with `sources.sdk.entryCount` populated.
- **Response shape (verbatim, port of server.ts `/v1/usage/quota`):**
  ```json
  {"profile": "<id|null>",
   "buckets": [{"type","status","utilization","resetsAt","isUsingOverage","overageStatus","overageResetsAt","overageDisabledReason","surpassedThreshold","observedAt"}],
   "extraUsage": null,
   "sources": {"oauth": null, "sdk": {"entryCount": <n>}}}
  ```
  The internal `"default"` bucket (events with no `rateLimitType`) is recorded but FILTERED OUT of the `buckets` array (it is a Meridian-side fallback, not a real Anthropic bucket). `entryCount` counts the filtered (real) buckets, matching the TS `sdkEntries.length` after its `rateLimitType !== undefined` filter.
- **Profile for the response:** `?profile=<id>` query param, else `resolve_id(None)` (active/first/default). Empty store → `200` with `buckets: []`.
- **No new crates.** `observedAt` is epoch-ms via `std::time::SystemTime::now()`.
- **No Claude attribution** on commits. Match the repo's dense hand style; do NOT run `cargo fmt` (no rustfmt config; `main` fails `fmt --check` by design). Gate: `cargo build` + `cargo test` (default suite) + `cargo clippy --workspace --all-targets -- -D warnings`, all green. Skip `#[ignore]` live tests.

---

## File Structure

- **Modify** `crates/meridian-transport/src/codec.rs` — add `CliMessage::RateLimitEvent { info: Value, raw: Value }` + parse arm.
- **Create** `crates/meridian/src/rate_limit.rs` — `RateLimitStore` (record / get_all / entry_count / clear), `bucket_to_json`.
- **Modify** `crates/meridian/src/lib.rs` — `pub mod rate_limit;`.
- **Modify** `crates/meridian/src/pooled_runner.rs` — `pooled_runner(...)` takes `Arc<RateLimitStore>`; both the `run_one_turn` collect loop and the `run_stream` pump record `RateLimitEvent`s.
- **Modify** `crates/meridian/src/server.rs` — `AppState` gains `rate_limit: Arc<RateLimitStore>`; `router` / `router_with_auth` take it; add `GET /v1/usage/quota` handler; `profiles_active` clears it.
- **Modify** `bin/meridian-cli/src/main.rs` — create the shared store, pass to `pooled_runner` and `router`.
- **Modify** all existing `router(...)` / `router_with_auth(...)` / `pooled_runner(...)` call sites in tests to pass a fresh `Arc::new(RateLimitStore::new())`.

---

## Task 1: Codec variant + `RateLimitStore`

**Files:**
- Modify: `crates/meridian-transport/src/codec.rs`
- Create: `crates/meridian/src/rate_limit.rs`
- Modify: `crates/meridian/src/lib.rs`
- Test: `crates/meridian-transport/tests/codec_test.rs` (append), `crates/meridian/tests/rate_limit_test.rs`

**Interfaces:**
- Produces: `CliMessage::RateLimitEvent { info: Value, raw: Value }` (info = the `rate_limit_info` object).
- Produces on `RateLimitStore`:
  - `pub fn new() -> Self` (+ `Default`)
  - `pub fn record(&self, info: &Value)` — no-op if `info` is not an object; bucket key = `info["rateLimitType"].as_str()` else `"default"`; last-write-wins; stamps `observedAt` (epoch ms).
  - `pub fn get_all(&self) -> Vec<Value>` — each stored entry as a normalized bucket JSON (see `bucket_to_json`), newest-first by `observedAt`, **excluding** the `"default"` bucket.
  - `pub fn entry_count(&self) -> usize` — number of real (non-`"default"`) buckets.
  - `pub fn clear(&self)`
- `pub fn bucket_to_json(info: &Value, observed_at: u64) -> Value` — builds the verbatim bucket object (type/status/utilization/resetsAt/isUsingOverage/overageStatus/overageResetsAt/overageDisabledReason/surpassedThreshold/observedAt), pulling each field from `info` with JSON `null` fallback (and `isUsingOverage` defaulting to `false`).

- [ ] **Step 1: Write the failing tests**

```rust
// crates/meridian-transport/tests/codec_test.rs  (append)
#[test]
fn parses_rate_limit_event() {
    use meridian_transport::codec::{parse_line, CliMessage};
    let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","rateLimitType":"five_hour","utilization":0.5},"uuid":"u","session_id":"s"}"#;
    match parse_line(line).unwrap() {
        CliMessage::RateLimitEvent { info, .. } => {
            assert_eq!(info["rateLimitType"], "five_hour");
            assert_eq!(info["status"], "allowed");
        }
        other => panic!("expected RateLimitEvent, got {other:?}"),
    }
}
```

```rust
// crates/meridian/tests/rate_limit_test.rs
use meridian::rate_limit::RateLimitStore;
use serde_json::json;

#[test]
fn records_by_bucket_last_write_wins_and_filters_default() {
    let s = RateLimitStore::new();
    s.record(&json!({"status":"allowed","rateLimitType":"five_hour","utilization":0.2}));
    s.record(&json!({"status":"allowed_warning","rateLimitType":"five_hour","utilization":0.9})); // overwrites
    s.record(&json!({"status":"allowed","rateLimitType":"seven_day"}));
    s.record(&json!({"status":"allowed"})); // no rateLimitType -> "default" bucket, filtered out

    let all = s.get_all();
    assert_eq!(s.entry_count(), 2, "default bucket excluded from count");
    assert_eq!(all.len(), 2, "default bucket excluded from get_all");
    let five = all.iter().find(|b| b["type"] == "five_hour").unwrap();
    assert_eq!(five["status"], "allowed_warning"); // last write won
    assert_eq!(five["utilization"], 0.9);
    // normalized fields present with null fallback
    let seven = all.iter().find(|b| b["type"] == "seven_day").unwrap();
    assert_eq!(seven["utilization"], serde_json::Value::Null);
    assert_eq!(seven["isUsingOverage"], false);
}

#[test]
fn record_ignores_non_objects_and_clear_empties() {
    let s = RateLimitStore::new();
    s.record(&json!("not an object"));
    s.record(&json!(null));
    assert_eq!(s.entry_count(), 0);
    s.record(&json!({"rateLimitType":"five_hour","status":"allowed"}));
    assert_eq!(s.entry_count(), 1);
    s.clear();
    assert_eq!(s.entry_count(), 0);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p meridian-transport --test codec_test parses_rate_limit_event` and `cargo test -p meridian --test rate_limit_test`
Expected: FAIL (variant / module absent).

- [ ] **Step 3: Implement**

Codec — add the variant and parse arm in `crates/meridian-transport/src/codec.rs`:

```rust
// in enum CliMessage:
    RateLimitEvent { info: Value, raw: Value },
// in parse_line match, before the `_ =>` arm:
        ("rate_limit_event", _) => CliMessage::RateLimitEvent {
            info: v.get("rate_limit_info").cloned().unwrap_or(Value::Null),
            raw: v,
        },
```

Module `crates/meridian/src/rate_limit.rs`:

```rust
//! In-memory snapshot of the Claude Max subscription quota, captured from the
//! CLI's `rate_limit_event` stream messages. One bucket per `rateLimitType`
//! (last-write-wins), plus an internal "default" bucket for events that omit
//! it. Port of src-original/src/proxy/rateLimitStore.ts (SDK-sourced subset).

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};

const DEFAULT_BUCKET: &str = "default";

struct Entry {
    info: Value,
    observed_at: u64,
}

#[derive(Default)]
pub struct RateLimitStore {
    entries: Mutex<HashMap<String, Entry>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl RateLimitStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a `rate_limit_info` snapshot. No-op for non-objects.
    pub fn record(&self, info: &Value) {
        if !info.is_object() {
            return;
        }
        let key = info.get("rateLimitType").and_then(Value::as_str).unwrap_or(DEFAULT_BUCKET).to_string();
        self.entries.lock().unwrap().insert(key, Entry { info: info.clone(), observed_at: now_ms() });
    }

    /// Real (non-default) buckets, newest-first by observedAt.
    pub fn get_all(&self) -> Vec<Value> {
        let g = self.entries.lock().unwrap();
        let mut out: Vec<(u64, Value)> = g
            .iter()
            .filter(|(k, _)| k.as_str() != DEFAULT_BUCKET)
            .map(|(_, e)| (e.observed_at, bucket_to_json(&e.info, e.observed_at)))
            .collect();
        out.sort_by(|a, b| b.0.cmp(&a.0));
        out.into_iter().map(|(_, v)| v).collect()
    }

    pub fn entry_count(&self) -> usize {
        self.entries.lock().unwrap().keys().filter(|k| k.as_str() != DEFAULT_BUCKET).count()
    }

    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }
}

/// Normalize a `rate_limit_info` object into the wire bucket shape (verbatim
/// field set from the TS original), with JSON null fallbacks.
pub fn bucket_to_json(info: &Value, observed_at: u64) -> Value {
    let f = |k: &str| info.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "type": f("rateLimitType"),
        "status": f("status"),
        "utilization": f("utilization"),
        "resetsAt": f("resetsAt"),
        "isUsingOverage": info.get("isUsingOverage").and_then(Value::as_bool).unwrap_or(false),
        "overageStatus": f("overageStatus"),
        "overageResetsAt": f("overageResetsAt"),
        "overageDisabledReason": f("overageDisabledReason"),
        "surpassedThreshold": f("surpassedThreshold"),
        "observedAt": observed_at,
    })
}
```

Add `pub mod rate_limit;` to `crates/meridian/src/lib.rs`.

- [ ] **Step 4: Run to verify pass** — both test commands green.
- [ ] **Step 5: Commit**

```bash
git add crates/meridian-transport/src/codec.rs crates/meridian-transport/tests/codec_test.rs crates/meridian/src/rate_limit.rs crates/meridian/src/lib.rs crates/meridian/tests/rate_limit_test.rs
git commit -m "feat: parse rate_limit_event + RateLimitStore (per-bucket quota snapshot)"
```

---

## Task 2: Wire recording + `GET /v1/usage/quota` route

**Files:**
- Modify: `crates/meridian/src/pooled_runner.rs`, `crates/meridian/src/server.rs`, `bin/meridian-cli/src/main.rs`
- Modify: every `router(...)` / `router_with_auth(...)` / `pooled_runner(...)` call site in tests.
- Test: `crates/meridian/tests/quota_route_test.rs`

**Interfaces:**
- Consumes: `RateLimitStore` (Task 1), `CliMessage::RateLimitEvent`.
- Changed signatures:
  - `pub fn pooled_runner(exe: String, config_root: PathBuf, cap: usize, profiles: Arc<ProfileStore>, rate_limit: Arc<RateLimitStore>) -> PooledRunner`
  - `AppState<R> { runner, sessions, profiles, rate_limit: Arc<RateLimitStore> }`
  - `pub fn router<R>(runner, sessions, profiles, rate_limit) -> Router`
  - `pub fn router_with_auth<R>(runner, sessions, profiles, rate_limit, api_key) -> Router`
- New route: `GET /v1/usage/quota` (placed on the protected router, after `/v1/models`).

**Implementer notes — read the live files first.** The exact field/closure shapes (the `run_one_turn` collect loop and the `run_stream` pump in `pooled_runner.rs`, the `AppState` construction + manual `Clone` impl, the `router`/`router_with_auth` bodies in `server.rs`, and the serve wiring in `main.rs`) are the source of truth. The snippets below show intent.

- [ ] **Step 1: Write the failing test**

```rust
// crates/meridian/tests/quota_route_test.rs
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
    fn run_stream(&self, _m: String, _s: Option<String>, _p: String, _pr: Option<String>)
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
```

- [ ] **Step 2: Run to verify failure** — `cargo test -p meridian --test quota_route_test` fails to compile (signature/route missing). That compile failure is the "red".

- [ ] **Step 3: Implement**

1. `pooled_runner.rs`: add `rate_limit: Arc<RateLimitStore>` to `PooledRunner` + the `pooled_runner(...)` constructor param. In BOTH event loops, record rate-limit events. In `run_one_turn` (which has `proc: &mut CliProcess` but no store access), thread the store: give `run_one_turn` an extra `&RateLimitStore` arg, or inline the match. Simplest: have the `PooledRunner` methods pass `&self.rate_limit` into `run_one_turn`. Add a match arm:

```rust
CliMessage::RateLimitEvent { info, .. } => rate_limit.record(&info),
```

in the `collect` loop of `run_one_turn` and in the `run_stream` pump (use `lease.proc()` events). For `run_stream`, the spawned task must `move` an `Arc<RateLimitStore>` clone in.

2. `server.rs`: add `rate_limit: Arc<RateLimitStore>` to `AppState` (and its manual `Clone` impl). Thread it through `router` + `router_with_auth`. Register `.route("/v1/usage/quota", get(usage_quota::<R>))` on the protected router. Handler:

```rust
async fn usage_quota<R: TurnRunner + StreamRunner + 'static>(
    State(state): State<AppState<R>>,
    headers: axum::http::HeaderMap,
    axum::extract::RawQuery(q): axum::extract::RawQuery,
) -> axum::response::Response {
    // ?profile=<id> else resolve_id(None)
    let requested = q.as_deref().and_then(|qs| qs.split('&')
        .find_map(|kv| kv.strip_prefix("profile=")))
        .map(|s| s.to_string());
    let _ = &headers;
    let profile = state.profiles.resolve_id(requested.as_deref());
    let buckets = state.rate_limit.get_all();
    let count = state.rate_limit.entry_count();
    axum::Json(serde_json::json!({
        "profile": profile,
        "buckets": buckets,
        "extraUsage": serde_json::Value::Null,
        "sources": { "oauth": serde_json::Value::Null, "sdk": { "entryCount": count } },
    })).into_response()
}
```

(If a cleaner query extractor exists in the codebase, use it; `RawQuery` avoids adding `serde_urlencoded` typed structs. Drop the `headers` param if unused — shown only in case the existing handler style threads it.)

3. `profiles_active` handler: after `state.sessions.clear()`, add `state.rate_limit.clear();` (a profile switch invalidates the previous account's quota snapshot).

4. `bin/meridian-cli/src/main.rs`: in the serve path, `let rate_limit = std::sync::Arc::new(meridian::rate_limit::RateLimitStore::new());` then pass `rate_limit.clone()` to `pooled_runner(...)` and `rate_limit` to `router(...)`.

5. Update every other call site: search `router(` / `router_with_auth(` / `pooled_runner(` across `crates/meridian/tests`, `bin/meridian-cli/tests`, and `bin/meridian-cli/src` and add a fresh `Arc::new(RateLimitStore::new())` argument in the correct position. Run `cargo test` — the compiler will list each one.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p meridian --test quota_route_test`, then full `cargo test`, then `cargo clippy --workspace --all-targets -- -D warnings`.
Expected: all green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(meridian): GET /v1/usage/quota + record rate_limit_event in turn/stream loops"
```

---

## Self-Review

1. **Coverage:** codec variant (T1) ✓; store with bucket semantics + default-filter (T1) ✓; recording in both loops (T2) ✓; route + response shape (T2) ✓; clear-on-switch (T2) ✓. OAuth-usage overlay explicitly deferred (Global Constraints).
2. **Placeholders:** none — full code given; the one "read the live file" instruction is for matching existing `pooled_runner`/`server` shapes, with the intent code shown.
3. **Type consistency:** `RateLimitStore` methods (`record`/`get_all`/`entry_count`/`clear`) are used identically in T2's wiring and tests; the bucket JSON shape in `bucket_to_json` matches the route's `buckets` element and the response keys match the Global Constraints contract.
4. **Risk note for executor:** T2 changes `pooled_runner`/`AppState`/`router` signatures — every call site (bin + ~8 test files) must add the new arg. Let the compiler enumerate them; full `cargo test` must pass before commit.
