//! Original Playwright over a private stdio transport. No TCP debugging endpoint.
//! Only protocol routing, task ownership and execution lifecycle live here.
use super::automation::{main, Operation};
use super::*;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, Notify};

fn node_path(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(local) = text.strip_prefix(r"\\?\") {
        PathBuf::from(local)
    } else {
        path
    }
}
const EVENTS: &[&str] = &[
    "Page.frameAttached",
    "Page.frameDetached",
    "Page.frameNavigated",
    "Page.navigatedWithinDocument",
    "Page.frameRequestedNavigation",
    "Page.frameStartedLoading",
    "Page.frameStoppedLoading",
    "Page.lifecycleEvent",
    "Page.javascriptDialogOpening",
    "Page.javascriptDialogClosed",
    "Page.windowOpen",
    "Runtime.executionContextCreated",
    "Runtime.executionContextDestroyed",
    "Runtime.executionContextsCleared",
    "Runtime.consoleAPICalled",
    "Runtime.exceptionThrown",
    "Runtime.bindingCalled",
    "Network.requestWillBeSent",
    "Network.requestWillBeSentExtraInfo",
    "Network.responseReceived",
    "Network.responseReceivedExtraInfo",
    "Network.loadingFinished",
    "Network.loadingFailed",
    "Network.requestServedFromCache",
    "Network.webSocketCreated",
    "Network.webSocketClosed",
    "Network.webSocketFrameReceived",
    "Network.webSocketFrameSent",
    "Inspector.detached",
    "Inspector.targetCrashed",
    "Log.entryAdded",
    "Target.attachedToTarget",
    "Target.detachedFromTarget",
    "Target.targetInfoChanged",
    "Fetch.requestPaused",
    "Fetch.authRequired",
];
struct Route {
    tx: mpsc::UnboundedSender<Value>,
    targets: HashMap<String, Value>, // native tab id -> target info
    sessions: HashMap<String, (String, String)>, // public session -> (native tab, native session)
    subscriptions: Vec<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>,
    attached: bool,
}
thread_local! { static ROUTES:RefCell<HashMap<String,Route>>=RefCell::new(HashMap::new()); }
fn operation() -> Operation {
    Operation {
        cancelled: Arc::new(AtomicBool::new(false)),
        deadline: Instant::now() + Duration::from_secs(15),
    }
}
fn allowed_url(url: &str) -> bool {
    url == "about:blank"
        || tauri::Url::parse(url).is_ok_and(|u| matches!(u.scheme(), "http" | "https"))
}
fn session(tab: &str, native: &str) -> String {
    if native.is_empty() {
        format!("page:{tab}")
    } else {
        format!("frame:{tab}:{native}")
    }
}

async fn native_call(
    app: &tauri::AppHandle,
    task: &str,
    tab: &str,
    native_session: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let task = task.to_owned();
    let tab = tab.to_owned();
    let native_session = native_session.to_owned();
    let method = method.to_owned();
    let sender = Arc::new(std::sync::Mutex::new(Some(tx)));
    let callback = sender.clone();
    main(app, &operation(), move |h| {
        let permitted = ROUTES.with(|r| {
            r.borrow().get(&task).is_some_and(|r| {
                method == "Target.getTargetInfo"
                    || (r.targets.contains_key(&tab)
                        && r.sessions
                            .values()
                            .any(|(id, sid)| id == &tab && sid == &native_session))
            })
        });
        if !permitted || !h.borrow().tab(&tab).is_some_and(|t| allowed_url(&t.url)) {
            return Err("Task ownership or document changed before CDP dispatch".into());
        }
        let core = get_core(&h, &tab)?;
        let committed = unsafe { text(|p| core.Source(p)) };
        if !committed.is_empty() && !allowed_url(&committed) {
            return Err("Committed task document is outside HTTP(S)/about:blank".into());
        }
        let completion =
            CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |status, body| {
                let value = status
                    .map_err(|e| e.to_string())
                    .and_then(|_| serde_json::from_str(&body).map_err(|e| e.to_string()));
                if let Some(tx) = callback.lock().unwrap().take() {
                    let _ = tx.send(value);
                }
                Ok(())
            }));
        let result = unsafe {
            if native_session.is_empty() {
                core.CallDevToolsProtocolMethod(
                    PCWSTR(wide(&method).as_ptr()),
                    PCWSTR(wide(&params.to_string()).as_ptr()),
                    &completion,
                )
            } else {
                core.cast::<ICoreWebView2_11>().and_then(|core| {
                    core.CallDevToolsProtocolMethodForSession(
                        PCWSTR(wide(&native_session).as_ptr()),
                        PCWSTR(wide(&method).as_ptr()),
                        PCWSTR(wide(&params.to_string()).as_ptr()),
                        &completion,
                    )
                })
            }
        };
        if let Err(error) = result {
            if let Some(tx) = sender.lock().unwrap().take() {
                let _ = tx.send(Err(error.to_string()));
            }
        }
        Ok(())
    })
    .await?;
    tokio::time::timeout(Duration::from_secs(15), rx)
        .await
        .map_err(|_| "Native CDP response timed out".to_string())?
        .map_err(|_| "Native CDP callback closed".to_string())?
}

