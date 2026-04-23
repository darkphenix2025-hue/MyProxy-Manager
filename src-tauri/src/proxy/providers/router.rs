use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::proxy::config::{
    is_valid_provider_id, ProviderDispatchMode, ProviderProtocol,
    UpstreamProvider, ZaiConfig,
};

/// Result of provider selection, including the resolved model name
/// (with provider_id/ prefix stripped if applicable).
pub struct ProviderSelection<'a> {
    pub provider: &'a UpstreamProvider,
    /// The model name to use when forwarding (prefix stripped if provider_id routing was used).
    pub resolved_model: String,
}

/// Routes incoming requests to the appropriate upstream provider.
pub struct ProviderRouter {
    providers: Vec<UpstreamProvider>,
    rr_counter: Arc<AtomicUsize>,
    /// Index of provider_id -> provider index for O(1) lookups.
    provider_id_index: HashMap<String, usize>,
}

impl ProviderRouter {
    /// Build from config. If `providers` is empty, optionally migrate from legacy z.ai config.
    pub fn new(providers: Vec<UpstreamProvider>, legacy_zai: Option<&ZaiConfig>) -> Self {
        let enabled: Vec<_> = providers.into_iter().filter(|p| p.enabled).collect();

        if enabled.is_empty() {
            if let Some(zai) = legacy_zai {
                if zai.enabled && zai.dispatch_mode != crate::proxy::ZaiDispatchMode::Off {
                    return Self::from_zai_config(zai);
                }
            }
        }

        // Build provider_id index, validate format and uniqueness
        let mut provider_id_index: HashMap<String, usize> = HashMap::new();
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (i, p) in enabled.iter().enumerate() {
            // Use provider_id if set, otherwise fall back to lowercase name
            let pid = p.provider_id.clone().or_else(|| {
                let lower = p.name.to_lowercase();
                // Only use as fallback if it passes validation
                if is_valid_provider_id(&lower) {
                    Some(lower)
                } else {
                    None
                }
            });

            if let Some(ref id) = pid {
                if !is_valid_provider_id(id) {
                    tracing::warn!(
                        "[ProviderRouter] Invalid provider_id format: '{}' (provider: '{}'). \
                         Must be 1-10 alphanumeric chars, starting with a letter.",
                        id, p.name
                    );
                    continue;
                }
                if !seen_ids.insert(id.clone()) {
                    tracing::error!(
                        "[ProviderRouter] Duplicate provider_id: '{}' (provider: '{}'). \
                         This provider will not be routable by provider_id.",
                        id, p.name
                    );
                    provider_id_index.remove(id);
                    continue;
                }
                provider_id_index.insert(id.clone(), i);
            }
        }

        tracing::info!("[ProviderRouter] Built with {} providers, provider_id_index = {:?}", enabled.len(), provider_id_index);

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
            enabled: true,
            base_url: zai.base_url.clone(),
            api_key: zai.api_key.clone(),
            protocol: ProviderProtocol::AnthropicPassthrough,
            dispatch_mode,
            priority: 0,
            model_prefixes: Vec::new(),
            model_mapping: zai.model_mapping.clone(),
            available_models: None,
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

    /// Select the next provider for the given model.
    /// `prev_failed` excludes a provider that just failed, triggering fallback.
    ///
    /// Routing priority:
    /// 1. If model starts with a registered `provider_id/`, route directly to that provider.
    /// 2. Match model name against provider `model_prefixes`:
    ///    - Providers with matching prefixes become candidates.
    ///    - Providers with empty `model_prefixes` are treated as wildcards (match any model).
    ///    - If specific prefix matches exist, only those providers are considered.
    ///    - If no specific prefix matches, wildcard providers are used.
    /// 3. Among candidates, select by dispatch mode:
    ///    - Exclusive providers first (sorted by priority, then config order)
    ///    - Pooled providers next (sorted by priority, then RR)
    ///    - Fallback providers last (sorted by priority, then config order)
    pub fn select(
        &self,
        model: &str,
        prev_failed: Option<&str>,
    ) -> ProviderSelection<'_> {
        // STEP 1: Check for provider_id/ prefix routing (highest priority)
        if let Some((pid, rest)) = self.extract_provider_id_prefix(model) {
            tracing::info!("[ProviderRouter] provider_id match: '{}' -> provider='{}', rest='{}'", model, pid, rest);
            if let Some(&idx) = self.provider_id_index.get(pid) {
                let provider = &self.providers[idx];
                if prev_failed.map_or(true, |pf| provider.name != pf) {
                    return ProviderSelection {
                        provider,
                        resolved_model: rest.to_string(),
                    };
                }
                // provider_id matched but provider failed; fall through to default selection
            }
        } else {
            tracing::info!("[ProviderRouter] no provider_id prefix match for '{}', falling through to wildcard selection", model);
        }

        // STEP 2: Model matching via model_prefixes or available_models
        let model_lower = model.to_lowercase();
        let mut specific_matches: Vec<&UpstreamProvider> = Vec::new();
        let mut wildcard_matches: Vec<&UpstreamProvider> = Vec::new();

        for p in &self.providers {
            if prev_failed.map_or(false, |pf| p.name == pf) {
                continue;
            }

            let has_match = if !p.model_prefixes.is_empty() {
                // Check model_prefixes
                p.model_prefixes.iter().any(|prefix| {
                    model_lower.starts_with(prefix.to_lowercase().as_str())
                })
            } else if let Some(ref avail) = p.available_models {
                // Check available_models (comma-separated list)
                avail.split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .any(|m| m == model_lower)
            } else {
                // No constraints = wildcard (matches any model)
                false
            };

            if has_match {
                specific_matches.push(p);
            } else if p.model_prefixes.is_empty() && p.available_models.is_none() {
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
                .filter(|p| prev_failed.map_or(true, |pf| p.name != pf))
                .collect()
        } else {
            prefix_candidates
        };

        if candidates.is_empty() {
            return ProviderSelection {
                provider: &self.providers[0],
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
                provider: p,
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
                provider: same_priority[idx],
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
            self.providers.iter().find(|p| prev_failed.map_or(true, |pf| p.name != pf)).unwrap_or(&self.providers[0])
        });
        ProviderSelection {
            provider: fallback_provider,
            resolved_model: model.to_string(),
        }
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

    fn make_test_provider(
        name: &str,
        dispatch_mode: ProviderDispatchMode,
        priority: u8,
        provider_id: Option<&str>,
    ) -> UpstreamProvider {
        UpstreamProvider {
            name: name.to_string(),
            provider_id: provider_id.map(String::from),
            enabled: true,
            base_url: "https://example.com".to_string(),
            api_key: "test-key".to_string(),
            protocol: ProviderProtocol::AnthropicPassthrough,
            dispatch_mode,
            priority,
            model_prefixes: Default::default(),
            model_mapping: Default::default(),
            available_models: None,
            request_timeout_secs: None,
        }
    }

    fn build_router_with_providers(providers: Vec<UpstreamProvider>) -> ProviderRouter {
        let mut provider_id_index: HashMap<String, usize> = HashMap::new();
        for (i, p) in providers.iter().enumerate() {
            if let Some(ref pid) = p.provider_id {
                provider_id_index.insert(pid.clone(), i);
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
        assert_eq!(router.map_model(&provider, "claude-sonnet-4-20250514"), "claude-sonnet-4-20250514");
        assert_eq!(router.map_model(&provider, "claude-opus-4-20250514"), "claude-opus-4-20250514");
        assert_eq!(router.map_model(&provider, "gemini-3-flash"), "gemini-3-flash");
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
