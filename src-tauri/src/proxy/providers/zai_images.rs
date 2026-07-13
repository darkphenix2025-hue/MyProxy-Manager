use axum::http::{HeaderValue, StatusCode};
use serde_json::{json, Value};
use tokio::time::Duration;

use crate::proxy::config::UpstreamProvider;
use crate::proxy::server::AppState;

/// Build a reqwest::Client for upstream OpenAI-compatible image calls.
fn build_images_client(
    upstream_proxy: Option<crate::proxy::config::UpstreamProxyConfig>,
    timeout_secs: u64,
) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(timeout_secs.max(30))); // Images take longer

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

/// Forward a single image generation request to an OpenAI-compatible provider.
///
/// POSTs JSON body to `{base_url}/v1/images/generations` and returns
/// OpenAI-compatible `{"created": ..., "data": [{"url": ...} | {"b64_json": ...}]}`.
async fn forward_single_image(
    state: &AppState,
    provider: &UpstreamProvider,
    prompt: &str,
    model: &str,
    n: usize,
    size: Option<&str>,
    quality: Option<&str>,
    style: Option<&str>,
) -> Result<(Value, String), (StatusCode, String)> {
    let url = format!(
        "{}{}",
        provider.base_url.trim_end_matches('/'),
        "/v1/images/generations"
    );

    let timeout_secs = provider
        .request_timeout_secs
        .unwrap_or(state.request_timeout.max(30));

    let upstream_proxy = state.upstream_proxy.read().await.clone();
    let client = build_images_client(Some(upstream_proxy), timeout_secs)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let mut body = json!({
        "model": model,
        "prompt": prompt,
        "n": n
    });

    if let Some(s) = size {
        body["size"] = Value::String(s.to_string());
    }
    if let Some(q) = quality {
        body["quality"] = Value::String(q.to_string());
    }
    if let Some(s) = style {
        body["style"] = Value::String(s.to_string());
    }

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", provider.api_key)) {
        headers.insert("authorization", v);
    }

    tracing::debug!("Forwarding image generation to provider: {}", url);

    let response = client
        .post(&url)
        .headers(headers)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("Upstream request failed: {}", e),
            )
        })?;

    let status = response.status();
    if !status.is_success() {
        let error_body = response.bytes().await.unwrap_or_default();
        return Err((
            status,
            format!(
                "Provider error {}: {}",
                status,
                String::from_utf8_lossy(&error_body)
            ),
        ));
    }

    let bytes = response.bytes().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("Failed to read response: {}", e),
        )
    })?;

    let value: Value = serde_json::from_slice(&bytes).map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("Failed to parse response JSON: {}", e),
        )
    })?;

    // Use provider name as the identifier for provider-based routing
    Ok((value, provider.name.clone()))
}

/// Forward image generations to an OpenAI-compatible provider.
///
/// Handles concurrency by spawning tokio tasks (one per image).
/// Returns `(email_header, openai_response_json)` on success.
pub async fn forward_images_to_openai_compat(
    state: &AppState,
    provider: &UpstreamProvider,
    body: &Value,
) -> Result<(String, Value), (StatusCode, String)> {
    let prompt = body.get("prompt").and_then(|v| v.as_str()).ok_or((
        StatusCode::BAD_REQUEST,
        "Missing 'prompt' field".to_string(),
    ))?;

    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("dall-e-3");

    let n = body.get("n").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let size = body.get("size").and_then(|v| v.as_str());
    let quality = body.get("quality").and_then(|v| v.as_str());
    let style = body.get("style").and_then(|v| v.as_str());
    let response_format = body
        .get("response_format")
        .and_then(|v| v.as_str())
        .unwrap_or("b64_json");

    let provider_name = provider.name.clone();
    let max_attempts = 3;

    let mut images: Vec<Value> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // Spawn concurrent tasks (one per image)
    let mut tasks = Vec::new();
    for _ in 0..n {
        let state = state.clone();
        let provider = provider.clone();
        let prompt = prompt.to_string();
        let model = model.to_string();
        let size = size.map(|s| s.to_string());
        let quality = quality.map(|q| q.to_string());
        let style = style.map(|s| s.to_string());

        tasks.push(tokio::spawn(async move {
            let mut last_error = String::new();
            for attempt in 0..max_attempts {
                match forward_single_image(
                    &state,
                    &provider,
                    &prompt,
                    &model,
                    1,
                    size.as_deref(),
                    quality.as_deref(),
                    style.as_deref(),
                )
                .await
                {
                    Ok((resp, _provider_name)) => return Ok(resp),
                    Err((_, e)) => {
                        last_error = e;
                        if attempt < max_attempts - 1 {
                            tokio::time::sleep(Duration::from_millis(500)).await;
                            continue;
                        }
                    }
                }
            }
            Err(last_error)
        }));
    }

    for (idx, task) in tasks.into_iter().enumerate() {
        match task.await {
            Ok(Ok(resp)) => {
                // Extract image data from OpenAI-compatible response
                if let Some(data_array) = resp.get("data").and_then(|d| d.as_array()) {
                    for item in data_array {
                        if let Some(url) = item.get("url").and_then(|v| v.as_str()) {
                            images.push(json!({ "url": url }));
                        } else if let Some(b64) = item.get("b64_json").and_then(|v| v.as_str()) {
                            if response_format == "url" {
                                images.push(json!({
                                    "url": format!("data:image/png;base64,{}", b64)
                                }));
                            } else {
                                images.push(json!({ "b64_json": b64 }));
                            }
                        }
                    }
                }
                tracing::debug!("[Images] Provider task {} succeeded", idx);
            }
            Ok(Err(e)) => {
                tracing::error!("[Images] Provider task {} failed: {}", idx, e);
                errors.push(e);
            }
            Err(e) => {
                let err_msg = format!("Task join error: {}", e);
                tracing::error!("[Images] Task {} join error: {}", idx, e);
                errors.push(err_msg);
            }
        }
    }

    if images.is_empty() {
        let error_msg = if !errors.is_empty() {
            errors.join("; ")
        } else {
            "No images generated".to_string()
        };
        let status = if error_msg.contains("429") {
            StatusCode::TOO_MANY_REQUESTS
        } else if error_msg.contains("503") {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::BAD_GATEWAY
        };
        return Err((status, error_msg));
    }

    if !errors.is_empty() {
        tracing::warn!(
            "[Images] Partial success: {} out of {} succeeded. Errors: {}",
            images.len(),
            n,
            errors.join("; ")
        );
    }

    let openai_response = json!({
        "created": chrono::Utc::now().timestamp(),
        "data": images
    });

    Ok((provider_name, openai_response))
}

