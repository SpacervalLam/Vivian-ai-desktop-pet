//! 网络检测 —— 设置窗口「网络」页签的完整诊断
//!
//! 取代早期只发一个 GET 请求的「代理检测」。这里按运行时真实链路把一次
//! 出网请求拆成五个可独立证伪的环节，逐项给出结论：
//!
//! | id             | 检测内容                                   |
//! |----------------|--------------------------------------------|
//! | `proxy`        | 配置的代理服务本身能否连上（TCP 握手）      |
//! | `hosts`        | 目标域名是否被系统 hosts 文件劫持 / DNS 可解析 |
//! | `connectivity` | 按运行时相同的代理分流规则请求服务端点       |
//! | `tcp`          | 与目标主机 443 的直连 TCP 握手耗时           |
//! | `packet_loss`  | 系统 `ping` 的 ICMP 丢包率与往返延迟         |
//!
//! ## 为什么只返回事实、不返回文案
//!
//! 每项检测返回的是 `status` + `facts`（原始数值/字符串），界面上的标题、
//! 说明与右侧细节全部由前端按当前界面语言组装（见 `NetworkDiagnosisDialog.tsx`
//! 与 i18n 的 `config.diag_*`）。这样加一种语言不需要动 Rust，也不会出现
//! 后端硬编码中文串混进英文界面的情况。
//!
//! ## 目标端点怎么来
//!
//! 优先取路由矩阵里 `chat` 任务的 endpoint，其次 `ai.endpoint`，
//! 都没有时兜底到 [`FALLBACK_PROBE_URL`]（保持旧版「测试连接」的行为）。
//! 这样检测的就是**运行时真正会发请求的那个地址**，而不是一个无关的公共站点。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::config::manager::AppConfig;
use crate::network::proxy::{
    build_client_with_proxy, is_domestic_endpoint, ProxyConfig, ProxyMode,
};
use crate::utils::process::silent_command_async;

/// 未配置任何服务端点时的兜底探测目标
const FALLBACK_PROBE_URL: &str = "https://www.google.com";

/// 服务连通性探测的超时上限（秒）
///
/// 诊断是交互式操作，配置里 30s 的超时会让弹窗长时间空转；这里统一收口到 15s，
/// 超时本身就说明链路有问题，再等下去没有额外信息量。
const PROBE_TIMEOUT_CAP_SECS: u64 = 15;

/// 单项 TCP 探测超时（秒）
const TCP_PROBE_TIMEOUT_SECS: u64 = 5;

/// ICMP 探测发包数
const PING_COUNT: u32 = 4;
/// 单个回包等待上限（毫秒）
const PING_WAIT_MS: u32 = 1500;
/// ICMP 子进程整体超时（秒）
const PING_TOTAL_TIMEOUT_SECS: u64 = 12;

// ============ 数据结构 ============

/// 单项检测的结论
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisStatus {
    /// 通过
    Pass,
    /// 通过但有值得注意的情况（hosts 劫持、经代理转发、部分丢包）
    Warn,
    /// 失败
    Fail,
    /// 不适用（如直连模式下没有代理可检测、平台不支持 ICMP）
    Skip,
}

/// 单项检测的事实数据
///
/// 全部为原始数值/字符串，界面文案由前端按语言组装。
/// 所有字段可缺省，`skip_serializing_if` 让 JSON 保持精简。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagnosisFacts {
    /// 代理检测：生效的代理 URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    /// Hosts 解析：hosts 文件中命中的映射目标 IP
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hosts_ip: Option<String>,
    /// Hosts 解析：系统 DNS 解析结果
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_ip: Option<String>,
    /// 服务连通性：HTTP 状态码
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// 耗时（毫秒）—— 各检测语义不同：代理为 TCP 握手、连通性为整请求、TCP 为握手、ICMP 为往返均值
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    /// ICMP 丢包率（百分比整数）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loss_percent: Option<u32>,
    /// 失败原因 / 原始错误串（技术细节，不翻译）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 单项检测结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosisItem {
    /// 检测项 id（`proxy` / `hosts` / `connectivity` / `tcp` / `packet_loss`）
    pub id: String,
    /// 结论
    pub status: DiagnosisStatus,
    /// 事实数据
    pub facts: DiagnosisFacts,
}

