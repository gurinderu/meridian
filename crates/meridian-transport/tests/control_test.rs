use serde_json::{json, Value};
use meridian_transport::control::handle_control_request;
use meridian_transport::mcp::ToolRegistry;

struct PingTools;
impl ToolRegistry for PingTools {
    fn list(&self) -> Vec<Value> {
        vec![json!({"name":"ping","description":"p","inputSchema":{"type":"object","properties":{}}})]
    }
    fn call(&self, name: &str, _args: &Value) -> Value {
        assert_eq!(name, "ping");
        json!({"content":[{"type":"text","text":"PONG"}],"isError":false})
    }
}

#[test]
fn answers_tools_list() {
    let req = json!({"subtype":"mcp_message","server_name":"spike",
                     "message":{"jsonrpc":"2.0","id":7,"method":"tools/list"}});
    let resp = handle_control_request("rq", &req, &PingTools);
    assert_eq!(resp["type"], "control_response");
    assert_eq!(resp["response"]["subtype"], "success");
    assert_eq!(resp["response"]["request_id"], "rq");
    let mcp = &resp["response"]["response"]["mcp_response"];
    assert_eq!(mcp["id"], 7);
    assert_eq!(mcp["result"]["tools"][0]["name"], "ping");
}

#[test]
fn answers_tools_call() {
    let req = json!({"subtype":"mcp_message","server_name":"spike",
                     "message":{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"ping","arguments":{}}}});
    let resp = handle_control_request("rq2", &req, &PingTools);
    let mcp = &resp["response"]["response"]["mcp_response"];
    assert_eq!(mcp["result"]["content"][0]["text"], "PONG");
}

#[test]
fn notification_without_id_gets_stub() {
    let req = json!({"subtype":"mcp_message","server_name":"spike",
                     "message":{"jsonrpc":"2.0","method":"notifications/initialized"}});
    let resp = handle_control_request("rq3", &req, &PingTools);
    assert_eq!(resp["response"]["response"]["mcp_response"]["id"], 0);
}
