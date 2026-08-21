use serde::{Deserialize, Serialize};
// use std::path::PathBuf;
use std::collections::HashMap;
use std::fmt;
use std::sync::{OnceLock, RwLock};

// ============================================================================
// 辅助工具函数
// ============================================================================

/// 标准化代理 URL，如果缺失协议则默认补全 http://
pub fn normalize_proxy_url(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }
    if !url.contains("://") {
        format!("http://{}", url)
    } else {
        url.to_string()
    }
}

// ============================================================================
// 全局 Thinking Budget 配置存储
// 用于在 request transform 函数中访问配置（无需修改函数签名）
// ============================================================================
static GLOBAL_THINKING_BUDGET_CONFIG: OnceLock<RwLock<ThinkingBudgetConfig>> = OnceLock::new();

/// 获取当前 Thinking Budget 配置
pub fn get_thinking_budget_config() -> ThinkingBudgetConfig {
    GLOBAL_THINKING_BUDGET_CONFIG
        .get()
        .and_then(|lock| lock.read().ok())
        .map(|cfg| cfg.clone())
        .unwrap_or_default()
}

/// 更新全局 Thinking Budget 配置
pub fn update_thinking_budget_config(config: ThinkingBudgetConfig) {
    if let Some(lock) = GLOBAL_THINKING_BUDGET_CONFIG.get() {
        if let Ok(mut cfg) = lock.write() {
            *cfg = config.clone();
            tracing::info!(
                "[Thinking-Budget] Global config updated: mode={:?}, custom_value={}",
                config.mode,
                config.custom_value
            );
        }
    } else {
        // 首次初始化
        let _ = GLOBAL_THINKING_BUDGET_CONFIG.set(RwLock::new(config.clone()));
        tracing::info!(
            "[Thinking-Budget] Global config initialized: mode={:?}, custom_value={}",
            config.mode,
            config.custom_value
        );
    }
}

// ============================================================================
// 全局系统提示词配置存储
// 用户可在设置中配置一段全局提示词，自动注入到所有请求的 systemInstruction 中
// ============================================================================
static GLOBAL_SYSTEM_PROMPT_CONFIG: OnceLock<RwLock<GlobalSystemPromptConfig>> = OnceLock::new();

/// 获取当前全局系统提示词配置
pub fn get_global_system_prompt() -> GlobalSystemPromptConfig {
    GLOBAL_SYSTEM_PROMPT_CONFIG
        .get()
        .and_then(|lock| lock.read().ok())
        .map(|cfg| cfg.clone())
        .unwrap_or_default()
}

/// 更新全局系统提示词配置
pub fn update_global_system_prompt_config(config: GlobalSystemPromptConfig) {
    if let Some(lock) = GLOBAL_SYSTEM_PROMPT_CONFIG.get() {
        if let Ok(mut cfg) = lock.write() {
            *cfg = config.clone();
            tracing::info!(
                "[Global-System-Prompt] Config updated: enabled={}, content_len={}",
                config.enabled,
                config.content.len()
            );
        }
    } else {
        // 首次初始化
        let _ = GLOBAL_SYSTEM_PROMPT_CONFIG.set(RwLock::new(config.clone()));
        tracing::info!(
            "[Global-System-Prompt] Config initialized: enabled={}, content_len={}",
            config.enabled,
            config.content.len()
        );
    }
}

// ============================================================================
// 全局图像思维模式配置存储
// ============================================================================
static GLOBAL_IMAGE_THINKING_MODE: OnceLock<RwLock<String>> = OnceLock::new();

pub fn get_image_thinking_mode() -> String {
    GLOBAL_IMAGE_THINKING_MODE
        .get()
        .and_then(|lock| lock.read().ok())
        .map(|s| s.clone())
        .unwrap_or_else(|| "enabled".to_string())
}

pub fn update_image_thinking_mode(mode: Option<String>) {
    let val = mode.unwrap_or_else(|| "enabled".to_string());
    if let Some(lock) = GLOBAL_IMAGE_THINKING_MODE.get() {
        if let Ok(mut cfg) = lock.write() {
            if *cfg != val {
                *cfg = val.clone();
                tracing::info!("[Image-Thinking] Global config updated: {}", val);
            }
        }
    } else {
        let _ = GLOBAL_IMAGE_THINKING_MODE.set(RwLock::new(val.clone()));
    }
}

