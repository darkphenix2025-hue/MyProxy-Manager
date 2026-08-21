//! Provider model discovery.
//!
//! Fetches the model list exposed by an upstream provider API and normalizes
//! the common OpenAI, Anthropic, and Gemini response shapes into model IDs.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

use crate::proxy::config::{
    build_provider_api_url, normalize_proxy_url, ProviderProtocol, UpstreamProvider,
    UpstreamProxyConfig,
};

const DEFAULT_DISCOVERY_TIMEOUT_SECS: u64 = 15;
const MAX_DISCOVERY_TIMEOUT_SECS: u64 = 30;
const MAX_ERROR_BODY_CHARS: usize = 2_000;

/// A model returned by an upstream model-list endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub name: Option<String>,
    pub created: Option<u64>,
    pub owned_by: Option<String>,
}

/// Return the provider configuration to use for this model-list request.
///
/// The OpenAI-compatible mode is intentionally applied to a cloned provider
/// so the discovery choice never changes the provider's saved request
/// protocol.
pub fn provider_for_model_discovery(
    provider: &UpstreamProvider,
    use_openai_protocol: bool,
) -> UpstreamProvider {
    if use_openai_protocol && matches!(provider.protocol, ProviderProtocol::AnthropicPassthrough) {
        let mut discovery_provider = provider.clone();
        discovery_provider.protocol = ProviderProtocol::OpenAICompatible;
        discovery_provider
    } else {
        provider.clone()
    }
}

/// Discover models using the application's configured upstream proxy.
pub async fn discover_provider_models_with_proxy(
    provider: &UpstreamProvider,
    upstream_proxy: &UpstreamProxyConfig,
) -> Result<Vec<DiscoveredModel>, String> {
    let url = model_list_url(provider)?;
    let client = build_discovery_client(provider, upstream_proxy)?;

    let mut request = client.get(&url).header("accept", "application/json");
    match &provider.protocol {
        ProviderProtocol::OpenAICompatible => {
            if !provider.api_key.trim().is_empty() {
                request = request.bearer_auth(provider.api_key.trim());
            }
        }
        ProviderProtocol::CodexResponses => {
            request = request.header("originator", "codex_cli_rs").header(
                "user-agent",
                "codex_cli_rs/0.144.1 (MyProxy-Manager; auth-file)",
            );
            if let Some(account_id) = provider.account_id.as_deref() {
                request = request.header("ChatGPT-Account-Id", account_id);
            }
            if !provider.api_key.trim().is_empty() {
                request = request.bearer_auth(provider.api_key.trim());
            }
        }
        ProviderProtocol::AnthropicPassthrough => {
            request = request
                .header("anthropic-version", "2023-06-01")
                .header("user-agent", "claude-code/1.0.0");
            if !provider.api_key.trim().is_empty() {
                request = request.header("x-api-key", provider.api_key.trim());
            }
        }
        ProviderProtocol::GeminiV1Internal => {
            if !provider.api_key.trim().is_empty() {
                request = request.bearer_auth(provider.api_key.trim());
            }
        }
    }

    let response = request
        .send()
        .await
        .map_err(|error| format!("Request failed: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Failed to read response: {error}"))?;

    if !status.is_success() {
        return Err(format!("HTTP {}: {}", status, truncate_for_error(&body)));
    }

    let payload: Value = serde_json::from_str(&body)
        .map_err(|error| format!("Failed to parse model list response: {error}"))?;
    let models = parse_discovered_models(&payload);
    if models.is_empty() {
        return Err("Provider returned no models".to_string());
    }

    Ok(models)
}

fn build_discovery_client(
    provider: &UpstreamProvider,
    upstream_proxy: &UpstreamProxyConfig,
) -> Result<Client, String> {
    let timeout_secs = provider
        .request_timeout_secs
        .unwrap_or(DEFAULT_DISCOVERY_TIMEOUT_SECS)
        .clamp(5, MAX_DISCOVERY_TIMEOUT_SECS);
    let mut builder = Client::builder().timeout(Duration::from_secs(timeout_secs));

    if upstream_proxy.enabled && !upstream_proxy.url.trim().is_empty() {
        let proxy_url = normalize_proxy_url(&upstream_proxy.url);
        let proxy = reqwest::Proxy::all(&proxy_url)
            .map_err(|error| format!("Invalid upstream proxy URL: {error}"))?;
        builder = builder.proxy(proxy);
    }

    builder
        .build()
        .map_err(|error| format!("Failed to build HTTP client: {error}"))
}

/// Build the endpoint used to list models for a provider.
fn model_list_url(provider: &UpstreamProvider) -> Result<String, String> {
    let base = provider.base_url.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err("Provider base URL is empty".to_string());
    }

    match &provider.protocol {
        ProviderProtocol::OpenAICompatible | ProviderProtocol::AnthropicPassthrough => {
            Ok(build_provider_api_url(base, "models"))
        }
        ProviderProtocol::CodexResponses => Ok(format!("{base}/models?client_version=0.144.1")),
        ProviderProtocol::GeminiV1Internal => {
            if base.contains("/v1internal") {
                return Err(
                    "Gemini v1internal providers do not expose a standard model-list API; configure available models manually".to_string(),
                );
            }

            if base.ends_with("/models") {
                Ok(base.to_string())
            } else if base.ends_with("/v1beta") || base.ends_with("/v1") {
                Ok(format!("{base}/models"))
            } else {
                Ok(format!("{base}/v1beta/models"))
            }
        }
    }
}

