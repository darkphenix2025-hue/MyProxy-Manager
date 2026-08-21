//! Native Codex Responses upstream bridge.
//!
//! Claude Code speaks the Anthropic Messages protocol, while a Codex auth-file
//! account speaks the Responses protocol at `/backend-api/codex/responses`.
//! This module owns that boundary so a Codex route never falls through to the
//! legacy Google token pool.

use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::{Bytes, BytesMut};
use futures::StreamExt;
use serde_json::{json, Value};

use crate::proxy::config::UpstreamProvider;
use crate::proxy::mappers::claude::models::ClaudeRequest;
use crate::proxy::server::AppState;
use crate::proxy::translator;
use crate::proxy::translator::format::Format;

const CODEX_ORIGINATOR: &str = "codex_cli_rs";
const CODEX_USER_AGENT: &str = "codex_cli_rs/0.144.1 (MyProxy-Manager; auth-file)";

/// Remove fields accepted by the public Responses API but rejected by the
/// ChatGPT Codex backend, and add the backend's required compatibility flags.
pub fn sanitize_codex_responses_request(body: &mut Value) {
    let Some(object) = body.as_object_mut() else {
        return;
    };

    for field in [
        "max_output_tokens",
        "max_completion_tokens",
        "temperature",
        "top_p",
        "truncation",
        "user",
        "context_management",
    ] {
        object.remove(field);
    }

    object
        .entry("store".to_string())
        .or_insert(Value::Bool(false));
    object
        .entry("parallel_tool_calls".to_string())
        .or_insert(Value::Bool(true));
    object
        .entry("include".to_string())
        .or_insert_with(|| json!(["reasoning.encrypted_content"]));

    if let Some(Value::Array(items)) = object.get_mut("input") {
        for item in items {
            if let Some(item_object) = item.as_object_mut() {
                if item_object.get("role").and_then(Value::as_str) == Some("system") {
                    item_object.insert("role".to_string(), Value::String("developer".to_string()));
                }

                if item_object.get("type").and_then(Value::as_str) == Some("function_call") {
                    let current_id = item_object
                        .get("id")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    let has_valid_item_id = current_id
                        .as_deref()
                        .is_some_and(|id| id.starts_with("fc_"));
                    if !has_valid_item_id {
                        let source_id = item_object
                            .get("call_id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned)
                            .or(current_id)
                            .unwrap_or_else(|| "call_unknown".to_string());
                        let item_id = crate::proxy::translator::pairs::responses_to_openai::
                            codex_function_call_id(&source_id);
                        item_object.insert("id".to_string(), Value::String(item_id));
                    }
                }
            }
        }
    }
}

/// Forward a Claude Messages request to the native Codex Responses endpoint
/// and translate the Responses result back to Claude SSE/JSON.
pub async fn forward_claude_via_codex_responses(
    state: &AppState,
    provider: &UpstreamProvider,
    claude_req: &ClaudeRequest,
    claude_body: &Value,
    incoming_headers: &HeaderMap,
    llm_trace_id: Option<&str>,
) -> Response {
    let mapped_model = crate::proxy::providers::router::map_model_for_provider(
        &claude_req.model,
        &provider.model_mapping,
    );
    let original_request = match serde_json::to_vec(claude_body) {
        Ok(body) => body,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to serialize Claude request: {error}"),
            )
                .into_response();
        }
    };

    let mut codex_body = translator::translate_request(
        Format::Claude,
        Format::OpenAIResponses,
        &mapped_model,
        &original_request,
        claude_req.stream,
    );
    // The auth-file endpoint is stateful only on the client side. Explicitly
    // disable persistence so the request remains compatible with Codex CLI's
    // short-lived response flow.
    if let Ok(mut body) = serde_json::from_slice::<Value>(&codex_body) {
        sanitize_codex_responses_request(&mut body);
        if let Some(object) = body.as_object_mut() {
            object.insert("model".to_string(), Value::String(mapped_model.clone()));
            object.insert("stream".to_string(), Value::Bool(claude_req.stream));
            object
                .entry("store".to_string())
                .or_insert(Value::Bool(false));
        }
        if let Ok(serialized) = serde_json::to_vec(&body) {
            codex_body = serialized;
        }
    }

    let url = codex_responses_url(&provider.base_url);
    let timeout_secs = provider
        .request_timeout_secs
        .unwrap_or(state.request_timeout)
        .max(5);
    let upstream_proxy = state.upstream_proxy.read().await.clone();
    let mut client_builder = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .tcp_nodelay(true);
    if upstream_proxy.enabled && !upstream_proxy.url.trim().is_empty() {
        let proxy_url = crate::proxy::config::normalize_proxy_url(&upstream_proxy.url);
        let proxy = match reqwest::Proxy::all(&proxy_url) {
            Ok(proxy) => proxy,
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Invalid upstream proxy URL: {error}"),
                )
                    .into_response();
            }
        };
        client_builder = client_builder.proxy(proxy);
    }
    let client = match client_builder.build() {
        Ok(client) => client,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Build Codex client failed: {error}"),
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
                    request_body: String::from_utf8(codex_body.clone()).ok(),
                    response_body: None,
                },
            )
            .await;
    }

    let request_headers = build_codex_headers(
        incoming_headers,
        &provider.api_key,
        provider.account_id.as_deref(),
        claude_req.stream,
    );
    let response = match client
        .post(&url)
        .headers(request_headers)
        .body(codex_body.clone())
        .send()
        .await
    {
        Ok(response) => response,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("Codex upstream request failed: {error}"),
            )
                .into_response();
        }
    };

    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();

    if !status.is_success() {
        let body = response.bytes().await.unwrap_or_default();
        let body_text = String::from_utf8_lossy(&body).into_owned();
        if let Some(trace_id) = llm_trace_id {
            state
                .upstream_trace_cache
                .put(
                    trace_id,
                    crate::proxy::upstream_trace::UpstreamTrace {
                        request_body: None,
                        response_body: Some(body_text.clone()),
                    },
                )
                .await;
        }
        let mut output = Response::builder()
            .status(status)
            .header("x-provider-name", &provider.name)
            .header("x-upstream-protocol", "codex")
            .header("x-upstream-model", &mapped_model)
            .header("x-upstream-url", &url);
        if let Some(content_type) = content_type {
            output = output.header(header::CONTENT_TYPE, content_type);
        }
        return output
            .body(Body::from(body_text.clone()))
            .unwrap_or_else(|_| (status, body_text).into_response());
    }

    if claude_req.stream {
        return stream_codex_response(
            state,
            provider,
            mapped_model,
            original_request,
            codex_body,
            response,
            url,
            llm_trace_id,
        )
        .await;
    }

    let upstream_json = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(error) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("Failed to read Codex response: {error}"),
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
                    response_body: String::from_utf8(upstream_json.to_vec()).ok(),
                },
            )
            .await;
    }

    let mut translator_state = translator::global_registry()
        .read()
        .ok()
        .and_then(|registry| registry.new_stream_state(Format::Claude, Format::OpenAIResponses))
        .unwrap_or_else(|| Box::new(()));
    let claude_json = translator::translate_non_stream(
        Format::Claude,
        Format::OpenAIResponses,
        &mapped_model,
        &original_request,
        &codex_body,
        &upstream_json,
        translator_state.as_mut(),
    );

    let mut output = Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .header("x-provider-name", &provider.name)
        .header("x-upstream-protocol", "codex")
        .header("x-upstream-model", &mapped_model)
        .header("x-upstream-url", &url);
    if let Some(content_type) = content_type {
        // The client must receive Claude JSON, not the upstream Responses
        // content type. Keep this branch only to make the intent explicit.
        output = output.header("x-upstream-content-type", content_type);
    }
    output.body(Body::from(claude_json)).unwrap_or_else(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Failed to build response",
        )
            .into_response()
    })
}

