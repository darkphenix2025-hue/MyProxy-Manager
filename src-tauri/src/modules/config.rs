use serde_json;
use std::fs;

use super::account::get_data_dir;
use crate::models::AppConfig;

const CONFIG_FILE: &str = "gui_config.json";

fn migrate_menu_visibility_defaults(value: &mut serde_json::Value) -> bool {
    let current_version = value
        .get("menu_visibility_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0) as u8;
    if current_version >= crate::models::config::MENU_VISIBILITY_DEFAULTS_VERSION {
        return false;
    }

    let mut hidden_items: Vec<String> = value
        .get("hidden_menu_items")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default();

    for path in crate::models::config::default_hidden_menu_items() {
        if !hidden_items.contains(&path) {
            hidden_items.push(path);
        }
    }

    let Some(object) = value.as_object_mut() else {
        return false;
    };
    object.insert(
        "hidden_menu_items".to_string(),
        serde_json::json!(hidden_items),
    );
    object.insert(
        "menu_visibility_version".to_string(),
        serde_json::json!(crate::models::config::MENU_VISIBILITY_DEFAULTS_VERSION),
    );
    true
}

/// Load application configuration
pub fn load_app_config() -> Result<AppConfig, String> {
    let data_dir = get_data_dir()?;
    let config_path = data_dir.join(CONFIG_FILE);

    if !config_path.exists() {
        let config = AppConfig::new();
        // [FIX #1460] Persist initial config to prevent new API Key on every refresh
        let _ = save_app_config(&config);
        return Ok(config);
    }

    let content = fs::read_to_string(&config_path)
        .map_err(|e| format!("failed_to_read_config_file: {}", e))?;

    let mut v: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("failed_to_parse_config_file: {}", e))?;

    let mut modified = false;

    // Migration logic
    if let Some(proxy) = v.get_mut("proxy") {
        // [FIX #1738] Enhanced type checking for custom_mapping
        // Ensures the field is always parsed as an object, preventing type mismatch errors
        let mut custom_mapping = match proxy.get("custom_mapping") {
            Some(m) if m.is_object() => m.as_object().unwrap().clone(),
            Some(m) => {
                // If custom_mapping is not an object type (e.g., string), log warning and reset to empty
                tracing::warn!(
                    "Invalid custom_mapping type (expected object, got {:?}), resetting to empty",
                    m
                );
                serde_json::Map::new()
            }
            None => serde_json::Map::new(),
        };

        // Migrate Anthropic mapping
        if let Some(anthropic) = proxy
            .get_mut("anthropic_mapping")
            .and_then(|m| m.as_object_mut())
        {
            for (k, v) in anthropic.iter() {
                // Only move non-series fields, as series fields are now handled by Preset logic or builtin tables
                if !k.ends_with("-series") && !custom_mapping.contains_key(k) {
                    custom_mapping.insert(k.clone(), v.clone());
                }
            }
            // Remove old field
            proxy.as_object_mut().unwrap().remove("anthropic_mapping");
            modified = true;
        }

        // Migrate OpenAI mapping
        if let Some(openai) = proxy
            .get_mut("openai_mapping")
            .and_then(|m| m.as_object_mut())
        {
            for (k, v) in openai.iter() {
                if !k.ends_with("-series") && !custom_mapping.contains_key(k) {
                    custom_mapping.insert(k.clone(), v.clone());
                }
            }
            // Remove old field
            proxy.as_object_mut().unwrap().remove("openai_mapping");
            modified = true;
        }

        if modified {
            proxy.as_object_mut().unwrap().insert(
                "custom_mapping".to_string(),
                serde_json::Value::Object(custom_mapping),
            );
        }
    }

    if migrate_menu_visibility_defaults(&mut v) {
        modified = true;
    }

    let config: AppConfig = serde_json::from_value(v)
        .map_err(|e| format!("failed_to_convert_config_after_migration: {}", e))?;

    // If migration occurred, auto-save once to clean up the file
    if modified {
        let _ = save_app_config(&config);
    }

    Ok(config)
}

/// Save application configuration
pub fn save_app_config(config: &AppConfig) -> Result<(), String> {
    let data_dir = get_data_dir()?;
    let config_path = data_dir.join(CONFIG_FILE);

    let content = serde_json::to_string_pretty(config)
        .map_err(|e| format!("failed_to_serialize_config: {}", e))?;

    fs::write(&config_path, content).map_err(|e| format!("failed_to_save_config: {}", e))
}
