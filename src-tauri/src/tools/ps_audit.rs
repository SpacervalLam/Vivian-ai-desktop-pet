//! PowerShell 脚本 AST 审计 —— 动态工具脚本的静态安全检查
//!
//! 字符串碎片黑名单（[`super::builtin::coding_tools::FORBIDDEN_FRAGMENTS`]）
//! 无法覆盖别名、大小写、变量拼接与 AST 结构差异，只是装饰性防线。本模块用
//! PowerShell 官方 Parser（[`PS_AUDIT_PS1`]，独立一次性进程，纯解析不执行）
//! 提取脚本的真实行为面：
//! - **命令清单**：CommandAst 全量收集 + `Get-Command` 别名解析
//!   （`iex` → `Invoke-Expression`），按解析后的规范名判定
//! - **参数**：CommandParameterAst（拦 `-EncodedCommand` 类编码执行）
//! - **.NET 类型**：TypeExpressionAst + `New-Object` 首参字符串
//!
//! 判定策略（见 [`classify`]）：
//! - **硬拒**（创建与执行均拒绝）：不可逆破坏 / 持久化 / 隐蔽执行类——
//!   `Invoke-Expression`、`Start-Process`、`-EncodedCommand`、`Add-Type`、
//!   防护篡改（`*-MpPreference`）、磁盘/服务操作、原生互操作等
//! - **报告呈现**（不拒绝，随确认展示）：网络类 cmdlet（`Invoke-WebRequest`
//!   等 fetch 工具的正路）与常规查询——用户在权限确认里看到完整命令清单，
//!   比一个黑体的「Shell 风险」有用得多
//!
//! 这是纵深防御的一层，不是沙箱：PowerShell 与 .NET 面太大，AST 审计挡住
//! 常见破坏与显式注入，真正的隔离靠进程边界（AppContainer 方案另行推进）。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use base64::Engine;

/// 审计器脚本（纯 AST 解析，见文件头注释）
const PS_AUDIT_PS1: &str = include_str!("ps_audit.ps1");

/// 审计进程超时（秒）
const AUDIT_TIMEOUT_SECS: u64 = 30;

/// 单批审计脚本数上限
const MAX_BATCH: usize = 256;

// ============================================================================
// 判定表
// ============================================================================

/// 硬拒命令（规范名小写；含别名解析后的形态与外部可执行文件名，去 .exe 后缀比对）
const HARD_BLOCK_COMMANDS: &[&str] = &[
    // 隐蔽/动态执行
    "invoke-expression",
    "invoke-command",
    "start-process",
    "start-job",
    "start-threadjob",
    "invoke-item",
    "add-type",
    // 执行策略与防护篡改
    "set-executionpolicy",
    "add-mppreference",
    "remove-mppreference",
    "set-mppreference",
    // 磁盘与系统不可逆操作
    "format-volume",
    "clear-disk",
    "initialize-disk",
    "remove-partition",
    "stop-computer",
    "restart-computer",
    // 服务持久化
    "new-service",
    "set-service",
    // 外部可执行文件（持久化 / 凭据转储 / 系统修改的常用载体）
    "schtasks",
    "regsvr32",
    "rundll32",
    "bitsadmin",
    "certutil",
    "diskpart",
    "vssadmin",
    "bcdedit",
    "wevtutil",
    "fltmc",
    "wmic",
];

/// 硬拒 .NET 类型前缀（对类型名首个分量小写前缀匹配）。
///
/// 设计取向：网络/进程/WMI 等能力必须以**可见命令**的形态出现
/// （`Invoke-RestMethod` 等会被枚举进确认清单）；用 .NET 类型直接表达则绕开
/// 了命令呈现——一律拒绝。原生互操作（可调用任意 Win32）同罪。
const HARD_BLOCK_TYPE_PREFIXES: &[&str] = &[
    "system.diagnostics.process",
    "system.net.",
    "system.io.pipes",
    "system.management",              // WMI / PS 自动化运行时操控
    "system.runtime.interopservices", // P/Invoke 原生调用
    "system.serviceprocess",
    "system.directoryservices",
    "microsoft.win32.registry",
];

