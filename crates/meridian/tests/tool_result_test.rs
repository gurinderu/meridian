use serde_json::json;
use meridian::tools::unwrap_tool_results;

#[test]
fn unwraps_tool_result_blocks_to_text() {
    let msg = json!({"role":"user","content":[
        {"type":"tool_result","tool_use_id":"tu_1","content":"18C and sunny"}
    ]});
    let out = unwrap_tool_results(&msg).expect("some");
    assert!(out.contains("tu_1"), "includes the tool_use_id: {out}");
    assert!(out.contains("18C and sunny"), "includes the result content: {out}");
}

#[test]
fn unwraps_array_content_tool_result() {
    let msg = json!({"role":"user","content":[
        {"type":"tool_result","tool_use_id":"tu_2","content":[{"type":"text","text":"line1"},{"type":"text","text":"line2"}]}
    ]});
    let out = unwrap_tool_results(&msg).unwrap();
    assert!(out.contains("line1") && out.contains("line2"));
}

#[test]
fn returns_none_for_plain_user_message() {
    assert!(unwrap_tool_results(&json!({"role":"user","content":"hi"})).is_none());
    assert!(unwrap_tool_results(&json!({"role":"user","content":[{"type":"text","text":"hi"}]})).is_none());
}
