# Phase B — Session-Sticky `claude` Processes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Eliminate the ~0.8–1.3 s per-continuation-turn `claude` cold-start (spawn + session reload) by keeping the live process for a conversation parked in memory and routing the next turn of that conversation to it.

**Architecture:** Empirically de-risked — a single `claude --input-format stream-json` process handles multiple sequential user turns with a shared in-memory session (probe: turn 2 recalled a codeword set in turn 1). So a continuation can be sent as a delta to the *live* process — no respawn, no `--resume` disk reload.

The proxy is already set up for this with **zero server changes**: the server (a) sends `resume=Some(session_id)` + only the last user message (delta) on a continuation, and (b) after each turn stores `SessionStore[fingerprint(convo+reply)] = session_id`. Phase B adds a runner-local `ParkedStore` keyed by `(profile_id, session_id)`:

- **Warm path:** on `resume=Some(sid)`, if a live, healthy parked process exists under `(profile, sid)`, send the delta to it (no spawn), then re-park it under the new turn's `session_id`.
- **Cold path** (no parked process — first turn, evicted, post-restart, divergence): spawn exactly as today (with `--resume sid` when present), then **park** the live process under its result `session_id` instead of shutting it down.

Because the server keys `SessionStore` by fingerprint and stores the result `session_id`, the next request resolves the same `sid` and finds the parked process — consistent by construction, regardless of whether `claude` mints a new `session_id` per turn (we always park under the latest).

A background reaper bounds memory: parked processes idle past `--park-ttl-secs` are shut down, and the store is capped at `--max-parked` (LRU eviction).

**Tech Stack:** Rust, tokio, the existing transport `CliProcess`.

## Global Constraints

- **Scope = the non-streaming turn path only** (`run_turn`: `/v1/messages` non-stream + `/v1/chat/completions` non-stream). The streaming path does NOT do resume/session-continuity today (it sends only the last user text with no resume) — parking it requires first adding resume to streaming; **DEFERRED to Phase B2**. The passthrough/tool path (`run_passthrough`, `--max-turns 3`, surface-and-pause) is **NOT parked** in this phase (different process shape) — keep its current spawn+discard.
- **Correctness first — worst case must equal today's behavior.** Every warm-path failure (no parked proc, dead proc, profile mismatch, concurrent same-session take) falls back to the existing cold path. Phase B must never change a turn's *result*, only its latency.
- **The warm path sends the SAME delta the resume path sends** (the server already passes `req.prompt` = last user message only when `resume.is_some()`). Never send the flattened history to a parked process (it would double-count context).
- **Park key = `(profile_id, session_id)`** so a continuation resolved to a different account simply doesn't match (defensive; a session belongs to one account).
- **Liveness:** a parked process may die while idle. Check `is_alive()` on take; on a dead process, drop it (it is `kill_on_drop(true)`) and fall back to cold. Detect mid-turn death and fall back.
- **Memory governance:** `--max-parked` (default 8) caps parked count (LRU-evict + shutdown the evicted on insert over cap); `--park-ttl-secs` (default 300) — a reaper shuts down processes idle longer than that. The existing `--cap` still bounds in-flight cold spawns. Total live processes ≤ `cap + max_parked`.
- **No new crates.** No rustfmt (dense hand style; `main` fails `fmt --check` by design) — match surrounding style, do NOT run `cargo fmt`. Gate: `cargo build` + `cargo test` (default suite) + `cargo clippy --workspace --all-targets -- -D warnings`, all green. The live multi-turn validation is `#[ignore]` (needs an authenticated `claude`).
- **No Claude attribution** on commits.

---

## File Structure

