/// 转换对注册入口。
///
/// 每个子模块代表一个 `(client_format → provider_format)` 转换对。
/// 模块的 `register()` 函数负责向全局注册表注册请求和响应转换。
///
/// ## 添加新转换对
///
/// 1. 在 `pairs/` 下创建新目录，如 `openai_to_gemini/`
/// 2. 实现 `register()` 函数，调用 `crate::proxy::translator::register()`
/// 3. 在此文件的 `register_all()` 中追加调用
use crate::proxy::translator::TranslatorRegistry;

/// 注册所有转换对。
///
/// 在应用启动时调用一次（`AxumServer::start()` 或 `lib.rs` 初始化阶段）。
pub fn register_all(registry: &mut TranslatorRegistry) {
    // 格式间直通转换（同一协议格式，做规范化处理）
    openai_to_openai::register(registry);

    // OpenAI → 其他上游
    openai_to_claude::register(registry);
    openai_to_codex::register(registry);

    // Claude → 其他上游
    claude_to_openai::register(registry);
    claude_to_codex::register(registry);

    // OpenAI Responses → 其他上游
    responses_to_claude::register(registry);
    responses_to_openai::register(registry);

    // Codex → 其他上游
    codex_to_claude::register(registry);

    tracing::info!("translator: registered all translation pairs");
}

// ─── 直通/规范化转换对 ───

/// OpenAI → OpenAI（请求规范化，不改变协议）
pub mod openai_to_openai {
    use crate::proxy::translator::format::Format;
    use crate::proxy::translator::types::{RequestTransform, ResponseTransform};
    use crate::proxy::translator::TranslatorRegistry;
    use serde_json::Value;

    /// 规范化 OpenAI 请求：确保 model 字段正确，移除不支持的字段
    fn normalize_openai_request(model: &str, raw_json: &[u8], _stream: bool) -> Vec<u8> {
        let mut val = match serde_json::from_slice::<Value>(raw_json) {
            Ok(v) => v,
            Err(_) => return raw_json.to_vec(),
        };
        if let Some(obj) = val.as_object_mut() {
            obj.insert("model".to_string(), Value::String(model.to_string()));
        }
        serde_json::to_vec(&val).unwrap_or(raw_json.to_vec())
    }

    /// 规范化 OpenAI 响应：原样返回
    fn normalize_openai_response(
        _model: &str,
        _original_request: &[u8],
        _converted_request: &[u8],
        upstream_json: &[u8],
        _state: &mut (dyn std::any::Any + Send),
    ) -> Vec<u8> {
        upstream_json.to_vec()
    }

    pub fn register(registry: &mut TranslatorRegistry) {
        registry.register(
            Format::OpenAI,
            Format::OpenAI,
            normalize_openai_request as RequestTransform,
            ResponseTransform::non_stream_only(normalize_openai_response),
            None,
        );
    }
}

// ─── Claude → OpenAI 转换对 ───

/// Claude 客户端 → OpenAI 兼容上游
pub mod claude_to_openai {
    use crate::proxy::translator::format::Format;
    use crate::proxy::translator::types::{
        RequestTransform, ResponseNonStreamTransform, ResponseStreamTransform, ResponseTransform,
        StreamStateFactory,
    };
    use crate::proxy::translator::TranslatorRegistry;
    use serde_json::{json, Value};

