use serde_json::{json, Value};

pub trait ToolRegistry: Send + Sync {
    fn list(&self) -> Vec<Value>;
    fn call(&self, name: &str, args: &Value) -> Value;

    /// Decision for a PreToolUse hook event (passthrough captures + blocks here).
    /// Default: a no-op `{}` (the SDK rejects `undefined`; `{}` is the no-op).
    fn on_pre_tool_use(&self, _tool_name: &str, _tool_input: &Value, _tool_use_id: &str) -> Value {
        json!({})
    }
    /// Whether `initialize` should register a PreToolUse hook (passthrough mode).
    fn wants_pre_tool_use_hook(&self) -> bool {
        false
    }
    /// In-process ("sdk") MCP server names to register in `initialize`.
    fn sdk_mcp_servers(&self) -> Vec<String> {
        Vec::new()
    }
}
