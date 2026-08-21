use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::proxy::config::{
    is_valid_provider_id, merge_provider_protocol_entries, ProviderDispatchMode, ProviderProtocol,
    UpstreamProvider, ZaiConfig,
};

/// Result of provider selection, including the resolved model name.
///
/// The provider is owned so account targets can materialize a Codex auth-file
/// provider from `account/<credential-id>/<model>` without keeping a temporary
/// value alive inside the router.
#[derive(Debug, Clone)]
pub struct ProviderSelection {
    pub provider: UpstreamProvider,
    /// The model name to use when forwarding (routing prefix stripped).
    pub resolved_model: String,
}

/// Routes incoming requests to the appropriate upstream provider.
pub struct ProviderRouter {
    providers: Vec<UpstreamProvider>,
    rr_counter: Arc<AtomicUsize>,
    /// Index of provider_id (including legacy aliases) -> provider indexes.
    /// Multiple indexes are valid when one logical provider exposes several
    /// protocol variants.
    provider_id_index: HashMap<String, Vec<usize>>,
}

/// Stable identity used to exclude one provider/protocol variant during a
/// routing decision. Provider IDs are shared by merged protocol variants, so
/// protocol must remain part of the identity.
pub fn provider_route_key(provider: &UpstreamProvider) -> String {
    let provider_id = provider
        .provider_id
        .as_deref()
        .unwrap_or(provider.name.as_str());
    format!("{}::{}", provider_id, provider.protocol.as_str())
}

impl ProviderRouter {
    /// Build from config. If `providers` is empty, optionally migrate from legacy z.ai config.
    pub fn new(providers: Vec<UpstreamProvider>, legacy_zai: Option<&ZaiConfig>) -> Self {
        let enabled: Vec<_> = merge_provider_protocol_entries(providers)
            .into_iter()
            .filter(|p| p.enabled)
            .flat_map(|provider| provider.protocol_variants())
            .collect();

        if enabled.is_empty() {
            if let Some(zai) = legacy_zai {
                if zai.enabled && zai.dispatch_mode != crate::proxy::ZaiDispatchMode::Off {
                    return Self::from_zai_config(zai);
                }
            }
        }

        // Build provider_id index, validate format and uniqueness.  Protocol
        // variants of the same logical provider are deliberately allowed to
        // share an ID; unrelated providers with the same ID are rejected.
        let mut provider_id_index: HashMap<String, Vec<usize>> = HashMap::new();
        let mut invalid_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (i, p) in enabled.iter().enumerate() {
            let mut provider_ids = Vec::new();
            // Use provider_id if set, otherwise fall back to lowercase name.
            if let Some(provider_id) = p.provider_id.clone() {
                provider_ids.push(provider_id);
            } else {
                let lower = p.name.to_lowercase();
                if is_valid_provider_id(&lower) {
                    provider_ids.push(lower);
                }
            }
            provider_ids.extend(p.provider_id_aliases.iter().cloned());

            for id in provider_ids {
                if invalid_ids.contains(&id) {
                    continue;
                }
                if !is_valid_provider_id(&id) {
                    tracing::warn!(
                        "[ProviderRouter] Invalid provider_id format: '{}' (provider: '{}'). \
                         Must be 1-10 alphanumeric chars, starting with a letter.",
                        id,
                        p.name
                    );
                    continue;
                }
                let has_conflict = provider_id_index.get(&id).is_some_and(|indexes| {
                    indexes.iter().any(|index| enabled[*index].name != p.name)
                });
                if has_conflict {
                    tracing::error!(
                        "[ProviderRouter] Duplicate provider_id: '{}' (provider: '{}'). \
                         This ID will not be routable by provider_id.",
                        id,
                        p.name
                    );
                    invalid_ids.insert(id.clone());
                    provider_id_index.remove(&id);
                    continue;
                }
                let indexes = provider_id_index.entry(id).or_default();
                if !indexes.contains(&i) {
                    indexes.push(i);
                }
            }
        }

        tracing::info!(
            "[ProviderRouter] Built with {} providers, provider_id_index = {:?}",
            enabled.len(),
            provider_id_index
        );

        Self {
            providers: enabled,
            rr_counter: Arc::new(AtomicUsize::new(0)),
            provider_id_index,
        }
    }

