//! 工具执行确认注册表
//!
//! 当工具需要用户确认（如文件操作、屏幕截图等隐私敏感操作）时，
//! 通过此注册表创建一个 pending request，emit 事件给前端，
//! 前端弹 toast/Modal 询问用户，用户选择后通过 Tauri command 回传结果，
//! 注册表通过 oneshot channel 恢复 await 的工具执行流程。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::oneshot;

/// 单调递增的请求 ID
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// pending 请求的 TTL：超过 5 分钟未响应自动清理
const PENDING_TTL: Duration = Duration::from_secs(5 * 60);

/// 发送给前端的确认请求 payload
#[derive(Debug, Clone, Serialize)]
pub struct ConfirmationRequest {
    /// 唯一请求 ID，前端回传时需要带上
    pub request_id: u64,
    /// 工具名
    pub tool: String,
    /// 工具参数（JSON）
    pub arguments: serde_json::Value,
    /// 权限询问原因（如"该工具将读取文件 /etc/hosts"）
    pub reason: String,
    /// 风险等级描述（用于前端 UI 颜色/图标区分）
    pub risk_level: ConfirmationRisk,
    /// 发起请求的角色 ID（多角色场景下前端按此路由到对应 toast 窗口）
    pub char_id: String,
    /// "always allow" 按钮的作用域：persistent（持久信任，如应用白名单）/ session（本次运行）
    pub allow_always_scope: String,
}

/// 用户对确认请求的三态响应
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationResponse {
    /// 拒绝执行
    Deny,
    /// 仅本次放行
    AllowOnce,
    /// 始终允许（open_application 持久化到信任列表，其余工具会话级放行）
    AllowAlways,
}

/// 风险等级
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationRisk {
    /// 低风险（如读取文件、列目录）
    Low,
    /// 中风险（如截屏、OCR）
    Medium,
    /// 高风险（如写入文件、编辑文件）
    High,
}

/// pending 请求条目：oneshot sender + 创建时间（用于 TTL 清理）
struct PendingEntry {
    sender: oneshot::Sender<ConfirmationResponse>,
    created_at: Instant,
}

/// 工具确认注册表
///
/// 存储 pending 的确认请求 + oneshot sender，等待前端回传结果。
/// 线程安全，可被多个工具执行流共享。
///
/// 内存保护：pending 请求带 5 分钟 TTL，超过未响应的请求在下次 create/resolve/cancel 时
/// 惰性清理，避免用户忽略确认弹窗导致 pending 永驻内存。
pub struct ToolConfirmationRegistry {
    /// pending 请求：request_id → PendingEntry
    pending: Mutex<HashMap<u64, PendingEntry>>,
}

impl ToolConfirmationRegistry {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// 创建一个新的确认请求
    ///
    /// 返回 `(request_id, receiver)`：
    /// - `request_id`：用于 emit 事件给前端
    /// - `receiver`：await 此 receiver 获取用户选择（Deny/AllowOnce/AllowAlways）
    ///   receiver 在 sender 被 drop 时返回 `Err`，表示用户未响应（如关闭窗口或 TTL 清理）
    pub fn create_request(&self) -> (u64, oneshot::Receiver<ConfirmationResponse>) {
        let id = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        // 惰性清理过期请求，避免 pending 永驻
        self.cleanup_expired_locked();
        self.pending.lock().insert(
            id,
            PendingEntry {
                sender: tx,
                created_at: Instant::now(),
            },
        );
        (id, rx)
    }

    /// 解决一个 pending 请求
    ///
    /// 由前端 Tauri command 调用，回传用户的三态选择：
    /// - `Deny` → 工具返回 UserDenied
    /// - `AllowOnce` → 仅本次放行，工具继续执行
    /// - `AllowAlways` → 放行并记忆（open_application 持久信任 / 其余工具会话级放行）
    /// - 请求不存在（已被超时清理或重复解决）→ 返回 false
    pub fn resolve_request(&self, request_id: u64, response: ConfirmationResponse) -> bool {
        if let Some(entry) = self.pending.lock().remove(&request_id) {
            let _ = entry.sender.send(response);
            true
        } else {
            false
        }
    }

