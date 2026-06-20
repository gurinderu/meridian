use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use crate::codec::{parse_line, CliMessage};
use crate::control::handle_control_request;
use crate::mcp::ToolRegistry;
use crate::spawn::{build_args, build_env, build_initialize, SpawnConfig};

pub struct CliProcess {
    child: Child,
    stdin_tx: mpsc::Sender<String>,
    events_rx: mpsc::Receiver<CliMessage>,
}

pub async fn spawn(
    exe: &str,
    cfg: &SpawnConfig,
    base_env: &HashMap<String, String>,
    tools: Arc<dyn ToolRegistry>,
) -> std::io::Result<CliProcess> {
    // env_clear first so build_env's output is the COMPLETE child environment.
    // Without it, `.envs()` only *adds* to the inherited parent env, so the
    // STRIP in build_env is ineffective for any var it doesn't re-set: a host
    // ANTHROPIC_BASE_URL / CLAUDE_CODE_OAUTH_TOKEN / pre-set
    // CLAUDE_SECURESTORAGE_CONFIG_DIR would leak through and defeat profile
    // isolation. build_env starts from the full `base_env` snapshot, so PATH,
    // HOME, etc. are preserved.
    let mut child = Command::new(exe)
        .env_clear()
        // Kill the child if CliProcess is dropped without an explicit shutdown()
        // — tokio's Child does NOT kill on drop otherwise, which would orphan
        // the `claude` process and its pipes.
        .kill_on_drop(true)
        .args(build_args(cfg))
        .envs(build_env(cfg, base_env))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    // Drain stderr to EOF. The OS stderr pipe buffer is ~64KB; if nothing reads
    // it and `claude` writes that much, the child blocks on write — which stalls
    // stdout/NDJSON too and wedges the whole turn with no upper bound. Log at
    // debug for diagnostics.
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if !line.trim().is_empty() {
                tracing::debug!("meridian-transport: claude stderr: {line}");
            }
        }
    });

    // Single writer task: serialize all stdin writes through one channel.
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(64);
    tokio::spawn(async move {
        while let Some(mut line) = stdin_rx.recv().await {
            line.push('\n');
            if stdin.write_all(line.as_bytes()).await.is_err() {
                tracing::warn!("meridian-transport: stdin writer task exiting after write error");
                break;
            }
            let _ = stdin.flush().await;
        }
    });

    // Reader task: parse NDJSON, answer control_requests, forward the rest.
    let (events_tx, events_rx) = mpsc::channel::<CliMessage>(256);
    let writer = stdin_tx.clone();
    let tools_for_reader = tools.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(msg) = parse_line(&line) else { continue };
            if let CliMessage::ControlRequest { request_id, request } = &msg {
                let resp = handle_control_request(request_id, request, tools_for_reader.as_ref());
                let _ = writer.send(resp.to_string()).await;
                continue;
            }
            if events_tx.send(msg).await.is_err() {
                tracing::debug!("meridian-transport: reader task exiting, event consumer dropped");
                break;
            }
        }
    });

    // Send initialize if the registry wants it.
    if let Some(init) = build_initialize(tools.as_ref()) {
        let _ = stdin_tx.send(init.to_string()).await;
    }

    Ok(CliProcess { child, stdin_tx, events_rx })
}

impl CliProcess {
    pub async fn send_user_turn(&self, content: &str) -> std::io::Result<()> {
        let line = serde_json::json!({
            "type": "user",
            "message": { "role": "user", "content": content }
        })
        .to_string();
        self.stdin_tx
            .send(line)
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin closed"))
    }

    /// Receive the next event from the CLI process.
    ///
    /// **Important**: The caller MUST continuously poll this method and drive each turn to
    /// completion (until a `result` event or `None` is returned) before dropping the process.
    /// The events channel is bounded; a stalled consumer fills it and blocks the reader task,
    /// which blocks in-flight control_request/tool responses and causes timeouts.
    pub async fn next_event(&mut self) -> Option<CliMessage> {
        self.events_rx.recv().await
    }

    pub async fn shutdown(&mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}
