use bytes::BytesMut;
/// Codex → Claude 转换对。
///
/// 请求方向：Codex wire protocol → Claude Messages API
///   - `instructions` → `system`
///   - `input` → `messages`（解析为 user/assistant 交替消息）
///
/// 响应方向：Claude SSE → Codex SSE（`response.*` 事件）
use serde_json::{json, Value};

// ─── 请求转换 ───

/// 将 Codex 请求转换为 Claude Messages API 格式。
pub fn codex_to_claude_request(model: &str, raw_json: &[u8], stream: bool) -> Vec<u8> {
    let root: Value = serde_json::from_slice(raw_json).unwrap_or(Value::Null);

    // instructions → system
    let system = root
        .get("instructions")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // input → messages（解析 "role: content" 格式）
    let input = root.get("input").and_then(|v| v.as_str()).unwrap_or("");
    let messages = parse_codex_input_to_claude_messages(input);

    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": stream,
    });

    if let Some(ref s) = system {
        body["system"] = Value::String(s.clone());
    }

    // max_tokens
    if let Some(mt) = root.get("max_tokens").and_then(|v| v.as_u64()) {
        body["max_tokens"] = Value::Number(mt.into());
    } else {
        body["max_tokens"] = Value::Number(4096.into());
    }

    // temperature
    if let Some(t) = root.get("temperature").and_then(|v| v.as_f64()) {
        if let Some(num) = serde_json::Number::from_f64(t) {
            body["temperature"] = Value::Number(num);
        }
    }

    // top_p
    if let Some(t) = root.get("top_p").and_then(|v| v.as_f64()) {
        if let Some(num) = serde_json::Number::from_f64(t) {
            body["top_p"] = Value::Number(num);
        }
    }

    serde_json::to_vec(&body).unwrap_or(raw_json.to_vec())
}

/// 解析 Codex input 字符串为 Claude messages 数组。
///
/// 格式：每行是 "role: content"，按顺序构建消息。
/// 连续的同一 role 会合并为一条消息。
fn parse_codex_input_to_claude_messages(input: &str) -> Vec<Value> {
    if input.trim().is_empty() {
        return vec![json!({ "role": "user", "content": "" })];
    }

    let mut messages: Vec<Value> = Vec::new();
    let mut current_role: Option<String> = None;
    let mut current_content = String::new();

    for line in input.lines() {
        // 尝试解析 "role: content" 格式
        if let Some((role, content)) = parse_role_line(line) {
            // 保存之前的消息
            if let Some(ref prev_role) = current_role {
                let trimmed = current_content.trim().to_string();
                if !trimmed.is_empty() {
                    messages.push(json!({
                        "role": prev_role,
                        "content": trimmed
                    }));
                }
            }
            current_role = Some(role);
            current_content = content.to_string();
        } else {
            // 非 role 行，附加到当前内容
            if current_role.is_some() {
                current_content.push('\n');
                current_content.push_str(line);
            } else {
                // 还没有 role，默认 user
                current_role = Some("user".to_string());
                current_content = line.to_string();
            }
        }
    }

    // 保存最后一条消息
    if let Some(ref role) = current_role {
        let trimmed = current_content.trim().to_string();
        if !trimmed.is_empty() {
            messages.push(json!({
                "role": role,
                "content": trimmed
            }));
        }
    }

    if messages.is_empty() {
        messages.push(json!({ "role": "user", "content": input.trim() }));
    }

    messages
}

/// 解析 "role: content" 格式的行。
fn parse_role_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    // 匹配已知角色
    for role in &["user", "assistant", "system"] {
        let prefix = format!("{}:", role);
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            return Some((role.to_string(), rest.trim().to_string()));
        }
    }
    None
}

// ─── 非流式响应转换 ───

