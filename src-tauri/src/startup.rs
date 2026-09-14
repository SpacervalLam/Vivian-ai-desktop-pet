//! 启动预检：在主 LLM / 嵌入服务配置完成前不进入 Brain 初始化，
//! 并为本地 Ollama 嵌入服务提供“先启动服务、再继续加载”的顺序保证。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

use crate::memory::ollama_service::OllamaServiceManager;
use crate::state::AppState;

/// 启动进度全局句柄：启动流程中由 lib.rs 注入，供各初始化阶段发送统一进度事件。
static STARTUP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 是否处于启动/重初始化流程中，避免普通后台操作误触发启动进度 toast。
static STARTUP_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// 最近一次上报的进度快照：供 Toast 窗口挂载时拉取补齐（事件先于监听注册发出时不丢进度）。
static LAST_PROGRESS: Lazy<RwLock<Option<ProgressState>>> = Lazy::new(|| RwLock::new(None));

/// 最近一次上报的百分比：多角色阶段可能回退报数（如第二角色种子嵌入报 60），
/// 钳制为单调递增，避免前端进度条来回跳。
static LAST_PERCENT: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Serialize)]
pub struct ProgressState {
    pub current: usize,
    pub total: usize,
    pub stage: String,
}

/// 标记启动流程开始。
pub fn begin_startup() {
    STARTUP_IN_PROGRESS.store(true, Ordering::SeqCst);
    LAST_PERCENT.store(0, Ordering::SeqCst);
    *LAST_PROGRESS.write() = None;
}

/// 标记启动流程结束。
pub fn finish_startup() {
    STARTUP_IN_PROGRESS.store(false, Ordering::SeqCst);
}

/// 注入启动进度事件使用的 AppHandle（在 lib.rs setup 中调用一次）。
pub fn set_app_handle(handle: AppHandle) {
    *STARTUP_HANDLE.write() = Some(handle);
}

/// 当前已上报的百分比（未上报过为 0）。供嵌入阶段以当前进度为基点做区间映射。
pub fn last_percent() -> usize {
    LAST_PERCENT.load(Ordering::SeqCst)
}

/// 当前进度快照（含是否仍处于启动流程中），供前端挂载时拉取。
pub fn progress_snapshot() -> (bool, Option<ProgressState>) {
    (
        STARTUP_IN_PROGRESS.load(Ordering::SeqCst),
        LAST_PROGRESS.read().clone(),
    )
}

/// 当前是否仍处于启动流程中（供周期性重发循环判断退出）。
pub fn is_startup_in_progress() -> bool {
    STARTUP_IN_PROGRESS.load(Ordering::SeqCst)
}

/// 重发最近一次进度快照（供启动期间的周期性重发循环使用）。
/// 前端挂载可能晚于最初的事件广播（WebView2 加载慢），快照拉取 +
/// 周期重发双保险，确保 toast 窗口任意时刻就绪都能显示当前进度。
///
/// 持读锁跨 emit：与 emit_progress 的写锁互斥。否则「读到新快照 → 释放锁 →
/// 被 emit_progress 插队 → emit 旧值」会让前端先收到新进度再收到旧进度，
/// 同一条 toast 的文本在任务间来回回跳。
pub fn resend_last_progress(handle: &AppHandle) {
    let progress = LAST_PROGRESS.read();
    if let Some(p) = progress.as_ref() {
        let _ = handle.emit(
            "startup:progress",
            serde_json::json!({
                "current": p.current,
                "total": p.total,
                "stage": p.stage,
            }),
        );
    }
}

/// 发送统一启动进度事件 `startup:progress`。
///
/// 前端 ToastWindow 会使用同一个持久 toast 展示所有启动阶段，避免多条 toast 堆积。
/// 进度同时写入快照（供后挂载的前端拉取），百分比钳制为单调递增。
/// 持写锁跨 emit：与 resend_last_progress 的读锁互斥，保证「更新+广播」全局有序。
pub fn emit_progress(current: usize, total: usize, stage: &str) {
    if !STARTUP_IN_PROGRESS.load(Ordering::SeqCst) {
        return;
    }
    let ceiling = total.max(1);
    let clamped = current.max(LAST_PERCENT.load(Ordering::SeqCst)).min(ceiling);
    LAST_PERCENT.store(clamped, Ordering::SeqCst);
    let mut progress = LAST_PROGRESS.write();
    *progress = Some(ProgressState {
        current: clamped,
        total,
        stage: stage.to_string(),
    });
    if let Some(handle) = STARTUP_HANDLE.read().as_ref() {
        let _ = handle.emit(
            "startup:progress",
            serde_json::json!({
                "current": clamped,
                "total": total,
                "stage": stage,
            }),
        );
    }
}

