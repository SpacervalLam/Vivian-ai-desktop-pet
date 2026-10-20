//! All COM objects live on Tauri's STA UI thread. No Tauri script is injected into sites.
use super::model::*;
use std::result::Result;
use std::{cell::RefCell, collections::HashMap, rc::Rc};
use tauri::{Emitter, Manager};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, *};
pub(crate) mod automation;
use windows::{
    core::{Interface, BOOL, PCWSTR, PWSTR},
    Win32::{
        Foundation::{HWND, RECT},
        Graphics::Gdi::{CreateRectRgn, CreateRoundRectRgn, CombineRgn, DeleteObject, SetWindowRgn, RGN_DIFF, RGN_AND},
        System::Com::CoTaskMemFree,
        UI::{
            Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_SHIFT},
            WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, IsWindow, SetParent, SetWindowPos, ShowWindow, SW_HIDE, SW_SHOWNA, WINDOW_EX_STYLE, WS_POPUP, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, SWP_NOACTIVATE, SWP_NOZORDER,
            },
        },
    },
};

type HostRef = Rc<RefCell<Host>>;
thread_local! { static HOSTS: RefCell<HashMap<String, HostRef>> = RefCell::new(HashMap::new()); }
struct NativeTab {
    controller: ICoreWebView2Controller,
    core: ICoreWebView2,
    find_term: String,
    fullscreen: bool,
    generation: u64,
    world: Option<i64>,
    input_parked: bool,
}
struct Permission {
    view: BrowserPermission,
    args: ICoreWebView2PermissionRequestedEventArgs,
    deferral: ICoreWebView2Deferral,
}
struct Host {
    app: tauri::AppHandle,
    owner: String,
    hwnd: HWND,
    parked_hwnd: HWND,
    surface_hwnd: HWND,
    clip: Option<BrowserClip>,
    state: BrowserSnapshot,
    environment: Option<ICoreWebView2Environment>,
    tabs: HashMap<String, NativeTab>,
    tasks: HashMap<String, BrowserTab>,
    selections: HashMap<String, (String, u64)>,
    cookies: Option<ICoreWebView2CookieManager>,
    pending: HashMap<String, Permission>,
    downloads: HashMap<String, ICoreWebView2DownloadOperation>,
    extensions: HashMap<String, ICoreWebView2BrowserExtension>,
    bounds: BrowserBounds,
    visible: bool,
    closed: Vec<BrowserTab>,
    last_layout: u64,
    scrollbar_css: String,
}
impl Host {
    fn tab(&self, id: &str) -> Option<&BrowserTab> {
        self.state
            .tabs
            .iter()
            .find(|t| t.id == id)
            .or_else(|| self.tasks.get(id))
    }
    fn tab_mut(&mut self, id: &str) -> Option<&mut BrowserTab> {
        self.state
            .tabs
            .iter_mut()
            .find(|t| t.id == id)
            .or_else(|| self.tasks.get_mut(id))
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        unsafe {
            for (_, permission) in self.pending.drain() {
                let _ = permission.args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY);
                let _ = permission.deferral.Complete();
            }
            for (_, tab) in self.tabs.drain() {
                let _ = tab.controller.Close();
            }
            let _ = DestroyWindow(self.surface_hwnd);
            let _ = DestroyWindow(self.parked_hwnd);
        }
    }
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
unsafe fn text(get: impl FnOnce(*mut PWSTR) -> windows::core::Result<()>) -> String {
    let mut value = PWSTR::null();
    if get(&mut value).is_err() || value.is_null() {
        return String::new();
    }
    let result = value.to_string().unwrap_or_default();
    CoTaskMemFree(Some(value.0.cast()));
    result
}
fn root() -> std::path::PathBuf {
    crate::utils::path::get_user_data_dir().join("browser")
}
fn backup_pending() -> bool {
    crate::utils::path::get_user_data_dir().join(".backup.pending.json").exists()
}
static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn publish(host: &HostRef, persist: bool) {
    let (app, owner, state) = {
        let mut h = host.borrow_mut();
        h.state.revision = REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        h.state.background_tasks = h.tasks.len();
        h.state.permissions = h.pending.values().map(|p| p.view.clone()).collect();
        (h.app.clone(), h.owner.clone(), h.state.clone())
    };
    if persist {
        let saved = state.saved();
        // UI owns the snapshot; the serialized write is queued in order off the STA thread.
        PERSIST_TX.send(BrowserPersist::Save(saved)).ok();
    }
    let _ = app.emit_to(&owner, "browser:state", &state);
}
enum BrowserPersist {
    Save(BrowserSaved),
    Flush(std::sync::mpsc::Sender<Result<(), String>>),
}
static PERSIST_TX: once_cell::sync::Lazy<std::sync::mpsc::Sender<BrowserPersist>> =
    once_cell::sync::Lazy::new(|| {
        let (tx, rx) = std::sync::mpsc::channel::<BrowserPersist>();
        std::thread::spawn(move || {
            let mut last = Ok(());
            while let Ok(message) = rx.recv() {
                match message {
                    BrowserPersist::Save(saved) => {
                        last = serde_json::to_string(&saved).map_err(|e| e.to_string())
                            .and_then(|body| crate::utils::fs::write_atomic(&root().join("state.json"), &body).map_err(|e| e.to_string()));
                        if let Err(error) = &last {
                            tracing::error!("Browser persistence: {error}");
                        }
                    }
                    BrowserPersist::Flush(ack) => {
                        let _ = ack.send(last.clone());
                    }
                }
            }
        });
        tx
    });

pub(crate) fn flush_persistence() -> Result<(), String> {
    let Some(sender) = once_cell::sync::Lazy::get(&PERSIST_TX) else { return Ok(()); };
    let (tx, rx) = std::sync::mpsc::channel();
    sender.send(BrowserPersist::Flush(tx)).map_err(|e| e.to_string())?;
    rx.recv_timeout(std::time::Duration::from_secs(5)).map_err(|e| e.to_string())?
}

pub(crate) async fn shutdown_for_backup(app: tauri::AppHandle) -> Result<Vec<u32>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let hosts: Vec<_> = HOSTS.with(|hosts| hosts.borrow_mut().drain().map(|(_, h)| h).collect());
        let mut pids = Vec::new();
        for host in hosts {
            let (saved, tabs) = {
                let mut h = host.borrow_mut();
                h.cookies = None;
                h.environment = None;
                h.extensions.clear();
                h.downloads.clear();
                (h.state.saved(), std::mem::take(&mut h.tabs))
            };
            let _ = PERSIST_TX.send(BrowserPersist::Save(saved));
            for (_, tab) in tabs {
                unsafe {
                    let mut pid = 0;
                    let _ = tab.core.BrowserProcessId(&mut pid);
                    if pid != 0 { pids.push(pid); }
                    let _ = tab.controller.Close();
                }
            }
        }
        pids.sort_unstable();
        pids.dedup();
        let _ = tx.send(flush_persistence().map(|_| pids));
    }).map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}

