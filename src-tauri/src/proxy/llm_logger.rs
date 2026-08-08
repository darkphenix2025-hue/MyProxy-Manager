use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(not(test))]
const LOG_FILE: &str = "/tmp/proxy_llm.log";
#[cfg(not(test))]
const DETAILS_DIR: &str = "/tmp/proxy_llm_details";

#[cfg(test)]
const LOG_FILE: &str = "/tmp/proxy_llm_test.log";
#[cfg(test)]
const DETAILS_DIR: &str = "/tmp/proxy_llm_test_details";

static ENABLED: AtomicBool = AtomicBool::new(false);

/// Body content for logging — supports both JSON (for request/response bodies)
/// and raw text (for SSE streams).
#[derive(Debug, Clone)]
pub enum LlmBody {
    Json(serde_json::Value),
    Text(String),
}

impl Serialize for LlmBody {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            LlmBody::Json(v) => v.serialize(serializer),
            LlmBody::Text(s) => serializer.serialize_str(s),
        }
    }
}

/// A single stage of the LLM traffic log.
#[derive(Serialize)]
pub struct LlmLogEntry {
    pub trace_id: String,
    pub timestamp: String,
    pub stage: &'static str,
    pub method: String,
    pub url: String,
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub body_size_bytes: usize,
    pub body: LlmBody,
}

/// Initialize the logger: create the details directory if needed.
#[allow(dead_code)]
pub fn init() {
    let _ = std::fs::create_dir_all(DETAILS_DIR);
}

/// Enable or disable LLM traffic logging.
#[allow(dead_code)]
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    if enabled {
        init();
    }
}

/// Check if LLM traffic logging is enabled.
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Log a single entry: writes one line to the main log file and a full detail JSON file.
pub fn log_entry(entry: &LlmLogEntry) {
    if !is_enabled() {
        return;
    }

    let detail_filename = format!("{}_{}.json", &entry.trace_id, entry.stage);
    let detail_path = PathBuf::from(DETAILS_DIR).join(&detail_filename);

    // Write detail JSON file (full body)
    match serde_json::to_string_pretty(entry) {
        Ok(json_str) => {
            if let Err(e) = std::fs::write(&detail_path, &json_str) {
                tracing::warn!(
                    "[LLM-Log] Failed to write detail file {}: {}",
                    detail_path.display(),
                    e
                );
                return;
            }
        }
        Err(e) => {
            tracing::warn!("[LLM-Log] Failed to serialize entry: {}", e);
            return;
        }
    }

    // Build main log line
    let direction = match entry.stage {
        "client_request" | "upstream_request" => "→",
        _ => "←",
    };

    let status_str = entry.status.map(|s| format!("{} ", s)).unwrap_or_default();
    let model_str = entry
        .model
        .as_deref()
        .map(|m| format!("model={} ", m))
        .unwrap_or_default();

    let line = format!(
        "[{}] [trace:{}] {}  {}  {} {}{}{}detail={}\n",
        entry.timestamp,
        entry.trace_id,
        entry.stage,
        direction,
        &entry.method,
        &entry.url,
        status_str,
        model_str,
        detail_filename,
    );

    // Append to main log file
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(LOG_FILE)
    {
        Ok(mut file) => {
            if let Err(e) = file.write_all(line.as_bytes()) {
                tracing::warn!("[LLM-Log] Failed to write main log: {}", e);
            }
        }
        Err(e) => {
            tracing::warn!("[LLM-Log] Failed to open log file {}: {}", LOG_FILE, e);
        }
    }
}

