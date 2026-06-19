use serde_json::json;
use meridian::openai::{openai_tools_to_mcp_defs, tool_calls_completion};

#[test]
fn converts_openai_function_tools() {
    let tools = vec![
        json!({"type":"function","function":{"name":"get_weather","description":"Weather","parameters":{"type":"object","properties":{"city":{"type":"string"}}}}}),
        json!({"type":"other"}), // skipped
        json!({"type":"function","function":{"name":"noop"}}), // no desc/params
    ];
    let defs = openai_tools_to_mcp_defs(&tools);
    assert_eq!(defs.len(), 2, "non-function entries skipped");
    assert_eq!(defs[0]["name"], "get_weather");
    assert_eq!(defs[0]["description"], "Weather");
    assert_eq!(defs[0]["inputSchema"]["properties"]["city"]["type"], "string");
    assert_eq!(defs[1]["name"], "noop");
    assert_eq!(defs[1]["description"], "noop");
    assert!(defs[1]["inputSchema"].as_object().unwrap().is_empty());
}

#[test]
fn builds_tool_calls_completion() {
    let captured = vec![json!({"id":"call_1","name":"mcp__oc__get_weather","input":{"city":"Paris"}})];
    let out = tool_calls_completion(&captured, "opus");
    assert_eq!(out["object"], "chat.completion");
    assert_eq!(out["model"], "opus");
    assert_eq!(out["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(out["choices"][0]["message"]["content"], serde_json::Value::Null);
    let tc = &out["choices"][0]["message"]["tool_calls"][0];
    assert_eq!(tc["id"], "call_1");
    assert_eq!(tc["type"], "function");
    assert_eq!(tc["function"]["name"], "get_weather", "mcp__oc__ stripped");
    // arguments is a JSON STRING, not an object
    let args = tc["function"]["arguments"].as_str().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(args).unwrap();
    assert_eq!(parsed["city"], "Paris");
}