/// Forward an image edits request to an OpenAI-compatible provider.
pub async fn forward_images_edits_to_openai_compat(
    state: &AppState,
    provider: &UpstreamProvider,
    image_data: Option<String>,
    mask_data: Option<String>,
    reference_images: Vec<String>,
    prompt: &str,
    model: &str,
    _n: usize,
    _size: &str,
    response_format: &str,
    style: Option<&str>,
) -> Result<(String, Value), (StatusCode, String)> {
    let mut contents_parts = Vec::new();

    let full_prompt = match style {
        Some(s) => format!("{}, style: {}", prompt, s),
        None => prompt.to_string(),
    };
    contents_parts.push(json!({ "text": full_prompt }));

    if let Some(data) = image_data {
        contents_parts.push(json!({
            "inlineData": { "mimeType": "image/png", "data": data }
        }));
    }
    if let Some(data) = mask_data {
        contents_parts.push(json!({
            "inlineData": { "mimeType": "image/png", "data": data }
        }));
    }
    for ref_data in reference_images {
        contents_parts.push(json!({
            "inlineData": { "mimeType": "image/jpeg", "data": ref_data }
        }));
    }

    let url = format!(
        "{}{}",
        provider.base_url.trim_end_matches('/'),
        "/v1/images/generations"
    );

    let timeout_secs = provider
        .request_timeout_secs
        .unwrap_or(state.request_timeout.max(30));

    let upstream_proxy = state.upstream_proxy.read().await.clone();
    let client = build_images_client(Some(upstream_proxy), timeout_secs)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let body = json!({
        "model": model,
        "prompt": prompt,
        "n": 1,
        "images": contents_parts
    });

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", provider.api_key)) {
        headers.insert("authorization", v);
    }

    tracing::debug!("Forwarding image edits to provider: {}", url);

    let response = client
        .post(&url)
        .headers(headers)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("Upstream request failed: {}", e),
            )
        })?;

    let status = response.status();
    if !status.is_success() {
        let error_body = response.bytes().await.unwrap_or_default();
        return Err((
            status,
            format!(
                "Provider error {}: {}",
                status,
                String::from_utf8_lossy(&error_body)
            ),
        ));
    }

    let bytes = response.bytes().await.map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("Failed to read response: {}", e),
        )
    })?;

    let value: Value = serde_json::from_slice(&bytes).map_err(|e| {
        (
            StatusCode::BAD_GATEWAY,
            format!("Failed to parse response JSON: {}", e),
        )
    })?;

    let mut images: Vec<Value> = Vec::new();
    if let Some(data_array) = value.get("data").and_then(|d| d.as_array()) {
        for item in data_array {
            if let Some(url) = item.get("url").and_then(|v| v.as_str()) {
                images.push(json!({ "url": url }));
            } else if let Some(b64) = item.get("b64_json").and_then(|v| v.as_str()) {
                if response_format == "url" {
                    images.push(json!({
                        "url": format!("data:image/png;base64,{}", b64)
                    }));
                } else {
                    images.push(json!({ "b64_json": b64 }));
                }
            }
        }
    }

    let openai_response = json!({
        "created": chrono::Utc::now().timestamp(),
        "data": images
    });

    Ok((provider.name.clone(), openai_response))
}
