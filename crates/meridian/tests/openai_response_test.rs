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
fn model_list_is_openai_shaped() {
    let list = model_list();
    assert_eq!(list["object"], "list");
    let data = list["data"].as_array().unwrap();
    assert!(!data.is_empty());
    assert_eq!(data[0]["object"], "model");
    assert!(data.iter().any(|m| m["id"] == "claude-opus-4-8"));
    assert!(data.iter().all(|m| m["owned_by"] == "anthropic"));
}
