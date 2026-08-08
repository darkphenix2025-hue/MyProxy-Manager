/// Translator metrics — 轻量级调用统计。
///
/// 追踪每个 `(from → to)` 转换对的：
/// - 请求计数（hit count）
/// - 平均延迟（P50/P99）
/// - 错误计数（error count）
/// - 回退计数（fallback count — 无转换器时的 model-only 回退）
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::Instant;

use crate::proxy::translator::format::Format;

/// 单个转换对的统计。
#[derive(Default)]
pub struct PairMetrics {
    /// 成功转换次数
    pub hit_count: AtomicU64,
    /// 回退次数（无转换器，仅重写 model）
    pub fallback_count: AtomicU64,
    /// 错误次数
    pub error_count: AtomicU64,
    /// 延迟样本（毫秒，保留最近 1024 个样本用于 P50/P99 计算）
    latency_samples: RwLock<Vec<u64>>,
}

impl PairMetrics {
    pub fn record_hit(&self, elapsed_ms: u64) {
        self.hit_count.fetch_add(1, Ordering::Relaxed);
        let mut samples = self.latency_samples.write().unwrap();
        samples.push(elapsed_ms);
        // 保留最近 1024 个样本
        if samples.len() > 1024 {
            let drain = samples.len() - 1024;
            samples.drain(..drain);
        }
    }

    pub fn record_fallback(&self) {
        self.fallback_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_error(&self) {
        self.error_count.fetch_add(1, Ordering::Relaxed);
    }

    /// 计算 P50 延迟（毫秒）。
    pub fn p50_ms(&self) -> Option<u64> {
        let samples = self.latency_samples.read().unwrap();
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.clone();
        sorted.sort_unstable();
        let idx = sorted.len() / 2;
        Some(sorted[idx])
    }

    /// 计算 P99 延迟（毫秒）。
    pub fn p99_ms(&self) -> Option<u64> {
        let samples = self.latency_samples.read().unwrap();
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.clone();
        sorted.sort_unstable();
        let idx = (sorted.len() as f64 * 0.99) as usize;
        Some(sorted[idx.min(sorted.len() - 1)])
    }

    /// 平均延迟（毫秒）。
    pub fn avg_ms(&self) -> Option<u64> {
        let samples = self.latency_samples.read().unwrap();
        if samples.is_empty() {
            return None;
        }
        let sum: u64 = samples.iter().sum();
        Some(sum / samples.len() as u64)
    }
}

/// 全局 metrics 存储。
pub struct TranslatorMetrics {
    pairs: RwLock<HashMap<String, std::sync::Arc<PairMetrics>>>,
}

impl TranslatorMetrics {
    pub fn new() -> Self {
        Self {
            pairs: RwLock::new(HashMap::new()),
        }
    }

    /// 获取或创建指定转换对的 metrics。
    fn get_or_create_pair(&self, from: Format, to: Format) -> std::sync::Arc<PairMetrics> {
        let key = format!("{:?}->{:?}", from, to);
        // 先尝试读取
        {
            let pairs = self.pairs.read().unwrap();
            if let Some(pair) = pairs.get(&key) {
                return std::sync::Arc::clone(pair);
            }
        }
        // 需要创建 — 使用写锁
        let mut pairs = self.pairs.write().unwrap();
        // 双重检查
        if let Some(pair) = pairs.get(&key) {
            return std::sync::Arc::clone(pair);
        }
        let pair = std::sync::Arc::new(PairMetrics::default());
        pairs.insert(key, std::sync::Arc::clone(&pair));
        pair
    }

    /// 记录一次成功的转换。
    pub fn record_hit(&self, from: Format, to: Format, start: Instant) {
        let elapsed = start.elapsed();
        let elapsed_us = elapsed.as_micros() as u64;
        let pair = self.get_or_create_pair(from, to);
        pair.record_hit(elapsed_us);
    }

    /// 记录一次回退（无转换器）。
    pub fn record_fallback(&self, from: Format, to: Format) {
        let pair = self.get_or_create_pair(from, to);
        pair.record_fallback();
    }

    /// 记录一次错误。
    pub fn record_error(&self, from: Format, to: Format) {
        let pair = self.get_or_create_pair(from, to);
        pair.record_error();
    }

    /// 获取所有转换对的汇总报告。
    pub fn report(&self) -> Vec<MetricsReport> {
        let pairs = self.pairs.read().unwrap();
        pairs
            .iter()
            .map(|(key, pair)| {
                let hit_count = pair.hit_count.load(Ordering::Relaxed);
                let fallback_count = pair.fallback_count.load(Ordering::Relaxed);
                let error_count = pair.error_count.load(Ordering::Relaxed);
                let total = hit_count + fallback_count + error_count;
                let hit_rate = if total > 0 {
                    hit_count as f64 / total as f64
                } else {
                    0.0
                };

                MetricsReport {
                    pair: key.clone(),
                    hit_count,
                    fallback_count,
                    error_count,
                    total,
                    hit_rate,
                    p50_ms: pair.p50_ms(),
                    p99_ms: pair.p99_ms(),
                    avg_ms: pair.avg_ms(),
                }
            })
            .collect()
    }