/// 全局系统提示词配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GlobalSystemPromptConfig {
    /// 是否启用全局系统提示词
    #[serde(default)]
    pub enabled: bool,
    /// 系统提示词内容
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ProxyAuthMode {
    Off,
    Strict,
    AllExceptHealth,
    #[default]
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ZaiDispatchMode {
    /// Never use z.ai.
    #[default]
    Off,
    /// Use z.ai for all Anthropic protocol requests.
    Exclusive,
    /// Treat z.ai as one additional slot in the shared pool.
    Pooled,
    /// Use z.ai only when the Google pool is unavailable.
    Fallback,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ZaiMcpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub web_search_enabled: bool,
    #[serde(default)]
    pub web_reader_enabled: bool,
    #[serde(default)]
    pub vision_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZaiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_zai_base_url")]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub dispatch_mode: ZaiDispatchMode,
    /// Optional per-model mapping overrides for Anthropic/Claude model ids.
    /// Key: incoming `model` string, Value: upstream z.ai model id (e.g. `glm-4.7`).
    #[serde(default)]
    pub model_mapping: HashMap<String, String>,
    #[serde(default)]
    pub mcp: ZaiMcpConfig,
}

impl Default for ZaiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: default_zai_base_url(),
            api_key: String::new(),
            dispatch_mode: ZaiDispatchMode::Off,
            model_mapping: HashMap::new(),
            mcp: ZaiMcpConfig::default(),
        }
    }
}

/// Generic upstream provider protocol type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ProviderProtocol {
    /// Provider speaks Anthropic Messages API. Requests forwarded as-is.
    #[default]
    AnthropicPassthrough,
    /// Provider speaks OpenAI Chat Completions API.
    #[serde(alias = "openai_compatible")]
    OpenAICompatible,
    /// Provider speaks the native OpenAI Codex Responses API.
    CodexResponses,
    /// Provider speaks Google v1internal (Gemini) API.
    /// Requests are transformed from OpenAI/Claude to Gemini format.
    GeminiV1Internal,
}

impl ProviderProtocol {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::AnthropicPassthrough => "anthropic_passthrough",
            Self::OpenAICompatible => "open_a_i_compatible",
            Self::CodexResponses => "codex_responses",
            Self::GeminiV1Internal => "gemini_v1_internal",
        }
    }
}

/// Dispatch mode for a provider within the routing chain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ProviderDispatchMode {
    /// Always use this provider for matching models.
    Exclusive,
    /// Round-robin among pooled providers with same model capability.
    #[default]
    Pooled,
    /// Only when higher-priority providers fail.
    Fallback,
}

/// A single model exposed by an upstream provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderModelConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub supports_images: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasoning_efforts: Vec<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Protocol-specific connection and routing settings belonging to one logical
/// provider.  Model metadata intentionally stays on [`UpstreamProvider`] so a
/// provider such as BIGMODEL can expose one shared model catalog to all of its
/// enabled protocols.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderProtocolConfig {
    #[serde(default)]
    pub protocol: ProviderProtocol,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    #[serde(default)]
    pub dispatch_mode: ProviderDispatchMode,
    #[serde(default = "default_provider_priority")]
    pub priority: u8,
    #[serde(default)]
    pub model_prefixes: Vec<String>,
    #[serde(default)]
    pub model_mapping: HashMap<String, String>,
    #[serde(default)]
    pub request_timeout_secs: Option<u64>,
}

/// Model route used when the requested model has no valid provider match.
///
/// The effort is stored in the same protocol-neutral vocabulary as normal
/// route overrides and is translated immediately before forwarding.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FallbackModelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub reasoning_effort: String,
}

/// Runtime cooldown policy used after an upstream provider returns HTTP 429.
/// Cooldown entries themselves are intentionally kept in memory by the proxy
/// runtime and are not persisted in the application configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCooldownConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_model_cooldown_duration")]
    pub duration_secs: u64,
}

fn default_model_cooldown_duration() -> u64 {
    600
}

impl Default for ModelCooldownConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            duration_secs: default_model_cooldown_duration(),
        }
    }
}

/// A single upstream provider configured by API Key + Base URL.
#[derive(Clone, Serialize, Deserialize)]
pub struct UpstreamProvider {
    pub name: String,
    /// Short routing identifier (1-10 chars, alphanumeric, non-digit start).
    /// Enables direct routing via `provider_id/model_name` format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Older provider IDs kept as aliases when multiple protocol-specific
    /// entries are merged into one logical provider (for example `bigop`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_id_aliases: Vec<String>,
    /// Stable grouping key used by the UI when migrating legacy split entries
    /// into one logical provider record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_group: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    /// Optional credential managed by the provider auth-file runtime.
    /// When set, the current access token is resolved at request time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_id: Option<String>,
    /// Runtime-only ChatGPT account identifier required by Codex upstreams.
    /// It is resolved from the auth-file and is never persisted with provider config.
    #[serde(skip)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub protocol: ProviderProtocol,
    #[serde(default)]
    pub dispatch_mode: ProviderDispatchMode,
    #[serde(default = "default_provider_priority")]
    pub priority: u8,
    /// Model prefix patterns this provider supports (empty = wildcard).
    #[serde(default)]
    pub model_prefixes: Vec<String>,
    /// Explicit per-model overrides: incoming model -> upstream model name.
    #[serde(default)]
    pub model_mapping: HashMap<String, String>,
    /// Comma-separated list of available models, e.g. "gemini-2.5-pro,gemini-2.5-flash".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_models: Option<String>,
    /// Detailed model records used by the provider editor and route model picker.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_configs: Vec<ProviderModelConfig>,
    /// Enabled protocol variants for this logical provider.  Empty means the
    /// legacy single-protocol fields above are authoritative.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocols: Vec<ProviderProtocolConfig>,
    /// Optional HTTP request timeout in seconds.
    #[serde(default)]
    pub request_timeout_secs: Option<u64>,
}

