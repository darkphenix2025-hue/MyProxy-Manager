use crate::proxy::translator::format::Format;
use crate::proxy::translator::types::{ResponseTransform, StreamStateFactory};
use crate::proxy::translator::TranslatorRegistry;
/// Claude Messages → OpenAI Responses 转换对。
///
/// 请求方向：Claude Messages API → OpenAI Responses API
///   - `system` → `input` 中的 `developer` 消息
///   - `messages` → `input` 数组
///
/// 响应方向：OpenAI Responses SSE → Claude SSE（自动反转）
///   - `response.created` → `message_start`
///   - `response.output_text.delta` → `content_block_delta`
///   - `response.completed` → `message_stop` + 统计
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

// ─── 请求转换 ───

/// 将 Claude Messages API 请求转换为 OpenAI Responses API 格式。
pub fn claude_to_responses_request(model: &str, raw_json: &[u8], stream: bool) -> Vec<u8> {
    let root: Value = match serde_json::from_slice(raw_json) {
        Ok(value) => value,
        Err(_) => return raw_json.to_vec(),
    };

    // Claude content blocks 与 Responses input item 不是一一对应的：
    // 普通文本/图片属于 message，tool_use/tool_result 则是 Responses 的
    // 顶层 function_call/function_call_output item。
    let mut input = Vec::new();
    if let Some(system_blocks) = root.get("system").map(convert_system_content) {
        if !system_blocks.is_empty() {
            input.push(json!({
                "type": "message",
                "role": "developer",
                "content": system_blocks
            }));
        }
    }
    if let Some(messages) = root.get("messages").and_then(Value::as_array) {
        for message in messages {
            let role = message
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user");
            let content = message.get("content").unwrap_or(&Value::Null);
            let (message_blocks, special_items) = convert_claude_content(role, content);

            if !message_blocks.is_empty() {
                let normalized_role = if role == "assistant" {
                    "assistant"
                } else {
                    "user"
                };
                input.push(json!({
                    "type": "message",
                    "role": normalized_role,
                    "content": message_blocks
                }));
            }
            input.extend(special_items);
        }
    }

    // 构建 Responses API 请求
    let mut resp_req = json!({
        "model": model,
        "input": input,
        "stream": stream
    });

    // Claude 的工具定义转换为 Responses 的扁平 function tool 结构。
    if let Some(tools) = root.get("tools").and_then(Value::as_array) {
        let converted_tools: Vec<Value> = tools
            .iter()
            .filter_map(|tool| {
                if is_claude_web_search_tool(tool) {
                    let mut web_search = json!({"type": "web_search"});
                    if let Some(allowed_domains) = tool.get("allowed_domains") {
                        web_search["filters"]["allowed_domains"] = allowed_domains.clone();
                    }
                    if let Some(user_location) = tool.get("user_location") {
                        web_search["user_location"] = user_location.clone();
                    }
                    return Some(web_search);
                }
                let name = tool.get("name").and_then(Value::as_str)?;
                Some(json!({
                    "type": "function",
                    "name": shorten_codex_tool_name(name),
                    "description": tool
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    "parameters": normalize_tool_parameters(
                        tool.get("input_schema")
                            .cloned()
                            .unwrap_or_else(|| json!({"type": "object", "properties": {}}))
                    ),
                    "strict": false
                }))
            })
            .collect();
        if !converted_tools.is_empty() {
            resp_req["tools"] = Value::Array(converted_tools);
        }
    }

    if let Some(tool_choice) = convert_tool_choice(root.get("tool_choice")) {
        resp_req["tool_choice"] = tool_choice;
    }

    // Claude 的 output_config/thinking 是协议扩展字段，Codex Responses
    // 使用 reasoning.effort。路由覆盖值在发送前会再次写入此字段。
    let effort = extract_reasoning_effort(&root).unwrap_or_else(|| "medium".to_string());
    resp_req["reasoning"] = json!({"effort": effort});

    // The ChatGPT Codex backend has a narrower Responses schema than the
    // public OpenAI Responses API. In particular it rejects token-limit and
    // sampling parameters, so Claude's max_tokens/temperature/top_p/stop
    // fields must not be translated into max_output_tokens/temperature/top_p/stop.
    resp_req["parallel_tool_calls"] = Value::Bool(true);
    resp_req["reasoning"]["summary"] = Value::String("auto".to_string());
    resp_req["include"] = json!(["reasoning.encrypted_content"]);
    resp_req["store"] = Value::Bool(false);

    if let Some(disable_parallel) = root
        .get("tool_choice")
        .and_then(|choice| choice.get("disable_parallel_tool_use"))
        .and_then(Value::as_bool)
    {
        resp_req["parallel_tool_calls"] = Value::Bool(!disable_parallel);
    }

    serde_json::to_vec(&resp_req).unwrap_or(raw_json.to_vec())
}

