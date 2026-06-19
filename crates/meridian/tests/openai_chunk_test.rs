use serde_json::json;
use meridian::openai::new_chunker;

#[test]
fn translates_a_text_stream_to_openai_chunks() {
    let mut ch = new_chunker("opus");

    let start = ch.push(&json!({"type":"message_start","message":{"id":"msg_9"}}));
    assert_eq!(start.len(), 1);
    assert_eq!(start[0]["object"], "chat.completion.chunk");
    assert_eq!(start[0]["id"], "chatcmpl-msg_9");
    assert_eq!(start[0]["model"], "opus");
    assert_eq!(start[0]["choices"][0]["delta"]["role"], "assistant");
    assert_eq!(start[0]["choices"][0]["finish_reason"], serde_json::Value::Null);

    let none = ch.push(&json!({"type":"content_block_start","index":0}));
    assert!(none.is_empty(), "content_block_start (text) produces no chunk");

    let delta = ch.push(&json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}));
    assert_eq!(delta.len(), 1);
    assert_eq!(delta[0]["choices"][0]["delta"]["content"], "Hi");
    assert_eq!(delta[0]["id"], "chatcmpl-msg_9", "id carried from message_start");

    let fin = ch.push(&json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}));
    assert_eq!(fin.len(), 1);
    assert_eq!(fin[0]["choices"][0]["finish_reason"], "stop");
    assert!(fin[0]["choices"][0]["delta"].as_object().unwrap().is_empty());

    let stop = ch.push(&json!({"type":"message_stop"}));
    assert!(stop.is_empty(), "message_stop yields no chunk (handler appends [DONE])");
}
