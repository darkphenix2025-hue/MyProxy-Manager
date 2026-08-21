use axum::{
    body::Body,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
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
    let endpoint = path
        .trim_start_matches('/')
        .strip_prefix("v1/")
        .unwrap_or(path);
    Ok(crate::proxy::config::build_provider_api_url(
        base_url, endpoint,
    ))
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

/// OpenCode Go exposes some models through the OpenAI Responses API even
/// though the provider as a whole is configured as OpenAI-compatible.  Keep
/// this decision model-aware so a single provider can still serve its Chat
/// Completions models through the existing path.
pub(crate) fn uses_responses_endpoint(model: &str) -> bool {
    crate::proxy::config::uses_responses_api(model)
}

/// Forward a Claude Messages request to a public OpenAI Responses endpoint
/// and translate the Responses response back to Claude SSE/JSON.
pub async fn forward_claude_via_openai_responses(
    state: &AppState,
    provider: &UpstreamProvider,
    claude_req: &ClaudeRequest,
    headers: &axum::http::HeaderMap,
    llm_trace_id: Option<&str>,
) -> Response {
    use crate::proxy::translator;
    use crate::proxy::translator::format::Format;
    use bytes::{Bytes, BytesMut};
    use futures::StreamExt;

    let mapped_model = crate::proxy::providers::router::map_model_for_provider(
        &claude_req.model,
        &provider.model_mapping,
    );
    let original_request = match serde_json::to_vec(claude_req) {
        Ok(body) => body,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to serialize Claude request: {error}"),
            )
                .into_response();
        }
    };

    let mut responses_body = translator::translate_request(
        Format::Claude,
        Format::OpenAIResponses,
        &mapped_model,
        &original_request,
        claude_req.stream,
    );
    if let Ok(mut body) = serde_json::from_slice::<Value>(&responses_body) {
        if let Some(object) = body.as_object_mut() {
            object.insert("model".to_string(), Value::String(mapped_model.clone()));
            object.insert("stream".to_string(), Value::Bool(claude_req.stream));

            // The public Responses API uses max_output_tokens, while the
            // incoming Claude request uses max_tokens.  The Codex auth-file
            // bridge intentionally omits this field, but OpenCode Go's
            // Responses endpoint accepts the public form.
            if let Some(max_tokens) = object
                .get("max_tokens")
                .cloned()
                .or_else(|| serde_json::to_value(claude_req.max_tokens).ok())
            {
                if !max_tokens.is_null() && !object.contains_key("max_output_tokens") {
                    object.insert("max_output_tokens".to_string(), max_tokens);
                }
            }
        }
        if let Ok(serialized) = serde_json::to_vec(&body) {
            responses_body = serialized;
        }
    }

    let url = match build_openai_compat_url_for_provider(&provider.base_url, "/v1/responses") {
        Ok(url) => url,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build URL: {error}"),
            )
                .into_response();
        }
    };
    let timeout_secs = provider
        .request_timeout_secs
        .unwrap_or(state.request_timeout.max(5));
    let upstream_proxy = state.upstream_proxy.read().await.clone();
    let client = match build_openai_compat_client_for_provider(Some(upstream_proxy), timeout_secs) {
        Ok(client) => client,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build client: {error}"),
            )
                .into_response();
        }
    };

    if let Some(trace_id) = llm_trace_id {
        state
            .upstream_trace_cache
            .put(
                trace_id,
                crate::proxy::upstream_trace::UpstreamTrace {
                    request_body: String::from_utf8(responses_body.clone()).ok(),
                    response_body: None,
                },
            )
            .await;
    }

    let mut request_headers = axum::http::HeaderMap::new();
    request_headers.insert("content-type", HeaderValue::from_static("application/json"));
    request_headers.insert(
        "accept",
        if claude_req.stream {
            HeaderValue::from_static("text/event-stream")
        } else {
            HeaderValue::from_static("application/json")
        },
    );
    for (key, value) in headers {
        match key.as_str().to_ascii_lowercase().as_str() {
            "user-agent" | "accept-encoding" => {
                request_headers.insert(key.clone(), value.clone());
            }
            _ => {}
        }
    }
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", provider.api_key)) {
        request_headers.insert("authorization", value);
    }

    let response = match client
        .post(&url)
        .headers(request_headers)
        .body(responses_body.clone())
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("OpenAI Responses upstream request failed: {error}"),
            )
                .into_response();
        }
    };

    let status = response.status();
    let content_type = response.headers().get("content-type").cloned();
    if !status.is_success() {
        let error_body = response.bytes().await.unwrap_or_default();
        let error_text = String::from_utf8_lossy(&error_body).into_owned();
        if let Some(trace_id) = llm_trace_id {
            state
                .upstream_trace_cache
                .put(
                    trace_id,
                    crate::proxy::upstream_trace::UpstreamTrace {
                        request_body: None,
                        response_body: Some(error_text.clone()),
                    },
                )
                .await;
        }
        let mut output = Response::builder()
            .status(status)
            .header("x-provider-name", &provider.name)
            .header("x-upstream-protocol", "openai_responses")
            .header("x-upstream-model", &mapped_model)
            .header("x-upstream-url", &url);
        if let Some(content_type) = content_type {
            output = output.header("x-upstream-content-type", content_type);
        }
        return output
            .body(Body::from(error_text.clone()))
            .unwrap_or_else(|_| (status, error_text).into_response());
    }

    if claude_req.stream {
        let upstream_stream = crate::proxy::upstream_trace::capture_response_stream(
            Box::pin(response.bytes_stream()),
            state.upstream_trace_cache.clone(),
            llm_trace_id.map(str::to_owned),
        );
        let model_for_stream = mapped_model.clone();
        let original_for_stream = original_request.clone();
        let responses_for_stream = responses_body.clone();
        let state_opt = translator::global_registry()
            .read()
            .ok()
            .and_then(|registry| {
                registry.new_stream_state(Format::Claude, Format::OpenAIResponses)
            });
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(32);

        tokio::spawn(async move {
            let mut stream = upstream_stream;
            let mut buffer = BytesMut::new();
            let mut translator_state = state_opt.unwrap_or_else(|| Box::new(()));

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        let _ = tx.send(Err(std::io::Error::other(error))).await;
                        return;
                    }
                };
                buffer.extend_from_slice(&chunk);

                for output in translate_responses_sse_events(
                    &mut buffer,
                    &model_for_stream,
                    &original_for_stream,
                    &responses_for_stream,
                    translator_state.as_mut(),
                ) {
                    if tx.send(Ok(Bytes::from(output))).await.is_err() {
                        return;
                    }
                }
            }

            if !buffer.is_empty() {
                buffer.extend_from_slice(b"\n\n");
                for output in translate_responses_sse_events(
                    &mut buffer,
                    &model_for_stream,
                    &original_for_stream,
                    &responses_for_stream,
                    translator_state.as_mut(),
                ) {
                    if tx.send(Ok(Bytes::from(output))).await.is_err() {
                        return;
                    }
                }
            }
        });

        let translated_stream = tokio_stream::wrappers::ReceiverStream::new(rx)
            .map(|result| result.map_err(|error| std::io::Error::other(error.to_string())));
        let mut output = Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .header("connection", "keep-alive")
            .header("x-provider-name", &provider.name)
            .header("x-upstream-protocol", "openai_responses")
            .header("x-upstream-model", &mapped_model)
            .header("x-upstream-url", &url);
        if let Some(content_type) = content_type {
            output = output.header("x-upstream-content-type", content_type);
        }
        return output
            .body(Body::from_stream(translated_stream))
            .unwrap_or_else(|_| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to build response",
                )
                    .into_response()
            });
    }

    let upstream_body = match response.bytes().await {
        Ok(body) => body,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("Failed to read OpenAI Responses response: {error}"),
            )
                .into_response();
        }
    };
    if let Some(trace_id) = llm_trace_id {
        state
            .upstream_trace_cache
            .put(
                trace_id,
                crate::proxy::upstream_trace::UpstreamTrace {
                    request_body: None,
                    response_body: String::from_utf8(upstream_body.to_vec()).ok(),
                },
            )
            .await;
    }

    let mut translator_state = translator::global_registry()
        .read()
        .ok()
        .and_then(|registry| registry.new_stream_state(Format::Claude, Format::OpenAIResponses))
        .unwrap_or_else(|| Box::new(()));
    let claude_body = translator::translate_non_stream(
        Format::Claude,
        Format::OpenAIResponses,
        &mapped_model,
        &original_request,
        &responses_body,
        &upstream_body,
        translator_state.as_mut(),
    );
    let mut output = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .header("x-provider-name", &provider.name)
        .header("x-upstream-protocol", "openai_responses")
        .header("x-upstream-model", &mapped_model)
        .header("x-upstream-url", &url);
    if let Some(content_type) = content_type {
        output = output.header("x-upstream-content-type", content_type);
    }
    output.body(Body::from(claude_body)).unwrap_or_else(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to build response",
        )
            .into_response()
    })
}