fn value_to_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let text = blocks
                .iter()
                .filter_map(|block| {
                    if block.get("type").and_then(Value::as_str) == Some("text") {
                        block.get("text").and_then(Value::as_str)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn convert_system_content(value: &Value) -> Vec<Value> {
    match value {
        Value::String(text) if !text.is_empty() => vec![json!({
            "type": "input_text",
            "text": text
        })],
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| {
                (block.get("type").and_then(Value::as_str) == Some("text"))
                    .then(|| block.get("text").and_then(Value::as_str))
                    .flatten()
                    .filter(|text| !text.is_empty())
                    .map(|text| json!({"type": "input_text", "text": text}))
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn convert_claude_content(role: &str, content: &Value) -> (Vec<Value>, Vec<Value>) {
    let blocks = match content {
        Value::String(text) => vec![json!({
            "type": "text",
            "text": text
        })],
        Value::Array(blocks) => blocks.clone(),
        _ => Vec::new(),
    };

    let mut message_blocks = Vec::new();
    let mut special_items = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str).unwrap_or("text") {
            "text" => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    message_blocks.push(json!({
                        "type": if role == "assistant" { "output_text" } else { "input_text" },
                        "text": text
                    }));
                }
            }
            "image" if role != "assistant" => {
                if let Some(image_url) = claude_image_url(&block) {
                    message_blocks.push(json!({
                        "type": "input_image",
                        "image_url": image_url
                    }));
                }
            }
            "document" if role != "assistant" => {
                if let Some(file_data) = claude_document_url(&block) {
                    message_blocks.push(json!({
                        "type": "input_file",
                        "file_data": file_data
                    }));
                }
            }
            "tool_use" | "server_tool_use" => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("call_unknown");
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let arguments = block.get("input").cloned().unwrap_or_else(|| json!({}));
                special_items.push(json!({
                    "type": "function_call",
                    "id": codex_function_call_id(id),
                    "call_id": shorten_codex_call_id(id),
                    "name": shorten_codex_tool_name(name),
                    "arguments": serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".into())
                }));
            }
            "tool_result" | "web_search_tool_result" => {
                let call_id = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .unwrap_or("call_unknown");
                let output = block
                    .get("content")
                    .map(value_to_tool_output)
                    .unwrap_or_default();
                special_items.push(json!({
                    "type": "function_call_output",
                    "call_id": shorten_codex_call_id(call_id),
                    "output": output
                }));
            }
            // Thinking/signature blocks are model-internal state. Sending them as
            // user-visible text would leak hidden reasoning into the next turn.
            "thinking" | "redacted_thinking" => {}
            _ => {}
        }
    }
    (message_blocks, special_items)
}

fn value_to_tool_output(value: &Value) -> String {
    value_to_text(value).unwrap_or_else(|| match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => serde_json::to_string(other).unwrap_or_default(),
    })
}

fn claude_image_url(block: &Value) -> Option<String> {
    let source = block.get("source")?;
    if source.get("type").and_then(Value::as_str) == Some("url") {
        return source.get("url").and_then(Value::as_str).map(str::to_owned);
    }
    let media_type = source.get("media_type").and_then(Value::as_str)?;
    let data = source.get("data").and_then(Value::as_str)?;
    Some(format!("data:{media_type};base64,{data}"))
}

fn claude_document_url(block: &Value) -> Option<String> {
    let source = block.get("source")?;
    let media_type = source.get("media_type").and_then(Value::as_str)?;
    let data = source.get("data").and_then(Value::as_str)?;
    Some(format!("data:{media_type};base64,{data}"))
}

fn is_claude_web_search_tool(tool: &Value) -> bool {
    matches!(
        tool.get("type").and_then(Value::as_str),
        Some("web_search_20250305") | Some("web_search_20260209")
    )
}

fn normalize_tool_parameters(mut schema: Value) -> Value {
    let Some(object) = schema.as_object_mut() else {
        return json!({"type": "object", "properties": {}});
    };

    object.remove("$schema");
    if !object.contains_key("type") {
        object.insert("type".to_string(), Value::String("object".to_string()));
    }
    if object.get("type").and_then(Value::as_str) == Some("object")
        && !object.contains_key("properties")
    {
        object.insert("properties".to_string(), json!({}));
    }
    schema
}

fn shorten_codex_tool_name(name: &str) -> String {
    const LIMIT: usize = 64;
    if name.len() <= LIMIT {
        return name.to_string();
    }
    if let Some(separator) = name.strip_prefix("mcp__").and_then(|rest| rest.rfind("__")) {
        let suffix = &name[5 + separator + 2..];
        let candidate = format!("mcp__{suffix}");
        if candidate.len() <= LIMIT {
            return candidate;
        }
    }
    name.chars().take(LIMIT).collect()
}

pub(crate) fn shorten_codex_call_id(id: &str) -> String {
    const LIMIT: usize = 64;
    if id.len() <= LIMIT {
        return id.to_string();
    }

    let digest = Sha256::digest(id.as_bytes());
    let suffix = format!(
        "_{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7]
    );
    let prefix_len = LIMIT.saturating_sub(suffix.len());
    format!(
        "{}{}",
        id.chars().take(prefix_len).collect::<String>(),
        suffix
    )
}

/// Responses function-call items use two different identifiers:
/// `id` is an item ID beginning with `fc_`, while `call_id` is the stable
/// tool-call correlation ID used by the subsequent function_call_output.
/// Claude tool IDs commonly begin with `call_` or `toolu_`, so they cannot be
/// copied directly into the Responses `id` field.
pub(crate) fn codex_function_call_id(id: &str) -> String {
    const LIMIT: usize = 64;
    if id.starts_with("fc_") && id.len() <= LIMIT {
        return id.to_string();
    }

    let digest = Sha256::digest(id.as_bytes());
    let digest_hex = digest
        .iter()
        .take(16)
        .map(|byte| format!("{:02x}", byte))
        .collect::<String>();
    format!("fc_{digest_hex}")
}

