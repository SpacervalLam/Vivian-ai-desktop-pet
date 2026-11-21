#[path = "../../../src-tauri/src/browser_bridge/sites.rs"]
pub mod browser_sites;
pub mod browser_bridge { pub use crate::browser_sites as sites; }
#[path = "../../../src-tauri/src/embedded_browser/mod.rs"]
mod embedded_browser;
#[path = "../../../src-tauri/src/browser_bridge/service.rs"]
mod native_browser_service;
static BROWSER_SERVICE: once_cell::sync::OnceCell<
    std::sync::Arc<native_browser_service::BridgeState>,
> = once_cell::sync::OnceCell::new();
#[path = "../../../src-tauri/src/utils/fs.rs"]
pub mod fs;
pub mod utils {
    pub use crate::fs;
    pub mod path {
        pub fn get_user_data_dir() -> std::path::PathBuf {
            std::path::PathBuf::from(std::env::var_os("VIVIAN_BROWSER_TEST_DIR").unwrap())
        }
    }
}
use embedded_browser::{model::*, native};
use std::{
    io::{Read, Write},
    net::TcpListener,
};
static EXIT_CODE: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);

async fn wait(
    app: &tauri::AppHandle,
    predicate: impl Fn(&BrowserSnapshot) -> bool,
) -> BrowserSnapshot {
    let mut last = None;
    for _ in 0..300 {
        let state = native::request(app.clone(), "memory".into(), BrowserRequest::Init)
            .await
            .unwrap();
        if let Some(error) = &state.error {
            panic!("Native browser error: {error}");
        }
        if predicate(&state) {
            return state;
        }
        last = Some(state);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("Timed out waiting for browser state: {last:?}");
}
async fn smoke(app: tauri::AppHandle, base: String) {
    if std::env::var_os("VIVIAN_BROWSER_PLAYWRIGHT_ONLY").is_some() {
        playwright_smoke(&app,&base).await;
        EXIT_CODE.store(0,std::sync::atomic::Ordering::Relaxed);app.exit(0);return;
    }
    if std::env::var_os("VIVIAN_BROWSER_COMPUTER_ONLY").is_some() {
        computer_smoke(&app,&base).await;
        EXIT_CODE.store(0,std::sync::atomic::Ordering::Relaxed);app.exit(0);return;
    }
    println!("Checking navigation and profile isolation...");
    let state = wait(&app, |s| s.tabs.first().is_some_and(|t| !t.loading)).await;
    let first = state.active.unwrap();
    native::layout(
        app.clone(),
        "memory".into(),
        BrowserBounds {
            x: 10.0,
            y: 100.0,
            width: 800.0,
            height: 500.0,
        },
        true,
        1,
    )
    .await
    .unwrap();
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Navigate {
            id: first.clone(),
            input: format!("{base}/one"),
        },
    )
    .await
    .unwrap();
    wait(&app, |s| {
        s.tabs
            .iter()
            .any(|t| t.id == first && !t.loading && t.title == "one")
    })
    .await;
    if std::env::var_os("VIVIAN_BROWSER_RECOVERY_ONLY").is_some() {
        recovery_smoke(&app, &base, &first).await;
        EXIT_CODE.store(0, std::sync::atomic::Ordering::Relaxed);
        app.exit(0);
        return;
    }
    println!("Checking native viewport capture for browser overlays...");
    let frame = native::automation::capture_viewport(&app, &first).await.unwrap();
    assert!(frame.starts_with("data:image/png;base64,iVBOR"));
    assert!(frame.len() > 512, "capture must contain viewport pixels");
    std::fs::write(crate::utils::path::get_user_data_dir().join("overlay-frame.json"), serde_json::to_string(&frame).unwrap()).unwrap();
    let state = native::request(app.clone(), "memory".into(), BrowserRequest::Init).await.unwrap();
    assert_eq!(state.active.as_deref(), Some(first.as_str()));
    assert_eq!(state.tabs.iter().find(|tab| tab.id == first).unwrap().url, format!("{base}/one"), "capture must not navigate or replace the live page");
    println!("Checking bookmark rename and saved metadata...");
    for page in ["bookmarks", "downloads", "history", "extensions"] {
        let url = format!("about:blank#vivian-{page}");
        let internal = native::request(app.clone(), "memory".into(), BrowserRequest::NewTab { input:url.clone(), private:false }).await.unwrap();
        let id = internal.active.unwrap();
        let internal = wait(&app, |state| state.tabs.iter().any(|tab| tab.id == id && tab.url == url && !tab.loading)).await;
        assert!(internal.tabs.iter().find(|tab| tab.id == id).unwrap().start, "internal page must keep the native viewport hidden");
        assert!(!internal.history.iter().any(|entry| entry.url == url), "management pages are not web history");
        assert!(internal.saved().tabs.iter().any(|tab| tab.url == url), "management tabs survive session restoration");
        native::request(app.clone(), "memory".into(), BrowserRequest::Close { id }).await.unwrap();
    }
    native::request(app.clone(), "memory".into(), BrowserRequest::Select { id:first.clone() }).await.unwrap();
    let original = native::request(app.clone(), "memory".into(), BrowserRequest::Bookmark { id:first.clone() }).await.unwrap().bookmarks[0].clone();
    let renamed = native::request(app.clone(), "memory".into(), BrowserRequest::RenameBookmark { url:original.url.clone(), title:"  Custom bookmark  ".into() }).await.unwrap();
    assert_eq!(renamed.bookmarks[0].title, "Custom bookmark");
    assert_eq!(renamed.bookmarks[0].url, original.url);
    assert_eq!(renamed.bookmarks[0].at, original.at);
    assert_eq!(renamed.saved().bookmarks[0].title, "Custom bookmark");
    assert!(native::request(app.clone(), "memory".into(), BrowserRequest::RenameBookmark { url:original.url.clone(), title:"   ".into() }).await.is_err());
    native::request(app.clone(), "memory".into(), BrowserRequest::RemoveBookmark { url:original.url }).await.unwrap();
    if std::env::var_os("VIVIAN_BROWSER_BACKUP_ONLY").is_some() {
        native::request(app.clone(), "memory".into(), BrowserRequest::Bookmark { id:first.clone() }).await.unwrap();
        backup_roundtrip(&app).await;
        native::request(app.clone(), "memory".into(), BrowserRequest::Navigate { id:first.clone(), input:format!("{base}/two") }).await.unwrap();
        wait(&app, |s| s.tabs.iter().any(|t| t.id == first && !t.loading && t.title == "storage-retained")).await;
        println!("PASS: backup flush, browser shutdown, admission gate, restored tabs/bookmarks and retained cookies/localStorage.");
        EXIT_CODE.store(0, std::sync::atomic::Ordering::Relaxed);
        app.exit(0);
        return;
    }
    if std::env::var_os("VIVIAN_BROWSER_OVERLAY_ONLY").is_some() {
        assert!(native::automation::capture_viewport(&app, "inactive-tab").await.is_err());
        native::request(app.clone(), "memory".into(), BrowserRequest::Navigate { id:first.clone(), input:format!("{base}/overlay") }).await.unwrap();
        wait(&app, |state| state.tabs.iter().any(|tab| tab.id == first && !tab.loading && tab.title == "overlay fixture")).await;
        native::layout(app.clone(), "memory".into(), BrowserBounds { x:10.0, y:100.0, width:800.0, height:500.0 }, false, 2).await.unwrap();
        native::layout(app.clone(), "memory".into(), BrowserBounds { x:10.0, y:100.0, width:800.0, height:500.0 }, true, 3).await.unwrap();
        let restored = native::automation::capture_viewport(&app, &first).await.unwrap();
        assert!(restored.starts_with("data:image/png;base64,iVBOR"));
        assert!(restored.len() > 512);
        std::fs::write(crate::utils::path::get_user_data_dir().join("overlay-frame.json"), serde_json::to_string(&restored).unwrap()).unwrap();
        let state = native::request(app.clone(), "memory".into(), BrowserRequest::Init).await.unwrap();
        assert_eq!(state.tabs.iter().find(|tab| tab.id == first).unwrap().url, format!("{base}/overlay"));
        println!("PASS: real WebView2 viewport capture, active-tab guard, hide/restore and page preservation.");
        EXIT_CODE.store(0, std::sync::atomic::Ordering::Relaxed); app.exit(0); return;
    }
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Navigate {
            id: first.clone(),
            input: format!("{base}/two"),
        },
    )
    .await
    .unwrap();
    wait(&app, |s| {
        s.tabs
            .iter()
            .any(|t| t.id == first && !t.loading && t.title == "storage-retained" && t.can_back)
    })
    .await;
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Action {
            id: first.clone(),
            name: "back".into(),
            text: String::new(),
        },
    )
    .await
    .unwrap();
    wait(&app, |s| {
        s.tabs
            .iter()
            .any(|t| t.id == first && !t.loading && t.title == "one" && t.can_forward)
    })
    .await;
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Action {
            id: first.clone(),
            name: "forward".into(),
            text: String::new(),
        },
    )
    .await
    .unwrap();
    wait(&app, |s| {
        s.tabs
            .iter()
            .any(|t| t.id == first && !t.loading && t.title == "storage-retained")
    })
    .await;
    let state = native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Bookmark { id: first.clone() },
    )
    .await
    .unwrap();
    assert_eq!(state.bookmarks.len(), 1);
    let state = native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Action {
            id: first.clone(),
            name: "zoom_in".into(),
            text: String::new(),
        },
    )
    .await
    .unwrap();
    assert!(state.tabs.iter().find(|t| t.id == first).unwrap().zoom > 1.0);
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::NewTab {
            input: format!("{base}/two"),
            private: true,
        },
    )
    .await
    .unwrap();
    let state = wait(&app, |s| {
        s.tabs
            .iter()
            .any(|t| t.private && !t.loading && t.title == "private-isolated")
    })
    .await;
    assert!(state.saved().tabs.iter().all(|t| !t.private));
    let private = state.tabs.iter().find(|t| t.private).unwrap().id.clone();
    agent_smoke(&app, &base, &first, &private).await;
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Navigate {
            id: first.clone(),
            input: format!("{base}/open-popup"),
        },
    )
    .await
    .unwrap();
    let state = wait(&app, |s| {
        s.tabs.iter().any(|t| t.title == "popup-has-opener")
    })
    .await;
    let popup = state
        .tabs
        .iter()
        .find(|t| t.title == "popup-has-opener")
        .unwrap()
        .id
        .clone();
    println!("Checking popup close...");
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Navigate {
            id: popup.clone(),
            input: format!("{base}/close"),
        },
    )
    .await
    .unwrap();
    wait(&app, |s| s.tabs.iter().all(|t| t.id != popup)).await;
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Navigate {
            id: first.clone(),
            input: format!("{base}/open-blank"),
        },
    )
    .await
    .unwrap();
    let state = wait(&app, |s| s.tabs.iter().any(|t| t.title == "blank-popup")).await;
    let blank = state
        .tabs
        .iter()
        .find(|t| t.title == "blank-popup")
        .unwrap();
    assert_eq!(blank.url, "about:blank");
    assert!(
        !blank.start,
        "script-created about:blank pages must use the native surface"
    );
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Close {
            id: blank.id.clone(),
        },
    )
    .await
    .unwrap();
    println!("Checking find and site data cleanup...");
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Select { id: first.clone() },
    )
    .await
    .unwrap();
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Navigate {
            id: first.clone(),
            input: format!("{base}/two"),
        },
    )
    .await
    .unwrap();
    wait(&app, |s| {
        s.tabs
            .iter()
            .any(|t| t.id == first && t.title == "storage-retained")
    })
    .await;
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Action {
            id: first.clone(),
            name: "find".into(),
            text: "two".into(),
        },
    )
    .await
    .unwrap();
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Action {
            id: first.clone(),
            name: "find_stop".into(),
            text: String::new(),
        },
    )
    .await
    .unwrap();
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Action {
            id: first.clone(),
            name: "clear_data".into(),
            text: String::new(),
        },
    )
    .await
    .unwrap();
    wait(&app, |s| {
        s.tabs
            .iter()
            .any(|t| t.id == first && t.title == "private-isolated")
    })
    .await;
    let extension = utils::path::get_user_data_dir().join("fixture-extension");
    println!("Checking unpacked extension lifecycle...");
    std::fs::create_dir_all(&extension).unwrap();
    std::fs::write(extension.join("manifest.json"), r#"{"manifest_version":3,"name":"Vivian smoke extension","version":"1.0","content_scripts":[{"matches":["http://127.0.0.1/*"],"js":["content.js"]}]}"#).unwrap();
    std::fs::write(
        extension.join("content.js"),
        "if(location.pathname==='/extension')document.title='extension-active'",
    )
    .unwrap();
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Extension {
            id: String::new(),
            name: "install".into(),
            path: extension.to_string_lossy().into(),
        },
    )
    .await
    .unwrap();
    let state = wait(&app, |s| {
        s.extensions
            .iter()
            .any(|ext| ext.name == "Vivian smoke extension")
    })
    .await;
    let extension_id = state
        .extensions
        .iter()
        .find(|ext| ext.name == "Vivian smoke extension")
        .unwrap()
        .id
        .clone();
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Navigate {
            id: first.clone(),
            input: format!("{base}/extension"),
        },
    )
    .await
    .unwrap();
    wait(&app, |s| {
        s.tabs
            .iter()
            .any(|t| t.id == first && t.title == "extension-active")
    })
    .await;
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Extension {
            id: extension_id.clone(),
            name: "disable".into(),
            path: String::new(),
        },
    )
    .await
    .unwrap();
    wait(&app, |s| {
        s.extensions
            .iter()
            .any(|ext| ext.id == extension_id && !ext.enabled)
    })
    .await;
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Extension {
            id: extension_id.clone(),
            name: "remove".into(),
            path: String::new(),
        },
    )
    .await
    .unwrap();
    wait(&app, |s| {
        s.extensions.iter().all(|ext| ext.id != extension_id)
    })
    .await;
    agent_lifecycle(&app, &base, &first).await;
    native::layout(
        app.clone(),
        "memory".into(),
        BrowserBounds::default(),
        false,
        100,
    )
    .await
    .unwrap();
    native::layout(
        app.clone(),
        "memory".into(),
        BrowserBounds {
            width: 500.0,
            height: 300.0,
            ..Default::default()
        },
        true,
        99,
    )
    .await
    .unwrap();
    backup_roundtrip(&app).await;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        native::destroy("memory");
        let _ = tx.send(());
    })
    .unwrap();
    rx.await.unwrap();
    println!("PASS: real WebView2 navigation, history, cookies/storage, private isolation, bookmarks, zoom, popup opener/close, native find, clear site data, extensions and ordered layout.");
    EXIT_CODE.store(0, std::sync::atomic::Ordering::Relaxed);
    app.exit(0);
}