async fn attach(
    app: &tauri::AppHandle,
    key: &str,
    tab: &str,
    opener: Option<String>,
) -> Result<(), String> {
    let info = native_call(app, key, tab, "", "Target.getTargetInfo", json!({})).await?;
    let mut info = info["targetInfo"].clone();
    if !info["targetId"].is_string() {
        return Err("WebView2 returned no native target identity".into());
    }
    info["type"] = json!("page");
    info["attached"] = json!(true);
    if !info["browserContextId"].is_string() {
        info["browserContextId"] = json!("vivian-default");
    }
    if let Some(opener) = opener {
        info["openerId"] = json!(opener);
    }
    let key = key.to_owned();
    let focus_key = key.clone();
    let tab = tab.to_owned();
    let surface = tab.clone();
    let focus_tab = tab.clone();
    main(app,&operation(),move |h|{
        if !h.borrow().tab(&tab).is_some_and(|t|allowed_url(&t.url)){return Err("Only HTTP(S) task pages and about:blank may be attached".into());}
        let core=get_core(&h,&tab)?;
        let root_session=session(&tab,"");
        ROUTES.with(|routes|->Result<(),String>{
            let mut routes=routes.borrow_mut();
            if routes.iter().any(|(other,r)|*other!=key && r.targets.contains_key(&tab)){return Err("Page already belongs to another browser task".into());}
            let route=routes.get_mut(&key).ok_or("Task was closed")?;
            route.sessions.insert(root_session.clone(),(tab.clone(),String::new()));
            route.targets.insert(tab.clone(),info.clone());
            for method in EVENTS {
                let receiver=unsafe{core.GetDevToolsProtocolEventReceiver(PCWSTR(wide(method).as_ptr()))}.map_err(|e|e.to_string())?;
                let key=key.clone();let tab=tab.clone();let method=method.to_string();let weak=Rc::downgrade(&h);let epoch=h.borrow().lifecycle.epoch();
                let mut token=0;
                unsafe{receiver.add_DevToolsProtocolEventReceived(&DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_,args|{
                    let Some(h)=weak.upgrade().filter(|h|h.borrow().lifecycle.current(epoch)) else{return Ok(());};
                    if !h.borrow().tab(&tab).is_some_and(|t|allowed_url(&t.url)){return Ok(());}
                    let Some(args)=args else{return Ok(());};
                    let raw=text(|p|args.ParameterObjectAsJson(p));
                    let Ok(mut params)=serde_json::from_str::<Value>(&raw) else{return Ok(());};
                    let native=args.cast::<ICoreWebView2DevToolsProtocolEventReceivedEventArgs2>().ok().map(|a|text(|p|a.SessionId(p))).unwrap_or_default();
                    ROUTES.with(|routes|{
                        let mut routes=routes.borrow_mut();let Some(route)=routes.get_mut(&key)else{return;};
                        if method=="Target.attachedToTarget" {
                            if !matches!(params["targetInfo"]["type"].as_str(),Some("iframe"|"worker")) {return;}
                            let Some(child)=params["sessionId"].as_str().map(str::to_owned)else{return;};
                            let public=session(&tab,&child);route.sessions.insert(public.clone(),(tab.clone(),child));params["sessionId"]=json!(public);
                        } else if method=="Target.detachedFromTarget" {
                            if let Some(child)=params["sessionId"].as_str(){let public=session(&tab,child);route.sessions.remove(&public);params["sessionId"]=json!(public);}
                        }
                        let public=session(&tab,&native);
                        if route.sessions.contains_key(&public){let _=route.tx.send(json!({"kind":"cdp","message":{"method":method,"params":params,"sessionId":public}}));}
                    });Ok(())
                })),&mut token)}.map_err(|e|e.to_string())?;
                route.subscriptions.push((receiver,token));
            }
            if route.attached {let _=route.tx.send(json!({"kind":"cdp","message":{"method":"Target.attachedToTarget","params":{"sessionId":root_session,"targetInfo":info,"waitingForDebugger":false}}}));}
            Ok(())
        })
    }).await?;
    main(app, &operation(), move |h| {
        automation::park_render_surface(&h, &surface)
    })
    .await?;
    native_call(
        app,
        &focus_key,
        &focus_tab,
        "",
        "Emulation.setFocusEmulationEnabled",
        json!({"enabled":true}),
    )
    .await?;
    Ok(())
}