async fn stream_codex_response(
    state: &AppState,
    provider: &UpstreamProvider,
    mapped_model: String,
    original_request: Vec<u8>,
    codex_body: Vec<u8>,
    response: reqwest::Response,
    url: String,
    llm_trace_id: Option<&str>,
) -> Response {
    let upstream_stream = crate::proxy::upstream_trace::capture_response_stream(
        Box::pin(response.bytes_stream()),
        state.upstream_trace_cache.clone(),
        llm_trace_id.map(str::to_owned),
    );
    let provider_name = provider.name.clone();
    let model_for_stream = mapped_model.clone();
    let state_opt = translator::global_registry()
        .read()
        .ok()
        .and_then(|registry| registry.new_stream_state(Format::Claude, Format::OpenAIResponses));
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

            for output in translate_codex_sse_events(
                &mut buffer,
                &model_for_stream,
                &original_request,
                &codex_body,
                translator_state.as_mut(),
            ) {
                if tx.send(Ok(Bytes::from(output))).await.is_err() {
                    return;
                }
            }
        }

        // Some upstream implementations close the connection immediately
        // after the final data line and omit the optional blank-line
        // delimiter. Flush that final event instead of silently dropping it.
        if !buffer.is_empty() {
            buffer.extend_from_slice(b"\n\n");
            for output in translate_codex_sse_events(
                &mut buffer,
                &model_for_stream,
                &original_request,
                &codex_body,
                translator_state.as_mut(),
            ) {
                if tx.send(Ok(Bytes::from(output))).await.is_err() {
                    return;
                }
            }
        }
    });

    let provider_name_for_error = provider_name.clone();
    let translated_stream =
        tokio_stream::wrappers::ReceiverStream::new(rx).map(move |result| match result {
            Ok(bytes) => Ok::<Bytes, std::io::Error>(bytes),
            Err(error) => {
                tracing::error!(
                    provider = %provider_name_for_error,
                    "Codex-to-Anthropic stream error: {}",
                    error
                );
                Ok(crate::proxy::stream_error::anthropic_sse_error(&error))
            }
        });
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .header("connection", "keep-alive")
        .header("x-provider-name", provider_name)
        .header("x-upstream-protocol", "codex")
        .header("x-upstream-model", mapped_model)
        .header("x-upstream-url", url)
        .body(Body::from_stream(translated_stream))
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build response",
            )
                .into_response()
        })
}