/// 本次检测的目标（「当前目标」区块）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosisTarget {
    /// 服务端点（完整 URL）
    pub endpoint: String,
    /// 目标主机名
    pub host: String,
    /// 目标端口
    pub port: u16,
    /// `host:port`
    pub host_port: String,
    /// URL 协议（https / http）
    pub scheme: String,
    /// URL 路径（无路径时为 `/`）
    pub path: String,
    /// 代理模式原始值（direct / system / custom）
    pub proxy_mode: String,
    /// 该端点是否命中「国内厂商域名强制直连」规则
    pub force_direct: bool,
    /// 全局生效的代理 URL（仅展示，国内域名下仍会列出但实际不生效）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_proxy: Option<String>,
    /// 该端点**实际**走到的代理 URL（国内域名强制直连时为 None）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_proxy: Option<String>,
}

/// 完整检测报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkDiagnosisReport {
    /// 检测时间（本地时间，`2026/9/15 11:22:53` 形式，由前端直接展示）
    pub checked_at: String,
    /// 检测目标
    pub target: DiagnosisTarget,
    /// 检测项（固定顺序）
    pub items: Vec<DiagnosisItem>,
    /// 汇总：通过 / 警告 / 失败 计数
    pub passed: usize,
    pub warned: usize,
    pub failed: usize,
}

// ============ 目标解析 ============

/// 解析运行时实际使用的服务端点
///
/// 优先级：路由矩阵 `chat` 任务 → `ai.endpoint` → [`FALLBACK_PROBE_URL`]。
pub fn resolve_service_endpoint(config: &AppConfig) -> String {
    if config.enable_routing_matrix {
        if let Some(route) = config.routing_matrix.get("chat") {
            let ep = route.endpoint.trim();
            if !ep.is_empty() {
                return ep.to_string();
            }
        }
    }
    if let Some(ep) = config.ai.endpoint.as_ref() {
        let ep = ep.trim();
        if !ep.is_empty() {
            return ep.to_string();
        }
    }
    FALLBACK_PROBE_URL.to_string()
}

/// 把端点规范成绝对 URL
///
/// 配置里手填 `api.deepseek.com/v1` 或 `localhost:8080` 这类缺协议的写法很常见：
/// 前者 `Url::parse` 直接失败，后者更阴险 —— url crate 会把 `localhost` 当成
/// scheme、`8080` 当成不透明路径解析成功，于是 `host_str()` 为空。若不在这里
/// 修正，下游 Hosts / TCP / ICMP 三项都会拿着空主机名去探测，产出三条纯属虚构的
/// 失败结论。所以：解析不出主机名就补 `https://` 重新解析。
fn normalize_endpoint(endpoint: &str) -> String {
    let raw = endpoint.trim();
    if raw.is_empty() {
        return FALLBACK_PROBE_URL.to_string();
    }
    if let Ok(url) = reqwest::Url::parse(raw) {
        let has_host = url.host_str().map(|h| !h.is_empty()).unwrap_or(false);
        if has_host {
            return raw.to_string();
        }
    }
    format!("https://{raw}")
}

/// 从 URL 中拆出 (scheme, host, port, path)
///
/// 入参应已过 [`normalize_endpoint`]。真的什么都解析不出来时，退化成
/// "把输入当主机名"，至少探测目标是用户填的那台机器，而不是把整串 URL
/// 拿去当域名解析（那只会得到一串无信息量的失败）。
fn split_endpoint(endpoint: &str) -> (String, String, u16, String) {
    if let Ok(url) = reqwest::Url::parse(endpoint) {
        let host = url.host_str().unwrap_or_default().to_string();
        if !host.is_empty() {
            let scheme = url.scheme().to_string();
            let port = url.port().unwrap_or_else(|| default_port(&scheme));
            let path = if url.path().is_empty() {
                "/".to_string()
            } else {
                url.path().to_string()
            };
            return (scheme, host, port, path);
        }
    }

    // 兜底：剥掉协议与路径，剩下 host[:port]
    // 用 `last()` 而不是 `next_back()` —— 以 `&str` 为分隔符的 `Split` 不是
    // `DoubleEndedIterator`，反向取会编译不过。
    let without_scheme = endpoint.split("://").last().unwrap_or(endpoint);
    let host_only = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim();
    match host_only.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (
            "https".to_string(),
            h.to_string(),
            p.parse::<u16>().unwrap_or(443),
            "/".to_string(),
        ),
        _ => (
            "https".to_string(),
            host_only.to_string(),
            443,
            "/".to_string(),
        ),
    }
}

