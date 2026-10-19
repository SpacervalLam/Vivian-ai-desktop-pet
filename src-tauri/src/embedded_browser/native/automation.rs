//! Native agent access. CDP stays in process; pages get no host binding or credentials API.
use super::*;
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

const OWNER: &str = "memory";
/// Does not navigate, inject scripts, persist pixels, or expose a debugging port.
pub async fn capture_viewport(app: &tauri::AppHandle, id: &str) -> Result<String, String> {
    let op = Operation { cancelled: Arc::new(AtomicBool::new(false)), deadline: Instant::now() + Duration::from_secs(3) };
    let _cancel = CancelOnDrop(op.clone());
    tokio::time::timeout(Duration::from_secs(3), async {
        let key = id.to_string();
        let generation = main(app, &op, move |host| {
            let h = host.borrow();
            if h.state.active.as_deref() != Some(&key) { return Err("Browser tab is no longer active".into()); }
            h.tabs.get(&key).map(|tab| tab.generation).ok_or("Browser tab is not ready".into())
        }).await?;
        let image = cdp(app, &op, id, "Page.captureScreenshot", json!({"format":"png","fromSurface":true,"captureBeyondViewport":false})).await?;
        let data = image.get("data").and_then(Value::as_str).filter(|data| !data.is_empty() && data.len() <= 24 * 1024 * 1024).ok_or("Browser viewport image is unavailable or too large")?;
        let key = id.to_string();
        main(app, &op, move |host| {
            let h = host.borrow();
            if h.state.active.as_deref() != Some(&key) || !h.tabs.get(&key).is_some_and(|tab| tab.generation == generation) {
                return Err("Browser tab changed during capture".into());
            }
            Ok(())
        }).await?;
        Ok(format!("data:image/png;base64,{data}"))
    }).await.map_err(|_| "Browser viewport capture timed out".to_string())?
}
const PAGE: &str = include_str!("../scripts/page.js");
const EXTRACTORS: &str = include_str!("../scripts/extractors.js");
static TOOL_LOCK: once_cell::sync::Lazy<tokio::sync::Mutex<()>> =
    once_cell::sync::Lazy::new(|| tokio::sync::Mutex::new(()));

#[derive(Clone)]
struct Operation {
    cancelled: Arc<AtomicBool>,
    deadline: Instant,
}
impl Operation {
    fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Relaxed) || Instant::now() >= self.deadline {
            Err("浏览器操作已取消或超时，未继续执行".into())
        } else {
            Ok(())
        }
    }
}
struct CancelOnDrop(Operation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::Relaxed);
    }
}

async fn main<T: Send + 'static>(
    app: &tauri::AppHandle,
    op: &Operation,
    action: impl FnOnce(HostRef) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let app_copy = app.clone();
    let op = op.clone();
    app.run_on_main_thread(move || {
        if tx.is_closed() {
            return;
        }
        let result = op
            .check()
            .and_then(|_| initialize(app_copy, OWNER.into()))
            .and_then(action);
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "浏览器主线程响应已关闭".to_string())?
}

async fn cdp(
    app: &tauri::AppHandle,
    op: &Operation,
    id: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender = Arc::new(std::sync::Mutex::new(Some(tx)));
    let callback_sender = sender.clone();
    let app_copy = app.clone();
    let op = op.clone();
    let id = id.to_string();
    let method = method.to_string();
    app.run_on_main_thread(move || {
        let result = (|| {
            op.check()?;
            if sender
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|tx| tx.is_closed())
            {
                return Err("浏览器调用已取消".into());
            }
            let host = initialize(app_copy, OWNER.into())?;
            let core = get_core(&host, &id)?;
            unsafe {
                core.CallDevToolsProtocolMethod(
                    PCWSTR(wide(&method).as_ptr()),
                    PCWSTR(wide(&params.to_string()).as_ptr()),
                    &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                        move |result, body| {
                            let result = result.map_err(|e| e.to_string()).and_then(|_| {
                                serde_json::from_str(&body).map_err(|e| e.to_string())
                            });
                            if let Some(tx) = callback_sender.lock().unwrap().take() {
                                let _ = tx.send(result);
                            }
                            Ok(())
                        },
                    )),
                )
                .map_err(|e| e.to_string())
            }
        })();
        if let Err(error) = result {
            if let Some(tx) = sender.lock().unwrap().take() {
                let _ = tx.send(Err(error));
            }
        }
    })
    .map_err(|e| e.to_string())?;
    rx.await
        .map_err(|_| "网页已关闭或导航，操作结果不可用；请重新读取页面".to_string())?
}