    /// Build a single-provider router from legacy z.ai configuration.
    fn from_zai_config(zai: &ZaiConfig) -> Self {
        let dispatch_mode = match zai.dispatch_mode {
            crate::proxy::ZaiDispatchMode::Off => ProviderDispatchMode::Exclusive,
            crate::proxy::ZaiDispatchMode::Exclusive => ProviderDispatchMode::Exclusive,
            crate::proxy::ZaiDispatchMode::Pooled => ProviderDispatchMode::Pooled,
            crate::proxy::ZaiDispatchMode::Fallback => ProviderDispatchMode::Fallback,
        };

        let provider = UpstreamProvider {
            name: "z.ai".to_string(),
            provider_id: None,
            provider_id_aliases: Vec::new(),
            provider_group: None,
            enabled: true,
            base_url: zai.base_url.clone(),
            api_key: zai.api_key.clone(),
            credential_id: None,
            account_id: None,
            protocol: ProviderProtocol::AnthropicPassthrough,
            dispatch_mode,
            priority: 0,
            model_prefixes: Vec::new(),
            model_mapping: zai.model_mapping.clone(),
            available_models: None,
            model_configs: Vec::new(),
            protocols: Vec::new(),
            request_timeout_secs: None,
        };

        Self {
            providers: vec![provider],
            rr_counter: Arc::new(AtomicUsize::new(0)),
            provider_id_index: Default::default(),
        }
    }