fn translate_responses_sse_events(
    buffer: &mut bytes::BytesMut,
    model: &str,
    original_request: &[u8],
    responses_request: &[u8],
    translator_state: &mut (dyn std::any::Any + Send),
) -> Vec<Vec<u8>> {
    let mut outputs = Vec::new();
    while let Some((event_type, _event_id, data)) =
        crate::proxy::translator::sse::parse_sse_event(buffer)
    {
        let mut payload = Vec::with_capacity(data.len() + event_type.len() + 16);
        if !event_type.is_empty() {
            payload.extend_from_slice(b"event: ");
            payload.extend_from_slice(event_type.as_bytes());
            payload.push(b'\n');
        }
        payload.extend_from_slice(b"data: ");
        payload.extend_from_slice(&data);
        payload.extend_from_slice(b"\n\n");
        let translated = crate::proxy::translator::global_registry()
            .read()
            .ok()
            .map(|registry| {
                registry.translate_stream_chunk(
                    crate::proxy::translator::format::Format::Claude,
                    crate::proxy::translator::format::Format::OpenAIResponses,
                    model,
                    original_request,
                    responses_request,
                    &payload,
                    translator_state,
                )
            })
            .unwrap_or_else(|| vec![payload]);
        outputs.extend(translated);
    }
    outputs
}