fn wrap_responses_sse_event(event_type: &str, data: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(data.len() + event_type.len() + 16);
    if !event_type.is_empty() {
        payload.extend_from_slice(b"event: ");
        payload.extend_from_slice(event_type.as_bytes());
        payload.push(b'\n');
    }
    payload.extend_from_slice(b"data: ");
    payload.extend_from_slice(data);
    payload.extend_from_slice(b"\n\n");
    payload
}

fn translate_codex_sse_events(
    buffer: &mut BytesMut,
    model: &str,
    original_request: &[u8],
    codex_body: &[u8],
    translator_state: &mut (dyn std::any::Any + Send),
) -> Vec<Vec<u8>> {
    let mut outputs = Vec::new();
    while let Some((event_type, _event_id, data)) = translator::sse::parse_sse_event(buffer) {
        let payload = wrap_responses_sse_event(&event_type, &data);
        let translated = translator::global_registry()
            .read()
            .ok()
            .map(|registry| {
                registry.translate_stream_chunk(
                    Format::Claude,
                    Format::OpenAIResponses,
                    model,
                    original_request,
                    codex_body,
                    &payload,
                    translator_state,
                )
            })
            .unwrap_or_else(|| vec![payload]);
        outputs.extend(translated);
    }
    outputs
}

fn codex_responses_url(base_url: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    if base_url.ends_with("/responses") {
        base_url.to_string()
    } else {
        format!("{base_url}/responses")
    }
}

