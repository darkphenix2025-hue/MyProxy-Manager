use axum::{
    body::Body,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use serde_json::Value;
use tokio::time::Duration;

use crate::proxy::config::UpstreamProvider;
use crate::proxy::mappers::claude::models::ClaudeRequest;
use crate::proxy::server::AppState;

/// Build a URL for OpenAI-compatible API.
pub(crate) fn build_openai_compat_url_for_provider(
    base_url: &str,
    path: &str,
) -> Result<String, String> {
    let base = base_url.trim_end_matches('/');
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    Ok(format!("{}{}", base, path))
}

/// Check if a base URL already ends with an API version segment like /v1, /v2, /v3, /v4, etc.
/// Returns true if the URL ends with a version segment, meaning the default /v1 prefix
/// should not be applied — the api_path should be empty instead.
pub(crate) fn base_url_has_version_suffix(base_url: &str) -> bool {
    let tail = base_url.trim_end_matches('/');
    if let Some(pos) = tail.rfind('/') {
        let last_segment = &tail[pos + 1..];
        if last_segment.len() >= 2
            && last_segment.starts_with('v')
            && last_segment[1..].chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

/// Build a reqwest::Client for upstream OpenAI-compatible calls.
pub(crate) fn build_openai_compat_client_for_provider(
    upstream_proxy: Option<crate::proxy::config::UpstreamProxyConfig>,
    timeout_secs: u64,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(timeout_secs.max(5)));

    if let Some(config) = upstream_proxy {
        if config.enabled && !config.url.is_empty() {
            let url = crate::proxy::config::normalize_proxy_url(&config.url);
            let proxy = reqwest::Proxy::all(&url)
                .map_err(|e| format!("Invalid upstream proxy url: {}", e))?;
            builder = builder.proxy(proxy);
        }
    }

    builder
        .tcp_nodelay(true)
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}

/// Forward a Claude Messages request through an OpenAI-Compatible provider.
/// Converts Claude → OpenAI request, then OpenAI response → Claude SSE/JSON.
pub async fn forward_claude_via_openai_compat(
    state: &AppState,
    provider: &UpstreamProvider,
    claude_req: &ClaudeRequest,
    headers: &axum::http::HeaderMap,
    trace_id: Option<&str>, // [LLM Logging]
) -> Response {
    use crate::proxy::mappers::claude::request_bridge::claude_to_openai_body;
    use crate::proxy::mappers::claude::response_bridge::{
        create_claude_sse_from_openai_stream, openai_to_claude_response,
    };

    let trace_id = trace_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("req_{}", chrono::Utc::now().timestamp_subsec_millis()));

    // Map model name
    let mapped_model = crate::proxy::providers::router::map_model_for_provider(
        &claude_req.model,
        &provider.model_mapping,
    );

    // Convert Claude request to OpenAI format
    let openai_body = claude_to_openai_body(claude_req);
    let client_wants_stream = claude_req.stream;

    // Determine the API path prefix. If base_url already ends with a version segment
    // like /v4, use empty string instead of the default /v1 to avoid double-version.
    let default_api_path = if base_url_has_version_suffix(&provider.base_url) {
        ""
    } else {
        "/v1"
    };
    let api_path = format!(
        "{}/chat/completions",
        provider
            .api_path
            .as_deref()
            .unwrap_or(default_api_path)
            .trim_end_matches('/')
    );
    let url = match build_openai_compat_url_for_provider(&provider.base_url, &api_path) {
        Ok(u) => u,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build URL: {}", e),
            )
                .into_response();
        }
    };

    let timeout_secs = provider
        .request_timeout_secs
        .unwrap_or(state.request_timeout.max(5));

    let upstream_proxy = state.upstream_proxy.read().await.clone();
    let client = match build_openai_compat_client_for_provider(Some(upstream_proxy), timeout_secs) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build client: {}", e),
            )
                .into_response();
        }
    };

    let body_bytes = match serde_json::to_vec(&openai_body) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to serialize body: {}", e),
            )
                .into_response();
        }
    };

    // [LLM Logging] Log upstream request
    // [Upstream Trace Cache] Store request body for traffic log detail modal
    let openai_body_for_trace = openai_body.clone();
    let state_ref = state.clone();
    let trace_id_cache = trace_id.clone();
    tokio::spawn(async move {
        state_ref
            .upstream_trace_cache
            .put(
                &trace_id_cache,
                crate::proxy::upstream_trace::UpstreamTrace {
                    request_body: Some(
                        serde_json::to_string(&openai_body_for_trace).unwrap_or_default(),
                    ),
                    response_body: None,
                },
            )
            .await;
    });
    {
        let entry = crate::proxy::llm_logger::LlmLogEntry {
            trace_id: trace_id.clone(),
            timestamp: crate::proxy::llm_logger::now_timestamp(),
            stage: "upstream_request",
            method: "POST".to_string(),
            url: url.clone(),
            model: Some(mapped_model.clone()),
            status: None,
            content_type: Some("application/json".to_string()),
            body_size_bytes: body_bytes.len(),
            body: crate::proxy::llm_logger::LlmBody::Json(openai_body.clone()),
        };
        crate::proxy::llm_logger::log_entry(&entry);
    }

    // Build request headers
    let mut req_headers = axum::http::HeaderMap::new();
    req_headers.insert("content-type", HeaderValue::from_static("application/json"));
    // Bearer token auth
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", provider.api_key)) {
        req_headers.insert("authorization", v);
    }
    // Passthrough headers
    let mut has_user_agent = false;
    for (k, v) in headers.iter() {
        let key = k.as_str().to_ascii_lowercase();
        match key.as_str() {
            "accept" | "user-agent" | "accept-encoding" => {
                if key == "user-agent" {
                    has_user_agent = true;
                }
                req_headers.insert(k.clone(), v.clone());
            }
            _ => {}
        }
    }
    // [FIX] Set a default User-Agent if the client didn't provide one.
    // Some providers (e.g. BAILIAN coding.dashscope.aliyuncs.com) reject requests
    // without a User-Agent header (HTTP 405).
    if !has_user_agent {
        req_headers.insert("user-agent", HeaderValue::from_static("claude-code/1.0.0"));
    }

    let response = match client
        .post(&url)
        .headers(req_headers)
        .body(body_bytes)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("Upstream request failed: {}", e),
            )
                .into_response();
        }
    };

    let status = response.status();
    if !status.is_success() {
        let error_body = response.bytes().await.unwrap_or_default();
        return Response::builder()
            .status(status)
            .header("x-provider-name", &provider.name)
            .header("x-mapped-model", &mapped_model)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(Body::from(String::from_utf8_lossy(&error_body).to_string()))
            .unwrap_or_else(|_| (status, "Failed to build response").into_response());
    }

    if client_wants_stream {
        // Stream: wrap OpenAI SSE through Claude SSE converter
        let openai_stream = response.bytes_stream();

        // [LLM Logging] Wrap stream to collect bytes for upstream_response logging
        let trace_id_log = trace_id.clone();
        let model_log = mapped_model.clone();
        let url_log = url.clone();
        let state_for_resp = state.clone();
        let wrapped_stream = async_stream::stream! {
            let mut collected: Vec<u8> = Vec::new();
            let mut pinned = Box::pin(openai_stream);
            while let Some(chunk) = pinned.next().await {
                if let Ok(ref b) = chunk {
                    collected.extend_from_slice(b);
                }
                yield chunk;
            }
            let raw_text = String::from_utf8_lossy(&collected).to_string();
            let body = crate::proxy::llm_logger::LlmBody::Text(raw_text.clone());
            let entry = crate::proxy::llm_logger::LlmLogEntry {
                trace_id: trace_id_log.clone(),
                timestamp: crate::proxy::llm_logger::now_timestamp(),
                stage: "upstream_response",
                method: "POST".to_string(),
                url: url_log,
                model: Some(model_log),
                status: Some(status.as_u16()),
                content_type: Some("text/event-stream".to_string()),
                body_size_bytes: collected.len(),
                body,
            };
            crate::proxy::llm_logger::log_entry(&entry);
            // [Upstream Trace Cache] Store response body for traffic log detail modal
            let state_for_cache = state_for_resp;
            tokio::spawn(async move {
                state_for_cache.upstream_trace_cache.put(&trace_id_log, crate::proxy::upstream_trace::UpstreamTrace {
                    request_body: None,
                    response_body: Some(raw_text),
                }).await;
            });
        };

        let claude_stream =
            create_claude_sse_from_openai_stream(Box::pin(wrapped_stream), mapped_model.clone());

        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .header("connection", "keep-alive")
            .header("x-mapped-model", &mapped_model)
            .header("x-provider-name", &provider.name)
            .body(Body::from_stream(claude_stream))
            .unwrap()
    } else {
        // Non-streaming: collect JSON and convert
        let bytes = match response.bytes().await {
            Ok(b) => b,
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to read response body: {}", e),
                )
                    .into_response();
            }
        };

        // [LLM Logging] Log upstream response (non-stream)
        // [Upstream Trace Cache] Store response body for traffic log detail modal
        {
            let raw_text = String::from_utf8_lossy(&bytes).to_string();
            let state_ref = state.clone();
            let tid = trace_id.clone();
            let raw_text_for_cache = raw_text.clone();
            tokio::spawn(async move {
                state_ref
                    .upstream_trace_cache
                    .put(
                        &tid,
                        crate::proxy::upstream_trace::UpstreamTrace {
                            request_body: None,
                            response_body: Some(raw_text_for_cache),
                        },
                    )
                    .await;
            });
            let body = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw_text) {
                crate::proxy::llm_logger::LlmBody::Json(json)
            } else {
                crate::proxy::llm_logger::LlmBody::Text(raw_text.clone())
            };
            let entry = crate::proxy::llm_logger::LlmLogEntry {
                trace_id: trace_id.clone(),
                timestamp: crate::proxy::llm_logger::now_timestamp(),
                stage: "upstream_response",
                method: "POST".to_string(),
                url: url.clone(),
                model: Some(mapped_model.clone()),
                status: Some(status.as_u16()),
                content_type: None,
                body_size_bytes: bytes.len(),
                body,
            };
            crate::proxy::llm_logger::log_entry(&entry);
        }

        let openai_value: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to parse response JSON: {}", e),
                )
                    .into_response();
            }
        };

        let claude_resp = match openai_to_claude_response(&openai_value, &mapped_model) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to convert response: {}", e),
                )
                    .into_response();
            }
        };

        match serde_json::to_string(&claude_resp) {
            Ok(json) => Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/json")
                .header("x-mapped-model", &mapped_model)
                .header("x-provider-name", &provider.name)
                .body(Body::from(json))
                .unwrap(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to serialize response: {}", e),
            )
                .into_response(),
        }
    }
}
