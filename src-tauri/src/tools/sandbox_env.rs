//! 沙箱运行时 —— 把沙箱配置与会话上下文装配成执行请求
//!
//! 隔离层（`utils::sandbox`）只认 [`SandboxRequest`]，不认识配置、不认识会话。
//! 中间这一层负责：
//! - 持有沙箱配置（应用启动时 `install` 一次）
//! - 按配置决定是否启动本地代理，并记住它**实际**监听的端口
//! - 按会话上下文装配 scratch 归属、要授权的工作区、网络出口方式
//!
//! 装配集中在一处是为了避免漂移：配置里的可写面与真正生效的可写面必须是同一
//! 份来源，否则会出现"配置说放行、执行时写不进去"这类无解的问题。

use std::sync::OnceLock;

use crate::config::manager::SandboxConfig;
use crate::network::sandbox_proxy::{self, SandboxProxy};
use crate::tools::types::ToolUseContext;
use crate::utils::sandbox_policy::SandboxRequest;

struct SandboxRuntime {
    cfg: SandboxConfig,
    /// proxy 模式下启动的本地代理；`addr` 是实际监听地址（端口可能由系统分配）
    proxy: Option<SandboxProxy>,
}

static RUNTIME: OnceLock<SandboxRuntime> = OnceLock::new();

/// 安装沙箱运行时（应用启动时调用一次；重复调用只有第一次生效）
pub fn install(cfg: SandboxConfig) {
    if RUNTIME.get().is_some() {
        return;
    }
    // 只在 proxy 模式下启动代理；其他模式不占用端口，也不让沙箱进程绕道
    let proxy = match cfg.network_mode {
        crate::utils::sandbox_policy::SandboxNetworkMode::Proxy => {
            let allow: Vec<String> = cfg
                .proxy_allow_domains
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            match sandbox_proxy::start(&cfg.proxy_host, cfg.proxy_port, &allow) {
                Ok(p) => {
                    tracing::info!(
                        "[sandbox] 代理就绪 {}（配置端口 {}，实际 {}）",
                        p.addr,
                        cfg.proxy_port,
                        p.addr.port()
                    );
                    Some(p)
                }
                Err(e) => {
                    tracing::warn!("[sandbox] 代理启动失败，沙箱网络将不可用: {e}");
                    None
                }
            }
        }
        _ => None,
    };
    let _ = RUNTIME.set(SandboxRuntime { cfg, proxy });
}

/// 当前沙箱配置（未 install 时返回默认值）
pub fn config() -> SandboxConfig {
    RUNTIME.get().map(|r| r.cfg.clone()).unwrap_or_default()
}

/// 沙箱总开关：关闭时所有命令执行退回普通子进程
pub fn enabled() -> bool {
    config().enabled
}

/// 持久终端是否也进沙箱
pub fn sandbox_terminal() -> bool {
    config().sandbox_terminal
}

/// 是否要求必须有 OS 级隔离：true 时平台无沙箱就直接拒绝执行，不降级
pub fn require_isolation() -> bool {
    config().require_isolation
}

/// 实际生效的代理地址（仅 proxy 模式有值）
pub fn proxy_authority() -> Option<String> {
    RUNTIME.get().and_then(|r| r.proxy.as_ref()).map(|p| p.authority())
}

/// 按会话上下文装配执行请求
///
/// 工作区只收**可写**的目录：只读挂载的附加目录没必要授予写权限，授权是落到
/// 文件系统上的持久改动，能少改一处就少改一处。
pub fn request_for(ctx: &ToolUseContext) -> SandboxRequest {
    let cfg = config();
    let mut dirs: Vec<String> = Vec::new();
    if !cfg.grant_workspace_write {
        // 不授权时留空：沙箱进程只写 scratch，工作区表现为只读
    } else {
        if !ctx.working_directory.trim().is_empty() {
            dirs.push(ctx.working_directory.clone());
        }
        for extra in &ctx.extra_working_directories {
            if !extra.is_read_only && !extra.path.trim().is_empty() {
                dirs.push(extra.path.clone());
            }
        }
    }
    SandboxRequest {
        session_id: ctx.session_id.clone(),
        workspace_dirs: dirs,
        network_mode: cfg.network_mode,
        proxy_addr: proxy_authority(),
        output_cap: cfg.output_cap_bytes,
    }
}