    /// 重置所有统计。
    pub fn reset(&self) {
        let mut pairs = self.pairs.write().unwrap();
        pairs.clear();
    }
}

/// 汇总报告条目。
#[derive(Debug)]
pub struct MetricsReport {
    pub pair: String,
    pub hit_count: u64,
    pub fallback_count: u64,
    pub error_count: u64,
    pub total: u64,
    pub hit_rate: f64,
    pub p50_ms: Option<u64>,
    pub p99_ms: Option<u64>,
    pub avg_ms: Option<u64>,
}

impl std::fmt::Display for MetricsReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}: hits={} fallbacks={} errors={} total={} hit_rate={:.1}% p50={:?} p99={:?} avg={:?}",
            self.pair,
            self.hit_count,
            self.fallback_count,
            self.error_count,
            self.total,
            self.hit_rate * 100.0,
            self.p50_ms,
            self.p99_ms,
            self.avg_ms
        )
    }
}

// ─── 全局单例 ───

use std::sync::OnceLock;

static METRICS: OnceLock<TranslatorMetrics> = OnceLock::new();

pub fn global_metrics() -> &'static TranslatorMetrics {
    METRICS.get_or_init(TranslatorMetrics::new)
}

/// 便捷宏：记录转换开始时间。
#[macro_export]
macro_rules! translator_metrics_start {
    () => {
        std::time::Instant::now()
    };
}

/// 便捷宏：记录成功转换。
#[macro_export]
macro_rules! translator_metrics_record_hit {
    ($from:expr, $to:expr, $start:expr) => {
        $crate::proxy::translator::metrics::global_metrics().record_hit($from, $to, $start);
    };
}

/// 便捷宏：记录回退。
#[macro_export]
macro_rules! translator_metrics_record_fallback {
    ($from:expr, $to:expr) => {
        $crate::proxy::translator::metrics::global_metrics().record_fallback($from, $to);
    };
}

/// 便捷宏：记录错误。
#[macro_export]
macro_rules! translator_metrics_record_error {
    ($from:expr, $to:expr) => {
        $crate::proxy::translator::metrics::global_metrics().record_error($from, $to);
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_pair_metrics_basic() {
        let metrics = TranslatorMetrics::new();

        metrics.record_hit(Format::OpenAI, Format::Claude, Instant::now());
        metrics.record_hit(Format::OpenAI, Format::Claude, Instant::now());
        metrics.record_fallback(Format::OpenAI, Format::Claude);
        metrics.record_error(Format::OpenAI, Format::Claude);

        let report = metrics.report();
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].hit_count, 2);
        assert_eq!(report[0].fallback_count, 1);
        assert_eq!(report[0].error_count, 1);
        assert_eq!(report[0].total, 4);
        assert!((report[0].hit_rate - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_p50_p99_calculation() {
        let metrics = TranslatorMetrics::new();

        // 记录 100 次，延迟递增
        for i in 0..100 {
            let start = Instant::now();
            // 模拟不同延迟
            thread::sleep(Duration::from_micros(i));
            metrics.record_hit(Format::OpenAI, Format::Claude, start);
        }

        let report = metrics.report();
        assert_eq!(report.len(), 1);
        assert!(report[0].p50_ms.is_some());
        assert!(report[0].p99_ms.is_some());
        assert!(report[0].p50_ms.unwrap() <= report[0].p99_ms.unwrap());
    }

    #[test]
    fn test_empty_metrics() {
        let metrics = TranslatorMetrics::new();
        let report = metrics.report();
        assert!(report.is_empty());
    }

    #[test]
    fn test_reset() {
        let metrics = TranslatorMetrics::new();
        metrics.record_hit(Format::OpenAI, Format::Claude, Instant::now());
        assert_eq!(metrics.report().len(), 1);

        metrics.reset();
        assert!(metrics.report().is_empty());
    }

    #[test]
    fn test_multiple_pairs() {
        let metrics = TranslatorMetrics::new();
        metrics.record_hit(Format::OpenAI, Format::Claude, Instant::now());
        metrics.record_hit(Format::Claude, Format::OpenAI, Instant::now());
        metrics.record_hit(Format::OpenAI, Format::Codex, Instant::now());

        let report = metrics.report();
        assert_eq!(report.len(), 3);
    }

    #[test]
    fn test_global_metrics_singleton() {
        let m1 = global_metrics();
        let m2 = global_metrics();
        assert!(std::ptr::eq(m1, m2));
    }
}
