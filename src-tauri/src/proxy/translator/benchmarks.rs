//! Translator performance benchmarks — 新转换单元性能基准。
//!
//! 使用标准 #[test] + 手动计时（不依赖 criterion / nightly），
//! 在 CI 的 stable Rust 上可运行。
//!
//! 测试场景：
//! 1. 请求转换延迟（各翻译对）
//! 2. 非流响应转换延迟
//! 3. SSE 流响应逐 chunk 转换延迟

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    // ─── 测试 fixtures ───

    /// 典型的 OpenAI chat completions 请求
    fn openai_request_payload() -> Vec<u8> {
        serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Explain quantum computing in simple terms."}
            ],
            "max_tokens": 1024,
            "stream": true,
            "temperature": 0.7
        })
        .to_string()
        .into_bytes()
    }

    /// 典型的 Claude messages 请求
    fn claude_request_payload() -> Vec<u8> {
        serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "system": "You are a helpful assistant.",
            "messages": [
                {"role": "user", "content": "Explain quantum computing in simple terms."}
            ],
            "max_tokens": 1024,
            "stream": true,
            "temperature": 0.7
        })
        .to_string()
        .into_bytes()
    }

    /// 典型的上游 SSE chunk（Claude 格式）
    fn claude_sse_chunk() -> Vec<u8> {
        b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n".to_vec()
    }

    // ─── Benchmark harness ───

    /// 运行指定次数的操作并返回平均延迟。
    fn benchmark<F: FnMut()>(mut f: F, iterations: usize) -> Duration {
        // 预热
        for _ in 0..10 {
            f();
        }
        // 正式测量
        let start = Instant::now();
        for _ in 0..iterations {
            f();
        }
        start.elapsed() / iterations as u32
    }

    const ITERATIONS: usize = 1000;

    // ─── Benchmark 1: 请求转换 — OpenAI → Claude ───

    #[test]
    fn bench_request_openai_to_claude() {
        use crate::proxy::translator::{format::Format, register_all, translate_request};
        register_all();

        let payload = openai_request_payload();
        let avg = benchmark(
            || {
                let _ = translate_request(
                    Format::OpenAI,
                    Format::Claude,
                    "claude-sonnet-4-20250514",
                    &payload,
                    true,
                );
            },
            ITERATIONS,
        );
        eprintln!(
            "[BENCH] Request OpenAI→Claude: avg {:?} per iteration ({} iterations)",
            avg, ITERATIONS
        );
        // 应 < 10ms（宽松上限，CI 环境可能较慢）
        assert!(avg < Duration::from_millis(10));
    }

    // ─── Benchmark 2: 请求转换 — Claude → OpenAI ───

    #[test]
    fn bench_request_claude_to_openai() {
        use crate::proxy::translator::{format::Format, register_all, translate_request};
        register_all();

        let payload = claude_request_payload();
        let avg = benchmark(
            || {
                let _ = translate_request(
                    Format::Claude,
                    Format::OpenAI,
                    "claude-sonnet-4-20250514",
                    &payload,
                    true,
                );
            },
            ITERATIONS,
        );
        eprintln!(
            "[BENCH] Request Claude→OpenAI: avg {:?} per iteration ({} iterations)",
            avg, ITERATIONS
        );
        assert!(avg < Duration::from_millis(10));
    }

    // ─── Benchmark 3: 非流响应转换 — Claude → OpenAI ───

    #[test]
    fn bench_non_stream_claude_to_openai() {
        use crate::proxy::translator::{format::Format, register_all, translate_non_stream};
        register_all();

        let claude_resp = serde_json::json!({
            "id": "msg_abc123",
            "type": "message",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "Hello, world!"}
            ],
            "model": "claude-sonnet-4-20250514",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        })
        .to_string()
        .into_bytes();

        let req = openai_request_payload();
        let converted_req = req.clone();
        let mut state = Box::new(());

        let avg = benchmark(
            || {
                let _ = translate_non_stream(
                    Format::OpenAI,
                    Format::Claude,
                    "claude-sonnet-4-20250514",
                    &req,
                    &converted_req,
                    &claude_resp,
                    state.as_mut(),
                );
            },
            ITERATIONS,
        );
        eprintln!(
            "[BENCH] Non-stream Claude→OpenAI: avg {:?} per iteration ({} iterations)",
            avg, ITERATIONS
        );
        assert!(avg < Duration::from_millis(10));
    }

    // ─── Benchmark 4: SSE 流响应逐 chunk 转换 ───

    #[test]
    fn bench_stream_chunk_claude_to_openai() {
        use crate::proxy::translator::{format::Format, global_registry, register_all};
        register_all();

        let chunk = claude_sse_chunk();
        let req = openai_request_payload();
        let converted_req = req.clone();
        let registry = global_registry();

        let mut state = registry
            .read()
            .unwrap()
            .new_stream_state(Format::OpenAI, Format::Claude)
            .unwrap_or_else(|| Box::new(()));

        let avg = benchmark(
            || {
                let _ = registry.read().unwrap().translate_stream_chunk(
                    Format::OpenAI,
                    Format::Claude,
                    "claude-sonnet-4-20250514",
                    &req,
                    &converted_req,
                    &chunk,
                    state.as_mut(),
                );
            },
            ITERATIONS,
        );
        eprintln!(
            "[BENCH] Stream chunk Claude→OpenAI: avg {:?} per iteration ({} iterations)",
            avg, ITERATIONS
        );
        assert!(avg < Duration::from_millis(5));
    }

    // ─── Benchmark 5: Responses API → Claude ───

    #[test]
    fn bench_request_responses_to_claude() {
        use crate::proxy::translator::{format::Format, register_all, translate_request};
        register_all();

        let payload = serde_json::json!({
            "model": "claude-sonnet-4-20250514",
            "input": [
                {"role": "user", "content": "What is Rust?"}
            ],
            "instructions": "You are a helpful assistant.",
            "max_output_tokens": 1024,
            "stream": true
        })
        .to_string()
        .into_bytes();

        let avg = benchmark(
            || {
                let _ = translate_request(
                    Format::OpenAIResponses,
                    Format::Claude,
                    "claude-sonnet-4-20250514",
                    &payload,
                    true,
                );
            },
            ITERATIONS,
        );
        eprintln!(
            "[BENCH] Request Responses→Claude: avg {:?} per iteration ({} iterations)",
            avg, ITERATIONS
        );
        assert!(avg < Duration::from_millis(10));
    }

    // ─── Benchmark 6: Claude → Responses API ───

    #[test]
    fn bench_request_claude_to_responses() {
        use crate::proxy::translator::{format::Format, register_all, translate_request};
        register_all();

        let payload = claude_request_payload();
        let avg = benchmark(
            || {
                let _ = translate_request(
                    Format::Claude,
                    Format::OpenAIResponses,
                    "gpt-4o",
                    &payload,
                    true,
                );
            },
            ITERATIONS,
        );
        eprintln!(
            "[BENCH] Request Claude→Responses: avg {:?} per iteration ({} iterations)",
            avg, ITERATIONS
        );
        assert!(avg < Duration::from_millis(10));
    }

    // ─── Benchmark 7: OpenAI → Codex ───

    #[test]
    fn bench_request_openai_to_codex() {
        use crate::proxy::translator::{format::Format, register_all, translate_request};
        register_all();

        let payload = openai_request_payload();
        let avg = benchmark(
            || {
                let _ = translate_request(Format::OpenAI, Format::Codex, "o3", &payload, false);
            },
            ITERATIONS,
        );
        eprintln!(
            "[BENCH] Request OpenAI→Codex: avg {:?} per iteration ({} iterations)",
            avg, ITERATIONS
        );
        assert!(avg < Duration::from_millis(10));
    }

    // ─── Benchmark 8: Codex → OpenAI ───

    #[test]
    fn bench_request_codex_to_openai() {
        use crate::proxy::translator::{format::Format, register_all, translate_request};
        register_all();

        let payload = serde_json::json!({
            "model": "o3",
            "input": [
                {"role": "user", "content": "Write a Rust function."}
            ],
            "instructions": "Be concise."
        })
        .to_string()
        .into_bytes();

        let avg = benchmark(
            || {
                let _ = translate_request(Format::Codex, Format::OpenAI, "gpt-4o", &payload, false);
            },
            ITERATIONS,
        );
        eprintln!(
            "[BENCH] Request Codex→OpenAI: avg {:?} per iteration ({} iterations)",
            avg, ITERATIONS
        );
        assert!(avg < Duration::from_millis(10));
    }

    // ─── Benchmark 9: 回退路径（无转换器） ───

    #[test]
    fn bench_fallback_path() {
        use crate::proxy::translator::{format::Format, register_all, translate_request};
        register_all();

        let payload = openai_request_payload();
        let avg = benchmark(
            || {
                // OpenAI → OpenAI 没有注册的转换器，走回退路径
                let _ = translate_request(Format::OpenAI, Format::OpenAI, "gpt-4o", &payload, true);
            },
            ITERATIONS,
        );
        eprintln!(
            "[BENCH] Fallback (OpenAI→OpenAI, no transform): avg {:?} per iteration ({} iterations)",
            avg, ITERATIONS
        );
        assert!(avg < Duration::from_millis(5));
    }
}
