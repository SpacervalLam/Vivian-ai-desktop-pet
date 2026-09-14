//! 全局 CancellationToken
//!
//! 基于 `tokio::sync::watch` 实现的竞态安全取消信号，用于在应用退出时
//! 通知所有后台 tokio 任务优雅停止。
//!
//! 使用方式：
//! ```ignore
//! // 后台任务
//! tokio::spawn(async move {
//!     let cancel = crate::utils::cancel_token::cancel_token();
//!     loop {
//!         tokio::select! {
//!             _ = cancel.cancelled() => return,
//!             _ = tokio::time::sleep(Duration::from_secs(5)) => {
//!                 // 定期工作
//!             }
//!         }
//!     }
//! });
//!
//! // 退出时
//! crate::utils::cancel_token::cancel_token().cancel();
//! ```

use std::sync::{Arc, OnceLock};

use tokio::sync::watch;

struct Inner {
    sender: watch::Sender<bool>,
}

/// 全局取消令牌，所有后台任务共享同一实例
pub struct CancelToken(Arc<Inner>);

static CANCEL_TOKEN: OnceLock<CancelToken> = OnceLock::new();

/// 获取全局 CancelToken
///
/// 首次调用时初始化。返回的 token 是共享的，`cancel()` 会通知所有 `cancelled()` 等待者。
pub fn cancel_token() -> &'static CancelToken {
    CANCEL_TOKEN.get_or_init(|| {
        let (sender, _) = watch::channel(false);
        CancelToken(Arc::new(Inner { sender }))
    })
}

impl CancelToken {
    /// 触发取消信号，通知所有等待中的任务
    ///
    /// 幂等：多次调用效果相同，只有第一次实际发送信号。
    pub fn cancel(&self) {
        let _ = self.0.sender.send(true);
    }

    /// 是否已被取消
    pub fn is_cancelled(&self) -> bool {
        *self.0.sender.borrow()
    }

    /// 等待取消信号
    ///
    /// 若已取消则立即返回；否则阻塞直到 `cancel()` 被调用。
    /// 竞态安全：在检查与注册等待之间被 cancel 也能正确感知。
    pub async fn cancelled(&self) {
        let mut rx = self.0.sender.subscribe();
        if *rx.borrow() {
            return;
        }
        while rx.changed().await.is_ok() {
            if *rx.borrow() {
                return;
            }
        }
    }
}
