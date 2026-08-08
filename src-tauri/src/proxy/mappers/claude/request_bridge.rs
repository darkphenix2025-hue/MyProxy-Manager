use crate::proxy::mappers::claude::models::{ClaudeRequest, ContentBlock, MessageContent};
use serde_json::{json, Value};

/// Convert Claude Messages API request to OpenAI Chat Completions format.
///
/// This is the inverse of `to_claude_body()` in the OpenAI mapper.
pub fn claude_to_openai_body(request: &ClaudeRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();

    // Extract system prompt into a system message
    if let Some(ref system) = request.system {
        let system_text = match system {
            crate::proxy::mappers::claude::models::SystemPrompt::String(s) => s.clone(),
            crate::proxy::mappers::claude::models::SystemPrompt::Array(blocks) => blocks
                .iter()
                .map(|b| b.text.clone())
                .collect::<Vec<_>>()
                .join("\n"),
        };
        if !system_text.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": system_text,
            }));
        }
    }

    // Convert Claude messages to OpenAI messages
    for msg in &request.messages {
        let openai_role = match msg.role.as_str() {
            "user" => "user",
            "assistant" => "assistant",
            _ => "user",
        };

        match &msg.content {
            MessageContent::String(s) => {
                messages.push(json!({
                    "role": openai_role,
                    "content": s,
                }));
            }
            MessageContent::Array(blocks) => {
                // Check if all blocks are text and can be joined into a single string
                let all_text: Option<String> =
                    blocks.iter().try_fold(String::new(), |mut acc, b| {
                        if let ContentBlock::Text { text } = b {
                            acc.push_str(text);
                            Some(acc)
                        } else {
                            None
                        }
                    });

                if let Some(text) = all_text {
                    messages.push(json!({
                        "role": openai_role,
                        "content": text,
                    }));
                } else {
                    // Multi-modal: convert to OpenAI content blocks
                    let content_blocks: Vec<Value> = blocks
                        .iter()
                        .filter_map(|block| content_block_to_openai(block))
                        .collect();

                    if content_blocks.is_empty() {
                        messages.push(json!({
                            "role": openai_role,
                            "content": "",
                        }));
                    } else if content_blocks.len() == 1
                        && content_blocks[0].get("type").and_then(|v| v.as_str()) == Some("text")
                    {
                        // Single text block: use simple string content
                        messages.push(json!({
                            "role": openai_role,
                            "content": content_blocks[0].get("text").and_then(|v| v.as_str()).unwrap_or(""),
                        }));
                    } else {
                        messages.push(json!({
                            "role": openai_role,
                            "content": content_blocks,
                        }));
                    }
                }
            }
        }
    }

    let mut body = json!({
        "model": &request.model,
        "messages": messages,
        "stream": request.stream,
    });

    if let Some(mt) = request.max_tokens {
        body["max_tokens"] = Value::Number(mt.into());
    }
    if let Some(t) = request.temperature {
        if let Some(n) = serde_json::Number::from_f64(t) {
            body["temperature"] = n.into();
        }
    }
    if let Some(t) = request.top_p {
        if let Some(n) = serde_json::Number::from_f64(t) {
            body["top_p"] = n.into();
        }
    }

    // Convert Claude tools to OpenAI tools format
    if let Some(ref tools) = request.tools {
        let openai_tools: Vec<Value> = tools
            .iter()
            .filter(|t| !t.is_web_search())
            .filter_map(|t| {
                if let Some(ref schema) = t.input_schema {
                    Some(json!({
                        "type": "function",
                        "function": {
                            "name": t.get_name(),
                            "description": t.description.as_deref().unwrap_or(""),
                            "parameters": schema,
                        }
                    }))
                } else {
                    None
                }
            })
            .collect();
        if !openai_tools.is_empty() {
            body["tools"] = Value::Array(openai_tools);
        }
    }

    // Convert Claude thinking to OpenAI thinking format
    if let Some(ref thinking) = request.thinking {
        body["thinking"] = json!({
            "type": thinking.type_.clone(),
            "budget_tokens": thinking.budget_tokens,
            "effort": thinking.effort,
        });
    }

    body
}

/// Convert a single Claude ContentBlock to an OpenAI content block Value.
/// Returns None for block types that have no OpenAI equivalent.
fn content_block_to_openai(block: &ContentBlock) -> Option<Value> {
    match block {
        ContentBlock::Text { text } => Some(json!({
            "type": "text",
            "text": text,
        })),
        ContentBlock::Image { source, .. } => {
            // Convert base64 image to data URL
            let data_url = format!("data:{};base64,{}", source.media_type, source.data);
            Some(json!({
                "type": "image_url",
                "image_url": {
                    "url": data_url,
                }
            }))
        }
        ContentBlock::Thinking { thinking, .. } => Some(json!({
            "type": "text",
            "text": format!("<thinking>{}</thinking>", thinking),
        })),
        ContentBlock::ToolUse {
            id, name, input, ..
        } => Some(json!({
            "type": "tool_use",
            "id": id,
            "name": name,
            "input": input,
        })),
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
        } => {
            let text = match content {
                serde_json::Value::String(s) => s.clone(),
                other => serde_json::to_string(other).unwrap_or_default(),
            };
            let mut block = json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id,
                "content": text,
            });
            if let Some(true) = is_error {
                block["is_error"] = Value::Bool(true);
            }
            Some(block)
        }
        // These types don't have direct OpenAI equivalents
        ContentBlock::Document { .. }
        | ContentBlock::RedactedThinking { .. }
        | ContentBlock::ServerToolUse { .. }
        | ContentBlock::WebSearchToolResult { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::mappers::claude::models::{Message, MessageContent};

    #[test]
    fn test_basic_text_conversion() {
        let claude_req = ClaudeRequest {
            model: "claude-sonnet-4-20250514".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::String("Hello".to_string()),
            }],
            system: None,
            tools: None,
            stream: false,
            max_tokens: Some(1024),
            temperature: Some(0.7),
            top_p: None,
            top_k: None,
            thinking: None,
            metadata: None,
            output_config: None,
            size: None,
            quality: None,
        };

        let openai_body = claude_to_openai_body(&claude_req);

        assert_eq!(openai_body["model"], "claude-sonnet-4-20250514");
        assert_eq!(openai_body["stream"], false);
        assert_eq!(openai_body["max_tokens"], 1024);
        let msgs = openai_body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[0]["content"], "Hello");
    }

    #[test]
    fn test_system_prompt_conversion() {
        let claude_req = ClaudeRequest {
            model: "claude-sonnet-4-20250514".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: MessageContent::String("Hi".to_string()),
            }],
            system: Some(crate::proxy::mappers::claude::models::SystemPrompt::String(
                "You are a helpful assistant.".to_string(),
            )),
            tools: None,
            stream: false,
            max_tokens: None,
            temperature: None,
            top_p: None,
            top_k: None,
            thinking: None,
            metadata: None,
            output_config: None,
            size: None,
            quality: None,
        };

        let openai_body = claude_to_openai_body(&claude_req);
        let msgs = openai_body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "You are a helpful assistant.");
        assert_eq!(msgs[1]["role"], "user");
    }
}