fn convert_tool_choice(value: Option<&Value>) -> Option<Value> {
    let value = value?;
    if let Some(choice_type) = value.as_str() {
        return Some(match choice_type {
            "any" => json!("required"),
            "auto" | "none" => Value::String(choice_type.to_string()),
            _ => json!("auto"),
        });
    }
    let choice_type = value.get("type").and_then(Value::as_str)?;
    Some(match choice_type {
        "auto" => json!("auto"),
        "any" => json!("required"),
        "none" => json!("none"),
        "tool" => json!({
            "type": "function",
            "name": shorten_codex_tool_name(
                value.get("name").and_then(Value::as_str).unwrap_or("unknown")
            )
        }),
        _ => json!("auto"),
    })
}

fn extract_reasoning_effort(root: &Value) -> Option<String> {
    let direct = root
        .get("output_config")
        .and_then(|config| config.get("effort"))
        .and_then(Value::as_str)
        .or_else(|| {
            root.get("thinking")
                .and_then(|thinking| thinking.get("effort"))
                .and_then(Value::as_str)
        });
    if let Some(effort) = direct {
        return normalize_effort(effort);
    }

    let budget = root
        .get("thinking")
        .and_then(|thinking| thinking.get("budget_tokens"))
        .and_then(Value::as_u64)?;
    Some(
        match budget {
            0..=8_192 => "low",
            8_193..=16_384 => "medium",
            16_385..=24_576 => "high",
            _ => "xhigh",
        }
        .to_string(),
    )
}

fn normalize_effort(effort: &str) -> Option<String> {
    let normalized = effort.trim().to_ascii_lowercase();
    let normalized = match normalized.as_str() {
        "light" => "low",
        "extra high" | "extra_high" | "extra-high" => "xhigh",
        "max" | "ultra" | "xhigh" | "high" | "medium" | "low" => normalized.as_str(),
        _ => return None,
    };
    Some(normalized.to_string())
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

    let output = resp
        .get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut text_content = String::new();
    let mut content_blocks = Vec::new();
    let mut has_tool_use = false;

    for item in &output {
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "message" => {
                if let Some(reasoning) = item.get("reasoning") {
                    let summary = extract_reasoning_text(reasoning);
                    if !summary.is_empty() {
                        let mut block = json!({"type": "thinking", "thinking": summary});
                        if let Some(signature) = reasoning_signature(reasoning) {
                            block["signature"] = Value::String(signature);
                        }
                        content_blocks.push(block);
                    }
                }
                if let Some(blocks) = item.get("content").and_then(Value::as_array) {
                    for block in blocks {
                        if block.get("type").and_then(Value::as_str) == Some("output_text") {
                            if let Some(text) = block.get("text").and_then(Value::as_str) {
                                text_content.push_str(text);
                                content_blocks.push(json!({"type": "text", "text": text}));
                            }
                        }
                    }
                }
            }
            "function_call" => {
                has_tool_use = true;
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .or_else(|| item.get("id").and_then(Value::as_str))
                    .unwrap_or("call_unknown");
                let arguments = item
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!("{}"));
                let input = match arguments {
                    Value::String(arguments) => serde_json::from_str(&arguments)
                        .unwrap_or_else(|_| Value::String(arguments)),
                    other => other,
                };
                content_blocks.push(json!({
                    "type": "tool_use",
                    "id": call_id,
                    "name": item.get("name").and_then(Value::as_str).unwrap_or("unknown"),
                    "input": input
                }));
            }
            "reasoning" => {
                let summary = extract_reasoning_text(item);
                if !summary.is_empty() {
                    let mut block = json!({"type": "thinking", "thinking": summary});
                    if let Some(signature) = reasoning_signature(item) {
                        block["signature"] = Value::String(signature);
                    }
                    content_blocks.push(block);
                }
            }
            _ => {}
        }
    }

    if content_blocks.is_empty() && !text_content.is_empty() {
        content_blocks.push(json!({"type": "text", "text": text_content}));
    }

    let status = resp
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("completed");
    let stop_reason = if has_tool_use {
        "tool_use"
    } else {
        match status {
            "completed" => "end_turn",
            "incomplete" => "max_tokens",
            _ => "end_turn",
        }
    };

    let response_id = resp.get("id").and_then(|v| v.as_str()).unwrap_or("resp-0");

    if content_blocks.is_empty() {
        content_blocks.push(json!({
            "type": "text",
            "text": text_content
        }));
    }

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

