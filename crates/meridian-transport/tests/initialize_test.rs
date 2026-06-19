use serde_json::{json, Value};
use meridian_transport::mcp::ToolRegistry;
use meridian_transport::spawn::build_initialize;

struct Passthroughish;
impl ToolRegistry for Passthroughish {
    fn list(&self) -> Vec<Value> { vec![] }
    fn call(&self, _n: &str, _a: &Value) -> Value { json!({}) }
    fn wants_pre_tool_use_hook(&self) -> bool { true }
    fn sdk_mcp_servers(&self) -> Vec<String> { vec!["oc".into()] }
}
struct Bare;
impl ToolRegistry for Bare {
    fn list(&self) -> Vec<Value> { vec![] }
    fn call(&self, _n: &str, _a: &Value) -> Value { json!({}) }
}

#[test]
fn initialize_includes_servers_and_hook() {
    let v = build_initialize(&Passthroughish).expect("some");
    assert_eq!(v["type"], "control_request");
    assert_eq!(v["request"]["subtype"], "initialize");
    assert_eq!(v["request"]["sdkMcpServers"][0], "oc");
    assert_eq!(v["request"]["hooks"]["PreToolUse"][0]["hookCallbackIds"][0], "pre-tool-use");
    assert_eq!(v["request"]["hooks"]["PreToolUse"][0]["matcher"], "");
}

#[test]
fn initialize_is_none_for_bare_registry() {
    assert!(build_initialize(&Bare).is_none());
}
