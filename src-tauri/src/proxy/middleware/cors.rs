// CORS 中间件
use axum::http::{HeaderValue, Method};
use tower_http::cors::CorsLayer;

/// 创建代理接口 CORS layer（对外开放的 AI 代理接口）
/// 允许任意来源访问，因为这是 API 网关的核心功能
pub fn proxy_cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(tower_http::cors::Any)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::HEAD,
            Method::OPTIONS,
            Method::PATCH,
        ])
        .allow_headers(tower_http::cors::Any)
        .allow_credentials(false)
        .max_age(std::time::Duration::from_secs(3600))
}

/// 创建管理接口 CORS layer（白名单策略）
/// 仅允许可信来源：Tauri localhost、本地开发环境
pub fn admin_cors_layer() -> CorsLayer {
    // 管理接口允许的来源
    let allowed_origins = [
        "tauri://localhost",
        "http://tauri.localhost",
        "http://localhost:1420", // Vite dev server
        "http://127.0.0.1:1420", // Vite dev server (IPv4)
        "http://localhost:8045", // Proxy itself
        "http://127.0.0.1:8045", // Proxy itself (IPv4)
        "app://localhost",       // Tauri production
    ];

    let origins: Vec<HeaderValue> = allowed_origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();

    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::HEAD,
            Method::OPTIONS,
            Method::PATCH,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            axum::http::header::ACCEPT,
            axum::http::header::ORIGIN,
        ])
        .allow_credentials(false)
        .max_age(std::time::Duration::from_secs(3600))
}

/// 兼容旧接口：创建通用 CORS layer（使用代理策略）
#[deprecated(
    since = "4.2.0",
    note = "Use proxy_cors_layer or admin_cors_layer instead"
)]
#[allow(dead_code)]
pub fn cors_layer() -> CorsLayer {
    proxy_cors_layer()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::http::{header::ORIGIN, Request};
    use axum::{routing::get, Router};
    use tower::ServiceExt;

    fn test_app() -> Router {
        Router::new()
            .route("/api/test", get(|| async { "ok" }))
            .layer(admin_cors_layer())
    }

    #[test]
    fn test_proxy_cors_layer_creation() {
        let _layer = proxy_cors_layer();
    }

    #[test]
    fn test_admin_cors_layer_creation() {
        let _layer = admin_cors_layer();
    }

    #[tokio::test]
    async fn test_admin_cors_allows_trusted_origin() {
        // [SEC-002] 可信来源应获得 CORS 头
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/test")
                    .header(ORIGIN, "http://localhost:1420")
                    .header(axum::http::header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let allow_origin = response
            .headers()
            .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .expect("Should have Access-Control-Allow-Origin header");
        assert_eq!(allow_origin, "http://localhost:1420");
    }

    #[tokio::test]
    async fn test_admin_cors_rejects_malicious_origin() {
        // [SEC-002] 恶意来源不应获得 CORS 头
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/test")
                    .header(ORIGIN, "https://evil-site.example.com")
                    .header(axum::http::header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // tower-http CORS layer returns 200 but WITHOUT CORS headers for untrusted origins
        let has_cors = response
            .headers()
            .contains_key(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN);
        assert!(
            !has_cors,
            "Malicious origin should NOT receive Access-Control-Allow-Origin header"
        );
    }

    #[tokio::test]
    async fn test_admin_cors_rejects_spoofed_localhost() {
        // [SEC-002] 伪造的 localhost 域名应拒绝
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/test")
                    .header(ORIGIN, "http://localhost.evil.com")
                    .header(axum::http::header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let has_cors = response
            .headers()
            .contains_key(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN);
        assert!(
            !has_cors,
            "Spoofed localhost subdomain should NOT receive CORS headers"
        );
    }

    #[tokio::test]
    async fn test_admin_cors_rejects_wrong_port() {
        // [SEC-002] 错误端口应拒绝
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/test")
                    .header(ORIGIN, "http://localhost:9999")
                    .header(axum::http::header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let has_cors = response
            .headers()
            .contains_key(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN);
        assert!(
            !has_cors,
            "Wrong port (localhost:9999) should NOT receive CORS headers"
        );
    }

    #[tokio::test]
    async fn test_admin_cors_allows_proxy_self() {
        // [SEC-002] 代理自身（8045）应被允许
        let app = test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/test")
                    .header(ORIGIN, "http://localhost:8045")
                    .header(axum::http::header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let allow_origin = response
            .headers()
            .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .expect("Proxy self should have CORS header");
        assert_eq!(allow_origin, "http://localhost:8045");
    }
}