fn default_port(scheme: &str) -> u16 {
    match scheme {
        "http" | "ws" => 80,
        _ => 443,
    }
}

/// 从代理 URL 中拆出 (host, port)
///
/// 兼容三种常见写法：带协议的 `http://127.0.0.1:7897`、缺协议的 `127.0.0.1:7897`、
/// 以及 url crate 不认识默认端口的 socks 系列。
fn proxy_host_port(proxy_url: &str) -> Option<(String, u16)> {
    let normalized = if proxy_url.contains("://") {
        proxy_url.to_string()
    } else {
        format!("http://{proxy_url}")
    };
    let parsed = reqwest::Url::parse(&normalized).ok()?;
    let host = parsed.host_str()?.to_string();
    let port = parsed.port().or_else(|| match parsed.scheme() {
        "http" => Some(80),
        "https" => Some(443),
        "socks5" | "socks5h" | "socks4" | "socks4a" => Some(1080),
        _ => None,
    })?;
    Some((host, port))
}

/// 系统 hosts 文件路径
fn hosts_file_path() -> PathBuf {
    #[cfg(windows)]
    {
        let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
        PathBuf::from(root)
            .join("System32")
            .join("drivers")
            .join("etc")
            .join("hosts")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/etc/hosts")
    }
}

/// 在 hosts 文件中查找 `host` 的显式映射，返回命中的第一个 IP
fn lookup_hosts_entry(host: &str) -> Option<String> {
    let content = std::fs::read(hosts_file_path()).ok()?;
    // hosts 文件可能是 ANSI/GBK 编码（中文注释），IP 与主机名都是 ASCII，
    // 有损解码只影响注释部分，不影响匹配
    parse_hosts_content(&String::from_utf8_lossy(&content), host)
}

/// hosts 文本的解析规则（独立出来便于单测，不依赖本机 hosts 文件内容）
///
/// 只做精确主机名匹配（不处理通配），够覆盖"手动把域名钉到某个 IP"的场景。
fn parse_hosts_content(text: &str, host: &str) -> Option<String> {
    let target = host.to_ascii_lowercase();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(ip) = parts.next() else { continue };
        // 只认 IPv4/IPv6 字面量，避免把畸形行当成映射
        if ip.parse::<std::net::IpAddr>().is_err() {
            continue;
        }
        for name in parts {
            if name.to_ascii_lowercase() == target {
                return Some(ip.to_string());
            }
        }
    }
    None
}

// ============ 各项检测 ============

/// 代理检测：TCP 握手代理服务本身
///
/// 只验证"代理进程在监听"，不验证它能否出网 —— 后者由「服务连通性」覆盖。
/// 两者分开才能在报告里区分「代理没开」和「代理开着但出不去」。
async fn check_proxy(effective_proxy: Option<&str>) -> DiagnosisItem {
    let Some(url) = effective_proxy else {
        return DiagnosisItem {
            id: "proxy".into(),
            status: DiagnosisStatus::Skip,
            facts: DiagnosisFacts::default(),
        };
    };

    let facts = match proxy_host_port(url) {
        None => DiagnosisFacts {
            proxy_url: Some(url.to_string()),
            error: Some("代理地址无法解析".to_string()),
            ..Default::default()
        },
        Some((host, port)) => {
            let start = Instant::now();
            let connected = tokio::time::timeout(
                Duration::from_secs(TCP_PROBE_TIMEOUT_SECS),
                tokio::net::TcpStream::connect((host.as_str(), port)),
            )
            .await;
            let elapsed_ms = start.elapsed().as_millis() as u64;
            match connected {
                Ok(Ok(_)) => DiagnosisFacts {
                    proxy_url: Some(url.to_string()),
                    elapsed_ms: Some(elapsed_ms),
                    ..Default::default()
                },
                Ok(Err(e)) => DiagnosisFacts {
                    proxy_url: Some(url.to_string()),
                    error: Some(e.to_string()),
                    ..Default::default()
                },
                Err(_) => DiagnosisFacts {
                    proxy_url: Some(url.to_string()),
                    error: Some(format!("连接超时（{TCP_PROBE_TIMEOUT_SECS}s）")),
                    ..Default::default()
                },
            }
        }
    };

    let status = if facts.error.is_none() {
        DiagnosisStatus::Pass
    } else {
        DiagnosisStatus::Fail
    };
    DiagnosisItem {
        id: "proxy".into(),
        status,
        facts,
    }
}

