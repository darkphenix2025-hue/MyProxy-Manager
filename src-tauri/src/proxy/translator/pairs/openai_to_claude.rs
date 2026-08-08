/// OpenAI → Claude 转换对。
///
/// 请求方向：OpenAI Chat Completions → Anthropic Messages API
/// 响应方向：Anthropic Messages → OpenAI Chat Completions
use serde_json::{json, Value};

// ─── 请求转换 ───

/// 将 OpenAI Chat Completions 请求转换为 Anthropic Messages 格式。
///
/// 转换要点：
/// - `messages[].role`: system→user, assistant→assistant, tool→user, user→user
/// - `messages[].content`: text/image/audio blocks → Claude 格式
/// - system messages 提取为顶层 `system` 字段
/// - `max_tokens`, `temperature`, `top_p` 直接映射
/// - `thinking` 参数映射为 Claude thinking 配置
/// - `tools` 数组直接传递
pub fn openai_to_claude_request(model: &str, raw_json: &[u8], stream: bool) -> Vec<u8> {
    let root: Value = serde_json::from_slice(raw_json).unwrap_or(Value::Null);
    let messages = root
        .get("messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 转换消息（system → user 角色）
    let converted_messages: Vec<Value> = messages
        .iter()
        .map(|m| {
            let role = match m
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_lowercase()
                .as_str()
            {
                "system" => "user",
                "assistant" => "assistant",
                "tool" | "function" => "user",
                _ => "user",
            };
            let content = convert_openai_content(m.get("content"));
            json!({ "role": role, "content": content })
        })
        .collect();

    // 提取 system messages
    let system_messages: Vec<String> = messages
        .iter()
        .filter(|m| m.get("role").and_then(|v| v.as_str()) == Some("system"))
        .filter_map(|m| {
            m.get("content")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    let system_str = if system_messages.is_empty() {
        None
    } else {
        Some(system_messages.join("\n"))
    };

    let mut body = json!({
        "model": model,
        "messages": converted_messages,
        "stream": stream,
    });

    if let Some(ref s) = system_str {
        body["system"] = Value::String(s.clone());
    }

    // max_tokens
    if let Some(mt) = root.get("max_tokens").and_then(|v| v.as_u64()) {
        body["max_tokens"] = Value::Number(mt.into());
    } else {
        // z.ai Anthropic passthrough requires max_tokens
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

    // thinking budget
    if let Some(thinking) = root.get("thinking") {
        body["thinking"] = json!({
            "type": thinking.get("type").and_then(|v| v.as_str()).unwrap_or("enabled"),
            "budget_tokens": thinking.get("budget_tokens").and_then(|v| v.as_u64()).unwrap_or(16384),
            "effort": thinking.get("effort").and_then(|v| v.as_str()),
        });
    }

    // tools
    if let Some(tools) = root.get("tools") {
        body["tools"] = tools.clone();
    }

    serde_json::to_vec(&body).unwrap_or(raw_json.to_vec())
}

/// 转换 OpenAI content 为 Claude content 格式。
fn convert_openai_content(content: Option<&Value>) -> Value {
    match content {
        None => Value::String(String::new()),
        Some(Value::String(s)) => Value::String(s.clone()),
        Some(Value::Array(blocks)) => {
            let parts: Vec<Value> = blocks
                    .iter()
                    .map(|b| {
                        let block_type = b.get("type").and_then(|v| v.as_str()).unwrap_or("text");
                        match block_type {
                            "text" => {
                                json!({ "type": "text", "text": b.get("text").and_then(|v| v.as_str()).unwrap_or("") })
                            }
                            "image_url" => {
                                let url = b.get("image_url").and_then(|v| v.get("url")).and_then(|v| v.as_str()).unwrap_or("");
                                json!({ "type": "image", "source": { "type": "url", "url": url } })
                            }
                            "input_audio" | "audio_url" => {
                                let url = b.get("audio_url").and_then(|v| v.get("url"))
                                    .or_else(|| b.get("input_audio").and_then(|v| v.get("url")))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                json!({ "type": "text", "text": format!("[audio: {}]", url) })
                            }
                            _ => json!({ "type": "text", "text": b.to_string() }),
                        }
                    })
                    .collect();
            if parts.len() == 1 {
                parts[0]["text"].clone()
            } else {
                Value::Array(parts)
            }
        }
        Some(other) => other.clone(),
    }
}

// ─── 非流式响应转换 ───

/// 将 Anthropic 非流式响应转换为 OpenAI Chat Completions 格式。
pub fn claude_to_openai_response_non_stream(
    model: &str,
    _original_request: &[u8],
    _converted_request: &[u8],
    upstream_json: &[u8],
    _state: &mut (dyn std::any::Any + Send),
) -> Vec<u8> {
    let upstream: Value = serde_json::from_slice(upstream_json).unwrap_or(Value::Null);

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
    let finish_reason = match stop_reason {
        "end_turn" | "stop" => "stop",
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        _ => "stop",
    };

    let mut response = json!({
        "id": upstream.get("id").and_then(|v| v.as_str()).unwrap_or("chatcmpl-0"),
        "object": "chat.completion",
        "created": upstream.get("created_at").map(|_| chrono::Utc::now().timestamp())
            .or(Some(chrono::Utc::now().timestamp())),
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": if text_content.is_empty() { Value::Null } else { Value::String(text_content) },
            },
            "finish_reason": finish_reason,
        }],
    });

    // Add reasoning_content if thinking is present
    if !thinking_content.is_empty() {
        response["choices"][0]["message"]["reasoning_content"] = json!(thinking_content);
    }

    // Usage mapping
    if let Some(usage) = upstream.get("usage") {
        response["usage"] = json!({
            "prompt_tokens": usage.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
            "completion_tokens": usage.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
            "total_tokens": usage.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0)
                + usage.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
        });
    }

    serde_json::to_vec(&response).unwrap_or(upstream_json.to_vec())
}