fn fail(host: &HostRef, error: impl std::fmt::Display) {
    host.borrow_mut().state.error = Some(error.to_string());
    publish(host, false);
}
fn create_surface(parent: HWND) -> windows::core::Result<HWND> {
    unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(wide("STATIC").as_ptr()),
        PCWSTR(wide("Vivian browser viewport").as_ptr()),
        WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
        0, 0, 1, 1, Some(parent), None, None, None) }
}

/// Window regions affect painting/hit testing, never the webpage's layout size.
unsafe fn clip_surface(surface: HWND, rect: RECT, scale: f64, clip: Option<&BrowserClip>) -> windows::core::Result<()> {
    let width = (rect.right - rect.left).max(0);
    let height = (rect.bottom - rect.top).max(0);
    let radius = clip.map_or(0, |c| (c.radius * scale).round() as i32);
    let region = if radius > 0 { CreateRoundRectRgn(0, 0, width + 1, height + 1, radius * 2, radius * 2) }
        else { CreateRectRgn(0, 0, width, height) };
    if region.is_invalid() { return Err(windows::core::Error::from_win32()); }
    if let Some(clip) = clip {
        for (index, bounds) in std::iter::once(&clip.viewport).chain(clip.overlays.iter()).enumerate() {
            let part = CreateRectRgn(
                if index == 0 { (bounds.x * scale).ceil() as i32 - rect.left } else { (bounds.x * scale).floor() as i32 - rect.left },
                if index == 0 { (bounds.y * scale).ceil() as i32 - rect.top } else { (bounds.y * scale).floor() as i32 - rect.top },
                if index == 0 { ((bounds.x + bounds.width) * scale).floor() as i32 - rect.left } else { ((bounds.x + bounds.width) * scale).ceil() as i32 - rect.left },
                if index == 0 { ((bounds.y + bounds.height) * scale).floor() as i32 - rect.top } else { ((bounds.y + bounds.height) * scale).ceil() as i32 - rect.top });
            if part.is_invalid() { let _ = DeleteObject(region.into()); return Err(windows::core::Error::from_win32()); }
            CombineRgn(Some(region), Some(region), Some(part), if index == 0 { RGN_AND } else { RGN_DIFF });
            let _ = DeleteObject(part.into());
        }
    }
    if SetWindowRgn(surface, Some(region), true) == 0 {
        let _ = DeleteObject(region.into());
        return Err(windows::core::Error::from_win32());
    }
    // Windows owns the region after SetWindowRgn succeeds.
    Ok(())
}

