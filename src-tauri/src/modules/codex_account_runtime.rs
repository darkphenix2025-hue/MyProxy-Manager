use crate::models::connection::{
    AuthKind, ConnectionStatus, Credential, CredentialLifecycle, CredentialSummary, Identity,
    ProviderConnection, WireProtocol,
};
use crate::modules::account_platform_store::{AccountPlatformStore, AccountPlatformStoreError};
use crate::modules::codex_auth::{CodexAuthStart, CodexOAuthConfig};
use crate::modules::codex_identity::{
    CodexIdentityClient, CodexIdentityError, VerifiedCodexIdentity,
};
use crate::modules::codex_login::{
    CodexLoginError, CodexLoginFlow, CodexLoginResult, CodexLoginService,
};
use crate::modules::codex_tokens::{CodexTokenError, CodexTokenVault};
use crate::modules::secret_store::{KeyringSecretStore, SecretStore};
use serde::Serialize;

pub const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const CODEX_AUTHORIZATION_ENDPOINT: &str = "https://auth.openai.com/oauth/authorize";
pub const CODEX_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
pub const CODEX_USERINFO_ENDPOINT: &str = "https://auth.openai.com/userinfo";
pub const CODEX_CALLBACK_BIND_ADDRESS: &str = "127.0.0.1:0";
pub const CODEX_UPSTREAM_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex";
const ONBOARDING_LEASE_SECONDS: i64 = 10 * 60;

