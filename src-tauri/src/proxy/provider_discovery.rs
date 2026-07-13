//! Provider Model Discovery - 自动同步供应商可用模型列表
//!
//! 支持从不同协议的供应商 API 自动发现可用模型列表：
//! - OpenAI 兼容: GET /v1/models
//! - Ollama: GET /api/tags
//! - Anthropic: GET /v1/models
//!
//! 同步后可选地更新配置文件并热更新 ProviderRouter。

use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::proxy::config::{ProviderProtocol, UpstreamProvider};

/// 模型发现结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredModel {
    pub id: String,
    pub name: Option<String>,
    pub created: Option<u64>,
    pub owned_by: Option<String>,
}

/// 从供应商发现模型列表
pub async fn discover_provider_models(
    provider: &UpstreamProvider,
) -> Result<Vec<DiscoveredModel>, String> {
    if !provider.enabled {
        return Err("Provider is disabled".into());
    }

    let client = Client::builder()
        .timeout(Duration::from_secs(
            provider.request_timeout_secs.unwrap_or(15).min(30) as u64
        ))
        .build()
        .map_err(|e| format!("Failed to build client: {}", e))?;

    match provider.protocol {
        ProviderProtocol::OpenAICompatible => {
            discover_openai_models(&client, provider).await
        }
        ProviderProtocol::AnthropicPassthrough => {
            discover_anthropic_models(&client, provider).await
        }
        ProviderProtocol::GeminiV1Internal => {
            // Gemini 供应商也使用 OpenAI 兼容端点发现模型
            discover_openai_models(&client, provider).await
        }
    }
}

/// OpenAI 兼容协议: GET /v1/models
async fn discover_openai_models(
    client: &Client,
    provider: &UpstreamProvider,
) -> Result<Vec<DiscoveredModel>, String> {
    let base = provider.base_url.trim_end_matches('/');
    let url = format!("{}/v1/models", base);

    let mut request = client.get(&url);
    if !provider.api_key.is_empty() {
        request = request.header("Authorization", format!("Bearer {}", provider.api_key));
    }

    let resp = request
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "HTTP {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        ));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Parse JSON failed: {}", e))?;

    let models = body
        .get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(|v| v.as_str())?.to_string();
                    let name = m.get("name").and_then(|v| v.as_str()).map(String::from);
                    let created = m.get("created").and_then(|v| v.as_u64());
                    let owned_by = m
                        .get("owned_by")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    Some(DiscoveredModel {
                        id,
                        name,
                        created,
                        owned_by,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(models)
}

/// Anthropic 协议: GET /v1/models
async fn discover_anthropic_models(
    client: &Client,
    provider: &UpstreamProvider,
) -> Result<Vec<DiscoveredModel>, String> {
    let base = provider.base_url.trim_end_matches('/');
    let url = format!("{}/v1/models", base);

    let mut request = client
        .get(&url)
        .header("anthropic-version", "2023-06-01");
    if !provider.api_key.is_empty() {
        request = request.header("x-api-key", &provider.api_key);
    }

    let resp = request
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "HTTP {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        ));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Parse JSON failed: {}", e))?;

    let models = body
        .get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m.get("id").and_then(|v| v.as_str())?.to_string();
                    Some(DiscoveredModel {
                        id,
                        name: None,
                        created: None,
                        owned_by: None,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(models)
}

/// Ollama 原生: GET /api/tags
#[allow(dead_code)]
async fn discover_ollama_tags(
    client: &Client,
    provider: &UpstreamProvider,
) -> Result<Vec<DiscoveredModel>, String> {
    let base = provider.base_url.trim_end_matches('/');
    let url = format!("{}/api/tags", base);

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!(
            "HTTP {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        ));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Parse JSON failed: {}", e))?;

    let models = body
        .get("models")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| {
                    let id = m
                        .get("name")
                        .or_else(|| m.get("model"))
                        .and_then(|v| v.as_str())?
                        .to_string();
                    let digest = m.get("digest").and_then(|v| v.as_str()).map(String::from);
                    Some(DiscoveredModel {
                        id,
                        name: digest,
                        created: None,
                        owned_by: Some("ollama".into()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(models)
}

/// 将发现的模型列表转换为逗号分隔的字符串（用于 available_models 字段）
pub fn models_to_csv(models: &[DiscoveredModel]) -> String {
    models.iter().map(|m| m.id.clone()).collect::<Vec<_>>().join(", ")
}

/// 同步单个供应商的模型列表，返回更新后的模型列表
#[allow(dead_code)]
pub async fn sync_provider_models(
    provider: &UpstreamProvider,
) -> Result<Vec<String>, String> {
    let discovered = discover_provider_models(provider).await?;
    let model_ids: Vec<String> = discovered.iter().map(|m| m.id.clone()).collect();

    tracing::info!(
        "[ModelDiscovery] Provider '{}' discovered {} models: {}",
        provider.name,
        model_ids.len(),
        model_ids.join(", ")
    );

    Ok(model_ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_models_to_csv() {
        let models = vec![
            DiscoveredModel { id: "model-a".into(), name: None, created: None, owned_by: None },
            DiscoveredModel { id: "model-b".into(), name: None, created: None, owned_by: None },
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
