# Phase B2 — Streaming Resume + Session-Sticky Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Bring the streaming path up to parity with the non-stream path: session continuity (resume — send only the delta instead of re-flattening the whole conversation each turn) and session-sticky process reuse (no cold-start on streaming continuations). Builds on Phase B's `ParkedStore` and the streaming-context fix.

**Architecture:** Today the streaming path flattens the full conversation every turn and spawns fresh (no resume, no session store). The non-stream path already does resume + stores `SessionStore[fingerprint(convo+reply)] = session_id`. The reason streaming couldn't reuse the same trick: the `session_id` arrives in the CLI's `system/init` message (`CliMessage::Init`), which is NOT one of the Anthropic stream events forwarded to the client — so only `run_stream`'s pump can see it. Therefore the session bookkeeping must move INTO the pump: capture `session_id` from `Init`, accumulate the assistant reply text from the forwarded `content_block_delta` events, and on stream completion store `SessionStore[fingerprint(messages + assistant_reply)] = session_id` — using the SAME convo/fingerprint construction the non-stream path uses, so the two are interchangeable. With the session stored, the server resolves `resume` for the next streaming turn (send delta) and the pump can reuse a parked process (Phase B's `ParkedStore`, keyed `(profile, session_id)`).

**Tech Stack:** Rust, tokio, the existing `ParkedStore` / `CliProcess` / `SessionStore` / `fingerprint`.

## Global Constraints

- **Mirror the non-stream session convention EXACTLY** so a conversation can switch between stream/non-stream and stay consistent: store under `fingerprint(messages + {"role":"assistant","content":<reply_text>})` where `reply_text` is the concatenation of the assistant text. (Non-stream: server.rs builds `convo = messages.to_vec(); convo.push(json!({"role":"assistant","content":reply_text})); sessions.insert(fingerprint(&convo), sid)`.)
- **Resume sends the delta; no-resume flattens** — same as non-stream. The server computes `resume = sessions.get(fingerprint(prefix))` and `prompt = delta(last_user) if resume else flatten_conversation(messages)`.
- **Reply accumulation:** sum the text from forwarded stream events. An Anthropic `content_block_delta` event is `{"type":"content_block_delta","delta":{"type":"text_delta","text":"…"}}`; accumulate `event["delta"]["text"]` when `event["type"] == "content_block_delta"`. (Be defensive: ignore non-text deltas.)
- **Sticky (Task 2):** reuse Phase B's `ParkedStore<CliProcess>` on `PooledRunner`. Warm: on `resume=Some(sid)`, `parked.take(profile, sid)` → if alive, send delta + pump + re-park under the new `session_id`. Cold: spawn (with `--resume sid` when present) → pump → park under the result `session_id`. Park ONLY when a `session_id` was captured and the proc is alive; otherwise shutdown. Worst case == today's spawn-per-stream.
- **Both streaming call sites** (`/v1/messages` and `/v1/chat/completions`) go through the new `run_stream` signature. Factor the resume/prompt computation into a shared helper so both are resume-aware and consistent.
- **No new crates.** No rustfmt (dense hand style; `main` fails `fmt --check`) — match surrounding style, do NOT run `cargo fmt`. Gate: `cargo build` + `cargo test` (default) + `cargo clippy --workspace --all-targets -- -D warnings`, all green. Live validation is `#[ignore]`.
- **No Claude attribution** on commits.

---

## File Structure

- **Modify** `crates/meridian/src/server.rs` — `StreamRunner::run_stream` signature gains `resume: Option<String>`, `messages: Vec<Value>`, `sessions: Arc<SessionStore>`; a shared `fn stream_prompt(messages, sessions) -> (Option<String> /*resume*/, String /*prompt*/)` used by both streaming branches; update both call sites.
- **Modify** `crates/meridian/src/pooled_runner.rs` — `run_stream` impl: resume-aware spawn, capture session_id, accumulate reply, store session (Task 1); warm/cold + park (Task 2).
- **Modify** all `run_stream` call sites + `StreamRunner` stub impls in tests to the new signature.

---

## Task 1: Streaming resume + session store (no sticky yet)

**Files:** Modify `crates/meridian/src/server.rs`, `crates/meridian/src/pooled_runner.rs`; update the `StreamRunner` stubs in `crates/meridian/tests/*` and the call in `bin/meridian-cli/tests/stream_e2e_test.rs`. Test: a live `#[ignore]` test in `bin/meridian-cli/tests/stream_e2e_test.rs`.

