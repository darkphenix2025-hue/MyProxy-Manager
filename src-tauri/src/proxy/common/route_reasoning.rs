//! Per-route reasoning effort resolution and protocol normalization.
//!
//! A route stores one canonical value so the UI can remain stable while each
//! upstream protocol receives the field and value it understands.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::proxy::config::ProviderProtocol;
use crate::proxy::mappers::claude::models::{
    ClaudeRequest, OutputConfig, ThinkingConfig as ClaudeThinkingConfig,
};
use crate::proxy::mappers::openai::models::{
    OpenAIRequest, ThinkingConfig as OpenAIThinkingConfig,
};

/// Canonical values persisted by route configuration.
pub const CANONICAL_EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];

/// Separator used for target-specific route effort entries.
///
/// Legacy entries continue to use the original model template as the key;
/// new entries use `<template>::<resolved-target>` so each target in a 1:N
/// mapping can have its own reasoning setting.
const TARGET_EFFORT_SEPARATOR: &str = "::";

pub fn target_effort_key(route_pattern: &str, target: &str) -> String {
    format!("{route_pattern}{TARGET_EFFORT_SEPARATOR}{target}")
}

fn split_target_effort_key(key: &str) -> Option<(&str, &str)> {
    key.split_once(TARGET_EFFORT_SEPARATOR)
}

/// Resolve a route-level effort using the same exact-then-most-specific
/// wildcard precedence as model routing. Target-specific entries take
/// precedence over legacy template-level entries.
pub fn resolve_route_reasoning_effort(
    original_model: &str,
    resolved_model: Option<&str>,
    route_effort: &HashMap<String, String>,
) -> Option<String> {
    if let Some(resolved_model) = resolved_model {
        if let Some(value) = route_effort
            .get(&target_effort_key(original_model, resolved_model))
            .and_then(|value| canonical_effort(value))
        {
            return Some(value);
        }

        let mut best_target_match: Option<(&str, &str, usize)> = None;
        for (key, value) in route_effort {
            let Some((pattern, target)) = split_target_effort_key(key) else {
                continue;
            };
            if target != resolved_model
                || !pattern.contains('*')
                || !wildcard_match(pattern, original_model)
            {
                continue;
            }
            let specificity = pattern.chars().count() - pattern.matches('*').count();
            if best_target_match
                .as_ref()
                .map(|(_, _, current)| specificity > *current)
                .unwrap_or(true)
            {
                best_target_match = Some((pattern, value, specificity));
            }
        }
        if let Some((_, value, _)) = best_target_match {
            if let Some(value) = canonical_effort(value) {
                return Some(value);
            }
        }
    }

    if let Some(value) = route_effort.get(original_model) {
        return canonical_effort(value);
    }

    let mut best_match: Option<(&str, &str, usize)> = None;
    for (pattern, value) in route_effort {
        if split_target_effort_key(pattern).is_some() {
            continue;
        }
        if pattern.contains('*') && wildcard_match(pattern, original_model) {
            let specificity = pattern.chars().count() - pattern.matches('*').count();
            if best_match
                .as_ref()
                .map(|(_, _, current)| specificity > *current)
                .unwrap_or(true)
            {
                best_match = Some((pattern, value, specificity));
            }
        }
    }

    best_match.and_then(|(_, value, _)| canonical_effort(value))
}

/// Normalize UI aliases and reject values that must never be sent upstream.
pub fn canonical_effort(value: &str) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    let normalized = match normalized.as_str() {
        "light" => "low",
        "extra high" | "extra_high" | "extra-high" => "xhigh",
        "default" | "auto" | "none" | "" => return None,
        other => other,
    };

    CANONICAL_EFFORTS
        .contains(&normalized)
        .then(|| normalized.to_string())
}

