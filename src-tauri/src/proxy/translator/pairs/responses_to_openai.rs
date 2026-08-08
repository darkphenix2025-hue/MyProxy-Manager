use crate::proxy::translator::format::Format;
use crate::proxy::translator::types::{ResponseTransform, StreamStateFactory};
use crate::proxy::translator::TranslatorRegistry;
/// Claude Messages → OpenAI Responses 转换对。
///
/// 请求方向：Claude Messages API → OpenAI Responses API
///   - `system` → `instructions`
///   - `messages` → `input` 数组
///
/// 响应方向：OpenAI Responses SSE → Claude SSE（自动反转）
///   - `response.created` → `message_start`
///   - `response.output_text.delta` → `content_block_delta`
///   - `response.completed` → `message_stop` + 统计
use serde_json::{json, Value};

// ─── 请求转换 ───

/// 将 Claude Messages API 请求转换为 OpenAI Responses API 格式。
pub fn claude_to_responses_request(model: &str, raw_json: &[u8], stream: bool) -> Vec<u8> {
    let root: Value = serde_json::from_slice(raw_json).unwrap_or(Value::Null);

    // system → instructions
    let instructions = root
        .get("system")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // messages → input
    let messages = root
        .get("messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let input: Vec<Value> = messages
        .iter()
        .filter_map(|msg| {
            let role = msg.get("role")?.as_str()?;
            let content = msg.get("content")?;

            // 只处理 user 和 assistant 消息
            if role == "user" || role == "assistant" {
                let item = match content {
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
                Some(item)
            } else {
                None
            }
        })
        .collect();

    // 构建 Responses API 请求
    let mut resp_req = json!({
        "model": model,
        "input": input,
        "stream": stream
    });

    if let Some(ref inst) = instructions {
        resp_req["instructions"] = Value::String(inst.clone());
    }

    // max_tokens → max_output_tokens
    if let Some(mt) = root.get("max_tokens").and_then(|v| v.as_u64()) {
        resp_req["max_output_tokens"] = Value::Number(mt.into());
    }

    // temperature
    if let Some(t) = root.get("temperature").and_then(|v| v.as_f64()) {
        if let Some(num) = serde_json::Number::from_f64(t) {
            resp_req["temperature"] = Value::Number(num);
        }
    }

    // top_p
    if let Some(t) = root.get("top_p").and_then(|v| v.as_f64()) {
        if let Some(num) = serde_json::Number::from_f64(t) {
            resp_req["top_p"] = Value::Number(num);
        }
    }

    serde_json::to_vec(&resp_req).unwrap_or(raw_json.to_vec())
}

// ─── 非流式响应转换 ───

/// OpenAI Responses 非流式响应 → Claude Messages API 格式。
pub fn responses_to_claude_non_stream(
    model: &str,
    _original_request: &[u8],
    _converted_request: &[u8],
    upstream_json: &[u8],
    _state: &mut (dyn std::any::Any + Send),
) -> Vec<u8> {
    let resp: Value = serde_json::from_slice(upstream_json).unwrap_or(Value::Null);

    // 提取响应内容
    let output = resp
        .get("output")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 合并所有 output item 的文本内容
    let text_parts: Vec<String> = output
        .iter()
        .filter_map(|item| {
            item.get("content")
                .and_then(|c| c.as_array())
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|b| {
                            if b.get("type").and_then(|v| v.as_str()) == Some("output_text") {
                                b.get("text")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
        })
        .collect();

    let text_content = text_parts.join("\n");

    // 提取 reasoning
    let reasoning_parts: Vec<String> = output
        .iter()
        .filter_map(|item| {
            item.get("reasoning")
                .and_then(|r| r.as_array())
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|b| {
                            if b.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                                b.get("text")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
        })
        .collect();

    let reasoning_content = reasoning_parts.join("\n");

    let status = resp
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("completed");
    let stop_reason = match status {
        "completed" => "end_turn",
        "incomplete" => "max_tokens",
        _ => "end_turn",
    };

    let response_id = resp.get("id").and_then(|v| v.as_str()).unwrap_or("resp-0");

    // 构建 content blocks
    let mut content_blocks: Vec<Value> = Vec::new();
    if !reasoning_content.is_empty() {
        content_blocks.push(json!({
            "type": "thinking",
            "thinking": reasoning_content
        }));
    }
    content_blocks.push(json!({
        "type": "text",
        "text": text_content
    }));

    let claude_resp = json!({
        "id": response_id,
        "type": "message",
        "role": "assistant",
        "content": content_blocks,
        "model": model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": {
            "input_tokens": resp.get("usage").and_then(|u| u.get("input_tokens")).and_then(|v| v.as_i64()).unwrap_or(0),
            "output_tokens": resp.get("usage").and_then(|u| u.get("output_tokens")).and_then(|v| v.as_i64()).unwrap_or(0)
        }
    });

    serde_json::to_vec(&claude_resp).unwrap_or(upstream_json.to_vec())
}

// ─── 流式响应转换 ───

/// OpenAI Responses 流 → Claude SSE 格式。
pub fn responses_to_claude_stream(
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
                process_responses_event_for_claude(model, &event_type, &data, state, &mut outputs);
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

fn process_responses_event_for_claude(
    model: &str,
    event_type: &str,
    data: &str,
    state: &mut ResponsesToClaudeStreamState,
    outputs: &mut Vec<Vec<u8>>,
) {
    use crate::proxy::translator::sse;

    if data == "[DONE]" {
        // 发送 Claude 完成序列
        if !state.has_sent_stop {
            // message_stop
            let mut buf = bytes::BytesMut::new();
            sse::append_sse_event_no_id(&mut buf, "event", b"message_stop");
            buf.extend_from_slice(b"data: ");
            buf.extend_from_slice(b"{}\n\n");
            outputs.push(buf.to_vec());
            state.has_sent_stop = true;
        }
        return;
    }

    let parsed: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return,
    };

    // 初始化 message_id
    if state.message_id.is_none() {
        if let Some(id) = parsed
            .get("response")
            .and_then(|r| r.get("id"))
            .and_then(|v| v.as_str())
        {
            state.message_id = Some(id.to_string());
        } else if let Some(id) = parsed
            .get("item")
            .and_then(|i| i.get("id"))
            .and_then(|v| v.as_str())
        {
            state.message_id = Some(id.to_string());
        } else {
            state.message_id = Some("msg-resp-0".to_string());
        }
    }

    match event_type {
        "response.created" => {
            if !state.has_sent_start {
                let msg_id = state.message_id.as_deref().unwrap_or("msg-resp-0");
                // message_start
                let ev = json!({
                    "type": "message_start",
                    "message": {
                        "id": msg_id,
                        "type": "message",
                        "role": "assistant",
                        "model": model,
                        "content": [],
                        "stop_reason": null,
                        "stop_sequence": null,
                        "usage": {"input_tokens": 0}
                    }
                });
                let mut buf = bytes::BytesMut::new();
                sse::append_sse_event_no_id(&mut buf, "event", b"message_start");
                buf.extend_from_slice(b"data: ");
                buf.extend_from_slice(serde_json::to_string(&ev).unwrap_or_default().as_bytes());
                buf.extend_from_slice(b"\n\n");
                outputs.push(buf.to_vec());
                state.has_sent_start = true;
            }
        }
        "response.output_text.delta" => {
            if let Some(delta) = parsed.get("delta").and_then(|v| v.as_str()) {
                if !state.has_sent_block_start {
                    // content_block_start
                    let ev = json!({
                        "type": "content_block_start",
                        "index": 0,
                        "content_block": {
                            "type": "text",
                            "text": ""
                        }
                    });
                    let mut buf = bytes::BytesMut::new();
                    sse::append_sse_event_no_id(&mut buf, "event", b"content_block_start");
                    buf.extend_from_slice(b"data: ");
                    buf.extend_from_slice(
                        serde_json::to_string(&ev).unwrap_or_default().as_bytes(),
                    );
                    buf.extend_from_slice(b"\n\n");
                    outputs.push(buf.to_vec());
                    state.has_sent_block_start = true;
                }

                // content_block_delta
                let ev = json!({
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {
                        "type": "text_delta",
                        "text": delta
                    }
                });
                let mut buf = bytes::BytesMut::new();
                sse::append_sse_event_no_id(&mut buf, "event", b"content_block_delta");
                buf.extend_from_slice(b"data: ");
                buf.extend_from_slice(serde_json::to_string(&ev).unwrap_or_default().as_bytes());
                buf.extend_from_slice(b"\n\n");
                outputs.push(buf.to_vec());
                state.text_buffer.push_str(delta);
            }
        }
        "response.reasoning.delta" => {
            if let Some(delta) = parsed.get("delta").and_then(|v| v.as_str()) {
                if !state.has_reasoning_block_start {
                    // content_block_start for thinking
                    let ev = json!({
                        "type": "content_block_start",
                        "index": state.reasoning_index,
                        "content_block": {
                            "type": "thinking",
                            "thinking": ""
                        }
                    });
                    let mut buf = bytes::BytesMut::new();
                    sse::append_sse_event_no_id(&mut buf, "event", b"content_block_start");
                    buf.extend_from_slice(b"data: ");
                    buf.extend_from_slice(
                        serde_json::to_string(&ev).unwrap_or_default().as_bytes(),
                    );
                    buf.extend_from_slice(b"\n\n");
                    outputs.push(buf.to_vec());
                    state.has_reasoning_block_start = true;
                }

                // content_block_delta for thinking
                let ev = json!({
                    "type": "content_block_delta",
                    "index": state.reasoning_index,
                    "delta": {
                        "type": "thinking_delta",
                        "thinking": delta
                    }
                });
                let mut buf = bytes::BytesMut::new();
                sse::append_sse_event_no_id(&mut buf, "event", b"content_block_delta");
                buf.extend_from_slice(b"data: ");
                buf.extend_from_slice(serde_json::to_string(&ev).unwrap_or_default().as_bytes());
                buf.extend_from_slice(b"\n\n");
                outputs.push(buf.to_vec());
                state.reasoning_buffer.push_str(delta);
            }
        }
        "response.completed" => {
            // 发送 Claude 结束序列
            if !state.has_sent_stop {
                // message_delta with usage
                if let Some(usage) = parsed.get("response").and_then(|r| r.get("usage")) {
                    let ev = json!({
                        "type": "message_delta",
                        "delta": {
                            "stop_reason": "end_turn",
                            "stop_sequence": null
                        },
                        "usage": {
                            "input_tokens": usage.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
                            "output_tokens": usage.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0)
                        }
                    });
                    let mut buf = bytes::BytesMut::new();
                    sse::append_sse_event_no_id(&mut buf, "event", b"message_delta");
                    buf.extend_from_slice(b"data: ");
                    buf.extend_from_slice(
                        serde_json::to_string(&ev).unwrap_or_default().as_bytes(),
                    );
                    buf.extend_from_slice(b"\n\n");
                    outputs.push(buf.to_vec());
                }

                // message_stop
                let mut buf = bytes::BytesMut::new();
                sse::append_sse_event_no_id(&mut buf, "event", b"message_stop");
                buf.extend_from_slice(b"data: ");
                buf.extend_from_slice(b"{}\n\n");
                outputs.push(buf.to_vec());
                state.has_sent_stop = true;
            }
        }
        _ => {
            // 未知事件，忽略
        }
    }
}

/// 流状态。
#[derive(Default)]
pub struct ResponsesToClaudeStreamState {
    pub message_id: Option<String>,
    pub text_buffer: String,
    pub reasoning_buffer: String,
    pub reasoning_index: usize,
    pub has_sent_start: bool,
    pub has_sent_block_start: bool,
    pub has_reasoning_block_start: bool,
    pub has_sent_stop: bool,
}

fn responses_to_claude_state_factory() -> Box<dyn std::any::Any + Send> {
    Box::new(ResponsesToClaudeStreamState::default())
}

// ─── 注册函数 ───

pub fn register(registry: &mut TranslatorRegistry) {
    registry.register(
        Format::Claude,
        Format::OpenAIResponses,
        claude_to_responses_request,
        ResponseTransform {
            stream: Some(responses_to_claude_stream),
            non_stream: Some(responses_to_claude_non_stream),
            token_count: None,
        },
        Some(responses_to_claude_state_factory as StreamStateFactory),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_to_responses_request_basic() {
        let input = json!({
            "model": "claude-sonnet-4",
            "system": "Be helpful.",
            "messages": [
                {"role": "user", "content": "Hello!"}
            ],
            "stream": true,
            "max_tokens": 1024
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = claude_to_responses_request("claude-sonnet-4", &input_bytes, true);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["model"], "claude-sonnet-4");
        assert_eq!(output_val["instructions"], "Be helpful.");
        assert_eq!(output_val["stream"], true);
        assert_eq!(output_val["max_output_tokens"], 1024);

        let input_arr = output_val["input"].as_array().unwrap();
        assert_eq!(input_arr.len(), 1);
        assert_eq!(input_arr[0]["role"], "user");
        assert_eq!(input_arr[0]["content"], "Hello!");
    }

    #[test]
    fn test_claude_to_responses_request_multi_turn() {
        let input = json!({
            "model": "claude-sonnet-4",
            "system": "Be concise.",
            "messages": [
                {"role": "user", "content": "What is Rust?"},
                {"role": "assistant", "content": "A systems language."},
                {"role": "user", "content": "Is it safe?"}
            ],
            "stream": false
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = claude_to_responses_request("claude-sonnet-4", &input_bytes, false);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        let input_arr = output_val["input"].as_array().unwrap();
        assert_eq!(input_arr.len(), 3);
    }

    #[test]
    fn test_claude_to_responses_request_no_system() {
        let input = json!({
            "model": "claude-sonnet-4",
            "messages": [
                {"role": "user", "content": "Hello"}
            ],
            "stream": false
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = claude_to_responses_request("claude-sonnet-4", &input_bytes, false);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert!(output_val.get("instructions").is_none() || output_val["instructions"].is_null());
    }

    #[test]
    fn test_responses_to_claude_non_stream() {
        let resp = json!({
            "id": "resp_abc123",
            "object": "response",
            "status": "completed",
            "output": [{
                "id": "msg_abc123",
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "Hello from Responses!"
                }]
            }],
            "model": "gpt-4",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let upstream_bytes = serde_json::to_vec(&resp).unwrap();
        let mut state = ();
        let output = responses_to_claude_non_stream("gpt-4", &[], &[], &upstream_bytes, &mut state);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["id"], "resp_abc123");
        assert_eq!(output_val["type"], "message");
        assert_eq!(output_val["role"], "assistant");
        let content = output_val["content"].as_array().unwrap();
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "Hello from Responses!");
        assert_eq!(output_val["stop_reason"], "end_turn");
        assert_eq!(output_val["usage"]["input_tokens"], 10);
    }

    #[test]
    fn test_responses_to_claude_non_stream_with_reasoning() {
        let resp = json!({
            "id": "resp_xyz",
            "object": "response",
            "status": "completed",
            "output": [{
                "id": "msg_xyz",
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": "The answer is 42."
                }],
                "reasoning": [{
                    "type": "reasoning",
                    "text": "Let me think..."
                }]
            }],
            "model": "gpt-4"
        });
        let upstream_bytes = serde_json::to_vec(&resp).unwrap();
        let mut state = ();
        let output = responses_to_claude_non_stream("gpt-4", &[], &[], &upstream_bytes, &mut state);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        let content = output_val["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["thinking"], "Let me think...");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "The answer is 42.");
    }

    #[test]
    fn test_responses_to_claude_state_default() {
        let state = ResponsesToClaudeStreamState::default();
        assert!(!state.has_sent_start);
        assert!(!state.has_sent_block_start);
        assert!(!state.has_reasoning_block_start);
        assert!(!state.has_sent_stop);
        assert!(state.message_id.is_none());
        assert!(state.text_buffer.is_empty());
        assert!(state.reasoning_buffer.is_empty());
    }
}
