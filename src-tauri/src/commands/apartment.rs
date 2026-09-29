//! 可选 3D 公寓插件的安装状态与运行时门禁。
use std::sync::Arc;
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

pub fn plugin_dir() -> Option<std::path::PathBuf> {
    if cfg!(debug_assertions) {
        return Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("plugins/3d-apartment"));
    }
    std::env::current_exe().ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("plugins/3d-apartment")))
}

pub fn installed() -> bool {
    plugin_dir().is_some_and(|dir| {
        dir.join("plugin.json").is_file()
            && (cfg!(debug_assertions) || (dir.join("room").is_dir() && dir.join("ui/room.js").is_file()))
    })
}

pub fn enabled(state: &AppState) -> bool {
    installed() && state.config.read().get_all().base.apartment_enabled
}

#[derive(Serialize)]
pub struct ApartmentPluginStatus {
    installed: bool,
    enabled: bool,
    asset_root: Option<String>,
    plugin_root: Option<String>,
}

#[tauri::command]
pub fn apartment_plugin_status(state: State<'_, Arc<AppState>>) -> ApartmentPluginStatus {
    ApartmentPluginStatus {
        installed: installed(),
        enabled: enabled(state.inner()),
        asset_root: if cfg!(debug_assertions) {
            None
        } else {
            plugin_dir().map(|dir| dir.join("room"))
                .filter(|path| path.is_dir())
                .map(|path| path.to_string_lossy().into_owned())
        },
        plugin_root: plugin_dir()
            .filter(|path| path.join("plugin.json").is_file())
            .map(|path| path.to_string_lossy().into_owned()),
    }
}