/// Convert a canonical effort to the vocabulary supported by a target
/// protocol. The generic OpenAI and Gemini paths intentionally use their
/// highest common supported value for Codex-only levels.
pub fn normalize_for_protocol(effort: &str, protocol: &ProviderProtocol) -> Option<String> {
    let effort = canonical_effort(effort)?;
    let normalized = match protocol {
        ProviderProtocol::CodexResponses => effort,
        ProviderProtocol::OpenAICompatible => match effort.as_str() {
            "max" | "ultra" => "xhigh".to_string(),
            _ => effort,
        },
        ProviderProtocol::AnthropicPassthrough => match effort.as_str() {
            "ultra" => "max".to_string(),
            _ => effort,
        },
        ProviderProtocol::GeminiV1Internal => match effort.as_str() {
            "xhigh" | "max" | "ultra" => "high".to_string(),
            _ => effort,
        },
    };
    Some(normalized)
}

/// Convert effort to a Gemini thinking budget. These values are deliberately
/// conservative and are still capped by the model-specific transformer.
pub fn effort_to_gemini_budget(effort: &str) -> Option<u32> {
    match canonical_effort(effort)?.as_str() {
        "low" => Some(8_192),
        "medium" => Some(16_384),
        "high" => Some(24_576),
        "xhigh" | "max" | "ultra" => Some(32_768),
        _ => None,
    }
}

/// Apply a route effort to a native Gemini request body.  Gemini expresses
/// reasoning as a token budget nested under `generationConfig` rather than a
/// top-level effort string.
pub fn apply_to_gemini_body(body: &mut Value, effort: &str) {
    let Some(budget) = effort_to_gemini_budget(effort) else {
        return;
    };
    let Some(object) = body.as_object_mut() else {
        return;
    };
    let generation_config = object
        .entry("generationConfig".to_string())
        .or_insert_with(|| json!({}));
    let Some(generation_config) = generation_config.as_object_mut() else {
        return;
    };
    let thinking_config = generation_config
        .entry("thinkingConfig".to_string())
        .or_insert_with(|| json!({}));
    let Some(thinking_config) = thinking_config.as_object_mut() else {
        return;
    };
    thinking_config.insert("thinkingBudget".to_string(), json!(budget));
    thinking_config
        .entry("includeThoughts".to_string())
        .or_insert(json!(true));
}

/// Apply a route effort to the normalized OpenAI request before protocol
/// conversion.
pub fn apply_to_openai_request(
    request: &mut OpenAIRequest,
    effort: &str,
    protocol: &ProviderProtocol,
) {
    let Some(normalized) = normalize_for_protocol(effort, protocol) else {
        return;
    };

    match protocol {
        ProviderProtocol::OpenAICompatible => {
            request.reasoning_effort = Some(normalized);
        }
        ProviderProtocol::AnthropicPassthrough => {
            request.thinking = Some(OpenAIThinkingConfig {
                thinking_type: Some("adaptive".to_string()),
                budget_tokens: None,
                effort: Some(normalized),
            });
        }
        ProviderProtocol::GeminiV1Internal => {
            if let Some(budget) = effort_to_gemini_budget(effort) {
                request.thinking = Some(OpenAIThinkingConfig {
                    thinking_type: Some("enabled".to_string()),
                    budget_tokens: Some(budget),
                    effort: Some(normalized),
                });
            }
        }
        ProviderProtocol::CodexResponses => {
            // Codex Responses requests use the raw JSON path. Nothing is
            // serialized from OpenAIRequest for this protocol.
        }
    }
}

