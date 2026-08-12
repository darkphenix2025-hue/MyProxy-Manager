//! Translator 集成测试 — 验证各翻译对的往返转换正确性和边缘情况。

#[cfg(test)]
mod tests {
    use crate::proxy::translator;
    use crate::proxy::translator::format::Format;
    use serde_json::{json, Value};

    /// 确保注册表已初始化（测试环境中需要显式调用）
    fn ensure_registered() {
        translator::register_all();
    }

    // ─── 往返测试：OpenAI → Claude ───

    #[test]
    fn test_roundtrip_openai_claude_basic() {
        ensure_registered();
        let original = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "Hello!"}
            ],
            "stream": true,
            "temperature": 0.7
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let claude_bytes = translator::translate_request(
            Format::OpenAI,
            Format::Claude,
            "gpt-4",
            &original_bytes,
            true,
        );
        let claude: Value = serde_json::from_slice(&claude_bytes).unwrap();

        assert!(claude.get("model").is_some());
        assert!(claude.get("messages").is_some());
        assert_eq!(claude["system"], "You are a helpful assistant.");

        // 注意：system 消息被转换为 user 角色并保留在 messages 中
        let messages = claude["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "user"); // system → user
        assert_eq!(messages[0]["content"], "You are a helpful assistant.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Hello!");
    }

    #[test]
    fn test_roundtrip_openai_claude_multi_turn() {
        ensure_registered();
        let original = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": "What is Rust?"},
                {"role": "assistant", "content": "Rust is a systems programming language."},
                {"role": "user", "content": "Is it safe?"}
            ],
            "stream": false
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let claude_bytes = translator::translate_request(
            Format::OpenAI,
            Format::Claude,
            "gpt-4",
            &original_bytes,
            false,
        );
        let claude: Value = serde_json::from_slice(&claude_bytes).unwrap();

        let messages = claude["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[2]["role"], "user");
    }

    // ─── 往返测试：OpenAI → Codex ───

    #[test]
    fn test_roundtrip_openai_codex_basic() {
        ensure_registered();
        let original = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "system", "content": "Write clean code."},
                {"role": "user", "content": "Create a function that sorts an array."}
            ],
            "stream": true
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let codex_bytes = translator::translate_request(
            Format::OpenAI,
            Format::Codex,
            "gpt-4",
            &original_bytes,
            true,
        );
        let codex: Value = serde_json::from_slice(&codex_bytes).unwrap();

        assert_eq!(codex["model"], "gpt-4");
        assert_eq!(codex["stream"], true);
        assert!(codex.get("instructions").is_some());
        assert!(codex.get("input").is_some());
        assert_eq!(codex["instructions"], "Write clean code.");
        assert!(codex["input"]
            .as_str()
            .unwrap()
            .contains("Create a function"));
    }

    #[test]
    fn test_roundtrip_openai_codex_no_system() {
        ensure_registered();
        let original = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": "Hello"}
            ],
            "stream": false
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let codex_bytes = translator::translate_request(
            Format::OpenAI,
            Format::Codex,
            "gpt-4",
            &original_bytes,
            false,
        );
        let codex: Value = serde_json::from_slice(&codex_bytes).unwrap();

        assert!(!codex.as_object().unwrap().contains_key("instructions"));
        assert_eq!(codex["input"], "user: Hello");
    }

    // ─── 往返测试：Claude → Codex ───

    #[test]
    fn test_roundtrip_claude_codex_basic() {
        ensure_registered();
        let original = json!({
            "model": "claude-sonnet-4",
            "system": "Be concise.",
            "messages": [
                {"role": "user", "content": "Explain O(n)"}
            ],
            "stream": true,
            "max_tokens": 1024
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let codex_bytes = translator::translate_request(
            Format::Claude,
            Format::Codex,
            "claude-sonnet-4",
            &original_bytes,
            true,
        );
        let codex: Value = serde_json::from_slice(&codex_bytes).unwrap();

        assert_eq!(codex["model"], "claude-sonnet-4");
        assert_eq!(codex["instructions"], "Be concise.");
        assert_eq!(codex["input"], "user: Explain O(n)");
        assert_eq!(codex["stream"], true);
        assert_eq!(codex["max_tokens"], 1024);
    }

    #[test]
    fn test_roundtrip_claude_codex_array_content() {
        ensure_registered();
        let original = json!({
            "model": "claude-sonnet-4",
            "system": "Helpful assistant",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "First part"},
                        {"type": "text", "text": "Second part"}
                    ]
                }
            ],
            "stream": false
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let codex_bytes = translator::translate_request(
            Format::Claude,
            Format::Codex,
            "claude-sonnet-4",
            &original_bytes,
            false,
        );
        let codex: Value = serde_json::from_slice(&codex_bytes).unwrap();

        assert_eq!(codex["instructions"], "Helpful assistant");
        assert_eq!(codex["input"], "user: First part\nSecond part");
    }

    // ─── 往返测试：Codex → Claude ───

    #[test]
    fn test_roundtrip_codex_claude_basic() {
        ensure_registered();
        let original = json!({
            "model": "codex-model",
            "instructions": "You are a code expert.",
            "input": "user: Write a binary search in Python",
            "stream": true
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let claude_bytes = translator::translate_request(
            Format::Codex,
            Format::Claude,
            "codex-model",
            &original_bytes,
            true,
        );
        let claude: Value = serde_json::from_slice(&claude_bytes).unwrap();

        assert_eq!(claude["model"], "codex-model");
        assert_eq!(claude["system"], "You are a code expert.");
        let messages = claude["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert!(messages[0]["content"]
            .as_str()
            .unwrap()
            .contains("binary search"));
        assert_eq!(claude["stream"], true);
    }

    #[test]
    fn test_roundtrip_codex_claude_no_instructions() {
        ensure_registered();
        let original = json!({
            "model": "codex-model",
            "input": "user: Hello",
            "stream": false
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let claude_bytes = translator::translate_request(
            Format::Codex,
            Format::Claude,
            "codex-model",
            &original_bytes,
            false,
        );
        let claude: Value = serde_json::from_slice(&claude_bytes).unwrap();

        assert!(claude.get("system").is_none() || claude["system"].is_null());
    }

    #[test]
    fn test_roundtrip_codex_claude_multi_turn() {
        ensure_registered();
        let original = json!({
            "model": "codex-model",
            "instructions": "Help with coding.",
            "input": "user: What is Rust?\nassistant: A systems language.\nuser: Show me an example",
            "stream": true
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let claude_bytes = translator::translate_request(
            Format::Codex,
            Format::Claude,
            "codex-model",
            &original_bytes,
            true,
        );
        let claude: Value = serde_json::from_slice(&claude_bytes).unwrap();

        let messages = claude["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[2]["role"], "user");
        assert_eq!(claude["system"], "Help with coding.");
    }

    // ─── 边缘情况测试 ───

    #[test]
    fn test_empty_messages() {
        ensure_registered();
        let original = json!({
            "model": "gpt-4",
            "messages": [],
            "stream": false
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let claude_bytes = translator::translate_request(
            Format::OpenAI,
            Format::Claude,
            "gpt-4",
            &original_bytes,
            false,
        );
        let claude: Value = serde_json::from_slice(&claude_bytes).unwrap();

        let messages = claude["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 0);
    }

    #[test]
    fn test_special_characters_in_content() {
        ensure_registered();
        let original = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": "What is 2 > 1 && 1 < 2?"}
            ],
            "stream": false
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let claude_bytes = translator::translate_request(
            Format::OpenAI,
            Format::Claude,
            "gpt-4",
            &original_bytes,
            false,
        );
        let claude: Value = serde_json::from_slice(&claude_bytes).unwrap();

        let messages = claude["messages"].as_array().unwrap();
        assert_eq!(messages[0]["content"], "What is 2 > 1 && 1 < 2?");
    }

    #[test]
    fn test_unicode_content() {
        ensure_registered();
        let original = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": "你好世界 🌍 Привет мир"}
            ],
            "stream": false
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let claude_bytes = translator::translate_request(
            Format::OpenAI,
            Format::Claude,
            "gpt-4",
            &original_bytes,
            false,
        );
        let claude: Value = serde_json::from_slice(&claude_bytes).unwrap();

        let messages = claude["messages"].as_array().unwrap();
        assert_eq!(messages[0]["content"], "你好世界 🌍 Привет мир");
    }

    #[test]
    fn test_long_content() {
        ensure_registered();
        let long_text = "a".repeat(10000);
        let original = json!({
            "model": "gpt-4",
            "messages": [
                {"role": "user", "content": long_text}
            ],
            "stream": false
        });
        let original_bytes = serde_json::to_vec(&original).unwrap();

        let claude_bytes = translator::translate_request(
            Format::OpenAI,
            Format::Claude,
            "gpt-4",
            &original_bytes,
            false,
        );
        let claude: Value = serde_json::from_slice(&claude_bytes).unwrap();

        let messages = claude["messages"].as_array().unwrap();
        assert_eq!(messages[0]["content"].as_str().unwrap().len(), 10000);
    }

    // ─── 配置测试 ───

    #[test]
    fn test_translator_config_default() {
        use crate::proxy::translator::config::TranslatorConfig;

        let config = TranslatorConfig::default();
        assert!(!config.enabled);
        assert!(!config.is_format_enabled("openai"));
        assert!(!config.is_format_enabled("claude"));
    }

    #[test]
    fn test_translator_config_global_enable() {
        use crate::proxy::translator::config::TranslatorConfig;

        let mut config = TranslatorConfig::default();
        config.enabled = true;
        assert!(config.is_format_enabled("openai"));
        assert!(config.is_format_enabled("claude"));
        assert!(config.is_format_enabled("codex"));
    }

    #[test]
    fn test_translator_config_per_format() {
        use crate::proxy::translator::config::TranslatorConfig;
        use std::collections::HashMap;

        let mut config = TranslatorConfig::default();
        config.enabled = true;
        let mut toggle = HashMap::new();
        toggle.insert("openai".to_string(), true);
        toggle.insert("claude".to_string(), false);
        config.format_toggle = toggle;

        assert!(config.is_format_enabled("openai"));
        assert!(!config.is_format_enabled("claude"));
        assert!(config.is_format_enabled("codex"));
    }

    #[test]
    fn test_translator_config_disabled_overrides() {
        use crate::proxy::translator::config::TranslatorConfig;
        use std::collections::HashMap;

        let mut config = TranslatorConfig::default();
        config.enabled = false;
        let mut toggle = HashMap::new();
        toggle.insert("openai".to_string(), true);
        config.format_toggle = toggle;

        assert!(!config.is_format_enabled("openai"));
        assert!(!config.is_format_enabled("claude"));
    }
}
