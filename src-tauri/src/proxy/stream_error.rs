use bytes::Bytes;
use serde_json::{json, Value};
use std::fmt::Display;

fn upstream_error_message(error: impl Display) -> String {
    format!("Upstream stream error: {error}")
}

fn data_event(payload: Value) -> Bytes {
    let serialized = serde_json::to_string(&payload)
        .expect("stream error payloads are constructed from serializable JSON values");
    Bytes::from(format!("data: {serialized}\n\n"))
}

/// Build a protocol-valid Anthropic Messages error event.
pub fn anthropic_sse_error(error: impl Display) -> Bytes {
    let payload = json!({
        "type": "error",
        "error": {
            "type": "api_error",
            "message": upstream_error_message(error),
        },
    });
    let serialized = serde_json::to_string(&payload)
        .expect("stream error payloads are constructed from serializable JSON values");
    Bytes::from(format!("event: error\ndata: {serialized}\n\n"))
}

/// Build a protocol-valid OpenAI Chat Completions error data event.
pub fn openai_chat_sse_error(error: impl Display) -> Bytes {
    data_event(json!({
        "error": {
            "message": upstream_error_message(error),
            "type": "server_error",
            "param": null,
            "code": null,
        },
    }))
}

/// Build a protocol-valid OpenAI Responses/Codex error data event.
pub fn responses_sse_error(error: impl Display) -> Bytes {
    data_event(json!({
        "type": "error",
        "error": {
            "type": "upstream_error",
            "message": upstream_error_message(error),
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::{anthropic_sse_error, openai_chat_sse_error, responses_sse_error};
    use serde_json::Value;

    fn parse_sse_data(bytes: &[u8]) -> Value {
        let text = std::str::from_utf8(bytes).expect("SSE must be UTF-8");
        let data = text
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .expect("SSE must contain a data line");
        serde_json::from_str(data).expect("SSE data must be valid JSON")
    }

    #[test]
    fn anthropic_error_is_a_valid_error_event() {
        let event = anthropic_sse_error("body decode failed: \"invalid\\json\"\nretry");
        let text = std::str::from_utf8(&event).unwrap();

        assert!(text.starts_with("event: error\ndata: "));
        let body = parse_sse_data(&event);
        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["type"], "api_error");
        assert_eq!(
            body["error"]["message"],
            "Upstream stream error: body decode failed: \"invalid\\json\"\nretry"
        );
    }

    #[test]
    fn openai_chat_error_is_a_valid_json_data_event() {
        let event = openai_chat_sse_error("body decode failed");
        let body = parse_sse_data(&event);

        assert_eq!(body["error"]["type"], "server_error");
        assert_eq!(
            body["error"]["message"],
            "Upstream stream error: body decode failed"
        );
        assert!(body["error"].get("param").is_some());
        assert!(body["error"].get("code").is_some());
    }

    #[test]
    fn responses_error_is_a_valid_json_data_event() {
        let event = responses_sse_error("body decode failed");
        let body = parse_sse_data(&event);

        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["type"], "upstream_error");
        assert_eq!(
            body["error"]["message"],
            "Upstream stream error: body decode failed"
        );
    }
}
