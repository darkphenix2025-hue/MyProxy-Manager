use bytes::Bytes;
use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use std::pin::Pin;
use uuid::Uuid;

/// Convert an OpenAI Chat Completions SSE stream to a Claude Messages SSE stream.
pub fn create_claude_sse_from_openai_stream<S, E>(
    mut openai_stream: Pin<Box<S>>,
    model: String,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, String>> + Send>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + ?Sized + 'static,
    E: std::fmt::Display + Send + 'static,
{
    use bytes::BytesMut;

    let uuid_str = Uuid::new_v4().to_string();
    let message_id = format!(
        "msg_{}",
        uuid_str
            .replace('-', "")
            .chars()
            .take(24)
            .collect::<String>()
    );
    let _created_ts = chrono::Utc::now().timestamp() as u64;

    let stream = async_stream::stream! {
        let mut buffer = BytesMut::new();
        let mut content_block_index: u32 = 0;
        let mut emitted_message_start = false;
        let mut final_input_tokens: u32 = 0;
        let mut final_output_tokens: u32 = 0;
        let mut tool_call_index: u32 = 0;

        let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(15));
        heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                item = openai_stream.next() => {
                    match item {
                        Some(Ok(bytes)) => {
                            buffer.extend_from_slice(&bytes);
                            while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                                let line_raw = buffer.split_to(pos + 1);
                                if let Ok(line_str) = std::str::from_utf8(&line_raw) {
                                    let line = line_str.trim();
                                    if line.is_empty() { continue; }
                                    if !line.starts_with("data: ") { continue; }
                                    let data = &line["data: ".len()..];
                                    if data == "[DONE]" {
                                        // End of stream - emit message_delta with stop_reason and usage, then message_stop
                                        let usage_json = json!({
                                            "input_tokens": final_input_tokens,
                                            "output_tokens": final_output_tokens,
                                        });
                                        let delta_event = json!({
                                            "type": "message_delta",
                                            "delta": {
                                                "stop_reason": "end_turn",
                                                "stop_sequence": null,
                                            },
                                            "usage": usage_json,
                                        });
                                        let delta_bytes = Bytes::from(format!(
                                            "event: message_delta\ndata: {}\n\n",
                                            serde_json::to_string(&delta_event).unwrap_or_default()
                                        ));
                                        yield Ok(delta_bytes);

                                        let stop_event = Bytes::from(
                                            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
                                        );
                                        yield Ok(stop_event);
                                        break;
                                    }

                                    if let Ok(event) = serde_json::from_str::<Value>(data) {
                                        // Emit message_start on first meaningful event
                                        if !emitted_message_start && event.get("choices").is_some() {
                                            emitted_message_start = true;
                                            let start_event = json!({
                                                "type": "message_start",
                                                "message": {
                                                    "id": message_id,
                                                    "type": "message",
                                                    "role": "assistant",
                                                    "content": [],
                                                    "model": model,
                                                    "stop_reason": null,
                                                    "stop_sequence": null,
                                                    "usage": {
                                                        "input_tokens": 0,
                                                        "output_tokens": 0,
                                                    }
                                                }
                                            });
                                            let start_bytes = Bytes::from(format!(
                                                "event: message_start\ndata: {}\n\n",
                                                serde_json::to_string(&start_event).unwrap_or_default()
                                            ));
                                            yield Ok(start_bytes);
                                        }

                                        // Process choices
                                        if let Some(choices) = event.get("choices").and_then(|c| c.as_array()) {
                                            for choice in choices {
                                                let _index = choice.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                                                if let Some(delta) = choice.get("delta") {
                                                    // Text content
                                                    if let Some(text) = delta.get("content").and_then(|v| v.as_str()) {
                                                        if !text.is_empty() {
                                                            // Check if we need to start a new content block
                                                            let content_start = json!({
                                                                "type": "content_block_start",
                                                                "index": content_block_index,
                                                                "content_block": {
                                                                    "type": "text",
                                                                    "text": "",
                                                                }
                                                            });
                                                            yield Ok(Bytes::from(format!(
                                                                "event: content_block_start\ndata: {}\n\n",
                                                                serde_json::to_string(&content_start).unwrap_or_default()
                                                            )));

                                                            let content_delta = json!({
                                                                "type": "content_block_delta",
                                                                "index": content_block_index,
                                                                "delta": {
                                                                    "type": "text_delta",
                                                                    "text": text,
                                                                }
                                                            });
                                                            yield Ok(Bytes::from(format!(
                                                                "event: content_block_delta\ndata: {}\n\n",
                                                                serde_json::to_string(&content_delta).unwrap_or_default()
                                                            )));

                                                            let content_stop = json!({
                                                                "type": "content_block_stop",
                                                                "index": content_block_index,
                                                            });
                                                            yield Ok(Bytes::from(format!(
                                                                "event: content_block_stop\ndata: {}\n\n",
                                                                serde_json::to_string(&content_stop).unwrap_or_default()
                                                            )));
                                                            content_block_index += 1;
                                                        }
                                                    }

                                                    // Reasoning / thinking content
                                                    if let Some(reasoning) = delta.get("reasoning_content").and_then(|v| v.as_str()) {
                                                        if !reasoning.is_empty() {
                                                            let content_start = json!({
                                                                "type": "content_block_start",
                                                                "index": content_block_index,
                                                                "content_block": {
                                                                    "type": "thinking",
                                                                    "thinking": "",
                                                                    "signature": null,
                                                                }
                                                            });
                                                            yield Ok(Bytes::from(format!(
                                                                "event: content_block_start\ndata: {}\n\n",
                                                                serde_json::to_string(&content_start).unwrap_or_default()
                                                            )));

                                                            let content_delta = json!({
                                                                "type": "content_block_delta",
                                                                "index": content_block_index,
                                                                "delta": {
                                                                    "type": "thinking_delta",
                                                                    "thinking": reasoning,
                                                                }
                                                            });
                                                            yield Ok(Bytes::from(format!(
                                                                "event: content_block_delta\ndata: {}\n\n",
                                                                serde_json::to_string(&content_delta).unwrap_or_default()
                                                            )));

                                                            let content_stop = json!({
                                                                "type": "content_block_stop",
                                                                "index": content_block_index,
                                                            });
                                                            yield Ok(Bytes::from(format!(
                                                                "event: content_block_stop\ndata: {}\n\n",
                                                                serde_json::to_string(&content_stop).unwrap_or_default()
                                                            )));
                                                            content_block_index += 1;
                                                        }
                                                    }

                                                    // Tool calls
                                                    if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                                                        for tc in tool_calls {
                                                            let tc_index = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(tool_call_index as u64);
                                                            if let Some(function) = tc.get("function") {
                                                                if let Some(name) = function.get("name").and_then(|v| v.as_str()) {
                                                                    // Check if this is a new tool call
                                                                    if !delta.get("id").is_some() || tc_index as u32 == tool_call_index {
                                                                        let tool_id = tc.get("id")
                                                                            .and_then(|v| v.as_str())
                                                                            .unwrap_or(&format!("toolu_{}", Uuid::new_v4()))
                                                                            .to_string();
                                                                        let args = function.get("arguments")
                                                                            .and_then(|v| v.as_str())
                                                                            .unwrap_or("{}")
                                                                            .to_string();

                                                                        let content_start = json!({
                                                                            "type": "content_block_start",
                                                                            "index": content_block_index,
                                                                            "content_block": {
                                                                                "type": "tool_use",
                                                                                "id": tool_id,
                                                                                "name": name,
                                                                                "input": serde_json::from_str::<Value>(&args).unwrap_or(json!({})),
                                                                            }
                                                                        });
                                                                        yield Ok(Bytes::from(format!(
                                                                            "event: content_block_start\ndata: {}\n\n",
                                                                            serde_json::to_string(&content_start).unwrap_or_default()
                                                                        )));

                                                                        let content_stop = json!({
                                                                            "type": "content_block_stop",
                                                                            "index": content_block_index,
                                                                        });
                                                                        yield Ok(Bytes::from(format!(
                                                                            "event: content_block_stop\ndata: {}\n\n",
                                                                            serde_json::to_string(&content_stop).unwrap_or_default()
                                                                        )));
                                                                        content_block_index += 1;
                                                                        tool_call_index += 1;
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }

                                                // Usage
                                                if let Some(usage) = choice.get("usage") {
                                                    final_input_tokens = usage.get("prompt_tokens")
                                                        .and_then(|v| v.as_u64())
                                                        .unwrap_or(0) as u32;
                                                    final_output_tokens = usage.get("completion_tokens")
                                                        .and_then(|v| v.as_u64())
                                                        .unwrap_or(0) as u32;
                                                }

                                                // Finish reason
                                                if let Some(finish_reason) = choice.get("finish_reason").and_then(|v| v.as_str()) {
                                                    let stop_reason = match finish_reason {
                                                        "stop" => "end_turn",
                                                        "length" => "max_tokens",
                                                        "tool_calls" => "tool_use",
                                                        "content_filter" => "end_turn",
                                                        _ => "end_turn",
                                                    };
                                                    // Only emit message_delta if message_start was emitted
                                                    if emitted_message_start {
                                                        let usage_json = json!({
                                                            "input_tokens": final_input_tokens,
                                                            "output_tokens": final_output_tokens,
                                                        });
                                                        let delta_event = json!({
                                                            "type": "message_delta",
                                                            "delta": {
                                                                "stop_reason": stop_reason,
                                                                "stop_sequence": null,
                                                            },
                                                            "usage": usage_json,
                                                        });
                                                        yield Ok(Bytes::from(format!(
                                                            "event: message_delta\ndata: {}\n\n",
                                                            serde_json::to_string(&delta_event).unwrap_or_default()
                                                        )));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        Some(Err(e)) => {
                            let err_event = json!({
                                "type": "error",
                                "error": {
                                    "type": "upstream_error",
                                    "message": format!("Stream error: {}", e),
                                }
                            });
                            yield Ok(Bytes::from(format!(
                                "event: error\ndata: {}\n\n",
                                serde_json::to_string(&err_event).unwrap_or_default()
                            )));
                            break;
                        }
                        None => {
                            // Stream ended without [DONE]
                            if emitted_message_start {
                                let usage_json = json!({
                                    "input_tokens": final_input_tokens,
                                    "output_tokens": final_output_tokens,
                                });
                                let delta_event = json!({
                                    "type": "message_delta",
                                    "delta": {
                                        "stop_reason": "end_turn",
                                        "stop_sequence": null,
                                    },
                                    "usage": usage_json,
                                });
                                yield Ok(Bytes::from(format!(
                                    "event: message_delta\ndata: {}\n\n",
                                    serde_json::to_string(&delta_event).unwrap_or_default()
                                )));
                            }
                            yield Ok(Bytes::from(
                                "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
                            ));
                            break;
                        }
                    }
                }
                _ = heartbeat_interval.tick() => {
                    yield Ok(Bytes::from(": ping - heartbeat\n\n"));
                }
            }
        }
    };

    Box::pin(stream)
}

/// Convert an OpenAI Chat Completions JSON response to a Value suitable for Claude format.
/// This returns a JSON Value that matches the Claude Messages API non-streaming response shape.
pub fn openai_to_claude_response(openai_response: &Value, model: &str) -> Result<Value, String> {
    let uuid_str = Uuid::new_v4().to_string();
    let message_id = format!(
        "msg_{}",
        uuid_str
            .replace('-', "")
            .chars()
            .take(24)
            .collect::<String>()
    );

    let choices = openai_response
        .get("choices")
        .and_then(|c| c.as_array())
        .ok_or_else(|| "Missing choices in OpenAI response".to_string())?;

    let mut content_blocks: Vec<Value> = Vec::new();
    let mut stop_reason = "end_turn";

    for choice in choices {
        if let Some(message) = choice.get("message") {
            // Text content
            if let Some(text) = message.get("content").and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    content_blocks.push(json!({
                        "type": "text",
                        "text": text,
                    }));
                }
            }

            // Reasoning content
            if let Some(reasoning) = message.get("reasoning_content").and_then(|v| v.as_str()) {
                if !reasoning.is_empty() {
                    content_blocks.push(json!({
                        "type": "thinking",
                        "thinking": reasoning,
                        "signature": null,
                    }));
                }
            }

            // Tool calls
            if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
                for tc in tool_calls {
                    if let Some(function) = tc.get("function") {
                        let id = tc
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("toolu_unknown")
                            .to_string();
                        let name = function
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let input = function
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .and_then(|s| serde_json::from_str::<Value>(s).ok())
                            .unwrap_or(json!({}));
                        content_blocks.push(json!({
                            "type": "tool_use",
                            "id": id,
                            "name": name,
                            "input": input,
                        }));
                        stop_reason = "tool_use";
                    }
                }
            }
        }

        if let Some(fr) = choice.get("finish_reason").and_then(|v| v.as_str()) {
            stop_reason = match fr {
                "stop" => "end_turn",
                "length" => "max_tokens",
                "tool_calls" => "tool_use",
                "content_filter" => "end_turn",
                _ => "end_turn",
            };
        }
    }

    // Extract usage
    let usage = openai_response
        .get("usage")
        .map(|u| {
            json!({
                "input_tokens": u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                "output_tokens": u.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            })
        })
        .unwrap_or(json!({
            "input_tokens": 0,
            "output_tokens": 0,
        }));

    let claude_resp = json!({
        "id": message_id,
        "type": "message",
        "role": "assistant",
        "content": content_blocks,
        "model": model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": usage,
    });

    Ok(claude_resp)
}
