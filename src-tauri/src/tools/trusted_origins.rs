//! 浏览器可信来源白名单。
//!
//! 对高信任站点做规范化精确/通配匹配，命中时 `browser_navigate` 可直接放行，
//! 免去见用户弹出确认框，同时把信任边界收敛到白名单站点，其余改动仍要求确认。
//!
//! 规则来源两级合并：
//! 1. 内置默认（BUILTIN）；
//! 2. 用户配置 `<用户数据目录>/trusted_origins.json`（`{"origins": [...]}`），
//!    支持 `example.com` 子域通配、`*.example.com` 显式通配、`exact:example.com` 精确匹配。
//!    文件变更通过 mtime 检测自动热重载，无需重启。

use once_cell::sync::Lazy;

use crate::utils::path;

/// 单条信任规则：`exact` 表示仅精确匹配该 host，「`*.`」/缺省前缀为子域通配。
struct TrustedRule {
    exact: bool,
    host: String,
}

impl TrustedRule {
    fn matches(&self, host: &str) -> bool {
        if self.exact {
            host == self.host
        } else {
            host == self.host || host.ends_with(&format!(".{}", self.host))
        }
    }
}

/// 内置默认可信来源（桌宠常用的高信任站点）。
const BUILTIN: &[&str] = &[
    "github.com",
    "bilibili.com",
    "zhihu.com",
    "baidu.com",
    "bing.com",
    "google.com",
    "duckduckgo.com",
    "wikipedia.org",
    "doubao.com",
    "taobao.com",
    "jd.com",
];

/// 全局白名单实例（内置 + 用户配置，mtime 热重载）。
static TRUSTED: Lazy<TrustedOrigins> = Lazy::new(TrustedOrigins::build);

/// 用户配置文件路径：`<用户数据目录>/trusted_origins.json`。
fn user_config_path() -> std::path::PathBuf {
    path::get_user_data_dir().join("trusted_origins.json")
}

/// 可信来源集合：内置规则 + 用户规则（可热重载）。
struct TrustedOrigins {
    builtin: Vec<TrustedRule>,
    user: parking_lot::RwLock<Vec<TrustedRule>>,
    /// 上次加载用户配置时的文件 mtime（毫秒），用于变更检测。
    file_mtime: parking_lot::RwLock<Option<u128>>,
}

impl TrustedOrigins {
    fn build() -> Self {
        let builtin = BUILTIN.iter().map(|e| rule_from(e)).collect();
        let mut this = Self {
            builtin,
            user: parking_lot::RwLock::new(Vec::new()),
            file_mtime: parking_lot::RwLock::new(None),
        };
        this.reload_user_rules();
        this
    }

    /// 读取用户配置文件；文件缺失时写入自解释模板（含格式说明的空配置）。
    fn reload_user_rules(&mut self) {
        let file = user_config_path();
        let mtime = file
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis());

        if !file.exists() {
            // 首次运行写入模板，让用户能发现并理解该配置
            let template = serde_json::json!({
                "_hint": "额外可信来源，每项一个 host。支持 example.com（含子域）、*.example.com（显式通配）、exact:example.com（仅精确匹配）。与内置白名单合并生效，修改后自动热重载。",
                "origins": []
            });
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(text) = serde_json::to_string_pretty(&template) {
                let _ = std::fs::write(&file, text);
            }
        }

        let rules = read_user_rules(&file);
        *self.user.write() = rules;
        *self.file_mtime.write() = mtime;
    }

    /// mtime 变化时重新加载用户规则（每次查询一次 stat，开销可忽略）。
    fn ensure_fresh(&self) {
        let current = user_config_path()
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis());
        if current != *self.file_mtime.read() {
            *self.user.write() = read_user_rules(&user_config_path());
            *self.file_mtime.write() = current;
            tracing::info!("[trusted_origins] 用户配置变更，白名单已热重载");
        }
    }

    /// 判断完整 URL 的来源是否命中白名单。
    fn is_trusted_url(&self, url: &str) -> bool {
        self.ensure_fresh();
        let Some(host) = url_host(url) else {
            return false;
        };
        if self.builtin.iter().any(|r| r.matches(&host)) {
            return true;
        }
        self.user.read().iter().any(|r| r.matches(&host))
    }
}

/// 解析用户配置文件中的 origins 数组为规则列表。
fn read_user_rules(file: &std::path::Path) -> Vec<TrustedRule> {
    crate::utils::fs::load_json_or_backup::<serde_json::Value>(file)
        .and_then(|v| v.get("origins").and_then(|o| o.as_array()).cloned())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.as_str())
                .map(rule_from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

/// 从一条配置项构造规则。支持 `exact:` 精确匹配与 `*.` 子域通配前缀。
fn rule_from(entry: &str) -> TrustedRule {
    let e = entry.trim();
    if let Some(host) = e.strip_prefix("exact:") {
        TrustedRule {
            exact: true,
            host: normalize_host(host),
        }
    } else if let Some(host) = e.strip_prefix("*.") {
        TrustedRule {
            exact: false,
            host: normalize_host(host),
        }
    } else {
        TrustedRule {
            exact: false,
            host: normalize_host(e),
        }
    }
}

/// 规范化一个 host 串：去除 scheme、路径、端口，统一小写。
fn normalize_host(s: &str) -> String {
    let rest = s.find("://").map(|i| &s[i + 3..]).unwrap_or(s);
    let host = rest
        .split('/')
        .next()
        .unwrap_or(rest)
        .split(':')
        .next()
        .unwrap_or(rest);
    host.trim().to_ascii_lowercase()
}

/// 从完整 URL 提取来源 host（如 `https://www.bilibili.com/video` → `www.bilibili.com`）。
fn url_host(url: &str) -> Option<String> {
    let host = normalize_host(url);
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

/// 公开查询入口：URL 是否落在可信来源白名单内。
pub fn is_trusted_url(url: &str) -> bool {
    TRUSTED.is_trusted_url(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_matches() {
        assert!(is_trusted_url("https://www.bilibili.com/video/BV1xx"));
        assert!(is_trusted_url("https://github.com/vivian-rs"));
        assert!(!is_trusted_url("https://evil-example.com"));
    }

    #[test]
    fn rule_parsing() {
        assert!(rule_from("example.com").matches("a.example.com"));
        assert!(rule_from("*.example.com").matches("b.example.com"));
        let exact = rule_from("exact:example.com");
        assert!(exact.matches("example.com"));
        assert!(!exact.matches("sub.example.com"));
    }
}