async fn backup_roundtrip(app: &tauri::AppHandle) {
    println!("Checking backup flush, browser shutdown, admission gate and profile restoration...");
    let state = native::request(app.clone(), "memory".into(), BrowserRequest::Init).await.unwrap();
    let expected = state.saved();
    let pids = native::shutdown_for_backup(app.clone()).await.unwrap();
    assert!(!pids.is_empty());
    let root = crate::utils::path::get_user_data_dir();
    let saved: BrowserSaved = serde_json::from_slice(&std::fs::read(root.join("browser/state.json")).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&saved).unwrap(), serde_json::to_value(&expected).unwrap());
    std::fs::write(root.join(".backup.pending.json"), "test gate").unwrap();
    assert!(native::request(app.clone(), "memory".into(), BrowserRequest::Init).await.is_err());
    std::fs::remove_file(root.join(".backup.pending.json")).unwrap();
    let restored = wait(app, |s| !s.tabs.is_empty() && s.tabs.iter().all(|t| !t.loading)).await;
    assert_eq!(serde_json::to_value(restored.bookmarks).unwrap(), serde_json::to_value(expected.bookmarks).unwrap());
    assert_eq!(restored.tabs.iter().map(|t| &t.url).collect::<Vec<_>>(), expected.tabs.iter().map(|t| &t.url).collect::<Vec<_>>());
}

