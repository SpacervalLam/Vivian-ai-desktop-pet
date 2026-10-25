//! 沙箱运行时 —— 把沙箱配置与会话上下文装配成执行请求
//!
//! 隔离层（`utils::sandbox`）只认 [`SandboxRequest`]，不认识配置、不认识会话。
//! 中间这一层负责：
//! - 持有沙箱配置（应用启动及配置保存/重载时 `install`）
//! - 按配置决定是否启动本地代理，并记住它**实际**监听的端口
//! - 按会话上下文装配 scratch 归属、要授权的工作区、网络出口方式
//!
//! 装配集中在一处是为了避免漂移：配置里的可写面与真正生效的可写面必须是同一
//! 份来源，否则会出现"配置说放行、执行时写不进去"这类无解的问题。

use parking_lot::RwLock;
use std::sync::{Arc, LazyLock};

use crate::config::manager::SandboxConfig;
use crate::network::sandbox_proxy::{self, SandboxProxy};
use crate::tools::types::ToolUseContext;
use crate::utils::sandbox_policy::SandboxRequest;

struct SandboxRuntime {
    cfg: SandboxConfig,
    /// proxy 模式下启动的本地代理；`addr` 是实际监听地址（端口可能由系统分配）
    proxy: Option<Arc<SandboxProxy>>,
}

static RUNTIME: LazyLock<RwLock<Option<SandboxRuntime>>> = LazyLock::new(|| RwLock::new(None));

/// 安装或热更新沙箱配置。网络配置相同时复用监听器。
pub fn install(cfg: SandboxConfig) {
    let mut runtime = RUNTIME.write();
    if runtime.as_ref().is_some_and(|r| r.cfg == cfg) {
        return;
    }
    // 只在 proxy 模式下启动代理；其他模式不占用端口，也不让沙箱进程绕道
    let reusable = runtime
        .as_ref()
        .filter(|r| {
            r.cfg.network_mode == cfg.network_mode
                && r.cfg.proxy_host == cfg.proxy_host
                && r.cfg.proxy_port == cfg.proxy_port
                && r.cfg.proxy_allow_domains == cfg.proxy_allow_domains
        })
        .and_then(|r| r.proxy.clone());
    let proxy = match cfg.network_mode {
        crate::utils::sandbox_policy::SandboxNetworkMode::Proxy => {
            if let Some(proxy) = reusable {
                *runtime = Some(SandboxRuntime {
                    cfg,
                    proxy: Some(proxy),
                });
                return;
            }
            // 释放旧监听端口后再绑定相同端口；配置收紧立即停止旧代理接受新连接。
            runtime.take();
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
                    Some(Arc::new(p))
                }
                Err(e) => {
                    tracing::warn!("[sandbox] 代理启动失败，沙箱网络将不可用: {e}");
                    None
                }
            }
        }
        _ => None,
    };
    *runtime = Some(SandboxRuntime { cfg, proxy });
}

/// 当前沙箱配置（未 install 时返回默认值）
pub fn config() -> SandboxConfig {
    RUNTIME
        .read()
        .as_ref()
        .map(|r| r.cfg.clone())
        .unwrap_or_default()
}

/// 沙箱总开关：关闭时所有命令执行退回普通子进程
pub fn enabled() -> bool {
    RUNTIME.read().as_ref().map_or(true, |r| r.cfg.enabled)
}

/// 持久终端是否也进沙箱
pub fn sandbox_terminal() -> bool {
    RUNTIME
        .read()
        .as_ref()
        .map_or(true, |r| r.cfg.sandbox_terminal)
}

/// 是否要求必须有 OS 级隔离：true 时平台无沙箱就直接拒绝执行，不降级
pub fn require_isolation() -> bool {
    RUNTIME
        .read()
        .as_ref()
        .is_some_and(|r| r.cfg.require_isolation)
}

/// 实际生效的代理地址（仅 proxy 模式有值）
pub fn proxy_authority() -> Option<String> {
    RUNTIME
        .read()
        .as_ref()
        .and_then(|r| r.proxy.as_ref())
        .map(|p| p.authority())
}

/// 按会话上下文装配执行请求
///
/// 工作区只收**可写**的目录：只读挂载的附加目录没必要授予写权限，授权是落到
/// 文件系统上的持久改动，能少改一处就少改一处。
pub fn request_for(ctx: &ToolUseContext) -> SandboxRequest {
    let default = SandboxConfig::default();
    let runtime = RUNTIME.read();
    let cfg = runtime.as_ref().map(|r| &r.cfg).unwrap_or(&default);
    let mut dirs: Vec<String> = Vec::new();
    if !ctx.is_work_agent()
        || !cfg.grant_workspace_write
        || ctx.access_level == Some(crate::tools::types::AgentAccessLevel::ReadOnly)
    {
        // 不授权时留空：沙箱进程只写 scratch，工作区表现为只读
    } else {
        if !ctx.working_directory.trim().is_empty() {
            dirs.push(ctx.working_directory.clone());
        }
        for extra in &ctx.extra_working_directories {
            if !extra.is_read_only
                && extra.permissions.iter().any(|p| p == "write" || p == "*")
                && !extra.path.trim().is_empty()
                && !dirs
                    .iter()
                    .any(|dir| crate::tools::types::is_path_within(&extra.path, dir))
            {
                dirs.push(extra.path.clone());
            }
        }
    }
    SandboxRequest {
        session_id: if cfg.per_session_scratch {
            format!("{}:{}:{}", ctx.agent_kind, ctx.char_id, ctx.session_id)
        } else {
            String::new()
        },
        workspace_dirs: dirs,
        network_mode: cfg.network_mode,
        proxy_addr: runtime
            .as_ref()
            .and_then(|r| r.proxy.as_ref())
            .map(|p| p.authority()),
        output_cap: cfg.output_cap_bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::types::AgentAccessLevel;
    #[test]
    fn configuration_hot_reload_and_request_scopes_agree() {
        install(SandboxConfig::default());
        let chat = ToolUseContext::default().with_char_id("scope-a");
        assert!(request_for(&chat).workspace_dirs.is_empty());
        let mut work = chat
            .clone()
            .with_agent_kind("work")
            .with_working_directory("C:/audit/work");
        work.access_level = Some(AgentAccessLevel::FsWrite);
        assert_eq!(request_for(&work).workspace_dirs, vec!["C:/audit/work"]);
        assert_ne!(request_for(&chat).session_id, request_for(&work).session_id);
        work.access_level = Some(AgentAccessLevel::ReadOnly);
        assert!(request_for(&work).workspace_dirs.is_empty());
        let cfg = SandboxConfig {
            enabled: false,
            require_isolation: true,
            per_session_scratch: false,
            ..Default::default()
        };
        install(cfg);
        assert!(!enabled());
        assert!(require_isolation());
        assert!(request_for(&work).session_id.is_empty());
        let cfg = SandboxConfig {
            network_mode: crate::utils::sandbox_policy::SandboxNetworkMode::Proxy,
            ..Default::default()
        };
        install(cfg.clone());
        let address = proxy_authority().unwrap();
        install(SandboxConfig {
            output_cap_bytes: 1234,
            ..cfg
        });
        assert_eq!(proxy_authority().as_deref(), Some(address.as_str()));
        assert_eq!(request_for(&work).output_cap, 1234);
        install(SandboxConfig::default());
    }
}