/// Hosts 解析：hosts 文件劫持检查 + DNS 解析
///
/// DNS 解析用目标真实端口，而不是硬编码 443 —— 某些平台的解析器会按端口
/// 走不同路径（如 SRV/服务名解析），端口错了就有可能出现"连通性正常、
/// 这里却解析失败"的自相矛盾报告。
async fn check_hosts(host: &str, port: u16) -> DiagnosisItem {
    let entry = lookup_hosts_entry(host);
    let resolved = if host.is_empty() {
        None
    } else {
        tokio::net::lookup_host((host, port))
            .await
            .ok()
            .and_then(|mut it| it.next())
            .map(|addr| addr.ip().to_string())
    };

    let (status, facts) = match (&entry, &resolved) {
        // 有显式映射 —— 不算错，但用户排查"为什么连到别的机器"时第一眼要看这里
        (Some(ip), _) => (
            DiagnosisStatus::Warn,
            DiagnosisFacts {
                hosts_ip: Some(ip.clone()),
                resolved_ip: resolved.clone(),
                ..Default::default()
            },
        ),
        (None, Some(ip)) => (
            DiagnosisStatus::Pass,
            DiagnosisFacts {
                resolved_ip: Some(ip.clone()),
                ..Default::default()
            },
        ),
        (None, None) => (
            DiagnosisStatus::Fail,
            DiagnosisFacts {
                error: Some("DNS 解析失败".to_string()),
                ..Default::default()
            },
        ),
    };

    DiagnosisItem {
        id: "hosts".into(),
        status,
        facts,
    }
}

/// 服务连通性：按运行时相同的代理分流规则请求服务端点
///
/// 关键点 —— 国内厂商域名在运行时是**强制直连**的（`is_domestic_endpoint`），
/// 这里必须复用同一条规则，否则会出现"检测失败但实际能用"的误导结论。
/// 未鉴权的 GET 通常拿到 401/404，只要能收到 HTTP 响应就算端点可达。
async fn check_connectivity(proxy_config: &ProxyConfig, endpoint: &str, domestic: bool) -> DiagnosisItem {
    let mut effective = proxy_config.clone();
    if domestic {
        effective.mode = ProxyMode::Direct;
        effective.url.clear();
    }
    effective.timeout_secs = effective.timeout_secs.min(PROBE_TIMEOUT_CAP_SECS);

    let client = match build_client_with_proxy(&effective) {
        Ok(c) => c,
        Err(e) => {
            return DiagnosisItem {
                id: "connectivity".into(),
                status: DiagnosisStatus::Fail,
                facts: DiagnosisFacts {
                    error: Some(format!("客户端构建失败: {e}")),
                    ..Default::default()
                },
            };
        }
    };

    let start = Instant::now();
    let result = client.get(endpoint).send().await;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    let (status, facts) = match result {
        Ok(resp) => {
            let code = resp.status().as_u16();
            (
                // 5xx 说明链路通了但服务端自身异常，值得单独标黄
                if code >= 500 {
                    DiagnosisStatus::Warn
                } else {
                    DiagnosisStatus::Pass
                },
                DiagnosisFacts {
                    http_status: Some(code),
                    elapsed_ms: Some(elapsed_ms),
                    ..Default::default()
                },
            )
        }
        Err(e) => (
            DiagnosisStatus::Fail,
            DiagnosisFacts {
                elapsed_ms: Some(elapsed_ms),
                error: Some(if e.is_timeout() {
                    format!("请求超时（{}s）", effective.timeout_secs)
                } else if e.is_connect() {
                    format!("连接失败: {e}")
                } else {
                    e.to_string()
                }),
                ..Default::default()
            },
        ),
    };

    DiagnosisItem {
        id: "connectivity".into(),
        status,
        facts,
    }
}

