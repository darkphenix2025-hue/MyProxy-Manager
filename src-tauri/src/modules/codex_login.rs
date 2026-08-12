use crate::models::connection::SecretRef;
use crate::modules::codex_auth::{
    CodexAuthError, CodexAuthSessionManager, CodexAuthSessionStatus, CodexAuthStart,
    CodexOAuthConfig,
};
use crate::modules::codex_loopback::{CodexLoopbackError, CodexLoopbackListener};
use crate::modules::codex_tokens::{CodexTokenClient, CodexTokenError, CodexTokenVault};
use crate::modules::secret_store::SecretStore;

#[derive(Debug, thiserror::Error)]
pub enum CodexLoginError {
    #[error(transparent)]
    Auth(#[from] CodexAuthError),
    #[error(transparent)]
    Loopback(#[from] CodexLoopbackError),
    #[error(transparent)]
    Token(#[from] CodexTokenError),
    #[error("Codex login callback was cancelled")]
    Cancelled,
    #[error("Codex login callback timed out")]
    CallbackTimeout,
    #[error("Codex login was cancelled but persisted credentials require cleanup")]
    CleanupRequired {
        secret_ref: SecretRef,
        #[source]
        source: CodexTokenError,
    },
}

#[derive(Debug)]
pub struct CodexLoginFlow {
    start: CodexAuthStart,
    listener: CodexLoopbackListener,
    cancellation: tokio_util::sync::CancellationToken,
}

impl CodexLoginFlow {
    pub fn start(&self) -> &CodexAuthStart {
        &self.start
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexLoginResult {
    pub secret_ref: SecretRef,
    pub expires_at: i64,
}

#[derive(Debug, Clone)]
pub struct CodexLoginService<S: SecretStore> {
    config: CodexOAuthConfig,
    sessions: std::sync::Arc<CodexAuthSessionManager>,
    token_client: CodexTokenClient,
    vault: CodexTokenVault<S>,
    listeners: std::sync::Arc<dashmap::DashMap<uuid::Uuid, tokio_util::sync::CancellationToken>>,
    start_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    lifecycle_lock: std::sync::Arc<parking_lot::Mutex<()>>,
}

impl<S: SecretStore> CodexLoginService<S> {
    pub fn new(config: CodexOAuthConfig, store: S) -> Result<Self, CodexLoginError> {
        let token_client = CodexTokenClient::new(&config.token_endpoint)?;
        Ok(Self {
            config,
            sessions: std::sync::Arc::new(CodexAuthSessionManager::default()),
            token_client,
            vault: CodexTokenVault::new(store),
            listeners: std::sync::Arc::new(dashmap::DashMap::new()),
            start_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            lifecycle_lock: std::sync::Arc::new(parking_lot::Mutex::new(())),
        })
    }

    #[cfg(test)]
    fn with_client_for_test(
        config: CodexOAuthConfig,
        token_client: CodexTokenClient,
        store: S,
    ) -> Self {
        Self {
            config,
            sessions: std::sync::Arc::new(CodexAuthSessionManager::default()),
            token_client,
            vault: CodexTokenVault::new(store),
            listeners: std::sync::Arc::new(dashmap::DashMap::new()),
            start_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            lifecycle_lock: std::sync::Arc::new(parking_lot::Mutex::new(())),
        }
    }

    pub async fn start(
        &self,
        bind_address: impl ToString,
    ) -> Result<CodexLoginFlow, CodexLoginError> {
        let _start_guard = self.start_lock.lock().await;
        let listener = CodexLoopbackListener::bind(bind_address).await?;
        let (start, cancellation) = {
            let _lifecycle = self.lifecycle_lock.lock();
            let start = self
                .sessions
                .start(&self.config, &listener.redirect_uri())?;
            let cancellation = tokio_util::sync::CancellationToken::new();
            for existing in self.listeners.iter() {
                existing.value().cancel();
            }
            self.listeners.clear();
            self.listeners
                .insert(start.session_id, cancellation.clone());
            (start, cancellation)
        };
        Ok(CodexLoginFlow {
            start,
            listener,
            cancellation,
        })
    }

    pub async fn complete(
        &self,
        flow: CodexLoginFlow,
    ) -> Result<CodexLoginResult, CodexLoginError> {
        let session_id = flow.start.session_id;
        let remaining_seconds = flow
            .start
            .expires_at
            .saturating_sub(chrono::Utc::now().timestamp());
        if remaining_seconds <= 0 {
            self.listeners.remove(&session_id);
            let _ = self.sessions.cancel(session_id);
            return Err(CodexLoginError::CallbackTimeout);
        }
        let deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(remaining_seconds as u64);
        let (callback, material) = loop {
            let callback = tokio::select! {
                callback = flow.listener.accept() => callback?,
                _ = flow.cancellation.cancelled() => {
                    self.listeners.remove(&session_id);
                    return Err(CodexLoginError::Cancelled);
                }
                _ = tokio::time::sleep_until(deadline) => {
                    self.listeners.remove(&session_id);
                    let _ = self.sessions.cancel(session_id);
                    return Err(CodexLoginError::CallbackTimeout);
                }
            };
            match self
                .sessions
                .complete(session_id, callback.state(), callback.code())
            {
                Ok(material) => break (callback, material),
                Err(CodexAuthError::StateMismatch) => {
                    let _ = callback.respond_failure().await;
                }
                Err(error) => {
                    self.listeners.remove(&session_id);
                    let _ = callback.respond_failure().await;
                    return Err(error.into());
                }
            }
        };
        let exchange = self.token_client.exchange_code(
            &self.config.client_id,
            &material,
            chrono::Utc::now().timestamp(),
        );
        let token_set = match tokio::select! {
            result = exchange => result,
            _ = flow.cancellation.cancelled() => {
                self.listeners.remove(&session_id);
                let _ = callback.respond_failure().await;
                return Err(CodexLoginError::Cancelled);
            }
        } {
            Ok(token_set) => token_set,
            Err(error) => {
                self.listeners.remove(&session_id);
                let _ = callback.respond_failure().await;
                return Err(error.into());
            }
        };
        if flow.cancellation.is_cancelled() {
            self.listeners.remove(&session_id);
            let _ = callback.respond_failure().await;
            return Err(CodexLoginError::Cancelled);
        }
        let secret_ref = match self.vault.create(&token_set).await {
            Ok(secret_ref) => secret_ref,
            Err(error) => {
                self.listeners.remove(&session_id);
                let _ = callback.respond_failure().await;
                return Err(error.into());
            }
        };
        if flow.cancellation.is_cancelled() {
            if let Err(source) = self.vault.delete(&secret_ref).await {
                self.listeners.remove(&session_id);
                let _ = callback.respond_failure().await;
                return Err(CodexLoginError::CleanupRequired { secret_ref, source });
            }
            self.listeners.remove(&session_id);
            let _ = callback.respond_failure().await;
            return Err(CodexLoginError::Cancelled);
        }
        {
            let _lifecycle = self.lifecycle_lock.lock();
            if flow.cancellation.is_cancelled() {
                drop(_lifecycle);
                if let Err(source) = self.vault.delete(&secret_ref).await {
                    self.listeners.remove(&session_id);
                    let _ = callback.respond_failure().await;
                    return Err(CodexLoginError::CleanupRequired { secret_ref, source });
                }
                self.listeners.remove(&session_id);
                let _ = callback.respond_failure().await;
                return Err(CodexLoginError::Cancelled);
            }
            self.listeners.remove(&session_id);
        }
        let _ = callback.respond_success().await;
        Ok(CodexLoginResult {
            secret_ref,
            expires_at: token_set.expires_at(),
        })
    }

    pub fn cancel(&self, session_id: uuid::Uuid) -> Result<(), CodexLoginError> {
        let _lifecycle = self.lifecycle_lock.lock();
        if let Some((_, cancellation)) = self.listeners.remove(&session_id) {
            cancellation.cancel();
            let _ = self.sessions.cancel(session_id);
            return Ok(());
        }
        self.sessions.cancel(session_id)?;
        Ok(())
    }

    pub fn status(
        &self,
        session_id: uuid::Uuid,
    ) -> Result<CodexAuthSessionStatus, CodexLoginError> {
        match self.sessions.status(session_id) {
            Ok(status) => Ok(status),
            Err(error) => {
                if matches!(error, CodexAuthError::Expired) {
                    if let Some((_, cancellation)) = self.listeners.remove(&session_id) {
                        cancellation.cancel();
                    }
                }
                Err(error.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::secret_store::MemorySecretStore;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn config(token_endpoint: String) -> CodexOAuthConfig {
        CodexOAuthConfig {
            client_id: "public-client".to_string(),
            authorization_endpoint: "https://auth.openai.com/oauth/authorize".to_string(),
            token_endpoint,
            scopes: vec!["openid".to_string(), "offline_access".to_string()],
        }
    }

    async fn token_server() -> (String, tokio::sync::oneshot::Receiver<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (request_tx, request_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0_u8; 16 * 1024];
            let size = stream.read(&mut request).await.unwrap();
            let _ = request_tx.send(String::from_utf8_lossy(&request[..size]).to_string());
            let body = r#"{"access_token":"access-canary","refresh_token":"refresh-canary","id_token":"id-canary","expires_in":3600,"token_type":"Bearer"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        (format!("http://{address}/token"), request_rx)
    }

    #[tokio::test]
    async fn browser_flow_exchanges_callback_and_persists_only_secret_reference() {
        let (endpoint, token_request) = token_server().await;
        let store = MemorySecretStore::default();
        let service = CodexLoginService::with_client_for_test(
            config("https://auth.openai.com/oauth/token".to_string()),
            CodexTokenClient::for_test_http(endpoint).unwrap(),
            store.clone(),
        );
        let flow = service.start("127.0.0.1:0").await.unwrap();
        let start = flow.start().clone();
        let auth_url = url::Url::parse(&start.authorization_url).unwrap();
        let state = auth_url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let redirect = auth_url
            .query_pairs()
            .find(|(key, _)| key == "redirect_uri")
            .unwrap()
            .1
            .into_owned();
        let address = url::Url::parse(&redirect).unwrap();
        let port = address.port().unwrap();

        let complete = tokio::spawn(async move { service.complete(flow).await });
        let mut wrong = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        wrong
            .write_all(
                b"GET /auth/callback?code=wrong-code&state=wrong-state HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            )
            .await
            .unwrap();
        let mut wrong_response = String::new();
        wrong.read_to_string(&mut wrong_response).await.unwrap();
        assert!(wrong_response.starts_with("HTTP/1.1 400 Bad Request"));

        let mut browser = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        browser
            .write_all(
                format!(
                    "GET /auth/callback?code=code-canary&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        browser.read_to_string(&mut response).await.unwrap();

        let result = complete.await.unwrap().unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(!format!("{result:?}").contains("canary"));
        let restored = CodexTokenVault::new(store)
            .read(&result.secret_ref)
            .await
            .unwrap();
        assert_eq!(restored.access_token(), "access-canary");
        let request = token_request.await.unwrap();
        assert!(request.contains("code=code-canary"));
        assert!(!request.contains("access-canary"));
    }

    #[tokio::test]
    async fn cancelling_session_stops_pending_listener_wait() {
        let service = CodexLoginService::with_client_for_test(
            config("https://auth.openai.com/oauth/token".to_string()),
            CodexTokenClient::for_test_http("http://127.0.0.1:9/token".to_string()).unwrap(),
            MemorySecretStore::default(),
        );
        let flow = service.start("127.0.0.1:0").await.unwrap();
        let session_id = flow.start().session_id;
        let completing_service = service.clone();
        let completion = tokio::spawn(async move { completing_service.complete(flow).await });
        tokio::task::yield_now().await;

        service.cancel(session_id).unwrap();
        assert!(matches!(
            completion.await.unwrap(),
            Err(CodexLoginError::Cancelled)
        ));
    }
}
