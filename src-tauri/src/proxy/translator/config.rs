/// 转换器功能开关配置。
///
/// 用于灰度控制新协议转换单元的启用状态。
/// 初期默认全部关闭，保持现有 Gemini 枢纽路径不变。
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslatorConfig {
    /// 是否启用新转换单元（默认 false）
    #[serde(default)]
    pub enabled: bool,
    /// 按客户端协议独立开关
    /// 例如: `{"openai": true, "claude": false}`
    #[serde(default)]
    pub format_toggle: HashMap<String, bool>,
}

impl Default for TranslatorConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            format_toggle: HashMap::new(),
        }
    }
}

impl TranslatorConfig {
    /// 检查指定客户端协议是否启用新转换路径
    pub fn is_format_enabled(&self, format: &str) -> bool {
        if !self.enabled {
            return false;
        }
        // 如果 format_toggle 中有显式配置，以其为准
        // 否则默认启用（因为全局 enabled=true）
        self.format_toggle.get(format).copied().unwrap_or(true)
    }

    /// 配置文件路径
    fn config_path() -> PathBuf {
        // 使用应用的 config 目录
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("antigravity-tools");
        config_dir.join("translator_config.json")
    }

    /// 从文件加载配置，失败时返回默认配置
    pub fn load() -> Self {
        let path = Self::config_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// 保存配置到文件
    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}

// ─── 全局配置单例 ───

static GLOBAL_CONFIG: RwLock<Option<TranslatorConfig>> = RwLock::new(None);

impl TranslatorConfig {
    /// 获取全局配置（懒加载）
    pub fn global_config() -> Self {
        // 先尝试读
        {
            let guard = GLOBAL_CONFIG.read().unwrap();
            if let Some(config) = guard.as_ref() {
                return config.clone();
            }
        }
        // 需要加载
        let config = Self::load();
        {
            let mut guard = GLOBAL_CONFIG.write().unwrap();
            *guard = Some(config.clone());
        }
        config
    }

    /// 更新全局配置并保存
    pub fn update_global<F: FnOnce(&mut Self)>(&self, f: F) {
        let mut guard = GLOBAL_CONFIG.write().unwrap();
        let mut config = self.clone();
        f(&mut config);
        config.save();
        *guard = Some(config);
    }
}
