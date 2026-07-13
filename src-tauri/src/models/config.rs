use crate::modules::cloudflared::CloudflaredConfig;
use crate::proxy::ProxyConfig;
use serde::{Deserialize, Serialize};

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub language: String,
    pub theme: String,
    pub auto_refresh: bool,
    pub refresh_interval: i32, // minutes
    pub auto_sync: bool,
    pub sync_interval: i32, // minutes
    pub default_export_path: Option<String>,
    #[serde(default)]
    pub proxy: ProxyConfig,
    #[serde(default)]
    pub auto_launch: bool, // Launch on startup
    #[serde(default)]
    pub scheduled_warmup: ScheduledWarmupConfig, // [NEW] Scheduled warmup configuration
    #[serde(default)]
    pub quota_protection: QuotaProtectionConfig, // [NEW] Quota protection configuration
    #[serde(default)]
    pub pinned_quota_models: PinnedQuotaModelsConfig, // [NEW] Pinned quota models list
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerConfig, // [NEW] Circuit breaker configuration
    #[serde(default)]
    pub hidden_menu_items: Vec<String>, // Hidden menu item path list
    #[serde(default)]
    pub cloudflared: CloudflaredConfig, // [NEW] Cloudflared configuration
    /// Google OAuth client credentials (optional, fallback to env vars)
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    /// OAuth client secret — encrypted at rest via custom serializer
    #[serde(default, with = "oauth_secret_serde")]
    pub oauth_client_secret: Option<String>,
}

/// Scheduled warmup configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledWarmupConfig {
    /// Whether smart warmup is enabled
    pub enabled: bool,

    /// List of models to warmup
    #[serde(default = "default_warmup_models")]
    pub monitored_models: Vec<String>,
}

fn default_warmup_models() -> Vec<String> {
    vec![
        "gemini-3-flash".to_string(),
        "claude".to_string(),
        "gemini-3-pro-high".to_string(),
        "gemini-3-pro-image".to_string(),
    ]
}

impl ScheduledWarmupConfig {
    pub fn new() -> Self {
        Self {
            enabled: false,
            monitored_models: default_warmup_models(),
        }
    }
}

impl Default for ScheduledWarmupConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Quota protection configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaProtectionConfig {
    /// Whether quota protection is enabled
    pub enabled: bool,

    /// Reserved quota percentage (1-99)
    pub threshold_percentage: u32,

    /// List of monitored models (e.g. gemini-3-flash, gemini-3-pro-high, gemini-3.1-pro-high, claude-sonnet-4-6)
    #[serde(default = "default_monitored_models")]
    pub monitored_models: Vec<String>,
}

fn default_monitored_models() -> Vec<String> {
    vec![
        "claude".to_string(),
        "gemini-3-pro-high".to_string(),
        "gemini-3-flash".to_string(),
        "gemini-3-pro-image".to_string(),
    ]
}

impl QuotaProtectionConfig {
    pub fn new() -> Self {
        Self {
            enabled: false,
            threshold_percentage: 10, // Default 10% reserve
            monitored_models: default_monitored_models(),
        }
    }
}

impl Default for QuotaProtectionConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Pinned quota models configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinnedQuotaModelsConfig {
    /// List of pinned models (displayed outside the account list)
    #[serde(default = "default_pinned_models")]
    pub models: Vec<String>,
}

fn default_pinned_models() -> Vec<String> {
    vec![
        "gemini-3-pro-high".to_string(),
        "gemini-3-flash".to_string(),
        "gemini-3-pro-image".to_string(),
        "claude-sonnet-4-6-thinking".to_string(),
    ]
}

impl PinnedQuotaModelsConfig {
    pub fn new() -> Self {
        Self {
            models: default_pinned_models(),
        }
    }
}

impl Default for PinnedQuotaModelsConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Circuit breaker configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Whether circuit breaker is enabled
    pub enabled: bool,

    /// Unified backoff steps (seconds)
    /// Default: [60, 300, 1800, 7200]
    #[serde(default = "default_backoff_steps")]
    pub backoff_steps: Vec<u64>,
}

fn default_backoff_steps() -> Vec<u64> {
    vec![60, 300, 1800, 7200]
}

