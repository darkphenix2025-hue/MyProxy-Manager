use base64::Engine;

const MAX_IDENTITY_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Clone, PartialEq, Eq)]
pub struct VerifiedCodexIdentity {
    pub subject: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub workspace: Option<String>,
    pub plan: Option<String>,
}

impl std::fmt::Debug for VerifiedCodexIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedCodexIdentity")
            .field("subject", &"[REDACTED]")
            .field("email", &self.email.as_ref().map(|_| "[REDACTED]"))
            .field(
                "display_name",
                &self.display_name.as_ref().map(|_| "[REDACTED]"),
            )
            .field("workspace", &self.workspace.as_ref().map(|_| "[REDACTED]"))
            .field("plan", &self.plan)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CodexIdentityError {
    #[error("invalid Codex identity configuration")]
    InvalidConfiguration,
    #[error("Codex identity request failed")]
    RequestFailed,
    #[error("Codex identity endpoint returned HTTP {0}")]
    Provider(reqwest::StatusCode),
    #[error("Codex identity response is invalid")]
    InvalidResponse,
}

#[derive(Clone)]
pub struct CodexIdentityClient {
    endpoint: url::Url,
    client: reqwest::Client,
}

impl std::fmt::Debug for CodexIdentityClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexIdentityClient")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl CodexIdentityClient {
    pub fn new(endpoint: &str) -> Result<Self, CodexIdentityError> {
        let endpoint = validate_endpoint(endpoint, false)?;
        Ok(Self {
            endpoint,
            client: build_client()?,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test_http(endpoint: String) -> Result<Self, CodexIdentityError> {
        let endpoint = validate_endpoint(&endpoint, true)?;
        Ok(Self {
            endpoint,
            client: build_client()?,
        })
    }

    pub async fn verify(
        &self,
        access_token: &str,
    ) -> Result<VerifiedCodexIdentity, CodexIdentityError> {
        use futures::StreamExt;

        if access_token.trim().is_empty() {
            return Err(CodexIdentityError::InvalidResponse);
        }
        let response = self
            .client
            .get(self.endpoint.clone())
            .bearer_auth(access_token)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| CodexIdentityError::RequestFailed)?;
        if !response.status().is_success() {
            return Err(CodexIdentityError::Provider(response.status()));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_IDENTITY_RESPONSE_BYTES as u64)
        {
            return Err(CodexIdentityError::InvalidResponse);
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| CodexIdentityError::InvalidResponse)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_IDENTITY_RESPONSE_BYTES {
                return Err(CodexIdentityError::InvalidResponse);
            }
            bytes.extend_from_slice(&chunk);
        }
        let response: UserInfoResponse =
            serde_json::from_slice(&bytes).map_err(|_| CodexIdentityError::InvalidResponse)?;
        response.into_verified()
    }

    /// Extract the identity claims included in the OAuth `id_token`.
    ///
    /// Codex includes the ChatGPT account and organization claims in this
    /// token when `id_token_add_organizations=true` is requested. The token
    /// has already been obtained from the OAuth token endpoint; this method
    /// only reads metadata for account display and fingerprinting. Upstream
    /// requests continue to use the access token, never these claims.
    pub fn verify_id_token(
        &self,
        id_token: &str,
    ) -> Result<VerifiedCodexIdentity, CodexIdentityError> {
        let mut segments = id_token.split('.');
        let header = segments.next().filter(|value| !value.is_empty());
        let payload = segments.next().filter(|value| !value.is_empty());
        let signature = segments.next().filter(|value| !value.is_empty());
        if header.is_none() || payload.is_none() || signature.is_none() || segments.next().is_some()
        {
            return Err(CodexIdentityError::InvalidResponse);
        }

        let Some(payload) = payload else {
            return Err(CodexIdentityError::InvalidResponse);
        };
        let payload = decode_jwt_segment(payload)?;
        let claims: UserInfoResponse =
            serde_json::from_slice(&payload).map_err(|_| CodexIdentityError::InvalidResponse)?;
        claims.into_verified()
    }
}

