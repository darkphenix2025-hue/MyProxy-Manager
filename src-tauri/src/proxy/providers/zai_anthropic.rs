use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use serde_json::Value;
use tokio::time::Duration;

use crate::proxy::server::AppState;

fn map_model_for_zai(original: &str, state: &crate::proxy::ZaiConfig) -> String {
    let m = original.to_lowercase();
    if let Some(mapped) = state.model_mapping.get(original) {
        return mapped.clone();
    }
    if let Some(mapped) = state.model_mapping.get(&m) {
        return mapped.clone();
    }
    if m.starts_with("zai:") {
        return original[4..].to_string();
    }
    original.to_string()
}

pub fn join_base_url(base: &str, path: &str) -> Result<String, String> {
    let base = base.trim_end_matches('/');
    // If path is empty, just return the base URL as-is (caller already built full URL)
    if path.is_empty() {
        return Ok(base.to_string());
    }
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    Ok(format!("{}{}", base, path))
}

pub fn build_client(
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
        .tcp_nodelay(true) // [FIX #307] Disable Nagle's algorithm to improve latency for small requests
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))
}

pub fn copy_passthrough_headers(incoming: &HeaderMap) -> HeaderMap {
    // Only forward a conservative set of headers to avoid leaking the local proxy key or cookies.
    let mut out = HeaderMap::new();

    for (k, v) in incoming.iter() {
        let key = k.as_str().to_ascii_lowercase();
        match key.as_str() {
            "content-type" | "accept" | "anthropic-version" | "user-agent" => {
                out.insert(k.clone(), v.clone());
            }
            // [FIX] Forward anthropic-* headers (e.g. anthropic-beta: thinking-enabled)
            // Without these, upstream providers may strip thinking blocks while still
            // seeing the thinking config, causing "content[].thinking" errors.
            h if h.starts_with("anthropic-") => {
                out.insert(k.clone(), v.clone());
            }
            // Some clients use these for streaming; safe to pass through.
            "accept-encoding" | "cache-control" => {
                out.insert(k.clone(), v.clone());
            }
            _ => {}
        }
    }

    out
}

fn set_zai_auth(headers: &mut HeaderMap, incoming: &HeaderMap, api_key: &str) {
    // Prefer to keep the same auth scheme as the incoming request:
    // - If the client used x-api-key (Anthropic style), replace it.
    // - Else if it used Authorization, replace it with Bearer.
    // - Else default to x-api-key.
    let has_x_api_key = incoming.contains_key("x-api-key");
    let has_auth = incoming.contains_key(header::AUTHORIZATION);

    if has_x_api_key || !has_auth {
        if let Ok(v) = HeaderValue::from_str(api_key) {
            headers.insert("x-api-key", v);
        }
    }

    if has_auth {
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", api_key)) {
            headers.insert(header::AUTHORIZATION, v);
        }
    }
}

/// Recursively remove cache_control from all nested objects/arrays
/// [FIX #290] This is a defensive fix that works regardless of serde annotations
pub fn deep_remove_cache_control(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(v) = map.remove("cache_control") {
                tracing::info!(
                    "[ISSUE-744] Deep Cleaning found nested cache_control: {:?}",
                    v
                );
            }
            for v in map.values_mut() {
                deep_remove_cache_control(v);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                deep_remove_cache_control(v);
            }
        }
        _ => {}
    }
}

/// [FIX] Ensure tool_result blocks have an 'id' field matching 'tool_use_id'.
/// Some Anthropic-compatible providers (e.g. BIGMODEL) expect tool_result to have
/// an 'id' field, but the standard Claude API only has 'tool_use_id'.
pub fn ensure_tool_result_id(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(type_val) = map.get("type") {
                if type_val.as_str() == Some("tool_result") {
                    if let Some(tool_use_id) = map.get("tool_use_id") {
                        if !map.contains_key("id") {
                            map.insert("id".to_string(), tool_use_id.clone());
                        }
                    }
                }
            }
            for v in map.values_mut() {
                ensure_tool_result_id(v);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                ensure_tool_result_id(v);
            }
        }
        _ => {}
    }
}