/// Kill only the isolated embedded process to exercise real WebView2 invalidation.
async fn recovery_smoke(app: &tauri::AppHandle, base: &str, first: &str) {
    use std::os::windows::process::CommandExt;
    use serde_json::json;
    println!("Checking engine crash recovery without erasing profile data...");
    native::request(app.clone(), "memory".into(), BrowserRequest::Bookmark { id: first.into() }).await.unwrap();
    native::request(app.clone(), "memory".into(), BrowserRequest::Navigate {
        id: first.into(), input: format!("{base}/account"),
    }).await.unwrap();
    wait(app, |s| s.tabs.iter().any(|t| t.id == first && !t.loading && t.title == "fixture account")).await;
    let text = browser_tool(app, "browser_snapshot", json!({"tab_id":first}), 10000).await.unwrap();
    assert!(text.contains("Logged in as fixture-user"));
    // Close once normally so this assertion tests an already persisted profile, not pending disk writes.
    backup_roundtrip(app).await;
    let text = browser_tool(app, "browser_snapshot", json!({"tab_id":first}), 10000).await.unwrap();
    assert!(text.contains("Logged in as fixture-user"));
    let pid = native::automation::engine_process_id(app).await.unwrap().unwrap();
    assert_ne!(pid, std::process::id());
    assert!(std::process::Command::new("taskkill.exe").args(["/PID", &pid.to_string(), "/F"])
        .creation_flags(0x08000000).status().unwrap().success());
    let mut restored = None;
    for _ in 0..600 {
        if let Ok(state) = native::request(app.clone(), "memory".into(), BrowserRequest::Init).await {
            if state.error.is_none() && state.tabs.iter().any(|t| t.id == first && !t.loading && !t.title.is_empty())
                && native::automation::engine_process_id(app).await.ok().flatten().is_some_and(|new| new != pid) {
                restored = Some(state); break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let state = restored.expect("engine must automatically recover within 30 seconds");
    assert!(state.bookmarks.iter().any(|entry| entry.url == format!("{base}/one")));
    let error = browser_tool(app, "browser_click", json!({"tab_id":first,"index":0}), 10000).await.unwrap_err();
    assert!(error.contains("快照"), "old element selections survived engine recovery: {error}");
    let text = browser_tool(app, "browser_snapshot", json!({"tab_id":first}), 10000).await.unwrap();
    assert!(text.contains("Logged in as fixture-user"), "persistent login cookie lost during recovery");
    let bridge = BROWSER_SERVICE.get().unwrap();
    bridge.refresh_credentials().await;
    assert!(bridge.is_connected());
    assert!(bridge.error().is_none());
    assert!(bridge.credential_error().is_none());
    assert_eq!(bridge.platform_status().unwrap().1.len(), 10);
    // A private-only browser remains usable, but cannot supply normal-profile credentials.
    native::request(app.clone(), "memory".into(), BrowserRequest::NewTab {
        input: format!("{base}/account"), private: true,
    }).await.unwrap();
    wait(app, |s| s.tabs.iter().any(|t| t.private && !t.loading && t.title == "login required")).await;
    native::request(app.clone(), "memory".into(), BrowserRequest::Close { id: first.into() }).await.unwrap();
    bridge.refresh_credentials().await;
    assert!(bridge.is_connected(), "private-only browser must retain engine readiness");
    assert!(bridge.credential_error().is_some());
    let text = browser_tool(app, "browser_task_tab", json!({"url":format!("{base}/account"),"waitMs":0}), 10000).await.unwrap();
    assert!(text.contains("Logged in as fixture-user"));
    println!("PASS: browser crash recovery, retained tabs/bookmarks/login profile, revoked element selections, private-only engine readiness and browser_bridge credentials.");
}

async fn browser_tool(
    app: &tauri::AppHandle,
    name: &str,
    args: serde_json::Value,
    timeout_ms: u64,
) -> Result<String, String> {
    let _ = app;
    BROWSER_SERVICE
        .get()
        .unwrap()
        .request_tool(
            name,
            &args,
            std::time::Duration::from_millis(timeout_ms),
            Some("smoke-agent"),
        )
        .await
}
async fn agent_smoke(app: &tauri::AppHandle, base: &str, first: &str, private: &str) {
    use serde_json::json;
    println!("Checking authenticated native agent reads, isolated scripts and target affinity...");
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Navigate {
            id: first.into(),
            input: format!("{base}/account"),
        },
    )
    .await
    .unwrap();
    wait(app, |s| {
        s.tabs
            .iter()
            .any(|t| t.id == first && !t.loading && t.title == "fixture account")
    })
    .await;
    let snapshot = browser_tool(app, "browser_snapshot", json!({"tab_id":first}), 10000)
        .await
        .unwrap();
    assert!(snapshot.contains("Logged in as fixture-user"));
    for secret in [
        "synthetic-session",
        "synthetic-password",
        "synthetic-api-key",
    ] {
        assert!(
            !snapshot.contains(secret),
            "snapshot exposed a sensitive field"
        );
    }
    let page_text = browser_tool(app, "browser_get_text", json!({}), 10000)
        .await
        .unwrap();
    assert!(page_text.contains("Logged in as fixture-user"));
    assert!(!page_text.contains("synthetic-api-key"));
    let credentials = native::automation::credentials(app.clone())
        .await
        .unwrap()
        .expect("browser cookie access is available in the smoke harness");
    assert_eq!(credentials.status.len(), 10);
    assert!(credentials.status.values().all(|logged_in| !*logged_in));
    assert!(credentials.x.is_none() && credentials.reddit.is_none());
    let index = snapshot
        .lines()
        .find(|line| line.contains("input \"Comment\""))
        .and_then(|line| line.split('[').nth(1))
        .and_then(|part| part.split(']').next())
        .unwrap()
        .parse::<u64>()
        .unwrap();
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Select { id: private.into() },
    )
    .await
    .unwrap();
    browser_tool(
        app,
        "browser_type",
        json!({"index":index,"text":"native agent typed","replace":true}),
        10000,
    )
    .await
    .unwrap();
    assert!(
        browser_tool(app, "browser_get_text", json!({"selector":"#typed"}), 10000)
            .await
            .unwrap()
            .contains("native agent typed")
    );
    browser_tool(app, "browser_press", json!({"key":"Enter"}), 10000)
        .await
        .unwrap();
    let pressed = browser_tool(app, "browser_get_text", json!({"selector":"#key"}), 10000)
        .await
        .unwrap();
    if !pressed.contains("native-enter") {
        browser_tool(app, "browser_wait", json!({"ms":100}), 10000)
            .await
            .unwrap();
        let after_wait = browser_tool(app, "browser_get_text", json!({}), 10000)
            .await
            .unwrap();
        panic!("Native key was not observed immediately: {pressed}; after waiting: {after_wait}");
    }
    browser_tool(app, "browser_press", json!({"key":"Backspace"}), 10000)
        .await
        .unwrap();
    assert!(
        browser_tool(app, "browser_get_text", json!({"selector":"#typed"}), 10000)
            .await
            .unwrap()
            .contains("native agent type\n")
    );
    assert_eq!(
        native::request(app.clone(), "memory".into(), BrowserRequest::Init)
            .await
            .unwrap()
            .active
            .as_deref(),
        Some(private)
    );
    let before = native::request(app.clone(), "memory".into(), BrowserRequest::Init)
        .await
        .unwrap();
    let background = browser_tool(
        app,
        "browser_task_tab",
        json!({"url":format!("{base}/account"),"waitMs":0}),
        10000,
    )
    .await
    .unwrap();
    assert!(background.contains("Logged in as fixture-user"));
    let after = wait(app, |s| s.background_tasks == 0).await;
    assert_eq!(before.active, after.active);
    assert_eq!(before.tabs.len(), after.tabs.len());
    assert_eq!(before.history.len(), after.history.len());
    assert!(browser_tool(
        app,
        "browser_task_tab",
        json!({"url":format!("{base}/account"),"waitMs":10000}),
        250
    )
    .await
    .is_err());
    wait(app, |s| s.background_tasks == 0).await;
    let handle = app.clone();
    let url = format!("{base}/account");
    let cancelled = tauri::async_runtime::spawn(async move {
        browser_tool(
            &handle,
            "browser_task_tab",
            json!({"url":url,"waitMs":10000}),
            15000,
        )
        .await
    });
    wait(app, |s| s.background_tasks == 1).await;
    cancelled.abort();
    wait(app, |s| s.background_tasks == 0).await;
    for invalid in [
        json!({"url":"javascript:alert(1)"}),
        json!({"url":"https://user:pass@example.com"}),
        json!({"url":format!("{base}/account"),"code":"document.cookie"}),
        json!({"url":format!("{base}/account"),"waitMs":-1}),
    ] {
        assert!(browser_tool(app, "browser_task_tab", invalid, 10000)
            .await
            .is_err());
    }
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Navigate {
            id: first.into(),
            input: format!("{base}/one"),
        },
    )
    .await
    .unwrap();
    wait(app, |s| {
        s.tabs
            .iter()
            .any(|t| t.id == first && !t.loading && t.title == "one")
    })
    .await;
    assert!(browser_tool(
        app,
        "browser_type",
        json!({"index":index,"text":"stale action"}),
        10000
    )
    .await
    .unwrap_err()
    .contains("snapshot"));
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Select { id: first.into() },
    )
    .await
    .unwrap();
}
async fn agent_lifecycle(app: &tauri::AppHandle, base: &str, first: &str) {
    use tauri::Manager;
    println!("Checking browser session survives inspector closure...");
    native::request(
        app.clone(),
        "memory".into(),
        BrowserRequest::Navigate {
            id: first.into(),
            input: format!("{base}/one"),
        },
    )
    .await
    .unwrap();
    wait(app, |s| {
        s.tabs
            .iter()
            .any(|t| t.id == first && !t.loading && t.title == "one")
    })
    .await;
    let handle = app.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        tauri::WebviewWindowBuilder::new(
            &handle,
            "keepalive",
            tauri::WebviewUrl::External("about:blank".parse().unwrap()),
        )
        .visible(false)
        .build()
        .unwrap();
        handle
            .get_webview_window("memory")
            .unwrap()
            .close()
            .unwrap();
        let _ = tx.send(());
    })
    .unwrap();
    rx.await.unwrap();
    let text = browser_tool(
        app,
        "browser_task_tab",
        serde_json::json!({"url":format!("{base}/account"),"waitMs":0}),
        10000,
    )
    .await
    .unwrap();
    assert!(text.contains("Logged in as fixture-user"));
    assert!(
        native::request(app.clone(), "memory".into(), BrowserRequest::Init)
            .await
            .unwrap()
            .tabs
            .iter()
            .all(|t| !t.private)
    );
    let handle = app.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        tauri::WebviewWindowBuilder::new(
            &handle,
            "memory",
            tauri::WebviewUrl::External("about:blank".parse().unwrap()),
        )
        .visible(false)
        .build()
        .unwrap();
        let _ = tx.send(());
    })
    .unwrap();
    rx.await.unwrap();
    native::layout(
        app.clone(),
        "memory".into(),
        BrowserBounds {
            width: 800.0,
            height: 500.0,
            ..Default::default()
        },
        true,
        1000,
    )
    .await
    .unwrap();
    assert!(browser_tool(
        app,
        "browser_snapshot",
        serde_json::json!({"tab_id":first}),
        10000
    )
    .await
    .unwrap()
    .contains("one"));
}
async fn playwright_smoke(app:&tauri::AppHandle,base:&str){
    use serde_json::{json,Value};
    wait(app,|s|s.tabs.first().is_some_and(|t|!t.loading)).await;
    let created=browser_tool(app,"browser_open_tab",json!({"url":format!("{base}/computer")}),15000).await.unwrap();
    let created:Value=serde_json::from_str(&created).unwrap();let tab=created["tab_id"].as_str().unwrap();
    wait(app,|s|s.tabs.iter().any(|t|t.id==tab && !t.loading && t.title=="computer fixture")).await;
    let other:Value=serde_json::from_str(&browser_tool(app,"browser_open_tab",json!({"url":format!("{base}/one")}),15000).await.unwrap()).unwrap();
    let other=other["tab_id"].as_str().unwrap();
    let code=r#"page.setDefaultTimeout(5000);let progress=1; const save=page.locator('#batch-dialog').getByRole('button',{name:'Save',exact:true});
await page.getByLabel('Text entry',{exact:true}).fill('原生 Playwright 😀');
await expect(page.locator('#echo')).toHaveText('原生 Playwright 😀');
await save.click(); await expect(page.locator('#saved')).toHaveText('Saved',{timeout:3000});
({progress, count:await page.locator('#save-count').innerText(), snapshot:await page.ariaSnapshot({mode:'ai'})})"#;
    let request=json!({"task_id":"native","tab_id":tab,"request_id":"first","code":code,"wait_ms":10000});
    let mut receipt:Value=serde_json::from_str(&browser_tool(app,"browser_execute",request.clone(),30000).await.unwrap()).unwrap();
    for _ in 0..8{if !matches!(receipt["status"].as_str(),Some("queued"|"running")){break;}receipt=serde_json::from_str(&browser_tool(app,"browser_receipt",json!({"task_id":"native","request_id":"first","wait_ms":10000}),15000).await.unwrap()).unwrap();}
    assert_eq!(receipt["status"],"succeeded","{receipt}");
    assert_eq!(receipt["value"]["count"],"saved:1");
    let duplicate:Value=serde_json::from_str(&browser_tool(app,"browser_execute",request,15000).await.unwrap()).unwrap();assert_eq!(duplicate["value"],receipt["value"]);
    let code=r#"progress++; let duplicateRejected=false; try {await page.getByRole('button',{name:'Save',exact:true}).click({timeout:150});} catch(e){duplicateRejected=e.message.includes('strict mode');}
let coveredRejected=false;try{await page.getByRole('button',{name:'Covered click'}).click({timeout:200});}catch(e){coveredRejected=true;}
await page.screenshot(); ({progress,duplicateRejected,coveredRejected,count:await page.locator('#save-count').innerText()})"#;
    let receipt:Value=serde_json::from_str(&browser_tool(app,"browser_execute",json!({"task_id":"native","request_id":"second","code":code,"wait_ms":10000}),15000).await.unwrap()).unwrap();
    assert_eq!(receipt["status"],"succeeded","{receipt}");assert_eq!(receipt["value"]["progress"],2);assert_eq!(receipt["value"]["duplicateRejected"],true);assert_eq!(receipt["value"]["coveredRejected"],true);
    assert!(receipt["_images"].as_array().is_some_and(|images|!images.is_empty()),"real screenshot pixels: {receipt}");
    let code=std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("playwright-program.js")).unwrap();
    let mut third:Value=serde_json::from_str(&browser_tool(app,"browser_execute",json!({"task_id":"native","request_id":"frames-popup","code":code,"wait_ms":10000}),20000).await.unwrap()).unwrap();
    for _ in 0..5{if !matches!(third["status"].as_str(),Some("queued"|"running")){break;}third=serde_json::from_str(&browser_tool(app,"browser_receipt",json!({"task_id":"native","request_id":"frames-popup","wait_ms":10000}),15000).await.unwrap()).unwrap();}
    third.as_object_mut().unwrap().remove("_images");
    assert_eq!(third["status"],"succeeded","{third}");assert_eq!(third["value"]["frames"],3);assert_eq!(third["value"]["pages"],2);
    let state=native::request(app.clone(),"memory".into(),BrowserRequest::Init).await.unwrap();assert_eq!(state.active.as_deref(),Some(other),"automation and popups must not select the user's tab");
    let rejected:Value=serde_json::from_str(&browser_tool(app,"browser_execute",json!({"task_id":"native","request_id":"rejection","code":"await Promise.reject(new Error('receipt-regression'));","wait_ms":10000}),15000).await.unwrap()).unwrap();
    assert_eq!(rejected["status"],"failed","{rejected}");
    assert!(rejected["error"].as_str().unwrap().contains("receipt-regression"));
    let recovered:Value=serde_json::from_str(&browser_tool(app,"browser_execute",json!({"task_id":"native","request_id":"recovered","code":"progress","wait_ms":10000}),15000).await.unwrap()).unwrap();
    assert_eq!(recovered["status"],"succeeded","{recovered}");assert_eq!(recovered["value"],2);
    let timeout=json!({"task_id":"native","request_id":"cancel","code":"await save.click(); await new Promise(()=>{});","timeout_ms":1000,"wait_ms":0});
    browser_tool(app,"browser_execute",timeout.clone(),15000).await.unwrap();
    let cancelled:Value=serde_json::from_str(&browser_tool(app,"browser_receipt",json!({"task_id":"native","request_id":"cancel","wait_ms":5000}),10000).await.unwrap()).unwrap();
    assert_eq!(cancelled["status"],"interrupted","{cancelled}");
    let repeated:Value=serde_json::from_str(&browser_tool(app,"browser_execute",timeout,10000).await.unwrap()).unwrap();assert_eq!(repeated["status"],"interrupted","never re-execute a timed-out mutation");
    assert!(native::playwright::query(json!({"task_id":"native","request_id":"first"}),"another-agent-session".into()).await.is_err());
    println!("PASS: native same/cross-origin frames, inherited popups, newPage/close, unchanged selected tab, runtime cancellation and retained same-ID receipts");
    println!("PASS: ORIGINAL Playwright Locator/fill/click/expect/ariaSnapshot/screenshot over private stdio; persistent JS, scoped strict matching, coverage checks and request deduplication");
}
async fn computer_smoke(app:&tauri::AppHandle,base:&str){
    use serde_json::json;
    wait(app,|s|s.tabs.first().is_some_and(|t|!t.loading)).await;
    let created=browser_tool(app,"browser_open_tab",json!({"url":format!("{base}/computer")}),15000).await.unwrap();
    let created:serde_json::Value=serde_json::from_str(&created).unwrap();
    let id=created["tab_id"].as_str().unwrap();
    wait(app,|s|s.tabs.iter().any(|t|t.id==id && !t.loading && t.title=="computer fixture")).await;
    let tabs=browser_tool(app,"browser_tabs",json!({}),10000).await.unwrap();assert!(tabs.contains(id));
    let snapshot=browser_tool(app,"browser_snapshot",json!({"tab_id":id}),10000).await.unwrap();
    let index=|label:&str|snapshot.lines().find(|line|line.contains(label) && line.trim_start().starts_with('[')).and_then(|line|line.split('[').nth(1)).and_then(|s|s.split(']').next()).unwrap_or_else(||panic!("Missing {label}: {snapshot}")).parse::<u64>().unwrap();
    let select=index("Choice");let hover=index("Hover target");
    let receipt=browser_tool(app,"browser_select_option",json!({"tab_id":id,"index":select,"value":"b"}),10000).await.unwrap();
    let receipt:serde_json::Value=serde_json::from_str(&receipt).unwrap();
    assert_eq!(receipt["operation_completed"],true);assert_eq!(receipt["observation"]["success"],true);
    assert!(browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#chosen"}),10000).await.unwrap().contains("selected-b"));
    let hovered=browser_tool(app,"browser_hover",json!({"tab_id":id,"index":hover}),10000).await.unwrap();
    assert!(hovered.contains("operation_completed"));
    assert!(browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#hovered"}),10000).await.unwrap().contains("hover-confirmed"));
    // Exercise the real renderer; DOM dispatchEvent/value assignment cannot pass
    // these isTrusted and rich-editor structure assertions.
    let find = |label:&str|snapshot.lines().find(|line|line.contains(label) && line.trim_start().starts_with('[')).and_then(|line|line.split('[').nth(1)).and_then(|s|s.split(']').next()).unwrap_or_else(||panic!("Missing {label}: {snapshot}")).parse::<u64>().unwrap();
    let click=find("Trusted click");
    browser_tool(app,"browser_click",json!({"tab_id":id,"index":click}),10000).await.unwrap();
    let read = browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#clicked"}),10000).await.unwrap();
    assert!(read.contains("down:true up:true click:true count:1"),"{read}");
    browser_tool(app,"browser_click",json!({"tab_id":id,"index":click,"click_count":2}),10000).await.unwrap();
    assert!(browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#doubled"}),10000).await.unwrap().contains("double:true"));
    browser_tool(app,"browser_click",json!({"tab_id":id,"index":click,"button":"right"}),10000).await.unwrap();
    assert!(browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#context"}),10000).await.unwrap().contains("context:true"));
    let editor=find("Rich editor");
    browser_tool(app,"browser_type",json!({"tab_id":id,"index":editor,"text":"中文😀"}),10000).await.unwrap();
    assert!(browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#rich"}),10000).await.unwrap().contains("prefix中文😀"));
    assert!(browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#bold"}),10000).await.unwrap().contains("prefix"),"append preserves existing rich markup");
    assert!(browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#input-event"}),10000).await.unwrap().contains("input:true"));
    browser_tool(app,"browser_type",json!({"tab_id":id,"index":editor,"text":"replacement","replace":true}),10000).await.unwrap();
    browser_tool(app,"browser_press",json!({"tab_id":id,"key":"Control+A"}),10000).await.unwrap();
    browser_tool(app,"browser_press",json!({"tab_id":id,"key":"Backspace"}),10000).await.unwrap();
    let empty=browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#rich"}),10000).await.unwrap();
    assert!(!empty.contains("replacement") && !empty.contains("prefix"),"{empty}");
    let text=find("Text entry");
    browser_tool(app,"browser_type",json!({"tab_id":id,"index":text,"text":"Unicode中文😀","replace":true}),10000).await.unwrap();
    assert!(browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#echo"}),10000).await.unwrap().contains("Unicode中文😀"));
    browser_tool(app,"browser_type",json!({"tab_id":id,"index":text,"text":"","replace":true}),10000).await.unwrap();
    assert!(!browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#echo"}),10000).await.unwrap().contains("Unicode"));
    for label in ["Covered click","Disabled click","ARIA disabled"] {
        let error=browser_tool(app,"browser_click",json!({"tab_id":id,"index":find(label),"timeout_ms":100}),10000).await.unwrap_err();
        assert!(error.contains("covered") || error.contains("disabled"),"{label}: {error}");
    }
    assert!(browser_tool(app,"browser_type",json!({"tab_id":id,"index":find("Read only"),"text":"must not enter"}),10000).await.unwrap_err().contains("writable"));
    assert!(browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#blocked-count"}),10000).await.unwrap().contains("blocked:0"));
    browser_tool(app,"browser_click",json!({"tab_id":id,"index":find("Queue update")}),10000).await.unwrap();
    assert!(browser_tool(app,"browser_wait",json!({"tab_id":id,"selector":"#status","text":"ready","timeout_ms":2000}),10000).await.unwrap().contains("condition is satisfied"));
    browser_tool(app,"browser_wait",json!({"tab_id":id,"selector":"#spinner","state":"detached","timeout_ms":2000}),10000).await.unwrap();
    assert!(browser_tool(app,"browser_wait",json!({"tab_id":id,"selector":"#never","timeout_ms":50}),10000).await.unwrap_err().contains("timeout"));
    let batch=json!({"request_id":"save-once","tab_id":id,"steps":[
        {"action":"type","locator":{"selector":"#entry"},"args":{"text":"batch value","replace":true}},
        {"action":"click","locator":{"role":"button","name":"Save","scope":"#batch-dialog"}},
        {"action":"expect","args":{"selector":"#saved","text":"Saved","timeout_ms":2000}},
        {"action":"get_text","args":{"selector":"#saved"}}
    ]});
    let receipt=browser_tool(app,"browser_run",batch.clone(),15000).await.unwrap();
    let parsed:serde_json::Value=serde_json::from_str(&receipt).unwrap();
    assert_eq!(parsed["status"],"succeeded","{receipt}");
    assert_eq!(parsed["goal_verified"],false);
    assert_eq!(browser_tool(app,"browser_run",batch,15000).await.unwrap(),receipt,"same ID must not execute twice");
    assert_eq!(browser_tool(app,"browser_run_result",json!({"request_id":"save-once"}),10000).await.unwrap(),receipt);
    let count=browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#save-count"}),10000).await.unwrap();assert!(count.contains("saved:1"),"{count}");
    let ambiguous=browser_tool(app,"browser_run",json!({"request_id":"ambiguous","tab_id":id,"steps":[{"action":"click","locator":{"role":"button","name":"Save"}}]}),10000).await.unwrap();
    let ambiguous:serde_json::Value=serde_json::from_str(&ambiguous).unwrap();
    assert_eq!(ambiguous["status"],"failed");assert_eq!(ambiguous["mutation_state"],"none");
    let partial=json!({"request_id":"partial-save","tab_id":id,"steps":[
        {"action":"click","locator":{"selector":"#save"}},
        {"action":"expect","args":{"selector":"#saved","text":"Failed","timeout_ms":100}},
        {"action":"click","locator":{"selector":"#save"}}
    ]});
    let failed=browser_tool(app,"browser_run",partial.clone(),10000).await.unwrap();
    let parsed:serde_json::Value=serde_json::from_str(&failed).unwrap();
    assert_eq!(parsed["status"],"failed");assert_eq!(parsed["steps"].as_array().unwrap().len(),2);
    assert_eq!(browser_tool(app,"browser_run",partial,10000).await.unwrap(),failed);
    let count=browser_tool(app,"browser_get_text",json!({"tab_id":id,"selector":"#save-count"}),10000).await.unwrap();assert!(count.contains("saved:2"),"{count}");
    println!("PASS: strict scoped locators, batched actions with delayed assertions, retained receipt lookup, same-ID deduplication and stop after partial failure");
    println!("PASS: trusted pointer/click/double/right events; Unicode editing, rich markup, clear and shortcuts; blocked/disabled/readonly rejection; condition waits");
    let image=browser_tool(app,"browser_screenshot",json!({"tab_id":id}),10000).await.unwrap();
    let image:serde_json::Value=serde_json::from_str(&image).unwrap();assert!(image["image_png_base64"].as_str().unwrap().starts_with("iVBOR"));
    browser_tool(app,"browser_select_tab",json!({"tab_id":id}),10000).await.unwrap();
    assert!(browser_tool(app,"browser_hover",json!({"tab_id":id,"index":hover}),10000).await.is_err(),"tab selection requires a fresh snapshot");
    browser_tool(app,"browser_close_tab",json!({"tab_id":id}),10000).await.unwrap();
    assert!(!browser_tool(app,"browser_tabs",json!({}),10000).await.unwrap().contains(id));
    println!("PASS: native browser tabs/open/select/close, direct screenshot pixels, select-option, trusted hover, post-action observation and fresh-snapshot binding");
}
fn main() {
    let directory =
        std::env::temp_dir().join(format!("vivian-browser-smoke-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    std::env::set_var("VIVIAN_BROWSER_TEST_DIR", &directory);
    println!("Isolated browser data: {}", directory.display());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut buffer = [0; 8192];
            let n = stream.read(&mut buffer).unwrap();
            let input = String::from_utf8_lossy(&buffer[..n]);
            let path = input
                .lines()
                .next()
                .unwrap_or("")
                .split_whitespace()
                .nth(1)
                .unwrap_or("/");
            let body = match path {
            "/one" => "<title>one</title><script>localStorage.setItem('browser-smoke','yes');document.cookie='smoke=yes;path=/'</script>one",
            "/overlay" => "<title>overlay fixture</title><style>body{margin:0;padding:32px;font:16px system-ui;background:#f3f6fb;color:#23334d}h1{font-size:32px}p{color:#61738a}.cards{display:grid;grid-template-columns:repeat(3,1fr);gap:18px;margin-top:28px}.card{height:210px;padding:22px;border-radius:20px;background:#d9e7fa}.card:nth-child(2){background:#e8dff7}.card:nth-child(3){background:#f8ecc6}b{font-size:38px;display:block;margin:25px 0}footer{margin-top:28px;background:white;border-radius:14px;padding:24px}</style><h1>Native browser test page</h1><p>Isolated local fixture - no account or external website data</p><div class=cards><div class=card>Viewport<b>800 x 500</b>Keep the current page</div><div class=card>Browser overlays<b>Menu</b>Downloads and extensions</div><div class=card>Page session<b>Retained</b>No reload or navigation</div></div><footer>Visible page content remains behind the browser controls.</footer>",
            "/two" => "<script>function check(){document.title=localStorage.getItem('browser-smoke')==='yes'&&document.cookie.includes('smoke=yes')?'storage-retained':'private-isolated'}check();setInterval(check,100)</script>two",
            "/open-popup" => "<title>opener</title><script>setTimeout(()=>window.open('/popup','smoke-popup'),250)</script>",
            "/popup" => "<script>document.title=window.opener?'popup-has-opener':'popup-lost-opener'</script>",
            "/open-blank" => "<title>blank opener</title><script>setTimeout(()=>{const popup=window.open('','blank-popup');popup.document.write('<title>blank-popup</title>script-created page');popup.document.close()},250)</script>",
            "/close" => "<script>window.close()</script>",
            "/extension" => "<title>extension-base</title>",
            "/account" if input.contains("login-session=synthetic-session") => "<title>fixture account</title><main><p>Logged in as fixture-user</p><label for=comment>Comment</label><input id=comment oninput=\"document.getElementById('typed').textContent=this.value\" onkeydown=\"if(event.key==='Enter')document.getElementById('key').textContent='native-enter'\"><input type=password value=synthetic-password><textarea name=api_key>synthetic-api-key</textarea><p id=typed></p><p id=key></p></main><script>globalThis.vivianCreatePage=()=>()=>({text:'forged page result'});globalThis.vivianPageSessions=new Map()</script>",
            "/account" => "<title>login required</title><main>Please log in</main>",
            "/frame" => "<title>frame fixture</title><button onclick=\"document.getElementById('result').textContent='frame clicked'\">Frame action</button><p id=result></p>",
            "/computer" => r#"<title>computer fixture</title><main>
              <label for=choice>Choice</label><select id=choice onchange="document.getElementById('chosen').textContent='selected-'+this.value"><option value=a>A</option><option value=b>B</option></select>
              <button onpointerover="document.getElementById('hovered').textContent='hover-confirmed'">Hover target</button><p id=chosen></p><p id=hovered></p>
              <button id=trusted>Trusted click</button><p id=clicked></p><p id=doubled></p><p id=context></p>
              <div id=rich role=textbox aria-label="Rich editor" contenteditable=true><b id=bold>prefix</b></div><p id=input-event></p>
              <label for=entry>Text entry</label><input id=entry><p id=echo></p>
              <label for=readonly>Read only</label><input id=readonly readonly value=untouched>
              <div style="position:relative;width:160px;height:40px"><button style="width:100%;height:100%" id=covered>Covered click</button><div style="position:absolute;inset:0;background:#aaa"></div></div>
              <fieldset disabled><button id=disabled>Disabled click</button></fieldset><div aria-disabled=true><button id=aria>ARIA disabled</button></div><p id=blocked-count>blocked:0</p>
              <button id=queue>Queue update</button><p id=spinner>Loading</p><p id=status></p>
              <button>Save</button><div id=batch-dialog><button id=save>Save</button></div><p id=saved></p><p id=save-count>saved:0</p>
              </main><script>
              let down=false,up=false,count=0,blocked=0;
              trusted.onpointerdown=e=>down=e.isTrusted;trusted.onpointerup=e=>up=e.isTrusted;
              trusted.onclick=e=>{count++;document.getElementById('clicked').textContent=`down:${down} up:${up} click:${e.isTrusted} count:${count}`};
              trusted.ondblclick=e=>document.getElementById('doubled').textContent='double:'+e.isTrusted;
              trusted.oncontextmenu=e=>{e.preventDefault();document.getElementById('context').textContent='context:'+e.isTrusted};
              rich.oninput=e=>document.getElementById('input-event').textContent='input:'+e.isTrusted;
              entry.oninput=e=>document.getElementById('echo').textContent=e.target.value;
              for(const id of ['covered','disabled','aria'])document.getElementById(id).onclick=()=>document.getElementById('blocked-count').textContent='blocked:'+ ++blocked;
              queue.onclick=()=>setTimeout(()=>{document.getElementById('status').textContent='ready';document.getElementById('spinner').remove()},250);
              let saves=0;document.getElementById('save').onclick=()=>{document.getElementById('save-count').textContent='saved:'+ ++saves;document.getElementById('saved').textContent='Saving';setTimeout(()=>document.getElementById('saved').textContent='Saved',200)};
              </script>"#,
            _ => "<title>empty</title>empty",
        };
            // Exceed the snapshot label budget to catch redaction after truncation.
            let body = body.replace(
                "synthetic-api-key",
                &format!("synthetic-api-key-{}", "long-secret-part-".repeat(12)),
            );
            let body = body.replace(
                "if(event.key==='Enter')",
                "if(event.key==='Enter'&&event.isTrusted)",
            );
            let body = body.replace("<p id=key></p>", "<p id=key></p><p id=key-diagnostic></p><script>document.addEventListener('keydown',e=>document.getElementById('key-diagnostic').textContent='target='+e.target.id+' key='+e.key+' focus='+document.hasFocus());document.addEventListener('focusin',e=>document.getElementById('key-diagnostic').textContent='focused='+e.target.id)</script>");
            let cookie = if path == "/one" {
                "Set-Cookie: login-session=synthetic-session; HttpOnly; Path=/; SameSite=Lax; Max-Age=3600\r\n"
            } else {
                ""
            };
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n{cookie}Content-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            stream.write_all(response.as_bytes()).ok();
        }
    });
    tauri::Builder::default()
        .on_window_event(|window, event| {
            if matches!(
                event,
                tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
            ) {
                embedded_browser::window_destroyed(window.label());
            }
        })
        .setup(move |app| {
            let service = native_browser_service::BridgeState::new();
            service.start(app.handle().clone());
            let _ = BROWSER_SERVICE.set(service);
            tauri::WebviewWindowBuilder::new(
                app,
                "memory",
                tauri::WebviewUrl::External("about:blank".parse().unwrap()),
            )
            .visible(false)
            .inner_size(1000.0, 800.0)
            .build()?;
            let handle = app.handle().clone();
            let exit = handle.clone();
            tauri::async_runtime::spawn(async move {
                let task = tauri::async_runtime::spawn(smoke(handle, base));
                if let Err(error) = task.await {
                    eprintln!("FAIL: {error}");
                    exit.exit(1);
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("smoke application")
        .run_return(|_, _| {});
    std::process::exit(EXIT_CODE.load(std::sync::atomic::Ordering::Relaxed));
}
