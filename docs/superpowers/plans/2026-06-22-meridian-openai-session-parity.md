# OpenAI Session Parity (resume + sticky) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Give the OpenAI-compatible `/v1/chat/completions` path the same session continuity the Anthropic `/v1/messages` path already has (Phase B / B2): resume (send only the delta + reuse the live parked process on continuations) instead of re-sending the entire conversation history every turn.

**Architecture:** Today `chat_completions` (both stream and non-stream) calls the runner with `resume: None` and never stores a session. It puts the whole conversation history into the *system* prompt (`<conversation_history>…</conversation_history>`) and sends the last user message as the prompt — correct, but it re-sends everything each turn and cold-spawns. The runner already supports resume + session-sticky for BOTH `run_turn` and `run_stream` (Phase B/B2). All that's missing is the OpenAI handler doing the session bookkeeping the Anthropic handler already does: compute `resume` from a fingerprint of the conversation, send the delta on resume (and drop the history-in-system block), pass the message list + resume to the runner, and store `SessionStore[fingerprint(messages + assistant_reply)] = session_id` after the turn. The fingerprint is computed over the OpenAI message Values directly (`fingerprint` only reads `role` + text, which OpenAI messages have), so it is self-consistent across OpenAI turns.

**Tech Stack:** Rust, the existing `openai`/`session`/runner machinery.

## Global Constraints

- **Self-consistent within the OpenAI protocol.** Fingerprint over the raw OpenAI `messages` Values (each turn the client resends the same array, so the prefix fingerprint is stable). The stored session_id is `claude`'s — usable by `--resume`. (Anthropic and OpenAI conversations fingerprint over different message shapes; that's fine — a client uses one protocol.)
- **Resume = drop the history-in-system block AND send only the delta.** On a resume hit, `claude`'s session already has the history, so: `system` = the system messages only (NO `<conversation_history>` block), `prompt` = the last user message content. On a miss (first turn / divergence), keep TODAY's behavior exactly (history block in system + last user prompt).
- **Store on every completing turn** (stream via `run_stream`, non-stream via the handler after `run_turn`), keyed by `fingerprint(messages + {"role":"assistant","content":reply_text})` — the SAME construction the Anthropic path and `run_stream` use, so streaming and non-stream OpenAI turns are interchangeable.
- **Worst case == today** (no resume → full history in system, cold spawn). This only ADDS a faster path.
- **No new crates.** No rustfmt (dense hand style; `main` fails `fmt --check`). Gate: `cargo build` + `cargo test` (default) + `cargo clippy --workspace --all-targets -- -D warnings`, all green. Live validation is `#[ignore]`.
- **No Claude attribution** on commits.

---

## File Structure

- **Modify** `crates/meridian/src/openai.rs` — add `pub fn openai_canonical_resumable(body, resume_known: bool) -> Result<(model, system, prompt), String>` (or extend `openai_to_canonical` with a `resume: bool` that omits the history block + returns the delta prompt). Keep the existing `openai_to_canonical` for the no-resume case OR fold both into one.
- **Modify** `crates/meridian/src/server.rs` — `chat_completions`: extract the `messages` array; compute `resume = sessions.get(&fingerprint(prefix))`; build `(model, system, prompt)` resume-aware; pass `resume` + `messages` + `sessions` to `run_stream`; for non-stream, pass `resume` into `run_turn` and store `SessionStore[fingerprint(messages+reply)] = session_id` after a successful turn (mirror the `messages` handler).

---

## Task 1: OpenAI resume + session store (stream + non-stream)

**Files:** Modify `crates/meridian/src/openai.rs`, `crates/meridian/src/server.rs`. Test: live `#[ignore]` tests in `bin/meridian-cli/tests/openai_e2e_test.rs`; an `openai.rs` unit test for the resume-aware conversion.