fn decode_jwt_segment(segment: &str) -> Result<Vec<u8>, CodexIdentityError> {
    let remainder = segment.len() % 4;
    if remainder == 1 {
        return Err(CodexIdentityError::InvalidResponse);
    }
    let mut padded = segment.to_string();
    if remainder != 0 {
        padded.extend(std::iter::repeat_n('=', 4 - remainder));
    }
    base64::engine::general_purpose::URL_SAFE
        .decode(padded)
        .map_err(|_| CodexIdentityError::InvalidResponse)
}

fn validate_endpoint(value: &str, allow_http: bool) -> Result<url::Url, CodexIdentityError> {
    let endpoint = url::Url::parse(value).map_err(|_| CodexIdentityError::InvalidConfiguration)?;
    let allowed_scheme =
        endpoint.scheme() == "https" || (allow_http && endpoint.scheme() == "http");
    if !allowed_scheme
        || endpoint.host_str().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(CodexIdentityError::InvalidConfiguration);
    }
    Ok(endpoint)
}

fn build_client() -> Result<reqwest::Client, CodexIdentityError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| CodexIdentityError::InvalidConfiguration)
}

#[derive(serde::Deserialize)]
struct UserInfoResponse {
    #[serde(default)]
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default, rename = "https://api.openai.com/auth")]
    auth: Option<UserInfoAuth>,
}

#[derive(Default, serde::Deserialize)]
struct UserInfoAuth {
    #[serde(default)]
    chatgpt_account_id: Option<String>,
    #[serde(default)]
    chatgpt_plan_type: Option<String>,
    #[serde(default)]
    organizations: Vec<UserInfoOrganization>,
}

#[derive(serde::Deserialize)]
struct UserInfoOrganization {
    id: String,
    #[serde(default)]
    is_default: bool,
}

