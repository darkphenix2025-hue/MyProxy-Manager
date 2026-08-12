use crate::models::connection::SecretRef;
use crate::modules::codex_auth::CodexTokenExchangeMaterial;
use crate::modules::secret_store::{SecretStore, SecretStoreError};
use serde::{Deserialize, Serialize};
use std::fmt;

const MAX_TOKEN_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CodexTokenError {
    #[error("invalid Codex token configuration: {0}")]
    InvalidConfiguration(String),
    #[error("Codex token request failed")]
    RequestFailed,
    #[error("Codex token endpoint returned HTTP {status}")]
    Provider { status: u16 },
    #[error("Codex token response is invalid")]
    InvalidResponse,
    #[error("Codex refresh token is unavailable")]
    MissingRefreshToken,
    #[error("Codex token persistence failed")]
    SecretStore(#[from] SecretStoreError),
}

#[derive(PartialEq, Eq)]
pub struct CodexTokenSet {
    access_token: zeroize::Zeroizing<String>,
    refresh_token: zeroize::Zeroizing<String>,
    id_token: Option<zeroize::Zeroizing<String>>,
    token_type: String,
    expires_at: i64,
}

#[derive(Serialize, Deserialize)]
struct PersistedCodexTokenSet {
    access_token: String,
    refresh_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id_token: Option<String>,
    token_type: String,
    expires_at: i64,
}

impl CodexTokenSet {
    pub fn new(
        access_token: impl Into<String>,
        refresh_token: impl Into<String>,
        id_token: Option<String>,
        token_type: impl Into<String>,
        expires_at: i64,
    ) -> Result<Self, CodexTokenError> {
        let access_token = access_token.into();
        let refresh_token = refresh_token.into();
        let token_type = token_type.into();
        if access_token.trim().is_empty()
            || refresh_token.trim().is_empty()
            || !token_type.trim().eq_ignore_ascii_case("bearer")
            || expires_at <= 0
        {
            return Err(CodexTokenError::InvalidResponse);
        }
        Ok(Self {
            access_token: access_token.into(),
            refresh_token: refresh_token.into(),
            id_token: id_token.map(zeroize::Zeroizing::new),
            token_type,
            expires_at,
        })
    }

    pub fn access_token(&self) -> &str {
        self.access_token.as_str()
    }

    pub fn refresh_token(&self) -> &str {
        self.refresh_token.as_str()
    }

    pub fn id_token(&self) -> Option<&str> {
        self.id_token.as_deref().map(String::as_str)
    }

    pub fn token_type(&self) -> &str {
        &self.token_type
    }

    pub fn expires_at(&self) -> i64 {
        self.expires_at
    }

    fn encode_for_secret_store(&self) -> Result<zeroize::Zeroizing<String>, CodexTokenError> {
        let persisted = PersistedCodexTokenSet {
            access_token: self.access_token().to_string(),
            refresh_token: self.refresh_token().to_string(),
            id_token: self.id_token().map(str::to_string),
            token_type: self.token_type.clone(),
            expires_at: self.expires_at,
        };
        serde_json::to_string(&persisted)
            .map(zeroize::Zeroizing::new)
            .map_err(|_| CodexTokenError::InvalidResponse)
    }

    fn decode_from_secret_store(encoded: &str) -> Result<Self, CodexTokenError> {
        let persisted: PersistedCodexTokenSet =
            serde_json::from_str(encoded).map_err(|_| CodexTokenError::InvalidResponse)?;
        Self::new(
            persisted.access_token,
            persisted.refresh_token,
            persisted.id_token,
            persisted.token_type,
            persisted.expires_at,
        )
    }
}

impl fmt::Debug for CodexTokenSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexTokenSet")
            .field("access_token", &"[REDACTED]")
            .field("refresh_token", &"[REDACTED]")
            .field("id_token", &self.id_token.as_ref().map(|_| "[REDACTED]"))
            .field("token_type", &self.token_type)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Deserialize)]
struct TokenEndpointResponse {
    access_token: String,
    expires_in: i64,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default = "default_token_type")]
    token_type: String,
}

impl fmt::Debug for TokenEndpointResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenEndpointResponse")
            .field("access_token", &"[REDACTED]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[REDACTED]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[REDACTED]"))
            .field("expires_in", &self.expires_in)
            .field("token_type", &self.token_type)
            .finish()
    }
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

#[derive(Debug, Clone)]
pub struct CodexTokenClient {
    client: reqwest::Client,
    token_endpoint: url::Url,
}