impl UpstreamProvider {
    /// Expand one logical provider into routable protocol-specific entries.
    /// The router uses these entries internally while the config/UI continue
    /// to present a single provider and a shared model catalog.
    pub fn protocol_variants(&self) -> Vec<Self> {
        if self.protocols.is_empty() {
            return vec![self.clone()];
        }

        self.protocols
            .iter()
            .filter(|protocol| protocol.enabled)
            .map(|protocol| {
                let mut provider = self.clone();
                provider.protocol = protocol.protocol.clone();
                provider.base_url = protocol.base_url.clone();
                provider.api_key = protocol.api_key.clone();
                provider.credential_id = protocol.credential_id.clone();
                provider.dispatch_mode = protocol.dispatch_mode.clone();
                provider.priority = protocol.priority;
                provider.model_prefixes = protocol.model_prefixes.clone();
                provider.model_mapping = protocol.model_mapping.clone();
                provider.request_timeout_secs = protocol.request_timeout_secs;
                provider.protocols.clear();
                provider
            })
            .collect()
    }
}

fn provider_name_without_protocol_suffix(name: &str) -> String {
    let trimmed = name.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.ends_with("op") {
        let without_suffix = &trimmed[..trimmed.len() - 2];
        let without_separator = without_suffix.trim_end_matches(['_', '-', ' ']);
        if !without_separator.is_empty() {
            return without_separator.to_string();
        }
    }
    trimmed.to_string()
}

fn provider_group_key(provider: &UpstreamProvider) -> String {
    provider
        .provider_group
        .as_deref()
        .map(|group| group.trim().to_ascii_lowercase())
        .filter(|group| !group.is_empty())
        .unwrap_or_else(|| {
            provider_name_without_protocol_suffix(&provider.name).to_ascii_lowercase()
        })
}

fn legacy_provider_protocol_config(provider: &UpstreamProvider) -> ProviderProtocolConfig {
    ProviderProtocolConfig {
        protocol: provider.protocol.clone(),
        enabled: provider.enabled,
        base_url: provider.base_url.clone(),
        api_key: provider.api_key.clone(),
        credential_id: provider.credential_id.clone(),
        dispatch_mode: provider.dispatch_mode.clone(),
        priority: provider.priority,
        model_prefixes: provider.model_prefixes.clone(),
        model_mapping: provider.model_mapping.clone(),
        request_timeout_secs: provider.request_timeout_secs,
    }
}