#[derive(Debug, thiserror::Error)]
pub enum CodexAccountRuntimeError {
    #[error(transparent)]
    Login(#[from] CodexLoginError),
    #[error(transparent)]
    Identity(#[from] CodexIdentityError),
    #[error(transparent)]
    Token(#[from] CodexTokenError),
    #[error(transparent)]
    Store(#[from] AccountPlatformStoreError),
    #[error("Codex login session was not found")]
    SessionNotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexLoginPhase {
    Pending,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct CodexLoginSessionStart {
    pub session_id: uuid::Uuid,
    pub authorization_url: String,
    pub expires_at: i64,
}

impl std::fmt::Debug for CodexLoginSessionStart {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexLoginSessionStart")
            .field("session_id", &self.session_id)
            .field("authorization_url", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl From<&CodexAuthStart> for CodexLoginSessionStart {
    fn from(value: &CodexAuthStart) -> Self {
        Self {
            session_id: value.session_id,
            authorization_url: value.authorization_url.clone(),
            expires_at: value.expires_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodexLoginSessionStatus {
    pub session_id: uuid::Uuid,
    pub phase: CodexLoginPhase,
    pub expires_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccountConnectionSummary {
    pub identity: Identity,
    pub credential: CredentialSummary,
    pub connection: ProviderConnection,
}

pub type ProductionCodexAccountRuntime = CodexAccountRuntime<KeyringSecretStore>;

pub struct CodexAccountRuntime<S: SecretStore> {
    login: CodexLoginService<S>,
    identity: CodexIdentityClient,
    vault: CodexTokenVault<S>,
    store: AccountPlatformStore,
    callback_bind_address: String,
    sessions:
        dashmap::DashMap<uuid::Uuid, std::sync::Arc<parking_lot::Mutex<CodexLoginSessionStatus>>>,
    cancellations: dashmap::DashMap<uuid::Uuid, tokio_util::sync::CancellationToken>,
    lifecycle_lock: tokio::sync::Mutex<()>,
}

impl<S: SecretStore> std::fmt::Debug for CodexAccountRuntime<S> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexAccountRuntime")
            .field("login", &"CodexLoginService")
            .field("identity", &self.identity)
            .field("vault", &"[SECRET STORE]")
            .field("store", &self.store)
            .field("session_count", &self.sessions.len())
            .finish()
    }
}

impl ProductionCodexAccountRuntime {
    pub fn production(data_dir: &std::path::Path) -> Result<Self, CodexAccountRuntimeError> {
        let oauth = default_oauth_config();
        let secret_store = KeyringSecretStore::default();
        Ok(Self::new(
            CodexLoginService::new(oauth, secret_store.clone())?,
            CodexIdentityClient::new(CODEX_USERINFO_ENDPOINT)?,
            secret_store,
            AccountPlatformStore::new(data_dir.join("accounts-v3.json")),
            CODEX_CALLBACK_BIND_ADDRESS,
        ))
    }
}

impl<S: SecretStore> CodexAccountRuntime<S> {
    pub fn new(
        login: CodexLoginService<S>,
        identity: CodexIdentityClient,
        secret_store: S,
        store: AccountPlatformStore,
        callback_bind_address: impl Into<String>,
    ) -> Self {
        Self {
            login,
            identity,
            vault: CodexTokenVault::new(secret_store),
            store,
            callback_bind_address: callback_bind_address.into(),
            sessions: dashmap::DashMap::new(),
            cancellations: dashmap::DashMap::new(),
            lifecycle_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub async fn start(
        self: &std::sync::Arc<Self>,
    ) -> Result<CodexLoginSessionStart, CodexAccountRuntimeError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.cancel_pending_sessions_for_replacement().await?;
        self.reconcile_onboarding().await?;
        self.cleanup_sessions();
        let flow = self.login.start(&self.callback_bind_address).await?;
        let start = CodexLoginSessionStart::from(flow.start());
        let planned_ref = self.login.planned_secret_ref(start.session_id)?;
        if let Err(error) = self.store.begin_onboarding(&planned_ref).await {
            let _ = self.login.cancel(start.session_id);
            return Err(error.into());
        }
        self.sessions.insert(
            start.session_id,
            std::sync::Arc::new(parking_lot::Mutex::new(CodexLoginSessionStatus {
                session_id: start.session_id,
                phase: CodexLoginPhase::Pending,
                expires_at: start.expires_at,
                connection_id: None,
                error_code: None,
            })),
        );
        self.cancellations
            .insert(start.session_id, tokio_util::sync::CancellationToken::new());
        let runtime = self.clone();
        tokio::spawn(async move { runtime.complete_in_background(flow).await });
        Ok(start)
    }

    pub fn status(
        &self,
        session_id: uuid::Uuid,
    ) -> Result<CodexLoginSessionStatus, CodexAccountRuntimeError> {
        self.sessions
            .get(&session_id)
            .map(|status| status.lock().clone())
            .ok_or(CodexAccountRuntimeError::SessionNotFound)
    }

    pub async fn cancel(&self, session_id: uuid::Uuid) -> Result<(), CodexAccountRuntimeError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        let status = self
            .sessions
            .get(&session_id)
            .map(|entry| entry.clone())
            .ok_or(CodexAccountRuntimeError::SessionNotFound)?;
        if status.lock().phase != CodexLoginPhase::Pending {
            return Ok(());
        }
        if let Some(cancellation) = self.cancellations.get(&session_id) {
            cancellation.cancel();
        }
        let _ = self.login.cancel(session_id);
        let planned_ref = self.login.planned_secret_ref(session_id)?;
        self.vault.delete(&planned_ref).await?;
        self.store.clear_onboarding(&planned_ref).await?;
        let mut status = status.lock();
        if status.phase == CodexLoginPhase::Pending {
            status.phase = CodexLoginPhase::Cancelled;
            status.error_code = None;
        }
        Ok(())
    }

    pub async fn list_connections(
        &self,
    ) -> Result<Vec<AccountConnectionSummary>, CodexAccountRuntimeError> {
        let (identities, credentials, connections) = self.store.load().await?;
        let identities: std::collections::HashMap<_, _> = identities
            .into_iter()
            .map(|identity| (identity.id.clone(), identity))
            .collect();
        let credentials: std::collections::HashMap<_, _> = credentials
            .into_iter()
            .map(|credential| (credential.id.clone(), credential))
            .collect();
        connections
            .into_iter()
            .map(|connection| {
                let identity_id = connection
                    .identity_id
                    .as_deref()
                    .ok_or(AccountPlatformStoreError::InvalidDocument)?;
                let identity = identities
                    .get(identity_id)
                    .cloned()
                    .ok_or(AccountPlatformStoreError::InvalidDocument)?;
                let credential = credentials
                    .get(&connection.credential_id)
                    .map(CredentialSummary::from)
                    .ok_or(AccountPlatformStoreError::InvalidDocument)?;
                Ok(AccountConnectionSummary {
                    identity,
                    credential,
                    connection,
                })
            })
            .collect::<Result<Vec<_>, AccountPlatformStoreError>>()
            .map_err(CodexAccountRuntimeError::from)
    }

    async fn complete_in_background(self: std::sync::Arc<Self>, flow: CodexLoginFlow) {
        let session_id = flow.start().session_id;
        let Some(cancellation) = self
            .cancellations
            .get(&session_id)
            .map(|entry| entry.clone())
        else {
            return;
        };
        let Some(status) = self.sessions.get(&session_id).map(|entry| entry.clone()) else {
            return;
        };
        let outcome = match self.login.complete(flow).await {
            Ok(login) => {
                self.persist_verified_login_cancellable(login, &cancellation, Some(&status))
                    .await
            }
            Err(error) => Err(CodexAccountRuntimeError::Login(error)),
        };
        if outcome.is_ok() {
            self.cancellations.remove(&session_id);
            return;
        }
        let _lifecycle = self.lifecycle_lock.lock().await;
        let cleanup_ref = match &outcome {
            Err(CodexAccountRuntimeError::Login(CodexLoginError::CleanupRequired {
                secret_ref,
                ..
            })) => Some(secret_ref.clone()),
            _ => self.login.planned_secret_ref(session_id).ok(),
        };
        if let Some(secret_ref) = cleanup_ref {
            if self.vault.delete(&secret_ref).await.is_ok() {
                let _ = self.store.clear_onboarding(&secret_ref).await;
            }
        }
        let mut status = status.lock();
        if status.phase != CodexLoginPhase::Pending {
            return;
        }
        match outcome {
            Ok(_) => unreachable!("successful completion returned above"),
            Err(error) => {
                status.phase = if matches!(
                    error,
                    CodexAccountRuntimeError::Login(CodexLoginError::Cancelled)
                ) {
                    CodexLoginPhase::Cancelled
                } else {
                    CodexLoginPhase::Failed
                };
                status.error_code = Some(error_code(&error).to_string());
            }
        }
        self.cancellations.remove(&session_id);
    }

    #[cfg(test)]
    async fn persist_verified_login(
        &self,
        login: CodexLoginResult,
    ) -> Result<String, CodexAccountRuntimeError> {
        self.store.begin_onboarding(&login.secret_ref).await?;
        self.persist_verified_login_cancellable(
            login,
            &tokio_util::sync::CancellationToken::new(),
            None,
        )
        .await
    }

    async fn persist_verified_login_cancellable(
        &self,
        login: CodexLoginResult,
        cancellation: &tokio_util::sync::CancellationToken,
        completion_status: Option<&std::sync::Arc<parking_lot::Mutex<CodexLoginSessionStatus>>>,
    ) -> Result<String, CodexAccountRuntimeError> {
        let result = async {
            let tokens = self.vault.read(&login.secret_ref).await?;
            let verified = tokio::select! {
                _ = cancellation.cancelled() => {
                    return Err(CodexAccountRuntimeError::Login(CodexLoginError::Cancelled));
                }
                result = self.identity.verify(tokens.access_token()) => result?,
            };
            let (identity, credential, connection) =
                create_records(verified, login.secret_ref.clone(), login.expires_at);
            let connection_id = connection.id.clone();
            let _lifecycle = self.lifecycle_lock.lock().await;
            if cancellation.is_cancelled() {
                return Err(CodexAccountRuntimeError::Login(CodexLoginError::Cancelled));
            }
            self.store.append(identity, credential, connection).await?;
            if let Some(status) = completion_status {
                let mut status = status.lock();
                status.phase = CodexLoginPhase::Completed;
                status.connection_id = Some(connection_id.clone());
                status.error_code = None;
            }
            if self
                .store
                .clear_onboarding(&login.secret_ref)
                .await
                .is_err()
            {
                tracing::warn!("Codex onboarding journal cleanup deferred");
            }
            Ok(connection_id)
        }
        .await;
        result
    }

    async fn reconcile_onboarding(&self) -> Result<(), CodexAccountRuntimeError> {
        self.reconcile_onboarding_at(chrono::Utc::now().timestamp())
            .await
    }

    async fn reconcile_onboarding_at(&self, now: i64) -> Result<(), CodexAccountRuntimeError> {
        let Some(pending) = self.store.pending_onboarding().await? else {
            return Ok(());
        };
        let (_, credentials, _) = self.store.load().await?;
        if credentials
            .iter()
            .any(|credential| credential.material.secret_ref() == &pending)
        {
            self.store.clear_onboarding(&pending).await?;
            return Ok(());
        }
        let Some(secret_ref) = self
            .store
            .stale_onboarding(now, ONBOARDING_LEASE_SECONDS)
            .await?
        else {
            return Err(AccountPlatformStoreError::Conflict.into());
        };
        let (_, credentials, _) = self.store.load().await?;
        if !credentials
            .iter()
            .any(|credential| credential.material.secret_ref() == &secret_ref)
        {
            self.vault.delete(&secret_ref).await?;
        }
        self.store.clear_onboarding(&secret_ref).await?;
        Ok(())
    }

    async fn cancel_pending_sessions_for_replacement(
        &self,
    ) -> Result<(), CodexAccountRuntimeError> {
        let pending: Vec<(
            uuid::Uuid,
            std::sync::Arc<parking_lot::Mutex<CodexLoginSessionStatus>>,
        )> = self
            .sessions
            .iter()
            .filter_map(|entry| {
                (entry.value().lock().phase == CodexLoginPhase::Pending)
                    .then(|| (*entry.key(), entry.value().clone()))
            })
            .collect();
        for (session_id, status) in pending {
            if let Some(cancellation) = self.cancellations.get(&session_id) {
                cancellation.cancel();
            }
            let _ = self.login.cancel(session_id);
            let planned_ref = self.login.planned_secret_ref(session_id)?;
            self.vault.delete(&planned_ref).await?;
            self.store.clear_onboarding(&planned_ref).await?;
            let mut status = status.lock();
            if status.phase == CodexLoginPhase::Pending {
                status.phase = CodexLoginPhase::Cancelled;
                status.error_code = None;
            }
            self.cancellations.remove(&session_id);
        }
        Ok(())
    }

    fn cleanup_sessions(&self) {
        let cutoff = chrono::Utc::now().timestamp().saturating_sub(300);
        self.sessions
            .retain(|_, status| status.lock().expires_at >= cutoff);
        self.cancellations
            .retain(|session_id, _| self.sessions.contains_key(session_id));
    }
}

pub fn default_oauth_config() -> CodexOAuthConfig {
    CodexOAuthConfig {
        client_id: CODEX_OAUTH_CLIENT_ID.to_string(),
        authorization_endpoint: CODEX_AUTHORIZATION_ENDPOINT.to_string(),
        token_endpoint: CODEX_TOKEN_ENDPOINT.to_string(),
        scopes: vec![
            "openid".to_string(),
            "email".to_string(),
            "profile".to_string(),
            "offline_access".to_string(),
        ],
    }
}

fn create_records(
    verified: VerifiedCodexIdentity,
    secret_ref: crate::models::connection::SecretRef,
    expires_at: i64,
) -> (Identity, Credential, ProviderConnection) {
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    digest.update(verified.subject.as_bytes());
    digest.update([0]);
    if let Some(workspace) = verified.workspace.as_deref() {
        digest.update(workspace.as_bytes());
    }
    let fingerprint = format!(
        "sha256:{}",
        base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            digest.finalize()
        )
    );
    let identity_id = uuid::Uuid::new_v4().to_string();
    let credential_id = uuid::Uuid::new_v4().to_string();
    let mut identity = Identity::new(&identity_id, "openai");
    identity.subject = Some(fingerprint.clone());
    identity.email = verified.email;
    identity.display_name = verified.display_name;
    identity.workspace = verified.workspace;
    if let Some(plan) = verified.plan {
        identity
            .metadata
            .insert("plan".to_string(), serde_json::Value::String(plan));
    }
    let mut credential = Credential::new(
        &credential_id,
        Some(identity_id.clone()),
        AuthKind::CodexOauthTokenSet,
        secret_ref,
        fingerprint,
    );
    credential.lifecycle = CredentialLifecycle::Ready;
    credential.expires_at = Some(expires_at);
    credential.scopes = vec![
        "openid".to_string(),
        "email".to_string(),
        "profile".to_string(),
        "offline_access".to_string(),
    ];
    let mut connection = ProviderConnection::new(
        uuid::Uuid::new_v4().to_string(),
        "openai_codex",
        credential_id,
        CODEX_UPSTREAM_ENDPOINT,
        vec![WireProtocol::CodexResponsesUpstream],
    );
    connection.identity_id = Some(identity_id);
    connection.status = ConnectionStatus::Ready;
    connection.enabled = true;
    (identity, credential, connection)
}

pub fn error_code(error: &CodexAccountRuntimeError) -> &'static str {
    match error {
        CodexAccountRuntimeError::Login(CodexLoginError::Cancelled) => "cancelled",
        CodexAccountRuntimeError::Login(CodexLoginError::CallbackTimeout) => "callback_timeout",
        CodexAccountRuntimeError::Login(_) => "login_failed",
        CodexAccountRuntimeError::Identity(_) => "identity_validation_failed",
        CodexAccountRuntimeError::Token(_) => "credential_storage_failed",
        CodexAccountRuntimeError::Store(_) => "account_storage_failed",
        CodexAccountRuntimeError::SessionNotFound => "session_not_found",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::codex_tokens::CodexTokenSet;
    use crate::modules::secret_store::MemorySecretStore;
    use axum::{routing::get, Json, Router};
    use serde_json::json;

    async fn identity_endpoint(body: serde_json::Value) -> String {
        let app = Router::new().route("/userinfo", get(move || async move { Json(body.clone()) }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}/userinfo")
    }

    #[test]
    fn login_start_debug_redacts_authorization_url_and_state() {
        let start = CodexLoginSessionStart {
            session_id: uuid::Uuid::new_v4(),
            authorization_url: "https://auth.openai.com/oauth/authorize?state=state-canary"
                .to_string(),
            expires_at: 2_000_000_000,
        };
        let rendered = format!("{start:?}");
        assert!(!rendered.contains("state-canary"));
        assert!(!rendered.contains("auth.openai.com"));
    }

    #[tokio::test]
    async fn verified_login_creates_v3_records_without_raw_subject_or_tokens() {
        let endpoint = identity_endpoint(json!({
            "sub": "raw-user-subject",
            "email": "codex@example.com",
            "name": "Codex User",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "raw-account-id",
                "chatgpt_plan_type": "team",
                "organizations": [{"id": "workspace-1", "is_default": true}]
            }
        }))
        .await;
        let secrets = MemorySecretStore::default();
        let vault = CodexTokenVault::new(secrets.clone());
        let token_set = CodexTokenSet::new(
            "access-canary",
            "refresh-canary",
            None,
            "Bearer",
            2_000_000_000,
        )
        .unwrap();
        let secret_ref = vault.create(&token_set).await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store = AccountPlatformStore::new(directory.path().join("accounts-v3.json"));
        let runtime = CodexAccountRuntime::new(
            CodexLoginService::with_client_for_test(
                default_oauth_config(),
                crate::modules::codex_tokens::CodexTokenClient::for_test_http(
                    "http://127.0.0.1:1/token".to_string(),
                )
                .unwrap(),
                secrets.clone(),
            ),
            CodexIdentityClient::for_test_http(endpoint).unwrap(),
            secrets,
            store.clone(),
            "127.0.0.1:0",
        );

        let connection_id = runtime
            .persist_verified_login(CodexLoginResult {
                secret_ref,
                expires_at: 2_000_000_000,
            })
            .await
            .unwrap();

        let (identities, credentials, connections) = store.load().await.unwrap();
        assert_eq!(identities[0].email.as_deref(), Some("codex@example.com"));
        assert_eq!(identities[0].workspace.as_deref(), Some("workspace-1"));
        assert!(identities[0]
            .subject
            .as_deref()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(credentials[0].lifecycle, CredentialLifecycle::Ready);
        assert_eq!(connections[0].id, connection_id);
        let raw = std::fs::read_to_string(directory.path().join("accounts-v3.json")).unwrap();
        assert!(!raw.contains("raw-account-id"));
        assert!(!raw.contains("access-canary"));
        assert!(!raw.contains("refresh-canary"));
        let summaries = runtime.list_connections().await.unwrap();
        let public_json = serde_json::to_string(&summaries).unwrap();
        assert!(!public_json.contains("secret-"));
        assert!(!public_json.contains("access-canary"));
        assert!(!public_json.contains("refresh-canary"));
    }

    #[tokio::test]
    async fn runtime_owns_pending_flow_and_cancel_transitions_status() {
        let endpoint = identity_endpoint(json!({"sub": "unused"})).await;
        let secrets = MemorySecretStore::default();
        let directory = tempfile::tempdir().unwrap();
        let runtime = std::sync::Arc::new(CodexAccountRuntime::new(
            CodexLoginService::with_client_for_test(
                default_oauth_config(),
                crate::modules::codex_tokens::CodexTokenClient::for_test_http(
                    "http://127.0.0.1:1/token".to_string(),
                )
                .unwrap(),
                secrets.clone(),
            ),
            CodexIdentityClient::for_test_http(endpoint).unwrap(),
            secrets,
            AccountPlatformStore::new(directory.path().join("accounts-v3.json")),
            "127.0.0.1:0",
        ));

        let start = runtime.start().await.unwrap();
        assert_eq!(
            runtime.status(start.session_id).unwrap().phase,
            CodexLoginPhase::Pending
        );

        runtime.cancel(start.session_id).await.unwrap();

        assert_eq!(
            runtime.status(start.session_id).unwrap().phase,
            CodexLoginPhase::Cancelled
        );
    }

    #[tokio::test]
    async fn starting_again_replaces_the_previous_pending_flow() {
        let secrets = MemorySecretStore::default();
        let directory = tempfile::tempdir().unwrap();
        let runtime = std::sync::Arc::new(CodexAccountRuntime::new(
            CodexLoginService::with_client_for_test(
                default_oauth_config(),
                crate::modules::codex_tokens::CodexTokenClient::for_test_http(
                    "http://127.0.0.1:1/token".to_string(),
                )
                .unwrap(),
                secrets.clone(),
            ),
            CodexIdentityClient::for_test_http("http://127.0.0.1:1/userinfo".to_string()).unwrap(),
            secrets,
            AccountPlatformStore::new(directory.path().join("accounts-v3.json")),
            "127.0.0.1:0",
        ));

        let first = runtime.start().await.unwrap();
        let second = runtime.start().await.unwrap();

        assert_eq!(
            runtime.status(first.session_id).unwrap().phase,
            CodexLoginPhase::Cancelled
        );
        assert_eq!(
            runtime.status(second.session_id).unwrap().phase,
            CodexLoginPhase::Pending
        );
        runtime.cancel(second.session_id).await.unwrap();
    }

    #[tokio::test]
    async fn explicit_cancel_cleans_journal_before_next_start() {
        let secrets = MemorySecretStore::default();
        let directory = tempfile::tempdir().unwrap();
        let runtime = std::sync::Arc::new(CodexAccountRuntime::new(
            CodexLoginService::with_client_for_test(
                default_oauth_config(),
                crate::modules::codex_tokens::CodexTokenClient::for_test_http(
                    "http://127.0.0.1:1/token".to_string(),
                )
                .unwrap(),
                secrets.clone(),
            ),
            CodexIdentityClient::for_test_http("http://127.0.0.1:1/userinfo".to_string()).unwrap(),
            secrets,
            AccountPlatformStore::new(directory.path().join("accounts-v3.json")),
            "127.0.0.1:0",
        ));

        let first = runtime.start().await.unwrap();
        runtime.cancel(first.session_id).await.unwrap();
        let second = runtime.start().await.unwrap();

        assert_eq!(
            runtime.status(second.session_id).unwrap().phase,
            CodexLoginPhase::Pending
        );
        runtime.cancel(second.session_id).await.unwrap();
    }

    #[tokio::test]
    async fn recovery_deletes_uncommitted_secret_and_clears_journal() {
        let secrets = MemorySecretStore::default();
        let vault = CodexTokenVault::new(secrets.clone());
        let secret_ref = vault
            .create(
                &CodexTokenSet::new(
                    "access-recovery-canary",
                    "refresh-recovery-canary",
                    None,
                    "Bearer",
                    2_000_000_000,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let store = AccountPlatformStore::new(directory.path().join("accounts-v3.json"));
        store.begin_onboarding(&secret_ref).await.unwrap();
        let runtime = CodexAccountRuntime::new(
            CodexLoginService::with_client_for_test(
                default_oauth_config(),
                crate::modules::codex_tokens::CodexTokenClient::for_test_http(
                    "http://127.0.0.1:1/token".to_string(),
                )
                .unwrap(),
                secrets.clone(),
            ),
            CodexIdentityClient::for_test_http("http://127.0.0.1:1/userinfo".to_string()).unwrap(),
            secrets,
            store.clone(),
            "127.0.0.1:0",
        );

        runtime
            .reconcile_onboarding_at(chrono::Utc::now().timestamp() + ONBOARDING_LEASE_SECONDS + 1)
            .await
            .unwrap();

        assert!(matches!(
            vault.read(&secret_ref).await,
            Err(CodexTokenError::SecretStore(
                crate::modules::secret_store::SecretStoreError::NotFound
            ))
        ));
        assert!(store.pending_onboarding().await.unwrap().is_none());
    }
}