impl CodexTokenClient {
    pub fn new(token_endpoint: &str) -> Result<Self, CodexTokenError> {
        let token_endpoint = url::Url::parse(token_endpoint).map_err(|_| {
            CodexTokenError::InvalidConfiguration("token endpoint must be a valid URL".into())
        })?;
        if token_endpoint.scheme() != "https"
            || token_endpoint.host_str().is_none()
            || !token_endpoint.username().is_empty()
            || token_endpoint.password().is_some()
            || token_endpoint.query().is_some()
            || token_endpoint.fragment().is_some()
        {
            return Err(CodexTokenError::InvalidConfiguration(
                "token endpoint must be a safe HTTPS URL".into(),
            ));
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| CodexTokenError::InvalidConfiguration("HTTP client failed".into()))?,
            token_endpoint,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test_http(token_endpoint: String) -> Result<Self, CodexTokenError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| CodexTokenError::InvalidConfiguration("HTTP client failed".into()))?,
            token_endpoint: url::Url::parse(&token_endpoint)
                .map_err(|_| CodexTokenError::InvalidConfiguration("test endpoint".into()))?,
        })
    }

    pub async fn exchange_code(
        &self,
        client_id: &str,
        material: &CodexTokenExchangeMaterial,
        now: i64,
    ) -> Result<CodexTokenSet, CodexTokenError> {
        if client_id.trim().is_empty() {
            return Err(CodexTokenError::InvalidConfiguration(
                "client_id is required".into(),
            ));
        }
        let form = [
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("code", material.authorization_code()),
            ("redirect_uri", material.redirect_uri()),
            ("code_verifier", material.code_verifier()),
        ];
        let response = self.send_form(&form).await?;
        build_token_set(response, None, now)
    }

    pub async fn refresh(
        &self,
        client_id: &str,
        current: &CodexTokenSet,
        now: i64,
    ) -> Result<CodexTokenSet, CodexTokenError> {
        if client_id.trim().is_empty() {
            return Err(CodexTokenError::InvalidConfiguration(
                "client_id is required".into(),
            ));
        }
        if current.refresh_token().trim().is_empty() {
            return Err(CodexTokenError::MissingRefreshToken);
        }
        let form = [
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", current.refresh_token()),
        ];
        let response = self.send_form(&form).await?;
        build_token_set(response, Some(current.refresh_token()), now)
    }

    async fn send_form(
        &self,
        form: &[(&str, &str)],
    ) -> Result<TokenEndpointResponse, CodexTokenError> {
        let response = self
            .client
            .post(self.token_endpoint.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .form(form)
            .send()
            .await
            .map_err(|_| CodexTokenError::RequestFailed)?;
        let status = response.status();
        if !status.is_success() {
            return Err(CodexTokenError::Provider {
                status: status.as_u16(),
            });
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_TOKEN_RESPONSE_BYTES as u64)
        {
            return Err(CodexTokenError::InvalidResponse);
        }
        use futures::StreamExt;
        let mut stream = response.bytes_stream();
        let mut body = Vec::with_capacity(4096);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| CodexTokenError::InvalidResponse)?;
            if body.len().saturating_add(chunk.len()) > MAX_TOKEN_RESPONSE_BYTES {
                return Err(CodexTokenError::InvalidResponse);
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice::<TokenEndpointResponse>(&body)
            .map_err(|_| CodexTokenError::InvalidResponse)
    }
}

fn build_token_set(
    response: TokenEndpointResponse,
    fallback_refresh_token: Option<&str>,
    now: i64,
) -> Result<CodexTokenSet, CodexTokenError> {
    if response.expires_in <= 0 {
        return Err(CodexTokenError::InvalidResponse);
    }
    let refresh_token = response
        .refresh_token
        .filter(|token| !token.trim().is_empty())
        .or_else(|| fallback_refresh_token.map(str::to_string))
        .ok_or(CodexTokenError::MissingRefreshToken)?;
    CodexTokenSet::new(
        response.access_token,
        refresh_token,
        response.id_token,
        response.token_type,
        now.saturating_add(response.expires_in),
    )
}

#[derive(Debug, Clone)]
pub struct CodexTokenVault<S: SecretStore> {
    store: S,
}

#[derive(Clone)]
pub struct CodexRefreshCoordinator<S: SecretStore> {
    client_id: String,
    client: CodexTokenClient,
    vault: CodexTokenVault<S>,
}

impl<S: SecretStore> fmt::Debug for CodexRefreshCoordinator<S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexRefreshCoordinator")
            .field("client_id", &self.client_id)
            .field("client", &self.client)
            .field("vault", &"[SECRET STORE]")
            .finish()
    }
}

impl<S: SecretStore> CodexRefreshCoordinator<S> {
    pub fn new(client_id: impl Into<String>, client: CodexTokenClient, store: S) -> Self {
        Self {
            client_id: client_id.into(),
            client,
            vault: CodexTokenVault::new(store),
        }
    }

