use std::sync::Arc;
use serde_json::{json, Value};
use meridian_transport::codec::CliMessage;
use meridian_transport::factory;
use meridian_transport::mcp::ToolRegistry;
use meridian_transport::pool::{IsolationKey, Pool};

struct NoTools;
impl ToolRegistry for NoTools {
    fn list(&self) -> Vec<Value> { vec![] }
    fn call(&self, _n: &str, _a: &Value) -> Value { json!({}) }
}

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn pooled_real_process_runs_a_turn() {
    let root = std::env::temp_dir().join(format!("meridian-factory-{}", std::process::id()));
    let f = factory::new("claude", root, Arc::new(NoTools));
    let pool = Pool::new(f, 2);
    let key = IsolationKey { profile_id: "default".into(), cwd: "/".into(), options_hash: 0, resume: None };

    let mut lease = pool.acquire(&key).await.unwrap().unwrap();
    lease.proc().send_user_turn("Reply with exactly: OK").await.unwrap();
    let mut saw_result = false;
    while let Some(ev) = lease.proc().next_event().await {
        if let CliMessage::Result { .. } = ev { saw_result = true; break; }
    }
    lease.proc().shutdown().await;
    assert!(saw_result);
}