/// Merge legacy entries that represented one upstream service once per
/// protocol.  This is used at runtime as well as by the UI migration so an
/// old `BIGMODEL` + `BIGMODEL_OP` configuration immediately supports one
/// shared model route and protocol-aware dispatch, even before it is saved
/// again from the editor.
pub fn merge_provider_protocol_entries(providers: Vec<UpstreamProvider>) -> Vec<UpstreamProvider> {
    let mut groups: Vec<(String, Vec<UpstreamProvider>)> = Vec::new();
    for provider in providers {
        let key = provider_group_key(&provider);
        if let Some((_, members)) = groups.iter_mut().find(|(group_key, _)| group_key == &key) {
            members.push(provider);
        } else {
            groups.push((key, vec![provider]));
        }
    }

    groups
        .into_iter()
        .map(|(group_key, members)| {
            if members.len() == 1 {
                return members
                    .into_iter()
                    .next()
                    .expect("provider group is not empty");
            }

            let base = members
                .iter()
                .find(|provider| {
                    provider_name_without_protocol_suffix(&provider.name)
                        .eq_ignore_ascii_case(provider.name.trim())
                })
                .unwrap_or(&members[0]);

            let mut protocols: Vec<ProviderProtocolConfig> = Vec::new();
            for member in &members {
                let member_protocols = if member.protocols.is_empty() {
                    vec![legacy_provider_protocol_config(member)]
                } else {
                    member.protocols.clone()
                };
                for mut protocol in member_protocols {
                    protocol.enabled &= member.enabled;
                    if let Some(existing) = protocols
                        .iter_mut()
                        .find(|existing| existing.protocol == protocol.protocol)
                    {
                        existing.enabled |= protocol.enabled;
                    } else {
                        protocols.push(protocol);
                    }
                }
            }

            let active_protocol = protocols
                .iter()
                .find(|protocol| protocol.enabled)
                .or_else(|| protocols.first())
                .expect("provider group has at least one protocol");

            let mut model_configs = Vec::new();
            let mut model_ids = Vec::new();
            for member in &members {
                for model in &member.model_configs {
                    if !model_ids.contains(&model.id) {
                        model_ids.push(model.id.clone());
                        model_configs.push(model.clone());
                    }
                }
                if let Some(available_models) = &member.available_models {
                    for model_id in available_models
                        .split(',')
                        .map(str::trim)
                        .filter(|id| !id.is_empty())
                    {
                        if !model_ids.iter().any(|known| known == model_id) {
                            model_ids.push(model_id.to_string());
                            model_configs.push(ProviderModelConfig {
                                id: model_id.to_string(),
                                display_name: None,
                                alias: None,
                                supports_images: false,
                                reasoning_efforts: Vec::new(),
                            });
                        }
                    }
                }
            }

            let provider_ids: Vec<String> = members
                .iter()
                .flat_map(|member| {
                    member
                        .provider_id
                        .iter()
                        .chain(member.provider_id_aliases.iter())
                        .cloned()
                })
                .collect();
            let primary_provider_id = base
                .provider_id
                .clone()
                .or_else(|| provider_ids.first().cloned());
            let provider_id_aliases = provider_ids
                .into_iter()
                .filter(|provider_id| primary_provider_id.as_deref() != Some(provider_id.as_str()))
                .fold(Vec::new(), |mut aliases, provider_id| {
                    if !aliases.contains(&provider_id) {
                        aliases.push(provider_id);
                    }
                    aliases
                });

            let mut merged = base.clone();
            merged.name = provider_name_without_protocol_suffix(&base.name);
            merged.provider_id = primary_provider_id;
            merged.provider_id_aliases = provider_id_aliases;
            merged.provider_group = Some(group_key);
            merged.enabled = members.iter().any(|provider| provider.enabled);
            merged.protocol = active_protocol.protocol.clone();
            merged.base_url = active_protocol.base_url.clone();
            merged.api_key = active_protocol.api_key.clone();
            merged.credential_id = active_protocol.credential_id.clone();
            merged.dispatch_mode = active_protocol.dispatch_mode.clone();
            merged.priority = active_protocol.priority;
            merged.model_prefixes = active_protocol.model_prefixes.clone();
            merged.model_mapping = active_protocol.model_mapping.clone();
            merged.request_timeout_secs = active_protocol.request_timeout_secs;
            merged.available_models = (!model_ids.is_empty()).then(|| model_ids.join(", "));
            merged.model_configs = model_configs;
            merged.protocols = protocols;
            merged
        })
        .collect()
}

impl fmt::Debug for UpstreamProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UpstreamProvider")
            .field("name", &self.name)
            .field("provider_id", &self.provider_id)
            .field("provider_id_aliases", &self.provider_id_aliases)
            .field("provider_group", &self.provider_group)
            .field("enabled", &self.enabled)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("credential_id", &self.credential_id)
            .field(
                "account_id",
                &self.account_id.as_ref().map(|_| "[REDACTED]"),
            )
            .field("protocol", &self.protocol)
            .field("dispatch_mode", &self.dispatch_mode)
            .field("priority", &self.priority)
            .field("model_prefixes", &self.model_prefixes)
            .field("model_mapping", &self.model_mapping)
            .field("available_models", &self.available_models)
            .field("model_configs", &self.model_configs)
            .field("protocols", &self.protocols)
            .field("request_timeout_secs", &self.request_timeout_secs)
            .finish()
    }
}

/// Validate that a provider_id matches the allowed format:
/// 1-10 chars, alphanumeric, first char must be a letter.
pub fn is_valid_provider_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 10
        && id.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && id.chars().all(|c| c.is_ascii_alphanumeric())
}

fn default_provider_priority() -> u8 {
    0
}

/// 实验性功能配置 (Feature Flags)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperimentalConfig {
    /// 启用双层签名缓存 (Signature Cache)
    #[serde(default = "default_true")]
    pub enable_signature_cache: bool,

    /// 启用工具循环自动恢复 (Tool Loop Recovery)
    #[serde(default = "default_true")]
    pub enable_tool_loop_recovery: bool,

    /// 启用跨模型兼容性检查 (Cross-Model Checks)
    #[serde(default = "default_true")]
    pub enable_cross_model_checks: bool,

    /// 启用上下文用量缩放 (Context Usage Scaling)
    /// 激进模式: 缩放用量并激活自动压缩以突破 200k 限制
    /// 默认关闭以保持透明度,让客户端能触发原生压缩指令
    #[serde(default = "default_false")]
    pub enable_usage_scaling: bool,

    /// 上下文压缩阈值 L1 (Tool Trimming)
    #[serde(default = "default_threshold_l1")]
    pub context_compression_threshold_l1: f32,

    /// 上下文压缩阈值 L2 (Thinking Compression)
    #[serde(default = "default_threshold_l2")]
    pub context_compression_threshold_l2: f32,

    /// 上下文压缩阈值 L3 (Fork + Summary)
    #[serde(default = "default_threshold_l3")]
    pub context_compression_threshold_l3: f32,
}

