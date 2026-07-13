use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

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
