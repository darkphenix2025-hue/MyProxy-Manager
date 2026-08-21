use crate::models::connection::{
    AuthKind, ConnectionStatus, Credential, CredentialLifecycle, CredentialSummary, Identity,
    ProviderConnection, WireProtocol,
};
use crate::modules::account_platform_store::{AccountPlatformStore, AccountPlatformStoreError};
use crate::modules::auth_file_store::CodexAuthStore;
use crate::modules::codex_auth::{CodexAuthStart, CodexOAuthConfig};
use crate::modules::codex_identity::{
    CodexIdentityClient, CodexIdentityError, VerifiedCodexIdentity,
};
use crate::modules::codex_login::{
    CodexLoginError, CodexLoginFlow, CodexLoginResult, CodexLoginService,
};
use crate::modules::codex_tokens::{CodexTokenError, CodexTokenSet, CodexTokenVault};
use crate::modules::secret_store::SecretStore;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;

pub const CODEX_OAUTH_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
pub const CODEX_AUTHORIZATION_ENDPOINT: &str = "https://auth.openai.com/oauth/authorize";
pub const CODEX_TOKEN_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
pub const CODEX_USERINFO_ENDPOINT: &str = "https://auth.openai.com/userinfo";
/// Codex OAuth only accepts the callback registered by the Codex CLI client.
/// Keep the listener on loopback, while advertising the canonical `localhost`
/// redirect URI from `CodexLoopbackListener::redirect_uri`.
pub const CODEX_CALLBACK_BIND_ADDRESS: &str = "127.0.0.1:1455";
pub const CODEX_UPSTREAM_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex";
pub const CODEX_USAGE_ENDPOINT: &str = "https://chatgpt.com/backend-api/wham/usage";
pub const CODEX_RESET_CREDITS_ENDPOINT: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const CODEX_CLIENT_VERSION: &str = "0.144.1";
const CODEX_USER_AGENT: &str = "codex_cli_rs/0.144.1 (MyProxy-Manager; auth-file)";
const MAX_CODEX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_CODEX_ERROR_BODY_CHARS: usize = 2_000;
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
    #[error("Codex credential was not found")]
    CredentialNotFound,
    #[error("Codex credential is disabled")]
    CredentialDisabled,
    #[error("Codex model discovery failed: {0}")]
    ModelDiscovery(String),
    #[error("Codex quota discovery failed: {0}")]
    QuotaDiscovery(String),
    #[error("Codex account identifier is unavailable")]
    AccountIdUnavailable,
    #[error("Codex auth-file export failed")]
    AuthFileExport,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CodexModelSummary {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexQuotaWindow {
    #[serde(
        default,
        alias = "usedPercent",
        deserialize_with = "deserialize_optional_f64"
    )]
    pub used_percent: Option<f64>,
    #[serde(
        default,
        alias = "limitWindowSeconds",
        deserialize_with = "deserialize_optional_i64"
    )]
    pub limit_window_seconds: Option<i64>,
    #[serde(
        default,
        alias = "resetAfterSeconds",
        deserialize_with = "deserialize_optional_i64"
    )]
    pub reset_after_seconds: Option<i64>,
    #[serde(
        default,
        alias = "resetAt",
        deserialize_with = "deserialize_optional_timestamp"
    )]
    pub reset_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexQuotaLimit {
    #[serde(default)]
    pub allowed: Option<bool>,
    #[serde(default, alias = "limitReached")]
    pub limit_reached: Option<bool>,
    #[serde(default, alias = "primaryWindow")]
    pub primary_window: Option<CodexQuotaWindow>,
    #[serde(default, alias = "secondaryWindow")]
    pub secondary_window: Option<CodexQuotaWindow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexAdditionalQuota {
    #[serde(default, alias = "limitName")]
    pub limit_name: Option<String>,
    #[serde(default, alias = "meteredFeature")]
    pub metered_feature: Option<String>,
    #[serde(default, alias = "rateLimit")]
    pub rate_limit: Option<CodexQuotaLimit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodexResetCredit {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, alias = "resetType")]
    pub reset_type: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(
        default,
        alias = "grantedAt",
        deserialize_with = "deserialize_optional_timestamp"
    )]
    pub granted_at: Option<i64>,
    #[serde(
        default,
        alias = "expiresAt",
        deserialize_with = "deserialize_optional_timestamp"
    )]
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CodexQuotaSummary {
    pub plan_type: Option<String>,
    pub subscription_expires_at: Option<i64>,
    pub rate_limit: Option<CodexQuotaLimit>,
    pub code_review_rate_limit: Option<CodexQuotaLimit>,
    pub additional_rate_limits: Vec<CodexAdditionalQuota>,
    pub reset_credits_available_count: Option<i64>,
    pub reset_credits_applicable_available_count: Option<i64>,
    pub reset_credits: Vec<CodexResetCredit>,
    pub reset_credits_error: Option<String>,
    pub fetched_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CodexImportRequest {
    pub content: String,
    #[serde(default)]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CodexExportRequest {
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CodexEnabledRequest {
    pub enabled: bool,
}

#[derive(Debug, Default, Deserialize)]
struct CodexQuotaPayload {
    #[serde(default, alias = "planType")]
    plan_type: Option<String>,
    #[serde(
        default,
        alias = "subscriptionExpiresAt",
        deserialize_with = "deserialize_optional_timestamp"
    )]
    subscription_expires_at: Option<i64>,
    #[serde(default, alias = "subscription")]
    subscription: Option<CodexSubscriptionPayload>,
    #[serde(default, alias = "rateLimit")]
    rate_limit: Option<CodexQuotaLimit>,
    #[serde(default, alias = "codeReviewRateLimit")]
    code_review_rate_limit: Option<CodexQuotaLimit>,
    #[serde(
        default,
        alias = "additionalRateLimits",
        deserialize_with = "deserialize_vec_or_null"
    )]
    additional_rate_limits: Vec<CodexAdditionalQuota>,
    #[serde(default, alias = "rateLimitResetCredits")]
    rate_limit_reset_credits: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