/// TCP 连接延迟：与目标主机的**直连** TCP 握手
///
/// 直连口径是刻意的：它就是"不经代理能走多快"的答案。走代理的链路下直连失败
/// 属于预期，标黄并在前端文案里点明，而不是报错。
async fn check_tcp(host: &str, port: u16, proxy_in_use: bool) -> DiagnosisItem {
    let start = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(TCP_PROBE_TIMEOUT_SECS),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await;
    let elapsed_ms = start.elapsed().as_millis() as u64;

    let (status, facts) = match result {
        Ok(Ok(_)) => (
            DiagnosisStatus::Pass,
            DiagnosisFacts {
                elapsed_ms: Some(elapsed_ms),
                ..Default::default()
            },
        ),
        Ok(Err(e)) => (
            if proxy_in_use {
                DiagnosisStatus::Warn
            } else {
                DiagnosisStatus::Fail
            },
            DiagnosisFacts {
                elapsed_ms: Some(elapsed_ms),
                error: Some(e.to_string()),
                ..Default::default()
            },
        ),
        Err(_) => (
            if proxy_in_use {
                DiagnosisStatus::Warn
            } else {
                DiagnosisStatus::Fail
            },
            DiagnosisFacts {
                elapsed_ms: Some(elapsed_ms),
                error: Some(format!("连接超时（{TCP_PROBE_TIMEOUT_SECS}s）")),
                ..Default::default()
            },
        ),
    };

    DiagnosisItem {
        id: "tcp".into(),
        status,
        facts,
    }
}

/// ICMP 丢包率与往返延迟
///
/// 走系统 `ping` 而不是 raw socket：Windows 上裸 ICMP 需要管理员权限，
/// 而 `ping.exe` 任何权限都能跑。
async fn check_packet_loss(host: &str) -> DiagnosisItem {
    let mut cmd = silent_command_async("ping");
    #[cfg(windows)]
    {
        cmd.arg("-n")
            .arg(PING_COUNT.to_string())
            .arg("-w")
            .arg(PING_WAIT_MS.to_string());
    }
    #[cfg(not(windows))]
    {
        // Unix 的 -W 单位是秒
        cmd.arg("-c")
            .arg(PING_COUNT.to_string())
            .arg("-W")
            .arg(((PING_WAIT_MS + 999) / 1000).to_string());
    }
    cmd.arg(host).stdin(std::process::Stdio::null());

    let output = match tokio::time::timeout(
        Duration::from_secs(PING_TOTAL_TIMEOUT_SECS),
        cmd.output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            return DiagnosisItem {
                id: "packet_loss".into(),
                status: DiagnosisStatus::Skip,
                facts: DiagnosisFacts {
                    error: Some(format!("无法执行 ping: {e}")),
                    ..Default::default()
                },
            };
        }
        Err(_) => {
            return DiagnosisItem {
                id: "packet_loss".into(),
                status: DiagnosisStatus::Fail,
                facts: DiagnosisFacts {
                    error: Some(format!("ICMP 探测整体超时（{PING_TOTAL_TIMEOUT_SECS}s）")),
                    ..Default::default()
                },
            };
        }
    };

    // ping 的输出编码随系统区域设置变化（中文系统是 GBK），用有损解码即可 ——
    // 我们只关心其中的数字
    let text = String::from_utf8_lossy(&output.stdout);
    let text = if text.trim().is_empty() {
        String::from_utf8_lossy(&output.stderr).to_string()
    } else {
        text.to_string()
    };

    let loss_percent = parse_loss_percent(&text);
    let rtt_ms = parse_rtt_ms(&text);

    let (status, facts) = match loss_percent {
        None => {
            // 没有统计行 —— 通常是域名解析不了（"Ping 请求找不到主机"）
            let error = text
                .lines()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
                .unwrap_or_else(|| "未取得 ICMP 统计信息".to_string());
            (
                DiagnosisStatus::Fail,
                DiagnosisFacts {
                    error: Some(error),
                    ..Default::default()
                },
            )
        }
        Some(0) => (
            DiagnosisStatus::Pass,
            DiagnosisFacts {
                loss_percent: Some(0),
                elapsed_ms: rtt_ms,
                ..Default::default()
            },
        ),
        Some(100) => (
            // 很多云厂商默认禁 ICMP，全丢不等于服务不可用，故标黄而非报错
            DiagnosisStatus::Warn,
            DiagnosisFacts {
                loss_percent: Some(100),
                ..Default::default()
            },
        ),
        Some(p) => (
            DiagnosisStatus::Warn,
            DiagnosisFacts {
                loss_percent: Some(p),
                elapsed_ms: rtt_ms,
                ..Default::default()
            },
        ),
    };

    DiagnosisItem {
        id: "packet_loss".into(),
        status,
        facts,
    }
}

