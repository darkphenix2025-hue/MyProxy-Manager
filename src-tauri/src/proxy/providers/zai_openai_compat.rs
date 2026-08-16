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
        return (status, String::from_utf8_lossy(&error_body).to_string()).into_response();
    }

    if client_wants_stream {
        // Stream: wrap OpenAI SSE through Claude SSE converter
        let openai_stream = response.bytes_stream();
        let claude_stream =
            create_claude_sse_from_openai_stream(Box::pin(openai_stream), mapped_model.clone());

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
        return (status, String::from_utf8_lossy(&error_body).to_string()).into_response();
    }

    if client_wants_stream {
        // 流式响应：使用 translator 将 OpenAI SSE 转为 Claude SSE
        use tokio::sync::mpsc;

        let openai_stream = response.bytes_stream();
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
                        let _ = tx
                            .send(Err(std::io::Error::new(std::io::ErrorKind::Other, e)))
                            .await;
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
        let translated_stream = tokio_stream::wrappers::ReceiverStream::new(rx).map(|r| match r {
            Ok(b) => Ok::<axum::body::Bytes, std::io::Error>(b),
            Err(e) => Ok(axum::body::Bytes::from(format!("Stream error: {}", e))),
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
            let response_body_str = serde_json::to_string(&claude_value).ok();
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