/// 硬拒参数（小写）
const HARD_BLOCK_PARAMETERS: &[&str] = &["encodedcommand"];

// ============================================================================
// 结果结构
// ============================================================================

/// 单个脚本的审计结果
#[derive(Debug, Clone)]
pub struct ScriptAudit {
    /// 解析出的命令（别名已解析为规范名）
    pub commands: Vec<String>,
    /// 命令参数名
    pub parameters: Vec<String>,
    /// 引用的 .NET 类型名
    pub types: Vec<String>,
    /// 语法错误数
    pub parse_errors: usize,
    /// 违规条目（空 = 通过）
    pub violations: Vec<String>,
}

impl ScriptAudit {
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    /// 违规摘要（面向用户/LLM 的说明；通过时为 None）
    pub fn summary(&self) -> Option<String> {
        if self.violations.is_empty() {
            return None;
        }
        let commands = if self.commands.is_empty() {
            "（无）".to_string()
        } else {
            self.commands.join(", ")
        };
        Some(format!(
            "脚本审计未通过：{}\n脚本引用的命令：{commands}",
            self.violations.join("；")
        ))
    }
}

fn clean_audit() -> ScriptAudit {
    ScriptAudit {
        commands: Vec::new(),
        parameters: Vec::new(),
        types: Vec::new(),
        parse_errors: 0,
        violations: Vec::new(),
    }
}

// ============================================================================
// 审计执行（PS 一次性进程 + 结果缓存）
// ============================================================================

/// 审计脚本落盘路径（首次使用时写入临时目录，内容恒定）。
///
/// 必须带 UTF-8 BOM：Windows PowerShell 5.1 对无 BOM 文件按 ANSI（中文系统
/// 为 GBK）解码，脚本里的中文注释会被改写成破坏语法的字节序列。
fn audit_ps1_path() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let path = std::env::temp_dir().join("vivian-ps-audit.ps1");
        let content = format!("\u{feff}{}\n", PS_AUDIT_PS1.trim_end_matches(['\r', '\n']));
        let _ = crate::utils::fs::write_atomic(&path, &content);
        path
    })
}

/// 脚本哈希 → 审计结果缓存（执行期防重复审计同一脚本）
fn audit_cache() -> &'static Mutex<HashMap<u64, ScriptAudit>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, ScriptAudit>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn script_hash(script: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    script.hash(&mut h);
    h.finish()
}

fn cache_put(script: &str, audit: &ScriptAudit) {
    let mut cache = audit_cache().lock().unwrap();
    if cache.len() > 512 {
        cache.clear(); // 简单防膨胀：正常会话远达不到
    }
    cache.insert(script_hash(script), audit.clone());
}

/// 批量审计（一次 PS 进程处理全部脚本）。供装载路径用，避免逐脚本 spawn。
///
/// 审计进程本身失败（找不到 / 超时 / 输出损坏）返回 Err——审计不可用即拒绝，
/// 失败关闭。非 Windows 平台没有 PowerShell 威胁面，恒返回空违规。
pub async fn audit_scripts(scripts: &[String]) -> Result<Vec<ScriptAudit>, String> {
    #[cfg(not(windows))]
    {
        let _ = scripts;
        return Ok(scripts.iter().map(|_| clean_audit()).collect());
    }

    #[cfg(windows)]
    {
        if scripts.len() > MAX_BATCH {
            return Err(format!("单批审计脚本数超上限（{MAX_BATCH}）"));
        }
        if scripts.is_empty() {
            return Ok(Vec::new());
        }
        let payload = serde_json::json!({
            "scripts": scripts
                .iter()
                .map(|s| base64::engine::general_purpose::STANDARD.encode(s.as_bytes()))
                .collect::<Vec<_>>()
        });

        let mut cmd = crate::utils::process::silent_command_async("powershell");
        cmd.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(audit_ps1_path())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("启动审计进程失败: {e}"))?;
        crate::utils::process::assign_child_to_job(&child);

        use tokio::io::AsyncWriteExt;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(payload.to_string().as_bytes())
                .await
                .map_err(|e| format!("写审计输入失败: {e}"))?;
            drop(stdin);
        }

        let output = tokio::time::timeout(
            std::time::Duration::from_secs(AUDIT_TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| "审计进程超时".to_string())?
        .map_err(|e| format!("审计进程执行失败: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "审计进程异常退出（{}）: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).chars().take(300).collect::<String>()
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let audits = parse_audit_output(&stdout, scripts.len())?;
        for (script, audit) in scripts.iter().zip(&audits) {
            cache_put(script, audit);
        }
        Ok(audits)
    }
}