pub fn parse_discovered_models(value: &Value) -> Vec<DiscoveredModel> {
    let mut models = Vec::new();

    match value {
        Value::Array(items) => {
            for item in items {
                push_model(&mut models, item);
            }
        }
        Value::Object(object) => {
            for key in ["data", "models"] {
                if let Some(items) = object.get(key) {
                    match items {
                        Value::Array(items) => {
                            for item in items {
                                push_model(&mut models, item);
                            }
                        }
                        item => push_model(&mut models, item),
                    }
                }
            }
        }
        _ => {}
    }

    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    models
}

fn push_model(models: &mut Vec<DiscoveredModel>, value: &Value) {
    let (raw_id, name, created, owned_by) = match value {
        Value::String(id) => (Some(id.as_str()), None, None, None),
        Value::Object(object) => {
            let raw_id = ["id", "name", "model", "modelId", "slug"]
                .iter()
                .find_map(|key| object.get(*key).and_then(Value::as_str));
            let name = ["displayName", "display_name", "name"]
                .iter()
                .find_map(|key| object.get(*key).and_then(Value::as_str))
                .map(str::to_string);
            let created = object.get("created").and_then(Value::as_u64);
            let owned_by = object
                .get("owned_by")
                .or_else(|| object.get("ownedBy"))
                .and_then(Value::as_str)
                .map(str::to_string);
            (raw_id, name, created, owned_by)
        }
        _ => (None, None, None, None),
    };

    let Some(raw_id) = raw_id else {
        return;
    };
    let id = normalize_model_id(raw_id);
    if id.is_empty() || models.iter().any(|model| model.id == id) {
        return;
    }

    models.push(DiscoveredModel {
        id,
        name,
        created,
        owned_by,
    });
}

fn normalize_model_id(value: &str) -> String {
    value
        .trim()
        .strip_prefix("models/")
        .unwrap_or(value.trim())
        .trim()
        .to_string()
}

/// Parse a provider response into sorted, unique model IDs.
#[cfg_attr(not(test), allow(dead_code))]
pub fn parse_model_ids(value: &Value) -> Vec<String> {
    parse_discovered_models(value)
        .into_iter()
        .map(|model| model.id)
        .collect()
}

fn truncate_for_error(value: &str) -> String {
    let mut chars = value.chars();
    let preview: String = chars.by_ref().take(MAX_ERROR_BODY_CHARS).collect();
    if chars.next().is_some() {
        format!("{preview}...")
    } else {
        preview
    }
}