- **Modify** `crates/meridian-transport/src/process.rs` — add `CliProcess::is_alive(&mut self) -> bool` (non-blocking child liveness via `try_wait`).
- **Create** `crates/meridian/src/parked.rs` — `ParkedStore<P>` (generic over the process type so it's unit-testable with a fake): `take(profile_id, session_id) -> Option<P>`, `park(profile_id, session_id, proc, max_parked) -> Vec<P>` (returns evicted-over-cap procs for the caller to shut down), `reap(ttl) -> Vec<P>` (returns timed-out procs), `len()`.
- **Modify** `crates/meridian/src/lib.rs` — `pub mod parked;`.
- **Modify** `crates/meridian/src/pooled_runner.rs` — `PooledRunner` holds `Arc<ParkedStore<CliProcess>>` + `max_parked`; `run_turn` implements warm/cold + park; a `reap_once()` helper.
- **Modify** `bin/meridian-cli/src/main.rs` — `serve` flags `--max-parked` / `--park-ttl-secs`; construct the runner with them; spawn the reaper task.

---

## Task 1: `CliProcess::is_alive`

**Files:** Modify `crates/meridian-transport/src/process.rs`; Test `crates/meridian-transport/tests/process_test.rs` (append, `#[ignore]` if it needs a real spawn — otherwise a non-ignored test that spawns a trivial `/bin/sh -c exit`).

**Interfaces:** Produces `pub fn is_alive(&mut self) -> bool` on `CliProcess` — `true` while the child has not exited (`child.try_wait()` returns `Ok(None)`), `false` once it has exited or on error.

- [ ] **Step 1: Write the failing test**

```rust
// crates/meridian-transport/tests/process_test.rs (append)
// A direct unit test for is_alive without the full claude protocol: spawn a
// trivial child via the same CliProcess::spawn path is heavy, so test the
// liveness semantics against a process that exits quickly.
#[tokio::test]
async fn is_alive_reflects_child_exit() {
    use meridian_transport::process::spawn;
    use meridian_transport::spawn::SpawnConfig;
    use std::collections::HashMap;
    use std::sync::Arc;
    // Use `sh -c 'sleep 0.3'` as a stand-in "claude": it ignores our stdin
    // protocol but stays alive briefly then exits, which is all is_alive needs.
    let cfg = SpawnConfig {
        config_dir: std::env::temp_dir().join("mer-alive-test"),
        model: None, mcp_config: None, include_partial_messages: false,
        resume: None, max_turns: None, env_overlay: HashMap::new(),
    };
    // NoTools registry
    struct NoTools;
    impl meridian_transport::mcp::ToolRegistry for NoTools {
        fn list(&self) -> Vec<serde_json::Value> { vec![] }
        fn call(&self, _n: &str, _a: &serde_json::Value) -> serde_json::Value { serde_json::json!({}) }
    }
    let base: HashMap<String,String> = std::env::vars().collect();
    let mut proc = spawn("sh", &cfg, &base, Arc::new(NoTools)).await
        .expect("spawn sh");
    // NOTE: build_args prepends claude-specific flags; `sh` will get them as
    // args and likely exit immediately. That's fine — we only assert is_alive
    // transitions to false after the child exits.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!proc.is_alive(), "child should have exited");
}
```

> Implementer: if driving `sh` through `CliProcess::spawn` is awkward (the claude-specific args make `sh` error immediately, which is actually fine for asserting `is_alive()==false` after exit), keep the test as a liveness-after-exit assertion. If it proves flaky, mark it `#[ignore]` and instead add a pure assertion that a freshly-spawned long-lived `sleep` reports `is_alive()==true` then `false` after it exits. The goal is only to pin the `try_wait` semantics.

- [ ] **Step 2: Run to verify failure** — `cargo test -p meridian-transport --test process_test is_alive` (method missing).

- [ ] **Step 3: Implement** in `crates/meridian-transport/src/process.rs`, on `impl CliProcess`:

```rust
    /// True while the child process is still running. Non-blocking.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
```

- [ ] **Step 4: Run to verify pass.** Then `cargo clippy -p meridian-transport --all-targets -- -D warnings`.
- [ ] **Step 5: Commit**

```bash
git add crates/meridian-transport/src/process.rs crates/meridian-transport/tests/process_test.rs
git commit -m "feat(transport): CliProcess::is_alive (non-blocking liveness)"
```

---

## Task 2: `ParkedStore`

**Files:** Create `crates/meridian/src/parked.rs`; Modify `crates/meridian/src/lib.rs`; Test `crates/meridian/tests/parked_test.rs`.

**Interfaces (Produces):**
- `pub struct ParkedStore<P> { ... }` (internally `Mutex<HashMap<(String,String), Entry<P>>>` where the key is `(profile_id, session_id)`; `Entry { proc: P, last_used: Instant }`).
- `pub fn new() -> Self`
- `pub fn take(&self, profile_id: &str, session_id: &str) -> Option<P>` — remove + return the parked process for this key, if any.
- `pub fn park(&self, profile_id: String, session_id: String, proc: P, max_parked: usize) -> Vec<P>` — insert (last_used = now); if over `max_parked`, evict least-recently-used entries and RETURN them (caller shuts them down). Returns the evicted procs (may be empty).
- `pub fn reap(&self, ttl: std::time::Duration) -> Vec<P>` — remove + return all entries whose `last_used` is older than `ttl`.
- `pub fn len(&self) -> usize` (+ `is_empty`).

Generic over `P` so tests use a fake. `Instant::now()` is fine in the runtime (not a workflow script).

- [ ] **Step 1: Write the failing tests**

```rust
// crates/meridian/tests/parked_test.rs
use meridian::parked::ParkedStore;

#[test]
fn park_take_roundtrip_keyed_by_profile_and_session() {
    let s: ParkedStore<u32> = ParkedStore::new();
    assert!(s.park("p1".into(), "s1".into(), 100, 8).is_empty());
    // wrong profile / wrong session -> miss
    assert_eq!(s.take("p2", "s1"), None);
    assert_eq!(s.take("p1", "s2"), None);
    // exact key -> hit, and it's removed
    assert_eq!(s.take("p1", "s1"), Some(100));
    assert_eq!(s.take("p1", "s1"), None);
    assert_eq!(s.len(), 0);
}

#[test]
fn park_over_cap_evicts_lru() {
    let s: ParkedStore<u32> = ParkedStore::new();
    // cap = 2; insert 3 distinct keys -> the oldest is evicted and returned.
    assert!(s.park("p".into(), "a".into(), 1, 2).is_empty());
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert!(s.park("p".into(), "b".into(), 2, 2).is_empty());
    std::thread::sleep(std::time::Duration::from_millis(5));
    let evicted = s.park("p".into(), "c".into(), 3, 2);
    assert_eq!(evicted, vec![1], "the LRU entry (a=1) is evicted and returned");
    assert_eq!(s.len(), 2);
    assert_eq!(s.take("p", "a"), None);
    assert_eq!(s.take("p", "b"), Some(2));
    assert_eq!(s.take("p", "c"), Some(3));
}

#[test]
fn reap_returns_timed_out_entries() {
    let s: ParkedStore<u32> = ParkedStore::new();
    s.park("p".into(), "s".into(), 7, 8);
    std::thread::sleep(std::time::Duration::from_millis(20));
    // ttl shorter than the age -> reaped
    let reaped = s.reap(std::time::Duration::from_millis(10));
    assert_eq!(reaped, vec![7]);
    assert_eq!(s.len(), 0);
    // nothing left to reap
    assert!(s.reap(std::time::Duration::from_millis(0)).is_empty());
}
```

- [ ] **Step 2: Run to verify failure** — module missing.

- [ ] **Step 3: Implement** `crates/meridian/src/parked.rs`:

```rust
//! Session-sticky process park. Keeps a live `claude` process per conversation
//! `(profile_id, session_id)` so a continuation turn reuses it instead of
//! cold-spawning. Bounded by an LRU cap (park) + an idle TTL (reap); generic
//! over the process type for testability.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

struct Entry<P> {
    proc: P,
    last_used: Instant,
}

#[derive(Default)]
pub struct ParkedStore<P> {
    inner: Mutex<HashMap<(String, String), Entry<P>>>,
}

impl<P> ParkedStore<P> {
    pub fn new() -> Self {
        ParkedStore { inner: Mutex::new(HashMap::new()) }
    }

    pub fn take(&self, profile_id: &str, session_id: &str) -> Option<P> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.remove(&(profile_id.to_string(), session_id.to_string())).map(|e| e.proc)
    }

    /// Insert; evict LRU entries over `max_parked` and return the evicted procs
    /// (the caller shuts them down).
    pub fn park(&self, profile_id: String, session_id: String, proc: P, max_parked: usize) -> Vec<P> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.insert((profile_id, session_id), Entry { proc, last_used: Instant::now() });
        let mut evicted = Vec::new();
        while g.len() > max_parked.max(1) {
            // find the least-recently-used key
            let Some(lru) = g.iter().min_by_key(|(_, e)| e.last_used).map(|(k, _)| k.clone()) else { break };
            if let Some(e) = g.remove(&lru) {
                evicted.push(e.proc);
            }
        }
        evicted
    }

    pub fn reap(&self, ttl: Duration) -> Vec<P> {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let stale: Vec<(String, String)> = g.iter()
            .filter(|(_, e)| now.duration_since(e.last_used) > ttl)
            .map(|(k, _)| k.clone())
            .collect();
        stale.into_iter().filter_map(|k| g.remove(&k).map(|e| e.proc)).collect()
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
}
```

Add `pub mod parked;` to `crates/meridian/src/lib.rs`.

- [ ] **Step 4: Run to verify pass** + clippy clean.
- [ ] **Step 5: Commit**

```bash
git add crates/meridian/src/parked.rs crates/meridian/src/lib.rs crates/meridian/tests/parked_test.rs
git commit -m "feat(meridian): ParkedStore (session-keyed process park, LRU cap + TTL reap)"
```

---

## Task 3: Wire warm/cold + park into `run_turn`

**Files:** Modify `crates/meridian/src/pooled_runner.rs`. Test: a live `#[ignore]` multi-turn reuse test in `bin/meridian-cli/tests/sticky_e2e_test.rs`.

**Interfaces:** `PooledRunner` gains `parked: Arc<ParkedStore<CliProcess>>` and `max_parked: usize`; `pooled_runner(...)` gains a `max_parked` parameter (default wired in Task 4). Add `pub fn reap_parked(&self, ttl: Duration)` that calls `parked.reap` and shuts down each returned process. Add `pub fn parked(&self) -> Arc<ParkedStore<CliProcess>>` if the reaper needs it (or reap via the runner handle).

**run_turn logic (non-tools path):**
1. `let key_profile = profile_id(&req)`.
2. **Warm path:** if `let Some(sid) = &req.resume`, try `self.parked.take(&key_profile, sid)`:
   - got `mut proc`: if `proc.is_alive()` → run the delta turn on it (`run_one_turn(&mut proc, system, prompt, &rate_limit)`); on `Ok(result)`: re-park under `result.session_id` (if Some) via `park` (shut down any evicted), return result; on `Err`: shut the proc down (drop) and fall through to cold. If `!is_alive()` → drop, fall through.
3. **Cold path** (unchanged spawn): `pool.acquire(key)` → `run_one_turn` on the lease's proc. Then instead of `shutdown()+discard()`: if `result` is `Ok` AND has a `session_id`, **take the process OUT of the lease** and park it (so it isn't shut down); else `shutdown()+discard()` as today. (See "extracting the process from the lease" note below.)

