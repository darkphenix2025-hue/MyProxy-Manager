#![allow(dead_code)]

pub mod config;
/// 协议转换注册表 — 核心调度器。
///
/// ## 架构
///
/// 采用 CLIProxyAPI 的全对全注册表模式，替代现有的 Gemini 枢纽架构：
///
/// ```text
/// 现有:  Client → Gemini v1internal → Provider
/// 新:    Client → [Registry: from→to] → Provider → [Registry: to→from] → Client
/// ```
///
/// ## 设计原则
///
/// 1. **请求转换无状态** — 纯函数 `&[u8] → Vec<u8>`
/// 2. **响应转换有状态** — 流转换通过 `&mut (dyn Any + Send)` 携带跨 chunk 状态
/// 3. **响应方向自动反转** — 注册 `(from=OpenAI, to=Claude)` 自动处理
///    请求 OpenAI→Claude 和响应 Claude→OpenAI
/// 4. **缺失转换器回退** — 无转换器时仅重写 model 字段并透传
/// 5. **线程安全** — `RwLock` 保护注册表，读取无锁竞争
pub mod format;
pub mod metrics;
pub mod pairs;
pub mod sse;
pub mod types;

#[cfg(test)]
mod integration_tests;

#[cfg(test)]
mod benchmarks;

use std::collections::HashMap;
use std::sync::RwLock;

use self::format::Format;
use self::types::{RequestTransform, ResponseTransform, StreamStateFactory};

/// 协议转换注册表。
///
/// 维护所有 `(client_format → provider_format)` 转换对的映射。
pub struct TranslatorRegistry {
    /// 请求转换: `from → to → transform_fn`
    request_transforms: HashMap<Format, HashMap<Format, RequestTransform>>,
    /// 响应转换: `to → from → ResponseTransform`
    /// 注意: 响应方向与注册时相反！
    /// `Register(OpenAI, Claude, ...)` 注册的是:
    ///   请求 OpenAI → Claude
    ///   响应 Claude → OpenAI（即 `responses[Claude][OpenAI]`）
    response_transforms: HashMap<Format, HashMap<Format, ResponseTransform>>,
    /// 流状态工厂: `(to, from) → factory`
    stream_state_factories: HashMap<(Format, Format), StreamStateFactory>,
}

impl TranslatorRegistry {
    pub fn new() -> Self {
        Self {
            request_transforms: HashMap::new(),
            response_transforms: HashMap::new(),
            stream_state_factories: HashMap::new(),
        }
    }

    /// 注册一个转换对。
    ///
    /// # 参数
    /// * `from` — 客户端协议格式
    /// * `to` — 上游供应商协议格式
    /// * `request` — 请求转换函数（from → to）
    /// * `response` — 响应转换集合（to → from）
    /// * `state_factory` — 可选的流状态工厂
    pub fn register(
        &mut self,
        from: Format,
        to: Format,
        request: RequestTransform,
        response: ResponseTransform,
        state_factory: Option<StreamStateFactory>,
    ) {
        // 请求方向: from → to
        self.request_transforms
            .entry(from)
            .or_default()
            .insert(to, request);

        // 响应方向: to → from（自动反转）
        self.response_transforms
            .entry(to)
            .or_default()
            .insert(from, response);

        if let Some(factory) = state_factory {
            self.stream_state_factories.insert((to, from), factory);
        }
    }

    /// 转换请求体。
    ///
    /// 如果没有找到注册的转换器，回退为仅重写 `model` 字段。
    pub fn translate_request(
        &self,
        from: Format,
        to: Format,
        model: &str,
        body: &[u8],
        stream: bool,
    ) -> Vec<u8> {
        if let Some(by_to) = self.request_transforms.get(&from) {
            if let Some(transform) = by_to.get(&to) {
                return transform(model, body, stream);
            }
        }

        // 回退：仅重写 model 字段，防止客户端模型前缀泄漏到上游
        let mut val =
            serde_json::from_slice::<serde_json::Value>(body).unwrap_or(serde_json::Value::Null);
        if val.is_object() {
            val["model"] = serde_json::Value::String(model.to_string());
        }
        serde_json::to_vec(&val).unwrap_or(body.to_vec())
    }

    /// 转换流响应的一个 chunk。
    ///
    /// # 参数
    /// * `from` — 客户端协议格式
    /// * `to` — 上游协议格式
    /// * `model` — 上游模型名称
    /// * `original_request` — 客户端原始请求 JSON
    /// * `converted_request` — 已转换的上游请求 JSON
    /// * `upstream_chunk` — 上游 SSE chunk
    /// * `state` — 流状态累加器
    ///
    /// # 返回
    /// 零或多个客户端 SSE chunk。空向量表示此上游 chunk 不产生输出。
    pub fn translate_stream_chunk(
        &self,
        from: Format,
        to: Format,
        model: &str,
        original_request: &[u8],
        converted_request: &[u8],
        upstream_chunk: &[u8],
        state: &mut (dyn std::any::Any + Send),
    ) -> Vec<Vec<u8>> {
        // 响应方向: upstream 是 to 格式，需转成 from 格式
        if let Some(by_client) = self.response_transforms.get(&to) {
            if let Some(rt) = by_client.get(&from) {
                if let Some(stream_fn) = rt.stream {
                    return stream_fn(
                        model,
                        original_request,
                        converted_request,
                        upstream_chunk,
                        state,
                    );
                }
            }
        }
        // 回退：原样返回
        vec![upstream_chunk.to_vec()]
    }