// ─── 流式响应转换 ───

/// Claude 流 → OpenAI 流状态机。
///
/// Claude SSE 事件:
/// - `message_start` → 打开
/// - `content_block_start` → 新 content block
/// - `content_block_delta` → delta.text / delta.thinking
/// - `message_delta` → 结束统计
/// - `message_stop` → [DONE]
pub struct ClaudeToOpenAIStreamState {
    pub message_id: Option<String>,
    pub content_index: usize,
    pub text_buffer: String,
    pub reasoning_buffer: String,
    pub tool_call_id: Option<String>,
    pub tool_call_name: Option<String>,
    pub tool_call_args: String,
    pub has_sent_role: bool,
}

impl Default for ClaudeToOpenAIStreamState {
    fn default() -> Self {
        Self {
            message_id: None,
            content_index: 0,
            text_buffer: String::new(),
            reasoning_buffer: String::new(),
            tool_call_id: None,
            tool_call_name: None,
            tool_call_args: String::new(),
            has_sent_role: false,
        }
    }
}

pub fn claude_to_openai_stream(
    model: &str,
    _original_request: &[u8],
    _converted_request: &[u8],
    upstream_chunk: &[u8],
    state: &mut (dyn std::any::Any + Send),
) -> Vec<Vec<u8>> {
    let state = state.downcast_mut::<ClaudeToOpenAIStreamState>().unwrap();
    let mut outputs: Vec<Vec<u8>> = Vec::new();

    // 解析上游 SSE chunk
    let text = std::str::from_utf8(upstream_chunk).unwrap_or("");
    let lines = text.lines();
    let mut event_type = String::new();
    let mut data = String::new();

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            if !event_type.is_empty() {
                process_claude_event(model, &event_type, &data, state, &mut outputs);
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

fn process_claude_event(
    model: &str,
    event_type: &str,
    data: &str,
    state: &mut ClaudeToOpenAIStreamState,
    outputs: &mut Vec<Vec<u8>>,
) {
    use crate::proxy::translator::sse;

    if data == "[DONE]" {
        outputs.push(sse::openai_done_marker());
        return;
    }

    let parsed: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(_) => return,
    };

    match event_type {
        "message_start" => {
            if let Some(id) = parsed
                .get("message")
                .and_then(|m| m.get("id"))
                .and_then(|v| v.as_str())
            {
                state.message_id = Some(id.to_string());
            }
        }
        "content_block_start" => {
            let block_type = parsed
                .get("content_block")
                .and_then(|b| b.get("type"))
                .and_then(|v| v.as_str());
            match block_type {
                Some("text") | None => {
                    state.content_index = 0;
                }
                Some("thinking") => {
                    state.content_index = 0;
                }
                Some("tool_use") => {
                    state.tool_call_id = parsed
                        .get("content_block")
                        .and_then(|b| b.get("id"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    state.tool_call_name = parsed
                        .get("content_block")
                        .and_then(|b| b.get("name"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    state.tool_call_args.clear();
                }
                _ => {}
            }
        }
        "content_block_delta" => {
            let delta = parsed.get("delta");
            if let Some(text_delta) = delta.and_then(|d| d.get("text")).and_then(|v| v.as_str()) {
                if !state.has_sent_role {
                    let chunk = json!({
                        "id": state.message_id.as_deref().unwrap_or("chatcmpl-0"),
                        "object": "chat.completion.chunk",
                        "created": chrono::Utc::now().timestamp(),
                        "model": model,
                        "choices": [{
                            "index": 0,
                            "delta": { "role": "assistant", "content": text_delta },
                            "finish_reason": null,
                        }]
                    });
                    let mut buf = bytes::BytesMut::new();
                    sse::append_sse_event_no_id(
                        &mut buf,
                        "message",
                        &serde_json::to_vec(&chunk).unwrap_or_default(),
                    );
                    outputs.push(buf.to_vec());
                    state.has_sent_role = true;
                } else {
                    let chunk = json!({
                        "id": state.message_id.as_deref().unwrap_or("chatcmpl-0"),
                        "object": "chat.completion.chunk",
                        "created": chrono::Utc::now().timestamp(),
                        "model": model,
                        "choices": [{
                            "index": 0,
                            "delta": { "content": text_delta },
                            "finish_reason": null,
                        }]
                    });
                    let mut buf = bytes::BytesMut::new();
                    sse::append_sse_event_no_id(
                        &mut buf,
                        "message",
                        &serde_json::to_vec(&chunk).unwrap_or_default(),
                    );
                    outputs.push(buf.to_vec());
                }
                state.text_buffer.push_str(text_delta);
            } else if let Some(thinking_delta) = delta
                .and_then(|d| d.get("thinking"))
                .and_then(|v| v.as_str())
            {
                let chunk = json!({
                    "id": state.message_id.as_deref().unwrap_or("chatcmpl-0"),
                    "object": "chat.completion.chunk",
                    "created": chrono::Utc::now().timestamp(),
                    "model": model,
                    "choices": [{
                        "index": 0,
                        "delta": { "reasoning_content": thinking_delta },
                        "finish_reason": null,
                    }]
                });
                let mut buf = bytes::BytesMut::new();
                sse::append_sse_event_no_id(
                    &mut buf,
                    "message",
                    &serde_json::to_vec(&chunk).unwrap_or_default(),
                );
                outputs.push(buf.to_vec());
                state.reasoning_buffer.push_str(thinking_delta);
            } else if let Some(input_delta) = delta
                .and_then(|d| d.get("input_json"))
                .and_then(|v| v.as_str())
            {
                state.tool_call_args.push_str(input_delta);
            } else if let Some(partial_json_delta) = delta
                .and_then(|d| d.get("partial_json"))
                .and_then(|v| v.as_str())
            {
                state.tool_call_args.push_str(partial_json_delta);
            }
        }
        "message_delta" => {
            let stop_reason = parsed
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(|v| v.as_str());
            let finish_reason = match stop_reason {
                Some("end_turn") | Some("stop") => "stop",
                Some("max_tokens") => "length",
                Some("tool_use") => "tool_calls",
                _ => "stop",
            };

            // 发送结束 chunk
            let chunk = json!({
                "id": state.message_id.as_deref().unwrap_or("chatcmpl-0"),
                "object": "chat.completion.chunk",
                "created": chrono::Utc::now().timestamp(),
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": finish_reason,
                }]
            });
            let mut buf = bytes::BytesMut::new();
            sse::append_sse_event_no_id(
                &mut buf,
                "message",
                &serde_json::to_vec(&chunk).unwrap_or_default(),
            );
            outputs.push(buf.to_vec());

            // 发送 usage 信息
            if let Some(usage) = parsed.get("usage") {
                let usage_chunk = json!({
                    "id": state.message_id.as_deref().unwrap_or("chatcmpl-0"),
                    "object": "chat.completion.chunk",
                    "created": chrono::Utc::now().timestamp(),
                    "model": model,
                    "choices": [],
                    "usage": {
                        "prompt_tokens": usage.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
                        "completion_tokens": usage.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
                        "total_tokens": usage.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0)
                            + usage.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
                    }
                });
                let mut buf = bytes::BytesMut::new();
                sse::append_sse_event_no_id(
                    &mut buf,
                    "message",
                    &serde_json::to_vec(&usage_chunk).unwrap_or_default(),
                );
                outputs.push(buf.to_vec());
            }
        }
        "message_stop" => {
            outputs.push(sse::openai_done_marker());
        }
        _ => {}
    }
}

// ─── 注册函数 ───

pub fn register(registry: &mut crate::proxy::translator::TranslatorRegistry) {
    use crate::proxy::translator::format::Format;
    use crate::proxy::translator::types::{ResponseTransform, StreamStateFactory};

    let state_factory: StreamStateFactory = || Box::new(ClaudeToOpenAIStreamState::default());

    registry.register(
        Format::OpenAI,
        Format::Claude,
        openai_to_claude_request,
        ResponseTransform {
            stream: Some(claude_to_openai_stream),
            non_stream: Some(claude_to_openai_response_non_stream),
            token_count: None,
        },
        Some(state_factory),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_to_claude_request_basic() {
        let input = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "system", "content": "You are helpful."},
                {"role": "user", "content": "Hello"}
            ],
            "stream": true,
            "temperature": 0.7,
            "max_tokens": 1024
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = openai_to_claude_request("gpt-4", &input_bytes, true);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["model"], "gpt-4");
        assert_eq!(output_val["stream"], true);
        assert_eq!(output_val["system"], "You are helpful.");
        let messages = output_val["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "You are helpful.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Hello");
        assert_eq!(output_val["max_tokens"], 1024);
        assert_eq!(output_val["temperature"].as_f64().unwrap(), 0.7);
    }

    #[test]
    fn test_openai_to_claude_request_no_system() {
        let input = json!({
            "model": "claude-sonnet-4",
            "messages": [{"role": "user", "content": "Hi"}],
            "stream": false
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = openai_to_claude_request("claude-sonnet-4", &input_bytes, false);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert!(!output_val.as_object().unwrap().contains_key("system"));
        assert_eq!(output_val["max_tokens"], 4096);
    }

    #[test]
    fn test_claude_to_openai_non_stream_response() {
        let claude_response = json!({
            "id": "msg_abc123",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Hello there!"}
            ],
            "model": "claude-sonnet-4-20250514",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        });
        let upstream_bytes = serde_json::to_vec(&claude_response).unwrap();
        let mut state = ();
        let output = claude_to_openai_response_non_stream(
            "claude-sonnet-4",
            &[],
            &[],
            &upstream_bytes,
            &mut state,
        );
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["id"], "msg_abc123");
        assert_eq!(output_val["object"], "chat.completion");
        assert_eq!(output_val["model"], "claude-sonnet-4");
        assert_eq!(output_val["choices"][0]["message"]["role"], "assistant");
        assert_eq!(
            output_val["choices"][0]["message"]["content"],
            "Hello there!"
        );
        assert_eq!(output_val["choices"][0]["finish_reason"], "stop");
        assert_eq!(output_val["usage"]["prompt_tokens"], 10);
        assert_eq!(output_val["usage"]["completion_tokens"], 5);
        assert_eq!(output_val["usage"]["total_tokens"], 15);
    }

    #[test]
    fn test_claude_to_openai_non_stream_with_thinking() {
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
        let output = claude_to_openai_response_non_stream(
            "claude-sonnet-4",
            &[],
            &[],
            &upstream_bytes,
            &mut state,
        );
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(
            output_val["choices"][0]["message"]["content"],
            "The answer is 42."
        );
        assert_eq!(
            output_val["choices"][0]["message"]["reasoning_content"],
            "Let me think..."
        );
    }

    #[test]
    fn test_claude_to_openai_stream_state_default() {
        let state = ClaudeToOpenAIStreamState::default();
        assert!(state.message_id.is_none());
        assert_eq!(state.content_index, 0);
        assert!(state.text_buffer.is_empty());
        assert!(state.reasoning_buffer.is_empty());
        assert!(!state.has_sent_role);
    }
}
