use crate::proxy::translator::format::Format;
use crate::proxy::translator::types::{ResponseTransform, StreamStateFactory};
use crate::proxy::translator::TranslatorRegistry;
/// OpenAI Responses → Claude Messages 转换对。
///
/// 请求方向：OpenAI Responses API → Claude Messages API
///   - `input` 数组 → `messages` 数组
///   - `instructions` → `system`
///   - `model` → `model`
///
/// 响应方向：Claude SSE → OpenAI Responses SSE（自动反转）
///   - `message_start` → `response.created` + `response.output_item.added`
///   - `content_block_delta` → `response.output_text.delta`
///   - `message_stop` → completion sequence + `[DONE]`
use serde_json::{json, Value};

// ─── 请求转换 ───

/// 将 OpenAI Responses API 请求转换为 Claude Messages API 格式。
pub fn responses_to_claude_request(model: &str, raw_json: &[u8], stream: bool) -> Vec<u8> {
    let root: Value = serde_json::from_slice(raw_json).unwrap_or(Value::Null);

    // 提取 input 数组
    let input = root
        .get("input")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 转换 input → messages
    let messages: Vec<Value> = input
        .iter()
        .filter_map(|item| {
            let role = item.get("role")?.as_str()?;
            let content = item.get("content")?;

            // 只处理 user 和 assistant 消息
            if role == "user" || role == "assistant" {
                let msg = match content {
                    Value::String(s) => json!({
                        "role": role,
                        "content": s
                    }),
                    Value::Array(parts) => json!({
                        "role": role,
                        "content": parts
                    }),
                    _ => return None,
                };
                Some(msg)
            } else {
                None
            }
        })
        .collect();

    // 构建 Claude 请求
    let mut claude_req = json!({
        "model": model,
        "messages": messages,
        "stream": stream
    });

    // instructions → system
    if let Some(instructions) = root.get("instructions").and_then(|v| v.as_str()) {
        claude_req["system"] = Value::String(instructions.to_string());
    }

    // max_output_tokens → max_tokens
    if let Some(mt) = root.get("max_output_tokens").and_then(|v| v.as_u64()) {
        claude_req["max_tokens"] = Value::Number(mt.into());
    } else if let Some(mt) = root.get("max_tokens").and_then(|v| v.as_u64()) {
        claude_req["max_tokens"] = Value::Number(mt.into());
    }

    // temperature
    if let Some(t) = root.get("temperature").and_then(|v| v.as_f64()) {
        if let Some(num) = serde_json::Number::from_f64(t) {
            claude_req["temperature"] = Value::Number(num);
        }
    }

    // top_p
    if let Some(t) = root.get("top_p").and_then(|v| v.as_f64()) {
        if let Some(num) = serde_json::Number::from_f64(t) {
            claude_req["top_p"] = Value::Number(num);
        }
    }

    serde_json::to_vec(&claude_req).unwrap_or(raw_json.to_vec())
}

// ─── 非流式响应转换 ───