fn build_codex_headers(
    incoming: &HeaderMap,
    api_key: &str,
    account_id: Option<&str>,
    stream: bool,
) -> HeaderMap {
    let mut output = HeaderMap::new();
    for (key, value) in incoming {
        match key.as_str().to_ascii_lowercase().as_str() {
            "accept"
            | "originator"
            | "user-agent"
            | "version"
            | "x-codex-beta-features"
            | "x-codex-turn-metadata"
            | "x-client-request-id"
            | "session_id"
            | "session-id"
            | "conversation_id"
            | "conversation-id"
            | "thread-id"
            | "x-codex-window-id" => {
                output.insert(key.clone(), value.clone());
            }
            _ => {}
        }
    }
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {api_key}")) {
        output.insert(header::AUTHORIZATION, value);
    }
    if let Some(account_id) = account_id {
        if let Ok(value) = HeaderValue::from_str(account_id) {
            output.insert("ChatGPT-Account-Id", value);
        }
    }
    output.insert("originator", HeaderValue::from_static(CODEX_ORIGINATOR));
    output.insert(
        header::USER_AGENT,
        HeaderValue::from_static(CODEX_USER_AGENT),
    );
    output.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    output.insert("OpenAI-Beta", HeaderValue::from_static("codex-1"));
    output.insert(
        header::ACCEPT,
        if stream {
            HeaderValue::from_static("text/event-stream")
        } else {
            HeaderValue::from_static("application/json")
        },
    );
    output
}

#[cfg(test)]
mod tests {
    use super::{
        build_codex_headers, codex_responses_url, sanitize_codex_responses_request,
        wrap_responses_sse_event,
    };
    use axum::http::{header, HeaderMap};
    use serde_json::json;

    #[test]
    fn appends_responses_once() {
        assert_eq!(
            codex_responses_url("https://chatgpt.com/backend-api/codex"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            codex_responses_url("https://chatgpt.com/backend-api/codex/responses"),
            "https://chatgpt.com/backend-api/codex/responses"
        );
    }

    #[test]
    fn replaces_client_auth_and_sets_codex_headers() {
        let mut incoming = HeaderMap::new();
        incoming.insert(header::AUTHORIZATION, "Bearer claude".parse().unwrap());
        let headers = build_codex_headers(&incoming, "access-token", Some("acct-1"), true);
        assert_eq!(
            headers.get(header::AUTHORIZATION).unwrap(),
            "Bearer access-token"
        );
        assert_eq!(headers.get("ChatGPT-Account-Id").unwrap(), "acct-1");
        assert_eq!(headers.get("originator").unwrap(), "codex_cli_rs");
        assert_eq!(headers.get(header::ACCEPT).unwrap(), "text/event-stream");
    }

    #[test]
    fn wraps_data_only_responses_event_for_translator() {
        let payload = wrap_responses_sse_event("", br#"{"type":"response.completed"}"#);
        assert_eq!(
            std::str::from_utf8(&payload).unwrap(),
            "data: {\"type\":\"response.completed\"}\n\n"
        );
    }

    #[test]
    fn removes_codex_unsupported_generation_parameters() {
        let mut body = json!({
            "model": "gpt-5.6-sol",
            "max_output_tokens": 1024,
            "max_completion_tokens": 1024,
            "temperature": 0.2,
            "top_p": 0.9,
            "truncation": "auto",
            "user": "claude-code",
            "context_management": {}
        });

        sanitize_codex_responses_request(&mut body);

        for field in [
            "max_output_tokens",
            "max_completion_tokens",
            "temperature",
            "top_p",
            "truncation",
            "user",
            "context_management",
        ] {
            assert!(body.get(field).is_none(), "field {field} should be removed");
        }
    }

    #[test]
    fn normalizes_function_call_item_id_without_changing_call_correlation() {
        let mut body = json!({
            "input": [{
                "type": "function_call",
                "id": "call_DklngvgrHQ59Pi2DrPTkiuGX",
                "call_id": "call_DklngvgrHQ59Pi2DrPTkiuGX",
                "name": "lookup",
                "arguments": "{}"
            }]
        });

        sanitize_codex_responses_request(&mut body);

        assert!(body["input"][0]["id"].as_str().unwrap().starts_with("fc_"));
        assert_eq!(body["input"][0]["call_id"], "call_DklngvgrHQ59Pi2DrPTkiuGX");
    }
}
