# Meridian (Rust)

A single-binary HTTP proxy that exposes your **Claude Max subscription** through the
Anthropic and OpenAI APIs, so third-party tools that speak either protocol can use
Claude via your subscription instead of a pay-per-token API key.

This is a Rust port of [rynfar/meridian](https://github.com/rynfar/meridian) (TypeScript/Bun).
It is a faithful 1:1 replacement on the request path, multi-account, and management
surfaces — built as one static binary.

## How it works

Meridian does **not** call the Anthropic API directly. It spawns the real `claude`
CLI (the sanctioned Max-subscription client) in SDK streaming mode and speaks its
NDJSON stream-json + bidirectional control protocol over stdio — reimplemented
natively in Rust. Your subscription auth is handled by the CLI (OAuth via the
system keychain); the `ANTHROPIC_API_KEY` your client sends is just a placeholder.

```
client (Anthropic/OpenAI API)  ──►  meridian  ──►  spawns `claude` CLI  ──►  Anthropic
                                     (this proxy)    (your subscription)
```

A warm pool of `claude` processes, partitioned per account ("profile"), serves
requests; sessions can resume across turns; client tool calls are surfaced back to
the caller.

**Requirements:** the `claude` CLI on `PATH` (or `--claude <path>`), `curl`, and on
macOS `/usr/bin/security`. You must be logged in (`claude auth login`) — or add a
profile (see below).

## Build & run

```bash
cargo build --release          # produces ./target/release/meridian (~3.9 MB, static)
./target/release/meridian serve            # binds 127.0.0.1:8787 (loopback, default)
./target/release/meridian serve --port 9000 --cap 16 --claude /path/to/claude
./target/release/meridian serve --host 0.0.0.0   # expose on the network (see warning)
```

> By default `serve` binds **loopback** (`127.0.0.1`) — not reachable from the
> network. Pass `--host 0.0.0.0` to expose it, and set `MERIDIAN_API_KEY` (see below)
> when you do.

Point a client at it:

```bash
ANTHROPIC_BASE_URL=http://127.0.0.1:8787 ANTHROPIC_API_KEY=placeholder claude
# or any OpenAI client → http://127.0.0.1:8787/v1/chat/completions
```

### Commands

| Command | Purpose |
|---|---|
| `meridian serve [--port 8787] [--host 127.0.0.1] [--cap 10] [--claude claude]` | Run the proxy (default command). |
| `meridian status [--port 8787]` | Check whether a proxy is responding. |
| `meridian install` / `uninstall` | Install/remove the OS background service (launchd on macOS, systemd user unit on Linux). |
| `meridian profile list` | List configured profiles. |
| `meridian profile add <id> --oauth-token [TOKEN]` | Add a profile from a `claude setup-token` value. |
| `meridian profile add <id> [--headless]` | Add a profile via browser OAuth login (`claude auth login`, or `--headless` paste-code). |
| `meridian profile login <id> [--headless]` | Re-authenticate an existing claude-max profile. |
| `meridian profile use <id>` | Switch the active profile on the running proxy. |
| `meridian profile remove <id>` | Remove a profile. |

### Background service

`meridian install` writes a launchd plist (macOS) or systemd **user** unit (Linux)
that keeps one `serve` running; the HTTP server *is* the service (no custom daemon).
`meridian status` / `curl /health` verify it.

## Endpoints

Everything except `/health` sits behind the optional API-key gate (see below).

| Method | Path | |
|---|---|---|
| GET | `/health` | liveness (always open) |
| POST | `/v1/messages` | Anthropic Messages API (+ streaming, + tool passthrough) |
| POST | `/v1/chat/completions` | OpenAI Chat Completions (+ streaming, + tool_calls) |
| GET | `/v1/models` | model list |
| GET | `/v1/usage/quota` | Claude Max rate-limit snapshot (per bucket) |
| GET | `/profiles/list` · POST `/profiles/active` | profile management |
| POST | `/auth/refresh` | refresh the active/`x-meridian-profile` account's OAuth token |

## Profiles (multi-account)

A profile is a named auth context. Configure via `~/.config/meridian/profiles.json`
(picked up live, ~5s) or the `MERIDIAN_PROFILES` env var (a JSON array). Select
per-request with the `x-meridian-profile: <id>` header, or set an active default
with `meridian profile use <id>`.

```json
[
  { "id": "personal", "type": "claude-max", "claudeConfigDir": "/Users/me/.config/meridian/profiles/personal" },
  { "id": "work",     "type": "oauth-token", "oauthToken": "sk-ant-oat01-…" },
  { "id": "api",      "type": "api", "apiKey": "sk-ant-…", "baseUrl": "https://api.anthropic.com" }
]
```

Resolution order: `x-meridian-profile` header → active profile → first configured → default.
Each profile's credentials overlay the spawned CLI's environment; the warm pool stays
partitioned per account.

## Configuration (env)

| Variable | Effect |
|---|---|
| `MERIDIAN_API_KEY` | When set, all routes except `/health` require it via `x-api-key` or `Authorization: Bearer`. Unset = open. |
| `MERIDIAN_PROFILES` | JSON array of profiles (overrides on-disk `profiles.json`; disables live disk discovery). |
| `MERIDIAN_HOST` / `MERIDIAN_PORT` | Host/port the `profile use` CLI targets. |

`~/.config/meridian/settings.json` persists the active profile across restarts
(written `0o600`); the credential store and `profiles.json` are also `0o600` /
keychain-backed.

## Performance

- **Single static binary, ~3.9 MB** — no runtime to install (vs the Bun original,
  which ships a ~90 MB JS runtime plus `node_modules`).
- **Instant cold start** (`--help` ≈ 0 ms; `serve` binds immediately) — native code,
  no JS engine warmup.
- A background OAuth-refresh scheduler keeps the default account's refresh token warm
  even on an idle proxy.

> End-to-end latency is dominated by the `claude` CLI + the upstream API, which both
> implementations share. The Rust win is in footprint, startup, and the proxy's own
> per-request overhead.

## Scope

Implemented 1:1 with the original on the request path, multi-account, and management
surfaces. **Deliberately not ported:** telemetry, runtime-JS plugins, and the web UI.

## Layout

- `crates/meridian-transport` — spawn / NDJSON codec / control protocol / process pool.
- `crates/meridian` — HTTP server, protocol translation, sessions, profiles, auth, quota, OAuth.
- `bin/meridian-cli` — the thin `meridian` CLI.

## Testing

```bash
cargo test                       # unit + handler suite
cargo test -- --ignored          # live end-to-end (requires an authenticated `claude`)
cargo clippy --workspace --all-targets -- -D warnings
```