impl Default for ExperimentalConfig {
    fn default() -> Self {
        Self {
            enable_signature_cache: true,
            enable_tool_loop_recovery: true,
            enable_cross_model_checks: true,
            enable_usage_scaling: false, // 默认关闭,回归透明模式
            context_compression_threshold_l1: 0.4,
            context_compression_threshold_l2: 0.55,
            context_compression_threshold_l3: 0.7,
        }
    }
}

fn default_threshold_l1() -> f32 {
    0.4
}
fn default_threshold_l2() -> f32 {
    0.55
}
fn default_threshold_l3() -> f32 {
    0.7
}

/// Thinking Budget 模式
/// 控制如何处理调用方传入的 thinking_budget 参数
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ThinkingBudgetMode {
    /// 自动限制：对特定模型（Flash/Thinking）应用 24576 上限
    #[default]
    Auto,
    /// 透传：完全使用调用方传入的值，不做任何修改
    Passthrough,
    /// 自定义：使用用户设定的固定值覆盖所有请求
    Custom,
    /// 自适应：使用 effort 参数控制思考强度 (Claude 4.6+)
    Adaptive,
}

/// Thinking Budget 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThinkingBudgetConfig {
    /// 模式选择
    #[serde(default)]
    pub mode: ThinkingBudgetMode,
    /// 自定义固定值（仅在 mode=Custom 时生效）
    #[serde(default = "default_thinking_budget_custom_value")]
    pub custom_value: u32,
    /// 思考强度 (仅在 mode=Adaptive 时生效) : low, medium, high
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl Default for ThinkingBudgetConfig {
    fn default() -> Self {
        Self {
            mode: ThinkingBudgetMode::Auto,
            custom_value: default_thinking_budget_custom_value(),
            effort: None,
        }
    }
}

fn default_thinking_budget_custom_value() -> u32 {
    24576
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DebugLoggingConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub output_dir: Option<String>,
}

/// IP 黑名单配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpBlacklistConfig {
    /// 是否启用黑名单
    #[serde(default)]
    pub enabled: bool,

    /// 自定义封禁消息
    #[serde(default = "default_block_message")]
    pub block_message: String,
}

impl Default for IpBlacklistConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            block_message: default_block_message(),
        }
    }
}

fn default_block_message() -> String {
    "Access denied".to_string()
}

/// IP 白名单配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpWhitelistConfig {
    /// 是否启用白名单模式 (启用后只允许白名单IP访问)
    #[serde(default)]
    pub enabled: bool,

    /// 白名单优先模式 (白名单IP跳过黑名单检查)
    #[serde(default = "default_true")]
    pub whitelist_priority: bool,
}

impl Default for IpWhitelistConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            whitelist_priority: true,
        }
    }
}

/// 安全监控配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SecurityMonitorConfig {
    /// IP 黑名单配置
    #[serde(default)]
    pub blacklist: IpBlacklistConfig,

    /// IP 白名单配置
    #[serde(default)]
    pub whitelist: IpWhitelistConfig,
}

/// 一个模型路由规则的目标值。
///
/// 字符串形式用于兼容旧版配置；数组形式用于保存 1 对 N 的加权目标。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum CustomMappingValue {
    Single(String),
    Weighted(Vec<WeightedTarget>),
}

/// 1 对 N 路由目标及其整数比例权重；实际占比为 weight / 总权重。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WeightedTarget {
    pub target: String,
    pub weight: u32,
}

pub type CustomMappingTable = HashMap<String, CustomMappingValue>;

impl CustomMappingValue {
    /// 按规则选择一个目标。空目标或零权重目标不会参与选择。
    pub fn select_target(&self) -> Option<String> {
        self.select_target_with(|_| true)
    }

    /// 按规则选择一个目标，同时过滤当前不可用的目标。权重只在有效
    /// 目标之间重新计算，避免某个冷却目标继续占用原有权重比例。
    pub fn select_target_with<F>(&self, mut is_available: F) -> Option<String>
    where
        F: FnMut(&str) -> bool,
    {
        match self {
            Self::Single(target) => {
                let target = target.trim();
                (!target.is_empty() && is_available(target)).then(|| target.to_string())
            }
            Self::Weighted(targets) => {
                let total_weight: u64 = targets
                    .iter()
                    .filter(|target| {
                        target.weight > 0
                            && !target.target.trim().is_empty()
                            && is_available(target.target.trim())
                    })
                    .map(|target| u64::from(target.weight))
                    .sum();

                if total_weight == 0 {
                    return targets
                        .iter()
                        .find(|target| {
                            !target.target.trim().is_empty() && is_available(target.target.trim())
                        })
                        .map(|target| target.target.trim().to_string());
                }

                use rand::Rng;
                let mut offset = rand::thread_rng().gen_range(0..total_weight);
                for target in targets {
                    if target.weight == 0
                        || target.target.trim().is_empty()
                        || !is_available(target.target.trim())
                    {
                        continue;
                    }
                    let weight = u64::from(target.weight);
                    if offset < weight {
                        return Some(target.target.trim().to_string());
                    }
                    offset -= weight;
                }

                None
            }
        }
    }
}