fn apply_bounds(host: &HostRef) -> windows::core::Result<()> {
    unsafe {
        let (rect, scale, surface, clip, controllers) = {
            let h = host.borrow();
            let scale = h
                .app
                .get_webview_window(&h.owner)
                .and_then(|w| w.scale_factor().ok())
                .unwrap_or(1.0);
            let scale = h.clip.as_ref().map_or(scale, |clip| clip.pixel_ratio);
            let b = h.bounds;
            let mut rect = RECT {
                left: (b.x * scale).round() as i32,
                top: (b.y * scale).round() as i32,
                right: ((b.x + b.width) * scale).round() as i32,
                bottom: ((b.y + b.height) * scale).round() as i32,
            };
            if b.width <= 0.0 || b.height <= 0.0 {
                rect = RECT {
                    left: 0,
                    top: 0,
                    right: 1280,
                    bottom: 900,
                };
            }
            let controllers = h
                .tabs
                .iter()
                .map(|(id, tab)| {
                    let blank = h
                        .state
                        .tabs
                        .iter()
                        .find(|t| &t.id == id)
                        .is_none_or(|t| t.start);
                    let active = h.state.active.as_ref() == Some(id);
                    (
                        tab.controller.clone(),
                        tab.input_parked
                            || (h.visible && active && !blank && b.width > 0.0 && b.height > 0.0),
                        active && !tab.input_parked,
                    )
                })
                .collect::<Vec<_>>();
            (rect, scale, h.surface_hwnd, h.clip.clone(), controllers)
        };
        let surface_visible = controllers.iter().any(|(_, visible, active)| *visible && *active);
        clip_surface(surface, rect, scale, clip.as_ref())?;
        SetWindowPos(surface, None, rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top, SWP_NOACTIVATE | SWP_NOZORDER)?;
        let _ = ShowWindow(surface, if surface_visible { SW_SHOWNA } else { SW_HIDE });
        let local = RECT { left: 0, top: 0, right: rect.right - rect.left, bottom: rect.bottom - rect.top };
        // COM calls can dispatch events synchronously; never retain a RefCell borrow across them.
        for (controller, visible, active) in controllers {
            controller.SetIsVisible(visible)?;
            if active {
                let mut previous = RECT::default();
                controller.Bounds(&mut previous)?;
                if previous != local { controller.SetBounds(local)?; }
                controller.NotifyParentWindowPositionChanged()?;
            }
        }
        Ok(())
    }
}
fn refresh(host: &HostRef, id: &str, history: bool) {
    unsafe {
        let core = host.borrow().tabs.get(id).map(|tab| tab.core.clone());
        let Some(core) = core else {
            return;
        };
        let url = text(|p| core.Source(p));
        let title = text(|p| core.DocumentTitle(p));
        let mut back = BOOL::default();
        let mut forward = BOOL::default();
        let _ = core.CanGoBack(&mut back);
        let _ = core.CanGoForward(&mut forward);
        {
            let mut h = host.borrow_mut();
            let task = h.tasks.contains_key(id);
            if let Some(tab) = h.tab_mut(id) {
                tab.url = url.clone();
                tab.start = (tab.start && url == "about:blank") || is_library_page(&url);
                tab.title = if title.is_empty() {
                    url.clone()
                } else {
                    title.clone()
                };
                tab.can_back = back.as_bool();
                tab.can_forward = forward.as_bool();
                if history && !task && !tab.private && url != "about:blank" && !is_library_page(&url) {
                    h.state.history.push(PageEntry {
                        url,
                        title,
                        at: chrono::Utc::now().timestamp_millis(),
                    });
                }
            }
        }
        let _ = apply_bounds(host);
        publish(host, history);
    }
}
fn initialize(app: tauri::AppHandle, owner: String) -> Result<HostRef, String> {
    if backup_pending() {return Err("浏览器正在保存以进行备份".into());}
    if let Some(host) = HOSTS.with(|hosts| hosts.borrow().get(&owner).cloned()) {
        return Ok(host);
    }
    let saved = if root().join("state.json").exists() {
        let body = std::fs::read(root().join("state.json")).map_err(|e| e.to_string())?;
        serde_json::from_slice::<BrowserSaved>(&body)
            .map_err(|e| format!("Browser session cannot be restored: {e}"))?
    } else {
        BrowserSaved::default()
    };
    let mut state = BrowserSnapshot {
        tabs: saved.tabs,
        active: saved.active,
        history: saved.history,
        bookmarks: saved.bookmarks,
        ..Default::default()
    };
    if state.tabs.is_empty() {
        let tab = BrowserTab::new("about:blank".into(), false);
        state.active = Some(tab.id.clone());
        state.tabs.push(tab);
    }
    for tab in &mut state.tabs {
        tab.loading = true;
        tab.error = None;
        tab.can_back = false;
        tab.can_forward = false;
    }
    // A private, invisible HWND keeps the profile and tabs alive when the inspector closes.
    let parked_hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(wide("STATIC").as_ptr()),
            PCWSTR(wide("Vivian browser engine").as_ptr()),
            WS_POPUP,
            0,
            0,
            800,
            600,
            None,
            None,
            None,
            None,
        )
    }
    .map_err(|e| e.to_string())?;
    let hwnd = app
        .get_webview_window(&owner)
        .and_then(|w| w.hwnd().ok())
        .unwrap_or(parked_hwnd);
    let surface_hwnd = create_surface(hwnd).map_err(|e| e.to_string())?;
    let host = Rc::new(RefCell::new(Host {
        app,
        owner: owner.clone(),
        hwnd,
        parked_hwnd,
        surface_hwnd,
        clip: None,
        state,
        environment: None,
        tabs: HashMap::new(),
        tasks: HashMap::new(),
        selections: HashMap::new(),
        cookies: None,
        pending: HashMap::new(),
        downloads: HashMap::new(),
        extensions: HashMap::new(),
        bounds: BrowserBounds::default(),
        visible: false,
        closed: vec![],
        last_layout: 0,
        scrollbar_css: String::new(),
    }));
    HOSTS.with(|hosts| hosts.borrow_mut().insert(owner, host.clone()));
    let weak = Rc::downgrade(&host);
    unsafe {
        let options: ICoreWebView2EnvironmentOptions =
            CoreWebView2EnvironmentOptions::default().into();
        options
            .cast::<ICoreWebView2EnvironmentOptions6>()
            .map_err(|e| e.to_string())?
            .SetAreBrowserExtensionsEnabled(true)
            .map_err(|e| e.to_string())?;
        let directory = wide(&root().join("engine").to_string_lossy());
        CreateCoreWebView2EnvironmentWithOptions(
            PCWSTR::null(),
            PCWSTR(directory.as_ptr()),
            &options,
            &CreateCoreWebView2EnvironmentCompletedHandler::create(Box::new(
                move |result, environment| {
                    let Some(host) = weak.upgrade() else {
                        return Ok(());
                    };
                    if let Err(error) = result {
                        fail(&host, error);
                        return Ok(());
                    }
                    let Some(environment) = environment else {
                        fail(&host, "WebView2 environment is unavailable");
                        return Ok(());
                    };
                    if backup_pending() {return Ok(());}
                    host.borrow_mut().environment = Some(environment);
                    let tabs = host.borrow().state.tabs.clone();
                    for tab in tabs {
                        if let Err(error) = create_tab(&host, tab, None) {
                            fail(&host, error);
                        }
                    }
                    Ok(())
                },
            )),
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(host)
}

fn create_tab(
    host: &HostRef,
    tab: BrowserTab,
    popup: Option<(
        ICoreWebView2NewWindowRequestedEventArgs,
        ICoreWebView2Deferral,
    )>,
) -> windows::core::Result<()> {
    unsafe {
        let (environment, hwnd) = {
            let h = host.borrow();
            (h.environment.clone(), h.parked_hwnd)
        };
        let Some(environment) = environment else {
            return Ok(());
        };
        let environment = environment.cast::<ICoreWebView2Environment10>()?;
        let options = environment.CreateCoreWebView2ControllerOptions()?;
        options.SetProfileName(PCWSTR(
            wide(if tab.private { "Private" } else { "Default" }).as_ptr(),
        ))?;
        options.SetIsInPrivateModeEnabled(tab.private)?;
        let weak = Rc::downgrade(host);
        let id = tab.id.clone();
        environment.CreateCoreWebView2ControllerWithOptions(
            hwnd,
            &options,
            &CreateCoreWebView2ControllerCompletedHandler::create(Box::new(
                move |result, controller| {
                    let Some(host) = weak.upgrade() else {
                        if let Some(c) = controller {
                            let _ = c.Close();
                        }
                        if let Some((_, d)) = &popup {
                            let _ = d.Complete();
                        }
                        return Ok(());
                    };
                    if let Err(error) = result {
                        fail(&host, error);
                        if let Some((_, d)) = &popup {
                            let _ = d.Complete();
                        }
                        return Ok(());
                    }
                    let Some(controller) = controller else {
                        return Ok(());
                    };
                    if host.borrow().tab(&id).is_none() {
                        let _ = controller.Close();
                        if let Some((_, d)) = &popup {
                            let _ = d.Complete();
                        }
                        return Ok(());
                    }
                    if backup_pending() {let _=controller.Close();return Ok(());}
                    let core = controller.CoreWebView2()?;
                    let parent = host.borrow().surface_hwnd;
                    controller.SetParentWindow(if host.borrow().tasks.contains_key(&id) {
                        host.borrow().parked_hwnd
                    } else {
                        parent
                    })?;
                    if !tab.private {
                        host.borrow_mut().cookies =
                            Some(core.cast::<ICoreWebView2_2>()?.CookieManager()?);
                    }
                    if host.borrow().tasks.contains_key(&id) {
                        core.cast::<ICoreWebView2_8>()?.SetIsMuted(true)?;
                        controller.SetBounds(RECT {
                            left: 0,
                            top: 0,
                            right: 1280,
                            bottom: 900,
                        })?;
                    }
                    let settings = core.Settings()?;
                    settings.SetIsScriptEnabled(true)?;
                    settings.SetAreDefaultContextMenusEnabled(true)?;
                    settings.SetAreDefaultScriptDialogsEnabled(true)?;
                    if host.borrow().tasks.contains_key(&id) {
                        settings.SetAreDefaultScriptDialogsEnabled(false)?;
                    }
                    settings.SetAreDevToolsEnabled(true)?;
                    settings
                        .cast::<ICoreWebView2Settings3>()?
                        .SetAreBrowserAcceleratorKeysEnabled(true)?;
                    if !tab.private {
                        if let Ok(settings) = settings.cast::<ICoreWebView2Settings4>() {
                            settings.SetIsPasswordAutosaveEnabled(true)?;
                            settings.SetIsGeneralAutofillEnabled(true)?;
                        }
                    }
                    controller.SetZoomFactor(tab.zoom)?;
                    host.borrow_mut().tabs.insert(
                        id.clone(),
                        NativeTab {
                            controller: controller.clone(),
                            core: core.clone(),
                            find_term: String::new(),
                            fullscreen: false,
                            generation: 0,
                            world: None,
                            input_parked: false,
                        },
                    );
                    wire(&host, &id, &core, &controller)?;
                    apply_bounds(&host)?;
                    if let Some((args, deferral)) = &popup {
                        args.SetNewWindow(&core)?;
                        args.SetHandled(true)?;
                        deferral.Complete()?;
                    } else {
                        core.Navigate(PCWSTR(wide(&tab.url).as_ptr()))?;
                    }
                    refresh(&host, &id, false);
                    if !host.borrow().tasks.contains_key(&id) {
                        extensions(&host, &core)?;
                    }
                    Ok(())
                },
            )),
        )
    }
}

unsafe fn wire(
    host: &HostRef,
    id: &str,
    core: &ICoreWebView2,
    controller: &ICoreWebView2Controller,
) -> windows::core::Result<()> {
    let weak = Rc::downgrade(host);
    let key = id.to_string();
    let mut token = 0;
    core.add_SourceChanged(
        &SourceChangedEventHandler::create(Box::new(move |_, _| {
            if let Some(h) = weak.upgrade() {
                refresh(&h, &key, false);
            }
            Ok(())
        })),
        &mut token,
    )?;
    let weak = Rc::downgrade(host);
    let key = id.to_string();
    core.add_DocumentTitleChanged(
        &DocumentTitleChangedEventHandler::create(Box::new(move |_, _| {
            if let Some(h) = weak.upgrade() {
                refresh(&h, &key, false);
            }
            Ok(())
        })),
        &mut token,
    )?;
    let weak = Rc::downgrade(host);
    let key = id.to_string();
    core.add_HistoryChanged(
        &HistoryChangedEventHandler::create(Box::new(move |_, _| {
            if let Some(h) = weak.upgrade() {
                refresh(&h, &key, false);
            }
            Ok(())
        })),
        &mut token,
    )?;
    let weak = Rc::downgrade(host);
    let key = id.to_string();
    core.add_NavigationStarting(
        &NavigationStartingEventHandler::create(Box::new(move |_, _| {
            if let Some(h) = weak.upgrade() {
                if let Some(t) = h.borrow_mut().tab_mut(&key) {
                    t.loading = true;
                    t.error = None;
                }
                if let Some(t) = h.borrow_mut().tabs.get_mut(&key) {
                    t.generation += 1;
                    t.world = None;
                }
                publish(&h, false);
            }
            Ok(())
        })),
        &mut token,
    )?;
    let weak = Rc::downgrade(host);
    let key = id.to_string();
    core.add_NavigationCompleted(
        &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
            if let Some(h) = weak.upgrade() {
                let mut success = BOOL::default();
                let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                if let Some(args) = args {
                    args.IsSuccess(&mut success)?;
                    args.WebErrorStatus(&mut status)?;
                }
                if let Some(t) = h.borrow_mut().tab_mut(&key) {
                    t.loading = false;
                    if !success.as_bool()
                        && status != COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED
                    {
                        t.error = Some(format!("WebView2 navigation error {}", status.0));
                    }
                }
                if let Some(tab) = h.borrow().tabs.get(&key) {
                    apply_scrollbar_style(&tab.core, &h.borrow().scrollbar_css);
                }
                refresh(&h, &key, success.as_bool());
            }
            Ok(())
        })),
        &mut token,
    )?;
    let weak = Rc::downgrade(host);
    let key = id.to_string();
    core.add_NewWindowRequested(
        &NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
            let (Some(h), Some(args)) = (weak.upgrade(), args) else {
                return Ok(());
            };
            let url = text(|p| args.Uri(p));
            if let Some(task) = h.borrow_mut().tasks.get_mut(&key) {
                task.error = Some("后台页面请求弹窗，请在可见浏览器中完成登录或验证".into());
                args.SetHandled(true)?;
                return Ok(());
            }
            let private = h
                .borrow()
                .state
                .tabs
                .iter()
                .find(|t| t.id == key)
                .is_some_and(|t| t.private);
            let mut tab = BrowserTab::new(url, private);
            // An about:blank popup may receive document.write or an OAuth form from its opener.
            tab.start = false;
            let deferral = args.GetDeferral()?;
            {
                let mut h = h.borrow_mut();
                h.state.active = Some(tab.id.clone());
                h.state.tabs.push(tab.clone());
            }
            args.SetHandled(true)?;
            if let Err(error) = create_tab(&h, tab, Some((args, deferral.clone()))) {
                let _ = deferral.Complete();
                fail(&h, error);
            }
            publish(&h, true);
            Ok(())
        })),
        &mut token,
    )?;
    let weak = Rc::downgrade(host);
    let key = id.to_string();
    core.add_PermissionRequested(
        &PermissionRequestedEventHandler::create(Box::new(move |_, args| {
            let (Some(h), Some(args)) = (weak.upgrade(), args) else {
                return Ok(());
            };
            let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
            args.PermissionKind(&mut kind)?;
            if let Some(task) = h.borrow_mut().tasks.get_mut(&key) {
                task.error = Some("后台页面请求网站权限，请在可见浏览器中完成".into());
                args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                return Ok(());
            }
            let view = BrowserPermission {
                id: uuid::Uuid::new_v4().to_string(),
                tab_id: key.clone(),
                origin: text(|p| args.Uri(p)),
                kind: kind.0,
            };
            let permission = Permission {
                view: view.clone(),
                deferral: args.GetDeferral()?,
                args,
            };
            h.borrow_mut().pending.insert(view.id.clone(), permission);
            publish(&h, false);
            Ok(())
        })),
        &mut token,
    )?;
    let weak = Rc::downgrade(host);
    let key = id.to_string();
    core.add_ContainsFullScreenElementChanged(
        &ContainsFullScreenElementChangedEventHandler::create(Box::new(move |sender, _| {
            if let (Some(h), Some(core)) = (weak.upgrade(), sender) {
                let mut full = BOOL::default();
                core.ContainsFullScreenElement(&mut full)?;
                if let Some(t) = h.borrow_mut().tabs.get_mut(&key) {
                    t.fullscreen = full.as_bool();
                }
                apply_bounds(&h)?;
            }
            Ok(())
        })),
        &mut token,
    )?;
    let weak = Rc::downgrade(host);
    let key = id.to_string();
    core.add_WindowCloseRequested(
        &WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
            if let Some(h) = weak.upgrade() {
                let (app, owner) = {
                    let h = h.borrow();
                    (h.app.clone(), h.owner.clone())
                };
                let id = key.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Some(h) = HOSTS.with(|hosts| hosts.borrow().get(&owner).cloned()) {
                        if let Err(error) = operate(&h, BrowserRequest::Close { id }) {
                            fail(&h, error);
                        }
                    }
                });
            }
            Ok(())
        })),
        &mut token,
    )?;
    let weak = Rc::downgrade(host);
    let key = id.to_string();
    controller.add_ZoomFactorChanged(
        &ZoomFactorChangedEventHandler::create(Box::new(move |sender, _| {
            if let (Some(h), Some(controller)) = (weak.upgrade(), sender) {
                let mut zoom = 1.0;
                controller.ZoomFactor(&mut zoom)?;
                if let Some(t) = h.borrow_mut().state.tabs.iter_mut().find(|t| t.id == key) {
                    t.zoom = zoom;
                }
                publish(&h, false);
            }
            Ok(())
        })),
        &mut token,
    )?;
    let weak = Rc::downgrade(host);
    let key = id.to_string();
    controller.add_AcceleratorKeyPressed(
        &AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
            let (Some(h), Some(args)) = (weak.upgrade(), args) else {
                return Ok(());
            };
            let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
            args.KeyEventKind(&mut kind)?;
            if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
            {
                return Ok(());
            }
            let mut code = 0;
            args.VirtualKey(&mut code)?;
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            let action = match (ctrl, shift, code) {
                (_, _, 117) | (true, _, 76) => Some("address"),
                (true, _, 70) => Some("find"),
                (true, true, 84) => Some("reopen"),
                (true, true, 78) => Some("private_tab"),
                (true, false, 84) => Some("new_tab"),
                (true, _, 87) => Some("close_tab"),
                (true, true, 9) => Some("previous_tab"),
                (true, false, 9) => Some("next_tab"),
                _ => None,
            };
            if let Some(action) = action {
                args.SetHandled(true)?;
                let (app, owner) = {
                    let h = h.borrow();
                    (h.app.clone(), h.owner.clone())
                };
                let app_copy = app.clone();
                let id = key.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Some(view) = app_copy.get_webview_window(&owner) {
                        let _ = view.with_webview(|view| {
                            let _ = view
                                .controller()
                                .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC);
                        });
                    }
                    let _ = app_copy.emit_to(
                        &owner,
                        "browser:shortcut",
                        serde_json::json!({"action":action,"id":id}),
                    );
                });
            }
            Ok(())
        })),
        &mut token,
    )?;
    let weak = Rc::downgrade(host);
    let key = id.to_string();
    core.cast::<ICoreWebView2_4>()?.add_DownloadStarting(
        &DownloadStartingEventHandler::create(Box::new(move |_, args| {
            if let (Some(h), Some(args)) = (weak.upgrade(), args) {
                if let Some(task) = h.borrow_mut().tasks.get_mut(&key) {
                    task.error = Some("后台读取遇到下载，请在可见浏览器中操作".into());
                    args.SetCancel(true)?;
                    return Ok(());
                }
                let operation = args.DownloadOperation()?;
                watch_download(&h, operation)?;
            }
            Ok(())
        })),
        &mut token,
    )?;
    Ok(())
}

