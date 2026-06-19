use serde_json::json;
use meridian::tools::{anthropic_tools_to_mcp_defs, strip_oc_prefix};

#[test]
fn converts_anthropic_tools_to_mcp_defs() {
    let tools = vec![
        json!({"name":"edit_file","description":"Edit a file","input_schema":{"type":"object","properties":{"path":{"type":"string"}}}}),
        json!({"name":"noop"}), // no description, no schema
    ];
    let defs = anthropic_tools_to_mcp_defs(&tools);
    assert_eq!(defs[0]["name"], "edit_file");
    assert_eq!(defs[0]["description"], "Edit a file");
    assert_eq!(defs[0]["inputSchema"]["properties"]["path"]["type"], "string");
    // description falls back to name; inputSchema falls back to {}
    assert_eq!(defs[1]["name"], "noop");
    assert_eq!(defs[1]["description"], "noop");
    assert!(defs[1]["inputSchema"].as_object().unwrap().is_empty());
}

#[test]
fn strips_oc_prefix() {
    assert_eq!(strip_oc_prefix("mcp__oc__edit_file"), "edit_file");
    assert_eq!(strip_oc_prefix("edit_file"), "edit_file");
}