/// 反代服务配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// 是否启用反代服务
    pub enabled: bool,

    /// 是否允许局域网访问
    /// - false: 仅本机访问 127.0.0.1（默认，隐私优先）
    /// - true: 允许局域网访问 0.0.0.0
    #[serde(default)]
    pub allow_lan_access: bool,

    /// Authorization policy for the proxy.
    /// - off: no auth required
    /// - strict: auth required for all routes
    /// - all_except_health: auth required for all routes except `/healthz`
    /// - auto: recommended defaults (currently: allow_lan_access => all_except_health, else off)
    #[serde(default)]
    pub auth_mode: ProxyAuthMode,

    /// 监听端口
    pub port: u16,

    /// API 密钥
    pub api_key: String,

    /// Web UI 管理后台密码 (可选，如未设置则使用 api_key)
    pub admin_password: Option<String>,

    /// 是否自动启动
    pub auto_start: bool,

    /// 自定义模型映射表。值可以是单个目标模型，也可以是带权重的目标数组。
    #[serde(default)]
    pub custom_mapping: CustomMappingTable,

    /// 按路由目标覆盖推理强度。旧配置可以使用原始模型模板作为键；
    /// 新配置使用 `<模板>::<目标模型>`，从而让 1 对 N 的每个目标拥有独立强度。
    /// 值使用统一的 canonical effort 名称（low/medium/high/xhigh/max/ultra）。
    #[serde(default)]
    pub route_reasoning_effort: std::collections::HashMap<String, String>,

    /// Route used when neither the mapped model nor its provider can be
    /// matched. Empty/disabled values leave the original routing behavior
    /// unchanged.
    #[serde(default)]
    pub fallback_model: FallbackModelConfig,

    /// Temporary model cooling policy after upstream rate limiting.
    #[serde(default)]
    pub model_cooldown: ModelCooldownConfig,

    /// API 请求超时时间(秒)
    #[serde(default = "default_request_timeout")]
    pub request_timeout: u64,

    /// 是否开启请求日志记录 (监控)
    #[serde(default)]
    pub enable_logging: bool,

    /// 调试日志配置 (保存完整链路)
    #[serde(default)]
    pub debug_logging: DebugLoggingConfig,

    /// 上游代理配置
    #[serde(default)]
    pub upstream_proxy: UpstreamProxyConfig,

    /// Multi-provider API gateway configuration.
    /// When non-empty, the legacy `zai` field is ignored.
    #[serde(default)]
    pub providers: Vec<UpstreamProvider>,

    /// z.ai provider configuration (Anthropic-compatible).
    /// Kept for backward compatibility; ignored when `providers` is non-empty.
    #[serde(default)]
    pub zai: ZaiConfig,

    /// 自定义 User-Agent 请求头 (可选覆盖)
    #[serde(default)]
    pub user_agent_override: Option<String>,

    /// 账号调度配置 (粘性会话/限流重试)
    #[serde(default)]
    pub scheduling: crate::proxy::sticky_config::StickySessionConfig,

    /// 实验性功能配置
    #[serde(default)]
    pub experimental: ExperimentalConfig,

    /// 安全监控配置 (IP 黑白名单)
    #[serde(default)]
    pub security_monitor: SecurityMonitorConfig,

    /// 固定账号模式的账号ID (Fixed Account Mode)
    /// - None: 使用轮询模式
    /// - Some(account_id): 固定使用指定账号
    #[serde(default)]
    pub preferred_account_id: Option<String>,

    /// Saved User-Agent string (persisted even when override is disabled)
    #[serde(default)]
    pub saved_user_agent: Option<String>,

    /// Thinking Budget 配置
    /// 控制如何处理 AI 深度思考时的 Token 预算
    #[serde(default)]
    pub thinking_budget: ThinkingBudgetConfig,

    /// 全局系统提示词配置
    /// 自动注入到所有 API 请求的 systemInstruction 中
    #[serde(default)]
    pub global_system_prompt: GlobalSystemPromptConfig,

    /// 图像思维模式配置
    /// - enabled: 保留思维链 (默认)
    /// - disabled: 移除思维链 (画质优先)
    #[serde(default)]
    pub image_thinking_mode: Option<String>,

    /// 代理池配置
    #[serde(default)]
    pub proxy_pool: ProxyPoolConfig,

    /// 协议转换注册表配置 (灰度发布)
    #[serde(default)]
    pub translator: super::translator::config::TranslatorConfig,
}

