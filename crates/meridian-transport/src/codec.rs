use serde_json::Value;

#[derive(Debug, Clone)]
pub enum CliMessage {
    Init { model: String, session_id: String, raw: Value },
    Assistant { message: Value, session_id: Option<String>, raw: Value },
    Result { subtype: String, result: Option<String>, raw: Value },
    ControlRequest { request_id: String, request: Value },
    StreamEvent { event: Value, raw: Value },
    Other(Value),
}

/// Parse one NDJSON line from the CLI's stdout into a typed message.
/// Value-first dispatch keeps us robust to the protocol's many message types.
pub fn parse_line(line: &str) -> Result<CliMessage, serde_json::Error> {
    let v: Value = serde_json::from_str(line)?;
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    let sub = v.get("subtype").and_then(Value::as_str).unwrap_or("");
    Ok(match (ty, sub) {
        ("system", "init") => CliMessage::Init {
            model: v["model"].as_str().unwrap_or_default().to_string(),
            session_id: v["session_id"].as_str().unwrap_or_default().to_string(),
            raw: v,
        },
        ("assistant", _) => CliMessage::Assistant {
            message: v["message"].clone(),
            session_id: v["session_id"].as_str().map(str::to_string),
            raw: v,
        },
        ("result", _) => CliMessage::Result {
            subtype: sub.to_string(),
            result: v["result"].as_str().map(str::to_string),
            raw: v,
        },
        ("control_request", _) => CliMessage::ControlRequest {
            request_id: v["request_id"].as_str().unwrap_or_default().to_string(),
            request: v["request"].clone(),
        },
        ("stream_event", _) => CliMessage::StreamEvent {
            event: v.get("event").cloned().unwrap_or(Value::Null),
            raw: v,
        },
        _ => CliMessage::Other(v),
    })
}
