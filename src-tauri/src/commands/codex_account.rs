use crate::modules::codex_account_runtime::{
    AccountConnectionSummary, CodexLoginSessionStart, CodexLoginSessionStatus,
    ProductionCodexAccountRuntime,
};
use std::sync::Arc;
use tauri_plugin_opener::OpenerExt;

#[derive(Debug, serde::Serialize)]
pub struct CodexCommandError {
    pub code: &'static str,
    pub message: &'static str,
}

#[derive(Debug, serde::Serialize)]
pub struct CodexBrowserLoginStart {
    #[serde(flatten)]
    pub start: CodexLoginSessionStart,
    pub browser_opened: bool,
}

fn command_error(
    error: crate::modules::codex_account_runtime::CodexAccountRuntimeError,
) -> CodexCommandError {
    CodexCommandError {
        code: crate::modules::codex_account_runtime::error_code(&error),
        message: "Codex account operation failed",
    }
}

#[tauri::command]
pub async fn start_codex_login(
    app: tauri::AppHandle,
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
) -> Result<CodexBrowserLoginStart, CodexCommandError> {
    let start = runtime.start().await.map_err(command_error)?;
    let browser_opened = app
        .opener()
        .open_url(&start.authorization_url, None::<String>)
        .is_ok();
    Ok(CodexBrowserLoginStart {
        start,
        browser_opened,
    })
}

#[tauri::command]
pub fn get_codex_login_status(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
    session_id: uuid::Uuid,
) -> Result<CodexLoginSessionStatus, CodexCommandError> {
    runtime.status(session_id).map_err(command_error)
}

#[tauri::command]
pub async fn cancel_codex_login(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
    session_id: uuid::Uuid,
) -> Result<(), CodexCommandError> {
    runtime.cancel(session_id).await.map_err(command_error)
}

#[tauri::command]
pub async fn list_account_connections(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
) -> Result<Vec<AccountConnectionSummary>, CodexCommandError> {
    runtime.list_connections().await.map_err(command_error)
}
