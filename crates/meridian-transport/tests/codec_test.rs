use meridian_transport::codec::{parse_line, CliMessage};

#[test]
fn parses_init_line() {
    let line = std::fs::read_to_string("tests/fixtures/init.ndjson").unwrap();
    let line = line.lines().next().unwrap();
    match parse_line(line).unwrap() {
        CliMessage::Init { model, session_id, .. } => {
            assert!(!model.is_empty());
            assert!(!session_id.is_empty());
        }
        other => panic!("expected Init, got {other:?}"),
    }
}

#[test]
fn parses_control_request_mcp_message() {
    let line = r#"{"type":"control_request","request_id":"r1","request":{"subtype":"mcp_message","server_name":"spike","message":{"jsonrpc":"2.0","id":1,"method":"tools/list"}}}"#;
    match parse_line(line).unwrap() {
        CliMessage::ControlRequest { request_id, request } => {
            assert_eq!(request_id, "r1");
            assert_eq!(request["subtype"], "mcp_message");
        }
        other => panic!("expected ControlRequest, got {other:?}"),
    }
}

#[test]
fn unknown_type_falls_back_to_other() {
    assert!(matches!(parse_line(r#"{"type":"rate_limit_event"}"#).unwrap(), CliMessage::Other(_)));
}