    pub async fn refresh_if_expiring(
        &self,
        secret_ref: &SecretRef,
        now: i64,
        refresh_skew_seconds: i64,
    ) -> Result<(), CodexTokenError> {
        let current = self.vault.read(secret_ref).await?;
        if current.expires_at() > now.saturating_add(refresh_skew_seconds.max(0)) {
            return Ok(());
        }
        let lock_key = refresh_lock_key(secret_ref);
        let lock = refresh_locks()
            .entry(lock_key)
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        let _guard = lock.lock().await;

        let current = self.vault.read(secret_ref).await?;
        if current.expires_at() > now.saturating_add(refresh_skew_seconds.max(0)) {
            return Ok(());
        }
        let refreshed = self.client.refresh(&self.client_id, &current, now).await?;
        self.vault.replace(secret_ref, &refreshed).await
    }
}

fn refresh_lock_key(secret_ref: &SecretRef) -> String {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    digest.update(secret_ref.backend.as_bytes());
    digest.update([0]);
    digest.update(secret_ref.key.as_bytes());
    base64::Engine::encode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        digest.finalize(),
    )
}

fn refresh_locks() -> &'static dashmap::DashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>> {
    static LOCKS: std::sync::OnceLock<
        dashmap::DashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>,
    > = std::sync::OnceLock::new();
    LOCKS.get_or_init(dashmap::DashMap::new)
}