/// Claude 非流式响应 → OpenAI Responses API 格式。
pub fn claude_to_responses_non_stream(
    model: &str,
    _original_request: &[u8],
    _converted_request: &[u8],
    upstream_json: &[u8],
    _state: &mut (dyn std::any::Any + Send),
) -> Vec<u8> {
    let claude: Value = serde_json::from_slice(upstream_json).unwrap_or(Value::Null);

    // 提取 Claude 响应内容
    let content_blocks = claude
        .get("content")
        .and_then(|c| c.as_array())
        .cloned()
        .unwrap_or_default();

    let text_content: String = content_blocks
        .iter()
        .filter_map(|b| {
            if b.get("type").and_then(|v| v.as_str()) == Some("text") {
                b.get("text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let thinking_content: String = content_blocks
        .iter()
        .filter_map(|b| {
            if b.get("type").and_then(|v| v.as_str()) == Some("thinking") {
                b.get("thinking")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    let stop_reason = claude
        .get("stop_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("stop");
    let status = match stop_reason {
        "end_turn" | "stop" => "completed",
        "max_tokens" => "incomplete",
        _ => "completed",
    };

    let response_id = claude
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("resp-0");
    let item_id = format!("msg_{}", response_id.chars().take(16).collect::<String>());

    let mut response = json!({
        "id": response_id,
        "object": "response",
        "created_at": chrono::Utc::now().timestamp(),
        "status": status,
        "output": [{
            "id": item_id,
            "type": "message",
            "role": "assistant",
            "status": if status == "completed" { "completed" } else { "in_progress" },
            "content": [{
                "type": "output_text",
                "text": text_content
            }]
        }],
        "model": model,
        "usage": {
            "input_tokens": claude.get("usage").and_then(|u| u.get("input_tokens")).and_then(|v| v.as_i64()).unwrap_or(0),
            "output_tokens": claude.get("usage").and_then(|u| u.get("output_tokens")).and_then(|v| v.as_i64()).unwrap_or(0)
        }
    });

    // 如果有 reasoning，添加到 output item
    if !thinking_content.is_empty() {
        if let Some(output_arr) = response["output"].as_array_mut() {
            if let Some(item) = output_arr.first_mut() {
                item["reasoning"] = json!([{
                    "type": "reasoning",
                    "text": thinking_content
                }]);
            }
        }
    }

    serde_json::to_vec(&response).unwrap_or(upstream_json.to_vec())
}

// ─── 流式响应转换 ───

/// Claude 流 → Responses API SSE 格式。
pub fn claude_to_responses_stream(
    model: &str,
    _original_request: &[u8],
    _converted_request: &[u8],
    upstream_chunk: &[u8],
    state: &mut (dyn std::any::Any + Send),
) -> Vec<Vec<u8>> {
    let state = state
        .downcast_mut::<ResponsesToClaudeStreamState>()
        .unwrap();
    let mut outputs: Vec<Vec<u8>> = Vec::new();

    let text = std::str::from_utf8(upstream_chunk).unwrap_or("");
    let lines = text.lines();
    let mut event_type = String::new();
    let mut data = String::new();

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            if !event_type.is_empty() {
                process_claude_event_for_responses(model, &event_type, &data, state, &mut outputs);
                event_type.clear();
                data.clear();
            }
        } else if let Some(rest) = line.strip_prefix("event: ") {
            event_type = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("data: ") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        }
    }

    outputs
}

fn process_claude_event_for_responses(
    model: &str,
    event_type: &str,
    data: &str,
    state: &mut ResponsesToClaudeStreamState,
    outputs: &mut Vec<Vec<u8>>,
) {
    use crate::proxy::translator::sse;

    if data == "[DONE]" {
        send_responses_completion_sequence(state, model, outputs);
        return;
    }

    let parsed: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return,
    };

    // 初始化 response_id
    if state.response_id.is_none() {
        if let Some(id) = parsed.get("id").and_then(|v| v.as_str()) {
            state.response_id = Some(id.to_string());
            state.item_id = Some(format!("msg_{}", id.chars().take(16).collect::<String>()));
        } else {
            state.response_id = Some("resp-0".to_string());
            state.item_id = Some("msg-0".to_string());
        }
    }

    match event_type {
        "message_start" => {
            if !state.has_sent_created {
                let response_id = state.response_id.as_deref().unwrap_or("resp-0");
                let ev = json!({
                    "type": "response.created",
                    "response": {
                        "id": response_id,
                        "object": "response",
                        "status": "in_progress",
                        "output": [],
                        "model": model
                    }
                });
                let mut buf = bytes::BytesMut::new();
                sse::append_sse_event_no_id(
                    &mut buf,
                    "data",
                    &serde_json::to_vec(&ev).unwrap_or_default(),
                );
                outputs.push(buf.to_vec());
                state.has_sent_created = true;
            }
            if !state.has_sent_item_added {
                let item_id = state.item_id.as_deref().unwrap_or("msg-0");
                let ev = json!({
                    "type": "response.output_item.added",
                    "output_index": 0,
                    "item": {
                        "id": item_id,
                        "type": "message",
                        "role": "assistant",
                        "status": "in_progress",
                        "content": []
                    }
                });
                let mut buf = bytes::BytesMut::new();
                sse::append_sse_event_no_id(
                    &mut buf,
                    "data",
                    &serde_json::to_vec(&ev).unwrap_or_default(),
                );
                outputs.push(buf.to_vec());
                state.has_sent_item_added = true;
            }
        }
        "content_block_start" => {
            if !state.has_sent_content_added {
                let item_id = state.item_id.as_deref().unwrap_or("msg-0");
                let ev = json!({
                    "type": "response.content_part.added",
                    "item_id": item_id,
                    "output_index": 0,
                    "content_index": 0,
                    "part": { "type": "output_text", "text": "" }
                });
                let mut buf = bytes::BytesMut::new();
                sse::append_sse_event_no_id(
                    &mut buf,
                    "data",
                    &serde_json::to_vec(&ev).unwrap_or_default(),
                );
                outputs.push(buf.to_vec());
                state.has_sent_content_added = true;
            }
        }
        "content_block_delta" => {
            let item_id = state.item_id.as_deref().unwrap_or("msg-0");
            if let Some(text_delta) = parsed
                .get("delta")
                .and_then(|d| d.get("text"))
                .and_then(|v| v.as_str())
            {
                let ev = json!({
                    "type": "response.output_text.delta",
                    "item_id": item_id,
                    "output_index": 0,
                    "content_index": 0,
                    "delta": text_delta
                });
                let mut buf = bytes::BytesMut::new();
                sse::append_sse_event_no_id(
                    &mut buf,
                    "data",
                    &serde_json::to_vec(&ev).unwrap_or_default(),
                );
                outputs.push(buf.to_vec());
                state.text_buffer.push_str(text_delta);
            } else if let Some(thinking_delta) = parsed
                .get("delta")
                .and_then(|d| d.get("thinking"))
                .and_then(|v| v.as_str())
            {
                let ev = json!({
                    "type": "response.reasoning.delta",
                    "item_id": item_id,
                    "output_index": 0,
                    "content_index": 0,
                    "delta": thinking_delta
                });
                let mut buf = bytes::BytesMut::new();
                sse::append_sse_event_no_id(
                    &mut buf,
                    "data",
                    &serde_json::to_vec(&ev).unwrap_or_default(),
                );
                outputs.push(buf.to_vec());
                state.reasoning_buffer.push_str(thinking_delta);
            }
        }
        "message_delta" => {
            // 消息增量 — 通常包含 stop_reason，不产生输出
        }
        "message_stop" => {
            send_responses_completion_sequence(state, model, outputs);
        }
        _ => {}
    }
}

/// 发送 Responses API 完成事件序列。
fn send_responses_completion_sequence(
    state: &mut ResponsesToClaudeStreamState,
    model: &str,
    outputs: &mut Vec<Vec<u8>>,
) {
    use crate::proxy::translator::sse;

    if state.has_sent_completed {
        return;
    }

    let item_id = state.item_id.as_deref().unwrap_or("msg-0");
    let response_id = state.response_id.as_deref().unwrap_or("resp-0");

    // response.output_text.done
    let ev = json!({
        "type": "response.output_text.done",
        "item_id": item_id,
        "output_index": 0,
        "content_index": 0,
        "text": state.text_buffer
    });
    let mut buf = bytes::BytesMut::new();
    sse::append_sse_event_no_id(
        &mut buf,
        "data",
        &serde_json::to_vec(&ev).unwrap_or_default(),
    );
    outputs.push(buf.to_vec());

    // response.content_part.done
    let ev = json!({
        "type": "response.content_part.done",
        "item_id": item_id,
        "output_index": 0,
        "content_index": 0,
        "part": { "type": "output_text", "text": &state.text_buffer }
    });
    let mut buf = bytes::BytesMut::new();
    sse::append_sse_event_no_id(
        &mut buf,
        "data",
        &serde_json::to_vec(&ev).unwrap_or_default(),
    );
    outputs.push(buf.to_vec());

    // response.output_item.done
    let ev = json!({
        "type": "response.output_item.done",
        "output_index": 0,
        "item": {
            "id": item_id,
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [{ "type": "output_text", "text": &state.text_buffer }]
        }
    });
    let mut buf = bytes::BytesMut::new();
    sse::append_sse_event_no_id(
        &mut buf,
        "data",
        &serde_json::to_vec(&ev).unwrap_or_default(),
    );
    outputs.push(buf.to_vec());

    // response.completed
    let ev = json!({
        "type": "response.completed",
        "response": {
            "id": response_id,
            "object": "response",
            "status": "completed",
            "output": [{
                "id": item_id,
                "type": "message",
                "role": "assistant",
                "content": [{ "type": "output_text", "text": &state.text_buffer }]
            }],
            "model": model
        }
    });
    let mut buf = bytes::BytesMut::new();
    sse::append_sse_event_no_id(
        &mut buf,
        "data",
        &serde_json::to_vec(&ev).unwrap_or_default(),
    );
    outputs.push(buf.to_vec());

    // [DONE]
    outputs.push(sse::openai_done_marker());

    state.has_sent_completed = true;
}

/// 流状态。
#[derive(Default)]
pub struct ResponsesToClaudeStreamState {
    pub response_id: Option<String>,
    pub item_id: Option<String>,
    pub text_buffer: String,
    pub reasoning_buffer: String,
    pub has_sent_created: bool,
    pub has_sent_item_added: bool,
    pub has_sent_content_added: bool,
    pub has_sent_completed: bool,
}

fn responses_to_claude_state_factory() -> Box<dyn std::any::Any + Send> {
    Box::new(ResponsesToClaudeStreamState::default())
}

// ─── 注册函数 ───

pub fn register(registry: &mut TranslatorRegistry) {
    registry.register(
        Format::OpenAIResponses,
        Format::Claude,
        responses_to_claude_request,
        ResponseTransform {
            stream: Some(claude_to_responses_stream),
            non_stream: Some(claude_to_responses_non_stream),
            token_count: None,
        },
        Some(responses_to_claude_state_factory as StreamStateFactory),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_responses_to_claude_request_basic() {
        let input = json!({
            "model": "gpt-4",
            "input": [
                {"role": "user", "content": "Hello!"}
            ],
            "instructions": "Be helpful.",
            "stream": true,
            "max_output_tokens": 1024
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = responses_to_claude_request("gpt-4", &input_bytes, true);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["model"], "gpt-4");
        assert_eq!(output_val["system"], "Be helpful.");
        assert_eq!(output_val["stream"], true);
        assert_eq!(output_val["max_tokens"], 1024);

        let messages = output_val["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Hello!");
    }

    #[test]
    fn test_responses_to_claude_request_multi_turn() {
        let input = json!({
            "model": "gpt-4",
            "input": [
                {"role": "user", "content": "What is Rust?"},
                {"role": "assistant", "content": "A systems language."},
                {"role": "user", "content": "Is it safe?"}
            ],
            "stream": false
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = responses_to_claude_request("gpt-4", &input_bytes, false);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        let messages = output_val["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
    }

    #[test]
    fn test_responses_to_claude_request_no_instructions() {
        let input = json!({
            "model": "gpt-4",
            "input": [
                {"role": "user", "content": "Hello"}
            ],
            "stream": false
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = responses_to_claude_request("gpt-4", &input_bytes, false);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert!(output_val.get("system").is_none() || output_val["system"].is_null());
    }

    #[test]
    fn test_responses_to_claude_request_array_content() {
        let input = json!({
            "model": "gpt-4",
            "input": [
                {
                    "role": "user",
                    "content": [
                        {"type": "input_text", "text": "First part"},
                        {"type": "input_text", "text": "Second part"}
                    ]
                }
            ],
            "stream": false
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = responses_to_claude_request("gpt-4", &input_bytes, false);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        let messages = output_val["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        let content = messages[0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[0]["text"], "First part");
    }

    #[test]
    fn test_responses_to_claude_state_default() {
        let state = ResponsesToClaudeStreamState::default();
        assert!(!state.has_sent_created);
        assert!(!state.has_sent_item_added);
        assert!(!state.has_sent_content_added);
        assert!(!state.has_sent_completed);
        assert!(state.response_id.is_none());
        assert!(state.item_id.is_none());
        assert!(state.text_buffer.is_empty());
        assert!(state.reasoning_buffer.is_empty());
    }

    #[test]
    fn test_claude_to_responses_non_stream() {
        let claude_response = json!({
            "id": "msg_abc123",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Hello from Claude!"}
            ],
            "model": "claude-sonnet-4",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let upstream_bytes = serde_json::to_vec(&claude_response).unwrap();
        let mut state = ();
        let output = claude_to_responses_non_stream(
            "claude-sonnet-4",
            &[],
            &[],
            &upstream_bytes,
            &mut state,
        );
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["id"], "msg_abc123");
        assert_eq!(output_val["object"], "response");
        assert_eq!(output_val["status"], "completed");
        let output_items = output_val["output"].as_array().unwrap();
        assert_eq!(output_items.len(), 1);
        let content = output_items[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "output_text");
        assert_eq!(content[0]["text"], "Hello from Claude!");
        assert_eq!(output_val["usage"]["input_tokens"], 10);
        assert_eq!(output_val["usage"]["output_tokens"], 5);
    }

    #[test]
    fn test_claude_to_responses_non_stream_with_thinking() {
        let claude_response = json!({
            "id": "msg_xyz",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": "Let me think..."},
                {"type": "text", "text": "The answer is 42."}
            ],
            "model": "claude-sonnet-4",
            "stop_reason": "end_turn"
        });
        let upstream_bytes = serde_json::to_vec(&claude_response).unwrap();
        let mut state = ();
        let output = claude_to_responses_non_stream(
            "claude-sonnet-4",
            &[],
            &[],
            &upstream_bytes,
            &mut state,
        );
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        let output_items = output_val["output"].as_array().unwrap();
        assert_eq!(output_items[0]["content"][0]["text"], "The answer is 42.");
        let reasoning = output_items[0]["reasoning"].as_array().unwrap();
        assert_eq!(reasoning[0]["type"], "reasoning");
        assert_eq!(reasoning[0]["text"], "Let me think...");
    }
}