struct CodexSubscriptionPayload {
    #[serde(
        default,
        alias = "expiresAt",
        deserialize_with = "deserialize_optional_timestamp"
    )]
    expires_at: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
struct CodexResetCreditsPayload {
    #[serde(
        default,
        alias = "availableCount",
        deserialize_with = "deserialize_optional_i64"
    )]
    available_count: Option<i64>,
    #[serde(
        default,
        alias = "applicableAvailableCount",
        deserialize_with = "deserialize_optional_i64"
    )]
    applicable_available_count: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_vec_or_null")]
    credits: Vec<CodexResetCredit>,
}

fn deserialize_optional_value<'de, D>(
    deserializer: D,
) -> Result<Option<serde_json::Value>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<serde_json::Value>::deserialize(deserializer)
}

fn deserialize_vec_or_null<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

fn deserialize_optional_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = deserialize_optional_value(deserializer)?;
    Ok(value.and_then(|value| match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(value) => value.trim().parse::<f64>().ok(),
        _ => None,
    }))
}

fn deserialize_optional_i64<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = deserialize_optional_value(deserializer)?;
    Ok(value.and_then(|value| match value {
        serde_json::Value::Number(number) => number.as_i64(),
        serde_json::Value::String(value) => value.trim().parse::<i64>().ok(),
        _ => None,
    }))
}

fn deserialize_optional_timestamp<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = deserialize_optional_value(deserializer)?;
    Ok(value
        .and_then(|value| match value {
            serde_json::Value::Number(number) => number.as_i64(),
            serde_json::Value::String(value) => value.trim().parse::<i64>().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc3339(value.trim())
                    .ok()
                    .map(|date| date.timestamp())
            }),
            _ => None,
        })
        .map(|timestamp| {
            if timestamp > 10_000_000_000 {
                timestamp / 1_000
            } else {
                timestamp
            }
        }))
}