/// Apply a route effort to a Claude-format request before it is converted to
/// another upstream protocol.
pub fn apply_to_claude_request_for_protocol(
    request: &mut ClaudeRequest,
    effort: &str,
    protocol: &ProviderProtocol,
) {
    let Some(normalized) = normalize_for_protocol(effort, protocol) else {
        return;
    };

    match protocol {
        ProviderProtocol::AnthropicPassthrough | ProviderProtocol::OpenAICompatible => {
            request.thinking = Some(ClaudeThinkingConfig {
                type_: "adaptive".to_string(),
                budget_tokens: None,
                effort: None,
            });
            request.output_config = Some(OutputConfig {
                effort: Some(normalized),
            });
        }
        ProviderProtocol::GeminiV1Internal => {
            request.thinking = Some(ClaudeThinkingConfig {
                type_: "enabled".to_string(),
                budget_tokens: effort_to_gemini_budget(effort),
                effort: None,
            });
            request.output_config = None;
        }
        ProviderProtocol::CodexResponses => {
            // Claude requests are converted to a native Responses payload
            // after this point. Keep the route override in the Claude-shaped
            // request so the converter can emit reasoning.effort without
            // allowing the client's output_config to win.
            request.thinking = Some(ClaudeThinkingConfig {
                type_: "adaptive".to_string(),
                budget_tokens: None,
                effort: Some(normalized.clone()),
            });
            request.output_config = Some(OutputConfig {
                effort: Some(normalized),
            });
        }
    }
}

/// Apply a route effort to a Claude JSON body produced by a translator.
pub fn apply_to_claude_body(body: &mut Value, effort: &str) {
    let Some(normalized) = normalize_for_protocol(effort, &ProviderProtocol::AnthropicPassthrough)
    else {
        return;
    };
    let Some(object) = body.as_object_mut() else {
        return;
    };

    object.insert("thinking".to_string(), json!({ "type": "adaptive" }));
    object.insert("output_config".to_string(), json!({ "effort": normalized }));
}

