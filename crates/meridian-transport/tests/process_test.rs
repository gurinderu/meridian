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
        include_partial_messages: false, resume: None };
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
