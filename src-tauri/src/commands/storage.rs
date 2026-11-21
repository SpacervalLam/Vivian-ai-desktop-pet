use tauri_plugin_shell::ShellExt;

#[tauri::command]
pub fn open_storage_folder(app: tauri::AppHandle) -> Result<(), String> {
    let root = crate::utils::path::get_user_data_dir();
    if !root.is_dir() { return Err("数据目录不存在".into()); }
    // The JS shell scope only accepts URLs. This command opens one fixed directory.
    #[allow(deprecated)]
    app.shell().open(root.to_string_lossy().into_owned(), None).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_storage_usage() -> Result<crate::storage::usage::StorageUsage, String> {
    tokio::task::spawn_blocking(|| crate::storage::usage::scan(&crate::utils::path::get_user_data_dir()))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clear_storage_cache() -> Result<(), String> {
    tokio::task::spawn_blocking(|| crate::storage::usage::clear_cache(&crate::utils::path::get_user_data_dir()))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn clear_browser_cache() -> Result<crate::storage::usage::BrowserCacheClear, String> {
    tokio::task::spawn_blocking(|| crate::storage::usage::clear_browser_cache(&crate::utils::path::get_user_data_dir()))
        .await.map_err(|e| e.to_string())?
}