pub type ProductionCodexAccountRuntime = CodexAccountRuntime<CodexAuthStore>;

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
        let secret_store = CodexAuthStore::new(data_dir.join("auth-files"));
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
        let now = chrono::Utc::now().timestamp();
        let mut summaries = Vec::with_capacity(connections.len());
        for connection in connections {
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
                .ok_or(AccountPlatformStoreError::InvalidDocument)?;
            let mut credential_summary = CredentialSummary::from(credential);
            if !matches!(credential.lifecycle, CredentialLifecycle::Revoked) {
                match self.vault.read(credential.material.secret_ref()).await {
                    Ok(tokens) => {
                        credential_summary.expires_at = Some(tokens.expires_at());
                        credential_summary.lifecycle = if tokens.expires_at() <= now {
                            CredentialLifecycle::Expired
                        } else {
                            CredentialLifecycle::Ready
                        };
                    }
                    Err(_) => credential_summary.lifecycle = CredentialLifecycle::Unavailable,
                }
            }
            summaries.push(AccountConnectionSummary {
                identity,
                credential: credential_summary,
                connection,
            });
        }
        Ok(summaries)
    }

    pub async fn set_enabled(
        &self,
        credential_id: &str,
        enabled: bool,
    ) -> Result<(), CodexAccountRuntimeError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        self.store
            .set_connection_enabled(credential_id, enabled)
            .await
            .map_err(|error| match error {
                AccountPlatformStoreError::NotFound => CodexAccountRuntimeError::CredentialNotFound,
                other => CodexAccountRuntimeError::Store(other),
            })
    }

    pub async fn refresh_credential(
        &self,
        credential_id: &str,
    ) -> Result<(), CodexAccountRuntimeError> {
        let (_, credentials, _) = self.store.load().await?;
        let credential = credentials
            .into_iter()
            .find(|credential| credential.id == credential_id)
            .ok_or(CodexAccountRuntimeError::CredentialNotFound)?;
        self.login
            .refresh_now(
                credential.material.secret_ref(),
                chrono::Utc::now().timestamp(),
            )
            .await
            .map_err(CodexAccountRuntimeError::from)
    }

    pub async fn delete_credential(
        &self,
        credential_id: &str,
    ) -> Result<(), CodexAccountRuntimeError> {
        let _lifecycle = self.lifecycle_lock.lock().await;
        let (_, credentials, _) = self.store.load().await?;
        let credential = credentials
            .into_iter()
            .find(|credential| credential.id == credential_id)
            .ok_or(CodexAccountRuntimeError::CredentialNotFound)?;
        self.vault.delete(credential.material.secret_ref()).await?;
        self.store
            .remove_credential(credential_id)
            .await?
            .ok_or(CodexAccountRuntimeError::CredentialNotFound)?;
        Ok(())
    }

    pub async fn import_auth_file(
        &self,
        request: CodexImportRequest,
    ) -> Result<AccountConnectionSummary, CodexAccountRuntimeError> {
        let imported_metadata: serde_json::Value =
            serde_json::from_str(&request.content).map_err(|_| CodexTokenError::InvalidResponse)?;
        let imported_enabled = !imported_metadata
            .get("disabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let mut tokens =
            CodexTokenSet::from_auth_file_json(&request.content, chrono::Utc::now().timestamp())?;
        let verified = match tokens.id_token() {
            Some(id_token) => match self.identity.verify_id_token(id_token) {
                Ok(identity) => identity,
                Err(_) => {
                    self.verify_identity_from_access_token(
                        tokens.access_token(),
                        &tokio_util::sync::CancellationToken::new(),
                    )
                    .await?
                }
            },
            None => {
                self.verify_identity_from_access_token(
                    tokens.access_token(),
                    &tokio_util::sync::CancellationToken::new(),
                )
                .await?
            }
        };
        tokens.set_account_id(&verified.subject);
        let secret_ref = self.vault.create(&tokens).await?;
        let (identity, credential, connection) = create_records(
            verified,
            secret_ref.clone(),
            tokens.expires_at(),
            request.filename.as_deref(),
            imported_enabled,
        );
        let connection_id = connection.id.clone();
        let (existing_identities, _, _) = self.store.load().await?;
        if identity_subject_exists(&existing_identities, &identity) {
            let _ = self.vault.delete(&secret_ref).await;
            return Err(AccountPlatformStoreError::Conflict.into());
        }
        let append_result = self.store.append(identity, credential, connection).await;
        if let Err(error) = append_result {
            let _ = self.vault.delete(&secret_ref).await;
            return Err(error.into());
        }
        self.list_connections()
            .await?
            .into_iter()
            .find(|summary| summary.connection.id == connection_id)
            .ok_or(AccountPlatformStoreError::InvalidDocument.into())
    }

    pub async fn export_auth_file(
        &self,
        credential_id: &str,
        path: &str,
    ) -> Result<(), CodexAccountRuntimeError> {
        let (_, credentials, connections) = self.store.load().await?;
        let credential = credentials
            .into_iter()
            .find(|credential| credential.id == credential_id)
            .ok_or(CodexAccountRuntimeError::CredentialNotFound)?;
        let connection = connections
            .into_iter()
            .find(|connection| connection.credential_id == credential_id)
            .ok_or(CodexAccountRuntimeError::CredentialNotFound)?;
        let tokens = self.vault.read(credential.material.secret_ref()).await?;
        let encoded = tokens.to_auth_file_json_with_disabled(!connection.enabled)?;
        write_private_export(Path::new(path), encoded.as_str())
            .map_err(|_| CodexAccountRuntimeError::AuthFileExport)
    }

    pub async fn list_models(
        &self,
        credential_id: &str,
    ) -> Result<Vec<CodexModelSummary>, CodexAccountRuntimeError> {
        let (access_token, account_id) = self.auth_for_account_operation(credential_id).await?;
        let account_id = self
            .ensure_account_id(credential_id, &access_token, account_id)
            .await?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| CodexAccountRuntimeError::ModelDiscovery(error.to_string()))?;
        let response = codex_authenticated_request(
            client.get(format!(
                "{CODEX_UPSTREAM_ENDPOINT}/models?client_version={CODEX_CLIENT_VERSION}"
            )),
            &access_token,
            &account_id,
            "codex_cli_rs",
        )
        .send()
        .await
        .map_err(|error| CodexAccountRuntimeError::ModelDiscovery(error.to_string()))?;
        let status = response.status();
        let body = read_codex_response(response)
            .await
            .map_err(CodexAccountRuntimeError::ModelDiscovery)?;
        if !status.is_success() {
            return Err(CodexAccountRuntimeError::ModelDiscovery(format!(
                "HTTP {}: {}",
                status.as_u16(),
                truncate_codex_error_body(&body)
            )));
        }
        let payload = serde_json::from_str::<serde_json::Value>(&body).map_err(|_| {
            CodexAccountRuntimeError::ModelDiscovery("invalid JSON response".into())
        })?;
        let models = crate::proxy::provider_discovery::parse_discovered_models(&payload)
            .into_iter()
            .map(|model| CodexModelSummary {
                id: model.id,
                display_name: model.name,
                owned_by: model.owned_by,
            })
            .collect::<Vec<_>>();
        if models.is_empty() {
            return Err(CodexAccountRuntimeError::ModelDiscovery(
                "provider returned no models".into(),
            ));
        }
        Ok(models)
    }

    pub async fn get_quota(
        &self,
        credential_id: &str,
    ) -> Result<CodexQuotaSummary, CodexAccountRuntimeError> {
        let (access_token, account_id) = self.auth_for_account_operation(credential_id).await?;
        let account_id = self
            .ensure_account_id(credential_id, &access_token, account_id)
            .await?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| CodexAccountRuntimeError::QuotaDiscovery(error.to_string()))?;
        let response = codex_authenticated_request(
            client.get(CODEX_USAGE_ENDPOINT),
            &access_token,
            &account_id,
            "Codex Desktop",
        )
        .send()
        .await
        .map_err(|error| CodexAccountRuntimeError::QuotaDiscovery(error.to_string()))?;
        let status = response.status();
        let body = read_codex_response(response)
            .await
            .map_err(CodexAccountRuntimeError::QuotaDiscovery)?;
        if !status.is_success() {
            return Err(CodexAccountRuntimeError::QuotaDiscovery(format!(
                "HTTP {}: {}",
                status.as_u16(),
                truncate_codex_error_body(&body)
            )));
        }
        let payload = serde_json::from_str::<CodexQuotaPayload>(&body).map_err(|error| {
            CodexAccountRuntimeError::QuotaDiscovery(format!("invalid JSON response: {error}"))
        })?;
        let mut summary = quota_summary_from_payload(payload, chrono::Utc::now().timestamp());

        let reset_response = codex_authenticated_request(
            client.get(CODEX_RESET_CREDITS_ENDPOINT),
            &access_token,
            &account_id,
            "Codex Desktop",
        )
        .send()
        .await;
        match reset_response {
            Ok(response) => {
                let status = response.status();
                match read_codex_response(response).await {
                    Ok(body) if status.is_success() => {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
                            if let Some(reset_credits) = parse_reset_credits(&value) {
                                summary.reset_credits_available_count =
                                    reset_credits.available_count;
                                summary.reset_credits_applicable_available_count =
                                    reset_credits.applicable_available_count;
                                summary.reset_credits = reset_credits.credits;
                                summary.reset_credits_error = None;
                            }
                        }
                    }
                    Ok(body) => {
                        summary.reset_credits_error = Some(format!(
                            "HTTP {}: {}",
                            status.as_u16(),
                            truncate_codex_error_body(&body)
                        ));
                    }
                    Err(error) => summary.reset_credits_error = Some(error),
                }
            }
            Err(error) => summary.reset_credits_error = Some(error.to_string()),
        }
        Ok(summary)
    }

    /// Resolve a Codex credential for the proxy request path.
    ///
    /// The auth-file remains the source of truth. The token is only returned
    /// to the in-process request builder and is never serialized or logged.
    pub async fn access_token_for_credential(
        &self,
        credential_id: &str,
    ) -> Result<String, CodexAccountRuntimeError> {
        self.auth_for_credential(credential_id)
            .await
            .map(|(access_token, _)| access_token)
    }

    async fn auth_for_credential(
        &self,
        credential_id: &str,
    ) -> Result<(String, Option<String>), CodexAccountRuntimeError> {
        self.auth_for_credential_with_enabled(credential_id, true)
            .await
    }

    async fn auth_for_account_operation(
        &self,
        credential_id: &str,
    ) -> Result<(String, Option<String>), CodexAccountRuntimeError> {
        self.auth_for_credential_with_enabled(credential_id, false)
            .await
    }

    async fn auth_for_credential_with_enabled(
        &self,
        credential_id: &str,
        require_enabled: bool,
    ) -> Result<(String, Option<String>), CodexAccountRuntimeError> {
        let (_, credentials, connections) = self.store.load().await?;
        let credential = credentials
            .into_iter()
            .find(|credential| credential.id == credential_id)
            .ok_or(CodexAccountRuntimeError::CredentialNotFound)?;
        let connection = connections
            .into_iter()
            .find(|connection| connection.credential_id == credential_id)
            .ok_or(CodexAccountRuntimeError::CredentialNotFound)?;
        if require_enabled && !connection.enabled {
            return Err(CodexAccountRuntimeError::CredentialDisabled);
        }
        let secret_ref = credential.material.secret_ref().clone();
        self.login
            .refresh_if_expiring(&secret_ref, chrono::Utc::now().timestamp(), 300)
            .await?;
        let token = self.vault.read(&secret_ref).await?;
        Ok((
            token.access_token().to_string(),
            token.account_id().map(str::to_string),
        ))
    }

    async fn ensure_account_id(
        &self,
        credential_id: &str,
        access_token: &str,
        account_id: Option<String>,
    ) -> Result<String, CodexAccountRuntimeError> {
        if let Some(account_id) = account_id.filter(|value| !value.trim().is_empty()) {
            return Ok(account_id);
        }
        let verified = self.identity.verify(access_token).await?;
        let (_, credentials, _) = self.store.load().await?;
        let credential = credentials
            .into_iter()
            .find(|credential| credential.id == credential_id)
            .ok_or(CodexAccountRuntimeError::CredentialNotFound)?;
        let secret_ref = credential.material.secret_ref().clone();
        let mut token = self.vault.read(&secret_ref).await?;
        token.set_account_id(&verified.subject);
        self.vault.replace(&secret_ref, &token).await?;
        Ok(verified.subject)
    }

    pub async fn access_token_for_credential_if_configured(
        &self,
        mut provider: crate::proxy::config::UpstreamProvider,
    ) -> Result<crate::proxy::config::UpstreamProvider, String> {
        if let Some(credential_id) = provider.credential_id.as_deref() {
            let (access_token, account_id) = self
                .auth_for_credential(credential_id)
                .await
                .map_err(|error| format!("Provider credential is unavailable: {error}"))?;
            provider.api_key = access_token;
            provider.account_id = account_id;
        }
        Ok(provider)
    }

    async fn verify_identity_from_access_token(
        &self,
        access_token: &str,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<VerifiedCodexIdentity, CodexAccountRuntimeError> {
        tokio::select! {
            _ = cancellation.cancelled() => {
                Err(CodexAccountRuntimeError::Login(CodexLoginError::Cancelled))
            }
            result = self.identity.verify(access_token) => {
                result.map_err(CodexAccountRuntimeError::from)
            }
        }
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
            let mut tokens = self.vault.read(&login.secret_ref).await?;
            let verified = match tokens.id_token() {
                Some(id_token) => match self.identity.verify_id_token(id_token) {
                    Ok(identity) => identity,
                    Err(_) => {
                        self.verify_identity_from_access_token(tokens.access_token(), cancellation)
                            .await?
                    }
                },
                None => {
                    self.verify_identity_from_access_token(tokens.access_token(), cancellation)
                        .await?
                }
            };
            if tokens.account_id() != Some(verified.subject.as_str()) {
                tokens.set_account_id(&verified.subject);
                self.vault.replace(&login.secret_ref, &tokens).await?;
            }
            let (identity, credential, connection) = create_records(
                verified,
                login.secret_ref.clone(),
                login.expires_at,
                None,
                true,
            );
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

fn codex_authenticated_request(
    request: reqwest::RequestBuilder,
    access_token: &str,
    account_id: &str,
    originator: &str,
) -> reqwest::RequestBuilder {
    request
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("ChatGPT-Account-Id", account_id)
        .header("OpenAI-Beta", "codex-1")
        .header("Originator", originator)
        .header(reqwest::header::USER_AGENT, CODEX_USER_AGENT)
        .bearer_auth(access_token)
}

async fn read_codex_response(response: reqwest::Response) -> Result<String, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CODEX_RESPONSE_BYTES as u64)
    {
        return Err("upstream response is too large".to_string());
    }
    let body = response
        .bytes()
        .await
        .map_err(|error| format!("failed to read upstream response: {error}"))?;
    if body.len() > MAX_CODEX_RESPONSE_BYTES {
        return Err("upstream response is too large".to_string());
    }
    String::from_utf8(body.to_vec()).map_err(|_| "upstream response is not valid UTF-8".to_string())
}

