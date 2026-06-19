use std::sync::Mutex;

use serde_json::{json, Value};

use crate::mcp::ToolRegistry;

const SERVER: &str = "oc";
const BLOCK_REASON: &str = "This tool call has been forwarded to the client for execution. \
The result will be delivered in a future turn. Do not retry, do not call additional tools, \
and do not generate further text — end your turn now.";

/// A dynamic, per-request tool registry for passthrough: the client's tools are
/// registered as an in-process MCP server, and the PreToolUse hook captures each
/// `tool_use` and blocks execution so the model surfaces it to the client.
pub struct PassthroughTools {
    tool_defs: Vec<Value>,
    captured: Mutex<Vec<Value>>,
}

pub fn new(tool_defs: Vec<Value>) -> PassthroughTools {
    PassthroughTools { tool_defs, captured: Mutex::new(Vec::new()) }
}

impl PassthroughTools {
    pub fn captured(&self) -> Vec<Value> {
        self.captured.lock().unwrap().clone()
    }
}

impl ToolRegistry for PassthroughTools {
    fn list(&self) -> Vec<Value> {
        self.tool_defs.clone()
    }
    fn call(&self, _name: &str, _args: &Value) -> Value {
        // Never reached: the PreToolUse hook blocks before execution.
        json!({ "content": [{ "type": "text", "text": "blocked" }], "isError": true })
    }
    fn sdk_mcp_servers(&self) -> Vec<String> {
        vec![SERVER.to_string()]
    }
    fn wants_pre_tool_use_hook(&self) -> bool {
        true
    }
    fn on_pre_tool_use(&self, tool_name: &str, tool_input: &Value, tool_use_id: &str) -> Value {
        // Let the SDK handle ToolSearch internally (deferred tool loading).
        if tool_name == "ToolSearch" {
            return json!({});
        }
        self.captured.lock().unwrap().push(json!({
            "id": tool_use_id, "name": tool_name, "input": tool_input.clone()
        }));
        json!({ "decision": "block", "reason": BLOCK_REASON })
    }
}