pub async fn forward_anthropic_json(
    state: &AppState,
    method: Method,
    path: &str,
    incoming_headers: &HeaderMap,
    mut body: Value,
    message_count: usize, // [NEW v4.0.0] Pass message count for rewind detection
    trace_id: Option<&str>, // [LLM Logging]
) -> Response {
    let trace_id = trace_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()[..8].to_string());
    let zai = state.zai.read().await.clone();
    if !zai.enabled || zai.dispatch_mode == crate::proxy::ZaiDispatchMode::Off {
        return (StatusCode::BAD_REQUEST, "z.ai is disabled").into_response();
    }

    if zai.api_key.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "z.ai api_key is not set").into_response();
    }

    if let Some(model) = body.get("model").and_then(|v| v.as_str()) {
        let mapped = map_model_for_zai(model, &zai);
        body["model"] = Value::String(mapped.clone());

        // [FIX] Caching for z.ai (to support thinking-filter)
        if let Some(sig) = body
            .get("thinking")
            .and_then(|t| t.get("signature"))
            .and_then(|s| s.as_str())
        {
            crate::proxy::SignatureCache::global().cache_session_signature(
                "zai-session",
                sig.to_string(),
                message_count,
            );
            crate::proxy::SignatureCache::global().cache_thinking_family(sig.to_string(), mapped);
        }
    }

    let url = match join_base_url(&zai.base_url, path) {
        Ok(u) => u,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    let timeout_secs = state.request_timeout.max(5);
    let upstream_proxy = state.upstream_proxy.read().await.clone();
    let client = match build_client(Some(upstream_proxy), timeout_secs) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };

    let mut headers = copy_passthrough_headers(incoming_headers);
    set_zai_auth(&mut headers, incoming_headers, &zai.api_key);

    // Ensure JSON content type.
    headers
        .entry(header::CONTENT_TYPE)
        .or_insert(HeaderValue::from_static("application/json"));

    // [FIX #290] Clean cache_control before sending to Anthropic API
    // This prevents "Extra inputs are not permitted" errors
    deep_remove_cache_control(&mut body);

    // [FIX] Add 'id' to tool_result blocks for providers that require it
    ensure_tool_result_id(&mut body);

    // Remove non-standard fields that some providers (e.g. Aliyun/DashScope) reject
    for field in ["metadata", "context_management", "output_config"] {
        if body.get(field).is_some() {
            tracing::info!("[ZAI] Stripping non-standard field: {}", field);
            body.as_object_mut().map(|m| m.remove(field));
        }
    }

    // [FIX #307] Explicitly serialize body to Vec<u8> to ensure Content-Length is set correctly.
    // This avoids "Transfer-Encoding: chunked" for small bodies which caused connection errors.
    let body_bytes = serde_json::to_vec(&body).unwrap_or_default();
    let body_len = body_bytes.len();

    // [LLM Logging] Log upstream request
    let upstream_model = body
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let upstream_model_for_stream = upstream_model.clone();
    // [Upstream Trace Cache] Store request body for traffic log detail modal
    let body_for_trace = body.clone();
    let state_ref = state.clone();
    let trace_id_cache = trace_id.clone();
    tokio::spawn(async move {
        state_ref
            .upstream_trace_cache
            .put(
                &trace_id_cache,
                crate::proxy::upstream_trace::UpstreamTrace {
                    request_body: Some(serde_json::to_string(&body_for_trace).unwrap_or_default()),
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
            method: method.to_string(),
            url: url.clone(),
            model: upstream_model,
            status: None,
            content_type: Some("application/json".to_string()),
            body_size_bytes: body_len,
            body: crate::proxy::llm_logger::LlmBody::Json(body.clone()),
        };
        crate::proxy::llm_logger::log_entry(&entry);
    }

    tracing::debug!(
        "Forwarding request to z.ai (len: {} bytes): {}",
        body_len,
        url
    );

    let req = client
        .request(method.clone(), &url)
        .headers(headers)
        .body(body_bytes); // Use .body(Vec<u8>) instead of .json()

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("Upstream request failed: {}", e),
            )
                .into_response();
        }
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    let mut out = Response::builder().status(status);
    if let Some(ct) = resp.headers().get(header::CONTENT_TYPE) {
        out = out.header(header::CONTENT_TYPE, ct.clone());
    }
    out = out.header("X-Upstream-Path", path);

    // [LLM Logging] Wrap stream to capture upstream response bytes for logging
    let trace_id_owned = trace_id.clone();
    let state_for_response = state.clone();
    let upstream_model_owned = upstream_model_for_stream;
    let url_owned = url.clone();
    let method_owned = method.to_string();
    let wrapped_stream = async_stream::stream! {
        let mut collected: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            if let Ok(b) = &chunk {
                collected.extend_from_slice(b);
            }
            yield chunk;
        }
        // Log upstream response after stream is complete
        let raw_text = String::from_utf8_lossy(&collected).to_string();
        let body = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw_text) {
            crate::proxy::llm_logger::LlmBody::Json(json)
        } else {
            crate::proxy::llm_logger::LlmBody::Text(raw_text.clone())
        };
        let entry = crate::proxy::llm_logger::LlmLogEntry {
            trace_id: trace_id_owned.clone(),
            timestamp: crate::proxy::llm_logger::now_timestamp(),
            stage: "upstream_response",
            method: method_owned,
            url: url_owned,
            model: upstream_model_owned,
            status: Some(status.as_u16()),
            content_type: None,
            body_size_bytes: collected.len(),
            body,
        };
        crate::proxy::llm_logger::log_entry(&entry);
        // [Upstream Trace Cache] Store response body for traffic log detail modal
        let state_for_cache = state_for_response;
        tokio::spawn(async move {
            state_for_cache.upstream_trace_cache.put(&trace_id_owned, crate::proxy::upstream_trace::UpstreamTrace {
                request_body: None,
                response_body: Some(raw_text),
            }).await;
        });
    };

    out.body(Body::from_stream(wrapped_stream))
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build response",
            )
                .into_response()
        })
}

