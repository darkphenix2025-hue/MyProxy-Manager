use bytes::Bytes;
use futures::{Stream, StreamExt};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;

const MAX_CAPTURED_RESPONSE_BYTES: usize = 100 * 1024 * 1024;

/// Captured upstream request/response data for a single trace.
#[derive(Debug, Clone, Default)]
pub struct UpstreamTrace {
    pub request_body: Option<String>,
    pub response_body: Option<String>,
}

/// In-memory cache to temporarily store upstream request/response data keyed by trace_id.
///
/// Provider functions write upstream request/response here. Monitor middleware reads from here
/// to populate `ProxyRequestLog.upstream_request_body` / `upstream_response_body`.
///
/// Entries expire after `TTL_SECONDS` and the cache is capped at `MAX_ENTRIES`.
pub struct UpstreamTraceCache {
    store: Arc<RwLock<HashMap<String, (std::time::Instant, UpstreamTrace)>>>,
}

impl UpstreamTraceCache {
    const TTL_SECONDS: u64 = 300; // 5 minutes
    const MAX_ENTRIES: usize = 1000;

    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Store upstream trace data for the given trace_id.
    /// If an entry already exists, missing fields are filled in.
    pub async fn put(&self, trace_id: &str, trace: UpstreamTrace) {
        let mut store = self.store.write().await;
        self.cleanup_expired(&mut store);

        // Merge with existing entry if present
        let entry = store
            .entry(trace_id.to_string())
            .or_insert_with(|| (std::time::Instant::now(), UpstreamTrace::default()));
        entry.0 = std::time::Instant::now(); // refresh timestamp
        if trace.request_body.is_some() {
            entry.1.request_body = trace.request_body;
        }
        if trace.response_body.is_some() {
            entry.1.response_body = trace.response_body;
        }
    }

    /// Retrieve and remove the upstream trace for the given trace_id.
    pub async fn take(&self, trace_id: &str) -> Option<UpstreamTrace> {
        let mut store = self.store.write().await;
        store.remove(trace_id).map(|(_, trace)| trace)
    }

    /// Peek at the upstream trace without removing it.
    pub async fn get(&self, trace_id: &str) -> Option<UpstreamTrace> {
        let store = self.store.read().await;
        store.get(trace_id).map(|(_, trace)| trace.clone())
    }

    /// Append a chunk of the raw upstream response as soon as it is received.
    ///
    /// Streaming responses can be stopped by the downstream consumer immediately
    /// after a terminal event. Persisting each chunk here ensures the portion that
    /// actually arrived from the provider is still available for traffic-log
    /// diagnostics even when the stream is not polled to completion.
    pub async fn append_response_chunk(&self, trace_id: &str, chunk: &[u8]) {
        if chunk.is_empty() {
            return;
        }

        let mut store = self.store.write().await;
        self.cleanup_expired(&mut store);
        let entry = store
            .entry(trace_id.to_string())
            .or_insert_with(|| (std::time::Instant::now(), UpstreamTrace::default()));
        entry.0 = std::time::Instant::now();

        let response = entry.1.response_body.get_or_insert_with(String::new);
        if response.len() >= MAX_CAPTURED_RESPONSE_BYTES {
            return;
        }

        let remaining = MAX_CAPTURED_RESPONSE_BYTES - response.len();
        let captured = String::from_utf8_lossy(&chunk[..chunk.len().min(remaining)]);
        response.push_str(&captured);
    }

    fn cleanup_expired(&self, store: &mut HashMap<String, (std::time::Instant, UpstreamTrace)>) {
        let now = std::time::Instant::now();
        store.retain(|_, (ts, _)| now.duration_since(*ts).as_secs() < Self::TTL_SECONDS);

        // Cap at MAX_ENTRIES by removing oldest entries
        if store.len() > Self::MAX_ENTRIES {
            let mut entries: Vec<_> = store.iter().collect();
            entries.sort_by_key(|(_, (ts, _))| *ts);
            let to_remove = entries.len() - Self::MAX_ENTRIES;
            let keys_to_remove: Vec<_> = entries
                .into_iter()
                .take(to_remove)
                .map(|(k, _)| k.clone())
                .collect();
            for key in keys_to_remove {
                store.remove(&key);
            }
        }
    }
}

impl Clone for UpstreamTraceCache {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
        }
    }
}

/// Wrap a provider byte stream and capture raw response chunks before yielding
/// them to the protocol translator or downstream proxy response.
pub fn capture_response_stream<S, E>(
    stream: Pin<Box<S>>,
    cache: UpstreamTraceCache,
    trace_id: Option<String>,
) -> Pin<Box<dyn Stream<Item = Result<Bytes, E>> + Send>>
where
    S: Stream<Item = Result<Bytes, E>> + Send + ?Sized + 'static,
    E: Send + 'static,
{
    let captured = async_stream::stream! {
        let mut upstream = stream;
        while let Some(item) = upstream.next().await {
            if let (Some(trace_id), Ok(bytes)) = (trace_id.as_deref(), &item) {
                cache.append_response_chunk(trace_id, bytes).await;
            }
            yield item;
        }
    };

    Box::pin(captured)
}

#[cfg(test)]
mod tests {
    use super::{capture_response_stream, UpstreamTrace, UpstreamTraceCache};
    use bytes::Bytes;
    use futures::{stream, StreamExt};

    #[tokio::test]
    async fn captured_stream_records_each_chunk_before_yielding_it() {
        let cache = UpstreamTraceCache::new();
        cache
            .put(
                "trace-1",
                UpstreamTrace {
                    request_body: Some("request".to_string()),
                    response_body: None,
                },
            )
            .await;

        let mut captured = capture_response_stream(
            Box::pin(stream::iter(vec![
                Ok::<Bytes, std::io::Error>(Bytes::from_static(b"first")),
                Ok::<Bytes, std::io::Error>(Bytes::from_static(b" second")),
            ])),
            cache.clone(),
            Some("trace-1".to_string()),
        );

        assert_eq!(
            captured.next().await.unwrap().unwrap(),
            Bytes::from("first")
        );
        assert_eq!(
            cache.get("trace-1").await.unwrap().response_body.as_deref(),
            Some("first")
        );

        assert_eq!(
            captured.next().await.unwrap().unwrap(),
            Bytes::from(" second")
        );
        assert_eq!(
            cache.get("trace-1").await.unwrap().response_body.as_deref(),
            Some("first second")
        );
    }

    #[tokio::test]
    async fn captured_stream_keeps_partial_response_when_consumer_stops_early() {
        let cache = UpstreamTraceCache::new();
        cache
            .put(
                "trace-2",
                UpstreamTrace {
                    request_body: Some("request".to_string()),
                    response_body: None,
                },
            )
            .await;

        let mut captured = capture_response_stream(
            Box::pin(stream::iter(vec![
                Ok::<Bytes, std::io::Error>(Bytes::from_static(b"received")),
                Ok::<Bytes, std::io::Error>(Bytes::from_static(b" never-read")),
            ])),
            cache.clone(),
            Some("trace-2".to_string()),
        );

        let _ = captured.next().await;
        drop(captured);

        assert_eq!(
            cache.get("trace-2").await.unwrap().response_body.as_deref(),
            Some("received")
        );
    }
}
