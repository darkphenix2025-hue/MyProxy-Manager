use bytes::BytesMut;
/// OpenAI → Codex 转换对。
///
/// 请求方向：OpenAI Chat Completions → Codex wire protocol
///   - `messages` → `instructions` + `input` 格式
///   - system messages 合并为 `instructions`
///   - 其余 messages 合并为 `input` 字符串
///
/// 响应方向：Codex SSE → OpenAI Chat Completions SSE
///   - `response.created` → 打开
///   - `response.output_text.delta` → delta.content
///   - `response.output_text.done` → 结束
///   - `response.completed` → [DONE]
use serde_json::{json, Value};

// ─── 请求转换 ───

/// 将 OpenAI Chat Completions 请求转换为 Codex 格式。
///
/// Codex 请求格式：
/// - `model`: 模型名
/// - `instructions`: system prompt 字符串
/// - `input`: 用户输入字符串（messages 合并）
/// - `stream`: boolean
/// - `temperature`, `top_p`: 透传
pub fn openai_to_codex_request(model: &str, raw_json: &[u8], stream: bool) -> Vec<u8> {
    let root: Value = serde_json::from_slice(raw_json).unwrap_or(Value::Null);
    let messages = root
        .get("messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 提取 system messages 为 instructions
    let instructions: Vec<String> = messages
        .iter()
        .filter(|m| m.get("role").and_then(|v| v.as_str()) == Some("system"))
        .filter_map(|m| {
            m.get("content")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string())
        })
        .collect();
    let instructions_str = if instructions.is_empty() {
        None
    } else {
        Some(instructions.join("\n"))
    };

    // 将非 system messages 合并为 input 字符串
    let input_parts: Vec<String> = messages
        .iter()
        .filter(|m| m.get("role").and_then(|v| v.as_str()) != Some("system"))
        .filter_map(|m| {
            let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content = m.get("content").and_then(|c| c.as_str()).unwrap_or("");
            if content.is_empty() {
                None
            } else {
                Some(format!("{}: {}", role, content))
            }
        })
        .collect();
    let input_str = if input_parts.is_empty() {
        String::new()
    } else {
        input_parts.join("\n")
    };

    let mut body = json!({
        "model": model,
        "input": input_str,
        "stream": stream,
    });

    if let Some(ref inst) = instructions_str {
        body["instructions"] = Value::String(inst.clone());
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

    // max_tokens
    if let Some(mt) = root.get("max_tokens").and_then(|v| v.as_u64()) {
        body["max_tokens"] = Value::Number(mt.into());
    }

    serde_json::to_vec(&body).unwrap_or(raw_json.to_vec())
}

// ─── 非流式响应转换 ───

/// Codex 非流式响应 → OpenAI Chat Completions 格式。
///
/// Codex 非流式响应格式：
/// { "id": "resp-xxx", "status": "completed", "output": [...], "usage": {...} }
pub fn codex_to_openai_response_non_stream(
    model: &str,
    _original_request: &[u8],
    _converted_request: &[u8],
    upstream_json: &[u8],
    _state: &mut (dyn std::any::Any + Send),
) -> Vec<u8> {
    let upstream: Value = serde_json::from_slice(upstream_json).unwrap_or(Value::Null);

    // 提取 output 中的文本
    let text_content: String = extract_codex_output_text(&upstream);

    let response = json!({
        "id": upstream.get("id").and_then(|v| v.as_str()).unwrap_or("chatcmpl-0"),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": if text_content.is_empty() { Value::Null } else { Value::String(text_content) },
            },
            "finish_reason": match upstream.get("status").and_then(|v| v.as_str()) {
                Some("completed") => "stop",
                Some("failed") => "stop",
                _ => "stop",
            },
        }],
        "usage": upstream.get("usage").cloned().unwrap_or(json!({
            "prompt_tokens": 0,
            "completion_tokens": 0,
            "total_tokens": 0,
        })),
    });

    serde_json::to_vec(&response).unwrap_or(upstream_json.to_vec())
}

/// 从 Codex 响应 output 数组中提取文本。
fn extract_codex_output_text(root: &Value) -> String {
    let output = root
        .get("output")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut texts = Vec::new();

    for item in output {
        if let Some(content) = item.get("content").and_then(|v| v.as_array()) {
            for part in content {
                if part.get("type").and_then(|v| v.as_str()) == Some("output_text") {
                    if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        texts.push(text.to_string());
                    }
                }
            }
        }
        // 也检查顶层 text 字段（某些 Codex 变体）
        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
            texts.push(text.to_string());
        }
    }

    // 也检查 response 级别的 text
    if let Some(text) = root.get("text").and_then(|v| v.as_str()) {
        if !texts.is_empty() {
            texts.insert(0, text.to_string());
        } else {
            return text.to_string();
        }
    }

    texts.join("\n")
}

