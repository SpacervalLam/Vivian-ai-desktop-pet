//! 心智观察器（工作智能体的界面窗口）注意力状态。
//!
//! 工作智能体卡在"等用户拍板"时，要不要让陪伴角色开口提醒，取决于用户此刻
//! 是否真的看得见那个提问。这个判断横跨两个世界：
//!
//! - **窗口层**：窗口是否存在、可见、未最小化、处于前台——Rust 直接可查，
//!   而且是查询时现算。窗口状态变化远快于页签，缓存它只会多出一份会漂移的副本。
//! - **界面层**：当前停在哪个页签、激活的是哪个会话——只有前端知道，由
//!   `MindInspector` 与 `CodeAgentPageNew` 上报后写进下面的静态里。
//!
//! 判定一律**保守缺省**：任何一项拿不到（窗口查不到、前端还没上报过、字段为空），
//! 都算"用户看不见"。多提醒一次的代价，远小于让一个后台任务干等三十分钟超时。

use once_cell::sync::Lazy;
use parking_lot::RwLock;
use tauri::{AppHandle, Manager};

/// 心智观察器窗口 label（与前端 `openWindow('memory', 'memory', ...)` 一致）。
pub const INSPECTOR_WINDOW_LABEL: &str = "memory";

/// 工作页的页签 key（前端 `NavKey::Code`）。
const WORK_PAGE: &str = "code";

#[derive(Debug, Clone, Default)]
struct InspectorView {
    /// 当前页签；空串表示前端从未上报过
    nav: String,
    /// 当前激活的工作会话；空串表示未知
    session_id: String,
}

static VIEW: Lazy<RwLock<InspectorView>> = Lazy::new(|| RwLock::new(InspectorView::default()));

/// 前端上报心智观察器的当前页签与激活会话。
///
/// 由两个组件共同调用：`MindInspector` 只知道页签、`CodeAgentPageNew` 只知道
/// 激活会话（且只在工作页挂载）。所以两个参数都是可选的，缺省即保留原值——
/// 谁也不知道对方的全貌，不能互相覆盖成空。
#[tauri::command]
pub fn report_inspector_attention(
    nav: Option<String>,
    session_id: Option<String>,
) -> Result<(), String> {
    let mut v = VIEW.write();
    if let Some(nav) = nav {
        v.nav = nav;
    }
    if let Some(session_id) = session_id {
        v.session_id = session_id;
    }
    Ok(())
}

/// 用户此刻看得见的工作会话；`None` 表示看不见工作页。
///
/// 四个条件必须同时成立：窗口可见、未最小化、处于前台、停在工作页。
/// 只在四者都成立时才有"看得见哪个会话"这回事——否则用户在别的页签或别的应用里，
/// 工作页的 `coding:question` 监听器压根没挂载，提问卡片根本不会出现。
///
/// 返回值刻意做成"会话 id"而不是布尔：调用方要判断的是"用户看不看得见**这一条**
/// 提问"，而角色名下可能同时挂着来自不同会话的提问。拿会话 id 回去比，
/// 比在这里做一次专门判断更省事，也少一个要同步的接口。
pub fn visible_work_session(app: &AppHandle) -> Option<String> {
    if !inspector_window_visible_and_front(app) {
        return None;
    }
    let view = VIEW.read();
    if view.nav != WORK_PAGE || view.session_id.is_empty() {
        return None;
    }
    Some(view.session_id.clone())
}

/// 用户此刻是否正看着工作页（不要求选中了某条会话）。
///
/// 与 [`visible_work_session`] 的分工：那个还要求激活了具体会话，用来判断
/// "用户看不看得见**这一条**提问"；完成报告没有会话归属，只需要知道有没有
/// 看着工作页——用户停在码页却没选会话，仍然算正看着。
pub fn is_viewing_work_page(app: &AppHandle) -> bool {
    if VIEW.read().nav != WORK_PAGE {
        return false;
    }
    inspector_window_visible_and_front(app)
}

/// 心智观察器窗口是否可见且处于前台。
fn inspector_window_visible_and_front(app: &AppHandle) -> bool {
    let Some(win) = app.get_webview_window(INSPECTOR_WINDOW_LABEL) else {
        // 窗口还没打开（或已关闭）——用户自然看不见
        return false;
    };
    if !win.is_visible().unwrap_or(false) {
        return false;
    }
    if win.is_minimized().unwrap_or(false) {
        return false;
    }
    super::window::is_window_foreground(&win)
}
