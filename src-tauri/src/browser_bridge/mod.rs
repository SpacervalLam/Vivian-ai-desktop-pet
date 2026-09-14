//! 浏览器自动化桥：让 AI 角色通过精简 Chrome 扩展控制用户真实浏览器（保留登录态）。
//!
//! 架构：
//! - `protocol`：与扩展共享的 WebSocket 线协议（帧契约）。
//! - `server`：token 认证的本地回环 WS 服务，单活动连接，`browser_*` 工具经它派发。
//! - `tools`：模型侧 `browser_*` 工具实现，读操作放行、改动操作要求用户确认。

pub mod protocol;
pub mod server;
pub mod tools;

pub use protocol::{BRIDGE_PATH, BRIDGE_PORT};
pub use server::{serve, BridgeState};