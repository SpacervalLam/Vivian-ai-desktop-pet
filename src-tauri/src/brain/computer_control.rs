//! 电脑控制执行引擎。
//!
//! 简化实现策略（避免过度工程化）：
//! - 仅定义 `ComputerController` trait + 基本命令（打开应用、关闭应用、打开网址、执行 shell）
//! - 内置 Windows 平台的 `Win32ComputerController`，使用 `std::process::Command`
//!   + `tasklist`/`taskkill` 实现进程检查与关闭
//! - 不实现动态 exec 沙箱，因为这会引入脚本引擎依赖
//!   且存在安全风险；`execute_shell` 已禁用
//! - 应用映射表（app_map）

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::utils::process::silent_command;

/// 应用映射条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEntry {
    /// 进程名（可为多个候选，如浏览器）
    pub proc_names: Vec<String>,
    /// 启动命令
    pub launch_cmd: String,
}

impl AppEntry {
    pub fn single(proc_name: &str, launch_cmd: &str) -> Self {
        Self {
            proc_names: vec![proc_name.to_string()],
            launch_cmd: launch_cmd.to_string(),
        }
    }

    pub fn multi(proc_names: &[&str], launch_cmd: &str) -> Self {
        Self {
            proc_names: proc_names.iter().map(|s| s.to_string()).collect(),
            launch_cmd: launch_cmd.to_string(),
        }
    }
}

/// 电脑控制执行 trait
pub trait ComputerController: Send + Sync {
    /// 打开应用，返回人类可读的结果文本
    fn open_app(&self, app_name: &str) -> VivianResult<String>;

    /// 关闭应用
    fn close_app(&self, app_name: &str) -> VivianResult<String>;

    /// 在默认浏览器中打开网址
    fn open_url(&self, url: &str) -> VivianResult<String>;

    /// 受限 shell 命令执行
    fn execute_shell(&self, command: &str) -> VivianResult<String>;

    /// 检查进程是否正在运行
    fn is_process_running(&self, process_name: &str) -> bool;
}

/// Windows 平台实现
pub struct Win32ComputerController {
    app_map: HashMap<String, AppEntry>,
    /// 单实例应用列表（避免重复打开）
    singleton_procs: Vec<String>,
}

impl Win32ComputerController {
    pub fn new() -> Self {
        let mut app_map: HashMap<String, AppEntry> = HashMap::new();
        app_map.insert("微信".to_string(), AppEntry::single("WeChat.exe", "WeChat.exe"));
        app_map.insert("wechat".to_string(), AppEntry::single("WeChat.exe", "WeChat.exe"));
        app_map.insert(
            "浏览器".to_string(),
            AppEntry::multi(&["msedge.exe", "chrome.exe"], "msedge.exe"),
        );
        app_map.insert("edge".to_string(), AppEntry::single("msedge.exe", "msedge.exe"));
        app_map.insert("chrome".to_string(), AppEntry::single("chrome.exe", "chrome.exe"));
        app_map.insert("记事本".to_string(), AppEntry::single("notepad.exe", "notepad.exe"));
        app_map.insert("计算器".to_string(), AppEntry::single("CalculatorApp.exe", "calc.exe"));
        app_map.insert("音乐".to_string(), AppEntry::single("CloudMusic.exe", "cloudmusic.exe"));
        app_map.insert("vscode".to_string(), AppEntry::single("Code.exe", "Code.exe"));
        app_map.insert("cmd".to_string(), AppEntry::single("cmd.exe", "cmd.exe"));
        app_map.insert("任务管理器".to_string(), AppEntry::single("Taskmgr.exe", "taskmgr.exe"));
        app_map.insert("文件资源管理器".to_string(), AppEntry::single("explorer.exe", "explorer.exe"));
        app_map.insert("资源管理器".to_string(), AppEntry::single("explorer.exe", "explorer.exe"));
        app_map.insert("画图".to_string(), AppEntry::single("mspaint.exe", "mspaint.exe"));

        Self {
            app_map,
            singleton_procs: vec![
                "WeChat.exe".to_string(),
                "CloudMusic.exe".to_string(),
                "msedge.exe".to_string(),
                "chrome.exe".to_string(),
            ],
        }
    }

    /// 模糊查找应用映射（按子串匹配键名）
    fn lookup(&self, app_name: &str) -> Option<&AppEntry> {
        let lower = app_name.to_lowercase();
        if let Some(entry) = self.app_map.get(app_name) {
            return Some(entry);
        }
        for (key, entry) in &self.app_map {
            if key.to_lowercase().contains(&lower) || lower.contains(&key.to_lowercase()) {
                return Some(entry);
            }
        }
        None
    }

    fn is_singleton(&self, proc_name: &str) -> bool {
        self.singleton_procs.iter().any(|p| p.eq_ignore_ascii_case(proc_name))
    }
}

impl Default for Win32ComputerController {
    fn default() -> Self {
        Self::new()
    }
}