/// 解析审计器输出（容忍 PS 5.1 单元素数组的标量退化）并分类
fn parse_audit_output(stdout: &str, expected: usize) -> Result<Vec<ScriptAudit>, String> {
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).map_err(|e| format!("审计输出解析失败: {e}"))?;
    let results: Vec<serde_json::Value> = match &value {
        serde_json::Value::Array(arr) => arr.clone(),
        v => match v.get("results") {
            Some(serde_json::Value::Array(arr)) => arr.clone(),
            Some(single) => vec![single.clone()],
            None => return Err("审计输出缺 results".to_string()),
        },
    };
    if results.len() != expected {
        return Err(format!(
            "审计结果数不匹配（期望 {expected}，得到 {}）",
            results.len()
        ));
    }
    Ok(results.into_iter().map(|r| classify(&r)).collect())
}

/// 把审计器原始输出分类为判定结果
fn classify(raw: &serde_json::Value) -> ScriptAudit {
    let str_list = |key: &str| -> Vec<String> {
        match raw.get(key) {
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            Some(serde_json::Value::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        }
    };
    let commands: Vec<String> = str_list("commands");
    let parameters: Vec<String> = str_list("parameters");
    let types: Vec<String> = str_list("types");
    let parse_errors = raw
        .get("parse_errors")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);

    let mut violations = Vec::new();
    if parse_errors > 0 {
        // 解析失败的脚本行为不可预判（包括利用解析器差异的构造），拒绝
        violations.push(format!("脚本存在 {parse_errors} 处语法错误，无法审计"));
    }
    for cmd in &commands {
        let lower = cmd.to_ascii_lowercase();
        let base = lower.rsplit(['\\', '/']).next().unwrap_or(&lower);
        let base = base.strip_suffix(".exe").unwrap_or(base);
        if HARD_BLOCK_COMMANDS.contains(&base) {
            violations.push(format!("包含被禁止的命令「{cmd}」"));
        }
    }
    for param in &parameters {
        if HARD_BLOCK_PARAMETERS.contains(&param.to_ascii_lowercase().as_str()) {
            violations.push(format!("包含被禁止的参数「-{param}」"));
        }
    }
    for ty in &types {
        // 取泛型/数组标记前的首个类型分量，再比前缀
        let lower = ty.to_ascii_lowercase();
        let first = lower
            .trim_start_matches('[')
            .split(['[', ','])
            .next()
            .unwrap_or(&lower)
            .trim()
            .to_string();
        if HARD_BLOCK_TYPE_PREFIXES.iter().any(|p| first.starts_with(p)) {
            violations.push(format!("引用被禁止的 .NET 类型「{ty}」"));
        }
    }

    ScriptAudit { commands, parameters, types, parse_errors: parse_errors as usize, violations }
}

// ============================================================================
// 调用面：异步（执行期）与阻塞（装载/创建期）
// ============================================================================

/// 单脚本审计（异步入口，带缓存）。工具执行期的检查点——同一脚本首次审计后
/// 零进程开销。
pub async fn audit_cached(script: &str) -> Result<ScriptAudit, String> {
    let hash = script_hash(script);
    if let Some(a) = audit_cache().lock().unwrap().get(&hash) {
        return Ok(a.clone());
    }
    let mut v = audit_scripts(std::slice::from_ref(&script.to_string())).await?;
    Ok(v.remove(0))
}