    /// If `model` starts with a registered `provider_id/`, return `(provider_id, rest)`.
    fn extract_provider_id_prefix<'a>(&self, model: &'a str) -> Option<(&'a str, &'a str)> {
        if let Some(slash_pos) = model.find('/') {
            let candidate = &model[..slash_pos];
            if self.provider_id_index.contains_key(candidate) {
                let rest = &model[slash_pos + 1..];
                if !rest.is_empty() {
                    return Some((candidate, rest));
                }
            }
        }
        None
    }

    /// Parse a direct Codex auth-file route target.
    ///
    /// The credential ID is intentionally kept in the route target instead of
    /// being persisted as a provider ID (provider IDs have a short format
    /// limit). The credential is validated and refreshed by the account runtime
    /// immediately before forwarding.
    fn extract_account_target(model: &str) -> Option<(&str, &str)> {
        let rest = model.strip_prefix("account/")?;
        let slash_pos = rest.find('/')?;
        let credential_id = &rest[..slash_pos];
        let model_id = &rest[slash_pos + 1..];
        if credential_id.is_empty() || model_id.is_empty() {
            return None;
        }
        Some((credential_id, model_id))
    }

    fn account_provider(credential_id: &str) -> UpstreamProvider {
        UpstreamProvider {
            name: format!("Codex account {credential_id}"),
            provider_id: None,
            provider_id_aliases: Vec::new(),
            provider_group: None,
            enabled: true,
            base_url: crate::modules::codex_account_runtime::CODEX_UPSTREAM_ENDPOINT.to_string(),
            api_key: String::new(),
            credential_id: Some(credential_id.to_string()),
            account_id: None,
            protocol: ProviderProtocol::CodexResponses,
            dispatch_mode: ProviderDispatchMode::Exclusive,
            priority: 0,
            model_prefixes: Vec::new(),
            model_mapping: HashMap::new(),
            available_models: None,
            model_configs: Vec::new(),
            protocols: Vec::new(),
            request_timeout_secs: None,
        }
    }

    /// Whether a model has a real route in the configured provider set.
    ///
    /// This intentionally differs from [`Self::can_route`].  `can_route` is a
    /// legacy availability check and returns true as soon as any provider is
    /// configured.  Fallback selection needs a stricter answer so an unknown
    /// model does not silently get sent to an unrelated provider.
    pub fn has_model_match(&self, model: &str) -> bool {
        if Self::extract_account_target(model).is_some() {
            return true;
        }

        if let Some((provider_id, target_model)) = self.extract_provider_id_prefix(model) {
            let target_model = target_model.to_lowercase();
            return self
                .provider_id_index
                .get(provider_id)
                .into_iter()
                .flatten()
                .any(|index| Self::provider_matches_model(&self.providers[*index], &target_model));
        }

        let model_lower = model.to_lowercase();
        self.providers
            .iter()
            .any(|provider| Self::provider_matches_model(provider, &model_lower))
    }

    /// Whether a model can be routed even when no API-key providers exist.
    pub fn can_route(&self, model: &str) -> bool {
        !self.providers.is_empty() || Self::extract_account_target(model).is_some()
    }

    fn provider_matches_model(provider: &UpstreamProvider, model_lower: &str) -> bool {
        if !provider.model_prefixes.is_empty() {
            return provider
                .model_prefixes
                .iter()
                .any(|prefix| model_lower.starts_with(prefix.to_lowercase().as_str()));
        }

        if let Some(available_models) = &provider.available_models {
            return available_models
                .split(',')
                .map(|model| model.trim().to_lowercase())
                .filter(|model| !model.is_empty())
                .any(|model| model == model_lower);
        }

        if !provider.model_configs.is_empty()
            && provider
                .model_configs
                .iter()
                .any(|model| model.id.trim().eq_ignore_ascii_case(model_lower))
        {
            return true;
        }

        if !provider.model_mapping.is_empty()
            && provider
                .model_mapping
                .keys()
                .any(|key| model_lower.starts_with(key.to_lowercase().as_str()))
        {
            return true;
        }

        Self::is_unconstrained(provider)
    }

    fn is_unconstrained(provider: &UpstreamProvider) -> bool {
        provider.model_prefixes.is_empty()
            && provider.available_models.is_none()
            && provider.model_configs.is_empty()
            && provider.model_mapping.is_empty()
    }

    /// Select the next provider for the given model.
    /// `prev_failed` excludes a provider that just failed, triggering fallback.
    ///
    /// Routing priority:
    /// 1. If model starts with `account/<credential-id>/`, route to that Codex auth-file.
    /// 2. If model starts with a registered `provider_id/`, route directly to that provider.
    /// 3. Match model name against provider `model_prefixes`:
    ///    - Providers with matching prefixes become candidates.
    ///    - Providers with empty `model_prefixes` are treated as wildcards (match any model).
    ///    - If specific prefix matches exist, only those providers are considered.
    ///    - If no specific prefix matches, wildcard providers are used.
    /// 4. Among candidates, select by dispatch mode:
    ///    - Exclusive providers first (sorted by priority, then config order)
    ///    - Pooled providers next (sorted by priority, then RR)
    ///    - Fallback providers last (sorted by priority, then config order)
    pub fn select(&self, model: &str, prev_failed: Option<&str>) -> ProviderSelection {
        self.select_internal(model, prev_failed, None, None)
    }

    /// Select a provider while preferring the protocol required by the
    /// incoming endpoint. If no exact protocol variant exists, normal routing
    /// rules are used as a compatibility fallback.
    pub fn select_for_protocol(
        &self,
        model: &str,
        prev_failed: Option<&str>,
        protocol: ProviderProtocol,
    ) -> ProviderSelection {
        self.select_internal(model, prev_failed, Some(&protocol), None)
    }

    /// Select a provider while excluding route identities that are currently
    /// cooling down. Returning `None` is important: it tells the caller that
    /// every matching provider/model candidate has been exhausted, so the
    /// configured fallback or a clear cooldown error can be used.
    pub fn select_for_protocol_excluding(
        &self,
        model: &str,
        prev_failed: Option<&str>,
        protocol: ProviderProtocol,
        excluded: &HashSet<String>,
    ) -> Option<ProviderSelection> {
        if !self.has_eligible_candidate(model, prev_failed, Some(&protocol), excluded) {
            return None;
        }
        Some(self.select_internal(model, prev_failed, Some(&protocol), Some(excluded)))
    }

    fn select_internal(
        &self,
        model: &str,
        prev_failed: Option<&str>,
        preferred_protocol: Option<&ProviderProtocol>,
        excluded: Option<&HashSet<String>>,
    ) -> ProviderSelection {
        // STEP 1: Check for a direct Codex auth-file route target.
        if let Some((credential_id, rest)) = Self::extract_account_target(model) {
            let provider = Self::account_provider(credential_id);
            tracing::info!(
                "[ProviderRouter] account match: '{}' -> credential='{}', rest='{}'",
                model,
                credential_id,
                rest
            );
            if prev_failed.is_none_or(|pf| provider.name != pf)
                && !Self::is_excluded(&provider, excluded)
            {
                return ProviderSelection {
                    provider,
                    resolved_model: rest.to_string(),
                };
            }
        }

        // STEP 2: Check for provider_id/ prefix routing (highest priority)
        if let Some((pid, rest)) = self.extract_provider_id_prefix(model) {
            tracing::info!(
                "[ProviderRouter] provider_id match: '{}' -> provider='{}', rest='{}'",
                model,
                pid,
                rest
            );
            if let Some(indexes) = self.provider_id_index.get(pid) {
                let idx = preferred_protocol
                    .and_then(|protocol| {
                        indexes.iter().copied().find(|index| {
                            let provider = &self.providers[*index];
                            provider.protocol == *protocol
                                && prev_failed.is_none_or(|pf| provider.name != pf)
                                && !Self::is_excluded(provider, excluded)
                        })
                    })
                    .or_else(|| {
                        indexes.iter().copied().find(|index| {
                            let provider = &self.providers[*index];
                            prev_failed.is_none_or(|pf| provider.name != pf)
                                && !Self::is_excluded(provider, excluded)
                        })
                    });
                if let Some(idx) = idx {
                    let provider = &self.providers[idx];
                    return ProviderSelection {
                        provider: (*provider).clone(),
                        resolved_model: rest.to_string(),
                    };
                }
                // provider_id matched but provider failed; fall through to default selection
            }
        } else {
            tracing::info!("[ProviderRouter] no provider_id prefix match for '{}', falling through to wildcard selection", model);
        }

        // STEP 3: Model matching via model_prefixes or available_models
        let model_lower = model.to_lowercase();
        let mut specific_matches: Vec<&UpstreamProvider> = Vec::new();
        let mut wildcard_matches: Vec<&UpstreamProvider> = Vec::new();

        for p in &self.providers {
            if prev_failed.is_some_and(|pf| p.name == pf) || Self::is_excluded(p, excluded) {
                continue;
            }

            let has_match = Self::provider_matches_model(p, &model_lower);

            if has_match {
                specific_matches.push(p);
            } else if Self::is_unconstrained(p) {
                // Truly unconstrained = wildcard
                wildcard_matches.push(p);
            }
        }

        // Use specific matches if any, otherwise fall back to wildcards
        let prefix_candidates: Vec<&UpstreamProvider> = if !specific_matches.is_empty() {
            specific_matches
        } else {
            wildcard_matches
        };

        // STEP 3: Dispatch mode selection among prefix-matched candidates
        let candidates: Vec<_> = if prefix_candidates.is_empty() {
            // No prefix match at all, use all providers as fallback
            self.providers
                .iter()
                .filter(|p| {
                    prev_failed.is_none_or(|pf| p.name != pf) && !Self::is_excluded(p, excluded)
                })
                .collect()
        } else {
            prefix_candidates
        };

        // A logical provider can expose both OpenAI and Anthropic variants.
        // Prefer the exact protocol when at least one candidate supports it;
        // otherwise retain the old fallback behavior.
        let candidates: Vec<_> = if let Some(protocol) = preferred_protocol {
            let exact: Vec<_> = candidates
                .iter()
                .copied()
                .filter(|provider| provider.protocol == *protocol)
                .collect();
            if exact.is_empty() {
                candidates
            } else {
                exact
            }
        } else {
            candidates
        };

        if candidates.is_empty() {
            return ProviderSelection {
                provider: self
                    .providers
                    .first()
                    .cloned()
                    .unwrap_or_else(|| Self::account_provider("unavailable")),
                resolved_model: model.to_string(),
            };
        }

        // 1. Exclusive providers first (sorted by priority, stable sort preserves config order)
        let mut exclusive: Vec<_> = candidates
            .iter()
            .filter(|p| p.dispatch_mode == ProviderDispatchMode::Exclusive)
            .copied()
            .collect();
        exclusive.sort_by_key(|p| p.priority);
        if let Some(p) = exclusive.first() {
            return ProviderSelection {
                provider: (**p).clone(),
                resolved_model: model.to_string(),
            };
        }

        // 2. Round-robin among pooled providers
        let mut pooled: Vec<_> = candidates
            .iter()
            .filter(|p| p.dispatch_mode == ProviderDispatchMode::Pooled)
            .copied()
            .collect();
        if !pooled.is_empty() {
            pooled.sort_by_key(|p| p.priority);
            // Group by priority, RR within same priority group
            let top_priority = pooled[0].priority;
            let same_priority: Vec<_> = pooled
                .into_iter()
                .filter(|p| p.priority == top_priority)
                .collect();
            let idx = self.rr_counter.fetch_add(1, Ordering::Relaxed) % same_priority.len();
            return ProviderSelection {
                provider: (*same_priority[idx]).clone(),
                resolved_model: model.to_string(),
            };
        }

        // 3. Fallback providers
        let mut fallback: Vec<_> = candidates
            .iter()
            .filter(|p| p.dispatch_mode == ProviderDispatchMode::Fallback)
            .copied()
            .collect();
        fallback.sort_by_key(|p| p.priority);
        let fallback_provider = fallback.first().copied().unwrap_or_else(|| {
            // No fallback provider, use the first available provider
            self.providers
                .iter()
                .find(|p| {
                    prev_failed.is_none_or(|pf| p.name != pf) && !Self::is_excluded(p, excluded)
                })
                .unwrap_or(&self.providers[0])
        });
        ProviderSelection {
            provider: (*fallback_provider).clone(),
            resolved_model: model.to_string(),
        }
    }

    fn is_excluded(provider: &UpstreamProvider, excluded: Option<&HashSet<String>>) -> bool {
        excluded.is_some_and(|set| set.contains(&provider_route_key(provider)))
    }

    fn has_eligible_candidate(
        &self,
        model: &str,
        prev_failed: Option<&str>,
        preferred_protocol: Option<&ProviderProtocol>,
        excluded: &HashSet<String>,
    ) -> bool {
        if let Some((credential_id, _)) = Self::extract_account_target(model) {
            let provider = Self::account_provider(credential_id);
            return prev_failed.is_none_or(|pf| provider.name != pf)
                && !Self::is_excluded(&provider, Some(excluded));
        }

        let model_lower = model.to_lowercase();
        let candidates: Vec<&UpstreamProvider> =
            if let Some((provider_id, target_model)) = self.extract_provider_id_prefix(model) {
                self.provider_id_index
                    .get(provider_id)
                    .into_iter()
                    .flatten()
                    .map(|index| &self.providers[*index])
                    .filter(|provider| {
                        Self::provider_matches_model(provider, &target_model.to_lowercase())
                            && prev_failed.is_none_or(|pf| provider.name != pf)
                            && !Self::is_excluded(provider, Some(excluded))
                    })
                    .collect()
            } else {
                self.providers
                    .iter()
                    .filter(|provider| {
                        Self::provider_matches_model(provider, &model_lower)
                            && prev_failed.is_none_or(|pf| provider.name != pf)
                            && !Self::is_excluded(provider, Some(excluded))
                    })
                    .collect()
            };

        if candidates.is_empty() {
            return false;
        }

        preferred_protocol.is_none_or(|protocol| {
            // Exact protocol is preferred when it exists, but a compatible
            // variant remains eligible if no exact variant is configured.
            candidates
                .iter()
                .any(|provider| provider.protocol == *protocol)
                || candidates
                    .iter()
                    .any(|provider| provider.protocol != *protocol)
        })
    }

    /// Map incoming model name to the provider's native model ID.
    pub fn map_model(&self, provider: &UpstreamProvider, incoming: &str) -> String {
        // 1. Exact match in model_mapping
        if let Some(mapped) = provider.model_mapping.get(incoming) {
            return mapped.clone();
        }
        // 2. Prefix match in model_mapping
        let m = incoming.to_lowercase();
        if let Some((_, mapped)) = provider
            .model_mapping
            .iter()
            .find(|(k, _)| m.starts_with(k.as_str()))
        {
            return mapped.clone();
        }
        // 3. Strip `zai:` prefix
        if m.starts_with("zai:") {
            return incoming[4..].to_string();
        }
        // 4. Fallback: pass through as-is
        incoming.to_string()
    }

    /// Check if the router has any enabled providers.
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Find a provider by name.
    pub fn get_by_name(&self, name: &str) -> Option<&UpstreamProvider> {
        self.providers.iter().find(|p| p.name == name)
    }
}

