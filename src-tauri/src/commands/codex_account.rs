use crate::modules::codex_account_runtime::{
    AccountConnectionSummary, CodexImportRequest, CodexLoginSessionStart, CodexLoginSessionStatus,
    CodexModelSummary, CodexQuotaSummary, ProductionCodexAccountRuntime,
};
use std::sync::Arc;
use tauri_plugin_opener::OpenerExt;

#[derive(Debug, serde::Serialize)]
pub struct CodexCommandError {
    pub code: &'static str,
    pub message: String,
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
        message: error.to_string(),
    }
}

#[tauri::command(rename_all = "snake_case")]
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

#[tauri::command(rename_all = "snake_case")]
pub fn get_codex_login_status(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
    session_id: uuid::Uuid,
) -> Result<CodexLoginSessionStatus, CodexCommandError> {
    runtime.status(session_id).map_err(command_error)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn cancel_codex_login(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
    session_id: uuid::Uuid,
) -> Result<(), CodexCommandError> {
    runtime.cancel(session_id).await.map_err(command_error)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_account_connections(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
) -> Result<Vec<AccountConnectionSummary>, CodexCommandError> {
    runtime.list_connections().await.map_err(command_error)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_codex_connection_enabled(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
    credential_id: String,
    enabled: bool,
) -> Result<(), CodexCommandError> {
    runtime
        .set_enabled(&credential_id, enabled)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn refresh_codex_auth_file(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
    credential_id: String,
) -> Result<(), CodexCommandError> {
    runtime
        .refresh_credential(&credential_id)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn delete_codex_auth_file(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
    credential_id: String,
) -> Result<(), CodexCommandError> {
    runtime
        .delete_credential(&credential_id)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn import_codex_auth_file(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
    content: String,
    filename: Option<String>,
) -> Result<AccountConnectionSummary, CodexCommandError> {
    runtime
        .import_auth_file(CodexImportRequest { content, filename })
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn export_codex_auth_file(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
    credential_id: String,
    path: String,
) -> Result<(), CodexCommandError> {
    runtime
        .export_auth_file(&credential_id, &path)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn list_codex_models(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
    credential_id: String,
) -> Result<Vec<CodexModelSummary>, CodexCommandError> {
    runtime
        .list_models(&credential_id)
        .await
        .map_err(command_error)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_codex_quota(
    runtime: tauri::State<'_, Arc<ProductionCodexAccountRuntime>>,
    credential_id: String,
) -> Result<CodexQuotaSummary, CodexCommandError> {
    runtime
        .get_quota(&credential_id)
        .await
        .map_err(command_error)
}