// ─── 流式响应转换 ───

/// Codex SSE 流 → OpenAI 流状态机。
#[derive(Default)]
pub struct CodexToOpenAIStreamState {
    pub response_id: Option<String>,
    pub item_id: Option<String>,
    pub text_buffer: String,
    pub has_sent_role: bool,
}

pub fn codex_to_openai_stream(
    model: &str,
    _original_request: &[u8],
    _converted_request: &[u8],
    upstream_chunk: &[u8],
    state: &mut (dyn std::any::Any + Send),
) -> Vec<Vec<u8>> {
    let state = state.downcast_mut::<CodexToOpenAIStreamState>().unwrap();
    let mut outputs: Vec<Vec<u8>> = Vec::new();

    let text = std::str::from_utf8(upstream_chunk).unwrap_or("");
    let lines = text.lines();
    let mut data = String::new();

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            if !data.is_empty() {
                process_codex_event(model, &data, state, &mut outputs);
                data.clear();
            }
        } else if let Some(rest) = line.strip_prefix("data: ") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        }
    }

    outputs
}

fn process_codex_event(
    model: &str,
    data: &str,
    state: &mut CodexToOpenAIStreamState,
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

    let event_type = parsed.get("type").and_then(|v| v.as_str());

    match event_type {
        Some("response.created") => {
            if let Some(resp) = parsed.get("response") {
                if let Some(id) = resp.get("id").and_then(|v| v.as_str()) {
                    state.response_id = Some(id.to_string());
                }
            }
        }
        Some("response.output_item.added") => {
            if let Some(item_id) = parsed.get("item_id").and_then(|v| v.as_str()) {
                state.item_id = Some(item_id.to_string());
            }
            // 也可以从 item 中提取
            if let Some(item) = parsed.get("item") {
                if let Some(id) = item.get("id").and_then(|v| v.as_str()) {
                    state.item_id = Some(id.to_string());
                }
            }
        }
        Some("response.output_text.delta") => {
            if let Some(delta_text) = parsed.get("delta").and_then(|v| v.as_str()) {
                if !state.has_sent_role {
                    let chunk = json!({
                        "id": state.response_id.as_deref().unwrap_or("chatcmpl-0"),
                        "object": "chat.completion.chunk",
                        "created": chrono::Utc::now().timestamp(),
                        "model": model,
                        "choices": [{
                            "index": 0,
                            "delta": { "role": "assistant", "content": delta_text },
                            "finish_reason": null,
                        }]
                    });
                    let mut buf = BytesMut::new();
                    sse::append_sse_event_no_id(
                        &mut buf,
                        "message",
                        &serde_json::to_vec(&chunk).unwrap_or_default(),
                    );
                    outputs.push(buf.to_vec());
                    state.has_sent_role = true;
                } else {
                    let chunk = json!({
                        "id": state.response_id.as_deref().unwrap_or("chatcmpl-0"),
                        "object": "chat.completion.chunk",
                        "created": chrono::Utc::now().timestamp(),
                        "model": model,
                        "choices": [{
                            "index": 0,
                            "delta": { "content": delta_text },
                            "finish_reason": null,
                        }]
                    });
                    let mut buf = BytesMut::new();
                    sse::append_sse_event_no_id(
                        &mut buf,
                        "message",
                        &serde_json::to_vec(&chunk).unwrap_or_default(),
                    );
                    outputs.push(buf.to_vec());
                }
                state.text_buffer.push_str(delta_text);
            }
        }
        Some("response.output_text.done") => {
            // 文本完成，发送 finish_reason
            let chunk = json!({
                "id": state.response_id.as_deref().unwrap_or("chatcmpl-0"),
                "object": "chat.completion.chunk",
                "created": chrono::Utc::now().timestamp(),
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": {},
                    "finish_reason": "stop",
                }]
            });
            let mut buf = BytesMut::new();
            sse::append_sse_event_no_id(
                &mut buf,
                "message",
                &serde_json::to_vec(&chunk).unwrap_or_default(),
            );
            outputs.push(buf.to_vec());
        }
        Some("response.content_part.done") | Some("response.output_item.done") => {
            // 内容块/项目完成
        }
        Some("response.completed") => {
            // 最终完成
            if let Some(resp) = parsed.get("response") {
                // 提取 usage
                if let Some(usage) = resp.get("usage") {
                    let usage_chunk = json!({
                        "id": state.response_id.as_deref().unwrap_or("chatcmpl-0"),
                        "object": "chat.completion.chunk",
                        "created": chrono::Utc::now().timestamp(),
                        "model": model,
                        "choices": [],
                        "usage": usage
                    });
                    let mut buf = BytesMut::new();
                    sse::append_sse_event_no_id(
                        &mut buf,
                        "message",
                        &serde_json::to_vec(&usage_chunk).unwrap_or_default(),
                    );
                    outputs.push(buf.to_vec());
                }
            }
            outputs.push(sse::openai_done_marker());
        }
        Some("error") => {
            // 错误事件
            if let Some(error) = parsed.get("error") {
                let error_msg = error
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error");
                let chunk = json!({
                    "id": state.response_id.as_deref().unwrap_or("chatcmpl-0"),
                    "object": "chat.completion.chunk",
                    "created": chrono::Utc::now().timestamp(),
                    "model": model,
                    "choices": [{
                        "index": 0,
                        "delta": {},
                        "finish_reason": "error",
                    }],
                    "error": { "message": error_msg }
                });
                let mut buf = BytesMut::new();
                sse::append_sse_event_no_id(
                    &mut buf,
                    "message",
                    &serde_json::to_vec(&chunk).unwrap_or_default(),
                );
                outputs.push(buf.to_vec());
            }
            outputs.push(sse::openai_done_marker());
        }
        _ => {}
    }
}