unsafe fn watch_download(
    host: &HostRef,
    operation: ICoreWebView2DownloadOperation,
) -> windows::core::Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    host.borrow_mut()
        .downloads
        .insert(id.clone(), operation.clone());
    let update = |host: &HostRef, id: &str, operation: &ICoreWebView2DownloadOperation| {
        let mut received = 0;
        let mut total = 0;
        let mut state = COREWEBVIEW2_DOWNLOAD_STATE::default();
        let _ = operation.BytesReceived(&mut received);
        let _ = operation.TotalBytesToReceive(&mut total);
        let _ = operation.State(&mut state);
        let value = BrowserDownload {
            id: id.into(),
            path: text(|p| operation.ResultFilePath(p)),
            received,
            total,
            state: state.0,
        };
        {
            let mut h = host.borrow_mut();
            if let Some(old) = h.state.downloads.iter_mut().find(|old| old.id == id) {
                *old = value;
            } else {
                h.state.downloads.push(value);
            }
        }
        publish(host, false);
    };
    update(host, &id, &operation);
    let weak = Rc::downgrade(host);
    let key = id.clone();
    let mut token = 0;
    operation.add_BytesReceivedChanged(
        &BytesReceivedChangedEventHandler::create(Box::new(move |sender, _| {
            if let (Some(h), Some(op)) = (weak.upgrade(), sender) {
                update(&h, &key, &op);
            }
            Ok(())
        })),
        &mut token,
    )?;
    let weak = Rc::downgrade(host);
    operation.add_StateChanged(
        &StateChangedEventHandler::create(Box::new(move |sender, _| {
            if let (Some(h), Some(op)) = (weak.upgrade(), sender) {
                update(&h, &id, &op);
            }
            Ok(())
        })),
        &mut token,
    )?;
    Ok(())
}
unsafe fn extensions(host: &HostRef, core: &ICoreWebView2) -> windows::core::Result<()> {
    let profile = core.cast::<ICoreWebView2_13>()?.Profile()?;
    let private = text(|p| profile.ProfileName(p)) == "Private";
    let profile = profile.cast::<ICoreWebView2Profile7>()?;
    let weak = Rc::downgrade(host);
    profile.GetBrowserExtensions(&ProfileGetBrowserExtensionsCompletedHandler::create(
        Box::new(move |result, list| {
            let Some(h) = weak.upgrade() else {
                return Ok(());
            };
            if !h.borrow().state.tabs.iter().any(|tab| {
                Some(&tab.id) == h.borrow().state.active.as_ref() && tab.private == private
            }) {
                return Ok(());
            }
            result?;
            let Some(list) = list else {
                return Ok(());
            };
            let mut count = 0;
            list.Count(&mut count)?;
            let mut values = Vec::new();
            let mut objects = HashMap::new();
            for i in 0..count {
                let ext = list.GetValueAtIndex(i)?;
                let id = text(|p| ext.Id(p));
                // Runtime PDF and clipboard components cannot be managed as user extensions.
                if matches!(
                    id.as_str(),
                    "dgiklkfkllikcanfonkcabmbdfmgleag" | "mhjfbmdgcfjbbpaeojofohoefgiehjai"
                ) {
                    continue;
                }
                let mut enabled = BOOL::default();
                ext.IsEnabled(&mut enabled)?;
                values.push(BrowserExtension {
                    id: id.clone(),
                    name: text(|p| ext.Name(p)),
                    enabled: enabled.as_bool(),
                });
                objects.insert(id, ext);
            }
            {
                let mut h = h.borrow_mut();
                h.state.extensions = values;
                h.extensions = objects;
            }
            publish(&h, false);
            Ok(())
        }),
    ))
}

