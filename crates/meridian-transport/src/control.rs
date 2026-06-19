use serde_json::{json, Value};
use crate::mcp::ToolRegistry;

/// Build the `control_response` envelope for an inbound `control_request`.
/// Currently handles `mcp_message`; other subtypes get an empty success ack.
pub fn handle_control_request(request_id: &str, request: &Value, tools: &dyn ToolRegistry) -> Value {
    let inner = match request.get("subtype").and_then(Value::as_str) {
        Some("mcp_message") => mcp_inner(request, tools),
        Some("hook_callback") => {
            let input = &request["input"];
            tools.on_pre_tool_use(
                input.get("tool_name").and_then(Value::as_str).unwrap_or(""),
                input.get("tool_input").unwrap_or(&Value::Null),
                request.get("tool_use_id").and_then(Value::as_str).unwrap_or(""),
            )
        }
        _ => json!({}),
    };
    json!({
        "type": "control_response",
        "response": { "subtype": "success", "request_id": request_id, "response": inner }
    })
}

fn mcp_inner(request: &Value, tools: &dyn ToolRegistry) -> Value {
    let msg = &request["message"];
    let id = msg.get("id").cloned();
    // Notification (no id / null id): stub ack per the SDK's contract.
    if id.as_ref().is_none_or(Value::is_null) {
        return json!({ "mcp_response": { "jsonrpc": "2.0", "result": {}, "id": 0 } });
    }
    let id = id.unwrap();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let result = match method {
        "initialize" => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "meridian", "version": "0.0.0" }
        }),
        "tools/list" => json!({ "tools": tools.list() }),
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let args = msg["params"].get("arguments").cloned().unwrap_or(json!({}));
            tools.call(name, &args)
        }
        _ => json!({ "code": -32601, "message": "method not found" }),
    };
    json!({ "mcp_response": { "jsonrpc": "2.0", "id": id, "result": result } })
}