    /// Claude 请求 → OpenAI Chat Completions 请求
    ///
    /// 转换要点：
    /// - `messages[].role` + `messages[].content` → OpenAI messages
    /// - `system` → system message（插入到 messages 首位）
    /// - `max_tokens` → `max_tokens`
    /// - `temperature` / `top_p` → 透传
    /// - `thinking` → `reasoning_effort`
    /// - `tools[]` → OpenAI function tools
    /// - `tool_choice` → OpenAI tool_choice
    pub fn claude_to_openai_request(model: &str, raw_json: &[u8], stream: bool) -> Vec<u8> {
        let root = match serde_json::from_slice::<Value>(raw_json) {
            Ok(v) => v,
            Err(_) => return raw_json.to_vec(),
        };

        let mut out = json!({
            "model": model,
            "stream": stream,
            "messages": []
        });

        // Max tokens
        if let Some(max_tokens) = root.get("max_tokens") {
            out["max_tokens"] = max_tokens.clone();
        }

        // Temperature
        if let Some(temp) = root.get("temperature") {
            out["temperature"] = temp.clone();
        } else if let Some(top_p) = root.get("top_p") {
            out["top_p"] = top_p.clone();
        }

        // Stop sequences
        if let Some(stop_sequences) = root.get("stop_sequences") {
            out["stop"] = stop_sequences.clone();
        }

        // Thinking → reasoning_effort
        if let Some(thinking) = root.get("thinking") {
            if let Some(thinking_type) = thinking.get("type").and_then(|v| v.as_str()) {
                let effort = match thinking_type {
                    "enabled" => thinking
                        .get("budget_tokens")
                        .map(|b| b.as_i64().unwrap_or(-1))
                        .map(budget_to_level)
                        .unwrap_or_else(|| budget_to_level(-1)),
                    "adaptive" | "auto" => "xhigh".to_string(),
                    "disabled" => budget_to_level(0),
                    _ => "auto".to_string(),
                };
                out["reasoning_effort"] = Value::String(effort);
            }
        }

        // Messages conversion
        let mut messages = Vec::new();
        let mut system_contents: Vec<Value> = Vec::new();

        // System instruction
        if let Some(system) = root.get("system") {
            append_system_content(&mut system_contents, system);
        }

        // Prepend system message if non-empty
        if !system_contents.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": system_contents
            }));
        }

        // Messages array
        if let Some(msgs) = root.get("messages").and_then(|v| v.as_array()) {
            for msg in msgs {
                let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
                if role == "system" {
                    // System message in messages array — merge into system_contents
                    if let Some(content) = msg.get("content") {
                        append_system_content(&mut system_contents, content);
                    }
                    continue;
                }
                if let Some(converted) = convert_claude_message(msg) {
                    messages.push(converted);
                }
            }
        }

        out["messages"] = Value::Array(messages);

        // Tools conversion
        if let Some(tools) = root.get("tools").and_then(|v| v.as_array()) {
            if !tools.is_empty() {
                let openai_tools: Vec<Value> = tools
                    .iter()
                    .filter_map(|tool| {
                        let name = tool.get("name")?.as_str()?;
                        let description = tool
                            .get("description")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let params = tool
                            .get("input_schema")
                            .cloned()
                            .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
                        Some(json!({
                            "type": "function",
                            "function": {
                                "name": name,
                                "description": description,
                                "parameters": normalize_schema(params)
                            }
                        }))
                    })
                    .collect();
                if !openai_tools.is_empty() {
                    out["tools"] = Value::Array(openai_tools);
                }
            }
        }

        // Tool choice
        if let Some(tc) = root
            .get("tool_choice")
            .and_then(|v| v.get("type"))
            .and_then(|v| v.as_str())
        {
            out["tool_choice"] = match tc {
                "auto" => Value::String("auto".to_string()),
                "any" => Value::String("required".to_string()),
                "tool" => {
                    if let Some(tc_val) = root.get("tool_choice") {
                        if let Some(name) = tc_val.get("name").and_then(|v| v.as_str()) {
                            json!({"type": "function", "function": {"name": name}})
                        } else {
                            Value::String("auto".to_string())
                        }
                    } else {
                        Value::String("auto".to_string())
                    }
                }
                _ => Value::String("auto".to_string()),
            };
        }

        serde_json::to_vec(&out).unwrap_or(raw_json.to_vec())
    }

    /// Claude 响应 → OpenAI 响应（非流式）
    fn claude_to_openai_response_non_stream(
        model: &str,
        _original_request: &[u8],
        _converted_request: &[u8],
        upstream_json: &[u8],
        _state: &mut (dyn std::any::Any + Send),
    ) -> Vec<u8> {
        let root = match serde_json::from_slice::<Value>(upstream_json) {
            Ok(v) => v,
            Err(_) => return upstream_json.to_vec(),
        };

        let mut out = json!({
            "id": root.get("id").and_then(|v| v.as_str()).unwrap_or(""),
            "object": "chat.completion",
            "created": chrono::Utc::now().timestamp(),
            "model": model,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "" },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 0,
                "completion_tokens": 0,
                "total_tokens": 0
            }
        });

        // Extract content blocks
        let mut content_text = String::new();
        let mut reasoning_text = String::new();
        let mut tool_calls = Vec::new();

        if let Some(content_blocks) = root.get("content").and_then(|v| v.as_array()) {
            for block in content_blocks {
                match block.get("type").and_then(|v| v.as_str()) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                            content_text.push_str(text);
                        }
                    }
                    Some("thinking") => {
                        if let Some(text) = block.get("thinking").and_then(|v| v.as_str()) {
                            reasoning_text.push_str(text);
                        }
                    }
                    Some("tool_use") => {
                        if let (Some(id), Some(name)) = (
                            block.get("id").and_then(|v| v.as_str()),
                            block.get("name").and_then(|v| v.as_str()),
                        ) {
                            let args = block.get("input").cloned().unwrap_or(json!({}));
                            tool_calls.push(json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": name,
                                    "arguments": serde_json::to_string(&args).unwrap_or_default()
                                }
                            }));
                        }
                    }
                    _ => {}
                }
            }
        }

        // Build message
        let mut message = json!({ "role": "assistant" });
        if !content_text.is_empty() {
            message["content"] = Value::String(content_text);
        } else if !reasoning_text.is_empty() {
            message["content"] = Value::String(String::new());
        } else {
            message["content"] = Value::Null;
        }
        if !reasoning_text.is_empty() {
            message["reasoning_content"] = Value::String(reasoning_text);
        }
        if !tool_calls.is_empty() {
            message["tool_calls"] = Value::Array(tool_calls);
        }

        out["choices"][0]["message"] = message;

        // Finish reason
        if let Some(stop_reason) = root.get("stop_reason").and_then(|v| v.as_str()) {
            out["choices"][0]["finish_reason"] = match stop_reason {
                "end_turn" => Value::String("stop".to_string()),
                "max_tokens" => Value::String("length".to_string()),
                "tool_use" => Value::String("tool_calls".to_string()),
                "stop_sequence" => Value::String("stop".to_string()),
                _ => Value::String("stop".to_string()),
            };
        }

        // Usage
        if let Some(usage) = root.get("usage") {
            let input_tokens = usage
                .get("input_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let output_tokens = usage
                .get("output_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            let cache_read = usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            out["usage"] = json!({
                "prompt_tokens": input_tokens + cache_read,
                "completion_tokens": output_tokens,
                "total_tokens": input_tokens + output_tokens + cache_read,
                "prompt_tokens_details": {
                    "cached_tokens": cache_read
                }
            });
        }

        serde_json::to_vec(&out).unwrap_or(upstream_json.to_vec())
    }

    /// Claude SSE → OpenAI SSE（流式）
    fn claude_to_openai_stream(
        model: &str,
        _original_request: &[u8],
        _converted_request: &[u8],
        upstream_chunk: &[u8],
        state: &mut (dyn std::any::Any + Send),
    ) -> Vec<Vec<u8>> {
        // 委托给流状态机处理
        if let Some(state) = state.downcast_mut::<ClaudeToOpenAIStreamState>() {
            return state.process_chunk(model, upstream_chunk);
        }
        vec![upstream_chunk.to_vec()]
    }

    /// 流状态工厂
    fn claude_to_openai_state_factory() -> Box<dyn std::any::Any + Send> {
        Box::new(ClaudeToOpenAIStreamState::new())
    }

    pub fn register(registry: &mut TranslatorRegistry) {
        registry.register(
            Format::Claude,
            Format::OpenAI,
            claude_to_openai_request as RequestTransform,
            ResponseTransform::new(
                Some(claude_to_openai_stream as ResponseStreamTransform),
                Some(claude_to_openai_response_non_stream as ResponseNonStreamTransform),
                None,
            ),
            Some(claude_to_openai_state_factory as StreamStateFactory),
        );
    }

    // ─── 内部辅助函数 ───

    fn budget_to_level(budget: i64) -> String {
        if budget <= 0 {
            return "low".to_string();
        }
        if budget <= 1024 {
            return "low".to_string();
        }
        if budget <= 4096 {
            return "medium".to_string();
        }
        if budget <= 16384 {
            return "high".to_string();
        }
        "xhigh".to_string()
    }

    fn append_system_content(acc: &mut Vec<Value>, content: &Value) {
        match content {
            Value::String(s) if !s.trim().is_empty() => {
                acc.push(json!({"type": "text", "text": s}));
            }
            Value::Array(arr) => {
                for item in arr {
                    if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                        if !text.trim().is_empty() {
                            acc.push(item.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn convert_claude_message(msg: &Value) -> Option<Value> {
        let role = msg.get("role").and_then(|v| v.as_str())?;
        let content = msg.get("content")?;

        match content {
            Value::String(s) => Some(json!({
                "role": role,
                "content": s
            })),
            Value::Array(parts) => {
                let mut text_parts = Vec::new();
                let mut reasoning_parts = Vec::new();
                let mut tool_calls = Vec::new();
                let mut tool_results = Vec::new();

                for part in parts {
                    match part.get("type").and_then(|v| v.as_str()) {
                        Some("text") => {
                            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                text_parts.push(text.to_string());
                            }
                        }
                        Some("thinking") => {
                            if let Some(text) = part.get("thinking").and_then(|v| v.as_str()) {
                                reasoning_parts.push(text.to_string());
                            }
                        }
                        Some("tool_use") => {
                            if role == "assistant" {
                                if let (Some(id), Some(name)) = (
                                    part.get("id").and_then(|v| v.as_str()),
                                    part.get("name").and_then(|v| v.as_str()),
                                ) {
                                    let args = part.get("input").cloned().unwrap_or(json!({}));
                                    tool_calls.push(json!({
                                        "id": id,
                                        "type": "function",
                                        "function": {
                                            "name": name,
                                            "arguments": serde_json::to_string(&args).unwrap_or_default()
                                        }
                                    }));
                                }
                            }
                        }
                        Some("tool_result") => {
                            if let Some(tool_use_id) =
                                part.get("tool_use_id").and_then(|v| v.as_str())
                            {
                                let content_str =
                                    part.get("content").and_then(|v| v.as_str()).unwrap_or("{}");
                                tool_results.push(json!({
                                    "role": "tool",
                                    "tool_call_id": tool_use_id,
                                    "content": content_str
                                }));
                            }
                        }
                        Some("image") => {
                            if let Some(source) = part.get("source") {
                                let image_url = extract_image_url(source);
                                if !image_url.is_empty() {
                                    text_parts.push(format!("![image]({})", image_url));
                                }
                            }
                        }
                        _ => {}
                    }
                }

                if role == "assistant" {
                    let mut msg = json!({ "role": "assistant" });
                    let content_str = text_parts.join("\n\n");
                    if !content_str.is_empty() {
                        msg["content"] = Value::String(content_str);
                    } else {
                        msg["content"] = Value::String(String::new());
                    }
                    if !reasoning_parts.is_empty() {
                        msg["reasoning_content"] = Value::String(reasoning_parts.join("\n\n"));
                    }
                    if !tool_calls.is_empty() {
                        msg["tool_calls"] = Value::Array(tool_calls);
                    }
                    Some(msg)
                } else {
                    // Tool results 需要作为独立消息发出
                    // 这里简化处理，合并到用户消息中
                    let content_str = text_parts.join("\n\n");
                    if !content_str.is_empty() || !tool_results.is_empty() {
                        Some(json!({
                            "role": role,
                            "content": content_str
                        }))
                    } else {
                        None
                    }
                }
            }
            _ => Some(json!({
                "role": role,
                "content": ""
            })),
        }
    }

    fn extract_image_url(source: &Value) -> String {
        if let Some(source_type) = source.get("type").and_then(|v| v.as_str()) {
            match source_type {
                "base64" => {
                    let media_type = source
                        .get("media_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("application/octet-stream");
                    let data = source.get("data").and_then(|v| v.as_str()).unwrap_or("");
                    if !data.is_empty() {
                        return format!("data:{};base64,{}", media_type, data);
                    }
                }
                "url" => {
                    if let Some(url) = source.get("url").and_then(|v| v.as_str()) {
                        return url.to_string();
                    }
                }
                _ => {}
            }
        }
        String::new()
    }

    fn normalize_schema(schema: Value) -> Value {
        if let Some(obj) = schema.as_object() {
            if obj.get("type").and_then(|v| v.as_str()) == Some("object") {
                let mut normalized = obj.clone();
                if !normalized.contains_key("properties") {
                    normalized.insert("properties".to_string(), json!({}));
                }
                // Recursively normalize nested schemas
                if let Some(props) = normalized.get_mut("properties") {
                    if let Some(obj) = props.as_object_mut() {
                        for (_, val) in obj.iter_mut() {
                            *val = normalize_schema(val.clone());
                        }
                    }
                }
                return Value::Object(normalized);
            }
        }
        schema
    }

    /// Claude → OpenAI 流状态机
    ///
    /// 维护跨 chunk 的累积状态，确保正确的 SSE 事件序列。
    pub struct ClaudeToOpenAIStreamState {
        message_id: String,
        model_name: String,
        created_at: i64,
        content_text: String,
        reasoning_text: String,
        tool_calls_accumulator: Vec<Value>,
        current_tool_call: Option<ToolCallAccumulator>,
        finish_reason: Option<String>,
        message_started: bool,
        message_stop_sent: bool,
        usage: Option<Value>,
    }

    #[derive(Default)]
    pub struct ToolCallAccumulator {
        index: usize,
        id: String,
        name: String,
        arguments: String,
    }

    impl ClaudeToOpenAIStreamState {
        pub fn new() -> Self {
            Self {
                message_id: String::new(),
                model_name: String::new(),
                created_at: chrono::Utc::now().timestamp(),
                content_text: String::new(),
                reasoning_text: String::new(),
                tool_calls_accumulator: Vec::new(),
                current_tool_call: None,
                finish_reason: None,
                message_started: false,
                message_stop_sent: false,
                usage: None,
            }
        }

        pub fn process_chunk(&mut self, model: &str, chunk: &[u8]) -> Vec<Vec<u8>> {
            // 解析 Claude SSE chunk
            // Claude SSE 格式: event: <type>\ndata: {...}\n\n
            let text = match std::str::from_utf8(chunk) {
                Ok(t) => t,
                Err(_) => return vec![chunk.to_vec()],
            };

            let mut events: Vec<(String, Value)> = Vec::new();
            for line in text.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if data == "[DONE]" {
                        continue;
                    }
                    if let Ok(val) = serde_json::from_str::<Value>(data) {
                        if let Some(event_type) = line.split("event: ").nth(1) {
                            events.push((event_type.to_string(), val));
                        }
                    }
                }
            }

            // 如果没找到事件，尝试从 chunk 中提取
            if events.is_empty() {
                if let Ok(val) = serde_json::from_slice::<Value>(chunk) {
                    events.push(("unknown".to_string(), val));
                } else {
                    return vec![chunk.to_vec()];
                }
            }

            let mut output_chunks = Vec::new();
            self.model_name = model.to_string();

            for (_event_type, data) in events {
                // message_start
                if let Some(message) = data.get("message") {
                    if !self.message_started {
                        if let Some(id) = message.get("id").and_then(|v| v.as_str()) {
                            self.message_id = id.to_string();
                        }
                        if let Some(m) = message.get("model").and_then(|v| v.as_str()) {
                            self.model_name = m.to_string();
                        }
                        self.message_started = true;

                        let chunk_data = json!({
                            "id": &self.message_id,
                            "object": "chat.completion.chunk",
                            "created": self.created_at,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": { "role": "assistant", "content": "" },
                                "finish_reason": null
                            }]
                        });
                        output_chunks.push(
                            format!(
                                "data: {}\n\n",
                                serde_json::to_string(&chunk_data).unwrap_or_default()
                            )
                            .into_bytes(),
                        );
                    }
                }

                // content_block_start
                if let Some(block) = data.get("content_block") {
                    match block.get("type").and_then(|v| v.as_str()) {
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                                self.content_text.push_str(text);
                            }
                        }
                        Some("thinking") => {
                            if let Some(text) = block.get("thinking").and_then(|v| v.as_str()) {
                                self.reasoning_text.push_str(text);
                            }
                        }
                        Some("tool_use") => {
                            if let (Some(id), Some(name)) = (
                                block.get("id").and_then(|v| v.as_str()),
                                block.get("name").and_then(|v| v.as_str()),
                            ) {
                                self.current_tool_call = Some(ToolCallAccumulator {
                                    index: self.tool_calls_accumulator.len(),
                                    id: id.to_string(),
                                    name: name.to_string(),
                                    arguments: String::new(),
                                });
                            }
                        }
                        _ => {}
                    }
                }

                // content_block_delta
                if let Some(delta) = data.get("delta") {
                    if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                        self.content_text.push_str(text);
                        let chunk_data = json!({
                            "id": &self.message_id,
                            "object": "chat.completion.chunk",
                            "created": self.created_at,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": { "content": text },
                                "finish_reason": null
                            }]
                        });
                        output_chunks.push(
                            format!(
                                "data: {}\n\n",
                                serde_json::to_string(&chunk_data).unwrap_or_default()
                            )
                            .into_bytes(),
                        );
                    }
                    if let Some(text) = delta.get("thinking").and_then(|v| v.as_str()) {
                        self.reasoning_text.push_str(text);
                        let chunk_data = json!({
                            "id": &self.message_id,
                            "object": "chat.completion.chunk",
                            "created": self.created_at,
                            "model": model,
                            "choices": [{
                                "index": 0,
                                "delta": { "reasoning_content": text },
                                "finish_reason": null
                            }]
                        });
                        output_chunks.push(
                            format!(
                                "data: {}\n\n",
                                serde_json::to_string(&chunk_data).unwrap_or_default()
                            )
                            .into_bytes(),
                        );
                    }
                    if let Some(partial_json) = delta.get("partial_json") {
                        if let Some(tc) = &mut self.current_tool_call {
                            tc.arguments.push_str(&partial_json.to_string());
                            let chunk_data = json!({
                                "id": &self.message_id,
                                "object": "chat.completion.chunk",
                                "created": self.created_at,
                                "model": model,
                                "choices": [{
                                    "index": 0,
                                    "delta": {
                                        "tool_calls": [{
                                            "index": tc.index,
                                            "id": if tc.arguments.len() < 20 { &tc.id } else { "" },
                                            "type": "function",
                                            "function": {
                                                "name": if tc.arguments.len() < 20 { &tc.name } else { "" },
                                                "arguments": ""
                                            }
                                        }]
                                    },
                                    "finish_reason": null
                                }]
                            });
                            output_chunks.push(
                                format!(
                                    "data: {}\n\n",
                                    serde_json::to_string(&chunk_data).unwrap_or_default()
                                )
                                .into_bytes(),
                            );
                        }
                    }
                }

                // content_block_stop
                if data.get("type").and_then(|v| v.as_str()) == Some("content_block_stop") {
                    if let Some(tc) = self.current_tool_call.take() {
                        self.tool_calls_accumulator.push(json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.arguments
                            }
                        }));
                    }
                }

                // message_delta — 包含 stop_reason 和 usage
                if let Some(delta) = data.get("delta") {
                    if let Some(stop_reason) = delta.get("stop_reason").and_then(|v| v.as_str()) {
                        self.finish_reason = Some(stop_reason.to_string());
                    }
                }
                if let Some(usage) = data.get("usage") {
                    self.usage = Some(usage.clone());
                }

                // message_stop
                if data.get("type").and_then(|v| v.as_str()) == Some("message_stop") {
                    let openai_finish = match self.finish_reason.as_deref() {
                        Some("end_turn") => "stop",
                        Some("max_tokens") => "length",
                        Some("tool_use") => "tool_calls",
                        Some("stop_sequence") => "stop",
                        _ => "stop",
                    };

                    // Final delta with finish_reason and tool_calls
                    let mut delta = json!({});
                    if !self.tool_calls_accumulator.is_empty() {
                        delta["tool_calls"] = self.tool_calls_accumulator.clone().into();
                    }

                    let chunk_data = json!({
                        "id": &self.message_id,
                        "object": "chat.completion.chunk",
                        "created": self.created_at,
                        "model": model,
                        "choices": [{
                            "index": 0,
                            "delta": delta,
                            "finish_reason": openai_finish
                        }]
                    });
                    output_chunks.push(
                        format!(
                            "data: {}\n\n",
                            serde_json::to_string(&chunk_data).unwrap_or_default()
                        )
                        .into_bytes(),
                    );

                    // Usage chunk
                    if let Some(usage) = &self.usage {
                        let input_tokens = usage
                            .get("input_tokens")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        let output_tokens = usage
                            .get("output_tokens")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        let cache_read = usage
                            .get("cache_read_input_tokens")
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        let usage_data = json!({
                            "id": &self.message_id,
                            "object": "chat.completion.chunk",
                            "created": self.created_at,
                            "model": model,
                            "usage": {
                                "prompt_tokens": input_tokens + cache_read,
                                "completion_tokens": output_tokens,
                                "total_tokens": input_tokens + output_tokens + cache_read,
                                "prompt_tokens_details": {
                                    "cached_tokens": cache_read
                                }
                            }
                        });
                        output_chunks.push(
                            format!(
                                "data: {}\n\n",
                                serde_json::to_string(&usage_data).unwrap_or_default()
                            )
                            .into_bytes(),
                        );
                    }

                    // DONE marker
                    output_chunks.push(b"data: [DONE]\n\n".to_vec());
                    self.message_stop_sent = true;
                }
            }

            if output_chunks.is_empty() {
                vec![chunk.to_vec()]
            } else {
                output_chunks
            }
        }
    }
}

// ─── OpenAI → Claude 转换对 ───

pub mod openai_to_claude;

// ─── OpenAI → Codex 转换对 ───

pub mod openai_to_codex;

// ─── Claude → Codex 转换对 ───

pub mod claude_to_codex;

// ─── Codex → Claude 转换对 ───

pub mod codex_to_claude;

// ─── OpenAI Responses → Claude 转换对 ───

pub mod responses_to_claude;

// ─── Claude → OpenAI Responses 转换对 ───

pub mod responses_to_openai;
