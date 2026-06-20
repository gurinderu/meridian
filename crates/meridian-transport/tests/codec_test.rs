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
fn parses_rate_limit_event() {
    let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","rateLimitType":"five_hour","utilization":0.5},"uuid":"u","session_id":"s"}"#;
    match parse_line(line).unwrap() {
        CliMessage::RateLimitEvent { info, .. } => {
            assert_eq!(info["rateLimitType"], "five_hour");
            assert_eq!(info["status"], "allowed");
        }
        other => panic!("expected RateLimitEvent, got {other:?}"),
    }
}

#[test]
fn parses_stream_event_line() {
    // First stream_event line in the recorded fixture is a message_start.
    let content = std::fs::read_to_string("tests/fixtures/streaming_turn.ndjson").unwrap();
    let line = content
        .lines()
        .find(|l| l.contains("\"type\":\"stream_event\""))
        .expect("fixture has a stream_event line");
    match meridian_transport::codec::parse_line(line).unwrap() {
        meridian_transport::codec::CliMessage::StreamEvent { event, .. } => {
            assert!(event.get("type").and_then(|t| t.as_str()).is_some());
        }
        other => panic!("expected StreamEvent, got {other:?}"),
    }
}
