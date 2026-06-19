# Phase 1 Discovery Findings

Captured: 2026-06-19  
CLI version: 2.1.177 (Claude Code)  
SDK version: @anthropic-ai/claude-agent-sdk (as installed in spike/sdk-probe)  

---

## 1. Confirmed base-flag vector

The SDK's `initialize()` method (inside class `Nh` in `sdk.mjs`) always starts with:

```
["--output-format", "stream-json", "--verbose", "--input-format", "stream-json"]
```

Remaining flags are appended conditionally based on options. The full set of flag names visible in the SDK source (from `grep -oE '"--[a-z-]+"' sdk.mjs | sort -u`):

```
--add-dir
--agent
--allow-dangerously-skip-permissions
--betas
--channels
--continue
--debug
--debug-file
--debug-to-stderr
--effort
--fallback-model
--fork-session
--hard-fail
--include-hook-events
--include-partial-messages
--input-format
--json-schema
--managed-settings
--max-budget-usd
--max-thinking-tokens
--max-turns
--mcp-config
--model
--no-session-persistence
--output-format
--permission-mode
--permission-prompt-tool
--plugin-dir
--plugin-dir-no-mcp
--porcelain
--resume
--resume-session-at
--session-id
--session-mirror
--strict-mcp-config
--task-budget
--thinking
--thinking-display
--tools
--verbose
```

**The mandatory 4 flags emitted unconditionally:**
1. `--output-format stream-json`
2. `--verbose`
3. `--input-format stream-json`
4. (none after; all others are conditional)

---

## 2. Isolation mechanism

The SDK isolates a `query()` call NOT via `--strict-mcp-config` or `--no-session-persistence`, but via SDK options passed into the process options object:

- `tools: []` → appends `--tools ""` (empty string = no built-in tools)
- `settingSources: []` → appends `--setting-sources=` (empty = ignore user/project/local settings)
- `plugins: []` → no `--plugin-dir` flags appended (no plugins loaded)
- `CLAUDE_CONFIG_DIR` set in the process environment → isolates local config/session storage

**Which spec-§5 flags the live CLI does NOT require for isolation:**
- `--strict-mcp-config` is NOT used by default; it is only appended when `strictMcpConfig: true` is passed explicitly
- `--no-session-persistence` is NOT used by default; only appended when `persistSession: false`

The original Meridian spike uses `tools:[]` / `settingSources:[]` / `plugins:[]` + an isolated `CLAUDE_CONFIG_DIR`, NOT the two flags above.

**Default `settingSources` (when not overridden):** `["user", "project", "local"]`

---

## 3. Observed message types in streaming_turn.ndjson

Fixture: `crates/meridian-transport/tests/fixtures/streaming_turn.ndjson` (25 lines)

| type | subtype | Notes |
|------|---------|-------|
| `system` | `hook_started` | Emitted per hook before execution (6 in this run) |
| `system` | `hook_response` | Hook execution result (7 in this run) |
| `system` | `init` | First protocol message; contains model, session_id, tools list, mcp_servers, permissionMode, claude_code_version |
| `system` | `status` | Short status update (e.g. `"status":"requesting"`) |
| `stream_event` | (none) | Raw Anthropic API streaming events (message_start, content_block_delta, message_delta, etc.) emitted when `--include-partial-messages` is active |
| `assistant` | (none) | Assembled assistant message with full content array |
| `rate_limit_event` | (none) | Rate-limit/overage metadata |
| `result` | `success` | Turn completion; contains `result` (text), `stop_reason`, `duration_ms`, `total_cost_usd`, `usage` |

### Example lines (truncated to 250 chars)

**system/init:**
```json
{"type":"system","subtype":"init","cwd":"...","session_id":"8dfcd442-...","tools":[...],"mcp_servers":[],"model":"claude-opus-4-8[1m]","permissionMode":"bypassPermissions","claude_code_version":"2.1.177",...}
```

**assistant:**
```json
{"type":"assistant","message":{"model":"claude-opus-4-8","id":"msg_01JP...","type":"message","role":"assistant","content":[{"type":"text","text":"PONG"}],"stop_reason":null,...}}
```

**stream_event (partial delta):**
```json
{"type":"stream_event","event":{"type":"message_start","message":{"model":"claude-opus-4-8","id":"msg_01JP...","type":"message","role":"assistant","content":[],...}}}
```

**result:**
```json
{"type":"result","subtype":"success","is_error":false,"duration_ms":2190,"num_turns":1,"result":"PONG","stop_reason":"end_turn","session_id":"8dfcd442-...",...}
```

---

## 4. MCP control protocol shapes (mcp_roundtrip.ndjson)

Fixture: `crates/meridian-transport/tests/fixtures/mcp_roundtrip.ndjson` (2 lines)

These are the `control_request` shapes the CLI sends when it needs to drive an in-process MCP server (`type:"sdk"`):

**tools/list request:**
```json
{"type":"control_request","request_id":"r1","request":{"subtype":"mcp_message","server_name":"spike","message":{"jsonrpc":"2.0","id":1,"method":"tools/list"}}}
```

**tools/call request:**
```json
{"type":"control_request","request_id":"r2","request":{"subtype":"mcp_message","server_name":"spike","message":{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"ping","arguments":{}}}}}
```

The `control_response` shape sent back by the Rust process:
```json
{"type":"control_response","response":{"subtype":"success","request_id":"r1","response":{"mcp_response":{"jsonrpc":"2.0","id":1,"result":{...}}}}}
```

Note: The rust-spike (at `/Users/gurinderu/projects/fluencelabs/meridian/rust-spike`) outputs human-summarized stdout, not raw NDJSON. These shapes were verified from the spike's source code at `rust-spike/src/main.rs` and match the documented protocol shapes from the original session notes.

---

## 5. Environment / spawn observations

- `CLAUDE_CODE_ENTRYPOINT` is set to `"sdk-ts"` by the SDK before spawning
- `NODE_OPTIONS` is deleted from the environment before spawning  
- `DEBUG` env var is deleted unless `DEBUG_CLAUDE_AGENT_SDK` is set
- The CLI inherits `process.env` from the spawning Node.js process, with overrides for `CLAUDE_CONFIG_DIR` when isolation is needed

---

## 6. Fixture summary

| File | Lines | Types present |
|------|-------|---------------|
| `init.ndjson` | 1 | `system/init` |
| `streaming_turn.ndjson` | 25 | `system/hook_started`, `system/hook_response`, `system/init`, `system/status`, `stream_event`, `assistant`, `rate_limit_event`, `result/success` |
| `mcp_roundtrip.ndjson` | 2 | `control_request` (mcp_message: tools/list, tools/call) |
