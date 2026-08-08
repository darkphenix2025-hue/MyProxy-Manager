/// 协议格式枚举 — 标识客户端请求格式和上游响应格式。
///
/// 每个变体对应一种 AI API 协议。新的协议格式只需在此枚举中添加变体，
/// 然后在 `pairs/` 下添加对应的转换对即可扩展。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// OpenAI Chat Completions API (`/v1/chat/completions`)
    OpenAI,
    /// OpenAI Responses API (`/v1/responses`)
    OpenAIResponses,
    /// Anthropic Claude Messages API (`/v1/messages`)
    Claude,
    /// OpenAI Codex wire protocol (`/backend-api/codex/*`)
    Codex,
}

impl Format {
    /// 从字符串解析 Format，未知格式返回 None
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "openai" => Some(Self::OpenAI),
            "openai-response" | "openai_response" | "openai-responses" => {
                Some(Self::OpenAIResponses)
            }
            "claude" | "anthropic" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    /// 格式标识字符串，用于日志和配置
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::OpenAI => "openai",
            Self::OpenAIResponses => "openai-response",
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_from_str() {
        assert_eq!(Format::from_str("openai"), Some(Format::OpenAI));
        assert_eq!(Format::from_str("claude"), Some(Format::Claude));
        assert_eq!(Format::from_str("anthropic"), Some(Format::Claude));
        assert_eq!(
            Format::from_str("openai-response"),
            Some(Format::OpenAIResponses)
        );
        assert_eq!(
            Format::from_str("openai_response"),
            Some(Format::OpenAIResponses)
        );
        assert_eq!(Format::from_str("codex"), Some(Format::Codex));
        assert_eq!(Format::from_str("unknown"), None);
    }

    #[test]
    fn test_format_display() {
        assert_eq!(Format::OpenAI.to_string(), "openai");
        assert_eq!(Format::Claude.to_string(), "claude");
        assert_eq!(Format::Codex.to_string(), "codex");
    }
}
