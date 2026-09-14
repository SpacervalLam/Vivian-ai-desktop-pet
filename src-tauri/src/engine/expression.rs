//! 表情管理器
//!
//! 负责表情栈、临时表情与定时恢复。

use super::resource_loader::{ExpressionInfo, ResourceLoader};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, warn};

/// 默认表情名
pub const DEFAULT_EXPRESSION: &str = "neutral";

/// 表情变化回调类型（参数为当前表情名，None 表示重置到默认）
pub type ExpressionChangeCallback = Arc<dyn Fn(Option<String>) + Send + Sync>;

/// 表情恢复回调类型
pub type RevertCallback = Arc<dyn Fn(Option<String>) + Send + Sync>;

/// 表情管理器内部状态
struct ExpressionManagerInner {
    current_expression: Option<String>,
    expression_stack: Vec<Option<String>>,
    revert_timer: Option<tokio::task::JoinHandle<()>>,
    is_temporarily_changed: bool,
    revert_callback: Option<RevertCallback>,
    expression_change_callback: Option<ExpressionChangeCallback>,
}

impl ExpressionManagerInner {
    fn new() -> Self {
        Self {
            current_expression: None,
            expression_stack: Vec::new(),
            revert_timer: None,
            is_temporarily_changed: false,
            revert_callback: None,
            expression_change_callback: None,
        }
    }
}

/// 表情管理器 - 管理表情栈、临时表情与定时恢复
pub struct ExpressionManager {
    resource_loader: Arc<ResourceLoader>,
    inner: Arc<parking_lot::RwLock<ExpressionManagerInner>>,
    /// 当前角色的 ResourceManifest（用于表情名归一化）
    manifest: parking_lot::RwLock<Option<Arc<super::manifest::ResourceManifest>>>,
}

impl ExpressionManager {
    pub fn new(resource_loader: Arc<ResourceLoader>) -> Self {
        Self {
            resource_loader,
            inner: Arc::new(parking_lot::RwLock::new(ExpressionManagerInner::new())),
            manifest: parking_lot::RwLock::new(None),
        }
    }

    /// 注入当前角色的 ResourceManifest（用于表情名归一化）
    pub fn set_manifest(&self, manifest: Arc<super::manifest::ResourceManifest>) {
        *self.manifest.write() = Some(manifest);
    }

    /// 设置表情，返回是否成功
    pub fn set_expression(
        &self,
        name: &str,
        duration_ms: Option<u64>,
        force: bool,
        priority: i32,
    ) -> bool {
        // 空名或 default/neutral 重置表情
        if name.trim().is_empty() || name == "default" || name == "neutral" {
            self.reset_expression();
            self.trigger_expression_change_callback(None);
            return true;
        }

        // 应用表情名归一化（通过当前角色 manifest 守门员）
        // manifest 未注入时降级为原名（不阻塞）
        let mapped_name = {
            let guard = self.manifest.read();
            match guard.as_ref() {
                Some(m) => m.normalize_expression(name),
                None => name.to_string(),
            }
        };
        if mapped_name.is_empty() {
            return false;
        }

        let mut inner = self.inner.write();

        // 相同表情且非强制：仅刷新定时器
        if inner.current_expression.as_deref() == Some(mapped_name.as_str()) && !force {
            if let Some(ms) = duration_ms {
                if ms > 0 {
                    drop(inner);
                    self.start_revert_timer(ms);
                    return true;
                }
            }
            return true;
        }

        // 压栈逻辑
        let should_push = if inner.is_temporarily_changed && priority <= 0 {
            true
        } else if inner.current_expression.is_some() && !inner.is_temporarily_changed {
            true
        } else {
            false
        };

        if should_push {
            // 先克隆再 push，避免同一作用域内的可变与不可变借用冲突
            let current = inner.current_expression.clone();
            inner.expression_stack.push(current);
        }

        inner.current_expression = Some(mapped_name.clone());
        inner.is_temporarily_changed = duration_ms.map(|ms| ms > 0).unwrap_or(false);

        // 启动恢复定时器
        let timer_ms = duration_ms.filter(|&ms| ms > 0);
        drop(inner);
        if let Some(ms) = timer_ms {
            self.start_revert_timer(ms);
        }

        self.trigger_expression_change_callback(Some(mapped_name));
        true
    }