/// Forward an already-normalized OpenAI Responses request to a public
/// Responses upstream.  This is used when the client itself calls
/// `/v1/responses`; unlike the native Codex bridge, no Codex-only headers or
/// request sanitization are applied.
pub async fn forward_openai_responses_with_provider(
    state: &AppState,
    provider: &UpstreamProvider,
    body: &Value,
    headers: &axum::http::HeaderMap,
    llm_trace_id: Option<&str>,
) -> Response {
    use bytes::Bytes;
    use futures::StreamExt;

    let body_bytes = match serde_json::to_vec(body) {
        Ok(body) => body,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                format!("Invalid OpenAI Responses request: {error}"),
            )
                .into_response();
        }
    };
    let url = match build_openai_compat_url_for_provider(&provider.base_url, "/v1/responses") {
        Ok(url) => url,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build URL: {error}"),
            )
                .into_response();
        }
    };
    let timeout_secs = provider
        .request_timeout_secs
        .unwrap_or(state.request_timeout.max(5));
    let upstream_proxy = state.upstream_proxy.read().await.clone();
    let client = match build_openai_compat_client_for_provider(Some(upstream_proxy), timeout_secs) {
        Ok(client) => client,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build client: {error}"),
            )
                .into_response();
        }
    };

    if let Some(trace_id) = llm_trace_id {
        state
            .upstream_trace_cache
            .put(
                trace_id,
                crate::proxy::upstream_trace::UpstreamTrace {
                    request_body: String::from_utf8(body_bytes.clone()).ok(),
                    response_body: None,
                },
            )
            .await;
    }

    let stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    let mut request_headers = axum::http::HeaderMap::new();
    request_headers.insert("content-type", HeaderValue::from_static("application/json"));
    request_headers.insert(
        "accept",
        if stream {
            HeaderValue::from_static("text/event-stream")
        } else {
            HeaderValue::from_static("application/json")
        },
    );
    for (key, value) in headers {
        match key.as_str().to_ascii_lowercase().as_str() {
            "user-agent" | "accept-encoding" => {
                request_headers.insert(key.clone(), value.clone());
            }
            _ => {}
        }
    }
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {}", provider.api_key)) {
        request_headers.insert("authorization", value);
    }

    let response = match client
        .post(&url)
        .headers(request_headers)
        .body(body_bytes)
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("OpenAI Responses upstream request failed: {error}"),
            )
                .into_response();
        }
    };

    let status = response.status();
    let content_type = response.headers().get("content-type").cloned();
    if !status.is_success() {
        let error_body = response.bytes().await.unwrap_or_default();
        let error_text = String::from_utf8_lossy(&error_body).into_owned();
        if let Some(trace_id) = llm_trace_id {
            state
                .upstream_trace_cache
                .put(
                    trace_id,
                    crate::proxy::upstream_trace::UpstreamTrace {
                        request_body: None,
                        response_body: Some(error_text.clone()),
                    },
                )
                .await;
        }
        let mut output = Response::builder()
            .status(status)
            .header("x-provider-name", &provider.name)
            .header("x-upstream-protocol", "openai_responses")
            .header(
                "x-upstream-model",
                body.get("model")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .header("x-upstream-url", &url);
        if let Some(content_type) = content_type {
            output = output.header("x-upstream-content-type", content_type);
        }
        return output
            .body(Body::from(error_text.clone()))
            .unwrap_or_else(|_| (status, error_text).into_response());
    }

    let mut output = Response::builder()
        .status(StatusCode::OK)
        .header("x-provider-name", &provider.name)
        .header("x-upstream-protocol", "openai_responses")
        .header(
            "x-upstream-model",
            body.get("model")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .header("x-upstream-url", &url);
    if let Some(content_type) = content_type {
        output = output.header("content-type", content_type);
    }

    if stream {
        let upstream_stream = crate::proxy::upstream_trace::capture_response_stream(
            Box::pin(response.bytes_stream()),
            state.upstream_trace_cache.clone(),
            llm_trace_id.map(str::to_owned),
        );
        let provider_name_for_stream = provider.name.clone();
        let stream = upstream_stream.scan(false, move |errored, chunk| {
            if *errored {
                return std::future::ready(None);
            }

            let result = match chunk {
                Ok(bytes) => Ok::<Bytes, std::io::Error>(bytes),
                Err(error) => {
                    *errored = true;
                    tracing::error!(
                        provider = %provider_name_for_stream,
                        "OpenAI Responses upstream stream error: {}",
                        error
                    );
                    Ok(crate::proxy::stream_error::responses_sse_error(&error))
                }
            };
            std::future::ready(Some(result))
        });
        output.body(Body::from_stream(stream)).unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build response",
            )
                .into_response()
        })
    } else {
        let response_body = match response.bytes().await {
            Ok(body) => body,
            Err(error) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("Failed to read OpenAI Responses response: {error}"),
                )
                    .into_response();
            }
        };
        if let Some(trace_id) = llm_trace_id {
            state
                .upstream_trace_cache
                .put(
                    trace_id,
                    crate::proxy::upstream_trace::UpstreamTrace {
                        request_body: None,
                        response_body: String::from_utf8(response_body.to_vec()).ok(),
                    },
                )
                .await;
        }
        output.body(Body::from(response_body)).unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build response",
            )
                .into_response()
        })
    }
}

