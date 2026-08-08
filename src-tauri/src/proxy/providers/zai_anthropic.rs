use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
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
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    Ok(format!("{}{}", base, path))
}

fn target_path_from_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let mut target = parsed.path().to_string();
    if target.is_empty() {
        target.push('/');
    }
    if let Some(query) = parsed.query() {
        target.push('?');
        target.push_str(query);
    }
    Some(target)
}

pub fn build_client(
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
                tracing::info!("[ISSUE-744] Deep Cleaning found nested cache_control: {:?}", v);
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

pub async fn forward_anthropic_json(
    state: &AppState,
    method: Method,
    path: &str,
    incoming_headers: &HeaderMap,
    mut body: Value,
    message_count: usize, // [NEW v4.0.0] Pass message count for rewind detection
) -> Response {
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
        if let Some(sig) = body.get("thinking").and_then(|t| t.get("signature")).and_then(|s| s.as_str()) {
            crate::proxy::SignatureCache::global().cache_session_signature(
                "zai-session",
                sig.to_string(),
                message_count
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
    if let Some(cc) = body.get("cache_control") {
        tracing::info!("[ISSUE-744] Deep cleaning cache_control from ROOT: {:?}", cc);
    }
    deep_remove_cache_control(&mut body);

    // [FIX #307] Explicitly serialize body to Vec<u8> to ensure Content-Length is set correctly.
    // This avoids "Transfer-Encoding: chunked" for small bodies which caused connection errors.
    let body_bytes = serde_json::to_vec(&body).unwrap_or_default();
    let body_len = body_bytes.len();
    
    tracing::debug!("Forwarding request to z.ai (len: {} bytes): {}", body_len, url);

    let req = client.request(method, &url)
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

    // Stream response body to the client (covers SSE and non-SSE).
    let stream = resp.bytes_stream().map(|chunk| match chunk {
        Ok(b) => Ok::<Bytes, std::io::Error>(b),
        Err(e) => Ok(Bytes::from(format!("Upstream stream error: {}", e))),
    });

    out.body(Body::from_stream(stream)).unwrap_or_else(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "Failed to build response").into_response()
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
    upstream_protocol: &str,
    llm_trace_id: Option<&str>,
) -> Response {
    if api_key.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "Provider api_key is not set").into_response();
    }

    // Model mapping — track the mapped model name for logging
    let mut mapped_model_name: Option<String> = None;
    if let Some(model) = body.get("model").and_then(|v| v.as_str()) {
        let mapped = super::router::map_model_for_provider(model, model_mapping);
        mapped_model_name = Some(mapped.clone());
        body["model"] = Value::String(mapped);

        // Signature caching
        if let Some(sig) = body.get("thinking").and_then(|t| t.get("signature")).and_then(|s| s.as_str()) {
            crate::proxy::SignatureCache::global().cache_session_signature(
                "provider-session",
                sig.to_string(),
                message_count
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

    let body_bytes = serde_json::to_vec(&body).unwrap_or_default();
    let body_len = body_bytes.len();

    // Write request body to upstream trace cache for traffic logging
    if let Some(trace_id) = llm_trace_id {
        let request_body_str = String::from_utf8(body_bytes.clone()).ok();
        state
            .upstream_trace_cache
            .put(
                trace_id,
                crate::proxy::upstream_trace::UpstreamTrace {
                    request_body: request_body_str,
                    response_body: None, // SSE stream — cannot collect full response
                },
            )
            .await;
    }

    tracing::debug!("Forwarding request to provider (len: {} bytes): {}", body_len, url);

    let req = client.request(method, &url)
        .headers(headers)
        .body(body_bytes);

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
    out = out.header("x-provider-name", provider_name);
    if let Some(ref mapped) = mapped_model_name {
        out = out.header("X-Mapped-Model", mapped.as_str());
    }
    // Add upstream protocol, model and URL headers for monitor middleware
    out = out.header("X-Upstream-Protocol", upstream_protocol);
    if let Some(ref mapped) = mapped_model_name {
        out = out.header("X-Upstream-Model", mapped.as_str());
    }
    // Report the path of the final URL that was actually requested. Using only
    // base_url omitted the header for origin-only provider URLs.
    if let Some(target_path) = target_path_from_url(&url) {
        out = out.header("X-Upstream-URL", target_path);
    }

    let stream = resp.bytes_stream().map(|chunk| match chunk {
        Ok(b) => Ok::<Bytes, std::io::Error>(b),
        Err(e) => Ok(Bytes::from(format!("Upstream stream error: {}", e))),
    });

    out.body(Body::from_stream(stream)).unwrap_or_else(|_| {
        (StatusCode::INTERNAL_SERVER_ERROR, "Failed to build response").into_response()
    })
}

#[cfg(test)]
mod tests {
    use super::target_path_from_url;

    #[test]
    fn extracts_target_path_from_origin_only_provider_url() {
        assert_eq!(
            target_path_from_url("https://provider.example/v1/messages"),
            Some("/v1/messages".to_string())
        );
    }

    #[test]
    fn preserves_provider_prefix_and_query_in_target_path() {
        assert_eq!(
            target_path_from_url("https://provider.example/apps/anthropic/v1/messages?beta=true"),
            Some("/apps/anthropic/v1/messages?beta=true".to_string())
        );
    }
}
