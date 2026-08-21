use crate::proxy::config::{ProviderProtocol, UpstreamProvider};
use std::collections::HashSet;

/// Return the configured models in stable order, optionally narrowed to one
/// explicitly requested model.
pub fn collect_test_models(
    provider: &UpstreamProvider,
    requested_model: Option<&str>,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut models = Vec::new();

    if let Some(available_models) = provider.available_models.as_deref() {
        for model in available_models
            .split(',')
            .map(str::trim)
            .filter(|model| !model.is_empty())
        {
            if seen.insert(model.to_string()) {
                models.push(model.to_string());
            }
        }
    }

    for model in &provider.model_configs {
        let model_id = model.id.trim();
        if !model_id.is_empty() && seen.insert(model_id.to_string()) {
            models.push(model_id.to_string());
        }
    }

    let requested_model = requested_model
        .map(str::trim)
        .filter(|model| !model.is_empty());
    if let Some(requested_model) = requested_model {
        return models
            .into_iter()
            .filter(|model| model == requested_model)
            .collect();
    }

    models
}

/// Expand the provider into enabled protocol variants and optionally narrow
/// the result to one protocol.
pub fn collect_test_variants(
    provider: &UpstreamProvider,
    requested_protocol: Option<&ProviderProtocol>,
) -> Vec<UpstreamProvider> {
    provider
        .protocol_variants()
        .into_iter()
        .filter(|variant| requested_protocol.is_none_or(|protocol| variant.protocol == *protocol))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::config::{ProviderDispatchMode, ProviderModelConfig, ProviderProtocolConfig};
    use std::collections::HashMap;

    fn protocol_config(protocol: ProviderProtocol, enabled: bool) -> ProviderProtocolConfig {
        ProviderProtocolConfig {
            protocol,
            enabled,
            base_url: "https://example.com".to_string(),
            api_key: "test-key".to_string(),
            credential_id: None,
            dispatch_mode: ProviderDispatchMode::Exclusive,
            priority: 0,
            model_prefixes: Vec::new(),
            model_mapping: HashMap::new(),
            request_timeout_secs: None,
        }
    }

    fn provider() -> UpstreamProvider {
        UpstreamProvider {
            name: "Test Provider".to_string(),
            provider_id: Some("test".to_string()),
            provider_id_aliases: Vec::new(),
            provider_group: None,
            enabled: true,
            base_url: "https://example.com".to_string(),
            api_key: "test-key".to_string(),
            credential_id: None,
            account_id: None,
            protocol: ProviderProtocol::AnthropicPassthrough,
            dispatch_mode: ProviderDispatchMode::Exclusive,
            priority: 0,
            model_prefixes: Vec::new(),
            model_mapping: HashMap::new(),
            available_models: Some("alpha, beta".to_string()),
            model_configs: vec![
                ProviderModelConfig {
                    id: "beta".to_string(),
                    display_name: Some("Beta".to_string()),
                    alias: None,
                    supports_images: false,
                    reasoning_efforts: Vec::new(),
                },
                ProviderModelConfig {
                    id: "gamma".to_string(),
                    display_name: None,
                    alias: None,
                    supports_images: false,
                    reasoning_efforts: Vec::new(),
                },
            ],
            protocols: vec![
                protocol_config(ProviderProtocol::AnthropicPassthrough, true),
                protocol_config(ProviderProtocol::OpenAICompatible, true),
                protocol_config(ProviderProtocol::GeminiV1Internal, false),
            ],
            request_timeout_secs: None,
        }
    }

    #[test]
    fn collects_unique_models_from_legacy_and_detailed_fields() {
        let models = collect_test_models(&provider(), None);
        assert_eq!(models, ["alpha", "beta", "gamma"]);
    }

    #[test]
    fn narrows_to_requested_model_and_rejects_unknown_model() {
        let provider = provider();
        assert_eq!(collect_test_models(&provider, Some("gamma")), ["gamma"]);
        assert!(collect_test_models(&provider, Some("missing")).is_empty());
    }

    #[test]
    fn returns_only_enabled_protocol_variants() {
        let variants = collect_test_variants(&provider(), None);
        assert_eq!(variants.len(), 2);
        assert!(variants
            .iter()
            .all(|variant| variant.protocol != ProviderProtocol::GeminiV1Internal));
    }

    #[test]
    fn narrows_to_requested_protocol() {
        let provider = provider();
        let variants = collect_test_variants(&provider, Some(&ProviderProtocol::OpenAICompatible));
        assert_eq!(variants.len(), 1);
        assert_eq!(variants[0].protocol, ProviderProtocol::OpenAICompatible);
    }
}
