use serde_json::{json, Value};

/// Convert a Gemini native request body to Claude Messages API format.
/// This is a basic conversion — complex features (tool calling, system instructions)
/// are preserved as-is where possible.
pub fn gemini_to_claude_body(body: &Value, model: &str) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    let mut system_text = String::new();

    // Extract system instruction
    if let Some(sys) = body.get("systemInstruction") {
        if let Some(parts) = sys.get("parts").and_then(|p| p.as_array()) {
            for part in parts {
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    if !system_text.is_empty() {
                        system_text.push_str("\n");
                    }
                    system_text.push_str(text);
                }
            }
        }
    }

    // Convert contents to messages
    if let Some(contents) = body.get("contents").and_then(|c| c.as_array()) {
        for content in contents {
            let role = content
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("user");
            let claude_role = match role {
                "user" => "user",
                "model" => "assistant",
                _ => "user",
            };

            if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                // Collect text parts
                let texts: Vec<String> = parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(|t| t.as_str()).map(String::from))
                    .collect();

                let content_value = if texts.len() == 1 {
                    Value::String(texts[0].clone())
                } else if !texts.is_empty() {
                    Value::String(texts.join("\n"))
                } else {
                    Value::String(String::new())
                };

                messages.push(json!({
                    "role": claude_role,
                    "content": content_value,
                }));
            }
        }
    }

    let mut result = json!({
        "model": model,
        "messages": messages,
        "stream": false,
    });

    if !system_text.is_empty() {
        result["system"] = Value::String(system_text);
    }

    // Map generationConfig to Claude parameters
    if let Some(config) = body.get("generationConfig") {
        if let Some(max_tokens) = config.get("maxOutputTokens").and_then(|v| v.as_u64()) {
            result["max_tokens"] = Value::Number(max_tokens.into());
        }
        if let Some(temp) = config.get("temperature").and_then(|v| v.as_f64()) {
            if let Some(n) = serde_json::Number::from_f64(temp) {
                result["temperature"] = n.into();
            }
        }
        if let Some(top_p) = config.get("topP").and_then(|v| v.as_f64()) {
            if let Some(n) = serde_json::Number::from_f64(top_p) {
                result["top_p"] = n.into();
            }
        }
    }

    result
}

/// Convert a Claude Messages API response to Gemini native format.
pub fn claude_to_gemini_response(claude_response: &Value) -> Value {
    let mut text_parts: Vec<Value> = Vec::new();

    if let Some(content) = claude_response.get("content").and_then(|c| c.as_array()) {
        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(json!({ "text": text }));
                    }
                }
                Some("thinking") => {
                    if let Some(thinking) = block.get("thinking").and_then(|t| t.as_str()) {
                        text_parts.push(json!({
                            "text": thinking,
                            "thought": true,
                        }));
                    }
                }
                _ => {}
            }
        }
    }

    let model = claude_response
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("unknown")
        .to_string();

    json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": text_parts,
            },
            "finishReason": "STOP",
        }],
        "modelVersion": model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_to_claude_basic() {
        let gemini = json!({
            "contents": [{
                "role": "user",
                "parts": [{"text": "Hello"}]
            }],
            "generationConfig": {
                "temperature": 0.7,
                "maxOutputTokens": 1024,
            }
        });

        let claude = gemini_to_claude_body(&gemini, "claude-sonnet-4-20250514");
        assert_eq!(claude["model"], "claude-sonnet-4-20250514");
        assert_eq!(claude["messages"][0]["role"], "user");
        assert_eq!(claude["messages"][0]["content"], "Hello");
        assert_eq!(claude["max_tokens"], 1024);
    }

    #[test]
    fn test_claude_to_gemini_basic() {
        let claude = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [{"type": "text", "text": "Hi there"}],
            "model": "claude-sonnet-4",
            "stop_reason": "end_turn",
            "stop_sequence": null,
            "usage": {"input_tokens": 10, "output_tokens": 5},
        });

        let gemini = claude_to_gemini_response(&claude);
        assert!(gemini.get("candidates").is_some());
        let candidates = gemini["candidates"].as_array().unwrap();
        assert_eq!(candidates[0]["content"]["parts"][0]["text"], "Hi there");
    }
}