unsafe fn find(
    host: &HostRef,
    id: &str,
    core: &ICoreWebView2,
    action: &str,
    term: &str,
) -> windows::core::Result<()> {
    let find = core.cast::<ICoreWebView2_28>()?.Find()?;
    if action == "find_stop" || term.is_empty() {
        if let Some(tab) = host.borrow_mut().tabs.get_mut(id) {
            tab.find_term.clear();
        }
        return find.Stop();
    }
    let same = host
        .borrow()
        .tabs
        .get(id)
        .is_some_and(|tab| tab.find_term == term);
    if same {
        return if action == "find_previous" {
            find.FindPrevious()
        } else {
            find.FindNext()
        };
    }
    let environment = host.borrow().environment.clone().unwrap();
    let options = environment
        .cast::<ICoreWebView2Environment15>()?
        .CreateFindOptions()?;
    options.SetFindTerm(PCWSTR(wide(term).as_ptr()))?;
    options.SetShouldHighlightAllMatches(true)?;
    options.SetSuppressDefaultFindDialog(true)?;
    if let Some(tab) = host.borrow_mut().tabs.get_mut(id) {
        tab.find_term = term.into();
    }
    let weak = Rc::downgrade(host);
    find.Start(
        &options,
        &FindStartCompletedHandler::create(Box::new(move |result| {
            if let (Some(h), Err(error)) = (weak.upgrade(), result) {
                fail(&h, error);
            }
            Ok(())
        })),
    )
}

