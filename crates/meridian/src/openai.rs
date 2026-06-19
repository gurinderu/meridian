use serde_json::{json, Value};

/// OpenAI message content: a string, or an array of `{type:"text", text}` parts.
pub fn extract_openai_content(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Translate an OpenAI chat request into the canonical (model, system, prompt).
/// Single-turn: prior user/assistant turns are packed into a
/// `<conversation_history>` block; the last user message is the prompt.
pub fn openai_to_canonical(body: &Value) -> Result<(String, Option<String>, String), String> {
    let messages = body
        .get("messages")
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| "messages must be a non-empty array".to_string())?;

    let model = body.get("model").and_then(Value::as_str).unwrap_or("sonnet").to_string();

    let role = |m: &Value| m.get("role").and_then(Value::as_str).unwrap_or("").to_string();
    let content = |m: &Value| extract_openai_content(m.get("content").unwrap_or(&Value::Null));

    let last_user_idx = messages
        .iter()
        .rposition(|m| role(m) == "user")
        .ok_or_else(|| "no user message found".to_string())?;
    let prompt = content(&messages[last_user_idx]);

    let mut system_parts: Vec<String> = messages
        .iter()
        .filter(|m| role(m) == "system")
        .map(&content)
        .filter(|s| !s.is_empty())
        .collect();

    let history: Vec<String> = messages
        .iter()
        .enumerate()
        .filter(|(i, m)| *i != last_user_idx && matches!(role(m).as_str(), "user" | "assistant"))
        .map(|(_, m)| format!("{}: {}", role(m), content(m)))
        .collect();
    if !history.is_empty() {
        system_parts.push(format!("<conversation_history>\n{}\n</conversation_history>", history.join("\n")));
    }

    let system = if system_parts.is_empty() { None } else { Some(system_parts.join("\n\n")) };
    Ok((model, system, prompt))
}

/// Anthropic `stop_reason` -> OpenAI `finish_reason`.
pub fn finish_reason(stop_reason: Option<&str>) -> &'static str {
    match stop_reason {
        Some("max_tokens") => "length",
        Some("tool_use") => "tool_calls",
        _ => "stop",
    }
}

/// Translate an Anthropic `message` object into an OpenAI `chat.completion`.
pub fn anthropic_to_openai(msg: &Value, model: &str) -> Value {
    let text = msg
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .concat()
        })
        .unwrap_or_default();

    let id = msg.get("id").and_then(Value::as_str).unwrap_or("meridian");
    let input = msg["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let output = msg["usage"]["output_tokens"].as_u64().unwrap_or(0);
    let fr = finish_reason(msg.get("stop_reason").and_then(Value::as_str));

    json!({
        "id": format!("chatcmpl-{id}"),
        "object": "chat.completion",
        "created": 0,
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": text },
            "finish_reason": fr
        }],
        "usage": {
            "prompt_tokens": input,
            "completion_tokens": output,
            "total_tokens": input + output
        }
    })
}

const EXPOSED_MODELS: &[&str] = &[
    "claude-opus-4-8", "claude-sonnet-4-6", "claude-haiku-4-5", "opus", "sonnet", "haiku",
];

/// OpenAI `/v1/models` list of the models this proxy exposes.
pub fn model_list() -> Value {
    let data: Vec<Value> = EXPOSED_MODELS
        .iter()
        .map(|id| json!({"id": id, "object": "model", "created": 0, "owned_by": "anthropic"}))
        .collect();
    json!({ "object": "list", "data": data })
}
