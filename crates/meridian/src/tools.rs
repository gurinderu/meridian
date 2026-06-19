use serde_json::{json, Value};

const OC_PREFIX: &str = "mcp__oc__";

/// Convert Anthropic request `tools` into in-process MCP tool definitions.
pub fn anthropic_tools_to_mcp_defs(tools: &[Value]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| {
            let name = t.get("name").and_then(Value::as_str).unwrap_or("");
            let description = t
                .get("description")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(name);
            let input_schema = t.get("input_schema").cloned().unwrap_or_else(|| json!({}));
            json!({ "name": name, "description": description, "inputSchema": input_schema })
        })
        .collect()
}

/// Strip the passthrough MCP prefix (`mcp__oc__`) from a captured tool name.
pub fn strip_oc_prefix(name: &str) -> &str {
    name.strip_prefix(OC_PREFIX).unwrap_or(name)
}