fn operate(host: &HostRef, request: BrowserRequest) -> Result<(), String> {
    unsafe {
        match request {
            BrowserRequest::Init => {}
            BrowserRequest::NewTab { input, private } => {
                let tab = BrowserTab::new(address(&input)?, private);
                {
                    let mut h = host.borrow_mut();
                    h.state.active = Some(tab.id.clone());
                    h.state.tabs.push(tab.clone());
                }
                create_tab(host, tab, None).map_err(|e| e.to_string())?;
            }
            BrowserRequest::Select { id } => {
                if !host.borrow().state.tabs.iter().any(|t| t.id == id) {
                    return Err("Tab no longer exists".into());
                }
                host.borrow_mut().state.active = Some(id.clone());
                let controller = host.borrow().tabs.get(&id).map(|t| t.controller.clone());
                apply_bounds(host).map_err(|e| e.to_string())?;
                if let Some(controller) = controller {
                    controller
                        .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
                        .map_err(|e| e.to_string())?;
                }
                if let Ok(core) = get_core(host, &id) {
                    extensions(host, &core).map_err(|e| e.to_string())?;
                }
            }
            BrowserRequest::Close { id } => {
                let native = host.borrow_mut().tabs.remove(&id);
                if let Some(tab) = native {
                    tab.controller.Close().map_err(|e| e.to_string())?;
                }
                {
                    let mut h = host.borrow_mut();
                    let index = h
                        .state
                        .tabs
                        .iter()
                        .position(|t| t.id == id)
                        .ok_or("Tab no longer exists")?;
                    let tab = h.state.tabs.remove(index);
                    if !tab.private {
                        h.closed.push(tab);
                    }
                    if h.state.active.as_ref() == Some(&id) {
                        h.state.active = h
                            .state
                            .tabs
                            .get(index.min(h.state.tabs.len().saturating_sub(1)))
                            .map(|t| t.id.clone());
                    }
                    let keys: Vec<_> = h
                        .pending
                        .iter()
                        .filter(|(_, p)| p.view.tab_id == id)
                        .map(|(id, _)| id.clone())
                        .collect();
                    let permissions = keys
                        .into_iter()
                        .filter_map(|id| h.pending.remove(&id))
                        .collect::<Vec<_>>();
                    drop(h);
                    for p in permissions {
                        p.args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY).ok();
                        p.deferral.Complete().ok();
                    }
                }
            }
            BrowserRequest::Reopen => {
                let tab = host.borrow_mut().closed.pop();
                if let Some(mut tab) = tab {
                    tab.id = uuid::Uuid::new_v4().to_string();
                    host.borrow_mut().state.tabs.push(tab.clone());
                    host.borrow_mut().state.active = Some(tab.id.clone());
                    create_tab(host, tab, None).map_err(|e| e.to_string())?;
                }
            }
            BrowserRequest::Navigate { id, input } => {
                let url = address(&input)?;
                let core = get_core(host, &id)?;
                core.Navigate(PCWSTR(wide(&url).as_ptr()))
                    .map_err(|e| e.to_string())?;
            }
            BrowserRequest::Action {
                id,
                name,
                text: input,
            } => {
                let core = get_core(host, &id)?;
                let result = match name.as_str() {
                    "back" => core.GoBack(),
                    "forward" => core.GoForward(),
                    "reload" => core.Reload(),
                    "stop" => core.Stop(),
                    "devtools" => core.OpenDevToolsWindow(),
                    "print" => core
                        .cast::<ICoreWebView2_16>()
                        .and_then(|c| c.ShowPrintUI(COREWEBVIEW2_PRINT_DIALOG_KIND_BROWSER)),
                    "downloads" => core
                        .cast::<ICoreWebView2_9>()
                        .and_then(|c| c.OpenDefaultDownloadDialog()),
                    "save" => core.cast::<ICoreWebView2_25>().and_then(|c| {
                        c.ShowSaveAsUI(&ShowSaveAsUICompletedHandler::create(Box::new(
                            |error, _| {
                                error?;
                                Ok(())
                            },
                        )))
                    }),
                    "zoom_in" | "zoom_out" | "zoom_reset" => {
                        let controller = host
                            .borrow()
                            .tabs
                            .get(&id)
                            .map(|t| t.controller.clone())
                            .ok_or("Tab is initializing")?;
                        let zoom = {
                            let mut h = host.borrow_mut();
                            let tab = h
                                .state
                                .tabs
                                .iter_mut()
                                .find(|t| t.id == id)
                                .ok_or("Unknown tab")?;
                            tab.zoom = if name == "zoom_reset" {
                                1.0
                            } else {
                                (tab.zoom + if name == "zoom_in" { 0.1 } else { -0.1 })
                                    .clamp(0.25, 5.0)
                            };
                            tab.zoom
                        };
                        controller.SetZoomFactor(zoom)
                    }
                    "find" | "find_previous" | "find_stop" => find(host, &id, &core, &name, &input),
                    "clear_data" => {
                        let weak = Rc::downgrade(host);
                        let profile = core
                            .cast::<ICoreWebView2_13>()
                            .and_then(|c| c.Profile())
                            .and_then(|p| p.cast::<ICoreWebView2Profile2>())
                            .map_err(|e| e.to_string())?;
                        profile.ClearBrowsingData(
                            COREWEBVIEW2_BROWSING_DATA_KINDS_ALL_SITE
                                | COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE
                                | COREWEBVIEW2_BROWSING_DATA_KINDS_SETTINGS,
                            &ClearBrowsingDataCompletedHandler::create(Box::new(move |result| {
                                if let Some(h) = weak.upgrade() {
                                    if let Err(error) = result {
                                        fail(&h, error);
                                    } else {
                                        publish(&h, false);
                                    }
                                }
                                Ok(())
                            })),
                        )
                    }
                    "mute" => core.cast::<ICoreWebView2_8>().and_then(|c| {
                        let mut muted = BOOL::default();
                        c.IsMuted(&mut muted)?;
                        c.SetIsMuted(!muted.as_bool())
                    }),
                    _ => return Err("Unknown browser action".into()),
                };
                result.map_err(|e| format!("{name}: {e}"))?;
            }
            BrowserRequest::Bookmark { id } => {
                let mut h = host.borrow_mut();
                let tab = h
                    .state
                    .tabs
                    .iter()
                    .find(|t| t.id == id)
                    .cloned()
                    .ok_or("Unknown tab")?;
                if h.state.bookmarks.iter().any(|b| b.url == tab.url) {
                    h.state.bookmarks.retain(|b| b.url != tab.url);
                } else {
                    h.state.bookmarks.push(PageEntry {
                        url: tab.url,
                        title: tab.title,
                        at: chrono::Utc::now().timestamp_millis(),
                    });
                }
            }
            BrowserRequest::RemoveBookmark { url } => {
                host.borrow_mut().state.bookmarks.retain(|b| b.url != url)
            }
            BrowserRequest::RenameBookmark { url, title } => {
                let title = title.trim();
                if title.is_empty() || title.chars().count() > 500 { return Err("Bookmark name must contain 1–500 characters".into()); }
                let mut h = host.borrow_mut();
                let bookmark = h.state.bookmarks.iter_mut().find(|entry| entry.url == url).ok_or("Bookmark no longer exists")?;
                bookmark.title = title.to_string();
            }
            BrowserRequest::Permission { id, allow } => {
                let permission = host
                    .borrow_mut()
                    .pending
                    .remove(&id)
                    .ok_or("Permission request expired")?;
                permission
                    .args
                    .SetState(if allow {
                        COREWEBVIEW2_PERMISSION_STATE_ALLOW
                    } else {
                        COREWEBVIEW2_PERMISSION_STATE_DENY
                    })
                    .map_err(|e| e.to_string())?;
                permission.deferral.Complete().map_err(|e| e.to_string())?;
            }
            BrowserRequest::Download { id, name } => {
                let op = host
                    .borrow()
                    .downloads
                    .get(&id)
                    .cloned()
                    .ok_or("Download not found")?;
                match name.as_str() {
                    "pause" => op.Pause(),
                    "resume" => op.Resume(),
                    "cancel" => op.Cancel(),
                    _ => return Err("Unknown download action".into()),
                }
                .map_err(|e| e.to_string())?;
            }
            BrowserRequest::Extension { id, name, path } => {
                let active = host.borrow().state.active.clone().ok_or("No active tab")?;
                let core = get_core(host, &active)?;
                if name == "install" {
                    if !std::path::Path::new(&path).join("manifest.json").is_file() {
                        return Err(
                            "Select an unpacked extension directory containing manifest.json"
                                .into(),
                        );
                    }
                    let weak = Rc::downgrade(host);
                    let core_copy = core.clone();
                    core.cast::<ICoreWebView2_13>()
                        .and_then(|c| c.Profile())
                        .and_then(|p| p.cast::<ICoreWebView2Profile7>())
                        .map_err(|e| e.to_string())?
                        .AddBrowserExtension(
                            PCWSTR(wide(&path).as_ptr()),
                            &ProfileAddBrowserExtensionCompletedHandler::create(Box::new(
                                move |result, _| {
                                    if let Some(h) = weak.upgrade() {
                                        if let Err(e) = result {
                                            fail(&h, e);
                                        } else {
                                            extensions(&h, &core_copy)?;
                                        }
                                    }
                                    Ok(())
                                },
                            )),
                        )
                        .map_err(|e| e.to_string())?;
                } else {
                    let ext = host
                        .borrow()
                        .extensions
                        .get(&id)
                        .cloned()
                        .ok_or("Extension no longer exists")?;
                    let weak = Rc::downgrade(host);
                    if name == "remove" {
                        ext.Remove(&BrowserExtensionRemoveCompletedHandler::create(Box::new(
                            move |result| {
                                if let Some(h) = weak.upgrade() {
                                    if let Err(e) = result {
                                        fail(&h, e);
                                    } else {
                                        extensions(&h, &core)?;
                                    }
                                }
                                Ok(())
                            },
                        )))
                        .map_err(|e| e.to_string())?;
                    } else {
                        ext.Enable(
                            name == "enable",
                            &BrowserExtensionEnableCompletedHandler::create(Box::new(
                                move |result| {
                                    if let Some(h) = weak.upgrade() {
                                        if let Err(e) = result {
                                            fail(&h, e);
                                        } else {
                                            extensions(&h, &core)?;
                                        }
                                    }
                                    Ok(())
                                },
                            )),
                        )
                        .map_err(|e| e.to_string())?;
                    }
                }
            }
            BrowserRequest::ClearHistory => host.borrow_mut().state.history.clear(),
        }
        apply_bounds(host).map_err(|e| e.to_string())?;
        publish(host, true);
        Ok(())
    }
}
fn get_core(host: &HostRef, id: &str) -> Result<ICoreWebView2, String> {
    host.borrow()
        .tabs
        .get(id)
        .map(|t| t.core.clone())
        .ok_or("Browser tab is still initializing".into())
}

