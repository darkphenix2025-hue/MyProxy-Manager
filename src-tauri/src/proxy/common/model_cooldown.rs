use dashmap::DashMap;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A provider/model pair that is temporarily unavailable after an upstream
/// rate-limit response.  Protocol is part of the key because one logical
/// provider may expose the same model through multiple protocols.
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize)]
pub struct CooldownKey {
    pub provider: String,
    pub protocol: String,
    pub model: String,
}

/// A cooldown entry stored in memory. `Instant` is used for reliable expiry
/// checks; the wall-clock timestamp is only for API/UI presentation.
#[derive(Debug, Clone)]
struct CooldownEntry {
    started_at: Instant,
    started_at_epoch_secs: i64,
    duration_secs: u64,
    reason: String,
}

impl CooldownEntry {
    fn is_expired(&self) -> bool {
        self.started_at.elapsed() >= Duration::from_secs(self.duration_secs)
    }

    fn remaining_secs(&self) -> u64 {
        self.duration_secs
            .saturating_sub(self.started_at.elapsed().as_secs())
    }
}

/// Serializable active cooldown returned to the management UI.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveCooldown {
    pub provider: String,
    pub protocol: String,
    pub model: String,
    pub remaining_secs: u64,
    pub duration_secs: u64,
    pub started_at: i64,
    pub reason: String,
}

/// Runtime model cooldown registry.
///
/// Entries are kept in memory intentionally: a rate-limit is a transient
/// runtime condition and must not survive an application restart. Expired
/// entries are removed lazily whenever they are queried or when a new entry
/// is inserted.
pub struct ModelCooldownManager {
    entries: DashMap<CooldownKey, CooldownEntry>,
    enabled: AtomicBool,
    default_duration_secs: AtomicU64,
}

impl ModelCooldownManager {
    pub fn new(enabled: bool, default_duration_secs: u64) -> Self {
        Self {
            entries: DashMap::new(),
            enabled: AtomicBool::new(enabled),
            default_duration_secs: AtomicU64::new(normalize_duration(default_duration_secs)),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Apply hot-updated settings without clearing active entries. Disabling
    /// cooldown prevents new entries and makes existing entries immediately
    /// irrelevant to routing; they are cleared to avoid stale UI state.
    pub fn update_settings(&self, enabled: bool, duration_secs: u64) {
        self.enabled.store(enabled, Ordering::Relaxed);
        self.default_duration_secs
            .store(normalize_duration(duration_secs), Ordering::Relaxed);
        if !enabled {
            self.clear();
        }
    }

    pub fn default_duration_secs(&self) -> u64 {
        self.default_duration_secs.load(Ordering::Relaxed)
    }

    /// Record a model cooldown using the configured duration.
    pub fn mark_cooldown(&self, key: CooldownKey, reason: &str) -> bool {
        self.mark_cooldown_for(key, reason, None)
    }

    /// Record a model cooldown, optionally honoring an upstream Retry-After
    /// value. A server-provided duration is bounded to keep malformed headers
    /// from creating an effectively permanent route block.
    pub fn mark_cooldown_for(
        &self,
        key: CooldownKey,
        reason: &str,
        duration_override_secs: Option<u64>,
    ) -> bool {
        if !self.is_enabled() {
            return false;
        }

        self.purge_expired();
        let duration_secs = duration_override_secs
            .map(normalize_duration)
            .unwrap_or_else(|| self.default_duration_secs());
        let started_at_epoch_secs = unix_timestamp_secs();

        tracing::warn!(
            "[ModelCooldown] provider='{}', protocol='{}', model='{}' entering cooldown for {}s (reason: {})",
            key.provider,
            key.protocol,
            key.model,
            duration_secs,
            reason
        );
        self.entries.insert(
            key,
            CooldownEntry {
                started_at: Instant::now(),
                started_at_epoch_secs,
                duration_secs,
                reason: reason.to_string(),
            },
        );
        true
    }

    /// Check whether a model is currently cooling down. Expired entries are
    /// removed as part of the check so routing never sees stale state.
    pub fn is_cooled_down(&self, key: &CooldownKey) -> bool {
        if let Some(entry) = self.entries.get(key) {
            if entry.is_expired() {
                drop(entry);
                self.entries.remove(key);
                return false;
            }
            return true;
        }
        false
    }

    pub fn remaining_secs(&self, key: &CooldownKey) -> u64 {
        if let Some(entry) = self.entries.get(key) {
            if entry.is_expired() {
                drop(entry);
                self.entries.remove(key);
                return 0;
            }
            return entry.remaining_secs();
        }
        0
    }

    /// Remove expired entries and return the number removed.
    pub fn purge_expired(&self) -> usize {
        let mut count = 0;
        self.entries.retain(|_, entry| {
            let alive = !entry.is_expired();
            if !alive {
                count += 1;
            }
            alive
        });
        count
    }

    pub fn clear(&self) -> usize {
        let count = self.entries.len();
        self.entries.clear();
        count
    }

    /// Return active entries in a stable order for the UI and HTTP API.
    pub fn get_active_cooldowns(&self) -> Vec<ActiveCooldown> {
        self.purge_expired();
        let mut active: Vec<_> = self
            .entries
            .iter()
            .map(|entry| ActiveCooldown {
                provider: entry.key().provider.clone(),
                protocol: entry.key().protocol.clone(),
                model: entry.key().model.clone(),
                remaining_secs: entry.value().remaining_secs(),
                duration_secs: entry.value().duration_secs,
                started_at: entry.value().started_at_epoch_secs,
                reason: entry.value().reason.clone(),
            })
            .collect();
        active.sort_by(|left, right| {
            (&left.provider, &left.protocol, &left.model).cmp(&(
                &right.provider,
                &right.protocol,
                &right.model,
            ))
        });
        active
    }
}

fn normalize_duration(duration_secs: u64) -> u64 {
    duration_secs.clamp(1, 86_400)
}

fn unix_timestamp_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(protocol: &str) -> CooldownKey {
        CooldownKey {
            provider: "big".to_string(),
            protocol: protocol.to_string(),
            model: "glm-5".to_string(),
        }
    }

    #[test]
    fn cooldown_is_scoped_by_protocol() {
        let manager = ModelCooldownManager::new(true, 600);
        manager.mark_cooldown(key("openai_compatible"), "429");

        assert!(manager.is_cooled_down(&key("openai_compatible")));
        assert!(!manager.is_cooled_down(&key("anthropic_passthrough")));
        assert_eq!(manager.get_active_cooldowns().len(), 1);
    }

    #[test]
    fn disabled_manager_does_not_record_entries() {
        let manager = ModelCooldownManager::new(false, 600);
        assert!(!manager.mark_cooldown(key("openai_compatible"), "429"));
        assert!(manager.get_active_cooldowns().is_empty());
    }

    #[test]
    fn retry_after_duration_is_exposed_to_callers() {
        let manager = ModelCooldownManager::new(true, 600);
        manager.mark_cooldown_for(key("openai_compatible"), "429", Some(37));

        let active = manager.get_active_cooldowns();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].duration_secs, 37);
        assert!(active[0].remaining_secs <= 37);
        assert!(active[0].started_at > 0);
    }
}