fn evaluated(value: Value) -> Result<Value, String> {
    if value.get("exceptionDetails").is_some() {
        let error = value
            .pointer("/exceptionDetails/exception/description")
            .or_else(|| value.pointer("/exceptionDetails/text"))
            .and_then(Value::as_str)
            .unwrap_or("网页脚本执行失败");
        return Err(error.chars().take(500).collect());
    }
    Ok(value
        .pointer("/result/value")
        .cloned()
        .unwrap_or(Value::Null))
}
async fn world(app: &tauri::AppHandle, op: &Operation, id: &str) -> Result<i64, String> {
    let key = id.to_string();
    let (cached, generation) = main(app, op, move |h| {
        let h = h.borrow();
        let t = h.tabs.get(&key).ok_or("标签页尚未就绪")?;
        Ok((t.world, t.generation))
    })
    .await?;
    if let Some(cached) = cached {
        return Ok(cached);
    }
    let tree = cdp(app, op, id, "Page.getFrameTree", json!({})).await?;
    let frame = tree
        .pointer("/frameTree/frame/id")
        .and_then(Value::as_str)
        .ok_or("无法定位网页主框架")?;
    let created = cdp(
        app,
        op,
        id,
        "Page.createIsolatedWorld",
        json!({"frameId":frame,"worldName":"Vivian agent","grantUniveralAccess":false}),
    )
    .await?;
    let context = created
        .get("executionContextId")
        .and_then(Value::as_i64)
        .ok_or("无法创建网页隔离上下文")?;
    let expression = format!(
        "(()=>{{{EXTRACTORS}\n{PAGE}\nglobalThis.vivianPageSessions=new Map();return true;}})()"
    );
    evaluated(cdp(app,op,id,"Runtime.evaluate",json!({"expression":expression,"contextId":context,"returnByValue":true,"awaitPromise":true})).await?)?;
    let key = id.to_string();
    main(app, op, move |h| {
        let mut h = h.borrow_mut();
        let t = h.tabs.get_mut(&key).ok_or("标签页已关闭")?;
        if t.generation != generation {
            return Err("网页已导航，请重新读取页面".into());
        }
        t.world = Some(context);
        Ok(())
    })
    .await?;
    Ok(context)
}
async fn page(
    app: &tauri::AppHandle,
    op: &Operation,
    id: &str,
    session: &str,
    action: &str,
    args: &Value,
) -> Result<Value, String> {
    let context = world(app, op, id).await?;
    let payload = json!([session, action, args]).to_string();
    let expression = format!("(()=>{{const [session,action,args]={payload};let run=vivianPageSessions.get(session);if(!run){{run=vivianCreatePage();vivianPageSessions.set(session,run);}}return run(action,args);}})()");
    evaluated(cdp(app,op,id,"Runtime.evaluate",json!({"expression":expression,"contextId":context,"returnByValue":true,"awaitPromise":true,"userGesture":true})).await?)
}

