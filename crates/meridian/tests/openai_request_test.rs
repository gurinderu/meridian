use serde_json::json;
use meridian::openai::{extract_openai_content, openai_to_canonical};

#[test]
fn content_string_and_parts() {
    assert_eq!(extract_openai_content(&json!("hi")), "hi");
    let parts = json!([{"type":"text","text":"a"},{"type":"text","text":"b"}]);
    assert_eq!(extract_openai_content(&parts), "a\nb");
}

#[test]
fn canonical_extracts_model_system_and_last_user() {
    let body = json!({
        "model":"opus",
        "messages":[
            {"role":"system","content":"be terse"},
            {"role":"user","content":"first"},
            {"role":"assistant","content":"ok"},
            {"role":"user","content":"second"}
        ]
    });
    let (model, system, prompt) = openai_to_canonical(&body).unwrap();
    assert_eq!(model, "opus");
    assert_eq!(prompt, "second");
    let sys = system.unwrap();
    assert!(sys.contains("be terse"));
    assert!(sys.contains("<conversation_history>"));
    assert!(sys.contains("first") && sys.contains("ok"));
    assert!(!sys.contains("second"), "the last user msg is the prompt, not history");
}

#[test]
fn canonical_defaults_model_and_errors_without_user() {
    let (model, system, _p) = openai_to_canonical(&json!({"messages":[{"role":"user","content":"x"}]})).unwrap();
    assert_eq!(model, "sonnet");
    assert!(system.is_none());
    assert!(openai_to_canonical(&json!({"messages":[{"role":"system","content":"x"}]})).is_err());
    assert!(openai_to_canonical(&json!({"messages":[]})).is_err());
}