**Extracting the process to park (lease ↔ park hand-off):** the pool `Lease` owns the process and recycles/discards on drop. To park a cold-path process you must move it out of the lease without the lease shutting it down. Add to `meridian-transport` `pool.rs` a `Lease::take_proc(&mut self) -> Option<P>` that takes the `proc` out (like `discard` but returns it) and marks the lease so Drop frees only the global-cap slot (the process is now owned by the parked store). Then: `let mut proc = lease.take_proc().unwrap();` and `self.parked.park(key_profile, sid, proc, self.max_parked)` (shut down evicted). On the warm path the process is already owned (taken from parked), so no lease is involved.

> Implementer: read `crates/meridian-transport/src/pool.rs` — `Lease` has a `discard` flag and `proc: Option<P>`. `take_proc` = set discard-equivalent (free the slot, don't release to idle) + `self.proc.take()`. Confirm the Drop impl frees the cap slot when the proc was taken.

**run_one_turn already returns `TurnResult { session_id, .. }`** — reuse it. The warm path calls the SAME `run_one_turn`.

- [ ] **Step 1: Write the live multi-turn reuse test (`#[ignore]`)**

```rust
// bin/meridian-cli/tests/sticky_e2e_test.rs
use std::sync::Arc;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use meridian::pooled_runner::pooled_runner;
use meridian::profiles::ProfileStore;
use meridian::rate_limit::RateLimitStore;
use meridian::server::router;
use meridian::session::SessionStore;

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn continuation_reuses_a_parked_process() {
    let root = std::env::temp_dir().join(format!("mer-sticky-{}", std::process::id()));
    let profiles = Arc::new(ProfileStore::new(vec![], root.clone()));
    let runner = Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), Arc::new(RateLimitStore::new()), 8));
    let sessions = Arc::new(SessionStore::new());
    let app = router(runner.clone(), sessions, profiles, Arc::new(RateLimitStore::new()));

    // Turn 1: set a codeword (no prior context).
    let r1 = app.clone().oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(json!({"model":"sonnet","messages":[
            {"role":"user","content":"Remember the codeword TANGERINE19. Reply with just OK."}]}).to_string())).unwrap())
        .await.unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    // a process is now parked
    assert_eq!(runner.parked().len(), 1, "turn 1 should park its process");

    // Turn 2: continuation (includes turn-1 user + assistant) -> resume -> warm reuse.
    let r2 = app.oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(json!({"model":"sonnet","messages":[
            {"role":"user","content":"Remember the codeword TANGERINE19. Reply with just OK."},
            {"role":"assistant","content":"OK"},
            {"role":"user","content":"What was the exact codeword?"}]}).to_string())).unwrap())
        .await.unwrap();
    assert_eq!(r2.status(), StatusCode::OK);
    let v: Value = serde_json::from_slice(&axum::body::to_bytes(r2.into_body(), usize::MAX).await.unwrap()).unwrap();
    let text = v["content"][0]["text"].as_str().unwrap_or("");
    assert!(text.contains("TANGERINE19"), "continuation recalled the codeword: {text}");
}
```

- [ ] **Step 2: Run to verify failure** — `pooled_runner`'s new `max_parked` arg + `runner.parked()` don't exist yet (compile error = red).
- [ ] **Step 3: Implement** the warm/cold + park logic in `pooled_runner.rs` and `Lease::take_proc` in `pool.rs` per above. Keep `run_passthrough` and `run_stream` unchanged (still discard).
- [ ] **Step 4: Run** the default suite (must stay green; the sticky test is `#[ignore]`), then the live test: `cargo test -p meridian-cli --test sticky_e2e_test -- --ignored --nocapture`. Confirm turn 2 recalls the codeword AND `parked().len()==1` after turn 1. Also re-run `profile_e2e_test`/`stream_e2e_test --ignored` to confirm no regression. clippy clean.
- [ ] **Step 5: Commit**

```bash
git add crates/meridian/src/pooled_runner.rs crates/meridian-transport/src/pool.rs bin/meridian-cli/tests/sticky_e2e_test.rs
git commit -m "feat(meridian): session-sticky warm reuse + park on the turn path"
```

---

## Task 4: Reaper + serve flags

**Files:** Modify `bin/meridian-cli/src/main.rs`, `crates/meridian/src/pooled_runner.rs` (the `pooled_runner` signature already took `max_parked` in Task 3; add nothing new there besides `reap_parked`). Test: a unit test for the reaper hand-off is covered by Task 2's `reap` test; the serve wiring is smoke-tested.

**Interfaces:** `serve` flags `--max-parked` (default 8) and `--park-ttl-secs` (default 300). A reaper tokio task: every `min(ttl, 60)` seconds, call `runner.reap_parked(Duration::from_secs(ttl))`.

- [ ] **Step 1: Implement**
- Add to `ServeArgs`: `#[arg(long, default_value_t = 8)] max_parked: usize` and `#[arg(long = "park-ttl-secs", default_value_t = 300)] park_ttl_secs: u64`. Update the `None` default-command construction.
- Pass `args.max_parked` into `pooled_runner(...)`.
- After building the runner, spawn the reaper:
```rust
{
    let runner = runner.clone();
    let ttl = std::time::Duration::from_secs(args.park_ttl_secs);
    let tick = std::time::Duration::from_secs(args.park_ttl_secs.min(60).max(5));
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(tick).await;
            runner.reap_parked(ttl);
        }
    });
}
```
- `reap_parked(&self, ttl)` in `pooled_runner.rs`: `for mut p in self.parked.reap(ttl) { p.shutdown().await; }` (or `tokio::spawn` the shutdowns so the reaper doesn't block; a simple sequential await is fine for a low-frequency reaper).

- [ ] **Step 2: Build + smoke** — `cargo build -p meridian-cli`; `meridian serve --help` shows `--max-parked` / `--park-ttl-secs`; `meridian serve --max-parked 4 --park-ttl-secs 30` binds and `/health` is ok.
- [ ] **Step 3: Update every other `pooled_runner(...)` call site** (tests) to pass the new `max_parked` arg — let the compiler enumerate them; pass `8`.
- [ ] **Step 4: Gate** — full `cargo test`, `cargo clippy --workspace --all-targets -- -D warnings`, all green.
- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(meridian-cli): --max-parked/--park-ttl-secs + background reaper for sticky processes"
```

---

## Self-Review

1. **Coverage:** is_alive (T1); ParkedStore park/take/LRU/reap (T2); warm/cold + park wiring + live multi-turn reuse (T3); reaper + flags + memory bound (T4). Streaming + passthrough parking explicitly deferred.
2. **Correctness:** worst-case == today (every warm miss/death/mismatch → cold path). Warm path sends the delta (server already does). Park key `(profile, session)` prevents cross-account reuse. Liveness checked on take. Memory bounded by `max_parked` (LRU) + TTL reaper + existing `--cap`.
3. **Type consistency:** `ParkedStore<CliProcess>` in the runner; `take`/`park`/`reap` signatures match T2; `Lease::take_proc` returns the `P` the park stores; `run_one_turn`'s existing `TurnResult.session_id` is the park key.
4. **Risk notes for the executor:** (a) `Lease::take_proc` must free the pool's global-cap slot but NOT shut the process down (it's being parked). (b) The live test asserts BOTH the codeword recall AND `parked().len()==1` after turn 1 — if len is 0, the cold path failed to park. (c) Re-run profile_e2e + stream_e2e `--ignored` to confirm no regression on the non-sticky paths. (d) Park under the result's `session_id`; if a turn returns no `session_id`, do NOT park (shut down) — it can't be resumed anyway.
