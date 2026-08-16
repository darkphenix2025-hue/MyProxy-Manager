use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub const ACCOUNT_PLATFORM_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    pub provider_namespace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

impl Identity {
    pub fn new(id: impl Into<String>, provider_namespace: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            provider_namespace: provider_namespace.into(),
            subject: None,
            display_name: None,
            email: None,
            workspace: None,
            metadata: BTreeMap::new(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct SecretRef {
    pub backend: String,
    pub key: String,
}

impl SecretRef {
    pub fn new(backend: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            backend: backend.into(),
            key: key.into(),
        }
    }
}

impl fmt::Debug for SecretRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SecretRef")
            .field("backend", &self.backend)
            .field("key", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    OpenAiApiKey,
    CodexOauthTokenSet,
    ExternalCliRef,
    AwsProfileRef,
    BearerTokenRef,
    GoogleOauthRefreshLegacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialLifecycle {
    PendingValidation,
    Ready,
    Expired,
    Revoked,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CredentialMaterial {
    OpenAiApiKey(SecretRef),
    CodexOauthTokenSet(SecretRef),
    ExternalCliRef(SecretRef),
    AwsProfileRef(SecretRef),
    BearerTokenRef(SecretRef),
    GoogleOauthRefreshLegacy(SecretRef),
}

impl CredentialMaterial {
    pub fn auth_kind(&self) -> AuthKind {
        match self {
            Self::OpenAiApiKey(_) => AuthKind::OpenAiApiKey,
            Self::CodexOauthTokenSet(_) => AuthKind::CodexOauthTokenSet,
            Self::ExternalCliRef(_) => AuthKind::ExternalCliRef,
            Self::AwsProfileRef(_) => AuthKind::AwsProfileRef,
            Self::BearerTokenRef(_) => AuthKind::BearerTokenRef,
            Self::GoogleOauthRefreshLegacy(_) => AuthKind::GoogleOauthRefreshLegacy,
        }
    }

    pub fn secret_ref(&self) -> &SecretRef {
        match self {
            Self::OpenAiApiKey(value)
            | Self::CodexOauthTokenSet(value)
            | Self::ExternalCliRef(value)
            | Self::AwsProfileRef(value)
            | Self::BearerTokenRef(value)
            | Self::GoogleOauthRefreshLegacy(value) => value,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Credential {
    pub id: String,
    pub identity_id: Option<String>,
    pub material: CredentialMaterial,
    pub lifecycle: CredentialLifecycle,
    pub expires_at: Option<i64>,
    pub scopes: Vec<String>,
    pub fingerprint: String,
}

impl Credential {
    pub fn new(
        id: impl Into<String>,
        identity_id: Option<String>,
        auth_kind: AuthKind,
        secret_ref: SecretRef,
        fingerprint: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            identity_id,
            material: match auth_kind {
                AuthKind::OpenAiApiKey => CredentialMaterial::OpenAiApiKey(secret_ref),
                AuthKind::CodexOauthTokenSet => CredentialMaterial::CodexOauthTokenSet(secret_ref),
                AuthKind::ExternalCliRef => CredentialMaterial::ExternalCliRef(secret_ref),
                AuthKind::AwsProfileRef => CredentialMaterial::AwsProfileRef(secret_ref),
                AuthKind::BearerTokenRef => CredentialMaterial::BearerTokenRef(secret_ref),
                AuthKind::GoogleOauthRefreshLegacy => {
                    CredentialMaterial::GoogleOauthRefreshLegacy(secret_ref)
                }
            },
            lifecycle: CredentialLifecycle::PendingValidation,
            expires_at: None,
            scopes: Vec::new(),
            fingerprint: fingerprint.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialSummary {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<String>,
    pub auth_kind: AuthKind,
    pub lifecycle: CredentialLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    pub fingerprint: String,
}

impl From<&Credential> for CredentialSummary {
    fn from(credential: &Credential) -> Self {
        Self {
            id: credential.id.clone(),
            identity_id: credential.identity_id.clone(),
            auth_kind: credential.material.auth_kind(),
            lifecycle: credential.lifecycle,
            expires_at: credential.expires_at,
            fingerprint: mask_fingerprint(&credential.fingerprint),
        }
    }
}

fn mask_fingerprint(value: &str) -> String {
    let suffix: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let prefix = value.split_once(':').map(|(prefix, _)| prefix);
    match (prefix, suffix.is_empty()) {
        (Some(prefix), false) => format!("{prefix}:…{suffix}"),
        (None, false) => format!("…{suffix}"),
        _ => "[REDACTED]".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredSecretRefV3 {
    backend: String,
    key: String,
}

impl From<&SecretRef> for StoredSecretRefV3 {
    fn from(value: &SecretRef) -> Self {
        Self {
            backend: value.backend.clone(),
            key: value.key.clone(),
        }
    }
}

impl From<StoredSecretRefV3> for SecretRef {
    fn from(value: StoredSecretRefV3) -> Self {
        Self::new(value.backend, value.key)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "auth_kind", rename_all = "snake_case")]
enum StoredCredentialMaterialV3 {
    OpenAiApiKey { secret_ref: StoredSecretRefV3 },
    CodexOauthTokenSet { secret_ref: StoredSecretRefV3 },
    ExternalCliRef { secret_ref: StoredSecretRefV3 },
    AwsProfileRef { secret_ref: StoredSecretRefV3 },
    BearerTokenRef { secret_ref: StoredSecretRefV3 },
    GoogleOauthRefreshLegacy { secret_ref: StoredSecretRefV3 },
}

impl From<&CredentialMaterial> for StoredCredentialMaterialV3 {
    fn from(value: &CredentialMaterial) -> Self {
        let secret_ref = StoredSecretRefV3::from(value.secret_ref());
        match value {
            CredentialMaterial::OpenAiApiKey(_) => Self::OpenAiApiKey { secret_ref },
            CredentialMaterial::CodexOauthTokenSet(_) => Self::CodexOauthTokenSet { secret_ref },
            CredentialMaterial::ExternalCliRef(_) => Self::ExternalCliRef { secret_ref },
            CredentialMaterial::AwsProfileRef(_) => Self::AwsProfileRef { secret_ref },
            CredentialMaterial::BearerTokenRef(_) => Self::BearerTokenRef { secret_ref },
            CredentialMaterial::GoogleOauthRefreshLegacy(_) => {
                Self::GoogleOauthRefreshLegacy { secret_ref }
            }
        }
    }
}

impl From<StoredCredentialMaterialV3> for CredentialMaterial {
    fn from(value: StoredCredentialMaterialV3) -> Self {
        match value {
            StoredCredentialMaterialV3::OpenAiApiKey { secret_ref } => {
                Self::OpenAiApiKey(secret_ref.into())
            }
            StoredCredentialMaterialV3::CodexOauthTokenSet { secret_ref } => {
                Self::CodexOauthTokenSet(secret_ref.into())
            }
            StoredCredentialMaterialV3::ExternalCliRef { secret_ref } => {
                Self::ExternalCliRef(secret_ref.into())
            }
            StoredCredentialMaterialV3::AwsProfileRef { secret_ref } => {
                Self::AwsProfileRef(secret_ref.into())
            }
            StoredCredentialMaterialV3::BearerTokenRef { secret_ref } => {
                Self::BearerTokenRef(secret_ref.into())
            }
            StoredCredentialMaterialV3::GoogleOauthRefreshLegacy { secret_ref } => {
                Self::GoogleOauthRefreshLegacy(secret_ref.into())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredCredentialV3 {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    identity_id: Option<String>,
    #[serde(flatten)]
    material: StoredCredentialMaterialV3,
    lifecycle: CredentialLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    scopes: Vec<String>,
    fingerprint: String,
}

impl From<&Credential> for StoredCredentialV3 {
    fn from(value: &Credential) -> Self {
        Self {
            id: value.id.clone(),
            identity_id: value.identity_id.clone(),
            material: StoredCredentialMaterialV3::from(&value.material),
            lifecycle: value.lifecycle,
            expires_at: value.expires_at,
            scopes: value.scopes.clone(),
            fingerprint: value.fingerprint.clone(),
        }
    }
}

impl From<StoredCredentialV3> for Credential {
    fn from(value: StoredCredentialV3) -> Self {
        Self {
            id: value.id,
            identity_id: value.identity_id,
            material: value.material.into(),
            lifecycle: value.lifecycle,
            expires_at: value.expires_at,
            scopes: value.scopes,
            fingerprint: value.fingerprint,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireProtocol {
    OpenAiResponses,
    OpenAiChatCompletions,
    AnthropicMessages,
    GeminiGenerativeLanguage,
    GeminiV1Internal,
    CodexResponsesUpstream,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Draft,
    Validating,
    Ready,
    Degraded,
    Unavailable,
    Disabled,
    MigrationAttention,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderConnection {
    pub id: String,
    pub provider_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_id: Option<String>,
    pub credential_id: String,
    pub endpoint: String,
    pub protocols: Vec<WireProtocol>,
    pub status: ConnectionStatus,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, serde_json::Value>,
}

impl ProviderConnection {
    pub fn new(
        id: impl Into<String>,
        provider_kind: impl Into<String>,
        credential_id: impl Into<String>,
        endpoint: impl Into<String>,
        protocols: Vec<WireProtocol>,
    ) -> Self {
        Self {
            id: id.into(),
            provider_kind: provider_kind.into(),
            identity_id: None,
            credential_id: credential_id.into(),
            endpoint: endpoint.into(),
            protocols,
            status: ConnectionStatus::Draft,
            enabled: false,
            config: BTreeMap::new(),
        }
    }
}

fn deserialize_v3_schema_version<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = u32::deserialize(deserializer)?;
    if version == ACCOUNT_PLATFORM_SCHEMA_VERSION {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(format!(
            "expected account platform schema_version 3, found {version}"
        )))
    }
}

/// Versioned persistence boundary for the provider-neutral account platform.
///
/// Runtime credentials are deliberately not serializable. Secret references can only cross the
/// persistence boundary through this explicit, version-checked document type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountPlatformDocumentV3 {
    #[serde(deserialize_with = "deserialize_v3_schema_version")]
    schema_version: u32,
    identities: Vec<Identity>,
    credentials: Vec<StoredCredentialV3>,
    connections: Vec<ProviderConnection>,
}

impl AccountPlatformDocumentV3 {
    pub fn new(
        identities: Vec<Identity>,
        credentials: Vec<Credential>,
        connections: Vec<ProviderConnection>,
    ) -> Self {
        Self {
            schema_version: ACCOUNT_PLATFORM_SCHEMA_VERSION,
            identities,
            credentials: credentials.iter().map(StoredCredentialV3::from).collect(),
            connections,
        }
    }

    pub fn into_parts(self) -> (Vec<Identity>, Vec<Credential>, Vec<ProviderConnection>) {
        (
            self.identities,
            self.credentials.into_iter().map(Credential::from).collect(),
            self.connections,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_version_is_v3() {
        assert_eq!(ACCOUNT_PLATFORM_SCHEMA_VERSION, 3);
    }

    #[test]
    fn persisted_document_serializes_numeric_v3_and_tagged_credential() {
        let credential = Credential::new(
            "credential-1",
            None,
            AuthKind::CodexOauthTokenSet,
            SecretRef::new("keyring", "codex-entry"),
            "sha256:abcdef",
        );
        let document = AccountPlatformDocumentV3::new(vec![], vec![credential], vec![]);

        let value = serde_json::to_value(document).expect("v3 document should serialize");
        assert_eq!(value["schema_version"], json!(3));
        assert_eq!(
            value["credentials"][0]["auth_kind"],
            json!("codex_oauth_token_set")
        );
        assert_eq!(
            value["credentials"][0]["secret_ref"]["key"],
            json!("codex-entry")
        );
    }

    #[test]
    fn persisted_document_rejects_wrong_schema_version() {
        let error = serde_json::from_value::<AccountPlatformDocumentV3>(json!({
            "schema_version": 2,
            "identities": [],
            "credentials": [],
            "connections": []
        }))
        .expect_err("v2 must not deserialize as v3");

        assert!(error.to_string().contains("schema_version 3"));
    }

    #[test]
    fn identity_does_not_require_email() {
        let identity = Identity::new("identity-1", "openai");

        assert_eq!(identity.id, "identity-1");
        assert_eq!(identity.provider_namespace, "openai");
        assert!(identity.email.is_none());
    }

    #[test]
    fn connection_supports_multiple_wire_protocols() {
        let connection = ProviderConnection::new(
            "connection-1",
            "openai",
            "credential-1",
            "https://api.openai.com/v1",
            vec![
                WireProtocol::OpenAiResponses,
                WireProtocol::OpenAiChatCompletions,
            ],
        );

        assert_eq!(connection.protocols.len(), 2);
        assert_eq!(connection.status, ConnectionStatus::Draft);
    }

    #[test]
    fn wire_protocol_names_are_stable_and_unknown_values_are_tolerated() {
        assert_eq!(
            serde_json::to_value(WireProtocol::GeminiV1Internal).unwrap(),
            json!("gemini_v1_internal")
        );
        assert_eq!(
            serde_json::from_value::<WireProtocol>(json!("future_protocol")).unwrap(),
            WireProtocol::Unknown
        );
    }

    #[test]
    fn credential_summary_never_serializes_secret_reference_or_tokens() {
        let credential = Credential::new(
            "credential-1",
            Some("identity-1".to_string()),
            AuthKind::OpenAiApiKey,
            SecretRef::new("keyring", "entry-42"),
            "sha256:0123456789abcdef",
        );

        let summary = CredentialSummary::from(&credential);
        let value = serde_json::to_value(summary).expect("summary should serialize");
        let encoded = value.to_string();

        assert_eq!(value["fingerprint"], json!("sha256:…cdef"));
        assert!(!encoded.contains("secret_ref"));
        assert!(!encoded.contains("entry-42"));
        assert!(!encoded.contains("access_token"));
        assert!(!encoded.contains("refresh_token"));
    }

    #[test]
    fn secret_reference_debug_output_is_redacted() {
        let secret_ref = SecretRef::new("keyring", "private-entry-name");

        let rendered = format!("{secret_ref:?}");
        assert!(rendered.contains("keyring"));
        assert!(!rendered.contains("private-entry-name"));
    }

    #[test]
    fn credential_lifecycle_defaults_to_pending_validation() {
        let credential = Credential::new(
            "credential-1",
            None,
            AuthKind::CodexOauthTokenSet,
            SecretRef::new("keyring", "codex-1"),
            "sha256:abcdef",
        );

        assert_eq!(credential.lifecycle, CredentialLifecycle::PendingValidation);
    }
}