fn extract_reasoning_text(value: &Value) -> String {
    if let Some(text) = value.get("text").and_then(Value::as_str) {
        return text.to_string();
    }
    if let Some(summary) = value.get("summary").and_then(Value::as_array) {
        return summary
            .iter()
            .filter_map(|entry| entry.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
    }
    if let Some(blocks) = value.as_array() {
        return blocks
            .iter()
            .filter_map(|entry| {
                entry
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| entry.get("thinking").and_then(Value::as_str))
            })
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

fn reasoning_signature(value: &Value) -> Option<String> {
    if let Some(entries) = value.as_array() {
        return entries.iter().find_map(reasoning_signature);
    }
    value
        .get("encrypted_content")
        .or_else(|| value.get("signature"))
        .and_then(Value::as_str)
        .filter(|signature| !signature.is_empty())
        .map(str::to_owned)
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
            if !data.is_empty() {
                // The proxy SSE parser passes only the data payload to the
                // translator. Native Responses payloads carry the event name
                // in `data.type`, so infer it when the `event:` line is gone.
                let inferred_event = if event_type.is_empty() {
                    serde_json::from_str::<Value>(&data)
                        .ok()
                        .and_then(|value| {
                            value.get("type").and_then(Value::as_str).map(str::to_owned)
                        })
                        .unwrap_or_default()
                } else {
                    event_type.clone()
                };
                process_responses_event_for_claude(
                    model,
                    &inferred_event,
                    &data,
                    state,
                    &mut outputs,
                );
            }
            event_type.clear();
            data.clear();
        } else if let Some(rest) = line.strip_prefix("event: ") {
            event_type = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("data: ") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest);
        }
    }

    // Be tolerant of a final event without the optional trailing blank line.
    if !data.is_empty() {
        let inferred_event = if event_type.is_empty() {
            serde_json::from_str::<Value>(&data)
                .ok()
                .and_then(|value| value.get("type").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_default()
        } else {
            event_type
        };
        process_responses_event_for_claude(model, &inferred_event, &data, state, &mut outputs);
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
    if data == "[DONE]" {
        finish_claude_stream(state, outputs, None);
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
            emit_message_start(model, state, outputs);
        }
        "response.reasoning_summary_part.added" => {
            finish_reasoning_block(outputs, state);
            emit_message_start(model, state, outputs);
            if !state.has_reasoning_block_start {
                state.reasoning_index = state.next_content_index;
                state.next_content_index += 1;
                push_claude_event(
                    outputs,
                    "content_block_start",
                    json!({
                        "type": "content_block_start",
                        "index": state.reasoning_index,
                        "content_block": {"type": "thinking", "thinking": ""}
                    }),
                );
                state.has_reasoning_block_start = true;
                state.reasoning_block_stopped = false;
            }
        }
        "response.content_part.added" => {
            if parsed
                .get("part")
                .and_then(|part| part.get("type"))
                .and_then(Value::as_str)
                == Some("output_text")
            {
                finish_reasoning_block(outputs, state);
                emit_message_start(model, state, outputs);
                emit_text_block_start(outputs, state);
            }
        }
        "response.output_text.delta" => {
            if let Some(delta) = parsed.get("delta").and_then(|v| v.as_str()) {
                finish_reasoning_block(outputs, state);
                emit_message_start(model, state, outputs);
                emit_text_block_start(outputs, state);

                // content_block_delta
                let ev = json!({
                    "type": "content_block_delta",
                    "index": state.text_block_index.unwrap_or(0),
                    "delta": {
                        "type": "text_delta",
                        "text": delta
                    }
                });
                push_claude_event(outputs, "content_block_delta", ev);
                state.text_buffer.push_str(delta);
            }
        }
        "response.output_text.done" => {
            emit_content_block_stop(
                outputs,
                state.text_block_index,
                &mut state.text_block_stopped,
            );
        }
        "response.reasoning.delta" | "response.reasoning_summary_text.delta" => {
            if let Some(delta) = parsed.get("delta").and_then(|v| v.as_str()) {
                state.reasoning_stop_pending = false;
                emit_message_start(model, state, outputs);
                if !state.has_reasoning_block_start {
                    state.reasoning_index = state.next_content_index;
                    state.next_content_index += 1;
                    let ev = json!({
                        "type": "content_block_start",
                        "index": state.reasoning_index,
                        "content_block": {
                            "type": "thinking",
                            "thinking": ""
                        }
                    });
                    push_claude_event(outputs, "content_block_start", ev);
                    state.has_reasoning_block_start = true;
                    state.reasoning_block_stopped = false;
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
                push_claude_event(outputs, "content_block_delta", ev);
                state.reasoning_buffer.push_str(delta);
            }
        }
        "response.reasoning_summary_text.done" | "response.reasoning_summary_part.done" => {
            state.reasoning_stop_pending = state.has_reasoning_block_start;
        }
        "response.output_item.added" => {
            if let Some(item) = parsed.get("item") {
                match item.get("type").and_then(Value::as_str) {
                    Some("reasoning") => {
                        emit_message_start(model, state, outputs);
                        state.reasoning_signature = item
                            .get("encrypted_content")
                            .and_then(Value::as_str)
                            .filter(|signature| !signature.is_empty())
                            .map(str::to_owned);
                    }
                    Some("function_call") => {
                        finish_reasoning_block(outputs, state);
                        emit_message_start(model, state, outputs);
                        state.has_tool_use = true;
                        state.stop_reason = Some("tool_use".to_string());
                        state.tool_call_id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .or_else(|| item.get("id").and_then(Value::as_str))
                            .map(str::to_owned);
                        state.tool_name =
                            item.get("name").and_then(Value::as_str).map(str::to_owned);
                        if !state.has_tool_block_start {
                            state.tool_block_index = Some(state.next_content_index);
                            state.next_content_index += 1;
                            let ev = json!({
                                "type": "content_block_start",
                                "index": state.tool_block_index,
                                "content_block": {
                                    "type": "tool_use",
                                    "id": state.tool_call_id.as_deref().unwrap_or("call_unknown"),
                                    "name": state.tool_name.as_deref().unwrap_or("unknown"),
                                    "input": {}
                                }
                            });
                            push_claude_event(outputs, "content_block_start", ev);
                            state.has_tool_block_start = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        "response.function_call_arguments.delta" | "response.mcp_call_arguments.delta" => {
            finish_reasoning_block(outputs, state);
            emit_message_start(model, state, outputs);
            if !state.has_tool_block_start {
                state.has_tool_use = true;
                state.stop_reason = Some("tool_use".to_string());
                state.tool_call_id = parsed
                    .get("item_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                state.tool_block_index = Some(state.next_content_index);
                state.next_content_index += 1;
                let ev = json!({
                    "type": "content_block_start",
                    "index": state.tool_block_index,
                    "content_block": {
                        "type": "tool_use",
                        "id": state.tool_call_id.as_deref().unwrap_or("call_unknown"),
                        "name": state.tool_name.as_deref().unwrap_or("unknown"),
                        "input": {}
                    }
                });
                push_claude_event(outputs, "content_block_start", ev);
                state.has_tool_block_start = true;
            }
            if let Some(delta) = parsed.get("delta").and_then(Value::as_str) {
                let ev = json!({
                    "type": "content_block_delta",
                    "index": state.tool_block_index.unwrap_or(0),
                    "delta": {"type": "input_json_delta", "partial_json": delta}
                });
                push_claude_event(outputs, "content_block_delta", ev);
                state.tool_arguments.push_str(delta);
            }
        }
        "response.function_call_arguments.done" | "response.mcp_call_arguments.done" => {
            if state.tool_arguments.is_empty() {
                if let Some(arguments) = parsed.get("arguments").and_then(Value::as_str) {
                    let ev = json!({
                        "type": "content_block_delta",
                        "index": state.tool_block_index.unwrap_or(0),
                        "delta": {"type": "input_json_delta", "partial_json": arguments}
                    });
                    push_claude_event(outputs, "content_block_delta", ev);
                    state.tool_arguments.push_str(arguments);
                }
            }
            emit_content_block_stop(
                outputs,
                state.tool_block_index,
                &mut state.tool_block_stopped,
            );
        }
        "response.output_item.done" => {
            if let Some(item) = parsed.get("item") {
                match item.get("type").and_then(Value::as_str) {
                    Some("reasoning") => {
                        if let Some(signature) = item
                            .get("encrypted_content")
                            .and_then(Value::as_str)
                            .filter(|signature| !signature.is_empty())
                        {
                            state.reasoning_signature = Some(signature.to_string());
                        }
                        if state.has_reasoning_block_start {
                            state.reasoning_stop_pending = true;
                        } else if state.reasoning_signature.is_some() {
                            emit_message_start(model, state, outputs);
                            state.reasoning_index = state.next_content_index;
                            state.next_content_index += 1;
                            push_claude_event(
                                outputs,
                                "content_block_start",
                                json!({
                                    "type": "content_block_start",
                                    "index": state.reasoning_index,
                                    "content_block": {"type": "thinking", "thinking": ""}
                                }),
                            );
                            state.has_reasoning_block_start = true;
                            state.reasoning_block_stopped = false;
                            finish_reasoning_block(outputs, state);
                        }
                    }
                    Some("function_call") => {
                        emit_content_block_stop(
                            outputs,
                            state.tool_block_index,
                            &mut state.tool_block_stopped,
                        );
                    }
                    _ => {}
                }
            }
        }
        "response.completed" | "response.incomplete" if !state.has_sent_stop => {
            if event_type == "response.incomplete" {
                let reason = parsed
                    .get("response")
                    .and_then(|response| response.get("incomplete_details"))
                    .and_then(|details| details.get("reason"))
                    .and_then(Value::as_str);
                state.stop_reason = Some(match reason {
                    Some("max_output_tokens") => "max_tokens".to_string(),
                    Some("content_filter") => "refusal".to_string(),
                    _ => "end_turn".to_string(),
                });
            }
            let usage = parsed
                .get("response")
                .and_then(|response| response.get("usage"));
            finish_claude_stream(state, outputs, usage);
        }
        _ => {}
    }
}

fn push_claude_event(outputs: &mut Vec<Vec<u8>>, event_type: &str, data: Value) {
    let mut buf = bytes::BytesMut::new();
    crate::proxy::translator::sse::append_sse_event_no_id(
        &mut buf,
        event_type,
        &serde_json::to_vec(&data).unwrap_or_default(),
    );
    outputs.push(buf.to_vec());
}

fn emit_message_start(
    model: &str,
    state: &mut ResponsesToClaudeStreamState,
    outputs: &mut Vec<Vec<u8>>,
) {
    if state.has_sent_start {
        return;
    }
    let message_id = state.message_id.as_deref().unwrap_or("msg-resp-0");
    push_claude_event(
        outputs,
        "message_start",
        json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {"input_tokens": 0}
            }
        }),
    );
    state.has_sent_start = true;
}

fn emit_text_block_start(outputs: &mut Vec<Vec<u8>>, state: &mut ResponsesToClaudeStreamState) {
    if state.has_sent_block_start {
        return;
    }

    state.text_block_index = Some(state.next_content_index);
    state.next_content_index += 1;
    push_claude_event(
        outputs,
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": state.text_block_index,
            "content_block": {"type": "text", "text": ""}
        }),
    );
    state.has_sent_block_start = true;
    state.text_block_stopped = false;
}

fn finish_reasoning_block(outputs: &mut Vec<Vec<u8>>, state: &mut ResponsesToClaudeStreamState) {
    if !state.has_reasoning_block_start || state.reasoning_block_stopped {
        state.reasoning_stop_pending = false;
        return;
    }

    if let Some(signature) = state.reasoning_signature.take() {
        push_claude_event(
            outputs,
            "content_block_delta",
            json!({
                "type": "content_block_delta",
                "index": state.reasoning_index,
                "delta": {"type": "signature_delta", "signature": signature}
            }),
        );
    }
    emit_content_block_stop(
        outputs,
        Some(state.reasoning_index),
        &mut state.reasoning_block_stopped,
    );
    state.reasoning_stop_pending = false;
}

fn emit_content_block_stop(outputs: &mut Vec<Vec<u8>>, index: Option<usize>, stopped: &mut bool) {
    if *stopped {
        return;
    }
    if let Some(index) = index {
        push_claude_event(
            outputs,
            "content_block_stop",
            json!({"type": "content_block_stop", "index": index}),
        );
        *stopped = true;
    }
}

fn finish_claude_stream(
    state: &mut ResponsesToClaudeStreamState,
    outputs: &mut Vec<Vec<u8>>,
    usage: Option<&Value>,
) {
    if state.has_sent_stop {
        return;
    }
    emit_content_block_stop(
        outputs,
        state.text_block_index,
        &mut state.text_block_stopped,
    );
    finish_reasoning_block(outputs, state);
    emit_content_block_stop(
        outputs,
        state.tool_block_index,
        &mut state.tool_block_stopped,
    );

    let (input_tokens, output_tokens) = usage
        .map(|usage| {
            (
                usage
                    .get("input_tokens")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
                usage
                    .get("output_tokens")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
            )
        })
        .unwrap_or((0, 0));
    push_claude_event(
        outputs,
        "message_delta",
        json!({
            "type": "message_delta",
            "delta": {
                "stop_reason": state.stop_reason.as_deref().unwrap_or("end_turn"),
                "stop_sequence": null
            },
            "usage": {"input_tokens": input_tokens, "output_tokens": output_tokens}
        }),
    );
    push_claude_event(outputs, "message_stop", json!({"type": "message_stop"}));
    state.has_sent_stop = true;
}

/// 流状态。
#[derive(Default)]
pub struct ResponsesToClaudeStreamState {
    pub message_id: Option<String>,
    pub text_buffer: String,
    pub reasoning_buffer: String,
    pub reasoning_index: usize,
    pub reasoning_signature: Option<String>,
    pub reasoning_stop_pending: bool,
    pub next_content_index: usize,
    pub has_sent_start: bool,
    pub has_sent_block_start: bool,
    pub text_block_index: Option<usize>,
    pub text_block_stopped: bool,
    pub has_reasoning_block_start: bool,
    pub reasoning_block_stopped: bool,
    pub has_tool_use: bool,
    pub tool_block_index: Option<usize>,
    pub tool_block_stopped: bool,
    pub has_tool_block_start: bool,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_arguments: String,
    pub stop_reason: Option<String>,
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
            "max_tokens": 1024,
            "temperature": 0.2,
            "top_p": 0.9,
            "stop_sequences": ["END"]
        });
        let input_bytes = serde_json::to_vec(&input).unwrap();
        let output = claude_to_responses_request("claude-sonnet-4", &input_bytes, true);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["model"], "claude-sonnet-4");
        assert!(output_val.get("instructions").is_none());
        assert_eq!(output_val["stream"], true);
        assert!(output_val.get("max_output_tokens").is_none());
        assert!(output_val.get("temperature").is_none());
        assert!(output_val.get("top_p").is_none());
        assert!(output_val.get("stop").is_none());

        let input_arr = output_val["input"].as_array().unwrap();
        assert_eq!(input_arr.len(), 2);
        assert_eq!(input_arr[0]["type"], "message");
        assert_eq!(input_arr[0]["role"], "developer");
        assert_eq!(input_arr[0]["content"][0]["type"], "input_text");
        assert_eq!(input_arr[0]["content"][0]["text"], "Be helpful.");
        assert_eq!(input_arr[1]["type"], "message");
        assert_eq!(input_arr[1]["role"], "user");
        assert_eq!(input_arr[1]["content"][0]["type"], "input_text");
        assert_eq!(input_arr[1]["content"][0]["text"], "Hello!");
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
        assert_eq!(input_arr.len(), 4);
        assert_eq!(input_arr[0]["role"], "developer");
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
    fn test_claude_to_responses_request_preserves_reasoning_tools_and_content_blocks() {
        let input = json!({
            "model": "claude-opus-5",
            "system": [{"type": "text", "text": "Be precise."}],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "Read this image."},
                        {
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": "image/png",
                                "data": "ZmFrZQ=="
                            }
                        }
                    ]
                },
                {
                    "role": "assistant",
                    "content": [
                        {
                            "type": "tool_use",
                            "id": "call_123",
                            "name": "lookup",
                            "input": {"query": "rust"}
                        }
                    ]
                },
                {
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": "call_123",
                        "content": "Rust is a systems language."
                    }]
                }
            ],
            "tools": [{
                "name": "lookup",
                "description": "Look something up",
                "input_schema": {
                    "type": "object",
                    "properties": {"query": {"type": "string"}}
                }
            }],
            "tool_choice": {"type": "auto"},
            "thinking": {"type": "adaptive"},
            "output_config": {"effort": "high"},
            "stream": true
        });

        let output =
            claude_to_responses_request("gpt-5.6-luna", &serde_json::to_vec(&input).unwrap(), true);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["model"], "gpt-5.6-luna");
        assert!(output_val.get("instructions").is_none());
        assert_eq!(output_val["reasoning"]["effort"], "high");
        assert_eq!(output_val["tool_choice"], "auto");
        assert_eq!(output_val["parallel_tool_calls"], true);
        assert_eq!(output_val["reasoning"]["summary"], "auto");
        assert_eq!(output_val["store"], false);
        assert_eq!(output_val["include"][0], "reasoning.encrypted_content");

        let input_items = output_val["input"].as_array().unwrap();
        assert_eq!(input_items[0]["role"], "developer");
        assert_eq!(input_items[0]["content"][0]["type"], "input_text");
        assert_eq!(input_items[0]["content"][0]["text"], "Be precise.");
        assert_eq!(input_items[1]["type"], "message");
        assert_eq!(input_items[1]["content"][0]["type"], "input_text");
        assert_eq!(input_items[1]["content"][1]["type"], "input_image");
        assert_eq!(input_items[2]["type"], "function_call");
        let function_item_id = input_items[2]["id"].as_str().unwrap();
        assert!(function_item_id.starts_with("fc_"));
        assert!(function_item_id.len() <= 64);
        assert_eq!(input_items[2]["call_id"], "call_123");
        assert_eq!(input_items[2]["arguments"], "{\"query\":\"rust\"}");
        assert_eq!(input_items[3]["type"], "function_call_output");
        assert_eq!(input_items[3]["output"], "Rust is a systems language.");

        assert_eq!(output_val["tools"][0]["type"], "function");
        assert_eq!(output_val["tools"][0]["name"], "lookup");
        assert_eq!(output_val["tools"][0]["parameters"]["type"], "object");
        assert_eq!(output_val["tools"][0]["strict"], false);
        assert!(output_val["tools"][0]["parameters"]
            .get("$schema")
            .is_none());
    }

    #[test]
    fn test_claude_to_responses_request_maps_codex_tool_choice_and_limits_identifiers() {
        let long_id = "call_".to_string() + &"x".repeat(100);
        let long_name = "mcp__server__".to_string() + &"tool".repeat(30);
        let input = json!({
            "model": "gpt-5.6-luna",
            "messages": [
                {"role": "assistant", "content": [{
                    "type": "tool_use",
                    "id": long_id,
                    "name": long_name,
                    "input": {"value": 1}
                }]},
                {"role": "user", "content": [{
                    "type": "tool_result",
                    "tool_use_id": long_id,
                    "content": "done"
                }]}
            ],
            "tools": [{
                "name": long_name,
                "description": "A long tool",
                "input_schema": {
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object"
                }
            }],
            "tool_choice": "any",
            "stream": true
        });

        let output =
            claude_to_responses_request("gpt-5.6-luna", &serde_json::to_vec(&input).unwrap(), true);
        let output_val: Value = serde_json::from_slice(&output).unwrap();
        let input_items = output_val["input"].as_array().unwrap();
        let function_call_id = input_items[0]["call_id"].as_str().unwrap();
        let function_output_id = input_items[1]["call_id"].as_str().unwrap();
        let function_item_id = input_items[0]["id"].as_str().unwrap();
        assert_eq!(function_call_id, function_output_id);
        assert!(function_call_id.len() <= 64);
        assert!(function_item_id.starts_with("fc_"));
        assert!(function_item_id.len() <= 64);
        assert!(input_items[0]["name"].as_str().unwrap().len() <= 64);
        assert_eq!(output_val["tool_choice"], "required");
        assert_eq!(output_val["tools"][0]["name"], input_items[0]["name"]);
        assert_eq!(output_val["tools"][0]["strict"], false);
        assert!(output_val["tools"][0]["parameters"]
            .get("$schema")
            .is_none());
    }

    #[test]
    fn test_claude_to_responses_request_honors_disable_parallel_tool_use() {
        let input = json!({
            "model": "gpt-5.6-luna",
            "messages": [{"role": "user", "content": "hello"}],
            "tool_choice": {"type": "auto", "disable_parallel_tool_use": true}
        });
        let output = claude_to_responses_request(
            "gpt-5.6-luna",
            &serde_json::to_vec(&input).unwrap(),
            false,
        );
        let output_val: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(output_val["parallel_tool_calls"], false);
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
                    "text": "Let me think...",
                    "encrypted_content": "gAAAAcodex-signature"
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
        assert_eq!(content[0]["signature"], "gAAAAcodex-signature");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "The answer is 42.");
    }

    #[test]
    fn test_responses_to_claude_non_stream_function_call() {
        let resp = json!({
            "id": "resp_tool",
            "status": "completed",
            "output": [{
                "type": "function_call",
                "id": "fc_123",
                "call_id": "call_123",
                "name": "lookup",
                "arguments": "{\"query\":\"rust\"}"
            }],
            "model": "gpt-5.6-luna"
        });
        let upstream_bytes = serde_json::to_vec(&resp).unwrap();
        let mut state = ();
        let output =
            responses_to_claude_non_stream("gpt-5.6-luna", &[], &[], &upstream_bytes, &mut state);
        let output_val: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(output_val["stop_reason"], "tool_use");
        assert_eq!(output_val["content"][0]["type"], "tool_use");
        assert_eq!(output_val["content"][0]["id"], "call_123");
        assert_eq!(output_val["content"][0]["name"], "lookup");
        assert_eq!(output_val["content"][0]["input"]["query"], "rust");
    }

    #[test]
    fn test_responses_to_claude_stream_uses_response_event_type_from_data() {
        let mut state: Box<dyn std::any::Any + Send> =
            Box::new(ResponsesToClaudeStreamState::default());

        let created = responses_to_claude_stream(
            "gpt-5.6-luna",
            &[],
            &[],
            b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_stream\"}}\n\n",
            state.as_mut(),
        );
        let delta = responses_to_claude_stream(
            "gpt-5.6-luna",
            &[],
            &[],
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
            state.as_mut(),
        );
        let completed = responses_to_claude_stream(
            "gpt-5.6-luna",
            &[],
            &[],
            b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_stream\",\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n",
            state.as_mut(),
        );

        let created_bytes = created.concat();
        let delta_bytes = delta.concat();
        let completed_bytes = completed.concat();
        let created_text = String::from_utf8_lossy(&created_bytes);
        let delta_text = String::from_utf8_lossy(&delta_bytes);
        let completed_text = String::from_utf8_lossy(&completed_bytes);
        assert!(created_text.contains("message_start"));
        assert!(delta_text.contains("content_block_delta"));
        assert!(delta_text.contains("hello"));
        assert!(completed_text.contains("message_delta"));
        assert!(completed_text.contains("message_stop"));
    }

    #[test]
    fn test_responses_to_claude_stream_converts_function_call_arguments() {
        let mut state: Box<dyn std::any::Any + Send> =
            Box::new(ResponsesToClaudeStreamState::default());
        let events = [
            r#"data: {"type":"response.created","response":{"id":"resp_tool_stream"}}

"#,
            r#"data: {"type":"response.output_item.added","item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"lookup"}}

"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"query\":"}

"#,
            r#"data: {"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"\"rust\"}"}

"#,
            r#"data: {"type":"response.function_call_arguments.done","item_id":"fc_1","name":"lookup","arguments":"{\"query\":\"rust\"}"}

"#,
            r#"data: {"type":"response.completed","response":{"id":"resp_tool_stream","usage":{"input_tokens":4,"output_tokens":2}}}

"#,
        ];

        let mut output = Vec::new();
        for event in events {
            output.extend(responses_to_claude_stream(
                "gpt-5.6-luna",
                &[],
                &[],
                event.as_bytes(),
                state.as_mut(),
            ));
        }
        let output = String::from_utf8_lossy(&output.concat()).into_owned();
        assert!(output.contains(r#""type":"tool_use""#));
        assert!(output.contains(r#""id":"call_1""#));
        assert!(output.contains(r#""type":"input_json_delta""#));
        assert!(output.contains(r#""partial_json":"{\"query\":""#));
        assert!(output.contains(r#""stop_reason":"tool_use""#));
        assert!(output.contains("content_block_stop"));
        assert!(output.contains("message_stop"));
    }

    #[test]
    fn test_responses_to_claude_stream_handles_reasoning_signature_and_incomplete() {
        let mut state: Box<dyn std::any::Any + Send> =
            Box::new(ResponsesToClaudeStreamState::default());
        let events = [
            r#"data: {"type":"response.created","response":{"id":"resp_reasoning"}}

"#,
            r#"data: {"type":"response.output_item.added","item":{"type":"reasoning","id":"rs_1","encrypted_content":"gAAAAcodex-signature"}}

"#,
            r#"data: {"type":"response.reasoning_summary_part.added","item_id":"rs_1"}

"#,
            r#"data: {"type":"response.reasoning_summary_text.delta","item_id":"rs_1","delta":"think"}

"#,
            r#"data: {"type":"response.reasoning_summary_part.done","item_id":"rs_1"}

"#,
            r#"data: {"type":"response.content_part.added","part":{"type":"output_text"}}

"#,
            r#"data: {"type":"response.output_text.delta","delta":"answer"}

"#,
            r#"data: {"type":"response.incomplete","response":{"id":"resp_reasoning","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"usage":{"input_tokens":7,"output_tokens":9}}}

"#,
        ];

        let mut output = Vec::new();
        for event in events {
            output.extend(responses_to_claude_stream(
                "gpt-5.6-luna",
                &[],
                &[],
                event.as_bytes(),
                state.as_mut(),
            ));
        }
        let output = String::from_utf8_lossy(&output.concat()).into_owned();
        assert!(output.contains(r#""type":"thinking""#));
        assert!(output.contains(r#""type":"signature_delta""#));
        assert!(output.contains("gAAAAcodex-signature"));
        assert!(output.contains(r#""stop_reason":"max_tokens""#));
        assert!(output.contains("message_stop"));
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
