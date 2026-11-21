//! Engine generations isolate delayed COM callbacks; retries never erase the browser profile.
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct EngineLifecycle {
    epoch: u64,
    started: Option<Instant>,
    retry_at: Option<Instant>,
    failures: u32,
}
impl EngineLifecycle {
    pub fn epoch(&self) -> u64 { self.epoch }
    pub fn current(&self, epoch: u64) -> bool { self.epoch == epoch }
    pub fn timed_out(&self) -> bool {
        self.started.is_some_and(|started| started.elapsed() >= Duration::from_secs(20))
    }
    pub fn can_start(&self) -> bool {
        self.can_start_at(Instant::now())
    }
    fn can_start_at(&self, now: Instant) -> bool {
        self.started.is_none_or(|started| now.duration_since(started) >= Duration::from_secs(20))
            && self.retry_at.is_none_or(|retry| now >= retry)
    }
    pub fn begin(&mut self) -> u64 {
        self.epoch += 1;
        self.started = Some(Instant::now());
        self.retry_at = None;
        self.epoch
    }
    pub fn succeeded(&mut self) {
        self.started = None;
        self.retry_at = None;
        self.failures = 0;
    }
    pub fn failed(&mut self) {
        self.epoch += 1;
        self.started = None;
        self.failures = self.failures.saturating_add(1);
        self.retry_at = Some(Instant::now() + Duration::from_secs(1 << self.failures.min(5)));
    }
}
pub(super) fn invalid_state(error: &windows::core::Error) -> bool {
    error.code().0 as u32 == 0x8007139F
}
/// Use the actual HRESULT, not the thread's possibly unrelated COM error message.
pub(super) fn engine_error(stage: &str, error: &windows::core::Error) -> String {
    let code = error.code().0 as u32;
    tracing::warn!(stage, hresult = format_args!("0x{code:08X}"), detail = %error, "WebView2 COM failure");
    let hint = match code {
        0x8007139F => "浏览器对象已失效或数据目录的启动配置冲突，正在自动重建浏览器",
        0x80040154 => "WebView2 运行时组件未注册，请安装或修复 Microsoft Edge WebView2 Runtime 后重试",
        0x80070002 => "未找到 WebView2 运行时，请安装 Microsoft Edge WebView2 Runtime 后重试",
        0x80004002 => "WebView2 运行时版本不支持所需接口，请更新 Microsoft Edge WebView2 Runtime",
        _ => "浏览器启动或操作失败，请稍后重试",
    };
    format!("{stage}：{hint} (0x{code:08X})")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failed_generation_rejects_late_callbacks_and_backs_off() {
        let mut state = EngineLifecycle::default();
        assert!(state.can_start());
        let epoch = state.begin();
        assert!(!state.can_start());
        assert!(state.current(epoch));
        state.failed();
        assert!(!state.current(epoch));
        assert!(!state.can_start());
        assert!(state.can_start_at(Instant::now() + Duration::from_secs(3)));
        let next = state.begin();
        assert_ne!(epoch, next);
        state.succeeded();
        assert!(state.current(next));
        assert_eq!(state.failures, 0);
    }
    #[test]
    fn abandoned_initialization_can_be_replaced() {
        let mut state = EngineLifecycle::default();
        let epoch = state.begin();
        assert!(state.can_start_at(Instant::now() + Duration::from_secs(21)));
        state.started = Some(Instant::now() - Duration::from_secs(21));
        assert!(state.timed_out());
        state.failed();
        assert!(!state.current(epoch));
    }
    #[test]
    fn error_text_is_derived_from_hresult_even_when_com_description_is_wrong() {
        let error = windows::core::Error::new(windows::core::HRESULT(0x8007139Fu32 as i32), "没有注册类");
        let message = engine_error("读取凭据", &error);
        assert!(message.contains("对象已失效"));
        assert!(message.contains("0x8007139F"));
        assert!(!message.contains("没有注册类"));
        let error = windows::core::Error::from_hresult(windows::core::HRESULT(0x80040154u32 as i32));
        assert!(engine_error("创建环境", &error).contains("运行时组件未注册"));
    }
}
