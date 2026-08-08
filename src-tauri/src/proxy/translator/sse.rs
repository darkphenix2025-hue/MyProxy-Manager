/// SSE 事件构建工具。
///
/// 提供低分配的 SSE 事件格式化函数，用于协议转换中的流响应构建。
/// 所有函数直接操作字节，避免中间 String 分配。
use bytes::BytesMut;

/// 将 SSE 事件追加到缓冲区。
///
/// 格式：
/// ```text
/// event: <event_type>
/// id: <id>
/// data: <data_json>
///
/// ```
pub fn append_sse_event(buf: &mut BytesMut, event_type: &str, id: Option<&str>, data: &[u8]) {
    buf.extend_from_slice(b"event: ");
    buf.extend_from_slice(event_type.as_bytes());
    buf.extend_from_slice(b"\n");
    if let Some(id) = id {
        buf.extend_from_slice(b"id: ");
        buf.extend_from_slice(id.as_bytes());
        buf.extend_from_slice(b"\n");
    }
    buf.extend_from_slice(b"data: ");
    buf.extend_from_slice(data);
    buf.extend_from_slice(b"\n\n");
}

/// 便捷函数：不带 ID 的 SSE 事件
pub fn append_sse_event_no_id(buf: &mut BytesMut, event_type: &str, data: &[u8]) {
    append_sse_event(buf, event_type, None, data);
}

/// 构建一个完整的 SSE 数据行（不含 event/id 头）。
/// 用于直接拼接 data: 前缀后的内容。
pub fn sse_data_line(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 8);
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(data);
    out.extend_from_slice(b"\n\n");
    out
}

/// 构建 OpenAI 风格的 `[DONE]` 结束标记。
pub fn openai_done_marker() -> Vec<u8> {
    sse_data_line(b"[DONE]")
}

/// 从 SSE 行中提取事件类型。
///
/// 解析 `event: <type>` 行，返回事件类型字符串。
/// 如果行不是事件声明，返回 None。
pub fn extract_event_type(line: &[u8]) -> Option<&str> {
    if line.starts_with(b"event: ") {
        std::str::from_utf8(&line[7..]).ok().map(|s| s.trim())
    } else {
        None
    }
}

/// 从 SSE 行中提取数据内容。
///
/// 解析 `data: <json>` 行，返回数据字节切片。
/// 如果行不是数据行，返回 None。
pub fn extract_data(line: &[u8]) -> Option<&[u8]> {
    if line.starts_with(b"data: ") {
        Some(&line[6..])
    } else {
        None
    }
}

/// 解析完整的 SSE 事件块。
///
/// SSE 事件以空行 (`\n\n`) 分隔。此函数从缓冲区中读取一个完整事件，
/// 返回 `(event_type, id, data)` 三元组。
///
/// 如果缓冲区中没有完整事件，返回 None。
pub fn parse_sse_event(buf: &mut BytesMut) -> Option<(String, Option<String>, Vec<u8>)> {
    // 查找事件分隔符
    let text = std::str::from_utf8(buf.as_ref()).ok()?;
    let end = text.find("\n\n")?;

    let event_bytes = buf.split_to(end + 2); // 包含 \n\n
    let event_text = std::str::from_utf8(&event_bytes).ok()?;

    let mut event_type = None;
    let mut event_id = None;
    let mut data = Vec::new();

    for line in event_text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("event: ") {
            event_type = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("id: ") {
            event_id = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("data: ") {
            data.extend_from_slice(rest.as_bytes());
        }
    }

    Some((event_type.unwrap_or_default(), event_id, data))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_append_sse_event() {
        let mut buf = BytesMut::new();
        append_sse_event(&mut buf, "message", Some("1"), b"{}");
        let result = std::str::from_utf8(&buf).unwrap();
        assert!(result.contains("event: message"));
        assert!(result.contains("id: 1"));
        assert!(result.contains("data: {}"));
        assert!(result.ends_with("\n\n"));
    }

    #[test]
    fn test_append_sse_event_no_id() {
        let mut buf = BytesMut::new();
        append_sse_event_no_id(&mut buf, "ping", b"");
        let result = std::str::from_utf8(&buf).unwrap();
        assert_eq!(result, "event: ping\ndata: \n\n");
    }

    #[test]
    fn test_openai_done_marker() {
        let result = openai_done_marker();
        assert_eq!(std::str::from_utf8(&result).unwrap(), "data: [DONE]\n\n");
    }

    #[test]
    fn test_extract_event_type() {
        assert_eq!(extract_event_type(b"event: message"), Some("message"));
        assert_eq!(extract_event_type(b"data: {}"), None);
    }

    #[test]
    fn test_extract_data() {
        assert_eq!(
            extract_data(b"data: {\"ok\":true}"),
            Some(&b"{\"ok\":true}"[..])
        );
        assert_eq!(extract_data(b"event: msg"), None);
    }

    #[test]
    fn test_parse_sse_event() {
        let mut buf =
            BytesMut::from("event: message\nid: 42\ndata: {\"text\":\"hi\"}\n\n".as_bytes());
        let (event_type, id, data) = parse_sse_event(&mut buf).unwrap();
        assert_eq!(event_type, "message");
        assert_eq!(id, Some("42".to_string()));
        assert_eq!(data, b"{\"text\":\"hi\"}");
    }
}