    /// 转换非流响应。
    pub fn translate_non_stream(
        &self,
        from: Format,
        to: Format,
        model: &str,
        original_request: &[u8],
        converted_request: &[u8],
        upstream_json: &[u8],
        state: &mut (dyn std::any::Any + Send),
    ) -> Vec<u8> {
        if let Some(by_client) = self.response_transforms.get(&to) {
            if let Some(rt) = by_client.get(&from) {
                if let Some(fn_) = rt.non_stream {
                    return fn_(
                        model,
                        original_request,
                        converted_request,
                        upstream_json,
                        state,
                    );
                }
            }
        }
        upstream_json.to_vec()
    }

    /// 转换 token 计数响应。
    pub fn translate_token_count(&self, from: Format, to: Format, count: i64) -> Vec<u8> {
        if let Some(by_client) = self.response_transforms.get(&to) {
            if let Some(rt) = by_client.get(&from) {
                if let Some(fn_) = rt.token_count {
                    return fn_(count);
                }
            }
        }
        serde_json::to_vec(&serde_json::json!({ "input_tokens": count })).unwrap_or_default()
    }

    /// 为新的流会话创建状态对象。
    pub fn new_stream_state(
        &self,
        from: Format,
        to: Format,
    ) -> Option<Box<dyn std::any::Any + Send>> {
        // 响应方向: to → from
        self.stream_state_factories.get(&(to, from)).map(|f| f())
    }

    /// 检查是否存在请求转换器。
    pub fn has_request_transform(&self, from: Format, to: Format) -> bool {
        self.request_transforms
            .get(&from)
            .and_then(|m| m.get(&to))
            .is_some()
    }

    /// 检查是否存在响应转换器。
    pub fn has_response_transform(&self, from: Format, to: Format) -> bool {
        self.response_transforms
            .get(&to)
            .and_then(|m| m.get(&from))
            .map(|rt| rt.has_any())
            .unwrap_or(false)
    }

    /// 检查是否存在流响应转换器。
    pub fn has_stream_response_transform(&self, from: Format, to: Format) -> bool {
        self.response_transforms
            .get(&to)
            .and_then(|m| m.get(&from))
            .and_then(|rt| rt.stream)
            .is_some()
    }
}

// ─── 全局单例 ───

use std::sync::OnceLock;

static REGISTRY: OnceLock<RwLock<TranslatorRegistry>> = OnceLock::new();

/// 获取全局注册表。
///
/// 首次调用时创建空注册表。转换对通过 `register_all()` 注册。
pub fn global_registry() -> &'static RwLock<TranslatorRegistry> {
    REGISTRY.get_or_init(|| RwLock::new(TranslatorRegistry::new()))
}

/// 注册所有转换对。
///
/// 在应用启动时调用一次。
pub fn register_all() {
    let registry = &mut *global_registry().write().unwrap();
    pairs::register_all(registry);
}

// ─── 便捷函数 ───

/// 转换请求体（使用全局注册表）。
pub fn translate_request(
    from: Format,
    to: Format,
    model: &str,
    body: &[u8],
    stream: bool,
) -> Vec<u8> {
    let start = std::time::Instant::now();
    let registry = global_registry().read().unwrap();

    let result = if let Some(by_to) = registry.request_transforms.get(&from) {
        if let Some(transform) = by_to.get(&to) {
            // 有转换器 — 记录 hit
            let output = transform(model, body, stream);
            drop(registry);
            metrics::global_metrics().record_hit(from, to, start);
            output
        } else {
            drop(registry);
            metrics::global_metrics().record_fallback(from, to);
            // 回退：仅重写 model 字段
            let mut val = serde_json::from_slice::<serde_json::Value>(body)
                .unwrap_or(serde_json::Value::Null);
            if val.is_object() {
                val["model"] = serde_json::Value::String(model.to_string());
            }
            serde_json::to_vec(&val).unwrap_or(body.to_vec())
        }
    } else {
        drop(registry);
        metrics::global_metrics().record_fallback(from, to);
        // 回退：仅重写 model 字段
        let mut val =
            serde_json::from_slice::<serde_json::Value>(body).unwrap_or(serde_json::Value::Null);
        if val.is_object() {
            val["model"] = serde_json::Value::String(model.to_string());
        }
        serde_json::to_vec(&val).unwrap_or(body.to_vec())
    };

    result
}

/// 转换非流响应（使用全局注册表）。
pub fn translate_non_stream(
    from: Format,
    to: Format,
    model: &str,
    original_request: &[u8],
    converted_request: &[u8],
    upstream_json: &[u8],
    state: &mut (dyn std::any::Any + Send),
) -> Vec<u8> {
    let registry = global_registry().read().unwrap();
    registry.translate_non_stream(
        from,
        to,
        model,
        original_request,
        converted_request,
        upstream_json,
        state,
    )
}

/// 检查是否存在转换器。
pub fn has_transform(from: Format, to: Format) -> bool {
    let registry = global_registry().read().unwrap();
    registry.has_request_transform(from, to)
}