fn truncate_codex_error_body(body: &str) -> String {
    let mut truncated = body
        .chars()
        .take(MAX_CODEX_ERROR_BODY_CHARS)
        .collect::<String>();
    if body.chars().count() > MAX_CODEX_ERROR_BODY_CHARS {
        truncated.push('…');
    }
    if truncated.trim().is_empty() {
        "empty response".to_string()
    } else {
        truncated
    }
}

fn quota_summary_from_payload(payload: CodexQuotaPayload, fetched_at: i64) -> CodexQuotaSummary {
    let embedded_reset_credits = payload
        .rate_limit_reset_credits
        .as_ref()
        .and_then(parse_reset_credits);
    let subscription_expires_at = payload
        .subscription_expires_at
        .or_else(|| payload.subscription.and_then(|value| value.expires_at));
    let (available_count, applicable_available_count, reset_credits) = embedded_reset_credits
        .map(|value| {
            (
                value.available_count,
                value.applicable_available_count,
                value.credits,
            )
        })
        .unwrap_or((None, None, Vec::new()));
    CodexQuotaSummary {
        plan_type: payload.plan_type,
        subscription_expires_at,
        rate_limit: payload.rate_limit,
        code_review_rate_limit: payload.code_review_rate_limit,
        additional_rate_limits: payload.additional_rate_limits,
        reset_credits_available_count: available_count,
        reset_credits_applicable_available_count: applicable_available_count,
        reset_credits,
        reset_credits_error: None,
        fetched_at,
    }
}