/// Generic Anthropic-compatible provider forwarding.
/// Used by the multi-provider API gateway to forward requests to any configured provider.
pub async fn forward_anthropic_with_provider(
    state: &AppState,
    base_url: &str,
    api_key: &str,
    model_mapping: &std::collections::HashMap<String, String>,
    method: Method,
    path: &str,
    incoming_headers: &HeaderMap,
    mut body: Value,
    message_count: usize,
    provider_name: &str,
    trace_id: Option<&str>,           // [LLM Logging]
    route_mapped_model: Option<&str>, // Route-mapped model with provider_id prefix
) -> Response {
    let trace_id = trace_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()[..8].to_string());
    if api_key.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "Provider api_key is not set").into_response();
    }

    // Model mapping — track the mapped model name for logging
    let mut mapped_model_name: Option<String> = None;
    if let Some(model) = body.get("model").and_then(|v| v.as_str()) {
        // `route_mapped_model` is the result of the first routing phase. When
        // it is `provider_id/model`, the router has already selected the
        // provider and the model is provider-native; applying model_mapping
        // here would incorrectly perform a third-party rewrite (e.g. to
        // qwen) and violate the explicit route selected by the user.
        let mapped = if super::router::preserve_explicit_route_model(route_mapped_model, model) {
            model.to_string()
        } else {
            super::router::map_model_for_provider(model, model_mapping)
        };
        mapped_model_name = Some(mapped.clone());
        body["model"] = Value::String(mapped);

        // Signature caching
        if let Some(sig) = body
            .get("thinking")
            .and_then(|t| t.get("signature"))
            .and_then(|s| s.as_str())
        {
            crate::proxy::SignatureCache::global().cache_session_signature(
                "provider-session",
                sig.to_string(),
                message_count,
            );
            crate::proxy::SignatureCache::global().cache_thinking_family(
                sig.to_string(),
                mapped_model_name.as_ref().unwrap().clone(),
            );
        }
    }

    let url = match join_base_url(base_url, path) {
        Ok(u) => u,
        Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
    };

    let timeout_secs = state.request_timeout.max(5);
    let upstream_proxy = state.upstream_proxy.read().await.clone();
    let client = match build_client(Some(upstream_proxy), timeout_secs) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    };

    let mut headers = copy_passthrough_headers(incoming_headers);
    // Set auth using the provider's API key
    set_zai_auth(&mut headers, incoming_headers, api_key);

    // Ensure JSON content type
    headers
        .entry(header::CONTENT_TYPE)
        .or_insert(HeaderValue::from_static("application/json"));

    deep_remove_cache_control(&mut body);

    // [FIX] Add 'id' to tool_result blocks for providers that require it
    ensure_tool_result_id(&mut body);

    // Remove non-standard fields that some providers (e.g. Aliyun/DashScope) reject
    for field in ["metadata", "context_management", "output_config"] {
        if body.get(field).is_some() {
            tracing::info!("[Provider] Stripping non-standard field: {}", field);
            body.as_object_mut().map(|m| m.remove(field));
        }
    }

    // [DEBUG] Log thinking state before forwarding to verify fixes are applied
    let has_thinking_cfg = body.get("thinking").is_some();
    let assistant_msgs_without_thinking: Vec<_> = body
        .get("messages")
        .and_then(|m| m.as_array())
        .map(|msgs| {
            msgs.iter()
                .enumerate()
                .filter(|(_, m)| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
                .filter(|(_, m)| {
                    !m.get("content")
                        .and_then(|c| c.as_array())
                        .map(|blocks| {
                            blocks
                                .iter()
                                .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("thinking"))
                        })
                        .unwrap_or(false)
                })
                .map(|(i, _)| i)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    tracing::info!(
        "[DEBUG-FORWARD] provider={} thinking_cfg={} assistant_msgs_without_thinking={:?}",
        provider_name,
        has_thinking_cfg,
        assistant_msgs_without_thinking
    );

    let body_bytes = serde_json::to_vec(&body).unwrap_or_default();
    let body_len = body_bytes.len();

    // [Upstream Trace Cache] Store request body for traffic log detail modal
    let body_for_trace = body.clone();
    let state_ref = state.clone();
    let trace_id_cache = trace_id.clone();
    tokio::spawn(async move {
        state_ref
            .upstream_trace_cache
            .put(
                &trace_id_cache,
                crate::proxy::upstream_trace::UpstreamTrace {
                    request_body: Some(serde_json::to_string(&body_for_trace).unwrap_or_default()),
                    response_body: None,
                },
            )
            .await;
    });

    // [LLM Logging] Log upstream request
    {
        let entry = crate::proxy::llm_logger::LlmLogEntry {
            trace_id: trace_id.clone(),
            timestamp: crate::proxy::llm_logger::now_timestamp(),
            stage: "upstream_request",
            method: method.to_string(),
            url: url.clone(),
            model: mapped_model_name.clone(),
            status: None,
            content_type: Some("application/json".to_string()),
            body_size_bytes: body_len,
            body: crate::proxy::llm_logger::LlmBody::Json(body.clone()),
        };
        crate::proxy::llm_logger::log_entry(&entry);
    }

    tracing::debug!(
        "Forwarding request to provider (len: {} bytes): {}",
        body_len,
        url
    );

    let req = client
        .request(method.clone(), &url)
        .headers(headers)
        .body(body_bytes);

    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            // Include provider headers in error response so monitoring logs can track which provider failed
            let error_body = serde_json::json!({
                "error": { "message": format!("Upstream request failed: {}", e) }
            });
            let json_str = serde_json::to_string(&error_body).unwrap_or_default();
            let mut resp = Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .header("x-provider-name", provider_name)
                .header(header::CONTENT_TYPE, "application/json");
            let display_model = route_mapped_model
                .or(mapped_model_name.as_deref())
                .unwrap_or("");
            if !display_model.is_empty() {
                resp = resp.header("X-Mapped-Model", display_model);
                resp = resp.header(
                    "X-Upstream-Model",
                    mapped_model_name.as_deref().unwrap_or(display_model),
                );
                resp = resp.header("X-Upstream-URL", url.as_str());
            }
            resp = resp.header("X-Upstream-Path", path);
            return resp.body(Body::from(json_str)).unwrap_or_else(|_| {
                (StatusCode::BAD_GATEWAY, "Upstream request failed").into_response()
            });
        }
    };

    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

    let mut out = Response::builder().status(status);
    if let Some(ct) = resp.headers().get(header::CONTENT_TYPE) {
        out = out.header(header::CONTENT_TYPE, ct.clone());
    }
    out = out.header("x-provider-name", provider_name);
    let display_model = route_mapped_model
        .or(mapped_model_name.as_deref())
        .unwrap_or("");
    if !display_model.is_empty() {
        out = out.header("X-Mapped-Model", display_model);
        out = out.header(
            "X-Upstream-Model",
            mapped_model_name.as_deref().unwrap_or(display_model),
        );
    }
    out = out.header("X-Upstream-URL", url.as_str());
    out = out.header("X-Upstream-Path", path);

    // [LLM Logging] Wrap stream to capture upstream response bytes for logging
    let trace_id_owned = trace_id.clone();
    let state_for_response = state.clone();
    let method_owned = method.to_string();
    let url_owned = url.clone();
    let model_owned = mapped_model_name.clone();
    let wrapped_stream = async_stream::stream! {
        let mut collected: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            if let Ok(b) = &chunk {
                collected.extend_from_slice(b);
            }
            yield chunk;
        }
        let raw_text = String::from_utf8_lossy(&collected).to_string();
        let body = if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw_text) {
            crate::proxy::llm_logger::LlmBody::Json(json)
        } else {
            crate::proxy::llm_logger::LlmBody::Text(raw_text.clone())
        };
        let entry = crate::proxy::llm_logger::LlmLogEntry {
            trace_id: trace_id_owned.clone(),
            timestamp: crate::proxy::llm_logger::now_timestamp(),
            stage: "upstream_response",
            method: method_owned,
            url: url_owned,
            model: model_owned,
            status: Some(status.as_u16()),
            content_type: None,
            body_size_bytes: collected.len(),
            body,
        };
        crate::proxy::llm_logger::log_entry(&entry);
        // [Upstream Trace Cache] Store response body for traffic log detail modal
        let state_for_cache = state_for_response;
        tokio::spawn(async move {
            state_for_cache.upstream_trace_cache.put(&trace_id_owned, crate::proxy::upstream_trace::UpstreamTrace {
                request_body: None,
                response_body: Some(raw_text),
            }).await;
        });
    };

    out.body(Body::from_stream(wrapped_stream))
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build response",
            )
                .into_response()
        })
}