/// Apply a route effort to a native Codex Responses JSON body.
pub fn apply_to_codex_body(body: &mut Value, effort: &str) {
    let Some(normalized) = normalize_for_protocol(effort, &ProviderProtocol::CodexResponses) else {
        return;
    };
    let Some(object) = body.as_object_mut() else {
        return;
    };

    let reasoning = object
        .entry("reasoning".to_string())
        .or_insert_with(|| json!({}));
    if !reasoning.is_object() {
        *reasoning = json!({});
    }
    if let Some(reasoning_object) = reasoning.as_object_mut() {
        reasoning_object.insert("effort".to_string(), json!(normalized));
    }
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }

    let mut text_pos = 0;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 {
            if !text[text_pos..].starts_with(part) {
                return false;
            }
            text_pos += part.len();
        } else if index == parts.len() - 1 {
            return text[text_pos..].ends_with(part);
        } else if let Some(position) = text[text_pos..].find(part) {
            text_pos += position + part.len();
        } else {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_route_effort_wins_over_wildcard() {
        let route_effort = HashMap::from([
            ("claude-*".to_string(), "low".to_string()),
            ("claude-opus-*".to_string(), "high".to_string()),
            ("claude-opus-4".to_string(), "max".to_string()),
        ]);

        assert_eq!(
            resolve_route_reasoning_effort("claude-opus-4", None, &route_effort),
            Some("max".to_string())
        );
        assert_eq!(
            resolve_route_reasoning_effort("claude-opus-4-6", None, &route_effort),
            Some("high".to_string())
        );
    }

    #[test]
    fn target_specific_effort_wins_over_legacy_template_effort() {
        let route_effort = HashMap::from([
            ("claude-*".to_string(), "low".to_string()),
            (
                target_effort_key("claude-*", "big/claude-sonnet"),
                "high".to_string(),
            ),
        ]);

        assert_eq!(
            resolve_route_reasoning_effort(
                "claude-sonnet-4-6",
                Some("big/claude-sonnet"),
                &route_effort,
            ),
            Some("high".to_string())
        );
        assert_eq!(
            resolve_route_reasoning_effort("claude-sonnet-4-6", None, &route_effort),
            Some("low".to_string())
        );
    }

    #[test]
    fn target_specific_wildcard_effort_only_matches_its_template() {
        let route_effort = HashMap::from([
            (target_effort_key("gpt-*", "big/gpt-5"), "high".to_string()),
            (
                target_effort_key("claude-*", "big/gpt-5"),
                "low".to_string(),
            ),
        ]);

        assert_eq!(
            resolve_route_reasoning_effort("gpt-5.4", Some("big/gpt-5"), &route_effort),
            Some("high".to_string())
        );
        assert_eq!(
            resolve_route_reasoning_effort("claude-sonnet-4-6", Some("big/gpt-5"), &route_effort),
            Some("low".to_string())
        );
        assert_eq!(
            resolve_route_reasoning_effort("gemini-2.5-pro", Some("big/gpt-5"), &route_effort),
            None
        );
    }

    #[test]
    fn aliases_and_protocol_specific_levels_are_normalized() {
        assert_eq!(canonical_effort("light"), Some("low".to_string()));
        assert_eq!(canonical_effort("extra high"), Some("xhigh".to_string()));
        assert_eq!(
            normalize_for_protocol("ultra", &ProviderProtocol::OpenAICompatible),
            Some("xhigh".to_string())
        );
        assert_eq!(
            normalize_for_protocol("ultra", &ProviderProtocol::AnthropicPassthrough),
            Some("max".to_string())
        );
    }

    #[test]
    fn codex_body_receives_reasoning_effort() {
        let mut body = json!({ "model": "gpt-5.6-sol" });
        apply_to_codex_body(&mut body, "xhigh");
        assert_eq!(body["reasoning"]["effort"], "xhigh");
    }

    #[test]
    fn openai_and_anthropic_requests_receive_protocol_fields() {
        let mut openai_request = OpenAIRequest {
            model: "gpt-5".to_string(),
            ..OpenAIRequest::default()
        };
        apply_to_openai_request(
            &mut openai_request,
            "max",
            &ProviderProtocol::OpenAICompatible,
        );
        assert_eq!(openai_request.reasoning_effort.as_deref(), Some("xhigh"));

        let mut claude_request: ClaudeRequest = serde_json::from_value(json!({
            "model": "claude-opus-4-6",
            "messages": []
        }))
        .expect("minimal Claude request should deserialize");
        apply_to_claude_request_for_protocol(
            &mut claude_request,
            "medium",
            &ProviderProtocol::AnthropicPassthrough,
        );
        assert_eq!(
            claude_request
                .thinking
                .as_ref()
                .map(|thinking| thinking.type_.as_str()),
            Some("adaptive")
        );
        assert_eq!(
            claude_request
                .output_config
                .as_ref()
                .and_then(|config| config.effort.as_deref()),
            Some("medium")
        );

        apply_to_claude_request_for_protocol(
            &mut claude_request,
            "xhigh",
            &ProviderProtocol::CodexResponses,
        );
        assert_eq!(
            claude_request
                .output_config
                .as_ref()
                .and_then(|config| config.effort.as_deref()),
            Some("xhigh")
        );
    }

    #[test]
    fn gemini_effort_becomes_explicit_budget() {
        let mut request: ClaudeRequest = serde_json::from_value(json!({
            "model": "gemini-3-pro",
            "messages": []
        }))
        .expect("minimal Claude request should deserialize");
        apply_to_claude_request_for_protocol(
            &mut request,
            "high",
            &ProviderProtocol::GeminiV1Internal,
        );
        assert_eq!(
            request
                .thinking
                .as_ref()
                .and_then(|thinking| thinking.budget_tokens),
            Some(24_576)
        );
        assert!(request.output_config.is_none());
    }

    #[test]
    fn native_gemini_body_receives_reasoning_budget() {
        let mut body = json!({ "contents": [] });
        apply_to_gemini_body(&mut body, "medium");
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["thinkingBudget"],
            16_384
        );
        assert_eq!(
            body["generationConfig"]["thinkingConfig"]["includeThoughts"],
            true
        );
    }
}
