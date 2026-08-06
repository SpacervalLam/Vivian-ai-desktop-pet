//! PowerShell 执行工具 —— 统一封装 PowerShell 调用，供感知层和工具层复用。

use crate::utils::process::silent_command;

/// 同步执行 PowerShell 脚本，返回标准输出字符串。
///
/// 自动设置 UTF-8 编码，使用 `-NoProfile -NonInteractive` 避免加载用户配置。
pub fn run_ps(script: &str) -> Result<String, String> {
    let wrapped = format!(
        "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8; $OutputEncoding = [System.Text.Encoding]::UTF8; {}",
        script
    );
    let output = silent_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &wrapped])
        .output()
        .map_err(|e| format!("启动 PowerShell 失败: {}", e))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// 在阻塞线程池中执行 PowerShell 脚本，避免同步子进程阻塞异步运行时工作线程。
pub async fn run_ps_async(script: &str) -> Result<String, String> {
    let script = script.to_string();
    tokio::task::spawn_blocking(move || run_ps(&script))
        .await
        .map_err(|e| format!("PowerShell 任务执行失败: {}", e))?
}