impl ComputerController for Win32ComputerController {
    fn open_app(&self, app_name: &str) -> VivianResult<String> {
        tracing::info!(app = %app_name, "尝试打开应用");

        let entry = match self.lookup(app_name) {
            Some(e) => e,
            None => {
                return Err(VivianError::Tool(format!(
                    "未找到应用 '{}'，仅允许打开白名单中的应用",
                    app_name
                )));
            }
        };
        let launch_cmd = entry.launch_cmd.clone();

        for proc in &entry.proc_names {
            if self.is_singleton(proc) && self.is_process_running(proc) {
                return Ok(format!("{} 已经在运行啦，不需要重复打开哦~", app_name));
            }
        }

        let result = silent_command("cmd")
            .args(["/C", "start", "", &launch_cmd])
            .spawn();

        match result {
            Ok(_) => Ok(format!("正在启动 {}...", app_name)),
            Err(e) => {
                let msg = format!("无法打开 {}: {}", app_name, e);
                tracing::error!(error = %msg);
                Err(VivianError::Tool(msg))
            }
        }
    }

    fn close_app(&self, app_name: &str) -> VivianResult<String> {
        tracing::info!(app = %app_name, "尝试关闭应用");

        let target_procs: Vec<String> = if let Some(entry) = self.lookup(app_name) {
            entry.proc_names.clone()
        } else {
            vec![format!("{}.exe", app_name)]
        };

        let mut killed: Vec<String> = Vec::new();
        let mut failed: Vec<(String, String)> = Vec::new();
        for proc in &target_procs {
            match silent_command("taskkill")
                .args(["/F", "/IM", proc])
                .output()
            {
                Ok(out) => {
                    if out.status.success() {
                        killed.push(proc.clone());
                    } else {
                        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                        // 128/126 通常是进程不存在，不算硬失败
                        let msg = if stderr.is_empty() {
                            format!("exit code {}", out.status.code().unwrap_or(-1))
                        } else {
                            stderr
                        };
                        failed.push((proc.clone(), msg));
                    }
                }
                Err(e) => {
                    tracing::warn!(proc = %proc, error = %e, "[close_app] taskkill 启动失败");
                    failed.push((proc.clone(), e.to_string()));
                }
            }
        }

        if killed.is_empty() && !failed.is_empty() {
            return Err(VivianError::Tool(format!(
                "关闭 {} 失败: {}",
                app_name,
                failed
                    .iter()
                    .map(|(p, m)| format!("{}({})", p, m))
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }

        Ok(format!(
            "已关闭 {} 进程{}",
            app_name,
            if failed.is_empty() {
                String::new()
            } else {
                format!("，部分失败: {}", failed.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join(","))
            }
        ))
    }

    fn open_url(&self, url: &str) -> VivianResult<String> {
        // 仅允许 http/https 协议
        let lower = url.to_lowercase();
        if !(lower.starts_with("http://") || lower.starts_with("https://")) {
            tracing::warn!(url = %url, "[security] open_url 拒绝非 http(s) URL");
            return Err(VivianError::Tool(format!(
                "仅允许打开 http/https 网址，拒绝: {}",
                url
            )));
        }
        tracing::info!(url = %url, "尝试打开网址");
        // 使用 cmd /C start 打开默认浏览器
        let result = silent_command("cmd")
            .args(["/C", "start", "", url])
            .spawn();

        match result {
            Ok(_) => Ok(format!("已在浏览器中打开 {}", url)),
            Err(e) => {
                let msg = format!("无法打开网址 {}: {}", url, e);
                tracing::error!(error = %msg);
                Err(VivianError::Tool(msg))
            }
        }
    }

    fn execute_shell(&self, command: &str) -> VivianResult<String> {
        tracing::warn!(
            cmd_preview = %&command[..command.len().min(80)],
            "[security] execute_shell 已禁用（防止 RCE），拒绝执行 LLM 传入的命令"
        );
        Err(VivianError::Tool(format!(
            "出于安全考虑，shell 命令执行已被禁用。如需打开应用请使用 open_application，打开网址请使用 open_url。"
        )))
    }

    fn is_process_running(&self, process_name: &str) -> bool {
        let output = silent_command("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {}", process_name)])
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                stdout.to_lowercase().contains(&process_name.to_lowercase())
            }
            Err(_) => false,
        }
    }
}

/// 全局控制器单例（OnceCell）
static CONTROLLER: once_cell::sync::OnceCell<Win32ComputerController> = once_cell::sync::OnceCell::new();

/// 获取全局 Windows 电脑控制器
pub fn get_computer_controller() -> &'static Win32ComputerController {
    CONTROLLER.get_or_init(Win32ComputerController::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_map_lookup() {
        let ctrl = Win32ComputerController::new();
        assert!(ctrl.lookup("微信").is_some());
        assert!(ctrl.lookup("wechat").is_some());
        assert!(ctrl.lookup("浏览器").is_some());
        // 模糊匹配
        assert!(ctrl.lookup("文件资源管理器").is_some());
    }

    #[test]
    fn test_singleton_detection() {
        let ctrl = Win32ComputerController::new();
        assert!(ctrl.is_singleton("WeChat.exe"));
        assert!(ctrl.is_singleton("chrome.exe"));
        assert!(!ctrl.is_singleton("notepad.exe"));
    }

    #[test]
    fn test_app_entry_multi() {
        let entry = AppEntry::multi(&["a.exe", "b.exe"], "a.exe");
        assert_eq!(entry.proc_names.len(), 2);
        assert_eq!(entry.launch_cmd, "a.exe");
    }
}
