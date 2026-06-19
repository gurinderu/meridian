use axum::response::sse::Event;
use serde_json::Value;

/// SSE `(event_name, data_json)` for an Anthropic stream event.
pub fn sse_fields(event: &Value) -> (String, String) {
    let name = event.get("type").and_then(Value::as_str).unwrap_or("message").to_string();
    (name, event.to_string())
}

/// Build an axum SSE `Event` (named `event:` line + JSON `data:` line).
pub fn sse_event(event: &Value) -> Event {
    let (name, data) = sse_fields(event);
    Event::default().event(name).data(data)
}

/// A stream of raw Anthropic stream-event objects (the `.event` of each CLI
/// `stream_event`, or a synthetic `{"type":"error",...}`). Handlers frame these
/// into the wire format they serve (Anthropic SSE or OpenAI chunks).
pub type EventStream =
    tokio_stream::wrappers::ReceiverStream<serde_json::Value>;