    /// 恢复到上一个表情
    pub fn revert_expression(&self) -> bool {
        let mut inner = self.inner.write();
        // 停止恢复定时器
        if let Some(handle) = inner.revert_timer.take() {
            handle.abort();
        }

        if let Some(last) = inner.expression_stack.pop() {
            if last.as_deref() != inner.current_expression.as_deref() {
                let prev = inner.current_expression.take();
                inner.current_expression = last.clone();
                inner.is_temporarily_changed = false;
                let cb = inner.revert_callback.clone();
                drop(inner);
                if let Some(cb) = cb {
                    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(last)));
                }
                let _ = prev;
                return true;
            }
        }

        // 栈空，回到默认
        if inner.current_expression.as_deref() != Some(DEFAULT_EXPRESSION) {
            inner.current_expression = Some(DEFAULT_EXPRESSION.to_string());
            inner.is_temporarily_changed = false;
            let cb = inner.revert_callback.clone();
            drop(inner);
            if let Some(cb) = cb {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    cb(Some(DEFAULT_EXPRESSION.to_string()))
                }));
            }
            return true;
        }

        false
    }

    /// 重置表情到默认状态
    pub fn reset_expression(&self) {
        let mut inner = self.inner.write();
        inner.expression_stack.clear();
        inner.current_expression = None;
        inner.is_temporarily_changed = false;
        if let Some(handle) = inner.revert_timer.take() {
            handle.abort();
        }
    }

    /// 清空表情栈
    pub fn clear_stack(&self) {
        self.inner.write().expression_stack.clear();
    }

    /// 获取当前表情名称
    pub fn get_current_expression(&self) -> Option<String> {
        self.inner.read().current_expression.clone()
    }

    /// 是否处于临时表情状态
    pub fn is_temporarily_changed(&self) -> bool {
        self.inner.read().is_temporarily_changed
    }

    /// 设置表情恢复回调
    pub fn set_revert_callback(&self, callback: RevertCallback) {
        self.inner.write().revert_callback = Some(callback);
    }

    /// 设置表情变化回调
    pub fn on_expression_change(&self, callback: ExpressionChangeCallback) {
        self.inner.write().expression_change_callback = Some(callback);
    }

    /// 触发表情变化回调
    pub fn trigger_expression_change_callback(&self, expression_name: Option<String>) {
        let cb = self.inner.read().expression_change_callback.clone();
        if let Some(cb) = cb {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cb(expression_name);
            }));
        }
    }

    /// 启动恢复定时器
    fn start_revert_timer(&self, duration_ms: u64) {
        // 先取消已有定时器
        {
            let mut inner = self.inner.write();
            if let Some(handle) = inner.revert_timer.take() {
                handle.abort();
            }
        }

        let inner = self.inner.clone();
        let handle = spawn_timer(duration_ms, move || {
            on_revert_timeout(inner);
        });
        if let Some(h) = handle {
            self.inner.write().revert_timer = Some(h);
        }
    }

    /// 获取表情信息
    pub fn get_expression_info(&self, name: &str) -> Option<ExpressionInfo> {
        self.resource_loader.get_expression(name)
    }

    /// 列出所有表情名称
    pub fn list_expressions(&self) -> Vec<String> {
        self.resource_loader.list_expression_names()
    }

    /// 获取统计信息
    pub fn get_statistics(&self) -> ExpressionStatistics {
        let inner = self.inner.read();
        ExpressionStatistics {
            current_expression: inner.current_expression.clone(),
            stack_depth: inner.expression_stack.len(),
            is_temporarily_changed: inner.is_temporarily_changed,
            expression_count: self.resource_loader.list_expression_names().len(),
        }
    }
}

/// 表情管理器统计信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpressionStatistics {
    pub current_expression: Option<String>,
    pub stack_depth: usize,
    pub is_temporarily_changed: bool,
    pub expression_count: usize,
}

/// 恢复定时器回调
fn on_revert_timeout(inner: Arc<parking_lot::RwLock<ExpressionManagerInner>>) {
    // 复用 revert_expression 的逻辑：构造一个临时管理器视图
    // 由于 revert_expression 是 &self 方法，这里直接操作 inner
    let mut inner_w = inner.write();
    if let Some(handle) = inner_w.revert_timer.take() {
        handle.abort();
    }

    if let Some(last) = inner_w.expression_stack.pop() {
        if last.as_deref() != inner_w.current_expression.as_deref() {
            inner_w.current_expression = last.clone();
            inner_w.is_temporarily_changed = false;
            let cb = inner_w.revert_callback.clone();
            drop(inner_w);
            if let Some(cb) = cb {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| cb(last)));
            }
            debug!("[ExpressionManager] 定时恢复到上一个表情");
            return;
        }
    }

    // 回到默认
    if inner_w.current_expression.as_deref() != Some(DEFAULT_EXPRESSION) {
        inner_w.current_expression = Some(DEFAULT_EXPRESSION.to_string());
        inner_w.is_temporarily_changed = false;
        let cb = inner_w.revert_callback.clone();
        drop(inner_w);
        if let Some(cb) = cb {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cb(Some(DEFAULT_EXPRESSION.to_string()))
            }));
        }
        debug!("[ExpressionManager] 定时恢复到默认表情");
    }
}

/// 启动一个可取消的 tokio 定时器
fn spawn_timer(
    duration_ms: u64,
    callback: impl FnOnce() + Send + 'static,
) -> Option<tokio::task::JoinHandle<()>> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => Some(handle.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(duration_ms)).await;
            callback();
        })),
        Err(_) => {
            warn!("[ExpressionManager] 无法启动定时器：不在 tokio runtime 上下文中");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manager() -> ExpressionManager {
        let loader = Arc::new(ResourceLoader::new("/tmp", "Vivian"));
        ExpressionManager::new(loader)
    }

    #[test]
    fn test_reset_expression() {
        let mgr = make_manager();
        mgr.reset_expression();
        assert_eq!(mgr.get_current_expression(), None);
        assert!(!mgr.is_temporarily_changed());
    }

    #[test]
    fn test_default_constant() {
        assert_eq!(DEFAULT_EXPRESSION, "neutral");
    }
}