fn parse_reset_credits(value: &serde_json::Value) -> Option<CodexResetCreditsPayload> {
    let candidate = value.get("data").unwrap_or(value);
    let mut payload = if candidate.is_array() {
        CodexResetCreditsPayload {
            credits: serde_json::from_value(candidate.clone()).ok()?,
            ..Default::default()
        }
    } else {
        serde_json::from_value(candidate.clone()).ok()?
    };
    payload.credits.retain(|credit| {
        let status_available = credit
            .status
            .as_deref()
            .is_none_or(|status| status.eq_ignore_ascii_case("available"));
        let is_codex_rate_limit = credit
            .reset_type
            .as_deref()
            .is_none_or(|reset_type| reset_type.eq_ignore_ascii_case("codex_rate_limits"));
        status_available && is_codex_rate_limit
    });
    Some(payload)
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
    auth_file_name: Option<&str>,
    enabled: bool,
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
    let display_email = verified.email.clone();
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
        credential_id.clone(),
        CODEX_UPSTREAM_ENDPOINT,
        vec![WireProtocol::CodexResponsesUpstream],
    );
    connection.identity_id = Some(identity_id);
    connection.status = if enabled {
        ConnectionStatus::Ready
    } else {
        ConnectionStatus::Disabled
    };
    connection.enabled = enabled;
    connection.config.insert(
        "auth_file_name".to_string(),
        serde_json::Value::String(auth_file_display_name(
            auth_file_name,
            display_email.as_deref(),
            &credential_id,
        )),
    );
    (identity, credential, connection)
}