/// Helper: build a timestamp string for log entries.
pub fn now_timestamp() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize all test file operations to prevent cross-test contamination.
    static TEST_MUTEX: Mutex<()> = Mutex::new(());

    fn cleanup() {
        let _ = std::fs::remove_file(LOG_FILE);
        let _ = std::fs::remove_dir_all(DETAILS_DIR);
    }

    #[test]
    fn test_disabled_by_default() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_enabled(false);
        assert!(!is_enabled());
    }

    #[test]
    fn test_enable_and_disable() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_enabled(true);
        assert!(is_enabled());
        set_enabled(false);
        assert!(!is_enabled());
    }

    #[test]
    fn test_init_creates_details_dir() {
        let _guard = TEST_MUTEX.lock().unwrap();
        cleanup();
        init();
        assert!(std::path::Path::new(DETAILS_DIR).is_dir());
        cleanup();
    }

    #[test]
    fn test_log_entry_when_disabled_does_nothing() {
        let _guard = TEST_MUTEX.lock().unwrap();
        cleanup();
        set_enabled(false);
        let entry = LlmLogEntry {
            trace_id: "test123".to_string(),
            timestamp: now_timestamp(),
            stage: "client_request",
            method: "POST".to_string(),
            url: "/v1/messages".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            status: None,
            content_type: Some("application/json".to_string()),
            body_size_bytes: 42,
            body: LlmBody::Json(serde_json::json!({"test": true})),
        };
        log_entry(&entry);
        // No files should be created
        assert!(!std::path::Path::new(LOG_FILE).exists());
        assert!(!std::path::Path::new(DETAILS_DIR).exists());
    }

    #[test]
    fn test_log_entry_creates_main_log_and_detail() {
        let _guard = TEST_MUTEX.lock().unwrap();
        cleanup();
        set_enabled(true);

        let entry = LlmLogEntry {
            trace_id: "testabc".to_string(),
            timestamp: now_timestamp(),
            stage: "client_request",
            method: "POST".to_string(),
            url: "/v1/messages".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            status: None,
            content_type: Some("application/json".to_string()),
            body_size_bytes: 42,
            body: LlmBody::Json(
                serde_json::json!({"model": "claude-sonnet-4-6", "messages": [{"role": "user", "content": "hello"}]}),
            ),
        };
        log_entry(&entry);

        // Main log exists
        assert!(std::path::Path::new(LOG_FILE).exists());
        let main_log = std::fs::read_to_string(LOG_FILE).unwrap();
        assert!(main_log.contains("client_request"));
        assert!(main_log.contains("testabc"));
        assert!(main_log.contains("claude-sonnet-4-6"));
        assert!(main_log.contains("detail=testabc_client_request.json"));

        // Detail file exists
        let detail_path = format!("{}/testabc_client_request.json", DETAILS_DIR);
        assert!(std::path::Path::new(&detail_path).exists());
        let detail: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&detail_path).unwrap()).unwrap();
        assert_eq!(detail["trace_id"], "testabc");
        assert_eq!(detail["stage"], "client_request");
        assert_eq!(detail["model"], "claude-sonnet-4-6");

        cleanup();
    }

    #[test]
    fn test_log_entry_with_text_body_for_sse() {
        let _guard = TEST_MUTEX.lock().unwrap();
        cleanup();
        set_enabled(true);

        let sse_body = "event: message_start\ndata: {\"type\":\"message_start\"}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\"}\n\n";
        let entry = LlmLogEntry {
            trace_id: "testdef".to_string(),
            timestamp: now_timestamp(),
            stage: "upstream_response",
            method: "POST".to_string(),
            url: "https://example.com/v1/messages".to_string(),
            model: Some("gemini-2.5-pro".to_string()),
            status: Some(200),
            content_type: Some("text/event-stream".to_string()),
            body_size_bytes: sse_body.len(),
            body: LlmBody::Text(sse_body.to_string()),
        };
        log_entry(&entry);

        let main_log = std::fs::read_to_string(LOG_FILE).unwrap();
        assert!(main_log.contains("upstream_response"));
        assert!(main_log.contains("200 "));
        assert!(main_log.contains("gemini-2.5-pro"));

        let detail_path = format!("{}/testdef_upstream_response.json", DETAILS_DIR);
        let detail_content = std::fs::read_to_string(&detail_path).unwrap();
        assert!(detail_content.contains("text/event-stream"));
        assert!(detail_content.contains("message_start"));

        cleanup();
    }

    #[test]
    fn test_four_stage_complete_chain() {
        let _guard = TEST_MUTEX.lock().unwrap();
        cleanup();
        set_enabled(true);
        let tid = "chain01";

        // Stage 1: client_request
        log_entry(&LlmLogEntry {
            trace_id: tid.to_string(),
            timestamp: now_timestamp(),
            stage: "client_request",
            method: "POST".to_string(),
            url: "/v1/messages".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            status: None,
            content_type: Some("application/json".to_string()),
            body_size_bytes: 100,
            body: LlmBody::Json(
                serde_json::json!({"model": "claude-sonnet-4-6", "messages": [{"role": "user", "content": "Hello"}]}),
            ),
        });

        // Stage 2: upstream_request
        log_entry(&LlmLogEntry {
            trace_id: tid.to_string(),
            timestamp: now_timestamp(),
            stage: "upstream_request",
            method: "POST".to_string(),
            url: "https://cloudcode-pa.googleapis.com/v1internal:streamGenerateContent".to_string(),
            model: Some("gemini-2.5-pro".to_string()),
            status: None,
            content_type: Some("application/json".to_string()),
            body_size_bytes: 200,
            body: LlmBody::Json(
                serde_json::json!({"contents": [{"role": "user", "parts": [{"text": "Hello"}]}]}),
            ),
        });

        // Stage 3: upstream_response
        log_entry(&LlmLogEntry {
            trace_id: tid.to_string(),
            timestamp: now_timestamp(),
            stage: "upstream_response",
            method: "POST".to_string(),
            url: "https://cloudcode-pa.googleapis.com/v1internal:streamGenerateContent".to_string(),
            model: Some("gemini-2.5-pro".to_string()),
            status: Some(200),
            content_type: Some("text/event-stream".to_string()),
            body_size_bytes: 300,
            body: LlmBody::Text("data: {\"response\": {\"candidates\": [{\"content\": {\"parts\": [{\"text\": \"Hi!\"}]}}]}}\n\ndata: [DONE]\n\n".to_string()),
        });

        // Stage 4: client_response
        log_entry(&LlmLogEntry {
            trace_id: tid.to_string(),
            timestamp: now_timestamp(),
            stage: "client_response",
            method: "POST".to_string(),
            url: "/v1/messages".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            status: Some(200),
            content_type: Some("text/event-stream".to_string()),
            body_size_bytes: 400,
            body: LlmBody::Text("event: message_start\ndata: {\"type\":\"message_start\"}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_string()),
        });

        // Verify main log has 4 lines
        let main_log = std::fs::read_to_string(LOG_FILE).unwrap();
        let line_count = main_log.lines().count();
        assert_eq!(
            line_count, 4,
            "Expected 4 log lines, got {}: {}",
            line_count, main_log
        );
        assert!(main_log.contains("client_request"));
        assert!(main_log.contains("upstream_request"));
        assert!(main_log.contains("upstream_response"));
        assert!(main_log.contains("client_response"));

        // Verify 4 detail files exist
        for stage in &[
            "client_request",
            "upstream_request",
            "upstream_response",
            "client_response",
        ] {
            let path = format!("{}/{}_{}.json", DETAILS_DIR, tid, stage);
            assert!(
                std::path::Path::new(&path).exists(),
                "Missing detail file: {}",
                path
            );
            let content = std::fs::read_to_string(&path).unwrap();
            let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
            assert_eq!(parsed["trace_id"], tid);
            assert_eq!(parsed["stage"], *stage);
        }

        cleanup();
    }

    #[test]
    fn test_multiple_trace_ids_do_not_interfere() {
        let _guard = TEST_MUTEX.lock().unwrap();
        cleanup();
        set_enabled(true);

        for tid in &["traceA", "traceB"] {
            log_entry(&LlmLogEntry {
                trace_id: tid.to_string(),
                timestamp: now_timestamp(),
                stage: "client_request",
                method: "GET".to_string(),
                url: "/v1/health".to_string(),
                model: None,
                status: Some(200),
                content_type: None,
                body_size_bytes: 0,
                body: LlmBody::Json(serde_json::json!({})),
            });
        }

        let detail_dir = std::path::Path::new(DETAILS_DIR);
        assert!(detail_dir.join("traceA_client_request.json").exists());
        assert!(detail_dir.join("traceB_client_request.json").exists());

        cleanup();
    }

    #[test]
    fn test_log_entry_with_deeply_nested_json() {
        let _guard = TEST_MUTEX.lock().unwrap();
        cleanup();
        set_enabled(true);

        let big_body = serde_json::json!({
            "model": "claude-sonnet-4-6",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "Explain this code", "cache_control": {"type": "ephemeral"}}]},
                {"role": "assistant", "content": [{"type": "thinking", "thinking": "Let me analyze...", "signature": "base64sig123"}]}
            ],
            "thinking": {"type": "enabled", "budget_tokens": 8192},
            "tools": [
                {"name": "bash", "description": "Run shell commands", "input_schema": {"type": "object", "properties": {"command": {"type": "string"}}}}
            ]
        });

        log_entry(&LlmLogEntry {
            trace_id: "nested01".to_string(),
            timestamp: now_timestamp(),
            stage: "upstream_request",
            method: "POST".to_string(),
            url: "/v1/messages".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            status: None,
            content_type: Some("application/json".to_string()),
            body_size_bytes: serde_json::to_string(&big_body).unwrap().len(),
            body: LlmBody::Json(big_body),
        });

        let detail_path = format!("{}/nested01_upstream_request.json", DETAILS_DIR);
        let content = std::fs::read_to_string(&detail_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            parsed["body"]["messages"][1]["content"][0]["type"],
            "thinking"
        );
        assert_eq!(parsed["body"]["thinking"]["budget_tokens"], 8192);

        cleanup();
    }
}
