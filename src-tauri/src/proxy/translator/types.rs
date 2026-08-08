/// 转换函数类型定义和响应转换结构体。
///
/// 设计理念：
/// - 请求转换是无状态的纯函数：`&[u8] → Vec<u8>`
/// - 响应流转换是有状态的：每个 SSE chunk 独立处理，通过 `&mut (dyn Any + Send)` 携带状态
/// - 非流响应是一次性转换：完整 JSON → 完整 JSON
use std::any::Any;

// ─── 请求转换 ───

/// 请求转换函数。
///
/// # 参数
/// * `model` — 解析后的上游模型名称（已去除 provider_id 前缀）
/// * `raw_json` — 客户端原始请求体 JSON
/// * `stream` — 客户端是否请求流式响应
///
/// # 返回
/// 转换后的上游请求体 JSON
pub type RequestTransform = fn(model: &str, raw_json: &[u8], stream: bool) -> Vec<u8>;

// ─── 响应转换 ───

/// 流式响应转换函数。
///
/// 每次上游返回一个 SSE chunk 时调用此函数，产出零或多个客户端 SSE chunk。
///
/// # 参数
/// * `model` — 上游模型名称
/// * `original_request` — 客户端原始请求 JSON（用于上下文参考）
/// * `converted_request` — 已转换的上游请求 JSON（用于上下文参考）
/// * `upstream_chunk` — 上游 SSE chunk 原始字节
/// * `state` — 跨 chunk 持久化的状态累加器（必须 Send 以支持异步流处理）
///
/// # 返回
/// 零或多个客户端 SSE chunk 字节序列。空向量表示此上游 chunk 不产生输出。
pub type ResponseStreamTransform = fn(
    model: &str,
    original_request: &[u8],
    converted_request: &[u8],
    upstream_chunk: &[u8],
    state: &mut (dyn Any + Send),
) -> Vec<Vec<u8>>;

/// 非流式响应转换函数。
///
/// # 参数
/// * `model` — 上游模型名称
/// * `original_request` — 客户端原始请求 JSON
/// * `converted_request` — 已转换的上游请求 JSON
/// * `upstream_json` — 上游完整响应 JSON
/// * `state` — 状态累加器（非流模式下通常为空）
///
/// # 返回
/// 客户端响应体 JSON
pub type ResponseNonStreamTransform = fn(
    model: &str,
    original_request: &[u8],
    converted_request: &[u8],
    upstream_json: &[u8],
    state: &mut (dyn Any + Send),
) -> Vec<u8>;

/// Token 计数响应转换函数。
///
/// 将上游 token 计数转换为客户端格式的 JSON 字节。
pub type ResponseTokenCountTransform = fn(count: i64) -> Vec<u8>;

/// 流状态工厂 — 为每个新流会话创建独立的状态对象。
/// 返回的类型必须实现 `Send` 以支持在 tokio::spawn 中使用。
pub type StreamStateFactory = fn() -> Box<dyn Any + Send>;

// ─── 响应转换集合 ───

/// 将三种响应转换函数组合在一起。
///
/// 一个转换对注册时，请求方向是 `from → to`，
/// 响应方向自动反转为 `to → from`。
pub struct ResponseTransform {
    pub stream: Option<ResponseStreamTransform>,
    pub non_stream: Option<ResponseNonStreamTransform>,
    pub token_count: Option<ResponseTokenCountTransform>,
}

impl ResponseTransform {
    pub fn new(
        stream: Option<ResponseStreamTransform>,
        non_stream: Option<ResponseNonStreamTransform>,
        token_count: Option<ResponseTokenCountTransform>,
    ) -> Self {
        Self {
            stream,
            non_stream,
            token_count,
        }
    }

    /// 仅流式响应的转换
    pub fn stream_only(stream: ResponseStreamTransform) -> Self {
        Self {
            stream: Some(stream),
            non_stream: None,
            token_count: None,
        }
    }

    /// 仅非流式响应的转换
    pub fn non_stream_only(non_stream: ResponseNonStreamTransform) -> Self {
        Self {
            stream: None,
            non_stream: Some(non_stream),
            token_count: None,
        }
    }

    pub fn has_any(&self) -> bool {
        self.stream.is_some() || self.non_stream.is_some() || self.token_count.is_some()
    }
}
