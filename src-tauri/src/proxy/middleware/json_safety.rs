use axum::{
    body::Body,
    http::{header::CONTENT_TYPE, HeaderValue, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Maximum body size (bytes) to inspect and potentially modify (~2 MB).
/// Bodies larger than this pass through without wrapping.
const WRAP_SIZE_LIMIT: usize = 2 * 1024 * 1024;

/// JSON Safety Middleware
///
/// Runs between auth and handler. Ensures that:
/// 1. Error responses (4xx/5xx) always have `content-type: application/json`
/// 2. Corrupted/truncated JSON in error responses is replaced with a safe fallback
/// 3. Non-JSON error responses are wrapped in `{"error":{"message":"..."}}`
///
/// Panics are caught via `tokio::spawn` — a panicked handler returns a safe
/// JSON error instead of crashing the server.
///
/// Streaming responses (`text/event-stream`) pass through unchanged.
pub async fn json_safety_middleware(req: Request<Body>, next: Next) -> Response {
    // Spawn in a separate task so panics are caught as JoinError, not server crashes
    let resp_future = async {
        let resp = next.run(req).await;

        // Skip streaming responses entirely
        if let Some(ct) = resp.headers().get(CONTENT_TYPE) {
            if ct
                .to_str()
                .map_or(false, |s| s.contains("text/event-stream"))
            {
                return resp;
            }
        }

        let status = resp.status();
        let is_error = status.is_client_error() || status.is_server_error();
        let already_json = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map_or(false, |s| s.contains("application/json"));

        // Clone headers before consuming the body (resp.into_body() moves resp)
        let original_headers = resp.headers().clone();

        // Collect body — skip if too large to avoid memory pressure
        let body_bytes = match axum::body::to_bytes(resp.into_body(), WRAP_SIZE_LIMIT).await {
            Ok(b) => b,
            Err(_) => return default_error_response(),
        };

        // Skip bodies at the size limit (potentially larger payloads)
        if body_bytes.len() >= WRAP_SIZE_LIMIT {
            return make_response(body_bytes, None, status, original_headers.clone());
        }

        if already_json {
            // Validate JSON integrity for error responses with JSON content-type
            if is_error && serde_json::from_slice::<serde_json::Value>(&body_bytes).is_err() {
                // Corrupted/truncated JSON — replace with safe fallback
                let fallback = serde_json::json!({
                    "error": {
                        "message": "Internal server error (response corrupted)"
                    }
                });
                let bytes = serde_json::to_string(&fallback)
                    .unwrap_or_else(|_| r#"{"error":{"message":"Internal server error"}}"#.into());
                return make_response(
                    bytes.into_bytes().into(),
                    Some("application/json"),
                    status,
                    original_headers.clone(),
                );
            }
            // Valid JSON (or non-error) — pass through with original headers unchanged
            return make_response(body_bytes, None, status, original_headers.clone());
        }

        if is_error {
            // Non-JSON error response — wrap in JSON envelope
            let message = String::from_utf8_lossy(&body_bytes).to_string();
            let wrapped = serde_json::json!({ "error": { "message": message } });
            let bytes = serde_json::to_string(&wrapped)
                .unwrap_or_else(|_| r#"{"error":{"message":"Internal server error"}}"#.into());
            return make_response(
                bytes.into_bytes().into(),
                Some("application/json"),
                status,
                original_headers.clone(),
            );
        }

        // Non-error, non-JSON response — pass through with original headers
        make_response(body_bytes, None, status, original_headers)
    };

    // Spawn to catch panics — prevents handler crashes from taking down the server
    match tokio::spawn(resp_future).await {
        Ok(resp) => resp,
        Err(join_err) => {
            tracing::error!(?join_err, "Handler panicked or was cancelled");
            default_error_response()
        }
    }
}

/// Rebuild a Response with the given body, preserving all original headers.
/// If a new content-type is provided, it replaces the existing one.
fn make_response(
    body: bytes::Bytes,
    content_type: Option<&'static str>,
    status: StatusCode,
    original_headers: axum::http::HeaderMap,
) -> Response {
    let mut builder = Response::builder().status(status);

    // Add all non-content-type headers from the original response
    for (key, value) in original_headers.iter() {
        if key != CONTENT_TYPE {
            builder = builder.header(key, value);
        }
    }

    // Set content-type: use the new value if provided, otherwise preserve original
    if let Some(ct) = content_type {
        builder = builder.header(CONTENT_TYPE, HeaderValue::from_static(ct));
    } else if let Some(v) = original_headers.get(CONTENT_TYPE) {
        builder = builder.header(CONTENT_TYPE, v.clone());
    }

    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| default_error_response())
}

/// Safe fallback JSON error response — used when something goes wrong.
fn default_error_response() -> Response {
    let body = r#"{"error":{"message":"Internal server error"}}"#;
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| {
            // Ultimate fallback — bare minimum response
            (StatusCode::INTERNAL_SERVER_ERROR, body.to_string()).into_response()
        })
}