fn identity_subject_exists(identities: &[Identity], candidate: &Identity) -> bool {
    candidate.subject.as_deref().is_some_and(|subject| {
        identities
            .iter()
            .any(|identity| identity.subject.as_deref() == Some(subject))
    })
}

fn auth_file_display_name(
    source: Option<&str>,
    email: Option<&str>,
    credential_id: &str,
) -> String {
    let fallback = format!("codex-{credential_id}.json");
    let candidate = source
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| email.map(|value| format!("codex-{value}.json")))
        .unwrap_or_else(|| fallback.clone());
    let candidate = candidate.rsplit(['/', '\\']).next().unwrap_or(&candidate);
    let mut safe = candidate
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '@')
        })
        .take(120)
        .collect::<String>();
    if safe.is_empty() {
        safe = fallback;
    }
    if !safe.ends_with(".json") {
        safe.push_str(".json");
    }
    safe
}

fn write_private_export(path: &Path, content: &str) -> std::io::Result<()> {
    if path.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "export path is empty",
        ));
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "export directory does not exist",
        ));
    }
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "export path is not a regular file",
            ));
        }
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(content.as_bytes())?;
    temporary.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
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
        CodexAccountRuntimeError::CredentialNotFound => "credential_not_found",
        CodexAccountRuntimeError::CredentialDisabled => "credential_disabled",
        CodexAccountRuntimeError::ModelDiscovery(_) => "model_discovery_failed",
        CodexAccountRuntimeError::QuotaDiscovery(_) => "quota_discovery_failed",
        CodexAccountRuntimeError::AccountIdUnavailable => "account_id_unavailable",
        CodexAccountRuntimeError::AuthFileExport => "auth_file_export_failed",
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

    #[test]
    fn default_callback_binding_matches_codex_oauth_registration() {
        assert_eq!(CODEX_CALLBACK_BIND_ADDRESS, "127.0.0.1:1455");
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
        let credential_id = credentials[0].id.clone();
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
        assert_eq!(
            runtime
                .access_token_for_credential(&credential_id)
                .await
                .unwrap(),
            "access-canary"
        );
    }

    #[tokio::test]
    async fn verified_login_uses_id_token_when_userinfo_is_unavailable() {
        let secrets = MemorySecretStore::default();
        let vault = CodexTokenVault::new(secrets.clone());
        let token_set = CodexTokenSet::new(
            "access-canary",
            "refresh-canary",
            Some(test_id_token()),
            "Bearer",
            2_000_000_000,
        )
        .unwrap();
        let secret_ref = vault.create(&token_set).await.unwrap();
        let directory = tempfile::tempdir().unwrap();
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
            AccountPlatformStore::new(directory.path().join("accounts-v3.json")),
            "127.0.0.1:0",
        );

        runtime
            .persist_verified_login(CodexLoginResult {
                secret_ref,
                expires_at: 2_000_000_000,
            })
            .await
            .unwrap();

        let summaries = runtime.list_connections().await.unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(
            summaries[0].identity.email.as_deref(),
            Some("codex@example.com")
        );
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

    #[test]
    fn quota_payload_accepts_codex_field_aliases_and_filters_reset_credits() {
        let payload: CodexQuotaPayload = serde_json::from_value(json!({
            "planType": "plus",
            "rateLimit": {
                "allowed": true,
                "limitReached": false,
                "primaryWindow": {
                    "usedPercent": 42,
                    "resetAt": 1_900_000_000,
                    "resetAfterSeconds": "3600"
                }
            },
            "codeReviewRateLimit": {
                "secondary_window": {"used_percent": 5}
            },
            "rateLimitResetCredits": {
                "availableCount": "2",
                "applicableAvailableCount": 1,
                "credits": [
                    {"id": "available", "resetType": "codex_rate_limits", "status": "available", "expiresAt": 1_900_000_000},
                    {"id": "consumed", "resetType": "codex_rate_limits", "status": "consumed"},
                    {"id": "other", "resetType": "other", "status": "available"}
                ]
            }
        }))
        .unwrap();

        let summary = quota_summary_from_payload(payload, 1_700_000_000);
        assert_eq!(summary.plan_type.as_deref(), Some("plus"));
        assert_eq!(
            summary
                .rate_limit
                .as_ref()
                .unwrap()
                .primary_window
                .as_ref()
                .unwrap()
                .used_percent,
            Some(42.0)
        );
        assert_eq!(summary.reset_credits_available_count, Some(2));
        assert_eq!(summary.reset_credits_applicable_available_count, Some(1));
        assert_eq!(summary.reset_credits.len(), 1);
        assert_eq!(summary.reset_credits[0].id.as_deref(), Some("available"));
    }

    #[test]
    fn quota_payload_accepts_null_collection_fields() {
        let payload: CodexQuotaPayload = serde_json::from_value(json!({
            "additionalRateLimits": null,
            "rateLimitResetCredits": {
                "availableCount": null,
                "applicableAvailableCount": null,
                "credits": null
            }
        }))
        .expect("nullable quota collections should deserialize");

        assert!(payload.additional_rate_limits.is_empty());
        let reset_credits = parse_reset_credits(
            payload
                .rate_limit_reset_credits
                .as_ref()
                .expect("reset credits payload should be present"),
        )
        .expect("reset credits object should deserialize");
        assert!(reset_credits.credits.is_empty());
    }

    fn test_id_token() -> String {
        use base64::Engine;

        let encode = |value: serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&value).unwrap())
        };
        format!(
            "{}.{}.signature",
            encode(json!({"alg": "RS256", "typ": "JWT"})),
            encode(json!({
                "sub": "codex-subject",
                "email": "codex@example.com",
                "name": "Codex User",
                "https://api.openai.com/auth": {
                    "chatgpt_account_id": "account-123",
                    "chatgpt_plan_type": "plus",
                    "organizations": [{"id": "workspace-1", "is_default": true}]
                }
            }))
        )
    }
}
