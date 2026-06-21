use std::collections::HashMap;
use std::sync::Arc;
use serde_json::{json, Value};
use meridian_transport::codec::CliMessage;
use meridian_transport::mcp::ToolRegistry;
use meridian_transport::process::spawn;
use meridian_transport::spawn::SpawnConfig;

struct PingTools;
impl ToolRegistry for PingTools {
    fn list(&self) -> Vec<Value> {
        vec![json!({"name":"ping","description":"p","inputSchema":{"type":"object","properties":{}}})]
    }
    fn call(&self, _n: &str, _a: &Value) -> Value {
        json!({"content":[{"type":"text","text":"PONG-FROM-RUST"}],"isError":false})
    }
}

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn live_streaming_turn_reaches_result() {
    let dir = std::env::temp_dir().join(format!("meridian-iso-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = SpawnConfig { config_dir: dir, model: None,
        mcp_config: Some(json!({"mcpServers":{"spike":{"type":"sdk","name":"spike"}}})),
        include_partial_messages: false, resume: None, max_turns: None, env_overlay: Default::default() };
    let base: HashMap<String,String> = std::env::vars().collect();
    let mut p = spawn("claude", &cfg, &base, Arc::new(PingTools)).await.unwrap();
    p.send_user_turn("Reply with exactly: PONG").await.unwrap();

    let mut got_result = false;
    while let Some(ev) = p.next_event().await {
        if let CliMessage::Result { .. } = ev { got_result = true; break; }
    }
    p.shutdown().await;
    assert!(got_result, "did not observe a result event");
}

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
