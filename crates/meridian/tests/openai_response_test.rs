use serde_json::json;
use meridian::openai::{anthropic_to_openai, finish_reason, model_list};

#[test]
fn finish_reason_mapping() {
    assert_eq!(finish_reason(Some("max_tokens")), "length");
    assert_eq!(finish_reason(Some("tool_use")), "tool_calls");
    assert_eq!(finish_reason(Some("end_turn")), "stop");
    assert_eq!(finish_reason(None), "stop");
}

#[test]
fn translate_message_to_chat_completion() {
    let msg = json!({
        "id":"msg_abc","role":"assistant",
        "content":[{"type":"text","text":"Hel"},{"type":"text","text":"lo"}],
        "stop_reason":"end_turn",
        "usage":{"input_tokens":10,"output_tokens":3}
    });
    let out = anthropic_to_openai(&msg, "opus");
    assert_eq!(out["object"], "chat.completion");
    assert_eq!(out["id"], "chatcmpl-msg_abc");
    assert_eq!(out["model"], "opus");
    assert_eq!(out["choices"][0]["message"]["role"], "assistant");
    assert_eq!(out["choices"][0]["message"]["content"], "Hello");
    assert_eq!(out["choices"][0]["finish_reason"], "stop");
    assert_eq!(out["usage"]["prompt_tokens"], 10);
    assert_eq!(out["usage"]["completion_tokens"], 3);
    assert_eq!(out["usage"]["total_tokens"], 13);
}

#[test]
fn tool_use_only_turn_yields_null_content() {
    let msg = json!({
        "id":"msg_tool","role":"assistant",
        "content":[{"type":"tool_use","id":"call_1","name":"get_weather","input":{"city":"NYC"}}],
        "stop_reason":"tool_use",
        "usage":{"input_tokens":5,"output_tokens":2}
    });
    let out = anthropic_to_openai(&msg, "sonnet");
    assert_eq!(out["object"], "chat.completion");
    assert_eq!(out["choices"][0]["finish_reason"], "tool_calls");
    assert!(
        out["choices"][0]["message"]["content"].is_null(),
        "expected null content for tool_use turn, got: {}",
        out["choices"][0]["message"]["content"]
    );
}

#[test]
fn text_turn_still_yields_string_content() {
    let msg = json!({
        "id":"msg_txt","role":"assistant",
        "content":[{"type":"text","text":"hello"}],
        "stop_reason":"end_turn",
        "usage":{"input_tokens":3,"output_tokens":1}
    });
    let out = anthropic_to_openai(&msg, "haiku");
    assert_eq!(out["choices"][0]["message"]["content"], "hello");
}

#[test]
fn model_list_is_openai_shaped() {
    let list = model_list();
    assert_eq!(list["object"], "list");
    let data = list["data"].as_array().unwrap();
    assert!(!data.is_empty());
    assert_eq!(data[0]["object"], "model");
    assert!(data.iter().any(|m| m["id"] == "claude-opus-4-8"));
    assert!(data.iter().all(|m| m["owned_by"] == "anthropic"));
}