/// 批量审计（阻塞入口，带缓存）。供同步装载路径用——只对未缓存的脚本起
/// 一次 PS 进程。不得在异步上下文调用；异步路径用 [`audit_cached`]。
pub fn audit_scripts_blocking(scripts: &[String]) -> Result<Vec<ScriptAudit>, String> {
    let mut out: Vec<Option<ScriptAudit>> = {
        let cache = audit_cache().lock().unwrap();
        scripts.iter().map(|s| cache.get(&script_hash(s)).cloned()).collect()
    };
    let missing: Vec<(usize, String)> = out
        .iter()
        .zip(scripts.iter())
        .enumerate()
        .filter(|(_, (slot, _))| slot.is_none())
        .map(|(i, (_, s))| (i, s.clone()))
        .collect();
    if !missing.is_empty() {
        let payloads: Vec<String> = missing.iter().map(|(_, s)| s.clone()).collect();
        let fresh = tokio::runtime::Runtime::new()
            .map_err(|e| format!("创建审计运行时失败: {e}"))?
            .block_on(audit_scripts(&payloads))?;
        for ((i, script), audit) in missing.into_iter().zip(fresh) {
            cache_put(&script, &audit);
            out[i] = Some(audit);
        }
    }
    Ok(out.into_iter().map(|a| a.unwrap_or_else(clean_audit)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn benign_script_passes() {
        let a = audit_cached(
            r#"$args = [Console]::In.ReadToEnd() | ConvertFrom-Json; Get-ChildItem $args.path | Select-Object Name; Write-Output "done""#,
        )
        .await
        .expect("审计进程应成功");
        assert!(a.is_clean(), "良性脚本不应有违规: {:?}", a.violations);
        assert!(a.commands.iter().any(|c| c.eq_ignore_ascii_case("Get-ChildItem")));
    }

    #[tokio::test]
    async fn dynamic_execution_rejected_even_by_alias() {
        // iex 是 Invoke-Expression 的别名——碎片黑名单挡不住的典型绕过
        for script in ["iex 'Get-Process'", "Invoke-Expression 'Get-Process'"] {
            let a = audit_cached(script).await.expect("审计应成功");
            assert!(!a.is_clean(), "「{script}」应被拒绝");
            assert!(a.violations.iter().any(|v| v.contains("Invoke-Expression")));
        }
    }

    #[tokio::test]
    async fn encoded_command_and_start_process_rejected() {
        let a = audit_cached("Start-Process notepad").await.expect("审计应成功");
        assert!(a.violations.iter().any(|v| v.contains("Start-Process")));

        let b = audit_cached("powershell -EncodedCommand AAAA").await.expect("审计应成功");
        assert!(b.violations.iter().any(|v| v.contains("EncodedCommand")));
    }

    #[tokio::test]
    async fn dangerous_dotnet_types_rejected() {
        // New-Object 的类型名是字符串实参，AST 上不是 TypeExpression
        let a = audit_cached("New-Object System.Net.WebClient").await.expect("审计应成功");
        assert!(a.violations.iter().any(|v| v.contains("System.Net.WebClient")));

        let b = audit_cached("[System.Diagnostics.Process]::Start('notepad')").await.expect("审计应成功");
        assert!(b.violations.iter().any(|v| v.contains("System.Diagnostics.Process")));

        let c = audit_cached("Add-Type -TypeDefinition 'class X {}'").await.expect("审计应成功");
        assert!(c.violations.iter().any(|v| v.contains("Add-Type")));
    }

    #[tokio::test]
    async fn syntax_error_rejected() {
        let a = audit_cached("function { broken").await.expect("审计应成功");
        assert!(a.violations.iter().any(|v| v.contains("语法错误")));
    }

    #[test]
    fn blocking_entry_matches_async() {
        // 阻塞入口（装载路径）与异步入口判定一致
        let a = audit_scripts_blocking(&["iex 'dir'".to_string()])
            .expect("审计应成功")
            .remove(0);
        assert!(!a.is_clean());
        let b = audit_scripts_blocking(&["Get-Date".to_string()])
            .expect("审计应成功")
            .remove(0);
        assert!(b.is_clean());
    }
}