// ─── 注册函数 ───

pub fn register(registry: &mut crate::proxy::translator::TranslatorRegistry) {
    use crate::proxy::translator::format::Format;
    use crate::proxy::translator::types::{ResponseTransform, StreamStateFactory};

    let state_factory: StreamStateFactory = || Box::new(CodexToOpenAIStreamState::default());

    registry.register(
        Format::OpenAI,
        Format::Codex,
        openai_to_codex_request,
        ResponseTransform {
            stream: Some(codex_to_openai_stream),
            non_stream: Some(codex_to_openai_response_non_stream),
            token_count: None,
        },
        Some(state_factory),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_to_codex_request_basic() {
        let input = json!({
            "model": "o3",
            "messages": [
                {"role": "system", "content": "You are a coding assistant."},
                {"role": "user", "content": "Write a hello world in Rust"}
            ],
            "stream": true,
            "temperature": 0.5
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = openai_to_codex_request("o3", &input_bytes, true);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["model"], "o3");
        assert_eq!(output_val["stream"], true);
        assert_eq!(output_val["instructions"], "You are a coding assistant.");
        assert_eq!(output_val["input"], "user: Write a hello world in Rust");
        assert_eq!(output_val["temperature"].as_f64().unwrap(), 0.5);
    }

    #[test]
    fn test_openai_to_codex_request_no_system() {
        let input = json!({
            "model": "o3",
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there!"},
                {"role": "user", "content": "How are you?"}
            ],
            "stream": false
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = openai_to_codex_request("o3", &input_bytes, false);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert!(!output_val.as_object().unwrap().contains_key("instructions"));
        assert_eq!(
            output_val["input"],
            "user: Hello\nassistant: Hi there!\nuser: How are you?"
        );
    }

    #[test]
    fn test_codex_to_openai_non_stream_response() {
        let codex_response = json!({
            "id": "resp_abc123",
            "status": "completed",
            "output": [
                {
                    "id": "item_xyz",
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {"type": "output_text", "text": "Hello from Codex!"}
                    ]
                }
            ],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });
        let upstream_bytes = serde_json::to_vec(&codex_response).unwrap();
        let mut state = ();
        let output =
            codex_to_openai_response_non_stream("o3", &[], &[], &upstream_bytes, &mut state);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["id"], "resp_abc123");
        assert_eq!(output_val["object"], "chat.completion");
        assert_eq!(output_val["model"], "o3");
        assert_eq!(output_val["choices"][0]["message"]["role"], "assistant");
        assert_eq!(
            output_val["choices"][0]["message"]["content"],
            "Hello from Codex!"
        );
        assert_eq!(output_val["choices"][0]["finish_reason"], "stop");
        assert_eq!(output_val["usage"]["prompt_tokens"], 10);
    }

    #[test]
    fn test_codex_to_openai_stream_state_default() {
        let state = CodexToOpenAIStreamState::default();
        assert!(state.response_id.is_none());
        assert!(state.item_id.is_none());
        assert!(state.text_buffer.is_empty());
        assert!(!state.has_sent_role);
    }

    #[test]
    fn test_extract_codex_output_text() {
        let root = json!({
            "output": [
                {
                    "id": "item_1",
                    "type": "message",
                    "content": [
                        {"type": "output_text", "text": "Hello"},
                        {"type": "output_text", "text": "World"}
                    ]
                }
            ]
        });
        let text = extract_codex_output_text(&root);
        assert_eq!(text, "Hello\nWorld");
    }
}
