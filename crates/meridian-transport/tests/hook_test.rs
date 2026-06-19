use serde_json::{json, Value};
use meridian_transport::control::handle_control_request;
use meridian_transport::mcp::ToolRegistry;

struct CapturingTools { captured: std::sync::Mutex<Vec<Value>> }
impl ToolRegistry for CapturingTools {
    fn list(&self) -> Vec<Value> { vec![] }
    fn call(&self, _n: &str, _a: &Value) -> Value { json!({}) }
    fn on_pre_tool_use(&self, tool_name: &str, tool_input: &Value, tool_use_id: &str) -> Value {
        if tool_name == "ToolSearch" { return json!({}); }
        self.captured.lock().unwrap().push(json!({"id":tool_use_id,"name":tool_name,"input":tool_input}));
        json!({"decision":"block","reason":"forwarded"})
    }
}

#[test]
fn hook_callback_blocks_and_captures() {
    let tools = CapturingTools { captured: std::sync::Mutex::new(vec![]) };
    let req = json!({
        "subtype":"hook_callback",
        "callback_id":"pre-tool-use",
        "tool_use_id":"tu_1",
        "input":{"tool_name":"mcp__oc__edit_file","tool_input":{"path":"foo.txt"}}
    });
    let resp = handle_control_request("rq", &req, &tools);
    assert_eq!(resp["type"], "control_response");
    assert_eq!(resp["response"]["request_id"], "rq");
    assert_eq!(resp["response"]["response"]["decision"], "block");
    let cap = tools.captured.lock().unwrap();
    assert_eq!(cap.len(), 1);
    assert_eq!(cap[0]["name"], "mcp__oc__edit_file");
    assert_eq!(cap[0]["id"], "tu_1");
}

#[test]
fn hook_callback_toolsearch_is_noop_not_block() {
    let tools = CapturingTools { captured: std::sync::Mutex::new(vec![]) };
    let req = json!({"subtype":"hook_callback","tool_use_id":"x","input":{"tool_name":"ToolSearch","tool_input":{}}});
    let resp = handle_control_request("rq", &req, &tools);
    assert!(resp["response"]["response"].as_object().unwrap().is_empty(), "ToolSearch -> {{}} no-op");
    assert!(tools.captured.lock().unwrap().is_empty());
}