/// Claude 非流式响应 → Codex 响应格式。
pub fn claude_to_codex_response_non_stream(
    model: &str,
    _original_request: &[u8],
    _converted_request: &[u8],
    upstream_json: &[u8],
    _state: &mut (dyn std::any::Any + Send),
) -> Vec<u8> {
    let upstream: Value = serde_json::from_slice(upstream_json).unwrap_or(Value::Null);

    // 提取文本
    let content_blocks = upstream
        .get("content")
        .and_then(|v| v.as_array())
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

    let stop_reason = upstream
        .get("stop_reason")
        .and_then(|v| v.as_str())
        .unwrap_or("stop");
    let status = match stop_reason {
        "end_turn" | "stop" => "completed",
        "max_tokens" => "incomplete",
        _ => "completed",
    };

    let response_id = upstream
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("resp-0");
    let item_id = format!(
        "item-{}",
        &response_id.chars().take(16).collect::<String>()
    );

    let response = json!({
        "id": response_id,
        "status": status,
        "model": model,
        "output": [{
            "id": item_id,
            "type": "message",
            "role": "assistant",
            "status": if status == "completed" { "completed" } else { "in_progress" },
            "content": [
                { "type": "output_text", "text": text_content }
            ]
        }],
        "usage": upstream.get("usage").cloned().unwrap_or(json!({
            "input_tokens": 0,
            "output_tokens": 0,
        })),
    });

    // 如果有 reasoning，添加到 response
    let mut response = response;
    if !thinking_content.is_empty() {
        if let Some(obj) = response.as_object_mut() {
            obj.insert(
                "reasoning".to_string(),
                json!([{
                    "type": "reasoning",
                    "text": thinking_content
                }]),
            );
        }
    }

    serde_json::to_vec(&response).unwrap_or(upstream_json.to_vec())
}

// ─── 流式响应转换 ───

/// Claude 流 → Codex 流状态机。
pub struct ClaudeToCodexStreamState {
    pub response_id: Option<String>,
    pub item_id: Option<String>,
    pub text_buffer: String,
    pub reasoning_buffer: String,
    pub has_sent_created: bool,
    pub has_sent_item_added: bool,
    pub has_sent_content_added: bool,
}

impl Default for ClaudeToCodexStreamState {
    fn default() -> Self {
        Self {
            response_id: None,
            item_id: None,
            text_buffer: String::new(),
            reasoning_buffer: String::new(),
            has_sent_created: false,
            has_sent_item_added: false,
            has_sent_content_added: false,
        }
    }
}

pub fn claude_to_codex_stream(
    model: &str,
    _original_request: &[u8],
    _converted_request: &[u8],
    upstream_chunk: &[u8],
    state: &mut (dyn std::any::Any + Send),
) -> Vec<Vec<u8>> {
    let state = state.downcast_mut::<ClaudeToCodexStreamState>().unwrap();
    let mut outputs: Vec<Vec<u8>> = Vec::new();

    let text = std::str::from_utf8(upstream_chunk).unwrap_or("");
    let lines = text.lines();
    let mut event_type = String::new();
    let mut data = String::new();

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            if !event_type.is_empty() {
                process_claude_event_for_codex(model, &event_type, &data, state, &mut outputs);
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

fn process_claude_event_for_codex(
    model: &str,
    event_type: &str,
    data: &str,
    state: &mut ClaudeToCodexStreamState,
    outputs: &mut Vec<Vec<u8>>,
) {
    use crate::proxy::translator::sse;

    if data == "[DONE]" {
        // 发送 Codex 完成事件序列
        send_codex_completion_sequence(state, outputs);
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
            state.item_id = Some(format!("item-{}", &id.chars().take(16).collect::<String>()));
        } else {
            state.response_id = Some("resp-0".to_string());
            state.item_id = Some("item-0".to_string());
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
                let mut buf = BytesMut::new();
                sse::append_sse_event_no_id(
                    &mut buf,
                    "data",
                    &serde_json::to_vec(&ev).unwrap_or_default(),
                );
                outputs.push(buf.to_vec());
                state.has_sent_created = true;
            }
            if !state.has_sent_item_added {
                let item_id = state.item_id.as_deref().unwrap_or("item-0");
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
                let mut buf = BytesMut::new();
                sse::append_sse_event_no_id(
                    &mut buf,
                    "data",
                    &serde_json::to_vec(&ev).unwrap_or_default(),
                );
                outputs.push(buf.to_vec());
                state.has_sent_item_added = true;
            }
            if !state.has_sent_content_added {
                let item_id = state.item_id.as_deref().unwrap_or("item-0");
                let ev = json!({
                    "type": "response.content_part.added",
                    "item_id": item_id,
                    "output_index": 0,
                    "content_index": 0,
                    "part": { "type": "output_text", "text": "" }
                });
                let mut buf = BytesMut::new();
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
            let item_id = state.item_id.as_deref().unwrap_or("item-0");
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
                let mut buf = BytesMut::new();
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
                let mut buf = BytesMut::new();
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
            // 消息增量 — 通常包含 stop_reason
        }
        "message_stop" => {
            send_codex_completion_sequence(state, outputs);
        }
        _ => {}
    }
}

/// 发送 Codex 完成事件序列。
fn send_codex_completion_sequence(
    state: &mut ClaudeToCodexStreamState,
    outputs: &mut Vec<Vec<u8>>,
) {
    use crate::proxy::translator::sse;

    let item_id = state.item_id.as_deref().unwrap_or("item-0");
    let response_id = state.response_id.as_deref().unwrap_or("resp-0");

    // response.output_text.done
    let ev = json!({
        "type": "response.output_text.done",
        "item_id": item_id,
        "output_index": 0,
        "content_index": 0,
        "text": state.text_buffer
    });
    let mut buf = BytesMut::new();
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
    let mut buf = BytesMut::new();
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
    let mut buf = BytesMut::new();
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
            "model": ""
        }
    });
    let mut buf = BytesMut::new();
    sse::append_sse_event_no_id(
        &mut buf,
        "data",
        &serde_json::to_vec(&ev).unwrap_or_default(),
    );
    outputs.push(buf.to_vec());

    // [DONE]
    outputs.push(sse::openai_done_marker());
}

