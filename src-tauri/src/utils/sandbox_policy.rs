//! 沙箱策略描述 —— 跨平台、零 Windows 依赖的执行请求与网络策略
//!
//! 单独拆出来的原因有两个：
//! - **依赖方向**：隔离层（`utils::sandbox`，仅 Windows）不该反过来依赖配置层。
//!   网络模式与请求描述是策略，不属于任何平台实现，放在这里两边都能引用。
//! - **可验证**：本模块不碰任何 Windows API，能在隔离 harness 里直接编译跑测试，
//!   不必等整 crate 的 `cargo check`（本机 16GB 会 OOM）。
//!
//! 这里只描述"要什么"，不负责"怎么做"。

/// 沙箱网络出口模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxNetworkMode {
    /// 放行：沙箱进程网络出口不受限（默认，与隔离层接入前的行为一致）
    Allow,
    /// 代理：沙箱进程流量经本地代理，按域名白名单逐条放行
    Proxy,
    /// 阻断：不给沙箱进程任何网络出口
    Blocked,
}

impl Default for SandboxNetworkMode {
    fn default() -> Self {
        SandboxNetworkMode::Allow
    }
}

impl SandboxNetworkMode {
    /// 从配置字符串解析；未识别的值一律落到 `Allow`（放行），
    /// 不因为拼写错误把用户的网络悄悄断掉。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "proxy" | "allowlist" | "whitelist" => SandboxNetworkMode::Proxy,
            "blocked" | "block" | "offline" | "none" => SandboxNetworkMode::Blocked,
            _ => SandboxNetworkMode::Allow,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SandboxNetworkMode::Allow => "allow",
            SandboxNetworkMode::Proxy => "proxy",
            SandboxNetworkMode::Blocked => "blocked",
        }
    }
}

/// 沙箱执行请求
///
/// 一次命令执行的环境描述：scratch 归属、要授权的工作区、网络出口方式。
/// 由调用方从会话上下文与沙箱配置装配，沙箱内部不再自行猜测。
#[derive(Debug, Clone, Default)]
pub struct SandboxRequest {
    /// 会话 ID。非空时 scratch 落在会话子目录，避免不同会话的脚本与临时文件
    /// 互相可见；留空用共享 scratch。
    pub session_id: String,
    /// 需要授予写权限的工作区目录（主工作区 + 附加工作区）
    pub workspace_dirs: Vec<String>,
    /// 网络出口模式
    pub network_mode: SandboxNetworkMode,
    /// 本地代理地址 `host:port`。`network_mode` 为 `proxy` 时注入到子进程
    /// 环境变量；`allow` 时不注入；`blocked` 时忽略本字段。
    pub proxy_addr: Option<String>,
    /// 单路输出上限（字节）；0 用默认值
    pub output_cap: usize,
}

impl SandboxRequest {
    /// 解析后的输出上限（0 → 默认 4 MiB）
    pub fn cap(&self) -> usize {
        if self.output_cap == 0 {
            4 * 1024 * 1024
        } else {
            self.output_cap
        }
    }
}

/// 域名是否命中白名单
///
/// 匹配规则：忽略大小写；以 `.` 开头表示后缀匹配（`.example.com` 命中
/// `a.example.com` 与 `example.com` 之外的所有子域）。
///
/// **白名单为空 = 拒绝。** 空表是"没放行过任何东西"，不是"全都放行"——
/// 把空表读成放行会让 proxy 模式形同虚设。
pub fn domain_allowed(host: &str, allow: &[String]) -> bool {
    if allow.is_empty() {
        return false;
    }
    let host = host.trim().trim_end_matches('.').to_lowercase();
    if host.is_empty() {
        return false;
    }
    for rule in allow {
        let rule = rule.trim().to_lowercase();
        if rule.is_empty() {
            continue;
        }
        if let Some(suffix) = rule.strip_prefix('.') {
            // 后缀匹配：example.com 本身也算命中
            if host == suffix || host.ends_with(&format!(".{suffix}")) {
                return true;
            }
        } else if host == rule {
            return true;
        }
    }
    false
}

/// 从 `host:port` 里取出主机名（兼容 IPv6 字面量的方括号写法）
pub fn host_of_authority(authority: &str) -> &str {
    let authority = authority.trim();
    if let Some(rest) = authority.strip_prefix('[') {
        // [::1]:8080
        return rest.split(']').next().unwrap_or(rest);
    }
    authority.rsplit_once(':').map(|(h, _)| h).unwrap_or(authority)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_allowlist_denies_everything() {
        assert!(!domain_allowed("example.com", &[]));
        assert!(!domain_allowed("api.openai.com", &[]));
    }

    #[test]
    fn exact_and_suffix_match() {
        let allow = vec!["example.com".to_string(), ".github.com".to_string()];
        assert!(domain_allowed("example.com", &allow));
        assert!(domain_allowed("EXAMPLE.com", &allow));
        assert!(domain_allowed("a.github.com", &allow));
        assert!(domain_allowed("github.com", &allow));
        assert!(!domain_allowed("notgithub.com", &allow));
        assert!(!domain_allowed("evil.com", &allow));
    }

    #[test]
    fn host_of_authority_handles_ipv6_and_port() {
        assert_eq!(host_of_authority("example.com:443"), "example.com");
        assert_eq!(host_of_authority("[::1]:8080"), "::1");
        assert_eq!(host_of_authority("127.0.0.1"), "127.0.0.1");
    }

    #[test]
    fn mode_roundtrip() {
        for s in ["allow", "proxy", "blocked"] {
            let m = SandboxNetworkMode::parse(s);
            assert_eq!(m.as_str(), s);
        }
        // 未知值落回放行，不静默断网
        assert_eq!(SandboxNetworkMode::parse("typo"), SandboxNetworkMode::Allow);
    }
}