/// 启动预检。
///
/// 流程：
/// 1. 检查主 LLM 与嵌入服务配置——任一未配置则立即打开设置窗口展示配置指引，
///    返回 `false` 阻断初始化（嵌入预加载延后到用户在设置中保存配置、
///    `reinitialize` 触发时执行）；
/// 2. 嵌入配置为本地 Ollama 时立即启动服务并等待 HTTP API 就绪、确保模型可用，
///    就绪后才放行 `state.initialize()`（嵌入任务在 Ollama 可用后才开始）；
/// 3. 嵌入配置为云端 API 时不启动任何本地服务，直接放行。
///
/// 返回 `true` 表示可以继续 `state.initialize()`；`false` 表示需要用户先完成设置。
pub async fn preflight(handle: &AppHandle, state: &Arc<AppState>) -> bool {
    let cfg = state.config.read().get_all();

    emit_progress(0, 100, "正在检查主 LLM 与嵌入服务配置…");

    // 1. 主 LLM 是否已配置
    let main_llm_ok = !cfg.ai.model.trim().is_empty()
        && (cfg
            .ai
            .api_key
            .as_deref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
            || cfg
                .ai
                .api_secret
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false)
            || cfg
                .ai
                .app_id
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false));

    if !main_llm_ok {
        tracing::info!("[Startup] 主 LLM 未配置，打开设置引导，暂不初始化角色");
        emit_progress(100, 100, "请先在设置中配置主 LLM");
        open_config_with_guide(handle);
        return false;
    }

    // 2. 嵌入服务配置状态（local 需路径+模型；云端需 API Key + Endpoint + 模型）
    let emb = cfg.memory.embedding.clone();
    let embedding_ok = if emb.source == "local" {
        !emb.ollama_path.trim().is_empty() && !emb.ollama_model.trim().is_empty()
    } else {
        !emb.api_key.trim().is_empty()
            && !emb.endpoint.trim().is_empty()
            && !emb.model.trim().is_empty()
    };

    if !embedding_ok {
        tracing::info!(
            "[Startup] 嵌入服务（source={}）未配置，打开设置引导，暂不初始化角色",
            emb.source
        );
        emit_progress(100, 100, "请先在设置中配置嵌入服务");
        open_config_with_guide(handle);
        return false;
    }

    // 3. 本地 Ollama 嵌入：立即启动服务并等待就绪，再继续初始化；
    //    云端嵌入不启动任何本地服务
    if emb.source == "local" {
        tracing::info!(
            "[Startup] 本地 Ollama 嵌入已配置，先启动服务并检查模型 {}",
            emb.ollama_model
        );
        emit_progress(10, 100, "正在启动 Ollama 嵌入服务…");
        let svc = crate::memory::ollama_service::ollama_service().await;
        if let Err(e) = svc.start(&emb.ollama_path).await {
            tracing::error!("[Startup] 启动 Ollama 失败: {}", e);
            emit_progress(100, 100, "Ollama 启动失败，请检查设置");
            open_config_with_guide(handle);
            return false;
        }

        emit_progress(20, 100, "正在检查/拉取 Ollama 嵌入模型…");
        // ensure_model_installed 内部等待 HTTP API 就绪（GET /v1/models 可解析）
        // 后才返回 true，保证紧随其后的嵌入任务不会因服务未就绪而失败
        let model_ready = OllamaServiceManager::ensure_model_installed(
            &emb.ollama_model,
            &emb.ollama_path,
        )
        .await;
        if !model_ready {
            tracing::error!(
                "[Startup] Ollama 模型 {} 未就绪，暂不初始化角色",
                emb.ollama_model
            );
            emit_progress(100, 100, "Ollama 模型未就绪，请检查设置");
            open_config_with_guide(handle);
            return false;
        }
        tracing::info!("[Startup] Ollama 嵌入服务与模型已就绪");
        emit_progress(35, 100, "Ollama 嵌入服务与模型已就绪");
    }

    emit_progress(40, 100, "启动预检完成，准备初始化角色…");
    true
}

/// 创建 startup_toast 窗口失败后的剩余重试次数（偶发 WebView2 ERROR_BUSY 时退避重试）。
/// 快速重启场景下上一实例的 WebView2 子进程退出需要数秒，重试窗口需覆盖该周期。
static TOAST_RETRY_LEFT: AtomicUsize = AtomicUsize::new(10);