impl CircuitBreakerConfig {
    pub fn new() -> Self {
        Self {
            enabled: true,
            backoff_steps: default_backoff_steps(),
        }
    }
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl AppConfig {
    pub fn new() -> Self {
        Self {
            language: "zh".to_string(),
            theme: "system".to_string(),
            auto_refresh: true,
            refresh_interval: 15,
            auto_sync: false,
            sync_interval: 5,
            default_export_path: None,
            proxy: ProxyConfig::default(),
            auto_launch: false,
            scheduled_warmup: ScheduledWarmupConfig::default(),
            quota_protection: QuotaProtectionConfig::default(),
            pinned_quota_models: PinnedQuotaModelsConfig::default(),
            circuit_breaker: CircuitBreakerConfig::default(),
            hidden_menu_items: Vec::new(),
            cloudflared: CloudflaredConfig::default(),
            oauth_client_id: None,
            oauth_client_secret: None,
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Custom serde module for OAuth client secret — encrypts at rest, decrypts on load.
/// [R2-SEC-004] OAuth 密钥存储加固：配置文件中的密钥以 AES-256-GCM 加密存储。
/// Backward compatible: reads plaintext secrets from legacy configs, but always saves encrypted.
mod oauth_secret_serde {
    use crate::utils::crypto::{decrypt_string, encrypt_string};
    use serde::{Deserialize, Deserializer, Serializer};

    const ENCRYPTED_MARKER: &str = "ag_enc_";

    pub fn serialize<S>(value: &Option<String>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match value {
            None => serializer.serialize_none(),
            Some(secret) if secret.is_empty() => serializer.serialize_none(),
            Some(secret) => {
                // [R2-SEC-004] 防止双重加密：如果已经是加密格式，直接返回
                if secret.starts_with(ENCRYPTED_MARKER) {
                    serializer.serialize_str(secret)
                } else {
                    // 明文 → 加密后存储
                    let encrypted = encrypt_string(secret).map_err(serde::ser::Error::custom)?;
                    serializer.serialize_str(&encrypted)
                }
            }
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = Option::<String>::deserialize(deserializer)?;
        match raw {
            None => Ok(None),
            Some(s) if s.is_empty() => Ok(None),
            Some(s) => {
                if s.starts_with(ENCRYPTED_MARKER) {
                    // 已加密 → 解密
                    match decrypt_string(&s) {
                        Ok(plain) => Ok(Some(plain)),
                        Err(_) => {
                            // 解密失败（如密钥变更），返回加密原文防止数据丢失
                            Ok(Some(s))
                        }
                    }
                } else {
                    // 兼容旧版明文配置 → 直接返回
                    Ok(Some(s))
                }
            }
        }
    }
}

#[cfg(test)]
mod oauth_secret_tests {
    use super::*;

    #[test]
    fn test_oauth_secret_encrypts_on_save() {
        // [R2-SEC-004] 保存时密钥应被加密
        let config = AppConfig {
            oauth_client_secret: Some("my-secret-123".to_string()),
            ..AppConfig::new()
        };

        let json = serde_json::to_string(&config).unwrap();
        // 序列化后的 JSON 中不应包含明文密钥
        assert!(
            !json.contains("my-secret-123"),
            "Plaintext secret should not appear in serialized JSON"
        );
        assert!(json.contains("ag_enc_"));
    }

    #[test]
    fn test_oauth_secret_decrypts_on_load() {
        // [R2-SEC-004] 加载时应自动解密
        let config = AppConfig {
            oauth_client_secret: Some("my-secret-456".to_string()),
            ..AppConfig::new()
        };

        let json = serde_json::to_string(&config).unwrap();
        let loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(
            loaded.oauth_client_secret,
            Some("my-secret-456".to_string())
        );
    }

    #[test]
    fn test_oauth_secret_handles_plaintext_legacy() {
        // [R2-SEC-004] 向后兼容：旧配置文件中的明文密钥应能读取
        // Build a valid AppConfig JSON with a plaintext secret
        let mut config = AppConfig::new();
        config.oauth_client_secret = Some("legacy-plaintext-secret".to_string());
        // Serialize normally — the secret will be encrypted in the output
        // To test legacy reading, we need to manually craft JSON with the plaintext value
        // We'll use a different approach: verify that deserialize accepts plaintext
        let json = serde_json::json!({
            "language": "zh",
            "theme": "system",
            "auto_refresh": true,
            "refresh_interval": 15,
            "auto_sync": false,
            "sync_interval": 5,
            "proxy": {
                "enabled": false,
                "port": 8045,
                "api_key": "",
                "auto_start": false,
                "request_timeout": 30,
                "enable_logging": true,
                "upstream_proxy": { "enabled": false, "url": "" },
                "auth_mode": "off",
                "allow_lan_access": false,
                "admin_password": "",
            },
            "scheduled_warmup": { "enabled": false, "monitored_models": [] },
            "quota_protection": { "enabled": false, "threshold_percentage": 10, "monitored_models": [] },
            "pinned_quota_models": { "models": [] },
            "circuit_breaker": { "enabled": false, "backoff_steps": [] },
            "hidden_menu_items": [],
            "cloudflared": { "enabled": false, "mode": "quick", "port": 8045, "use_http2": true },
            "oauth_client_id": null,
            "oauth_client_secret": "legacy-plaintext-secret"
        });
        let json_str = json.to_string();
        let loaded: AppConfig = serde_json::from_str(&json_str).unwrap();
        assert_eq!(
            loaded.oauth_client_secret,
            Some("legacy-plaintext-secret".to_string())
        );
    }

    #[test]
    fn test_oauth_secret_none_when_empty() {
        let config = AppConfig {
            oauth_client_secret: Some("".to_string()),
            ..AppConfig::new()
        };
        let json = serde_json::to_string(&config).unwrap();
        let loaded: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(loaded.oauth_client_secret.is_none());
    }
}