pub(super) fn task_for_tab(tab: &str) -> Option<String> {
    ROUTES.with(|r| {
        r.borrow()
            .iter()
            .find(|(_, r)| r.targets.contains_key(tab))
            .map(|(key, _)| key.clone())
    })
}
pub(super) fn sync_surfaces(host: &HostRef) -> windows::core::Result<()> {
    let changes = {
        let mut h = host.borrow_mut();
        let selected = h.state.active.clone();
        let show = h.visible && h.bounds.width > 0.0 && h.bounds.height > 0.0;
        let surface = h.surface_hwnd;
        let parked = h.parked_hwnd;
        h.tabs
            .iter_mut()
            .filter_map(|(id, tab)| {
                if task_for_tab(id).is_none() {
                    return None;
                }
                let visible = show && selected.as_deref() == Some(id.as_str());
                let park = !visible;
                if tab.input_parked == park {
                    return None;
                }
                tab.input_parked = park;
                Some((
                    tab.controller.clone(),
                    if park { parked } else { surface },
                    park,
                ))
            })
            .collect::<Vec<_>>()
    };
    for (controller, parent, park) in changes {
        unsafe {
            let mut bounds = RECT::default();
            controller.Bounds(&mut bounds)?;
            controller.SetIsVisible(false)?;
            controller.SetParentWindow(parent)?;
            if park {
                let width = (bounds.right - bounds.left).max(128);
                let height = (bounds.bottom - bounds.top).max(128);
                controller.SetBounds(RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                })?;
                controller.SetIsVisible(true)?;
            }
        }
    }
    Ok(())
}
pub(super) fn closed(tab: &str) {
    ROUTES.with(|routes|{for route in routes.borrow_mut().values_mut(){if let Some(info)=route.targets.remove(tab){
        route.sessions.retain(|_,(id,_)|id!=tab);
        let _=route.tx.send(json!({"kind":"cdp","message":{"method":"Target.detachedFromTarget","params":{"sessionId":session(tab,""),"targetId":info["targetId"]}}}));
    }}});
}
pub(super) fn popup(app: tauri::AppHandle, opener: String, tab: String) {
    if std::env::var_os("VIVIAN_PLAYWRIGHT_TRACE").is_some() {
        eprintln!("Playwright popup requested: {opener} -> {tab}");
    }
    let owner = ROUTES.with(|r| {
        r.borrow().iter().find_map(|(key, r)| {
            r.targets.get(&opener).map(|info| {
                (
                    key.clone(),
                    info["targetId"].as_str().unwrap_or("").to_string(),
                )
            })
        })
    });
    if let Some((key, opener)) = owner {
        tauri::async_runtime::spawn(async move {
            // Popup controller creation is asynchronous; subscribe as soon as it is ready.
            for _ in 0..100 {
                let id = tab.clone();
                if main(&app, &operation(), move |h| {
                    Ok(h.borrow().tabs.contains_key(&id))
                })
                .await
                    == Ok(true)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            if let Err(error) = attach(&app, &key, &tab, Some(opener)).await {
                if std::env::var_os("VIVIAN_PLAYWRIGHT_TRACE").is_some() {
                    eprintln!("Playwright popup attachment failed: {error}");
                }
                tracing::warn!("Playwright popup attachment: {error}");
            }
        });
    }
}
async fn release(app: &tauri::AppHandle, key: String) {
    let _ = main(app, &operation(), move |h| {
        if let Some(route) = ROUTES.with(|routes| routes.borrow_mut().remove(&key)) {
            for (receiver, token) in route.subscriptions {
                unsafe {
                    let _ = receiver.remove_DevToolsProtocolEventReceived(token);
                }
            }
            for tab in route.targets.keys() {
                if let Ok(core) = get_core(&h, tab) {
                    unsafe {
                        let _ = core.CallDevToolsProtocolMethod(
                            PCWSTR(wide("Emulation.setFocusEmulationEnabled").as_ptr()),
                            PCWSTR(wide("{\"enabled\":false}").as_ptr()),
                            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                                |_, _| Ok(()),
                            )),
                        );
                    }
                }
                let _ = automation::restore_input_surface(&h, tab);
            }
        }
        Ok(())
    })
    .await;
}
async fn route(app: tauri::AppHandle, key: String, message: Value) -> Value {
    let id = message["id"].clone();
    let public = message["sessionId"].as_str().unwrap_or("").to_string();
    let method = message["method"].as_str().unwrap_or("").to_string();
    let params = message
        .get("params")
        .filter(|p| p.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = route_inner(&app, &key, &public, &method, params).await;
    if std::env::var_os("VIVIAN_PLAYWRIGHT_TRACE").is_some() {
        eprintln!(
            "Playwright CDP {method} [{public}]: {}",
            match &result {
                Ok(_) => "ok".into(),
                Err(e) => e.clone(),
            }
        );
    }
    let mut response = match result {
        Ok(result) => json!({"id":id,"result":result}),
        Err(error) => json!({"id":id,"error":{"code":-32000,"message":error}}),
    };
    if !public.is_empty() {
        response["sessionId"] = json!(public);
    }
    response
}
async fn route_inner(
    app: &tauri::AppHandle,
    key: &str,
    public: &str,
    method: &str,
    mut params: Value,
) -> Result<Value, String> {
    let owner = key.to_owned();
    let sid = public.to_owned();
    let route = main(app, &operation(), move |h| {
        ROUTES.with(|routes| {
            let mut routes = routes.borrow_mut();
            let r = routes.get_mut(&owner).ok_or("Browser task was released")?;
            if sid.is_empty() {
                Ok(None)
            } else {
                let (tab, native) = r
                    .sessions
                    .get(&sid)
                    .cloned()
                    .ok_or("Unknown or unowned CDP session")?;
                if !h.borrow().tab(&tab).is_some_and(|t| allowed_url(&t.url)) {
                    return Err("Task document is no longer allowed".into());
                }
                Ok(Some((tab, native)))
            }
        })
    })
    .await?;
    if let Some((tab, native)) = route {
        if method == "Page.bringToFront" {
            return Ok(json!({}));
        }
        if method.starts_with("Browser.")
            || method.starts_with("Storage.")
            || method == "Network.getAllCookies"
            || (method.starts_with("Target.")
                && !matches!(method, "Target.setAutoAttach" | "Target.detachFromTarget"))
        {
            return Err("Browser management and unrelated targets are outside this task".into());
        }
        if method == "Target.setAutoAttach" {
            params["filter"] = json!([{"type":"iframe","exclude":false},{"type":"worker","exclude":false},{"exclude":true}]);
        }
        if method == "Network.getCookies" {
            let id = tab.clone();
            let origin = main(app, &operation(), move |h| {
                h.borrow()
                    .tab(&id)
                    .and_then(|t| tauri::Url::parse(&t.url).ok())
                    .map(|u| u.origin().ascii_serialization())
                    .ok_or("Task page has no HTTP origin".into())
            })
            .await?;
            if params["urls"].as_array().is_some_and(|urls| {
                urls.iter().any(|u| {
                    u.as_str()
                        .and_then(|u| tauri::Url::parse(u).ok())
                        .is_none_or(|u| u.origin().ascii_serialization() != origin)
                })
            }) {
                return Err("Cookies for unrelated origins are outside this task".into());
            }
        }
        if method == "Page.navigate" && !params["url"].as_str().is_some_and(allowed_url) {
            return Err("Task navigation requires HTTP(S)".into());
        }
        if method == "Target.detachFromTarget" {
            let raw = params["sessionId"].as_str().ok_or("sessionId required")?;
            let owner = key.to_owned();
            let sid = raw.to_owned();
            let resolved = main(app, &operation(), move |_| {
                ROUTES.with(|r| {
                    r.borrow()
                        .get(&owner)
                        .and_then(|r| r.sessions.get(&sid).cloned())
                        .ok_or("Unowned child session".into())
                })
            })
            .await?;
            if resolved.0 != tab {
                return Err("Cross-page child session".into());
            }
            params["sessionId"] = json!(resolved.1);
        }
        return native_call(app, key, &tab, &native, method, params).await;
    }
    if method == "Browser.getVersion" {
        let owner = key.to_owned();
        let tab = main(app, &operation(), move |_| {
            ROUTES.with(|r| {
                r.borrow()
                    .get(&owner)
                    .and_then(|r| r.targets.keys().next().cloned())
                    .ok_or("Task has no pages".into())
            })
        })
        .await?;
        return native_call(app, key, &tab, "", method, params).await;
    }
    if method == "Target.createTarget" {
        let url = params["url"]
            .as_str()
            .filter(|url| allowed_url(url))
            .ok_or("New task pages require HTTP(S) or about:blank")?
            .to_owned();
        let tab = main(app, &operation(), move |h| {
            let mut tab = BrowserTab::new(url, false);
            tab.start = false;
            let id = tab.id.clone();
            h.borrow_mut().state.tabs.push(tab.clone());
            create_tab(&h, tab, None).map_err(|e| e.to_string())?;
            publish(&h, true);
            Ok(id)
        })
        .await?;
        for _ in 0..100 {
            let id = tab.clone();
            if main(app, &operation(), move |h| {
                Ok(h.borrow().tabs.contains_key(&id))
            })
            .await?
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        attach(app, key, &tab, None).await?;
        let id = tab.clone();
        main(app, &operation(), move |h| {
            automation::park_render_surface(&h, &id)
        })
        .await?;
        native_call(
            app,
            key,
            &tab,
            "",
            "Emulation.setFocusEmulationEnabled",
            json!({"enabled":true}),
        )
        .await?;
        let owner = key.to_owned();
        return main(app, &operation(), move |_| {
            ROUTES.with(|r| {
                r.borrow()
                    .get(&owner)
                    .and_then(|r| r.targets.get(&tab))
                    .map(|info| json!({"targetId":info["targetId"]}))
                    .ok_or("New task target disappeared".into())
            })
        })
        .await;
    }
    if method == "Target.closeTarget" {
        let target = params["targetId"]
            .as_str()
            .ok_or("targetId required")?
            .to_owned();
        let owner = key.to_owned();
        return main(app, &operation(), move |h| {
            let tab = ROUTES.with(|r| {
                r.borrow()
                    .get(&owner)
                    .and_then(|r| {
                        r.targets
                            .iter()
                            .find(|(_, info)| info["targetId"] == target)
                            .map(|(tab, _)| tab.clone())
                    })
                    .ok_or("Unknown targetId".to_string())
            })?;
            operate(&h, BrowserRequest::Close { id: tab.clone() })?;
            Ok(json!({"success":true}))
        })
        .await;
    }
    let owner = key.to_owned();
    let method = method.to_owned();
    main(app,&operation(),move |_|ROUTES.with(|routes|{
        let mut routes=routes.borrow_mut();let r=routes.get_mut(&owner).ok_or("Task closed")?;
        match method.as_str(){
            "Target.setAutoAttach"=>{
                if params["autoAttach"]==true && !r.attached{r.attached=true;for (tab,info) in &r.targets{let _=r.tx.send(json!({"kind":"cdp","message":{"method":"Target.attachedToTarget","params":{"sessionId":session(tab,""),"targetInfo":info,"waitingForDebugger":false}}}));}}
                Ok(json!({}))
            }
            "Target.getTargetInfo"=>{
                if let Some(target)=params["targetId"].as_str(){r.targets.values().find(|info|info["targetId"]==target).cloned().map(|info|json!({"targetInfo":info})).ok_or("Unknown targetId".into())}
                else{Ok(json!({"targetInfo":{"targetId":"vivian-task","type":"browser","title":"Vivian task","url":"","attached":true}}))}
            }
            "Target.getTargets"=>Ok(json!({"targetInfos":r.targets.values().collect::<Vec<_>>()})),
            "Target.setDiscoverTargets"=>Ok(json!({})),
            _=>Err(format!("Browser command is outside this task: {method}")),
        }
    })).await
}

struct Receipt {
    signature: String,
    value: Value,
}
struct State {
    ready: bool,
    alive: bool,
    active: Option<String>,
    receipts: HashMap<String, Receipt>,
    updated: Instant,
}
struct Runtime {
    key: String,
    tab: String,
    artifacts: PathBuf,
    tx: mpsc::UnboundedSender<Value>,
    stop: Arc<Notify>,
    state: Arc<parking_lot::Mutex<State>>,
}
static TASKS: once_cell::sync::Lazy<tokio::sync::Mutex<HashMap<String, Arc<Runtime>>>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(HashMap::new()));
fn runtime_paths(app: &tauri::AppHandle) -> Result<(PathBuf, PathBuf), String> {
    let installed =
        node_path(app.path().resource_dir().map_err(|e| e.to_string())?).join("browser-runtime");
    #[cfg(debug_assertions)]
    {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let host = [
            manifest.clone(),
            manifest.parent().unwrap_or(&manifest).to_owned(),
        ]
        .into_iter()
        .map(|p| p.join("src/embedded_browser/runtime/host.mjs"))
        .find(|p| p.is_file());
        let node = std::env::var_os("PATH")
            .into_iter()
            .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .map(|p| p.join("node.exe"))
            .find(|p| p.is_file());
        if let (Some(node), Some(host)) = (node, host) {
            return Ok((node, host));
        }
    }
    if installed.join("node.exe").is_file() && installed.join("host.mjs").is_file() {
        return Ok((installed.join("node.exe"), installed.join("host.mjs")));
    }
    Err("Original Playwright runtime is missing. Build/package browser-runtime before using browser_execute".into())
}
async fn launch(app: tauri::AppHandle, key: String, tab: String) -> Result<Arc<Runtime>, String> {
    let (node, host) = runtime_paths(&app)?;
    let artifacts = root()
        .join("automation")
        .join(uuid::Uuid::new_v4().to_string());
    tokio::fs::create_dir_all(&artifacts)
        .await
        .map_err(|e| e.to_string())?;
    let (tx, mut incoming) = mpsc::unbounded_channel::<Value>();
    let state = Arc::new(parking_lot::Mutex::new(State {
        ready: false,
        alive: true,
        active: None,
        receipts: HashMap::new(),
        updated: Instant::now(),
    }));
    let stop = Arc::new(Notify::new());
    let runtime = Arc::new(Runtime {
        key: key.clone(),
        tab: tab.clone(),
        artifacts: artifacts.clone(),
        tx: tx.clone(),
        stop: stop.clone(),
        state: state.clone(),
    });
    let owner = key.clone();
    let sender = tx.clone();
    main(&app, &operation(), move |_| {
        ROUTES.with(|r| {
            r.borrow_mut().insert(
                owner,
                Route {
                    tx: sender,
                    targets: HashMap::new(),
                    sessions: HashMap::new(),
                    subscriptions: Vec::new(),
                    attached: false,
                },
            );
        });
        Ok(())
    })
    .await?;
    if let Err(error) = attach(&app, &key, &tab, None).await {
        release(&app, key).await;
        return Err(error);
    }
    let owner = key.clone();
    // Full JavaScript execution is classified Shell, not InputControl. Workers
    // are used for termination; they are never represented as OS security sandboxes.
    let mut command = tokio::process::Command::new(node);
    command
        .arg(&host)
        .arg(&artifacts)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear();
    for name in ["SystemRoot", "WINDIR", "PATH", "TEMP", "TMP"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.creation_flags(0x08000000);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            release(&app, owner).await;
            return Err(error.to_string());
        }
    };
    if std::env::var_os("VIVIAN_PLAYWRIGHT_TRACE").is_some() {
        eprintln!(
            "Playwright runtime {}: {}",
            child.id().unwrap_or(0),
            host.display()
        );
    }
    if let Some(stderr) = child.stderr.take() {
        let state = state.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if std::env::var_os("VIVIAN_PLAYWRIGHT_TRACE").is_some() {
                    eprintln!(
                        "Playwright runtime stderr: {}",
                        line.chars().take(1000).collect::<String>()
                    );
                }
                let mut s = state.lock();
                for receipt in s.receipts.values_mut() {
                    if matches!(receipt.value["status"].as_str(), Some("queued" | "running")) {
                        receipt.value["runtime_error"] =
                            json!(line.chars().take(1000).collect::<String>());
                    }
                }
            }
        });
    }
    let mut input = child.stdin.take().ok_or("Runtime stdin is unavailable")?;
    let output = child.stdout.take().ok_or("Runtime stdout is unavailable")?;
    let writer = tokio::spawn(async move {
        while let Some(value) = incoming.recv().await {
            let line = value.to_string() + "\n";
            if input.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    // Playwright can pipeline mouse move/down/up. Preserve their wire order:
    // independently spawned futures can otherwise release before pressing.
    let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let input_app = app.clone();
    let input_key = key.clone();
    let input_output = tx.clone();
    let input_worker = tokio::spawn(async move {
        while let Some(command) = input_rx.recv().await {
            let response = route(input_app.clone(), input_key.clone(), command).await;
            let _ = input_output.send(json!({"kind":"cdp","message":response}));
        }
    });
    tokio::spawn(async move {
        let mut lines = BufReader::new(output).lines();
        let mut idle = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _=stop.notified()=>break,
                _=idle.tick()=>{let s=state.lock();if s.active.is_none() && s.updated.elapsed()>Duration::from_secs(90){break;}},
                line=lines.next_line()=>{
                    let Ok(Some(line))=line else{break;};
                    if line.len()>4*1024*1024{break;}
                    let Ok(message)=serde_json::from_str::<Value>(&line) else{break;};
                    if std::env::var_os("VIVIAN_PLAYWRIGHT_TRACE").is_some(){eprintln!("Playwright runtime message: {}",message["kind"]);}
                    match message["kind"].as_str(){
                        Some("cdp")=>{
                            let app=app.clone();let key=key.clone();let tx=tx.clone();let command=message["message"].clone();
                            if command["method"].as_str().is_some_and(|m| m.starts_with("Input.")) {
                                let _ = input_tx.send(command);
                            } else {
                                tokio::spawn(async move{let response=route(app,key,command).await;let _=tx.send(json!({"kind":"cdp","message":response}));});
                            }
                        }
                        Some("ready")=>state.lock().ready=true,
                        Some("receipt")=>{
                            let mut s=state.lock();let id=message["id"].as_str().unwrap_or("");
                            if s.active.as_deref()==Some(id){
                                if let Some(receipt)=s.receipts.get_mut(id){receipt.value=message.clone();receipt.value["request_id"]=json!(id);receipt.value["mutation_state"]=json!("possible");}
                                s.active=None;s.updated=Instant::now();
                            }
                        }
                        Some("fatal")=>{let mut s=state.lock();for receipt in s.receipts.values_mut(){if matches!(receipt.value["status"].as_str(),Some("queued"|"running")){receipt.value["error"]=message["error"].clone();}}break;}
                        _=>{}
                    }
                }
            }
        }
        input_worker.abort();
        writer.abort();
        let _ = child.kill().await;
        {
            let mut s = state.lock();
            s.alive = false;
            s.active = None;
            for receipt in s.receipts.values_mut() {
                if matches!(receipt.value["status"].as_str(), Some("running" | "queued")) {
                    receipt.value["status"] = json!("interrupted");
                    receipt.value["mutation_state"] = json!("possible");
                    receipt.value["next_step"]=json!("Runtime ended. Inspect the page and business records; do not replay mutations");
                }
            }
        }
        release(&app, key).await;
    });
    Ok(runtime)
}
pub(crate) async fn execute(
    app: tauri::AppHandle,
    args: Value,
    owner: String,
) -> Result<String, String> {
    let task = args["task_id"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 100)
        .ok_or("task_id is required (1..100 bytes)")?;
    let key = format!("{owner}/{task}");
    if args["action"] == "finish" {
        let task = TASKS
            .lock()
            .await
            .get(&key)
            .cloned()
            .ok_or("Unknown browser task")?;
        task.stop.notify_one();
        return Ok(
            json!({"task_id":task.key,"status":"finishing","pages_preserved":true}).to_string(),
        );
    }
    let request = args["request_id"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 100)
        .ok_or("request_id is required (1..100 bytes)")?
        .to_string();
    let code = args["code"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 64000)
        .ok_or("code is required (up to 64000 bytes)")?
        .to_string();
    let timeout = args["timeout_ms"]
        .as_u64()
        .unwrap_or(30000)
        .clamp(1000, 120000);
    let signature = json!({"code":code,"timeout_ms":timeout}).to_string();
    let runtime = {
        let mut tasks = TASKS.lock().await;
        tasks.retain(|_, r| {
            let state = r.state.lock();
            state.updated.elapsed() < Duration::from_secs(900) || state.alive
        });
        if let Some(runtime) = tasks.get(&key) {
            runtime.clone()
        } else {
            if tasks.values().filter(|r| r.state.lock().alive).count() >= 4 || tasks.len() >= 64 {
                return Err("Browser task capacity reached; finish unused tasks first".into());
            }
            let tab = args["tab_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("First call requires tab_id from browser_tabs")?
                .to_string();
            let runtime = launch(app, key.clone(), tab).await?;
            tasks.insert(key.clone(), runtime.clone());
            runtime
        }
    };
    let submit = {
        let mut state = runtime.state.lock();
        state.updated = Instant::now();
        if args["tab_id"]
            .as_str()
            .is_some_and(|tab| tab != runtime.tab)
        {
            return Err("Task is already bound to another page; use a new task_id".into());
        }
        if let Some(receipt) = state.receipts.get(&request) {
            if receipt.signature != signature {
                return Err("request_id already identifies different code; query browser_receipt before deciding how to continue".into());
            }
            false
        } else {
            if !state.alive {
                return Err("Task runtime ended. Inspect retained receipts and pages; use a new task_id only after reconciliation".into());
            }
            if state.active.is_some() {
                return Err(
                    "A program is already running; query its receipt before another submission"
                        .into(),
                );
            }
            if state.receipts.len() >= 64 {
                return Err(
                    "Task receipt limit reached; finish and reconcile before creating another task"
                        .into(),
                );
            }
            state.active = Some(request.clone());
            state.receipts.insert(request.clone(),Receipt{signature,value:json!({"request_id":request,"task_id":task,"status":"queued","goal_verified":false,"mutation_state":"possible","next_step":"Query browser_receipt using this task_id and request_id; do not resubmit input"})});
            true
        }
    };
    if submit {
        let runtime = runtime.clone();
        let id = request.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            loop {
                let (ready, alive) = {
                    let s = runtime.state.lock();
                    (s.ready, s.alive)
                };
                if ready {
                    break;
                }
                if !alive || started.elapsed() > Duration::from_secs(20) {
                    runtime.stop.notify_one();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            {
                let mut s = runtime.state.lock();
                if let Some(receipt) = s.receipts.get_mut(&id) {
                    receipt.value["status"] = json!("running");
                }
            }
            let _ = runtime
                .tx
                .send(json!({"kind":"execute","id":id,"code":code}));
            tokio::time::sleep(Duration::from_millis(timeout)).await;
            if runtime.state.lock().active.as_deref() == Some(&id) {
                runtime.stop.notify_one();
            }
        });
    }
    receipt(
        &runtime,
        &request,
        args["wait_ms"].as_u64().unwrap_or(1000).min(10000),
    )
    .await
}
pub(crate) async fn query(args: Value, owner: String) -> Result<String, String> {
    let task = args["task_id"].as_str().ok_or("task_id is required")?;
    let request = args["request_id"]
        .as_str()
        .ok_or("request_id is required")?;
    let runtime = TASKS
        .lock()
        .await
        .get(&format!("{owner}/{task}"))
        .cloned()
        .ok_or("Task receipt unavailable. Inspect the application before retrying")?;
    receipt(
        &runtime,
        request,
        args["wait_ms"].as_u64().unwrap_or(1000).min(10000),
    )
    .await
}
async fn receipt(runtime: &Runtime, request: &str, wait: u64) -> Result<String, String> {
    let until = Instant::now() + Duration::from_millis(wait);
    let mut value = loop {
        let receipt = runtime
            .state
            .lock()
            .receipts
            .get(request)
            .map(|r| r.value.clone())
            .ok_or("Unknown request_id; no program was executed by this query")?;
        if !matches!(receipt["status"].as_str(), Some("running" | "queued"))
            || Instant::now() >= until
        {
            break receipt;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let mut images = Vec::new();
    for image in value["images"].as_array().into_iter().flatten().take(4) {
        let Some(path) = image["path"].as_str().map(PathBuf::from) else {
            continue;
        };
        let Ok(path) = tokio::fs::canonicalize(path).await else {
            continue;
        };
        let Ok(root) = tokio::fs::canonicalize(&runtime.artifacts).await else {
            continue;
        };
        if !path.starts_with(&root) {
            continue;
        }
        if let Ok(data) = tokio::fs::read(path).await {
            if data.len() <= 12 * 1024 * 1024 {
                use base64::Engine;
                images.push(json!({"media_type":image["media_type"],"data":base64::engine::general_purpose::STANDARD.encode(data)}));
            }
        }
    }
    value["_images"] = json!(images);
    Ok(value.to_string())
}