**Interfaces:**
- `openai.rs`: `pub fn openai_to_canonical_resumable(body: &Value, resume: bool) -> Result<(String, Option<String>, String), String>` — same as `openai_to_canonical` but when `resume == true`, the returned `system` OMITS the `<conversation_history>` block (system messages only) and `prompt` is the last user message (the delta). When `resume == false`, identical to today. (Refactor: have `openai_to_canonical` delegate with `resume=false`.)
- `server.rs` `chat_completions`: read `messages = body["messages"]` (array, non-empty, has a user — else 400 as today via openai_to_canonical's error); `let last_user_idx = messages.iter().rposition(role=="user")`; `let resume = state.sessions.get(&fingerprint(&messages[..last_user_idx]))`; `let (model, system, prompt) = openai_to_canonical_resumable(&body, resume.is_some())?`; thread `resume` + `messages.to_vec()` + `state.sessions.clone()` to `run_stream` (streaming) and `run_turn` (non-stream, via `TurnRequest.resume`).
  - Non-stream: after `Ok(r)`, if `r.session_id` is `Some(sid)`, build `convo = messages + {"role":"assistant","content": reply_text_from(r.message)}` and `state.sessions.insert(fingerprint(&convo), sid)` — copy the exact reply_text extraction + convo construction from the `messages` handler.

- [ ] **Step 1: Write a unit test for the resume-aware conversion**

```rust
// crates/meridian/tests/openai_resume_test.rs
use meridian::openai::{openai_to_canonical, openai_to_canonical_resumable};
use serde_json::json;

#[test]
fn resume_omits_history_block_and_sends_delta() {
    let body = json!({"model":"sonnet","messages":[
        {"role":"user","content":"first"},
        {"role":"assistant","content":"reply one"},
        {"role":"user","content":"second"}]});
    // no-resume: history goes into system, prompt is the last user msg
    let (_m, sys_cold, prompt_cold) = openai_to_canonical(&body).unwrap();
    assert_eq!(prompt_cold, "second");
    assert!(sys_cold.as_deref().unwrap_or("").contains("conversation_history"));
    assert!(sys_cold.as_deref().unwrap_or("").contains("first"));
    // resume: no history block; prompt is still the delta (last user)
    let (_m2, sys_warm, prompt_warm) = openai_to_canonical_resumable(&body, true).unwrap();
    assert_eq!(prompt_warm, "second");
    assert!(!sys_warm.as_deref().unwrap_or("").contains("conversation_history"));
    // openai_to_canonical == resumable(false)
    let (_m3, sys_cold2, _p) = openai_to_canonical_resumable(&body, false).unwrap();
    assert_eq!(sys_cold2, sys_cold);
}
```

- [ ] **Step 2: Run to verify failure** — `openai_to_canonical_resumable` doesn't exist.
- [ ] **Step 3: Implement.** In `openai.rs`, refactor `openai_to_canonical` so the `<conversation_history>` block is only appended when `!resume`, exposing `openai_to_canonical_resumable(body, resume)`; `openai_to_canonical(body)` = `openai_to_canonical_resumable(body, false)`. In `server.rs` `chat_completions`, compute `resume` from the messages prefix fingerprint, call the resumable conversion, thread `resume`/`messages`/`sessions` into both runner calls, and add the non-stream session store mirroring the `messages` handler.

- [ ] **Step 4: Write the live tests (`#[ignore]`)** in `bin/meridian-cli/tests/openai_e2e_test.rs`:
  - non-stream multi-turn recall (codeword set turn 1, asked turn 2 — recalls) AND a session is stored (`sessions.len_for_test() >= 1` after turn 1).
  - streaming multi-turn recall (accumulate OpenAI `choices[].delta.content` from the SSE chunks — NOT a raw `contains`, since text splits across chunks).

- [ ] **Step 5: Run** the default suite (green; live `#[ignore]`), then the live tests `cargo test -p meridian-cli --test openai_e2e_test -- --ignored`. Re-run `profile_e2e`/`stream_e2e` `--ignored` for regression. clippy clean.
- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(meridian): OpenAI /v1/chat/completions resume + session store (parity with /v1/messages)"
```

---

## Self-Review

1. **Coverage:** resume-aware OpenAI conversion (drop history block + delta) + session store on stream and non-stream. Reuses run_turn/run_stream's existing resume + sticky.
2. **Correctness:** worst case == today (no resume → history-in-system + cold spawn). Fingerprint over OpenAI messages is stable across turns (client resends the array). Store construction byte-identical to the Anthropic/`run_stream` path. Streaming reply accumulation parses OpenAI `choices[].delta.content`.
3. **Type consistency:** `fingerprint`/`message_text_pub` work on OpenAI message Values (role + content). `run_stream`/`run_turn` already take `resume`/`messages`/`sessions`.
4. **Risk notes:** (a) the resume-prefix fingerprint must match the convo stored last turn — both use the raw OpenAI message Values, so they agree as long as the client echoes the prior assistant reply (same assumption as the Anthropic path). (b) On a resume hit, the history-in-system block MUST be dropped (else doubled context). (c) the live streaming test must accumulate `delta.content`, not substring-match the raw SSE body.