/// 创建专用的启动进度 toast 窗口。
///
/// 普通角色窗口尚未创建时，也需要一个 ToastWindow 来显示启动进度；
/// 该窗口使用 `character_id=startup`，只处理 `startup:progress`，不会干扰角色 toast。
/// 与普通角色 toast 窗口对齐：置顶、定位屏幕右上角、高度撑满屏幕、隐藏任务栏且不抢焦点，
/// 避免进度条被其他窗口遮挡或停在不预期位置导致用户完全看不到。
/// 创建失败（如与其他窗口的 WebView2 初始化并发触发 ERROR_BUSY）时延迟重试，
/// 确保启动进度 toast 一定能出现。
pub fn ensure_startup_toast() {
    let Some(handle) = STARTUP_HANDLE.read().as_ref().cloned() else {
        return;
    };
    if handle.get_webview_window("startup_toast").is_some() {
        TOAST_RETRY_LEFT.store(0, Ordering::SeqCst);
        return;
    }
    // 主显示器尺寸（逻辑像素），用于撑满屏幕高度并定位右上角
    // 宽度收窄到 360：startup 窗口内容锚定右上（toast 宽 ~360），贴右缘更自然，
    // 且与角色 toast 窗口（400px 宽、右下锚定）错开，互不遮盖
    let (pos_x, height) = handle
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let factor = m.scale_factor();
            let w = m.size().width as f64 / factor;
            let h = m.size().height as f64 / factor;
            (w - 360.0, h)
        })
        .unwrap_or((0.0, 600.0));
    match WebviewWindowBuilder::new(
        &handle,
        "startup_toast",
        WebviewUrl::App("index.html?view=toast&character_id=startup".into()),
    )
    .title("Vivian Startup")
    .inner_size(360.0, height)
    .position(pos_x, 0.0)
    .resizable(false)
    .transparent(true)
    .decorations(false)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    .build()
    {
        Ok(win) => {
            TOAST_RETRY_LEFT.store(0, Ordering::SeqCst);
            // 点击穿透：进度 toast 无交互元素，避免 400px 宽的全高窗口遮挡屏幕右上区域的鼠标操作
            let _ = win.set_ignore_cursor_events(true);
            // 显示兜底：lib.rs 的 show_startup_toast() 早于本窗口创建（no-op），
            // 窗口可见性完全依赖前端挂载后的 hasContent effect，而 WebView/vite
            // 首屏可能数秒甚至数十秒——窗口可能迟到。此处等前端大概率已完成
            // React 挂载后主动 show，并重发最新进度快照，让窗口一出现即有内容。
            let win2 = win.clone();
            let handle2 = handle.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(600)).await;
                let _ = win2.show();
                resend_last_progress(&handle2);
            });
        }
        Err(e) => {
            tracing::error!("[Startup] 创建启动进度 Toast 窗口失败: {}", e);
            let left = TOAST_RETRY_LEFT.fetch_sub(1, Ordering::SeqCst);
            if left > 0 {
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(800)).await;
                    ensure_startup_toast();
                });
            }
        }
    }
}

/// 显示启动进度 Toast 窗口（后端兜底）。
///
/// 前端挂载后通过 hasContent effect 自行 show，这里在握手返回后再 show 一次作为双保险，
/// 避免前端渲染异常时进度条窗口无法出现。窗口未创建或已关闭时静默跳过。
pub fn show_startup_toast() {
    if let Some(handle) = STARTUP_HANDLE.read().as_ref() {
        if let Some(win) = handle.get_webview_window("startup_toast") {
            let _ = win.show();
        }
    }
}


/// 打开设置窗口，并请求其展示配置说明弹窗。
///
/// 若设置窗口已存在则聚焦并广播 `setup-guide:show`；否则直接以
/// `/?view=config&guide=1` 创建，ConfigWindow 挂载时会根据 URL 参数自动弹窗。
pub(crate) fn open_config_with_guide(handle: &AppHandle) {
    const LABEL: &str = "config";
    if let Some(win) = handle.get_webview_window(LABEL) {
        let _ = win.show();
        let _ = win.set_focus();
        let _ = handle.emit("setup-guide:show", ());
        return;
    }

    let _ = WebviewWindowBuilder::new(
        handle,
        LABEL,
        WebviewUrl::App("index.html?view=config&guide=1".into()),
    )
    .title("设置")
    .inner_size(768.0, 624.0)
    .min_inner_size(768.0, 624.0)
    .decorations(false)
    .transparent(false)
    .shadow(true)
    .resizable(true)
    .center()
    .build();
}
