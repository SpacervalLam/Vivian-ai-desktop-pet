//! Non-recursive native watches for directories currently visible in the workspace tree.
use std::{collections::HashMap, path::PathBuf};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use tauri::ipc::Channel;

static WATCHES: Lazy<Mutex<HashMap<String, (String, RecommendedWatcher)>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[tauri::command]
pub fn coding_watch_directories(
    window: tauri::Window,
    directories: Vec<String>,
    on_change: Channel<String>,
) -> Result<String, String> {
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        match event {
            Ok(event) => {
                // Content/access/metadata changes cannot change directory tree entries.
                if event.need_rescan() || matches!(event.kind, EventKind::Any | EventKind::Other) {
                    let _ = on_change.send(String::new());
                } else if matches!(event.kind, EventKind::Create(_) | EventKind::Remove(_)
                    | EventKind::Modify(notify::event::ModifyKind::Name(_))) {
                    for path in event.paths {
                        if let Some(parent) = path.parent() {
                            let _ = on_change.send(parent.to_string_lossy().into_owned());
                        }
                    }
                }
            }
            Err(error) => {
                tracing::warn!("Workspace filesystem watch error: {error}");
                let _ = on_change.send(String::new());
            }
        }
    }).map_err(|e| e.to_string())?;
    for directory in directories {
        let path = PathBuf::from(directory);
        // A deleted expanded directory can disappear before watch registration.
        if path.is_dir() {
            watcher.watch(&path, RecursiveMode::NonRecursive).map_err(|e| e.to_string())?;
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    WATCHES.lock().insert(id.clone(), (window.label().to_owned(), watcher));
    Ok(id)
}

#[tauri::command]
pub fn coding_unwatch_directories(window: tauri::Window, id: String) {
    let mut watches = WATCHES.lock();
    if watches.get(&id).is_some_and(|(owner, _)| owner == window.label()) {
        watches.remove(&id);
    }
}

pub fn window_destroyed(label: &str) {
    WATCHES.lock().retain(|_, (owner, _)| owner != label);
}