/// Map a model name to the provider's expected model name.
/// Uses prefix-based rules and explicit model_mapping only.
/// This is a free function so it can be used without holding a `ProviderRouter` lock.
pub fn map_model_for_provider(
    original: &str,
    model_mapping: &std::collections::HashMap<String, String>,
) -> String {
    // 1. Exact match in model_mapping
    if let Some(mapped) = model_mapping.get(original) {
        return mapped.clone();
    }
    // 2. Lowercase match
    let m = original.to_lowercase();
    if let Some(mapped) = model_mapping.get(&m) {
        return mapped.clone();
    }
    // 3. Strip `zai:` prefix
    if m.starts_with("zai:") {
        return original[4..].to_string();
    }
    // 4. Fallback: pass through as-is
    original.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::config::ProviderProtocolConfig;

    fn make_test_provider(
        name: &str,
        dispatch_mode: ProviderDispatchMode,
        priority: u8,
        provider_id: Option<&str>,
    ) -> UpstreamProvider {
        UpstreamProvider {
            name: name.to_string(),
            provider_id: provider_id.map(String::from),
            provider_id_aliases: Default::default(),
            provider_group: None,
            enabled: true,
            base_url: "https://example.com".to_string(),
            api_key: "test-key".to_string(),
            credential_id: None,
            account_id: None,
            protocol: ProviderProtocol::AnthropicPassthrough,
            dispatch_mode,
            priority,
            model_prefixes: Default::default(),
            model_mapping: Default::default(),
            available_models: None,
            model_configs: Default::default(),
            protocols: Default::default(),
            request_timeout_secs: None,
        }
    }

    fn build_router_with_providers(providers: Vec<UpstreamProvider>) -> ProviderRouter {
        let mut provider_id_index: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, p) in providers.iter().enumerate() {
            if let Some(ref pid) = p.provider_id {
                provider_id_index.insert(pid.clone(), vec![i]);
            }
        }
        ProviderRouter {
            providers,
            rr_counter: Arc::new(AtomicUsize::new(0)),
            provider_id_index,
        }
    }

    #[test]
    fn test_select_exclusive() {
        let router = build_router_with_providers(vec![
            make_test_provider("A", ProviderDispatchMode::Exclusive, 1, Some("a")),
            make_test_provider("B", ProviderDispatchMode::Pooled, 0, Some("b")),
        ]);

        let sel = router.select("any-model", None);
        assert_eq!(sel.provider.name, "A"); // Exclusive wins over Pooled
        assert_eq!(sel.resolved_model, "any-model");
    }

    #[test]
    fn test_select_by_provider_id() {
        let p1 = make_test_provider("google", ProviderDispatchMode::Exclusive, 0, Some("g"));
        let p2 = make_test_provider("anthropic", ProviderDispatchMode::Exclusive, 0, Some("a"));

        let router = build_router_with_providers(vec![p1.clone(), p2.clone()]);

        // Direct provider_id routing
        let sel = router.select("g/gemini-2.5-pro", None);
        assert_eq!(sel.provider.name, "google");
        assert_eq!(sel.resolved_model, "gemini-2.5-pro");

        let sel = router.select("a/claude-sonnet-4-6", None);
        assert_eq!(sel.provider.name, "anthropic");
        assert_eq!(sel.resolved_model, "claude-sonnet-4-6");

        // No prefix -> default selection (exclusive, lowest priority first)
        // Both have priority 0, so config order first -> google
        let sel = router.select("gemini-2.5-pro", None);
        assert_eq!(sel.provider.name, "google");
        assert_eq!(sel.resolved_model, "gemini-2.5-pro");
    }

    #[test]
    fn test_provider_id_prefix_stripping() {
        let p = make_test_provider("google", ProviderDispatchMode::Pooled, 0, Some("g"));
        let router = build_router_with_providers(vec![p]);

        let sel = router.select("g/gemini-2.5-flash", None);
        assert_eq!(sel.provider.name, "google");
        assert_eq!(sel.resolved_model, "gemini-2.5-flash");

        // Unknown provider_id -> default selection
        let sel = router.select("x/unknown-model", None);
        assert_eq!(sel.provider.name, "google");
    }

    #[test]
    fn test_logical_provider_protocol_variants_and_legacy_alias() {
        let mut provider =
            make_test_provider("BIGMODEL", ProviderDispatchMode::Exclusive, 0, Some("big"));
        provider.provider_id_aliases = vec!["bigop".to_string()];
        provider.protocols = vec![
            ProviderProtocolConfig {
                protocol: ProviderProtocol::AnthropicPassthrough,
                enabled: true,
                base_url: "https://example.com/anthropic".to_string(),
                api_key: "anthropic-key".to_string(),
                credential_id: None,
                dispatch_mode: ProviderDispatchMode::Exclusive,
                priority: 0,
                model_prefixes: vec![],
                model_mapping: HashMap::new(),
                request_timeout_secs: None,
            },
            ProviderProtocolConfig {
                protocol: ProviderProtocol::OpenAICompatible,
                enabled: true,
                base_url: "https://example.com/v4".to_string(),
                api_key: "openai-key".to_string(),
                credential_id: None,
                dispatch_mode: ProviderDispatchMode::Exclusive,
                priority: 0,
                model_prefixes: vec![],
                model_mapping: HashMap::new(),
                request_timeout_secs: None,
            },
        ];

        let router = ProviderRouter::new(vec![provider], None);

        let anthropic = router.select_for_protocol(
            "big/claude-sonnet-4",
            None,
            ProviderProtocol::AnthropicPassthrough,
        );
        assert_eq!(
            anthropic.provider.protocol,
            ProviderProtocol::AnthropicPassthrough
        );
        assert_eq!(anthropic.provider.base_url, "https://example.com/anthropic");

        let openai =
            router.select_for_protocol("bigop/glm-5", None, ProviderProtocol::OpenAICompatible);
        assert_eq!(openai.provider.protocol, ProviderProtocol::OpenAICompatible);
        assert_eq!(openai.provider.base_url, "https://example.com/v4");
        assert_eq!(openai.resolved_model, "glm-5");
    }

    #[test]
    fn test_legacy_split_provider_entries_are_merged_at_runtime() {
        let mut anthropic =
            make_test_provider("BIGMODEL", ProviderDispatchMode::Exclusive, 0, Some("big"));
        anthropic.base_url = "https://example.com/anthropic".to_string();
        anthropic.protocol = ProviderProtocol::AnthropicPassthrough;

        let mut openai = make_test_provider(
            "BIGMODEL_OP",
            ProviderDispatchMode::Exclusive,
            0,
            Some("bigop"),
        );
        openai.base_url = "https://example.com/v4".to_string();
        openai.protocol = ProviderProtocol::OpenAICompatible;

        let router = ProviderRouter::new(vec![anthropic, openai], None);
        assert_eq!(router.providers.len(), 2);

        let selection =
            router.select_for_protocol("big/glm-5", None, ProviderProtocol::OpenAICompatible);
        assert_eq!(
            selection.provider.protocol,
            ProviderProtocol::OpenAICompatible
        );
        assert_eq!(selection.provider.base_url, "https://example.com/v4");
        assert_eq!(selection.resolved_model, "glm-5");
    }

    #[test]
    fn test_select_codex_account_target_without_configured_provider() {
        let router = ProviderRouter::new(vec![], None);
        let target = "account/credential-1/gpt-5.4";

        assert!(router.is_empty());
        assert!(router.can_route(target));

        let selection = router.select(target, None);
        assert_eq!(selection.resolved_model, "gpt-5.4");
        assert_eq!(
            selection.provider.credential_id.as_deref(),
            Some("credential-1")
        );
        assert_eq!(
            selection.provider.protocol,
            ProviderProtocol::CodexResponses
        );
        assert_eq!(
            selection.provider.base_url,
            crate::modules::codex_account_runtime::CODEX_UPSTREAM_ENDPOINT
        );
    }

    #[test]
    fn test_has_model_match_respects_provider_model_catalog() {
        let mut provider =
            make_test_provider("catalog", ProviderDispatchMode::Exclusive, 0, Some("cat"));
        provider.available_models = Some("known-model, another-model".to_string());

        let router = build_router_with_providers(vec![provider]);

        assert!(router.has_model_match("known-model"));
        assert!(!router.has_model_match("unknown-model"));
        assert!(router.has_model_match("cat/known-model"));
        assert!(!router.has_model_match("cat/unknown-model"));
    }

    #[test]
    fn test_has_model_match_accepts_unconstrained_and_direct_routes() {
        let unconstrained = make_test_provider(
            "unconstrained",
            ProviderDispatchMode::Exclusive,
            0,
            Some("free"),
        );
        let router = build_router_with_providers(vec![unconstrained]);

        assert!(router.has_model_match("any-model"));
        assert!(router.has_model_match("free/explicit-model"));
        assert!(router.has_model_match("account/credential-1/gpt-5.4"));
    }

    #[test]
    fn test_has_model_match_uses_model_mapping_as_a_constraint() {
        let mut provider =
            make_test_provider("mapped", ProviderDispatchMode::Exclusive, 0, Some("mapped"));
        provider
            .model_mapping
            .insert("friendly-model".to_string(), "native-model".to_string());

        let router = build_router_with_providers(vec![provider]);

        assert!(router.has_model_match("friendly-model"));
        assert!(!router.has_model_match("unrelated-model"));
    }

    #[test]
    fn test_default_selection_priority() {
        // Exclusive > Pooled > Fallback
        let router = build_router_with_providers(vec![
            make_test_provider("fallback", ProviderDispatchMode::Fallback, 0, Some("f")),
            make_test_provider("pooled", ProviderDispatchMode::Pooled, 0, Some("p")),
            make_test_provider("exclusive", ProviderDispatchMode::Exclusive, 0, Some("e")),
        ]);

        let sel = router.select("any-model", None);
        assert_eq!(sel.provider.name, "exclusive");
    }

    #[test]
    fn excluded_provider_is_not_selected_for_the_next_route_attempt() {
        let provider_a = make_test_provider("A", ProviderDispatchMode::Exclusive, 0, Some("a"));
        let provider_b = make_test_provider("B", ProviderDispatchMode::Exclusive, 1, Some("b"));
        let router = build_router_with_providers(vec![provider_a.clone(), provider_b.clone()]);
        let mut excluded = std::collections::HashSet::new();
        excluded.insert(provider_route_key(&provider_a));

        let selection = router
            .select_for_protocol_excluding(
                "any-model",
                None,
                ProviderProtocol::AnthropicPassthrough,
                &excluded,
            )
            .expect("the second provider should remain eligible");
        assert_eq!(selection.provider.name, "B");
    }

    #[test]
    fn excluding_all_matching_providers_returns_none() {
        let provider = make_test_provider("A", ProviderDispatchMode::Exclusive, 0, Some("a"));
        let router = build_router_with_providers(vec![provider.clone()]);
        let mut excluded = std::collections::HashSet::new();
        excluded.insert(provider_route_key(&provider));

        assert!(router
            .select_for_protocol_excluding(
                "any-model",
                None,
                ProviderProtocol::AnthropicPassthrough,
                &excluded,
            )
            .is_none());
    }

    #[test]
    fn test_priority_ordering() {
        // Higher priority number = lower priority (lower number wins)
        let router = build_router_with_providers(vec![
            make_test_provider("low", ProviderDispatchMode::Exclusive, 5, Some("l")),
            make_test_provider("high", ProviderDispatchMode::Exclusive, 1, Some("h")),
        ]);

        let sel = router.select("any-model", None);
        assert_eq!(sel.provider.name, "high");
    }

    #[test]
    fn test_map_model_passthrough() {
        let provider = make_test_provider("test", ProviderDispatchMode::Exclusive, 0, None);
        let router = build_router_with_providers(vec![provider.clone()]);

        // Without explicit mapping, models pass through as-is
        assert_eq!(
            router.map_model(&provider, "claude-sonnet-4-20250514"),
            "claude-sonnet-4-20250514"
        );
        assert_eq!(
            router.map_model(&provider, "claude-opus-4-20250514"),
            "claude-opus-4-20250514"
        );
        assert_eq!(
            router.map_model(&provider, "gemini-3-flash"),
            "gemini-3-flash"
        );
    }

    #[test]
    fn test_from_zai_config() {
        let zai = ZaiConfig {
            enabled: true,
            base_url: "https://api.z.ai/api/anthropic".to_string(),
            api_key: "zai-key".to_string(),
            dispatch_mode: crate::proxy::ZaiDispatchMode::Fallback,
            model_mapping: Default::default(),
            mcp: Default::default(),
        };

        let router = ProviderRouter::new(vec![], Some(&zai));
        assert!(!router.is_empty());

        let sel = router.select("claude-sonnet-4-20250514", None);
        assert_eq!(sel.provider.name, "z.ai");
        assert_eq!(sel.provider.base_url, "https://api.z.ai/api/anthropic");
    }

    #[test]
    fn test_is_valid_provider_id() {
        assert!(is_valid_provider_id("g"));
        assert!(is_valid_provider_id("google"));
        assert!(is_valid_provider_id("g1"));
        assert!(is_valid_provider_id("abc123"));
        assert!(is_valid_provider_id("a1b2c3d4e5")); // 10 chars

        assert!(!is_valid_provider_id("")); // empty
        assert!(!is_valid_provider_id("1a")); // starts with digit
        assert!(!is_valid_provider_id("a1b2c3d4e56")); // 11 chars
        assert!(!is_valid_provider_id("a-b")); // hyphen not allowed
        assert!(!is_valid_provider_id("a_b")); // underscore not allowed
    }
}
