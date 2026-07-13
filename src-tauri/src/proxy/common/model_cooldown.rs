use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// 冷却键：模型名 + Provider 名的组合
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct CooldownKey {
    pub model: String,
    pub provider: String,
}

/// 冷却条目
#[derive(Debug, Clone)]
pub struct CooldownEntry {
    pub started_at: Instant,
    pub duration_secs: u64,
    #[allow(dead_code)]
    pub reason: String,
}

impl CooldownEntry {
    pub fn is_expired(&self) -> bool {
        self.started_at.elapsed() >= Duration::from_secs(self.duration_secs)
    }
}

/// 模型冷却管理器
pub struct ModelCooldownManager {
    entries: DashMap<CooldownKey, CooldownEntry>,
    default_duration_secs: u64,
}

impl ModelCooldownManager {
    pub fn new(default_duration_secs: u64) -> Self {
        Self {
            entries: DashMap::new(),
            default_duration_secs,
        }
    }

    /// 标记模型进入冷却状态
    pub fn mark_cooldown(&self, key: CooldownKey, reason: &str) {
        tracing::warn!(
            "[ModelCooldown] {}@{} entering cooldown for {}s (reason: {})",
            key.model,
            key.provider,
            self.default_duration_secs,
            reason
        );
        self.entries.insert(
            key,
            CooldownEntry {
                started_at: Instant::now(),
                duration_secs: self.default_duration_secs,
                reason: reason.to_string(),
            },
        );
    }

    /// 检查模型是否处于冷却状态（过期条目自动清理）
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

    /// 获取冷却剩余秒数
    pub fn remaining_secs(&self, key: &CooldownKey) -> u64 {
        if let Some(entry) = self.entries.get(key) {
            let elapsed = entry.started_at.elapsed().as_secs();
            return entry.duration_secs.saturating_sub(elapsed);
        }
        0
    }

    /// 清理过期条目，返回清理数量
    pub fn purge_expired(&self) -> usize {
        let mut count = 0;
        self.entries.retain(|_key, entry| {
            let alive = !entry.is_expired();
            if !alive {
                count += 1;
            }
            alive
        });
        count
    }

    /// 清除所有冷却条目
    pub fn clear(&self) -> usize {
        let count = self.entries.len();
        self.entries.clear();
        count as usize
    }

    /// 获取所有活跃冷却条目（用于前端展示）
    pub fn get_active_cooldowns(&self) -> Vec<(CooldownKey, u64)> {
        self.entries
            .iter()
            .filter_map(|entry| {
                let key = entry.key().clone();
                let value = entry.value();
                if value.is_expired() {
                    None
                } else {
                    let remaining = value
                        .duration_secs
                        .saturating_sub(value.started_at.elapsed().as_secs());
                    Some((key, remaining))
                }
            })
            .collect()
    }
}