pub(crate) fn http_url(input: &str) -> Result<String, String> {
    let url = tauri::Url::parse(input.trim()).map_err(|_| "需要完整的 HTTP(S) 网址".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("仅支持不含账号密码的 HTTP(S) 网址".into());
    }
    Ok(url.into())
}
async fn ready(app: &tauri::AppHandle, op: &Operation) -> Result<(), String> {
    loop {
        let ready = main(app, op, |h| {
            let h = h.borrow();
            if h.environment.is_none() {
                if let Some(error) = &h.state.error {
                    return Err(error.clone());
                }
            }
            Ok(h.environment.is_some() && h.cookies.is_some())
        })
        .await?;
        if ready {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
async fn settled(
    app: &tauri::AppHandle,
    op: &Operation,
    id: &str,
    extra: u64,
) -> Result<(), String> {
    loop {
        let key = id.to_string();
        let loading = main(app, op, move |h| {
            let h = h.borrow();
            let tab = h.tab(&key).ok_or("标签页已关闭")?;
            if let Some(error) = &tab.error {
                return Err(error.clone());
            }
            Ok(tab.loading || !h.tabs.contains_key(&key))
        })
        .await?;
        if !loading {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if extra > 0 {
        tokio::time::sleep(Duration::from_millis(extra)).await;
    }
    op.check()
}
fn wrap(text: &str) -> String {
    let nonce = uuid::Uuid::new_v4();
    format!(
        "<UNTRUSTED_PAGE_CONTENT nonce=\"{nonce}\">\n{}\n</UNTRUSTED_PAGE_CONTENT>",
        text.chars().take(32000).collect::<String>()
    )
}
fn page_text(result: Value) -> String {
    let mut text = result
        .get("text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| result.to_string());
    if let Some(delta) = result.get("pageContent").and_then(Value::as_str) {
        text.push('\n');
        text.push_str(delta);
    }
    wrap(&text)
}

struct TaskTab {
    app: tauri::AppHandle,
    id: String,
}
struct FocusEmulation {
    app: tauri::AppHandle,
    id: String,
    active: bool,
}
fn restore_input_surface(h: &HostRef, id: &str) -> Result<(), String> {
    let target = {
        let mut host = h.borrow_mut();
        let hwnd = host.surface_hwnd;
        let Some(tab) = host.tabs.get_mut(id) else {
            return Ok(());
        };
        if !tab.input_parked {
            return Ok(());
        }
        tab.input_parked = false;
        (tab.controller.clone(), hwnd)
    };
    unsafe {
        target.0.SetIsVisible(false).map_err(|e| e.to_string())?;
        target
            .0
            .SetParentWindow(target.1)
            .map_err(|e| e.to_string())?;
    }
    apply_bounds(h).map_err(|e| e.to_string())
}
impl Drop for FocusEmulation {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let id = self.id.clone();
        let _ = self.app.run_on_main_thread(move || {
            if let Some(h) = HOSTS.with(|hosts| hosts.borrow().get(OWNER).cloned()) {
                let _ = restore_input_surface(&h, &id);
                if let Ok(core) = get_core(&h, &id) {
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
            }
        });
    }
}
impl Drop for TaskTab {
    fn drop(&mut self) {
        let id = self.id.clone();
        let _ = self.app.run_on_main_thread(move || {
            if let Some(h) = HOSTS.with(|hosts| hosts.borrow().get(OWNER).cloned()) {
                let native = {
                    let mut h = h.borrow_mut();
                    h.tasks.remove(&id);
                    h.tabs.remove(&id)
                };
                if let Some(tab) = native {
                    unsafe {
                        let _ = tab.controller.Close();
                    }
                }
                publish(&h, false);
            }
        });
    }
}
async fn task(
    app: &tauri::AppHandle,
    op: &Operation,
    args: &Value,
    history: bool,
) -> Result<String, String> {
    if args.get("code").is_some() {
        return Err("后台任务使用内置提取器，不接受任意代码".into());
    }
    let url = http_url(args.get("url").and_then(Value::as_str).ok_or("缺少 url")?)?;
    let extractor = args.get("extractor").and_then(Value::as_str);
    if args.get("extractor").is_some()
        && !matches!(
            extractor,
            Some("bilibili" | "xiaohongshu" | "douyin" | "zhihu" | "images")
        )
    {
        return Err("未知平台提取器".into());
    }
    let wait = match args.get("waitMs") {
        None => 800,
        Some(v) => v
            .as_u64()
            .filter(|v| *v <= 10000)
            .ok_or("waitMs 必须为 0–10000 的整数")?,
    };
    let tab = BrowserTab::new(url, false);
    let id = tab.id.clone();
    // Own cleanup before enqueuing creation: cancellation during initialization cannot orphan a tab.
    let _cleanup = TaskTab {
        app: app.clone(),
        id: id.clone(),
    };
    main(app, op, move |h| {
        h.borrow_mut().tasks.insert(tab.id.clone(), tab.clone());
        publish(&h, false);
        create_tab(&h, tab, None).map_err(|e| e.to_string())
    })
    .await?;
    settled(app, op, &id, wait).await?;
    let result = page(
        app,
        op,
        &id,
        "background",
        if history {
            "browser_bilibili_history"
        } else if extractor.is_some() {
            "browser_extract"
        } else {
            "browser_snapshot"
        },
        args,
    )
    .await?;
    Ok(page_text(result))
}

async fn run(
    app: &tauri::AppHandle,
    op: &Operation,
    name: &str,
    args: &Value,
    session: &str,
) -> Result<String, String> {
    ready(app, op).await?;
    if name == "browser_task_tab" {
        return task(app, op, args, false).await;
    }
    if name == "browser_open_tab" {
        let url = http_url(args.get("url").and_then(Value::as_str).ok_or("缺少 url")?)?;
        main(app, op, move |h| {
            operate(
                &h,
                BrowserRequest::NewTab {
                    input: url,
                    private: false,
                },
            )
        })
        .await?;
        return Ok("已在内嵌浏览器打开页面，请手动完成登录。".into());
    }
    let key = session.to_string();
    let explicit = args
        .get("tab_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let snapshot = name == "browser_snapshot";
    let needs_snapshot = matches!(name, "browser_click" | "browser_type" | "browser_press");
    let id = main(app, op, move |h| {
        let h = h.borrow();
        let selection = h.selections.get(&key);
        let id = explicit
            .or_else(|| {
                if snapshot {
                    h.state.active.clone()
                } else {
                    selection
                        .map(|(id, _)| id.clone())
                        .or_else(|| h.state.active.clone())
                }
            })
            .ok_or("请先在内嵌浏览器中打开网页")?;
        let native = h.tabs.get(&id).ok_or("标签页尚未就绪")?;
        if h.tab(&id).is_none() {
            return Err("标签页已关闭，请重新读取页面".into());
        }
        if needs_snapshot
            && !selection
                .is_some_and(|(old, generation)| *old == id && *generation == native.generation)
        {
            return Err("操作目标缺少有效快照或已发生导航，请先 browser_snapshot".into());
        }
        Ok(id)
    })
    .await?;
    if name == "browser_bilibili_history" {
        let key = id.clone();
        let url = main(app, op, move |h| {
            Ok(h.borrow().tab(&key).ok_or("标签已关闭")?.url.clone())
        })
        .await?;
        let url = tauri::Url::parse(&url).map_err(|e| e.to_string())?;
        if !url
            .host_str()
            .is_some_and(|host| host == "bilibili.com" || host.ends_with(".bilibili.com"))
        {
            return Err("当前受控标签页不在 B站".into());
        }
        let limit = args
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(50)
            .clamp(1, 100);
        return task(app,op,&json!({"url":format!("https://api.bilibili.com/x/web-interface/history/cursor?ps={limit}"),"limit":limit,"waitMs":0}),true).await;
    }
    if matches!(
        name,
        "browser_navigate" | "browser_back" | "browser_forward" | "browser_reload"
    ) {
        let action = match name {
            "browser_navigate" => BrowserRequest::Navigate {
                id: id.clone(),
                input: http_url(args.get("url").and_then(Value::as_str).ok_or("缺少 url")?)?,
            },
            _ => BrowserRequest::Action {
                id: id.clone(),
                name: name.trim_start_matches("browser_").into(),
                text: String::new(),
            },
        };
        main(app, op, move |h| operate(&h, action)).await?;
        return Ok(format!(
            "已向标签 {id} 发送导航操作，请重新 browser_snapshot 读取页面。"
        ));
    }
    settled(app, op, &id, 0).await?;
    let key = id.clone();
    let read_generation = main(app, op, move |h| {
        Ok(h.borrow().tabs.get(&key).ok_or("标签页已关闭")?.generation)
    })
    .await?;
    // Focus emulation must cover the preceding DOM focus as well as key dispatch.
    // Calling input.focus() while a hidden renderer is unfocused can be ignored.
    let mut focus = if matches!(name, "browser_click" | "browser_type" | "browser_press") {
        let guard = FocusEmulation {
            app: app.clone(),
            id: id.clone(),
            active: true,
        };
        if name == "browser_press" {
            let target = id.clone();
            main(app, op, move |h| {
                let (controller, hwnd) = {
                    let mut host = h.borrow_mut();
                    let hwnd = host.parked_hwnd;
                    let tab = host.tabs.get_mut(&target).ok_or("标签页已关闭")?;
                    let mut visible = BOOL::default();
                    unsafe {
                        tab.controller
                            .IsVisible(&mut visible)
                            .map_err(|e| e.to_string())?;
                    }
                    if visible.as_bool() {
                        return Ok(());
                    }
                    tab.input_parked = true;
                    (tab.controller.clone(), hwnd)
                };
                // Keep the native input surface alive under an invisible parent. The
                // user's selected tab and OS foreground window remain unchanged.
                unsafe {
                    controller
                        .SetParentWindow(hwnd)
                        .map_err(|e| e.to_string())?;
                    controller
                        .SetBounds(RECT {
                            left: 0,
                            top: 0,
                            right: 1280,
                            bottom: 900,
                        })
                        .map_err(|e| e.to_string())?;
                    controller.SetIsVisible(true).map_err(|e| e.to_string())?;
                }
                Ok(())
            })
            .await?;
        }
        cdp(
            app,
            op,
            &id,
            "Emulation.setFocusEmulationEnabled",
            json!({"enabled":true}),
        )
        .await?;
        Some(guard)
    } else {
        None
    };
    let result = if name == "browser_press" {
        let key = args.get("key").and_then(Value::as_str).ok_or("缺少 key")?;
        let (code, vk) = match key {
            "Enter" => ("Enter", 13),
            "Tab" => ("Tab", 9),
            "Escape" | "Esc" => ("Escape", 27),
            "ArrowLeft" => ("ArrowLeft", 37),
            "ArrowUp" => ("ArrowUp", 38),
            "ArrowRight" => ("ArrowRight", 39),
            "ArrowDown" => ("ArrowDown", 40),
            "Backspace" => ("Backspace", 8),
            "Delete" => ("Delete", 46),
            " " | "Space" => ("Space", 32),
            _ => return Err("不支持的按键，请用 browser_type 输入文字".into()),
        };
        let key = if code == "Space" {
            " "
        } else if code == "Escape" {
            "Escape"
        } else {
            key
        };
        let mut down = json!({"type":"keyDown","key":key,"code":code,"windowsVirtualKeyCode":vk,"nativeVirtualKeyCode":vk,"modifiers":0});
        if vk == 13 {
            down["text"] = json!("\r");
            down["unmodifiedText"] = json!("\r");
        } else if vk == 32 {
            down["text"] = json!(" ");
            down["unmodifiedText"] = json!(" ");
        }
        cdp(app, op, &id, "Input.dispatchKeyEvent", down).await?;
        cdp(
            app,
            op,
            &id,
            "Input.dispatchKeyEvent",
            json!({"type":"keyUp","key":key,"code":code,"windowsVirtualKeyCode":vk,"nativeVirtualKeyCode":vk,"modifiers":0}),
        )
        .await?;
        json!({"text":"已发送原生网页按键，请重新读取页面确认结果。"})
    } else {
        if !matches!(
            name,
            "browser_snapshot"
                | "browser_get_text"
                | "browser_click"
                | "browser_type"
                | "browser_scroll"
                | "browser_wait"
                | "browser_extract"
        ) {
            return Err("未知浏览器工具".into());
        }
        page(app, op, &id, session, name, args).await?
    };
    if let Some(guard) = focus.as_mut() {
        // Wait for restoration before releasing TOOL_LOCK: a queued fire-and-forget
        // disable can otherwise overtake the next tool's enable/key dispatch.
        cdp(
            app,
            op,
            &id,
            "Emulation.setFocusEmulationEnabled",
            json!({"enabled":false}),
        )
        .await?;
        let target = id.clone();
        main(app, op, move |h| restore_input_surface(&h, &target)).await?;
        guard.active = false;
    }
    if snapshot {
        let key = session.to_string();
        let tab_id = id.clone();
        main(app, op, move |h| {
            let mut h = h.borrow_mut();
            let generation = h.tabs.get(&tab_id).ok_or("标签已关闭")?.generation;
            if generation != read_generation {
                return Err("读取过程中网页发生导航，请重新快照".into());
            }
            h.selections.insert(key, (tab_id, generation));
            Ok(())
        })
        .await?;
    }
    Ok(format!("Browser tab: {id}\n{}", page_text(result)))
}

pub(crate) async fn tool(
    app: tauri::AppHandle,
    name: String,
    args: Value,
    timeout: Duration,
    session: String,
) -> Result<String, String> {
    let op = Operation {
        cancelled: Arc::new(AtomicBool::new(false)),
        deadline: Instant::now() + timeout,
    };
    let _cancel = CancelOnDrop(op.clone());
    tokio::time::timeout(timeout, async {
        let _serial = TOOL_LOCK.lock().await;
        op.check()?;
        run(&app, &op, &name, &args, &session).await
    })
    .await
    .map_err(|_| "浏览器操作超时，后台页面已回收；请检查页面或手动完成验证".to_string())?
}

// Credential values never implement Serialize and never enter a browser tool response.
#[derive(Default)]
pub(crate) struct Credentials {
    pub status: HashMap<String, bool>,
    pub x: Option<String>,
    pub reddit: Option<String>,
}
async fn cookies(
    app: &tauri::AppHandle,
    op: &Operation,
    url: &str,
) -> Result<HashMap<String, String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let app_copy = app.clone();
    let url = url.to_string();
    let op = op.clone();
    let sender = Arc::new(std::sync::Mutex::new(Some(tx)));
    let callback = sender.clone();
    app.run_on_main_thread(move || {
        let result = (|| {
            op.check()?;
            let h = initialize(app_copy, OWNER.into())?;
            let manager = h.borrow().cookies.clone().ok_or("浏览器配置尚未就绪")?;
            unsafe {
                manager
                    .GetCookies(
                        PCWSTR(wide(&url).as_ptr()),
                        &GetCookiesCompletedHandler::create(Box::new(move |result, list| {
                            let parsed = (|| {
                                result.map_err(|e| e.to_string())?;
                                let mut values = HashMap::new();
                                if let Some(list) = list {
                                    let mut count = 0;
                                    list.Count(&mut count).map_err(|e| e.to_string())?;
                                    for i in 0..count {
                                        let cookie =
                                            list.GetValueAtIndex(i).map_err(|e| e.to_string())?;
                                        let name = text(|p| cookie.Name(p));
                                        let value = text(|p| cookie.Value(p));
                                        if !value.is_empty() {
                                            values.insert(name, value);
                                        }
                                    }
                                }
                                Ok(values)
                            })();
                            if let Some(tx) = callback.lock().unwrap().take() {
                                let _ = tx.send(parsed);
                            }
                            Ok(())
                        })),
                    )
                    .map_err(|e| e.to_string())
            }
        })();
        if let Err(error) = result {
            if let Some(tx) = sender.lock().unwrap().take() {
                let _ = tx.send(Err(error));
            }
        }
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|_| "浏览器凭据读取已关闭".to_string())?
}
pub(crate) async fn credentials(app: tauri::AppHandle) -> Result<Credentials, String> {
    let op = Operation {
        cancelled: Arc::new(AtomicBool::new(false)),
        deadline: Instant::now() + Duration::from_secs(15),
    };
    let _cancel = CancelOnDrop(op.clone());
    tokio::time::timeout(Duration::from_secs(15), async {
        ready(&app, &op).await?;
        let mut out = Credentials::default();
        for (platform, url, sentinel) in [
            ("bilibili", "https://www.bilibili.com/", "SESSDATA"),
            ("zhihu", "https://www.zhihu.com/", "z_c0"),
            ("xiaohongshu", "https://www.xiaohongshu.com/", "web_session"),
            ("douyin", "https://www.douyin.com/", "sessionid_ss"),
            ("weibo", "https://weibo.com/", "SUB"),
            ("v2ex", "https://www.v2ex.com/", "A2"),
            ("bangumi", "https://bgm.tv/", "chii_auth"),
            ("youtube", "https://www.youtube.com/", "SAPISID"),
            ("twitter", "https://x.com/", "auth_token"),
            ("reddit", "https://www.reddit.com/", "reddit_session"),
        ] {
            let values = cookies(&app, &op, url).await?;
            out.status
                .insert(platform.into(), values.contains_key(sentinel));
            if platform == "twitter" {
                if let (Some(auth), Some(ct0)) = (values.get("auth_token"), values.get("ct0")) {
                    out.x = Some(format!("auth_token={auth}; ct0={ct0}"));
                }
            }
            if platform == "reddit" && values.contains_key(sentinel) {
                out.reddit = Some(
                    values
                        .iter()
                        .map(|(name, value)| format!("{name}={value}"))
                        .collect::<Vec<_>>()
                        .join("; "),
                );
            }
        }
        Ok(out)
    })
    .await
    .map_err(|_| "浏览器凭据检测超时".to_string())?
}