pub async fn request(
    app: tauri::AppHandle,
    owner: String,
    request: BrowserRequest,
) -> Result<BrowserSnapshot, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let app_copy = app.clone();
    app.run_on_main_thread(move || {
        if matches!(request, BrowserRequest::Init) {
            let retry = HOSTS.with(|hosts| {
                hosts.borrow().get(&owner).is_some_and(|h| {
                    h.borrow().environment.is_none() && h.borrow().state.error.is_some()
                })
            });
            if retry {
                destroy(&owner);
            }
        }
        let result = initialize(app_copy, owner.clone()).and_then(|h| {
            if !matches!(request, BrowserRequest::Init) {
                h.borrow_mut().state.error = None;
            }
            operate(&h, request)?;
            let snapshot = h.borrow().state.clone();
            Ok(snapshot)
        });
        if let Err(error) = &result {
            if let Some(h) = HOSTS.with(|hosts| hosts.borrow().get(&owner).cloned()) {
                fail(&h, error);
            }
        }
        tx.send(result).ok();
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}
// Presentation only: pages receive CSS, never a Tauri bridge.
fn apply_scrollbar_style(core: &ICoreWebView2, css: &str) {
    if css.is_empty() { return; }
    let css = serde_json::to_string(css).unwrap_or_default();
    let script = format!("(() => {{ let s = document.getElementById('__vivian_browser_scrollbars'); if (!s) {{ s = document.createElement('style'); s.id = '__vivian_browser_scrollbars'; (document.head || document.documentElement).appendChild(s); }} s.textContent = {css}; }})()");
    unsafe { let _ = core.ExecuteScript(PCWSTR(wide(&script).as_ptr()), &ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())))); }
}
pub async fn set_scrollbar_style(app: tauri::AppHandle, owner: String, css: String) -> Result<(), String> {
    if css.len() > 8192 { return Err("Browser scrollbar style is too large".into()); }
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        if let Some(host) = HOSTS.with(|hosts| hosts.borrow().get(&owner).cloned()) {
            let mut h = host.borrow_mut();
            if h.scrollbar_css != css {
                for tab in h.tabs.values() { apply_scrollbar_style(&tab.core, &css); }
                h.scrollbar_css = css;
            }
        }
        let _ = tx.send(());
    }).map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())
}
// Compatibility entry for the native browser smoke harness.
#[allow(dead_code)]
pub async fn layout(
    app: tauri::AppHandle,
    owner: String,
    bounds: BrowserBounds,
    visible: bool,
    sequence: u64,
) -> Result<(), String> {
    layout_clipped(app, owner, bounds, visible, sequence, None).await
}
pub async fn layout_clipped(
    app: tauri::AppHandle, owner: String, bounds: BrowserBounds, visible: bool,
    sequence: u64, clip: Option<BrowserClip>,
) -> Result<(), String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let host = HOSTS.with(|hosts| hosts.borrow().get(&owner).cloned());
        let result = if let Some(h) = host {
            if visible {
                let parent = {
                    let h = h.borrow();
                    h.app
                        .get_webview_window(&h.owner)
                        .and_then(|w| w.hwnd().ok())
                };
                if let Some(parent) = parent {
                    if let Err(error) = attach(&h, parent) {
                        let _ = tx.send(Err(error));
                        return;
                    }
                }
            }
            let apply = {
                let mut h = h.borrow_mut();
                if sequence > h.last_layout {
                    h.bounds = bounds;
                    h.clip = clip;
                    h.visible = visible;
                    h.last_layout = sequence;
                    true
                } else {
                    false
                }
            };
            if apply {
                apply_bounds(&h).map_err(|e| e.to_string())
            } else {
                Ok(())
            }
        } else {
            Ok(())
        };
        tx.send(result).ok();
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}
pub fn destroy(owner: &str) {
    HOSTS.with(|hosts| {
        hosts.borrow_mut().remove(owner);
    });
}