// ─── 注册函数 ───

pub fn register(registry: &mut crate::proxy::translator::TranslatorRegistry) {
    use crate::proxy::translator::format::Format;
    use crate::proxy::translator::types::{ResponseTransform, StreamStateFactory};

    let state_factory: StreamStateFactory = || Box::new(ClaudeToCodexStreamState::default());

    registry.register(
        Format::Codex,
        Format::Claude,
        codex_to_claude_request,
        ResponseTransform {
            stream: Some(claude_to_codex_stream),
            non_stream: Some(claude_to_codex_response_non_stream),
            token_count: None,
        },
        Some(state_factory),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codex_to_claude_request_basic() {
        let input = json!({
            "model": "claude-sonnet-4",
            "instructions": "You are a coding assistant.",
            "input": "user: Write a hello world in Rust",
            "stream": true,
            "temperature": 0.7,
            "max_tokens": 4096
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = codex_to_claude_request("claude-sonnet-4", &input_bytes, true);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["model"], "claude-sonnet-4");
        assert_eq!(output_val["system"], "You are a coding assistant.");
        let messages = output_val["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Write a hello world in Rust");
        assert_eq!(output_val["max_tokens"], 4096);
    }

    #[test]
    fn test_codex_to_claude_request_multi_turn() {
        let input = json!({
            "model": "claude-sonnet-4",
            "instructions": "Be helpful.",
            "input": "user: Hello\nassistant: Hi there!\nuser: How are you?",
            "stream": false
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = codex_to_claude_request("claude-sonnet-4", &input_bytes, false);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["system"], "Be helpful.");
        let messages = output_val["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Hello");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], "Hi there!");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(messages[2]["content"], "How are you?");
    }

    #[test]
    fn test_codex_to_claude_request_no_instructions() {
        let input = json!({
            "model": "claude-sonnet-4",
            "input": "user: Just a simple question",
            "stream": true
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = codex_to_claude_request("claude-sonnet-4", &input_bytes, true);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert!(!output_val.as_object().unwrap().contains_key("system"));
    }

    #[test]
    fn test_parse_codex_input_basic() {
        let messages = parse_codex_input_to_claude_messages("user: Hello\nassistant: Hi");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Hello");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[1]["content"], "Hi");
    }

    #[test]
    fn test_parse_codex_input_empty() {
        let messages = parse_codex_input_to_claude_messages("");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "");
    }

    #[test]
    fn test_claude_to_codex_stream_state_default() {
        let state = ClaudeToCodexStreamState::default();
        assert!(state.response_id.is_none());
        assert!(state.item_id.is_none());
        assert!(state.text_buffer.is_empty());
        assert!(!state.has_sent_created);
        assert!(!state.has_sent_item_added);
        assert!(!state.has_sent_content_added);
    }

    #[test]
    fn test_claude_to_codex_non_stream_response() {
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
        let output = claude_to_codex_response_non_stream(
            "claude-sonnet-4",
            &[],
            &[],
            &upstream_bytes,
            &mut state,
        );
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["id"], "msg_abc123");
        assert_eq!(output_val["status"], "completed");
        let output_items = output_val["output"].as_array().unwrap();
        assert_eq!(output_items.len(), 1);
        let content = output_items[0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "output_text");
        assert_eq!(content[0]["text"], "Hello from Claude!");
    }
}
