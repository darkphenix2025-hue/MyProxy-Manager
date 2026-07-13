use crate::utils::protobuf;
use rusqlite::Connection;
use std::path::PathBuf;

/// Get database path (cross-platform)
pub fn get_db_path() -> Result<PathBuf, String> {
    // Standard mode: use system default path
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir().ok_or("Failed to get home directory")?;
        Ok(home.join("Library/Application Support/Antigravity/User/globalStorage/state.vscdb"))
    }

    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA")
            .map_err(|_| "Failed to get APPDATA environment variable".to_string())?;
        Ok(PathBuf::from(appdata).join("Antigravity\\User\\globalStorage\\state.vscdb"))
    }

    #[cfg(target_os = "linux")]
    {
        let home = dirs::home_dir().ok_or("Failed to get home directory")?;
        Ok(home.join(".config/Antigravity/User/globalStorage/state.vscdb"))
    }
}

/// Inject Token and Email into database
pub fn inject_token(
    db_path: &PathBuf,
    access_token: &str,
    refresh_token: &str,
    expiry: i64,
    email: &str,
    is_gcp_tos: bool,
    project_id: Option<&str>,
) -> Result<String, String> {
    crate::modules::logger::log_info("Starting Token injection...");

    inject_new_format(
        db_path,
        access_token,
        refresh_token,
        expiry,
        email,
        is_gcp_tos,
        project_id,
    )
}

/// New format injection
fn inject_new_format(
    db_path: &PathBuf,
    access_token: &str,
    refresh_token: &str,
    expiry: i64,
    email: &str,
    is_gcp_tos: bool,
    project_id: Option<&str>,
) -> Result<String, String> {
    let conn = Connection::open(db_path).map_err(|e| format!("Failed to open database: {}", e))?;

    // Create OAuthTokenInfo (binary)
    let oauth_info = protobuf::create_oauth_info(access_token, refresh_token, expiry, is_gcp_tos);
    let outer_b64 = protobuf::create_unified_state_entry("oauthTokenInfoSentinelKey", &oauth_info);

    conn.execute(
        "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?, ?)",
        ["antigravityUnifiedStateSync.oauthToken", &outer_b64],
    )
    .map_err(|e| format!("Failed to write new format: {}", e))?;

    inject_user_status(&conn, email)?;

    if let Some(project_id) = project_id.map(str::trim).filter(|pid| !pid.is_empty()) {
        inject_enterprise_project_preference(&conn, project_id)?;
    } else {
        clear_enterprise_project_preference(&conn)?;
    }

    // Inject Onboarding flag
    conn.execute(
        "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?, ?)",
        ["antigravityOnboarding", "true"],
    )
    .map_err(|e| format!("Failed to write onboarding flag: {}", e))?;

    Ok("Token injection successful".to_string())
}

fn inject_user_status(conn: &Connection, email: &str) -> Result<(), String> {
    let payload = protobuf::create_minimal_user_status_payload(email);
    let entry_b64 = protobuf::create_unified_state_entry("userStatusSentinelKey", &payload);

    conn.execute(
        "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?, ?)",
        ["antigravityUnifiedStateSync.userStatus", &entry_b64],
    )
    .map_err(|e| format!("Failed to write user status: {}", e))?;

    Ok(())
}

fn inject_enterprise_project_preference(conn: &Connection, project_id: &str) -> Result<(), String> {
    let payload = protobuf::create_string_value_payload(project_id);
    let entry_b64 = protobuf::create_unified_state_entry("enterpriseGcpProjectId", &payload);

    conn.execute(
        "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?, ?)",
        [
            "antigravityUnifiedStateSync.enterprisePreferences",
            &entry_b64,
        ],
    )
    .map_err(|e| format!("Failed to write enterprise preferences: {}", e))?;

    Ok(())
}

fn clear_enterprise_project_preference(conn: &Connection) -> Result<(), String> {
    conn.execute(
        "DELETE FROM ItemTable WHERE key = ?",
        ["antigravityUnifiedStateSync.enterprisePreferences"],
    )
    .map_err(|e| format!("Failed to clear enterprise preferences: {}", e))?;

    Ok(())
}
