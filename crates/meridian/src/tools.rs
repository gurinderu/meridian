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

fn tool_result_content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// If a user message carries `tool_result` blocks, render them as a plain-text
/// result message (the model's blocked tool_use was already resolved by the
/// deny, so a structured tool_result is ignored — we feed the raw content).
/// Returns `None` for messages with no tool_result blocks.
pub fn unwrap_tool_results(user_msg: &Value) -> Option<String> {
    let arr = user_msg.get("content")?.as_array()?;
    let results: Vec<String> = arr
        .iter()
        .filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
        .map(|b| {
            let id = b.get("tool_use_id").and_then(Value::as_str).unwrap_or("");
            let text = tool_result_content_text(b.get("content"));
            format!("The result of your tool call (id {id}) is:\n{text}")
        })
        .collect();
    if results.is_empty() {
        return None;
    }
    Some(format!(
        "{}\n\nUse these tool results to answer the user's request.",
        results.join("\n\n")
    ))
}
