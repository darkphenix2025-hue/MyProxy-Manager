/// Claude → Codex 转换对。
///
/// 请求方向：Claude Messages API → Codex wire protocol
///   - `system` → `instructions`
///   - `messages` → `input` 字符串
///
/// 响应方向：Codex SSE → OpenAI Chat Completions SSE（复用 openai_to_codex 的流状态机）
use serde_json::{json, Value};

// ─── 请求转换 ───

/// 将 Claude Messages API 请求转换为 Codex 格式。
pub fn claude_to_codex_request(model: &str, raw_json: &[u8], stream: bool) -> Vec<u8> {
    let root: Value = serde_json::from_slice(raw_json).unwrap_or(Value::Null);

    // system → instructions
    let instructions = root
        .get("system")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // messages → input 字符串
    let messages = root
        .get("messages")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let input_parts: Vec<String> = messages
        .iter()
        .filter_map(|m| {
            let role = m.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content = extract_claude_message_content(m);
            if content.is_empty() {
                None
            } else {
                Some(format!("{}: {}", role, content))
            }
        })
        .collect();
    let input_str = input_parts.join("\n");

    let mut body = json!({
        "model": model,
        "input": input_str,
        "stream": stream,
    });

    if let Some(ref inst) = instructions {
        body["instructions"] = Value::String(inst.clone());
    }

    // max_tokens
    if let Some(mt) = root.get("max_tokens").and_then(|v| v.as_u64()) {
        body["max_tokens"] = Value::Number(mt.into());
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

/// 提取 Claude 消息的文本内容（处理 content 为字符串或数组的情况）。
fn extract_claude_message_content(msg: &Value) -> String {
    match msg.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| {
                if p.get("type").and_then(|v| v.as_str()) == Some("text") {
                    p.get("text")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

// ─── 非流式响应转换 ───

/// Codex 非流式响应 → OpenAI Chat Completions 格式。
///
/// 复用 openai_to_codex 的逻辑（相同的 Codex 响应格式）。
pub fn codex_to_openai_response_non_stream_claude(
    model: &str,
    original_request: &[u8],
    converted_request: &[u8],
    upstream_json: &[u8],
    state: &mut (dyn std::any::Any + Send),
) -> Vec<u8> {
    // 委托给 openai_to_codex 的转换函数（Codex 响应格式相同）
    crate::proxy::translator::pairs::openai_to_codex::codex_to_openai_response_non_stream(
        model,
        original_request,
        converted_request,
        upstream_json,
        state,
    )
}

// ─── 流式响应转换 ───

/// Codex 流 → OpenAI 流（复用 openai_to_codex 的状态机）。
pub fn codex_to_openai_stream_claude(
    model: &str,
    original_request: &[u8],
    converted_request: &[u8],
    upstream_chunk: &[u8],
    state: &mut (dyn std::any::Any + Send),
) -> Vec<Vec<u8>> {
    crate::proxy::translator::pairs::openai_to_codex::codex_to_openai_stream(
        model,
        original_request,
        converted_request,
        upstream_chunk,
        state,
    )
}

/// 流状态工厂 — 复用 openai_to_codex 的 CodexToOpenAIStreamState。
fn claude_codex_state_factory() -> Box<dyn std::any::Any + Send> {
    Box::new(crate::proxy::translator::pairs::openai_to_codex::CodexToOpenAIStreamState::default())
}

// ─── 注册函数 ───

pub fn register(registry: &mut crate::proxy::translator::TranslatorRegistry) {
    use crate::proxy::translator::format::Format;
    use crate::proxy::translator::types::{ResponseTransform, StreamStateFactory};

    registry.register(
        Format::Claude,
        Format::Codex,
        claude_to_codex_request,
        ResponseTransform {
            stream: Some(codex_to_openai_stream_claude),
            non_stream: Some(codex_to_openai_response_non_stream_claude),
            token_count: None,
        },
        Some(claude_codex_state_factory as StreamStateFactory),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claude_to_codex_request_basic() {
        let input = json!({
            "model": "claude-sonnet-4",
            "system": "You are a coding assistant.",
            "messages": [
                {"role": "user", "content": "Write a hello world in Rust"}
            ],
            "stream": true,
            "max_tokens": 4096,
            "temperature": 0.7
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = claude_to_codex_request("claude-sonnet-4", &input_bytes, true);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["model"], "claude-sonnet-4");
        assert_eq!(output_val["instructions"], "You are a coding assistant.");
        assert_eq!(output_val["input"], "user: Write a hello world in Rust");
        assert_eq!(output_val["stream"], true);
        assert_eq!(output_val["max_tokens"], 4096);
    }

    #[test]
    fn test_claude_to_codex_request_no_system() {
        let input = json!({
            "model": "claude-sonnet-4",
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there!"}
            ],
            "stream": false
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = claude_to_codex_request("claude-sonnet-4", &input_bytes, false);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert!(!output_val.as_object().unwrap().contains_key("instructions"));
        assert_eq!(output_val["input"], "user: Hello\nassistant: Hi there!");
    }

    #[test]
    fn test_claude_to_codex_request_array_content() {
        let input = json!({
            "model": "claude-sonnet-4",
            "system": "Be helpful.",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Part 1"},
                        {"type": "text", "text": "Part 2"}
                    ]
                }
            ],
            "stream": true
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = claude_to_codex_request("claude-sonnet-4", &input_bytes, true);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["instructions"], "Be helpful.");
        assert_eq!(output_val["input"], "user: Part 1\nPart 2");
    }

    #[test]
    fn test_claude_to_codex_request_string_content() {
        let input = json!({
            "model": "claude-sonnet-4",
            "messages": [
                {"role": "user", "content": "Simple text message"}
            ],
            "stream": false
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = claude_to_codex_request("claude-sonnet-4", &input_bytes, false);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["input"], "user: Simple text message");
    }
}