impl<S: SecretStore> CodexTokenVault<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    pub async fn create(&self, token_set: &CodexTokenSet) -> Result<SecretRef, CodexTokenError> {
        let encoded = token_set.encode_for_secret_store()?;
        self.store
            .create("codex-oauth", &encoded)
            .await
            .map_err(CodexTokenError::from)
    }

    pub async fn read(&self, secret_ref: &SecretRef) -> Result<CodexTokenSet, CodexTokenError> {
        let encoded = self.store.read(secret_ref).await?;
        CodexTokenSet::decode_from_secret_store(&encoded)
    }

    pub async fn replace(
        &self,
        secret_ref: &SecretRef,
        token_set: &CodexTokenSet,
    ) -> Result<(), CodexTokenError> {
        let encoded = token_set.encode_for_secret_store()?;
        self.store.replace(secret_ref, &encoded).await?;
        Ok(())
    }

    pub async fn delete(&self, secret_ref: &SecretRef) -> Result<(), CodexTokenError> {
        self.store.delete(secret_ref).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::secret_store::MemorySecretStore;

    fn test_exchange_material(code: &str, verifier: &str) -> CodexTokenExchangeMaterial {
        CodexTokenExchangeMaterial::for_test(code, "http://127.0.0.1:1455/auth/callback", verifier)
    }

    enum TokenTestReply {
        Success(&'static str),
        Error { status: u16, body: &'static str },
        OversizedChunked,
    }

    struct TokenTestServer {
        address: std::net::SocketAddr,
        received: tokio::sync::oneshot::Receiver<String>,
    }

    impl TokenTestServer {
        async fn start(reply: TokenTestReply) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (tx, received) = tokio::sync::oneshot::channel();
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = vec![0_u8; 16 * 1024];
                let size = stream.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..size]).to_string();
                let body = request.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
                let _ = tx.send(body);
                match reply {
                    TokenTestReply::Success(response_body) => {
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                            response_body.len()
                        );
                        stream.write_all(response.as_bytes()).await.unwrap();
                    }
                    TokenTestReply::Error { status, body } => {
                        let response = format!(
                            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        stream.write_all(response.as_bytes()).await.unwrap();
                    }
                    TokenTestReply::OversizedChunked => {
                        stream
                            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n")
                            .await
                            .unwrap();
                        let chunk = "x".repeat(MAX_TOKEN_RESPONSE_BYTES + 1);
                        stream
                            .write_all(
                                format!("{:X}\r\n{chunk}\r\n0\r\n\r\n", chunk.len()).as_bytes(),
                            )
                            .await
                            .unwrap();
                    }
                }
            });
            Self { address, received }
        }

        fn endpoint(&self) -> String {
            format!("http://{}/token", self.address)
        }

        async fn received_form(self) -> std::collections::HashMap<String, String> {
            url::form_urlencoded::parse(self.received.await.unwrap().as_bytes())
                .into_owned()
                .collect()
        }
    }

    #[test]
    fn token_set_debug_does_not_expose_tokens() {
        let token_set = CodexTokenSet::new(
            "access-canary",
            "refresh-canary",
            Some("id-canary".to_string()),
            "Bearer",
            1_700_000_000,
        )
        .unwrap();

        let debug = format!("{token_set:?}");
        assert!(!debug.contains("access-canary"));
        assert!(!debug.contains("refresh-canary"));
        assert!(!debug.contains("id-canary"));
    }

    #[tokio::test]
    async fn exchange_posts_pkce_form_and_sanitizes_provider_errors() {
        let server = TokenTestServer::start(TokenTestReply::Error {
            status: 400,
            body: r#"{"error":"invalid_grant","access_token":"response-canary"}"#,
        })
        .await;
        let client = CodexTokenClient::for_test_http(server.endpoint()).unwrap();
        let material = test_exchange_material("auth-code-canary", "verifier-canary");

        let error = client
            .exchange_code("public-client", &material, 1_700_000_000)
            .await
            .unwrap_err();
        assert!(!error.to_string().contains("response-canary"));

        let form = server.received_form().await;
        assert_eq!(form["grant_type"], "authorization_code");
        assert_eq!(form["code"], "auth-code-canary");
        assert_eq!(form["code_verifier"], "verifier-canary");
        assert_eq!(form["redirect_uri"], "http://127.0.0.1:1455/auth/callback");
    }

    #[tokio::test]
    async fn exchange_rejects_oversized_chunked_response_without_content_length() {
        let server = TokenTestServer::start(TokenTestReply::OversizedChunked).await;
        let client = CodexTokenClient::for_test_http(server.endpoint()).unwrap();
        let material = test_exchange_material("auth-code", "verifier");

        assert!(matches!(
            client
                .exchange_code("public-client", &material, 1_700_000_000)
                .await,
            Err(CodexTokenError::InvalidResponse)
        ));
    }

    #[tokio::test]
    async fn refresh_reuses_old_refresh_token_when_provider_omits_rotation() {
        let server = TokenTestServer::start(TokenTestReply::Success(
            r#"{"access_token":"new-access","expires_in":3600,"token_type":"Bearer"}"#,
        ))
        .await;
        let client = CodexTokenClient::for_test_http(server.endpoint()).unwrap();
        let current =
            CodexTokenSet::new("old-access", "old-refresh", None, "Bearer", 1_600_000_000).unwrap();

        let refreshed = client
            .refresh("public-client", &current, 1_700_000_000)
            .await
            .unwrap();
        assert_eq!(refreshed.access_token(), "new-access");
        assert_eq!(refreshed.refresh_token(), "old-refresh");
        assert_eq!(refreshed.expires_at(), 1_700_003_600);
    }

    #[tokio::test]
    async fn vault_persists_complete_token_set_and_returns_only_secret_ref() {
        let store = MemorySecretStore::default();
        let vault = CodexTokenVault::new(store.clone());
        let token_set = CodexTokenSet::new(
            "access-canary",
            "refresh-canary",
            Some("id-canary".to_string()),
            "Bearer",
            1_700_000_000,
        )
        .unwrap();

        let secret_ref = vault.create(&token_set).await.unwrap();
        assert_eq!(secret_ref.backend, "memory");
        assert!(!format!("{secret_ref:?}").contains("canary"));

        let restored = vault.read(&secret_ref).await.unwrap();
        assert_eq!(restored.access_token(), "access-canary");
        assert_eq!(restored.refresh_token(), "refresh-canary");
        assert_eq!(restored.id_token(), Some("id-canary"));
    }

    #[tokio::test]
    async fn concurrent_refresh_is_deduplicated_per_secret_reference() {
        let server = TokenTestServer::start(TokenTestReply::Success(
            r#"{"access_token":"new-access","refresh_token":"new-refresh","expires_in":3600,"token_type":"Bearer"}"#,
        ))
        .await;
        let store = MemorySecretStore::default();
        let vault = CodexTokenVault::new(store.clone());
        let expired =
            CodexTokenSet::new("old-access", "old-refresh", None, "Bearer", 1_600_000_000).unwrap();
        let secret_ref = vault.create(&expired).await.unwrap();
        let coordinator = CodexRefreshCoordinator::new(
            "public-client",
            CodexTokenClient::for_test_http(server.endpoint()).unwrap(),
            store,
        );

        let (first, second) = tokio::join!(
            coordinator.refresh_if_expiring(&secret_ref, 1_700_000_000, 300),
            coordinator.refresh_if_expiring(&secret_ref, 1_700_000_000, 300),
        );
        first.unwrap();
        second.unwrap();
        let form = server.received_form().await;
        assert_eq!(form["grant_type"], "refresh_token");
        assert_eq!(
            vault.read(&secret_ref).await.unwrap().access_token(),
            "new-access"
        );
    }
}
