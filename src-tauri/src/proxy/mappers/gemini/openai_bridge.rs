use serde_json::{json, Value};

/// Convert a Gemini native request body to OpenAI Chat Completions format.
pub fn gemini_to_openai_body(body: &Value, model: &str) -> Value {
    let mut messages: Vec<Value> = Vec::new();

    // Extract system instruction as system message
    if let Some(sys) = body.get("systemInstruction") {
        if let Some(parts) = sys.get("parts").and_then(|p| p.as_array()) {
            let texts: Vec<String> = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(|t| t.as_str()).map(String::from))
                .collect();
            if !texts.is_empty() {
                messages.push(json!({
                    "role": "system",
                    "content": texts.join("\n"),
                }));
            }
        }
    }

    // Convert contents to OpenAI messages
    if let Some(contents) = body.get("contents").and_then(|c| c.as_array()) {
        for content in contents {
            let role = content
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("user");
            let openai_role = match role {
                "user" => "user",
                "model" => "assistant",
                _ => "user",
            };

            if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
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
                    "role": openai_role,
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

    // Map generationConfig
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

/// Convert an OpenAI Chat Completions response to Gemini native format.
pub fn openai_to_gemini_response(openai_response: &Value) -> Value {
    let mut text_parts: Vec<Value> = Vec::new();
    let mut model_version = "unknown".to_string();

    if let Some(choices) = openai_response.get("choices").and_then(|c| c.as_array()) {
        for choice in choices {
            if let Some(message) = choice.get("message") {
                if let Some(text) = message.get("content").and_then(|t| t.as_str()) {
                    if !text.is_empty() {
                        text_parts.push(json!({ "text": text }));
                    }
                }
                if let Some(reasoning) = message.get("reasoning_content").and_then(|t| t.as_str()) {
                    if !reasoning.is_empty() {
                        text_parts.push(json!({
                            "text": reasoning,
                            "thought": true,
                        }));
                    }
                }
            }
        }
    }

    if let Some(m) = openai_response.get("model").and_then(|v| v.as_str()) {
        model_version = m.to_string();
    }

    json!({
        "candidates": [{
            "content": {
                "role": "model",
                "parts": text_parts,
            },
            "finishReason": "STOP",
        }],
        "modelVersion": model_version,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_to_openai_basic() {
        let gemini = json!({
            "contents": [{
                "role": "user",
                "parts": [{"text": "Hello"}]
            }],
            "systemInstruction": {
                "parts": [{"text": "Be helpful"}]
            }
        });

        let openai = gemini_to_openai_body(&gemini, "gpt-4");
        assert_eq!(openai["model"], "gpt-4");
        assert_eq!(openai["messages"][0]["role"], "system");
        assert_eq!(openai["messages"][0]["content"], "Be helpful");
        assert_eq!(openai["messages"][1]["role"], "user");
        assert_eq!(openai["messages"][1]["content"], "Hello");
    }

    #[test]
    fn test_openai_to_gemini_basic() {
        let openai = json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {"role": "assistant", "content": "Hi there"},
                "finish_reason": "stop",
            }]
        });

        let gemini = openai_to_gemini_response(&openai);
        assert!(gemini.get("candidates").is_some());
        let candidates = gemini["candidates"].as_array().unwrap();
        assert_eq!(candidates[0]["content"]["parts"][0]["text"], "Hi there");
        assert_eq!(gemini["modelVersion"], "gpt-4");
    }
}