**Interfaces:**
- `StreamRunner::run_stream(&self, model: String, system: Option<String>, prompt: String, profile: Option<String>, resume: Option<String>, messages: Vec<Value>, sessions: Arc<crate::session::SessionStore>) -> EventStream`.
- `server.rs`: `fn stream_prompt(messages: &[Value], sessions: &SessionStore) -> (Option<String>, String)` — returns `(resume, prompt)`: `prefix = messages[..last_user_idx]`; `resume = sessions.get(&fingerprint(prefix))`; `prompt = message_text_pub(last_user) if resume.is_some() else flatten_conversation(messages)`. (If there's no user message, the caller already 400s.)

- [ ] **Step 1: Write the live test (`#[ignore]`)** — append to `bin/meridian-cli/tests/stream_e2e_test.rs`:

```rust
#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn streaming_resume_stores_and_continues() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use meridian::server::router;
    use meridian::session::SessionStore;
    let root = std::env::temp_dir().join(format!("meridian-streamresume-{}", std::process::id()));
    let profiles = std::sync::Arc::new(meridian::profiles::ProfileStore::new(Vec::new(), std::env::temp_dir()));
    let rate_limit = Arc::new(meridian::rate_limit::RateLimitStore::new());
    let sessions = Arc::new(SessionStore::new());
    let app = router(Arc::new(pooled_runner("claude".into(), root, 2, profiles.clone(), rate_limit.clone(), 8)), sessions.clone(), profiles, rate_limit);

    // Turn 1 (streaming): set a codeword. After it completes, a session must be stored.
    let b1 = serde_json::json!({"model":"sonnet","stream":true,"messages":[
        {"role":"user","content":"Remember the codeword PERSIMMON5. Reply with just OK."}]});
    let r1 = app.clone().oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(b1.to_string())).unwrap()).await.unwrap();
    assert_eq!(r1.status(), StatusCode::OK);
    let _ = axum::body::to_bytes(r1.into_body(), usize::MAX).await.unwrap(); // drain to completion
    // give the pump's post-stream store a beat (the SSE body is fully drained above,
    // but the store happens in the spawned task right before the channel closes).
    assert_eq!(sessions.len_for_test(), 1, "turn 1 must store a streaming session");

    // Turn 2 (streaming continuation): echoes turn 1 -> resolves resume -> delta send.
    let b2 = serde_json::json!({"model":"sonnet","stream":true,"messages":[
        {"role":"user","content":"Remember the codeword PERSIMMON5. Reply with just OK."},
        {"role":"assistant","content":"OK"},
        {"role":"user","content":"What was the exact codeword?"}]});
    let r2 = app.oneshot(Request::post("/v1/messages").header("content-type","application/json")
        .body(Body::from(b2.to_string())).unwrap()).await.unwrap();
    let text = String::from_utf8(axum::body::to_bytes(r2.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
    assert!(text.contains("PERSIMMON5"), "streaming continuation recalls the codeword via resume: {text}");
}
```

> The test asserts a session was stored after a streaming turn. Add `pub fn len_for_test(&self) -> usize` to `SessionStore` (returns the map length) if no length accessor exists. If asserting store-count proves racy (the store happens just before the channel closes, which is before `to_bytes` returns — so it should be settled), keep it; otherwise drop that one assertion and rely on the turn-2 recall.

- [ ] **Step 2: Run to verify failure** — the new `run_stream` signature doesn't exist (compile error).

- [ ] **Step 3: Implement.**
- `server.rs`: change the `StreamRunner` trait sig; add `stream_prompt`; both streaming branches compute `(resume, prompt) = stream_prompt(messages, &state.sessions)` (after the no-user-message 400 guard) and call `run_stream(model, system, prompt, profile, resume, messages.to_vec(), state.sessions.clone())`. (For `/v1/chat/completions`, `messages` are the already-converted anthropic messages — pass those.)
- `pooled_runner.rs` `run_stream`: build `IsolationKey { profile_id: pid, resume: resume.clone() }`; the spawned task captures `session_id` from `CliMessage::Init`, accumulates `reply_text` from `CliMessage::StreamEvent` (`content_block_delta`→`delta.text`), and on the `Result`/stream-end, if `Some(sid)`: `let mut convo = messages; convo.push(json!({"role":"assistant","content":reply_text})); sessions.insert(crate::session::fingerprint(&convo), sid);`. Keep the existing `is_error`→`error_event`, the 300s timeout, and (for Task 1) the final `shutdown()+discard()`. The cold spawn already honors `key.resume` via the factory's `SpawnConfig.resume`.

- [ ] **Step 4: Run** the default suite (green; the live test is `#[ignore]`), then the live test: `cargo test -p meridian-cli --test stream_e2e_test -- --ignored`. Confirm turn 2 recalls the codeword AND (if kept) a session was stored. Re-run the non-stream `profile_e2e`/the existing stream tests `--ignored` for regression. clippy clean.
- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(meridian): streaming resume + session store (delta continuation, parity with non-stream)"
```

---

## Task 2: Streaming session-sticky reuse

**Files:** Modify `crates/meridian/src/pooled_runner.rs`. Test: a live `#[ignore]` reuse assertion (parked count) appended to `bin/meridian-cli/tests/sticky_e2e_test.rs`.

**Interfaces:** uses `self.parked` (`ParkedStore<CliProcess>`) + `self.max_parked` already on `PooledRunner`.

- [ ] **Step 1: Write the live reuse test (`#[ignore]`)** — a streaming variant of the non-stream sticky test: two streaming turns; after turn 1 `runner.parked().len() == 1`; turn 2 recalls the codeword (warm reuse). (Mirror `continuation_reuses_a_parked_process` but with `stream:true` requests, draining each SSE body to completion. Assert `parked().len()==1` after turn 1.)

- [ ] **Step 2: Run to verify** the new behavior isn't there yet (turn 1 does not park → `len()==0`).

- [ ] **Step 3: Implement** in `run_stream`'s spawned task, replacing the cold-only Task-1 flow:
  - **Warm:** if `resume` is `Some(sid)`, `self.parked.take(&pid, sid)`; if `Some(mut proc)` and `proc.is_alive()`: send the delta, pump (forward + accumulate + capture sid), store the session, then re-park under the new sid (`park`, shutting down any evicted); DONE. On a dead proc or a mid-stream error, fall back to the cold path.
  - **Cold:** acquire from the pool + spawn (with `--resume sid`), pump, store the session, then — if a `session_id` was captured and the proc is alive — `lease.take_proc()` + `park(pid, sid, proc, max_parked)` (shut down evicted) INSTEAD of `shutdown()+discard()`. Otherwise `shutdown()+discard()`.
  - Since `run_stream` returns the `EventStream` synchronously and the work happens in the spawned task, the warm/cold branching lives inside that task. Keep the `tx`/`ReceiverStream` wiring; the only change is process acquisition (parked vs pool) and disposition (park vs discard).

> Implementer: this mirrors `run_turn`'s warm/cold+park (Phase B) but in the streaming pump. The same correctness rules apply: park key `(profile, session_id)`; liveness-check on take; worst case == cold path. The `Lease::take_proc` (Phase B) frees the cap slot.

- [ ] **Step 4: Run** default suite green; live `sticky_e2e_test --ignored` (both the non-stream and the new streaming reuse case) + `stream_e2e_test --ignored`. Confirm `parked().len()==1` after a streaming turn 1 and the codeword recall on turn 2. clippy clean.
- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(meridian): session-sticky reuse on the streaming path"
```

---

## Self-Review

1. **Coverage:** streaming resume + session store mirroring non-stream (T1); streaming sticky warm/cold+park (T2). Both streaming call sites resume-aware via `stream_prompt`.
2. **Correctness:** the convo/fingerprint construction is byte-identical to the non-stream path (interchangeable sessions). Worst case == today (no parked proc / dead / error → cold spawn; the streaming-context flatten still applies when no resume). Reply accumulation is defensive (ignores non-text deltas).
3. **Type consistency:** `run_stream`'s new params (resume, messages, sessions) are threaded from both server branches; `fingerprint`/`message_text_pub`/`flatten_conversation` reused; `ParkedStore`/`Lease::take_proc` reused from Phase B.
4. **Risk notes:** (a) the session store happens in the spawned pump task on stream completion — ensure it runs BEFORE the task exits (after the `Result`/break, before the final disposition). (b) The live test's store-count assertion may race; the turn-2 recall is the load-bearing assertion. (c) Re-run BOTH non-stream (`profile_e2e`) and streaming (`stream_e2e`, `sticky_e2e`) `--ignored` after each task — the run_turn path must stay green. (d) park only when a session_id was captured AND the proc is alive.
