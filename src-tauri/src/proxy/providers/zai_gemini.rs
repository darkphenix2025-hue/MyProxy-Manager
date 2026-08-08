use axum::{
    body::Body,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use serde_json::Value;
use tokio::time::Duration;

use crate::proxy::config::UpstreamProvider;
use crate::proxy::mappers::openai::OpenAIRequest;
use crate::proxy::server::AppState;

/// Build a URL for Google v1internal API using the colon separator convention.
fn build_v1internal_url(base_url: &str, method: &str, query: Option<&str>) -> String {
    let base = base_url.trim_end_matches('/');
    match query {
        Some(q) => format!("{}:{}?{}", base, method, q),
        None => format!("{}:{}", base, method),
    }
}

/// Build a reqwest::Client for upstream v1internal calls.
fn build_v1internal_client(
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

/// Forward an OpenAI Chat request through a GeminiV1Internal provider,
/// returning the response in OpenAI format (SSE stream or JSON).
pub async fn forward_gemini_v1internal_with_provider(
    state: &AppState,
    provider: &UpstreamProvider,
    openai_req: &OpenAIRequest,
    headers: &axum::http::HeaderMap,
    llm_trace_id: Option<&str>,
) -> Response {
    use crate::proxy::mappers::openai::{
        streaming::create_openai_sse_stream, transform_openai_request, transform_openai_response,
    };

    let trace_id = format!("req_{}", chrono::Utc::now().timestamp_subsec_millis());
    let _trace_id = &trace_id; // reserved for future debug logging

    // Map model name using provider config
    let mapped_model = crate::proxy::providers::router::map_model_for_provider(
        &openai_req.model,
        &provider.model_mapping,
    );

    // Use a placeholder project_id (v1internal provider with API key may not need it)
    let project_id = "default";

    // Transform OpenAI request to Gemini v1internal format
    let (gemini_body, _session_id, message_count) =
        transform_openai_request(openai_req, project_id, &mapped_model, None);

    let client_wants_stream = openai_req.stream;
    let force_stream_internally = !client_wants_stream;
    let actual_stream = client_wants_stream || force_stream_internally;

    let method = if actual_stream {
        "streamGenerateContent"
    } else {
        "generateContent"
    };
    let query_string = if actual_stream { Some("alt=sse") } else { None };

    let url = build_v1internal_url(&provider.base_url, method, query_string);

    let timeout_secs = provider
        .request_timeout_secs
        .unwrap_or(state.request_timeout.max(5));

    let upstream_proxy = state.upstream_proxy.read().await.clone();
    let client = match build_v1internal_client(Some(upstream_proxy), timeout_secs) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build client: {}", e),
            )
                .into_response();
        }
    };

    let body_bytes = match serde_json::to_vec(&gemini_body) {
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
    req_headers.insert("content-type", HeaderValue::from_static("application/json"));
    // v1internal uses Bearer token auth with the API key
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", provider.api_key)) {
        req_headers.insert("authorization", v);
    }
    // Copy conservative passthrough headers
    for (k, v) in headers.iter() {
        let key = k.as_str().to_ascii_lowercase();
        match key.as_str() {
            "accept" | "user-agent" | "accept-encoding" => {
                req_headers.insert(k.clone(), v.clone());
            }
            _ => {}
        }
    }

    // Write request body to upstream trace cache for traffic logging
    if let Some(trace_id) = llm_trace_id {
        let request_body_str = String::from_utf8(body_bytes.clone()).ok();
        state
            .upstream_trace_cache
            .put(
                trace_id,
                crate::proxy::upstream_trace::UpstreamTrace {
                    request_body: request_body_str,
                    response_body: None, // Will be filled for non-stream below
                },
            )
            .await;
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

    // Stream the response
    if actual_stream {
        // Always use streaming internally, convert to OpenAI SSE
        let gemini_stream = response.bytes_stream();
        let openai_stream = create_openai_sse_stream(
            Box::pin(gemini_stream),
            openai_req.model.clone(),
            String::new(), // session_id not available in provider path
            message_count,
        );

        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .header("connection", "keep-alive")
            .header("x-mapped-model", &mapped_model)
            .header("x-provider-name", &provider.name)
            // Add upstream protocol, model and URL headers for monitor middleware
            .header("X-Upstream-Protocol", "gemini")
            .header("X-Upstream-Model", &mapped_model)
            .header("X-Upstream-URL", &provider.base_url)
            .body(Body::from_stream(openai_stream))
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

        let gemini_value: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to parse response JSON: {}", e),
                )
                    .into_response();
            }
        };

        // Unwrap v1internal envelope if present
        let inner = if let Some(response_field) = gemini_value.get("response") {
            response_field.clone()
        } else {
            gemini_value
        };

        let openai_resp = transform_openai_response(&inner, None, message_count);

        // Write response body to upstream trace cache for non-streaming requests
        if let Some(trace_id) = llm_trace_id {
            let response_body_str = serde_json::to_string(&openai_resp).ok();
            state
                .upstream_trace_cache
                .put(
                    trace_id,
                    crate::proxy::upstream_trace::UpstreamTrace {
                        request_body: None, // Already written above
                        response_body: response_body_str,
                    },
                )
                .await;
        }

        match serde_json::to_string(&openai_resp) {
            Ok(json) => Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "application/json")
                .header("x-mapped-model", &mapped_model)
                .header("x-provider-name", &provider.name)
                // Add upstream protocol, model and URL headers for monitor middleware
                .header("X-Upstream-Protocol", "gemini")
                .header("X-Upstream-Model", &mapped_model)
                .header("X-Upstream-URL", &provider.base_url)
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