// ============ 输出解析 ============

static RE_LOSS_LABELED: Lazy<Regex> = Lazy::new(|| {
    // 覆盖三种系统的实际写法：
    // - Windows 中文「丢失 = 0 (0% 丢失)」
    // - Windows 日文「損失 = 0 (0% の損失)」
    // - Windows 英文「Lost = 0 (0% loss)」
    // - Unix「0% packet loss」
    // `の` / `packet` 是各自语言里夹在百分号与关键词之间的词，必须允许出现
    Regex::new(r"(?i)(\d+(?:\.\d+)?)%\s*(?:の\s*)?(?:packet\s+)?\s*(?:丢失|損失|loss)").unwrap()
});

static RE_LOSS_PAREN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\((\d+(?:\.\d+)?)%").unwrap());

static RE_RTT_AVG: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:平均|average|平均値)\s*[=:]\s*(\d+(?:\.\d+)?)\s*ms").unwrap()
});

/// Unix `ping` 的 `rtt min/avg/max/mdev = 1.2/3.4/5.6/1.0 ms`，取 avg 那一段
static RE_RTT_UNIX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"=\s*[\d.]+/(\d+(?:\.\d+)?)/").unwrap());

static RE_RTT_ANY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:时间|time)\s*[=<]\s*(\d+(?:\.\d+)?)\s*ms").unwrap());

/// 解析丢包率百分比
fn parse_loss_percent(text: &str) -> Option<u32> {
    let raw = RE_LOSS_LABELED
        .captures(text)
        .or_else(|| RE_LOSS_PAREN.captures(text))?
        .get(1)?
        .as_str()
        .to_string();
    raw.parse::<f64>().ok().map(|v| v.round() as u32)
}

/// 解析往返延迟均值；取不到均值时退化为首个回包的时间
fn parse_rtt_ms(text: &str) -> Option<u64> {
    let raw = RE_RTT_AVG
        .captures(text)
        .or_else(|| RE_RTT_UNIX.captures(text))
        .or_else(|| RE_RTT_ANY.captures(text))?
        .get(1)?
        .as_str()
        .to_string();
    raw.parse::<f64>().ok().map(|v| v.round() as u64)
}

// ============ 入口 ============