impl UserInfoResponse {
    fn into_verified(self) -> Result<VerifiedCodexIdentity, CodexIdentityError> {
        let auth = self.auth.unwrap_or_default();
        let subject = nonempty(auth.chatgpt_account_id).or_else(|| nonempty(Some(self.sub)));
        let subject = subject.ok_or(CodexIdentityError::InvalidResponse)?;
        let workspace = auth
            .organizations
            .iter()
            .find(|organization| organization.is_default)
            .or_else(|| auth.organizations.first())
            .and_then(|organization| nonempty(Some(organization.id.clone())));
        Ok(VerifiedCodexIdentity {
            subject,
            email: nonempty(self.email),
            display_name: nonempty(self.name),
            workspace,
            plan: nonempty(auth.chatgpt_plan_type),
        })
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::State, http::HeaderMap, response::IntoResponse, routing::get, Json, Router,
    };
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    async fn start_server(
        status: reqwest::StatusCode,
        body: serde_json::Value,
    ) -> (String, Arc<Mutex<Option<String>>>) {
        let authorization = Arc::new(Mutex::new(None));
        let captured = authorization.clone();
        let app = Router::new()
            .route(
                "/userinfo",
                get(
                    |State((status, body, captured)): State<(
                        reqwest::StatusCode,
                        serde_json::Value,
                        Arc<Mutex<Option<String>>>,
                    )>,
                     headers: HeaderMap| async move {
                        *captured.lock().unwrap() = headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string);
                        (status, Json(body)).into_response()
                    },
                ),
            )
            .with_state((status, body, captured));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}/userinfo"), authorization)
    }

    #[test]
    fn production_identity_endpoint_must_be_plain_https() {
        for endpoint in [
            "http://auth.openai.com/userinfo",
            "https://user:secret@auth.openai.com/userinfo",
            "https://auth.openai.com/userinfo?token=secret",
            "https://auth.openai.com/userinfo#fragment",
        ] {
            assert!(matches!(
                CodexIdentityClient::new(endpoint),
                Err(CodexIdentityError::InvalidConfiguration)
            ));
        }
        CodexIdentityClient::new("https://auth.openai.com/userinfo").unwrap();
    }

    #[test]
    fn verified_identity_debug_redacts_personal_identifiers() {
        let identity = VerifiedCodexIdentity {
            subject: "subject-canary".to_string(),
            email: Some("email-canary@example.com".to_string()),
            display_name: Some("name-canary".to_string()),
            workspace: Some("workspace-canary".to_string()),
            plan: Some("team".to_string()),
        };
        let rendered = format!("{identity:?}");
        for canary in [
            "subject-canary",
            "email-canary",
            "name-canary",
            "workspace-canary",
        ] {
            assert!(!rendered.contains(canary));
        }
    }

    #[tokio::test]
    async fn verifies_identity_from_authenticated_userinfo_response() {
        let (endpoint, authorization) = start_server(
            reqwest::StatusCode::OK,
            json!({
                "sub": "user-subject",
                "email": "user@example.com",
                "name": "Example User",
                "https://api.openai.com/auth": {
                    "chatgpt_account_id": "account-123",
                    "chatgpt_plan_type": "plus",
                    "organizations": [
                        {"id": "workspace-secondary", "is_default": false},
                        {"id": "workspace-primary", "is_default": true}
                    ]
                }
            }),
        )
        .await;
        let client = CodexIdentityClient::for_test_http(endpoint).unwrap();

        let identity = client.verify("access-canary").await.unwrap();

        assert_eq!(identity.subject, "account-123");
        assert_eq!(identity.email.as_deref(), Some("user@example.com"));
        assert_eq!(identity.display_name.as_deref(), Some("Example User"));
        assert_eq!(identity.workspace.as_deref(), Some("workspace-primary"));
        assert_eq!(identity.plan.as_deref(), Some("plus"));
        assert_eq!(
            authorization.lock().unwrap().as_deref(),
            Some("Bearer access-canary")
        );
    }

    #[test]
    fn verifies_codex_identity_from_id_token_claims_without_userinfo() {
        let client =
            CodexIdentityClient::for_test_http("http://127.0.0.1:1/userinfo".to_string()).unwrap();
        let id_token = test_id_token(json!({
            "sub": "user-subject",
            "email": "user@example.com",
            "name": "Example User",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "account-123",
                "chatgpt_plan_type": "plus",
                "organizations": [
                    {"id": "workspace-secondary", "is_default": false},
                    {"id": "workspace-primary", "is_default": true}
                ]
            }
        }));

        let identity = client.verify_id_token(&id_token).unwrap();

        assert_eq!(identity.subject, "account-123");
        assert_eq!(identity.email.as_deref(), Some("user@example.com"));
        assert_eq!(identity.display_name.as_deref(), Some("Example User"));
        assert_eq!(identity.workspace.as_deref(), Some("workspace-primary"));
        assert_eq!(identity.plan.as_deref(), Some("plus"));
    }

    #[test]
    fn rejects_malformed_codex_id_token() {
        let client =
            CodexIdentityClient::for_test_http("http://127.0.0.1:1/userinfo".to_string()).unwrap();

        assert!(matches!(
            client.verify_id_token("not-a-jwt"),
            Err(CodexIdentityError::InvalidResponse)
        ));
    }

    fn test_id_token(claims: serde_json::Value) -> String {
        use base64::Engine;

        let encode = |value: serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&value).unwrap())
        };
        format!(
            "{}.{}.signature",
            encode(json!({"alg": "RS256", "typ": "JWT"})),
            encode(claims)
        )
    }

    #[tokio::test]
    async fn rejects_missing_subject_and_never_echoes_provider_body() {
        let canary = "identity-provider-secret-canary";
        let (endpoint, _) =
            start_server(reqwest::StatusCode::UNAUTHORIZED, json!({"error": canary})).await;
        let client = CodexIdentityClient::for_test_http(endpoint).unwrap();

        let error = client.verify("access-token").await.unwrap_err();

        assert!(matches!(error, CodexIdentityError::Provider(_)));
        assert!(!error.to_string().contains(canary));

        let (endpoint, _) = start_server(reqwest::StatusCode::OK, json!({"email": "x@y.z"})).await;
        let client = CodexIdentityClient::for_test_http(endpoint).unwrap();
        assert!(matches!(
            client.verify("access-token").await,
            Err(CodexIdentityError::InvalidResponse)
        ));
    }
}
