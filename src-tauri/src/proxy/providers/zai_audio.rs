use axum::{
    http::{HeaderValue, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::Value;
use tokio::time::Duration;

use crate::proxy::config::UpstreamProvider;
use crate::proxy::server::AppState;

/// Build a reqwest::Client for upstream OpenAI-compatible audio calls.
fn build_audio_client(
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

/// Forward an audio transcription request to an OpenAI-compatible provider.
///
/// Sends multipart/form-data (file + model + prompt) to `{base_url}/v1/audio/transcriptions`
/// and returns OpenAI-compatible `{"text": "..."}` response.
pub async fn forward_audio_to_openai_compat(
    state: &AppState,
    provider: &UpstreamProvider,
    audio_bytes: &[u8],
    mime_type: &str,
    model: &str,
    prompt: &str,
) -> axum::response::Response {
    use reqwest::multipart;

    // Build multipart/form-data body
    let file_part = multipart::Part::bytes(audio_bytes.to_vec())
        .file_name("audio.wav")
        .mime_str(mime_type)
        .unwrap_or_else(|_| multipart::Part::bytes(audio_bytes.to_vec()));

    let form = multipart::Form::new()
        .part("file", file_part)
        .text("model", model.to_string())
        .text("prompt", prompt.to_string());

    let url = format!(
        "{}/audio/transcriptions",
        provider.base_url.trim_end_matches('/')
    );

    let timeout_secs = provider
        .request_timeout_secs
        .unwrap_or(state.request_timeout.max(5));

    let upstream_proxy = state.upstream_proxy.read().await.clone();
    let client = match build_audio_client(Some(upstream_proxy), timeout_secs) {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to build HTTP client: {}", e),
            )
                .into_response();
        }
    };

    let mut headers = axum::http::HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", provider.api_key)) {
        headers.insert("authorization", v);
    }

    tracing::debug!("Forwarding audio to provider: {}", url);

    let response = match client
        .post(&url)
        .multipart(form)
        .headers(headers)
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
        return axum::response::Response::builder()
            .status(status)
            .header("x-provider-name", &provider.name)
            .header("x-mapped-model", model)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(
                String::from_utf8_lossy(&error_body).to_string(),
            ))
            .unwrap_or_else(|_| (status, "Failed to build response").into_response());
    }

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

    // Parse and return OpenAI-compatible response
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) => {
            // Extract text if present, otherwise return raw response
            let text = value.get("text").and_then(|v| v.as_str()).unwrap_or("");
            (
                StatusCode::OK,
                [("X-Provider-Name", provider.name.as_str())],
                Json(serde_json::json!({ "text": text })),
            )
                .into_response()
        }
        Err(_) => {
            // Return raw response as-is
            (
                StatusCode::OK,
                [("X-Provider-Name", provider.name.as_str())],
                Json(serde_json::json!({
                    "text": String::from_utf8_lossy(&bytes),
                    "raw_response": String::from_utf8_lossy(&bytes)
                })),
            )
                .into_response()
        }
    }
}