/// 上游代理配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpstreamProxyConfig {
    /// 是否启用
    pub enabled: bool,
    /// 代理地址 (http://, https://, socks5://)
    pub url: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allow_lan_access: false, // 默认仅本机访问，隐私优先
            auth_mode: ProxyAuthMode::default(),
            port: 8150,
            api_key: format!("sk-{}", uuid::Uuid::new_v4().simple()),
            admin_password: None,
            auto_start: false,
            custom_mapping: CustomMappingTable::new(),
            route_reasoning_effort: std::collections::HashMap::new(),
            fallback_model: FallbackModelConfig::default(),
            model_cooldown: ModelCooldownConfig::default(),
            request_timeout: default_request_timeout(),
            enable_logging: true, // 默认开启，支持 token 统计功能
            debug_logging: DebugLoggingConfig::default(),
            upstream_proxy: UpstreamProxyConfig::default(),
            providers: Vec::new(),
            zai: ZaiConfig::default(),
            scheduling: crate::proxy::sticky_config::StickySessionConfig::default(),
            experimental: ExperimentalConfig::default(),
            security_monitor: SecurityMonitorConfig::default(),
            preferred_account_id: None, // 默认使用轮询模式
            user_agent_override: None,
            saved_user_agent: None,
            thinking_budget: ThinkingBudgetConfig::default(),
            global_system_prompt: GlobalSystemPromptConfig::default(),
            proxy_pool: ProxyPoolConfig::default(),
            image_thinking_mode: None,
            translator: super::translator::config::TranslatorConfig::default(),
        }
    }
}

fn default_request_timeout() -> u64 {
    120 // 默认 120 秒,原来 60 秒太短
}

fn default_zai_base_url() -> String {
    "https://api.z.ai/api/anthropic".to_string()
}

impl ProxyConfig {
    /// 获取实际的监听地址
    /// - allow_lan_access = false: 返回 "127.0.0.1"（默认，隐私优先）
    /// - allow_lan_access = true: 返回 "0.0.0.0"（允许局域网访问）
    pub fn get_bind_address(&self) -> &str {
        if self.allow_lan_access {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        }
    }
}

/// 代理认证信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyAuth {
    pub username: String,
    #[serde(
        serialize_with = "crate::utils::crypto::serialize_password",
        deserialize_with = "crate::utils::crypto::deserialize_password"
    )]
    pub password: String,
}

/// 单个代理配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyEntry {
    pub id: String,                       // 唯一标识
    pub name: String,                     // 显示名称
    pub url: String,                      // 代理地址 (http://, https://, socks5://)
    pub auth: Option<ProxyAuth>,          // 认证信息 (可选)
    pub enabled: bool,                    // 是否启用
    pub priority: i32,                    // 优先级 (数字越小优先级越高)
    pub tags: Vec<String>,                // 标签 (如 "美国", "住宅IP")
    pub max_accounts: Option<usize>,      // 最大绑定账号数 (0 = 无限制)
    pub health_check_url: Option<String>, // 健康检查 URL
    pub last_check_time: Option<i64>,     // 上次检查时间
    pub is_healthy: bool,                 // 健康状态
    pub latency: Option<u64>,             // 延迟 (毫秒) [NEW]
}

/// 代理池配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyPoolConfig {
    pub enabled: bool, // 是否启用代理池
    // pub mode: ProxyPoolMode,        // [REMOVED] 代理池模式，统一为 Hybrid 逻辑
    pub proxies: Vec<ProxyEntry>,         // 代理列表
    pub health_check_interval: u64,       // 健康检查间隔 (秒)
    pub auto_failover: bool,              // 自动故障转移
    pub strategy: ProxySelectionStrategy, // 代理选择策略
    /// 账号到代理的绑定关系 (account_id -> proxy_id)，持久化存储
    #[serde(default)]
    pub account_bindings: HashMap<String, String>,
}

impl Default for ProxyPoolConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            // mode: ProxyPoolMode::Global,
            proxies: Vec::new(),
            health_check_interval: 300,
            auto_failover: true,
            strategy: ProxySelectionStrategy::Priority,
            account_bindings: HashMap::new(),
        }
    }
}

/// 代理选择策略
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProxySelectionStrategy {
    /// 轮询: 依次使用
    RoundRobin,
    /// 随机: 随机选择
    Random,
    /// 优先级: 按 priority 字段排序
    Priority,
    /// 最少连接: 选择当前使用最少的代理
    LeastConnections,
    /// 加权轮询: 根据健康状态和优先级
    WeightedRoundRobin,
}

/// Build an upstream endpoint without duplicating an API version prefix.
/// Providers may be configured as an origin, an origin ending in /v1 or /v4,
/// or as a complete endpoint URL.
pub fn build_provider_api_url(base_url: &str, endpoint: &str) -> String {
    let configured_base = base_url.trim().trim_end_matches('/');
    let endpoint = endpoint.trim_start_matches('/');
    let endpoint_suffix = format!("/{endpoint}");

    if configured_base.ends_with(&endpoint_suffix) {
        return configured_base.to_string();
    }

    // The UI accepts both a provider root (`.../v1`) and a complete API
    // endpoint (`.../v1/responses`).  A complete endpoint is still a useful
    // base for a different operation, such as `GET /models`, so remove only
    // the known operation suffix before appending the requested endpoint.
    let base = strip_known_provider_endpoint(configured_base);

    if contains_api_version_segment(base) {
        format!("{base}/{endpoint}")
    } else {
        format!("{base}/v1/{endpoint}")
    }
}

