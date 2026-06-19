use serde_json::json;
use meridian::sse::sse_fields;

#[test]
fn sse_fields_uses_event_type_and_serializes_data() {
    let ev = json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}});
    let (name, data) = sse_fields(&ev);
    assert_eq!(name, "content_block_delta");
    let round: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(round["delta"]["text"], "hi");
}

#[test]
fn sse_fields_defaults_name_when_no_type() {
    let (name, _) = sse_fields(&json!({"foo":"bar"}));
    assert_eq!(name, "message");
}