/// Convert discovered models to the CSV representation used by available_models.
pub fn models_to_csv(models: &[DiscoveredModel]) -> String {
    models
        .iter()
        .map(|model| model.id.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Synchronize one provider's model IDs without an explicit proxy.
#[allow(dead_code)]
pub async fn sync_provider_models(provider: &UpstreamProvider) -> Result<Vec<String>, String> {
    sync_provider_models_with_proxy(provider, &UpstreamProxyConfig::default()).await
}

/// Synchronize one provider's model IDs using the application's proxy.
pub async fn sync_provider_models_with_proxy(
    provider: &UpstreamProvider,
    upstream_proxy: &UpstreamProxyConfig,
) -> Result<Vec<String>, String> {
    let discovered = discover_provider_models_with_proxy(provider, upstream_proxy).await?;
    let model_csv = models_to_csv(&discovered);
    let model_ids: Vec<String> = discovered.into_iter().map(|model| model.id).collect();

    tracing::info!(
        "[ModelDiscovery] Provider '{}' discovered {} models: {}",
        provider.name,
        model_ids.len(),
        model_csv
    );

    Ok(model_ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_list_url_avoids_duplicate_v1() {
        let provider = test_provider("https://example.com", ProviderProtocol::OpenAICompatible);
        assert_eq!(
            model_list_url(&provider).unwrap(),
            "https://example.com/v1/models"
        );

        let provider = test_provider("https://example.com/v1", ProviderProtocol::OpenAICompatible);
        assert_eq!(
            model_list_url(&provider).unwrap(),
            "https://example.com/v1/models"
        );

        let provider = test_provider(
            "https://example.com/api/anthropic",
            ProviderProtocol::AnthropicPassthrough,
        );
        assert_eq!(
            model_list_url(&provider).unwrap(),
            "https://example.com/api/anthropic/v1/models"
        );

        let provider = test_provider(
            "https://open.bigmodel.cn/api/paas/v4",
            ProviderProtocol::OpenAICompatible,
        );
        assert_eq!(
            model_list_url(&provider).unwrap(),
            "https://open.bigmodel.cn/api/paas/v4/models"
        );

        let provider = test_provider(
            "https://opencode.ai/zen/go/v1/responses",
            ProviderProtocol::OpenAICompatible,
        );
        assert_eq!(
            model_list_url(&provider).unwrap(),
            "https://opencode.ai/zen/go/v1/models"
        );

        let provider = test_provider(
            "https://chatgpt.com/backend-api/codex",
            ProviderProtocol::CodexResponses,
        );
        assert_eq!(
            model_list_url(&provider).unwrap(),
            "https://chatgpt.com/backend-api/codex/models?client_version=0.144.1"
        );
    }

    #[test]
    fn test_anthropic_model_discovery_can_use_openai_protocol() {
        let provider = test_provider(
            "https://example.com/api/anthropic",
            ProviderProtocol::AnthropicPassthrough,
        );

        assert_eq!(
            provider_for_model_discovery(&provider, false).protocol,
            ProviderProtocol::AnthropicPassthrough
        );
        assert_eq!(
            provider_for_model_discovery(&provider, true).protocol,
            ProviderProtocol::OpenAICompatible
        );
    }

    #[test]
    fn test_model_discovery_protocol_override_only_applies_to_anthropic() {
        let provider = test_provider("https://example.com/v1", ProviderProtocol::OpenAICompatible);

        assert_eq!(
            provider_for_model_discovery(&provider, true).protocol,
            ProviderProtocol::OpenAICompatible
        );
    }

    #[test]
    fn test_parse_model_ids_supports_openai_and_gemini_shapes() {
        let openai = serde_json::json!({
            "data": [
                {"id": "gpt-4o"},
                {"id": "gpt-4o"},
                {"id": "  "},
                {"name": "fallback-name"}
            ]
        });
        assert_eq!(parse_model_ids(&openai), vec!["fallback-name", "gpt-4o"]);

        let codex = serde_json::json!({
            "models": [{"slug": "gpt-5.4", "display_name": "GPT 5.4"}]
        });
        assert_eq!(parse_model_ids(&codex), vec!["gpt-5.4"]);

        let gemini = serde_json::json!({
            "models": [
                {"name": "models/gemini-2.5-pro"},
                {"name": "models/gemini-2.5-flash"}
            ]
        });
        assert_eq!(
            parse_model_ids(&gemini),
            vec!["gemini-2.5-flash", "gemini-2.5-pro"]
        );
    }

    #[test]
    fn test_parse_model_ids_supports_string_arrays_and_deduplicates() {
        let payload = serde_json::json!(["model-b", "model-a", "model-b", "models/model-c"]);
        assert_eq!(
            parse_model_ids(&payload),
            vec!["model-a", "model-b", "model-c"]
        );
    }

    #[test]
    fn test_parse_open_code_go_model_list_keeps_responses_models() {
        let payload = serde_json::json!({
            "object": "list",
            "data": [
                {"id": "gpt-5.6-luna", "owned_by": "opencode"},
                {"id": "grok-4.5", "owned_by": "opencode"}
            ]
        });

        assert_eq!(parse_model_ids(&payload), vec!["gpt-5.6-luna", "grok-4.5"]);
    }

    #[test]
    fn test_parse_discovered_models_preserves_display_metadata() {
        let payload = serde_json::json!({
            "data": [{
                "id": "gpt-5.6-luna",
                "display_name": "GPT 5.6 Luna",
                "owned_by": "opencode"
            }]
        });

        let models = parse_discovered_models(&payload);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5.6-luna");
        assert_eq!(models[0].name.as_deref(), Some("GPT 5.6 Luna"));
        assert_eq!(models[0].owned_by.as_deref(), Some("opencode"));
    }

    #[test]
    fn test_model_list_url_rejects_v1internal_provider() {
        let provider = test_provider(
            "https://cloudcode-pa.googleapis.com/v1internal",
            ProviderProtocol::GeminiV1Internal,
        );
        let error = model_list_url(&provider).unwrap_err();
        assert!(error.contains("v1internal"));
    }

    fn test_provider(base_url: &str, protocol: ProviderProtocol) -> UpstreamProvider {
        UpstreamProvider {
            name: "test".to_string(),
            provider_id: None,
            provider_id_aliases: Vec::new(),
            provider_group: None,
            enabled: true,
            base_url: base_url.to_string(),
            api_key: "secret".to_string(),
            credential_id: None,
            account_id: None,
            protocol,
            dispatch_mode: Default::default(),
            priority: 0,
            model_prefixes: Vec::new(),
            model_mapping: std::collections::HashMap::new(),
            available_models: None,
            model_configs: Vec::new(),
            protocols: Vec::new(),
            request_timeout_secs: None,
        }
    }

    #[test]
    fn test_models_to_csv() {
        let models = vec![
            DiscoveredModel {
                id: "model-a".into(),
                name: None,
                created: None,
                owned_by: None,
            },
            DiscoveredModel {
                id: "model-b".into(),
                name: None,
                created: None,
                owned_by: None,
            },
        ];
        let csv = models_to_csv(&models);
        assert_eq!(csv, "model-a, model-b");
    }

    #[test]
    fn test_models_to_csv_empty() {
        let models: Vec<DiscoveredModel> = vec![];
        let csv = models_to_csv(&models);
        assert_eq!(csv, "");
    }
}
