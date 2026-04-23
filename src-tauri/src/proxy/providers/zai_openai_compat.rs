use axum::{
    body::Body,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use tokio::time::Duration;

use crate::proxy::server::AppState;
use crate::proxy::config::UpstreamProvider;
use crate::proxy::mappers::claude::models::ClaudeRequest;

/// Build a URL for OpenAI-compatible API.
pub(crate) fn build_openai_compat_url_for_provider(base_url: &str, path: &str) -> Result<String, String> {
    let base = base_url.trim_end_matches('/');
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    Ok(format!("{}{}", base, path))
}

/// Build a reqwest::Client for upstream OpenAI-compatible calls.
pub(crate) fn build_openai_compat_client_for_provider(
    upstream_proxy: Option<crate::proxy::config::UpstreamProxyConfig>,
    timeout_secs: u64,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs.max(5)));

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
) -> Response {
    use crate::proxy::mappers::claude::request_bridge::claude_to_openai_body;
    use crate::proxy::mappers::claude::response_bridge::{
        create_claude_sse_from_openai_stream, openai_to_claude_response,
    };

    let trace_id = format!(
        "req_{}",
        chrono::Utc::now().timestamp_subsec_millis()
    );
    let _trace_id = &trace_id;

    // Map model name
    let mapped_model = crate::proxy::providers::router::map_model_for_provider(
        &claude_req.model,
        &provider.model_mapping,
    );

    // Convert Claude request to OpenAI format
    let openai_body = claude_to_openai_body(claude_req);
    let client_wants_stream = claude_req.stream;

    let url = match build_openai_compat_url_for_provider(&provider.base_url, "/v1/chat/completions") {
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

    // Build request headers
    let mut req_headers = axum::http::HeaderMap::new();
    req_headers.insert(
        "content-type",
        HeaderValue::from_static("application/json"),
    );
    // Bearer token auth
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", provider.api_key)) {
        req_headers.insert("authorization", v);
    }
    // Passthrough headers
    for (k, v) in headers.iter() {
        let key = k.as_str().to_ascii_lowercase();
        match key.as_str() {
            "accept" | "user-agent" | "accept-encoding" => {
                req_headers.insert(k.clone(), v.clone());
            }
            _ => {}
        }
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
        return (status, String::from_utf8_lossy(&error_body).to_string()).into_response();
    }

    if client_wants_stream {
        // Stream: wrap OpenAI SSE through Claude SSE converter
        let openai_stream = response.bytes_stream();
        let claude_stream = create_claude_sse_from_openai_stream(
            Box::pin(openai_stream),
            mapped_model.clone(),
        );

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