fn attach(host: &HostRef, parent: HWND) -> Result<(), String> {
    let (previous_parent, old_surface, controllers) = {
        let h = host.borrow();
        (h.hwnd, h.surface_hwnd, h.tabs.iter()
            .filter(|(id, tab)| !tab.input_parked && !h.tasks.contains_key(*id))
            .map(|(_, t)| t.controller.clone()).collect::<Vec<_>>())
    };
    let exists = unsafe { IsWindow(Some(old_surface)).as_bool() };
    if previous_parent == parent && exists { return Ok(()); }
    let surface = if exists {
        unsafe { SetParent(old_surface, Some(parent)).map_err(|e| e.to_string())?; }
        old_surface
    } else { create_surface(parent).map_err(|e| e.to_string())? };
    {
        let mut h = host.borrow_mut();
        h.hwnd = parent;
        h.surface_hwnd = surface;
    }
    for controller in controllers {
        unsafe { controller.SetParentWindow(surface).map_err(|e| e.to_string())?; }
    }
    Ok(())
}
pub fn park(owner: &str) {
    if let Some(h) = HOSTS.with(|hosts| hosts.borrow().get(owner).cloned()) {
        h.borrow_mut().visible = false;
        let private_tabs = h
            .borrow()
            .state
            .tabs
            .iter()
            .filter(|t| t.private)
            .map(|t| t.id.clone())
            .collect::<Vec<_>>();
        for id in private_tabs {
            let _ = operate(&h, BrowserRequest::Close { id });
        }
        let parent = {
            let mut h = h.borrow_mut();
            h.visible = false;
            h.parked_hwnd
        };
        let _ = apply_bounds(&h);
        let _ = attach(&h, parent);
    }
}

#[cfg(test)]
mod viewport_clip_tests {
    use super::*;
    use windows::Win32::Graphics::Gdi::{GetWindowRgn, PtInRegion};
    use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

    #[test]
    fn native_region_excludes_sidebar_without_resizing_page() {
        unsafe {
            let parent = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(wide("STATIC").as_ptr()),
                PCWSTR(wide("viewport clip test").as_ptr()), WS_POPUP,
                0, 0, 1200, 900, None, None, None, None).unwrap();
            let surface = create_surface(parent).unwrap();
            SetWindowPos(surface, None, 100, 120, 800, 600, SWP_NOACTIVATE | SWP_NOZORDER).unwrap();
            let rect = RECT { left: 100, top: 120, right: 900, bottom: 720 };
            let clip = BrowserClip {
                pixel_ratio: 1.25, radius: 0.0,
                viewport: BrowserBounds { x: 88.0, y: 96.0, width: 632.0, height: 480.0 },
                overlays: vec![BrowserBounds { x: 0.0, y: 0.0, width: 268.0, height: 900.0 }],
            };
            clip_surface(surface, rect, 1.25, Some(&clip)).unwrap();
            let region = CreateRectRgn(0, 0, 0, 0);
            assert_ne!(GetWindowRgn(surface, region).0, 0);
            assert!(!PtInRegion(region, 5, 100).as_bool(), "clip to the actual viewport left edge");
            assert!(!PtInRegion(region, 100, 100).as_bool(), "sidebar reveals the HTML window beneath");
            assert!(PtInRegion(region, 400, 100).as_bool(), "uncovered browser stays live");
            let mut actual = RECT::default();
            GetClientRect(surface, &mut actual).unwrap();
            assert_eq!((actual.right, actual.bottom), (800, 600), "clipping preserves document bounds");
            clip_surface(surface, rect, 1.25, None).unwrap();
            GetWindowRgn(surface, region);
            assert!(PtInRegion(region, 100, 100).as_bool(), "closing the sidebar restores the covered area");
            let _ = DeleteObject(region.into());
            DestroyWindow(surface).unwrap();
            DestroyWindow(parent).unwrap();
        }
    }
}
