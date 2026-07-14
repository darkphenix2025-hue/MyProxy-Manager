use crate::proxy::monitor::ProxyRequestLog;
use crate::proxy::server::AppState;
use axum::{
    body::Body,
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use serde_json::Value;
use std::time::Instant;
use tauri::Emitter;

const MAX_REQUEST_DURATION: std::time::Duration = std::time::Duration::from_secs(30);
const MAX_REQUEST_DURATION_MS: u64 = 30_000;

/// Extension type for propagating trace_id across the request lifecycle.
#[derive(Clone, Debug, serde::Serialize)]
pub struct LlmTraceId(pub String);

impl std::fmt::Display for LlmTraceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
use crate::proxy::middleware::auth::UserTokenIdentity;
use futures::StreamExt;

const MAX_REQUEST_LOG_SIZE: usize = 100 * 1024 * 1024; // 100MB
const MAX_RESPONSE_LOG_SIZE: usize = 100 * 1024 * 1024; // 100MB for image responses

/// Helper function to record User Token usage
fn record_user_token_usage(
    user_token_identity: &Option<UserTokenIdentity>,
    log: &ProxyRequestLog,
    user_agent: Option<String>,
) {
    if let Some(identity) = user_token_identity {
        let _ = crate::modules::user_token_db::record_token_usage_and_ip(
            &identity.token_id,
            log.client_ip.as_deref().unwrap_or("127.0.0.1"),
            log.model.as_deref().unwrap_or("unknown"),
            log.input_tokens.unwrap_or(0) as i32,
            log.output_tokens.unwrap_or(0) as i32,
            log.status as u16,
            user_agent,
        );
    }
}

/// [LLM Logging] Log client response from the proxy monitor log.
fn log_client_response_from_log(log: &ProxyRequestLog) {
    if let Some(ref body_str) = log.response_body {
        let body = if let Ok(json) = serde_json::from_str::<Value>(body_str) {
            crate::proxy::llm_logger::LlmBody::Json(json)
        } else {
            crate::proxy::llm_logger::LlmBody::Text(body_str.clone())
        };
        let entry = crate::proxy::llm_logger::LlmLogEntry {
            trace_id: log.id.clone(),
            timestamp: crate::proxy::llm_logger::now_timestamp(),
            stage: "client_response",
            method: log.method.clone(),
            url: log.url.clone(),
            model: log.model.clone(),
            status: Some(log.status),
            content_type: None,
            body_size_bytes: body_str.len(),
            body,
        };
        crate::proxy::llm_logger::log_entry(&entry);
    }
}

pub async fn monitor_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let _logging_enabled = state.monitor.is_enabled();

    let method = request.method().to_string();
    let uri = request.uri().to_string();

    // Skip non-LLM paths from traffic logging
    // [FIX] Normalize URI: Axum may return uri without leading slash in some cases
    let uri_path = uri.strip_prefix('/').unwrap_or(&uri);
    if uri.contains("event_logging")
        || uri.contains("/api/")
        || uri_path.starts_with("internal/")
        || uri_path.starts_with("admin")
        || uri_path.starts_with("accounts")
        || uri_path.starts_with("stats")
        || uri_path.starts_with("config")
        || uri_path.starts_with("security")
        || uri_path.starts_with("user-tokens")
        || uri_path.starts_with("auth")
        || uri_path.starts_with("system")
        || uri_path.starts_with("proxy/cli")
        || uri_path == "health"
        || uri_path == "healthz"
    {
        return next.run(request).await;
    }

    // [FIX] Skip HEAD requests from traffic logging
    if method == "HEAD" {
        return next.run(request).await;
    }

    let start = Instant::now();

    // Extract client IP from headers (X-Forwarded-For or X-Real-IP)
    // IMPORTANT: Extract from Request headers, not Response headers (since we want the client's IP)
    // Note: We need to do this BEFORE consuming the request body if possible, or extract it from the original request
    let client_ip = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .or_else(|| {
            request
                .headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        });

    let user_agent = request
        .headers()
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let mut model = if uri.contains("/v1beta/models/") {
        uri.split("/v1beta/models/")
            .nth(1)
            .and_then(|s| s.split(':').next())
            .map(|s| s.to_string())
    } else {
        None
    };

    let request_body_str;
    let llm_trace_id: String = uuid::Uuid::new_v4().to_string()[..8].to_string();

    // [FIX] 从请求 extensions 提取 UserTokenIdentity (由 Auth 中间件注入)
    // 必须在处理 request body 之前提取，因为 into_parts() 后需要保留这个值
    let user_token_identity = request.extensions().get::<UserTokenIdentity>().cloned();

    let request = if method == "POST" {
        let (mut parts, body) = request.into_parts();
        match axum::body::to_bytes(body, MAX_REQUEST_LOG_SIZE).await {
            Ok(bytes) => {
                if model.is_none() {
                    model = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| {
                        v.get("model")
                            .and_then(|m| m.as_str())
                            .map(|s| s.to_string())
                    });
                }
                request_body_str = if let Ok(s) = std::str::from_utf8(&bytes) {
                    Some(s.to_string())
                } else {
                    Some("[Binary Request Data]".to_string())
                };

                // [LLM Logging] Log client request with shared trace_id
                if let Ok(json_val) = serde_json::from_slice::<Value>(&bytes) {
                    let entry = crate::proxy::llm_logger::LlmLogEntry {
                        trace_id: llm_trace_id.clone(),
                        timestamp: crate::proxy::llm_logger::now_timestamp(),
                        stage: "client_request",
                        method: method.clone(),
                        url: uri.clone(),
                        model: model.clone(),
                        status: None,
                        content_type: Some("application/json".to_string()),
                        body_size_bytes: bytes.len(),
                        body: crate::proxy::llm_logger::LlmBody::Json(json_val),
                    };
                    crate::proxy::llm_logger::log_entry(&entry);
                }

                // Store trace_id in extensions for downstream stages
                parts.extensions.insert(LlmTraceId(llm_trace_id.clone()));
                // Also set as request header for reliable propagation
                if let Ok(hv) = axum::http::HeaderValue::from_str(&llm_trace_id) {
                    parts
                        .headers
                        .insert(axum::http::HeaderName::from_static("x-llm-trace-id"), hv);
                }

                Request::from_parts(parts, Body::from(bytes))
            }
            Err(_) => {
                request_body_str = None;
                Request::from_parts(parts, Body::empty())
            }
        }
    } else {
        request_body_str = None;
        request
    };

    // Resolve route metadata before entering the handler. This makes the
    // in-flight record useful immediately instead of waiting for the upstream
    // response (which may be a long-lived stream).
    let protocol = if uri.contains("/v1/messages") {
        Some("anthropic".to_string())
    } else if uri.contains("/v1beta/models") {
        Some("gemini".to_string())
    } else if uri.starts_with("/v1/") {
        Some("openai".to_string())
    } else {
        None
    };
    let route_metadata = if let Some(ref requested_model) = model {
        let mapped = crate::proxy::common::model_mapping::resolve_model_route(
            requested_model,
            &*state.custom_mapping.read().await,
        );
        let router = state.provider_router.read().await;
        if !router.is_empty() {
            let selection = router.select(&mapped, None, None, Some(&mapped));
            let provider = selection.provider;
            let upstream_url = if provider.base_url.is_empty() {
                None
            } else {
                Some(provider.base_url.clone())
            };
            Some((
                mapped,
                Some(provider.name.clone()),
                Some(selection.resolved_model),
                upstream_url,
                Some(match provider.protocol {
                    crate::proxy::config::ProviderProtocol::AnthropicPassthrough => "anthropic",
                    crate::proxy::config::ProviderProtocol::OpenAICompatible => "openai",
                    crate::proxy::config::ProviderProtocol::GeminiV1Internal => "gemini",
                }.to_string()),
            ))
        } else {
            Some((mapped, None, None, None, None))
        }
    } else {
        None
    };

    // Emit initial in_flight event for segmented log display
    {
        let app_handle = state.monitor.app_handle.clone();
        let client_ip_clone = client_ip.clone();
        let method_clone = method.clone();
        let uri_clone = uri.clone();
        let model_clone = model.clone();
        let route_metadata_clone = route_metadata.clone();
        let protocol_clone = protocol.clone();
        let username_clone = user_token_identity.as_ref().map(|i| i.username.clone());
        let trace_id_clone = llm_trace_id.clone();

        tokio::spawn(async move {
            let in_flight_log = ProxyRequestLog {
                id: trace_id_clone,
                timestamp: chrono::Utc::now().timestamp_millis(),
                method: method_clone,
                url: uri_clone,
                status: 0, // In-flight marker
                duration: 0,
                model: model_clone,
                mapped_model: route_metadata_clone.as_ref().map(|m| m.0.clone()),
                account_email: None,
                provider_name: route_metadata_clone.as_ref().and_then(|m| m.1.clone()),
                client_ip: client_ip_clone,
                error: None,
                request_body: None,
                response_body: None,
                input_tokens: None,
                output_tokens: None,
                protocol: protocol_clone,
                upstream_protocol: route_metadata_clone.as_ref().and_then(|m| m.4.clone()),
                upstream_model: route_metadata_clone.as_ref().and_then(|m| m.2.clone()),
                upstream_url: route_metadata_clone.as_ref().and_then(|m| m.3.clone()),
                upstream_request_body: None,
                upstream_response_body: None,
                username: username_clone,
                in_flight: true,
            };
            if let Some(app) = &app_handle {
                let _ = app.emit("proxy://request", &in_flight_log);
            }
            // No DB save for in_flight — final log will be saved
        });
    }

    let response = match tokio::time::timeout(MAX_REQUEST_DURATION, next.run(request)).await {
        Ok(response) => response,
        Err(_) => Response::builder()
            .status(axum::http::StatusCode::GATEWAY_TIMEOUT)
            .header("content-type", "application/json")
            .body(Body::from(r#"{"error":{"type":"timeout","message":"Request timed out after 30 seconds"}}"#))
            .unwrap_or_else(|_| Response::new(Body::empty())),
    };

    // user_token_identity 已在上面从请求 extensions 中提取

    let duration = start.elapsed().as_millis().min(MAX_REQUEST_DURATION_MS as u128) as u64;
    let status = response.status().as_u16();

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Extract account email from X-Account-Email header if present
    let account_email = response
        .headers()
        .get("X-Account-Email")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Extract provider name from X-Provider-Name header if present
    let provider_name = response
        .headers()
        .get("X-Provider-Name")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Extract mapped model from X-Mapped-Model header if present
    let mapped_model = response
        .headers()
        .get("X-Mapped-Model")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Extract upstream protocol from X-Upstream-Protocol header if present
    let upstream_protocol = response
        .headers()
        .get("X-Upstream-Protocol")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Extract upstream model from X-Upstream-Model header if present
    let upstream_model = response
        .headers()
        .get("X-Upstream-Model")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Extract upstream URL — prefer X-Upstream-Path (just the API path), fall back to extracting path from full URL.
    // Note: X-Upstream-Path may be empty when the caller already built the full URL (e.g. OpenAI→Anthropic conversion),
    // so filter out empty strings to ensure the X-Upstream-URL fallback is used.
    let upstream_url = response
        .headers()
        .get("X-Upstream-Path")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .or_else(|| {
            response
                .headers()
                .get("X-Upstream-URL")
                .and_then(|v| v.to_str().ok())
                .and_then(|url| {
                    // Extract just the path portion from full URL
                    if let Some(pos) = url.find("://") {
                        let after_scheme = &url[pos + 3..];
                        if let Some(slash_pos) = after_scheme.find('/') {
                            Some(after_scheme[slash_pos..].to_string())
                        } else {
                            None
                        }
                    } else {
                        Some(url.to_string())
                    }
                })
        });

    // Client IP has been extracted at the beginning of the function

    // Extract username from UserTokenIdentity if present
    let username = user_token_identity
        .as_ref()
        .map(|identity| identity.username.clone());

    let monitor = state.monitor.clone();
    let upstream_trace = state.upstream_trace_cache.take(&llm_trace_id).await;
    let upstream_request_body = upstream_trace.as_ref().and_then(|t| t.request_body.clone());
    let upstream_response_body = upstream_trace
        .as_ref()
        .and_then(|t| t.response_body.clone());
    let mut log = ProxyRequestLog {
        id: llm_trace_id.clone(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        method,
        url: uri,
        status,
        duration,
        model,
        mapped_model,
        account_email,
        provider_name,
        client_ip,
        error: None,
        request_body: request_body_str,
        response_body: None,
        input_tokens: None,
        output_tokens: None,
        protocol,
        upstream_protocol,
        upstream_model,
        upstream_url,
        upstream_request_body,
        upstream_response_body,
        username,
        in_flight: false,
    };

    if content_type.contains("text/event-stream") {
        let (parts, body) = response.into_parts();
        let mut stream = body.into_data_stream();
        let (tx, rx) = tokio::sync::mpsc::channel(64);

        tokio::spawn(async move {
            let mut all_stream_data = Vec::new();
            let mut last_few_bytes = Vec::new();
            let mut stream_timed_out = false;

            loop {
                let remaining = MAX_REQUEST_DURATION.saturating_sub(start.elapsed());
                let chunk_res = match tokio::time::timeout(remaining, stream.next()).await {
                    Ok(Some(chunk_res)) => chunk_res,
                    Ok(None) => break,
                    Err(_) => {
                        stream_timed_out = true;
                        break;
                    }
                };
                if let Ok(chunk) = chunk_res {
                    all_stream_data.extend_from_slice(&chunk);

                    if chunk.len() > 8192 {
                        last_few_bytes = chunk.slice(chunk.len() - 8192..).to_vec();
                    } else {
                        last_few_bytes.extend_from_slice(&chunk);
                        if last_few_bytes.len() > 8192 {
                            last_few_bytes.drain(0..last_few_bytes.len() - 8192);
                        }
                    }
                    let _ = tx.send(Ok::<_, axum::Error>(chunk)).await;
                } else if let Err(e) = chunk_res {
                    let _ = tx.send(Err(axum::Error::new(e))).await;
                }
            }

            if stream_timed_out {
                log.error = Some("Request timed out after 30 seconds".to_string());
            }

            // Parse and consolidate stream data into readable format
            if let Ok(full_response) = std::str::from_utf8(&all_stream_data) {
                let mut thinking_content = String::new();
                let mut response_content = String::new();
                let mut thinking_signature = String::new();
                let mut tool_calls: Vec<Value> = Vec::new();

                for line in full_response.lines() {
                    let line = line.trim();
                    if !line.starts_with("data:") {
                        continue;
                    }
                    let json_str = line.trim_start_matches("data:").trim();
                    if json_str == "[DONE]" {
                        continue;
                    }

                    if let Ok(json) = serde_json::from_str::<Value>(json_str) {
                        // OpenAI format: choices[0].delta.content / reasoning_content / tool_calls
                        if let Some(choices) = json.get("choices").and_then(|c| c.as_array()) {
                            for choice in choices {
                                if let Some(delta) = choice.get("delta") {
                                    // Thinking/reasoning content
                                    if let Some(thinking) =
                                        delta.get("reasoning_content").and_then(|v| v.as_str())
                                    {
                                        thinking_content.push_str(thinking);
                                    }
                                    // Main response content
                                    if let Some(content) =
                                        delta.get("content").and_then(|v| v.as_str())
                                    {
                                        response_content.push_str(content);
                                    }
                                    // Tool calls
                                    if let Some(delta_tool_calls) =
                                        delta.get("tool_calls").and_then(|t| t.as_array())
                                    {
                                        for tc in delta_tool_calls {
                                            if let Some(index) =
                                                tc.get("index").and_then(|i| i.as_u64())
                                            {
                                                let idx = index as usize;
                                                while tool_calls.len() <= idx {
                                                    tool_calls.push(serde_json::json!({
                                                        "id": "",
                                                        "type": "function",
                                                        "function": { "name": "", "arguments": "" }
                                                    }));
                                                }
                                                let current_tc = &mut tool_calls[idx];
                                                if let Some(id) =
                                                    tc.get("id").and_then(|v| v.as_str())
                                                {
                                                    current_tc["id"] =
                                                        Value::String(id.to_string());
                                                }
                                                if let Some(func) = tc.get("function") {
                                                    if let Some(name) =
                                                        func.get("name").and_then(|v| v.as_str())
                                                    {
                                                        current_tc["function"]["name"] =
                                                            Value::String(name.to_string());
                                                    }
                                                    if let Some(args) = func
                                                        .get("arguments")
                                                        .and_then(|v| v.as_str())
                                                    {
                                                        let old_args = current_tc["function"]
                                                            ["arguments"]
                                                            .as_str()
                                                            .unwrap_or("");
                                                        current_tc["function"]["arguments"] =
                                                            Value::String(format!(
                                                                "{}{}",
                                                                old_args, args
                                                            ));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Claude/Anthropic format: content_block_start, content_block_delta, etc.
                        let msg_type = json.get("type").and_then(|t| t.as_str());
                        match msg_type {
                            Some("message_start") => {
                                // [FIX] Extract input_tokens from message_start event
                                if let Some(usage) =
                                    json.get("message").and_then(|m| m.get("usage"))
                                {
                                    if let Some(input_tokens) =
                                        usage.get("input_tokens").and_then(|v| v.as_u64())
                                    {
                                        log.input_tokens = Some(input_tokens as u32);
                                    }
                                    if let Some(output_tokens) =
                                        usage.get("output_tokens").and_then(|v| v.as_u64())
                                    {
                                        log.output_tokens = Some(output_tokens as u32);
                                    }
                                }
                            }
                            Some("content_block_start") => {
                                if let (Some(index), Some(block)) = (
                                    json.get("index").and_then(|i| i.as_u64()),
                                    json.get("content_block"),
                                ) {
                                    let idx = index as usize;
                                    if block.get("type").and_then(|t| t.as_str())
                                        == Some("tool_use")
                                    {
                                        let id =
                                            block.get("id").and_then(|v| v.as_str()).unwrap_or("");
                                        let name = block
                                            .get("name")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("");
                                        while tool_calls.len() <= idx {
                                            tool_calls.push(Value::Null);
                                        }
                                        tool_calls[idx] = serde_json::json!({
                                            "id": id,
                                            "type": "function",
                                            "function": { "name": name, "arguments": "" }
                                        });
                                    }
                                }
                            }
                            Some("content_block_delta") => {
                                if let (Some(index), Some(delta)) = (
                                    json.get("index").and_then(|i| i.as_u64()),
                                    json.get("delta"),
                                ) {
                                    let idx = index as usize;

                                    // Tool use input delta
                                    if let Some(delta_json) =
                                        delta.get("input_json_delta").and_then(|v| v.as_str())
                                    {
                                        if idx < tool_calls.len() && !tool_calls[idx].is_null() {
                                            let old_args = tool_calls[idx]["function"]["arguments"]
                                                .as_str()
                                                .unwrap_or("");
                                            tool_calls[idx]["function"]["arguments"] =
                                                Value::String(format!(
                                                    "{}{}",
                                                    old_args, delta_json
                                                ));
                                        }
                                    }
                                    // Legacy/Native thinking block
                                    if let Some(thinking) =
                                        delta.get("thinking").and_then(|v| v.as_str())
                                    {
                                        thinking_content.push_str(thinking);
                                    }
                                    // Text content
                                    if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                        response_content.push_str(text);
                                    }
                                }
                            }
                            Some("message_delta") => {
                                // [FIX] Extract usage from root level AND delta level
                                // Anthropic SSE puts usage at root of message_delta, not inside delta
                                if let Some(usage) = json
                                    .get("usage")
                                    .or_else(|| json.get("delta").and_then(|d| d.get("usage")))
                                {
                                    if let Some(input_tokens) =
                                        usage.get("input_tokens").and_then(|v| v.as_u64())
                                    {
                                        log.input_tokens = Some(input_tokens as u32);
                                    }
                                    if let Some(output_tokens) =
                                        usage.get("output_tokens").and_then(|v| v.as_u64())
                                    {
                                        log.output_tokens = Some(output_tokens as u32);
                                    }
                                    if let Some(cache_creation) = usage
                                        .get("cache_creation_input_tokens")
                                        .and_then(|v| v.as_u64())
                                    {
                                        log.input_tokens =
                                            log.input_tokens.map(|v| v + cache_creation as u32);
                                    }
                                    if let Some(cache_read) = usage
                                        .get("cache_read_input_tokens")
                                        .and_then(|v| v.as_u64())
                                    {
                                        log.input_tokens =
                                            log.input_tokens.map(|v| v + cache_read as u32);
                                    }
                                }
                            }
                            _ => {}
                        }

                        // Legacy Claude delta (for older implementations or simplified streams)
                        if msg_type.is_none() {
                            if let Some(delta) = json.get("delta") {
                                // Thinking block
                                if let Some(thinking) =
                                    delta.get("thinking").and_then(|v| v.as_str())
                                {
                                    thinking_content.push_str(thinking);
                                }
                                // Thinking signature
                                if let Some(sig) = delta.get("signature").and_then(|v| v.as_str()) {
                                    thinking_signature = sig.to_string();
                                }
                                // Text content
                                if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                                    response_content.push_str(text);
                                }
                            }
                        }

                        // Token usage extraction
                        if let Some(usage) = json
                            .get("usage")
                            .or(json.get("usageMetadata"))
                            .or(json.get("response").and_then(|r| r.get("usage")))
                        {
                            log.input_tokens = usage
                                .get("prompt_tokens")
                                .or(usage.get("input_tokens"))
                                .or(usage.get("promptTokenCount"))
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32);
                            log.output_tokens = usage
                                .get("completion_tokens")
                                .or(usage.get("output_tokens"))
                                .or(usage.get("candidatesTokenCount"))
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32);

                            if log.input_tokens.is_none() && log.output_tokens.is_none() {
                                log.output_tokens = usage
                                    .get("total_tokens")
                                    .or(usage.get("totalTokenCount"))
                                    .and_then(|v| v.as_u64())
                                    .map(|v| v as u32);
                            }
                        }
                    }
                }

                // Build consolidated response object
                let mut consolidated = serde_json::Map::new();

                if !thinking_content.is_empty() {
                    consolidated.insert("thinking".to_string(), Value::String(thinking_content));
                }
                if !thinking_signature.is_empty() {
                    consolidated.insert(
                        "thinking_signature".to_string(),
                        Value::String(thinking_signature),
                    );
                }
                if !response_content.is_empty() {
                    consolidated.insert("content".to_string(), Value::String(response_content));
                }

                if !tool_calls.is_empty() {
                    let clean_tool_calls: Vec<Value> =
                        tool_calls.into_iter().filter(|v| !v.is_null()).collect();
                    if !clean_tool_calls.is_empty() {
                        consolidated
                            .insert("tool_calls".to_string(), Value::Array(clean_tool_calls));
                    }
                }
                if let Some(input) = log.input_tokens {
                    consolidated.insert("input_tokens".to_string(), Value::Number(input.into()));
                }
                if let Some(output) = log.output_tokens {
                    consolidated.insert("output_tokens".to_string(), Value::Number(output.into()));
                }

                if consolidated.is_empty() {
                    // Fallback: store raw SSE data if parsing failed
                    log.response_body = Some(full_response.to_string());
                } else {
                    log.response_body = Some(
                        serde_json::to_string_pretty(&Value::Object(consolidated))
                            .unwrap_or_else(|_| full_response.to_string()),
                    );
                }
            } else {
                log.response_body = Some(format!(
                    "[Binary Stream Data: {} bytes]",
                    all_stream_data.len()
                ));
            }

            // Fallback token extraction from tail if not already extracted
            if log.input_tokens.is_none() && log.output_tokens.is_none() {
                if let Ok(full_tail) = std::str::from_utf8(&last_few_bytes) {
                    for line in full_tail.lines().rev() {
                        if line.starts_with("data:")
                            && (line.contains("\"usage\"") || line.contains("\"usageMetadata\""))
                        {
                            let json_str = line.trim_start_matches("data:").trim();
                            if let Ok(json) = serde_json::from_str::<Value>(json_str) {
                                if let Some(usage) = json
                                    .get("usage")
                                    .or(json.get("usageMetadata"))
                                    .or(json.get("response").and_then(|r| r.get("usage")))
                                {
                                    log.input_tokens = usage
                                        .get("prompt_tokens")
                                        .or(usage.get("input_tokens"))
                                        .or(usage.get("promptTokenCount"))
                                        .and_then(|v| v.as_u64())
                                        .map(|v| v as u32);
                                    log.output_tokens = usage
                                        .get("completion_tokens")
                                        .or(usage.get("output_tokens"))
                                        .or(usage.get("candidatesTokenCount"))
                                        .and_then(|v| v.as_u64())
                                        .map(|v| v as u32);
                                    break;
                                }
                            }
                        }
                    }
                }
            }

            if log.status >= 400 {
                log.error = Some("Stream Error or Failed".to_string());
            }

            // Record User Token Usage
            record_user_token_usage(&user_token_identity, &log, user_agent.clone());

            // [LLM Logging] Log client response
            log_client_response_from_log(&log);

            monitor.log_request(log).await;
        });

        Response::from_parts(
            parts,
            Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)),
        )
    } else if content_type.contains("application/json") || content_type.contains("text/") {
        let (parts, body) = response.into_parts();
        match axum::body::to_bytes(body, MAX_RESPONSE_LOG_SIZE).await {
            Ok(bytes) => {
                if let Ok(s) = std::str::from_utf8(&bytes) {
                    if let Ok(json) = serde_json::from_str::<Value>(&s) {
                        // 支持 OpenAI "usage" 或 Gemini "usageMetadata"
                        if let Some(usage) = json.get("usage").or(json.get("usageMetadata")) {
                            log.input_tokens = usage
                                .get("prompt_tokens")
                                .or(usage.get("input_tokens"))
                                .or(usage.get("promptTokenCount"))
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32);
                            log.output_tokens = usage
                                .get("completion_tokens")
                                .or(usage.get("output_tokens"))
                                .or(usage.get("candidatesTokenCount"))
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32);

                            if log.input_tokens.is_none() && log.output_tokens.is_none() {
                                log.output_tokens = usage
                                    .get("total_tokens")
                                    .or(usage.get("totalTokenCount"))
                                    .and_then(|v| v.as_u64())
                                    .map(|v| v as u32);
                            }
                        }
                    }
                    log.response_body = Some(s.to_string());
                } else {
                    log.response_body = Some("[Binary Response Data]".to_string());
                }

                if log.status >= 400 {
                    log.error = log.response_body.clone();
                }

                // Record User Token Usage
                record_user_token_usage(&user_token_identity, &log, user_agent.clone());

                // [LLM Logging] Log client response
                log_client_response_from_log(&log);

                monitor.log_request(log).await;
                Response::from_parts(parts, Body::from(bytes))
            }
            Err(_) => {
                log.response_body = Some("[Response too large (>100MB)]".to_string());

                // Record User Token Usage (even if too large)
                record_user_token_usage(&user_token_identity, &log, user_agent.clone());

                // [LLM Logging] Log client response
                log_client_response_from_log(&log);

                monitor.log_request(log).await;
                Response::from_parts(parts, Body::empty())
            }
        }
    } else {
        log.response_body = Some(format!("[{}]", content_type));

        // Record User Token Usage
        record_user_token_usage(&user_token_identity, &log, user_agent);

        // [LLM Logging] Log client response
        log_client_response_from_log(&log);

        monitor.log_request(log).await;
        response
    }
}