/// 执行一次完整网络检测
///
/// 五项检测彼此独立，用 `join!` 并发跑 —— 串行的话 ICMP 那 6 秒会白白叠在
/// 前面几项之上。全部结果都返回 `Ok`：单项失败是"检测结论"，不是命令错误。
pub async fn run_diagnosis(config: &AppConfig) -> NetworkDiagnosisReport {
    let proxy_config = ProxyConfig::from_app_config(config);
    let effective_proxy = proxy_config.effective_proxy_url();
    let endpoint = normalize_endpoint(&resolve_service_endpoint(config));
    let (scheme, host, port, path) = split_endpoint(&endpoint);
    let domestic = is_domestic_endpoint(&endpoint);
    let endpoint_proxy = if domestic {
        None
    } else {
        effective_proxy.clone()
    };

    let (proxy_item, hosts_item, connectivity_item, tcp_item, ping_item) = tokio::join!(
        check_proxy(effective_proxy.as_deref()),
        check_hosts(&host, port),
        check_connectivity(&proxy_config, &endpoint, domestic),
        check_tcp(&host, port, endpoint_proxy.is_some()),
        check_packet_loss(&host),
    );

    let items = vec![
        proxy_item,
        hosts_item,
        connectivity_item,
        tcp_item,
        ping_item,
    ];

    let passed = items
        .iter()
        .filter(|i| i.status == DiagnosisStatus::Pass)
        .count();
    let warned = items
        .iter()
        .filter(|i| i.status == DiagnosisStatus::Warn)
        .count();
    let failed = items
        .iter()
        .filter(|i| i.status == DiagnosisStatus::Fail)
        .count();

    // 不补零的日期形式（2026/9/15 11:22:53）—— 用 Datelike 取数比 strftime 的
    // `%-m` 修饰符更稳妥，各平台行为一致
    let now = chrono::Local::now();
    use chrono::{Datelike as _, Timelike as _};
    NetworkDiagnosisReport {
        checked_at: format!(
            "{}/{}/{} {:02}:{:02}:{:02}",
            now.year(),
            now.month(),
            now.day(),
            now.hour(),
            now.minute(),
            now.second()
        ),
        target: DiagnosisTarget {
            endpoint,
            host_port: format!("{host}:{port}"),
            host,
            port,
            scheme,
            path,
            proxy_mode: proxy_config.mode.as_str().to_string(),
            force_direct: domestic,
            effective_proxy,
            endpoint_proxy,
        },
        items,
        passed,
        warned,
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_endpoint_https_with_path() {
        let (scheme, host, port, path) = split_endpoint("https://api.deepseek.com/v1");
        assert_eq!(scheme, "https");
        assert_eq!(host, "api.deepseek.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/v1");
    }

    #[test]
    fn test_split_endpoint_custom_port_and_root() {
        let (scheme, host, port, path) = split_endpoint("http://127.0.0.1:8080");
        assert_eq!(scheme, "http");
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 8080);
        assert_eq!(path, "/");
    }

    #[test]
    fn test_normalize_endpoint_adds_scheme() {
        assert_eq!(
            normalize_endpoint("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1"
        );
        // 缺协议：补 https
        assert_eq!(
            normalize_endpoint("api.deepseek.com/v1"),
            "https://api.deepseek.com/v1"
        );
        // `localhost:8080` 会被 url crate 当成 scheme:opaque 解析成"没有主机"，
        // 必须补协议，否则下游三项探测全部针对空主机名
        assert_eq!(normalize_endpoint("localhost:8080"), "https://localhost:8080");
        assert_eq!(normalize_endpoint("127.0.0.1:8080/v1"), "https://127.0.0.1:8080/v1");
        assert_eq!(normalize_endpoint("  "), FALLBACK_PROBE_URL);
    }

    #[test]
    fn test_split_endpoint_never_yields_empty_host() {
        // 经 normalize 后必须拿到真实主机名，不能是空串
        let (_, host, port, _) = split_endpoint(&normalize_endpoint("localhost:8080"));
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);

        let (_, host, port, path) =
            split_endpoint(&normalize_endpoint("api.deepseek.com/v1"));
        assert_eq!(host, "api.deepseek.com");
        assert_eq!(port, 443);
        assert_eq!(path, "/v1");
    }

    #[test]
    fn test_proxy_host_port_variants() {
        assert_eq!(
            proxy_host_port("http://127.0.0.1:7897"),
            Some(("127.0.0.1".to_string(), 7897))
        );
        // 缺协议时按 http 补齐
        assert_eq!(
            proxy_host_port("127.0.0.1:7897"),
            Some(("127.0.0.1".to_string(), 7897))
        );
        // 省略端口时按协议默认值
        assert_eq!(
            proxy_host_port("http://127.0.0.1"),
            Some(("127.0.0.1".to_string(), 80))
        );
        // socks 系列 url crate 不认默认端口，由我们兜底 1080
        assert_eq!(
            proxy_host_port("socks5://127.0.0.1"),
            Some(("127.0.0.1".to_string(), 1080))
        );
        assert_eq!(proxy_host_port(""), None);
    }

    #[test]
    fn test_parse_loss_percent_windows_zh() {
        let out = "数据包: 已发送 = 4，已接收 = 4，丢失 = 0 (0% 丢失)，";
        assert_eq!(parse_loss_percent(out), Some(0));
    }

    #[test]
    fn test_parse_loss_percent_windows_en() {
        let out = "    Packets: Sent = 4, Received = 3, Lost = 1 (25% loss),";
        assert_eq!(parse_loss_percent(out), Some(25));
    }

    #[test]
    fn test_parse_loss_percent_unix() {
        let out = "4 packets transmitted, 4 received, 0% packet loss, time 3005ms";
        assert_eq!(parse_loss_percent(out), Some(0));
    }

    #[test]
    fn test_parse_loss_percent_none_on_dns_failure() {
        let out = "Ping 请求找不到主机 nope.invalid。请检查该名称，然后重试。";
        assert_eq!(parse_loss_percent(out), None);
    }

    #[test]
    fn test_parse_rtt_ms_average_zh() {
        let out = "    最短 = 118ms，最长 = 122ms，平均 = 120ms";
        assert_eq!(parse_rtt_ms(out), Some(120));
    }

    #[test]
    fn test_parse_rtt_ms_falls_back_to_reply_time() {
        let out = "来自 1.2.3.4 的回复: 字节=32 时间=42ms TTL=52";
        assert_eq!(parse_rtt_ms(out), Some(42));
    }

    #[test]
    fn test_resolve_service_endpoint_prefers_routing_matrix() {
        let mut config = crate::config::manager::AppConfig::default();
        config.enable_routing_matrix = true;
        config.ai.endpoint = Some("https://api.openai.com/v1".to_string());
        config.routing_matrix.insert(
            "chat".to_string(),
            crate::config::manager::TaskRouteConfig {
                provider_type: "openai".to_string(),
                model: "gpt-5.5".to_string(),
                api_key: String::new(),
                endpoint: "https://api.deepseek.com".to_string(),
                api_secret: String::new(),
                app_id: String::new(),
                temperature: None,
                max_tokens: None,
                context_window: None,
                reasoning: None,
            },
        );
        assert_eq!(resolve_service_endpoint(&config), "https://api.deepseek.com");
    }

    #[test]
    fn test_resolve_service_endpoint_falls_back_to_ai() {
        let mut config = crate::config::manager::AppConfig::default();
        config.enable_routing_matrix = false;
        config.ai.endpoint = Some("https://api.anthropic.com".to_string());
        assert_eq!(resolve_service_endpoint(&config), "https://api.anthropic.com");
    }

    #[test]
    fn test_resolve_service_endpoint_fallback_url() {
        let mut config = crate::config::manager::AppConfig::default();
        config.enable_routing_matrix = false;
        config.ai.endpoint = None;
        assert_eq!(resolve_service_endpoint(&config), FALLBACK_PROBE_URL);
    }

    #[test]
    fn test_lookup_hosts_entry_ignores_comments_and_bad_lines() {
        let sample = "\
# 这是注释
127.0.0.1 localhost
not-an-ip some.host
192.168.1.10 example.com example.local  # 行尾注释
::1 ipv6.local
";
        assert_eq!(
            parse_hosts_content(sample, "example.com"),
            Some("192.168.1.10".to_string())
        );
        // 同一行的第二个主机名也要命中
        assert_eq!(
            parse_hosts_content(sample, "example.local"),
            Some("192.168.1.10".to_string())
        );
        assert_eq!(
            parse_hosts_content(sample, "localhost"),
            Some("127.0.0.1".to_string())
        );
        // IP 非法的行整行忽略
        assert_eq!(parse_hosts_content(sample, "some.host"), None);
        // IPv6 字面量同样支持
        assert_eq!(parse_hosts_content(sample, "ipv6.local"), Some("::1".to_string()));
        // 注释里的主机名不算映射
        assert_eq!(parse_hosts_content(sample, "nope.com"), None);
        // 大小写不敏感
        assert_eq!(
            parse_hosts_content(sample, "EXAMPLE.COM"),
            Some("192.168.1.10".to_string())
        );
    }

    #[test]
    fn test_parse_loss_percent_windows_ja() {
        let out = "パケット数: 送信 = 4、受信 = 4、損失 = 0 (0% の損失)、";
        assert_eq!(parse_loss_percent(out), Some(0));
    }

    #[test]
    fn test_parse_loss_percent_full_loss() {
        let out = "    数据包: 已发送 = 4，已接收 = 0，丢失 = 4 (100% 丢失)，";
        assert_eq!(parse_loss_percent(out), Some(100));
    }

    #[test]
    fn test_parse_rtt_ms_unix() {
        let out = "rtt min/avg/max/mdev = 1.234/3.456/5.678/1.0 ms";
        assert_eq!(parse_rtt_ms(out), Some(3));
    }

    #[test]
    fn test_parse_rtt_ms_average_en() {
        let out = "    Minimum = 118ms, Maximum = 122ms, Average = 120ms";
        assert_eq!(parse_rtt_ms(out), Some(120));
    }

}
