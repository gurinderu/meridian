use std::collections::HashMap;
use std::sync::Arc;
use serde_json::{json, Value};
use meridian_transport::codec::CliMessage;
use meridian_transport::factory::{self, EnvResolver, NoEnv};
use meridian_transport::mcp::ToolRegistry;
use meridian_transport::pool::{IsolationKey, Pool};

struct NoTools;
impl ToolRegistry for NoTools {
    fn list(&self) -> Vec<Value> { vec![] }
    fn call(&self, _n: &str, _a: &Value) -> Value { json!({}) }
}

/// Echoes the requested profile id into a sentinel env var.
struct FixedEnv;
impl EnvResolver for FixedEnv {
    fn overlay(&self, profile_id: &str) -> HashMap<String, String> {
        HashMap::from([("MERIDIAN_TEST_PROFILE".to_string(), profile_id.to_string())])
    }
}

#[test]
fn no_env_resolver_yields_empty_overlay() {
    assert!(NoEnv.overlay("anything").is_empty());
}

#[test]
fn resolver_overlay_is_keyed_by_profile_id() {
    assert_eq!(FixedEnv.overlay("work").get("MERIDIAN_TEST_PROFILE").map(String::as_str), Some("work"));
    // new_with_resolver must accept a custom resolver (compile-level wiring check).
    let root = std::env::temp_dir().join("meridian-factory-ctor");
    let _f = factory::new_with_resolver("claude", root, Arc::new(NoTools), Arc::new(FixedEnv));
}

#[tokio::test]
#[ignore = "requires a live, authenticated `claude` CLI"]
async fn pooled_real_process_runs_a_turn() {
    let root = std::env::temp_dir().join(format!("meridian-factory-{}", std::process::id()));
    let f = factory::new("claude", root, Arc::new(NoTools));
    let pool = Pool::new(f, 2);
    let key = IsolationKey { profile_id: "default".into(), resume: None };

    let mut lease = pool.acquire(&key).await.unwrap().unwrap();
    lease.proc().send_user_turn("Reply with exactly: OK").await.unwrap();
    let mut saw_result = false;
    while let Some(ev) = lease.proc().next_event().await {
        if let CliMessage::Result { .. } = ev { saw_result = true; break; }
    }
    lease.proc().shutdown().await;
    assert!(saw_result);
}

#[test]
fn safe_profile_segment_blocks_traversal() {
    use meridian_transport::factory::safe_profile_segment;
    // well-formed ids pass through unchanged
    assert_eq!(safe_profile_segment("work-1_x"), "work-1_x");
    // traversal / separators / absolute can't escape — no '/' or '..' survive
    for bad in ["../../etc", "/etc/passwd", "a/b", "..", "a/../b"] {
        let seg = safe_profile_segment(bad);
        assert!(!seg.contains('/') && seg != ".." && !seg.contains("../"),
            "segment {seg:?} from {bad:?} must be a single safe path component");
    }
    // empty -> non-empty fallback
    assert_eq!(safe_profile_segment(""), "default");
}