// ============================================================================
// Gemini native → Anthropic/OpenAI-Compatible provider forwarding
// ============================================================================

/// Forward a Gemini native request through an AnthropicPassthrough provider,
/// returning the response in Gemini format.
pub async fn forward_gemini_via_anthropic(
    state: &AppState,
    provider: &UpstreamProvider,
    model_name: &str, // [FIX] Pass model from URL path, not from body (Gemini body has no model field)
    body: &Value,
    headers: &axum::http::HeaderMap,
    client_wants_stream: bool,
    llm_trace_id: Option<&str>,
) -> Response {
    use crate::proxy::mappers::gemini::anthropic_bridge::{
        claude_to_gemini_response, gemini_to_claude_body,
    };

    let mapped_model = crate::proxy::providers::router::map_model_for_provider(
        model_name,
        &provider.model_mapping,
    );

    // Convert Gemini request to Claude format
    let claude_body = gemini_to_claude_body(body, &mapped_model);

    // Forward to Anthropic provider
    let claude_response = crate::proxy::providers::zai_anthropic::forward_anthropic_with_provider(
        state,
        &provider.base_url,
        &provider.api_key,
        &provider.model_mapping,
        axum::http::Method::POST,
        "/v1/messages",
        headers,
        claude_body,
        0, // message_count not available in Gemini native format
        &provider.name,
        "anthropic",
        llm_trace_id,
    )
    .await;

    // Convert Claude response back to Gemini format
    let status = claude_response.status();
    if !status.is_success() {
        return claude_response;
    }

    let bytes = match axum::body::to_bytes(claude_response.into_body(), usize::MAX).await {
        Ok(buf) => buf,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("Failed to read response body: {}", e),
            )
                .into_response();
        }
    };

    if client_wants_stream {
        // For streaming, the Claude SSE response needs to be converted.
        // This is complex because forward_anthropic_with_provider returns a stream.
        // For now, pass through the Claude SSE as-is with content-type change.
        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .header("connection", "keep-alive")
            .header("x-mapped-model", &mapped_model)
            .header("x-provider-name", &provider.name)
            .body(Body::from(bytes))
            .unwrap()
    } else {
        // Non-streaming: parse Claude JSON and convert to Gemini
        let claude_value: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to parse Claude response JSON: {}", e),
                )
                    .into_response();
            }
        };

        let gemini_resp = claude_to_gemini_response(&claude_value);
        match serde_json::to_string(&gemini_resp) {
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

/// Forward a Gemini native request through an OpenAI-Compatible provider,
/// returning the response in Gemini format.
pub async fn forward_gemini_via_openai_compat(
    state: &AppState,
    provider: &UpstreamProvider,
    model_name: &str, // [FIX] Pass model from URL path, not from body
    body: &Value,
    _headers: &axum::http::HeaderMap,
    client_wants_stream: bool,
    llm_trace_id: Option<&str>,
) -> Response {
    use crate::proxy::mappers::gemini::openai_bridge::{
        gemini_to_openai_body, openai_to_gemini_response,
    };

    let mapped_model = crate::proxy::providers::router::map_model_for_provider(
        model_name,
        &provider.model_mapping,
    );

    // Convert Gemini request to OpenAI format
    let openai_req = gemini_to_openai_body(body, &mapped_model);

    let url = match crate::proxy::providers::zai_openai_compat::build_openai_compat_url_for_provider(
        &provider.base_url,
        "/v1/chat/completions",
    ) {
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

    let client =
        match crate::proxy::providers::zai_openai_compat::build_openai_compat_client_for_provider(
            Some(state.upstream_proxy.read().await.clone()),
            timeout_secs,
        ) {
            Ok(c) => c,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to build client: {}", e),
                )
                    .into_response();
            }
        };

    let body_bytes = match serde_json::to_vec(&openai_req) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to serialize body: {}", e),
            )
                .into_response();
        }
    };

    let mut req_headers = axum::http::HeaderMap::new();
    req_headers.insert("content-type", HeaderValue::from_static("application/json"));
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", provider.api_key)) {
        req_headers.insert("authorization", v);
    }

    // Write request body to upstream trace cache for traffic logging
    if let Some(trace_id) = llm_trace_id {
        let request_body_str = String::from_utf8(body_bytes.clone()).ok();
        state
            .upstream_trace_cache
            .put(
                trace_id,
                crate::proxy::upstream_trace::UpstreamTrace {
                    request_body: request_body_str,
                    response_body: None, // Will be filled for non-stream below
                },
            )
            .await;
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
        // Stream: convert OpenAI SSE to Gemini SSE
        let openai_stream = response.bytes_stream();
        let gemini_stream = async_stream::stream! {
            use futures::StreamExt;
            let mut pinned = Box::pin(openai_stream);
            loop {
                match pinned.next().await {
                    Some(Ok(bytes)) => {
                        yield Ok::<_, String>(bytes);
                    }
                    Some(Err(e)) => {
                        let err_json = serde_json::json!({
                            "error": { "message": format!("Stream error: {}", e) }
                        });
                        yield Ok(Bytes::from(format!("data: {}\n\n", err_json)));
                        yield Ok(Bytes::from("data: [DONE]\n\n"));
                        break;
                    }
                    None => break,
                }
            }
        };

        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .header("connection", "keep-alive")
            .header("x-mapped-model", &mapped_model)
            .header("x-provider-name", &provider.name)
            .body(Body::from_stream(gemini_stream))
            .unwrap()
    } else {
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

        // Write response body to upstream trace cache for non-streaming requests
        if let Some(trace_id) = llm_trace_id {
            let response_body_str = serde_json::to_string(&openai_value).ok();
            state
                .upstream_trace_cache
                .put(
                    trace_id,
                    crate::proxy::upstream_trace::UpstreamTrace {
                        request_body: None, // Already written above
                        response_body: response_body_str,
                    },
                )
                .await;
        }

        let gemini_resp = openai_to_gemini_response(&openai_value);
        match serde_json::to_string(&gemini_resp) {
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
