use std::collections::HashMap;
use std::sync::Arc;
use serde_json::json;
use meridian_transport::codec::CliMessage;
use meridian_transport::passthrough;
use meridian_transport::process::spawn;
use meridian_transport::spawn::SpawnConfig;

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn passthrough_captures_tool_use_and_turn_ends() {
    let dir = std::env::temp_dir().join(format!("meridian-pt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = SpawnConfig {
        config_dir: dir, model: None, mcp_config: Some(json!({"mcpServers":{"oc":{"type":"sdk","name":"oc"}}})),
        include_partial_messages: false, resume: None, max_turns: Some(3),
        env_overlay: Default::default(),
    };
    let tools = Arc::new(passthrough::new(vec![json!({
        "name":"edit_file","description":"Edit a file (client-executed).",
        "inputSchema":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}
    })]));

    let mut base: HashMap<String,String> = std::env::vars().collect();
    base.insert("CLAUDE_SECURESTORAGE_CONFIG_DIR".into(), String::new()); // keychain auth fix
    let mut p = spawn("claude", &cfg, &base, tools.clone()).await.unwrap();
    p.send_user_turn("Call the tool mcp__oc__edit_file with {\"path\":\"foo.txt\",\"content\":\"hi\"}. You MUST call that tool and nothing else.").await.unwrap();

    let mut got_result = false;
    while let Some(ev) = p.next_event().await {
        if let CliMessage::Result { .. } = ev { got_result = true; break; }
    }
    p.shutdown().await;

    assert!(got_result, "turn did not end");
    let cap = tools.captured();
    assert!(cap.iter().any(|c| c["name"].as_str().unwrap_or("").contains("edit_file")),
        "no edit_file tool_use captured; got {cap:?}");
}