/// Forward a Claude Messages request through an OpenAI-Compatible provider.
/// Converts Claude → OpenAI request, then OpenAI response → Claude SSE/JSON.
pub async fn forward_claude_via_openai_compat(
    state: &AppState,
    provider: &UpstreamProvider,
    claude_req: &ClaudeRequest,
    headers: &axum::http::HeaderMap,
    llm_trace_id: Option<&str>,
) -> Response {
    use crate::proxy::mappers::claude::request_bridge::claude_to_openai_body;
    use crate::proxy::mappers::claude::response_bridge::{
        create_claude_sse_from_openai_stream, openai_to_claude_response,
    };

    let trace_id = format!("req_{}", chrono::Utc::now().timestamp_subsec_millis());
    let _trace_id = &trace_id;

    // Map model name
    let mapped_model = crate::proxy::providers::router::map_model_for_provider(
        &claude_req.model,
        &provider.model_mapping,
    );

    if uses_responses_endpoint(&mapped_model) {
        return forward_claude_via_openai_responses(
            state,
            provider,
            claude_req,
            headers,
            llm_trace_id,
        )
        .await;
    }

    // Convert Claude request to OpenAI format
    let openai_body = claude_to_openai_body(claude_req);
    let client_wants_stream = claude_req.stream;

    let url = match build_openai_compat_url_for_provider(&provider.base_url, "/v1/chat/completions")
    {
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

    // Build request headers
    let mut req_headers = axum::http::HeaderMap::new();
    req_headers.insert("content-type", HeaderValue::from_static("application/json"));
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
        let error_body = String::from_utf8_lossy(&error_body).into_owned();
        if let Some(trace_id) = llm_trace_id {
            state
                .upstream_trace_cache
                .put(
                    trace_id,
                    crate::proxy::upstream_trace::UpstreamTrace {
                        request_body: None,
                        response_body: Some(error_body.clone()),
                    },
                )
                .await;
        }
        return (status, error_body).into_response();
    }

    if client_wants_stream {
        // Stream: wrap OpenAI SSE through Claude SSE converter
        let openai_stream = crate::proxy::upstream_trace::capture_response_stream(
            Box::pin(response.bytes_stream()),
            state.upstream_trace_cache.clone(),
            llm_trace_id.map(str::to_owned),
        );
        let claude_stream =
            create_claude_sse_from_openai_stream(openai_stream, mapped_model.clone());

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

        // Write response body to upstream trace cache for non-streaming requests
        if let Some(trace_id) = llm_trace_id {
            let response_body_str = Some(String::from_utf8_lossy(&bytes).into_owned());
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

/// 带 translator 的 Claude → OpenAI 兼容转发函数。
///
/// 使用 translator 框架进行 Claude ↔ OpenAI 格式转换，而非旧版 mapper。
pub async fn forward_claude_via_openai_compat_with_translator(
    state: &AppState,
    provider: &UpstreamProvider,
    claude_req: &ClaudeRequest,
    headers: &axum::http::HeaderMap,
    llm_trace_id: Option<&str>,
) -> Response {
    use crate::proxy::translator;
    use crate::proxy::translator::format::Format;
    use axum::body::Body;
    use bytes::BytesMut;
    use futures::StreamExt;

    // Map model name
    let mapped_model = crate::proxy::providers::router::map_model_for_provider(
        &claude_req.model,
        &provider.model_mapping,
    );

    if uses_responses_endpoint(&mapped_model) {
        return forward_claude_via_openai_responses(
            state,
            provider,
            claude_req,
            headers,
            llm_trace_id,
        )
        .await;
    }

    // 序列化 Claude 请求
    let claude_bytes = match serde_json::to_vec(claude_req) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to serialize Claude request: {}", e),
            )
                .into_response();
        }
    };

    // 使用 translator 将 Claude 请求转为 OpenAI 格式
    let openai_bytes = translator::translate_request(
        Format::Claude,
        Format::OpenAI,
        &mapped_model,
        &claude_bytes,
        claude_req.stream,
    );

    let client_wants_stream = claude_req.stream;

    // 构建请求 URL
    let url = match build_openai_compat_url_for_provider(&provider.base_url, "/v1/chat/completions")
    {
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

    // 构建请求 headers
    let mut req_headers = axum::http::HeaderMap::new();
    req_headers.insert("content-type", HeaderValue::from_static("application/json"));
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

    // Write request body to upstream trace cache for traffic logging
    if let Some(trace_id) = llm_trace_id {
        let request_body_str = String::from_utf8(openai_bytes.clone()).ok();
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
        .body(openai_bytes.clone())
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
        let error_body = String::from_utf8_lossy(&error_body).into_owned();
        if let Some(trace_id) = llm_trace_id {
            state
                .upstream_trace_cache
                .put(
                    trace_id,
                    crate::proxy::upstream_trace::UpstreamTrace {
                        request_body: None,
                        response_body: Some(error_body.clone()),
                    },
                )
                .await;
        }
        return (status, error_body).into_response();
    }

    if client_wants_stream {
        // 流式响应：使用 translator 将 OpenAI SSE 转为 Claude SSE
        use tokio::sync::mpsc;

        let openai_stream = crate::proxy::upstream_trace::capture_response_stream(
            Box::pin(response.bytes_stream()),
            state.upstream_trace_cache.clone(),
            llm_trace_id.map(str::to_owned),
        );
        let model_for_sse = mapped_model.clone();
        let provider_name = provider.name.clone();
        let claude_bytes_for_stream = claude_bytes.clone();
        let openai_bytes_for_stream = openai_bytes.clone();

        // 创建流状态
        let state_opt = translator::global_registry()
            .read()
            .unwrap()
            .new_stream_state(Format::OpenAI, Format::Claude);

        let (tx, rx) = mpsc::channel::<Result<axum::body::Bytes, std::io::Error>>(32);

        // 在 tokio task 中处理流转换
        tokio::spawn(async move {
            use futures::StreamExt;

            let mut buf = BytesMut::new();
            let mut state = state_opt.unwrap_or_else(|| Box::new(()));
            let mut stream = openai_stream;

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(Err(std::io::Error::other(e))).await;
                        break;
                    }
                };

                buf.extend_from_slice(&chunk);

                // 解析 SSE 事件并转换
                while let Some((_event_type, _event_id, data)) =
                    translator::sse::parse_sse_event(&mut buf)
                {
                    let chunks = translator::global_registry()
                        .read()
                        .unwrap()
                        .translate_stream_chunk(
                            Format::OpenAI,
                            Format::Claude,
                            &model_for_sse,
                            &claude_bytes_for_stream,
                            &openai_bytes_for_stream,
                            &data,
                            state.as_mut(),
                        );

                    for chunk in chunks {
                        if tx.send(Ok(axum::body::Bytes::from(chunk))).await.is_err() {
                            return;
                        }
                    }
                }
            }
        });

        // 使用 tokio_stream 将 rx 转换为流
        let provider_name_for_error = provider_name.clone();
        let translated_stream =
            tokio_stream::wrappers::ReceiverStream::new(rx).map(move |r| match r {
                Ok(b) => Ok::<axum::body::Bytes, std::io::Error>(b),
                Err(e) => {
                    tracing::error!(
                        provider = %provider_name_for_error,
                        "Claude translation stream error: {}",
                        e
                    );
                    Ok(crate::proxy::stream_error::anthropic_sse_error(&e))
                }
            });

        Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .header("connection", "keep-alive")
            .header("x-mapped-model", &mapped_model)
            .header("x-provider-name", &provider_name)
            .body(Body::from_stream(translated_stream))
            .unwrap()
    } else {
        // 非流式响应：使用 translator 转换
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

        // 创建流状态（即使非流也需要 state 对象）
        let translator_state_opt = translator::global_registry()
            .read()
            .unwrap()
            .new_stream_state(Format::OpenAI, Format::Claude);

        let mut translator_state = translator_state_opt.unwrap_or_else(|| Box::new(()));

        let claude_resp = translator::translate_non_stream(
            Format::OpenAI,
            Format::Claude,
            &mapped_model,
            &claude_bytes,
            &openai_bytes,
            &bytes,
            translator_state.as_mut(),
        );

        // 尝试解析为 JSON Value 以便序列化
        let claude_value: serde_json::Value = match serde_json::from_slice(&claude_resp) {
            Ok(v) => v,
            Err(_) => {
                // 如果解析失败，返回原始响应
                return Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "application/json")
                    .header("x-mapped-model", &mapped_model)
                    .header("x-provider-name", &provider.name)
                    .body(Body::from(claude_resp))
                    .unwrap();
            }
        };

        // Write response body to upstream trace cache for non-streaming requests
        if let Some(trace_id) = llm_trace_id {
            let response_body_str = Some(String::from_utf8_lossy(&bytes).into_owned());
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

        match serde_json::to_string(&claude_value) {
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

#[cfg(test)]
mod tests {
    use super::{build_openai_compat_url_for_provider, uses_responses_endpoint};

    #[test]
    fn routes_open_code_go_responses_models_to_responses_endpoint() {
        assert!(uses_responses_endpoint("gpt-5.6-luna"));
        assert!(uses_responses_endpoint("grok-4.5"));
        assert!(!uses_responses_endpoint("glm-5.3"));
    }

    #[test]
    fn derives_sibling_endpoint_from_a_complete_provider_url() {
        assert_eq!(
            build_openai_compat_url_for_provider(
                "https://opencode.ai/zen/go/v1/responses",
                "/v1/responses"
            )
            .unwrap(),
            "https://opencode.ai/zen/go/v1/responses"
        );
        assert_eq!(
            build_openai_compat_url_for_provider(
                "https://opencode.ai/zen/go/v1/responses",
                "/v1/chat/completions"
            )
            .unwrap(),
            "https://opencode.ai/zen/go/v1/chat/completions"
        );
    }
}