    /// 取消一个 pending 请求（用于超时清理）
    ///
    /// 取消后 receiver 会收到 `Err`，工具执行流应处理此情况。
    pub fn cancel_request(&self, request_id: u64) {
        self.pending.lock().remove(&request_id);
    }

    /// 当前 pending 请求数（用于诊断/监控）
    pub fn pending_count(&self) -> usize {
        self.cleanup_expired_locked();
        self.pending.lock().len()
    }

    /// 惰性清理过期请求（超过 PENDING_TTL 未响应）
    ///
    /// 在 create_request / pending_count 时调用，无需额外后台任务。
    /// 清理时 sender 被 drop，receiver 收到 Err，await 处会处理为"未响应"。
    fn cleanup_expired_locked(&self) {
        let now = Instant::now();
        let mut pending = self.pending.lock();
        let before = pending.len();
        pending.retain(|_, entry| now.duration_since(entry.created_at) < PENDING_TTL);
        let removed = before - pending.len();
        if removed > 0 {
            tracing::warn!(
                "[Confirmation] 清理 {} 个超时未响应的 pending 请求（TTL={}s）",
                removed,
                PENDING_TTL.as_secs()
            );
        }
    }
}

impl Default for ToolConfirmationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局共享的确认注册表类型
pub type SharedConfirmationRegistry = Arc<ToolConfirmationRegistry>;

/// 根据工具名和参数推断风险等级与确认原因
///
/// 用于在无 `can_use_tool` 回调时，通过 Tauri 事件向前端请求确认。
/// 覆盖 9 个需要用户确认的工具（file_ops 6 + 感知 3）。
pub fn confirmation_info(tool_name: &str, args: &serde_json::Value) -> (ConfirmationRisk, String) {
    match tool_name {
        "write_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            (ConfirmationRisk::High, format!("将写入文件：{}", path))
        }
        "edit_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            (ConfirmationRisk::High, format!("将编辑文件：{}", path))
        }
        "read_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            (ConfirmationRisk::Low, format!("将读取文件：{}", path))
        }
        "list_directory" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("?");
            (ConfirmationRisk::Low, format!("将列出目录：{}", path))
        }
        "search_files" => {
            let dir = args.get("directory").and_then(|v| v.as_str()).unwrap_or("?");
            (ConfirmationRisk::Low, format!("将在目录 {} 中搜索文件", dir))
        }
        "grep" => {
            let dir = args.get("directory").and_then(|v| v.as_str()).unwrap_or("?");
            (ConfirmationRisk::Low, format!("将在目录 {} 中搜索文件内容", dir))
        }
        "open_application" => {
            let app = args
                .get("application")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            (ConfirmationRisk::Low, format!("想要启动应用「{}」", app))
        }
        _ => (ConfirmationRisk::Low, format!("工具 {} 请求执行", tool_name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_and_resolve() {
        let registry = ToolConfirmationRegistry::new();
        let (id, mut rx) = registry.create_request();

        // 在另一个"线程"（此处用同步调用模拟）解决请求
        let resolved = registry.resolve_request(id, ConfirmationResponse::AllowOnce);
        assert!(resolved);

        // receiver 应收到三态响应
        let response = rx.try_recv();
        assert_eq!(response, Ok(ConfirmationResponse::AllowOnce));
    }

    #[tokio::test]
    async fn test_resolve_unknown_returns_false() {
        let registry = ToolConfirmationRegistry::new();
        assert!(!registry.resolve_request(999, ConfirmationResponse::Deny));
    }

    #[tokio::test]
    async fn test_cancel_drops_sender() {
        let registry = ToolConfirmationRegistry::new();
        let (id, mut rx) = registry.create_request();
        registry.cancel_request(id);
        // sender 被 drop，receiver 收到 Err
        assert!(rx.try_recv().is_err());
    }
}