fn strip_known_provider_endpoint(base_url: &str) -> &str {
    ["/chat/completions", "/responses", "/messages", "/models"]
        .iter()
        .find_map(|suffix| base_url.strip_suffix(suffix))
        .unwrap_or(base_url)
}

fn contains_api_version_segment(base_url: &str) -> bool {
    let authority_and_path = base_url
        .split_once("://")
        .map(|(_, remainder)| remainder)
        .unwrap_or(base_url);
    let path = authority_and_path
        .split_once('/')
        .map(|(_, path)| path)
        .unwrap_or("");

    path.split(['/', '?', '#']).any(is_api_version_segment)
}

fn is_api_version_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    chars.next() == Some('v')
        && chars
            .next()
            .is_some_and(|character| character.is_ascii_digit())
}

/// OpenCode Go exposes these models through the Responses API.
pub fn uses_responses_api(model: &str) -> bool {
    let model = model
        .split_once('(')
        .map(|(base, _)| base)
        .unwrap_or(model)
        .trim();
    matches!(
        model.to_ascii_lowercase().as_str(),
        "gpt-5.6-luna" | "grok-4.5" | "muse-spark-1.2" | "muse-spark-1.2-contributor"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_proxy_url() {
        // 测试已有协议
        assert_eq!(
            normalize_proxy_url("http://127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
        assert_eq!(
            normalize_proxy_url("https://proxy.com"),
            "https://proxy.com"
        );
        assert_eq!(
            normalize_proxy_url("socks5://127.0.0.1:1080"),
            "socks5://127.0.0.1:1080"
        );
        assert_eq!(
            normalize_proxy_url("socks5h://127.0.0.1:1080"),
            "socks5h://127.0.0.1:1080"
        );

        // 测试缺少协议（默认补全 http://）
        assert_eq!(
            normalize_proxy_url("127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
        assert_eq!(
            normalize_proxy_url("localhost:1082"),
            "http://localhost:1082"
        );

        // 测试边缘情况
        assert_eq!(normalize_proxy_url(""), "");
        assert_eq!(normalize_proxy_url("   "), "");
    }

    #[test]
    fn test_build_provider_api_url_does_not_duplicate_v1() {
        assert_eq!(
            build_provider_api_url("https://opencode.ai/zen/go", "chat/completions"),
            "https://opencode.ai/zen/go/v1/chat/completions"
        );
        assert_eq!(
            build_provider_api_url("https://opencode.ai/zen/go/v1", "chat/completions"),
            "https://opencode.ai/zen/go/v1/chat/completions"
        );
        assert_eq!(
            build_provider_api_url(
                "https://opencode.ai/zen/go/v1/chat/completions",
                "chat/completions"
            ),
            "https://opencode.ai/zen/go/v1/chat/completions"
        );
        assert_eq!(
            build_provider_api_url("https://opencode.ai/zen/go/v1/responses", "models"),
            "https://opencode.ai/zen/go/v1/models"
        );
    }

    #[test]
    fn test_build_provider_api_url_respects_any_version_segment() {
        assert_eq!(
            build_provider_api_url("https://open.bigmodel.cn/api/paas/v4", "chat/completions"),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        assert_eq!(
            build_provider_api_url("https://example.com/api/v4/", "models"),
            "https://example.com/api/v4/models"
        );
        assert_eq!(
            build_provider_api_url("https://example.com/api/v1beta", "models"),
            "https://example.com/api/v1beta/models"
        );
        assert_eq!(
            build_provider_api_url("https://example.com/api", "models"),
            "https://example.com/api/v1/models"
        );
    }

    #[test]
    fn test_open_code_go_responses_model_detection() {
        assert!(uses_responses_api("gpt-5.6-luna"));
        assert!(uses_responses_api("gpt-5.6-luna(medium)"));
        assert!(uses_responses_api("grok-4.5"));
        assert!(uses_responses_api("muse-spark-1.2"));
        assert!(uses_responses_api("muse-spark-1.2-contributor"));
        assert!(!uses_responses_api("glm-5.3"));
    }

    #[test]
    fn test_custom_mapping_deserializes_legacy_and_weighted_values() {
        let mapping: CustomMappingTable = serde_json::from_value(serde_json::json!({
            "gpt-*": "big/gpt-5",
            "claude-*": [
                { "target": "ali/sonnet", "weight": 70 },
                { "target": "big/sonnet", "weight": 30 }
            ]
        }))
        .expect("custom mapping should accept string and weighted values");

        assert!(matches!(
            mapping.get("gpt-*") ,
            Some(CustomMappingValue::Single(target)) if target == "big/gpt-5"
        ));
        assert!(matches!(
            mapping.get("claude-*") ,
            Some(CustomMappingValue::Weighted(targets)) if targets.len() == 2
        ));
    }
}
