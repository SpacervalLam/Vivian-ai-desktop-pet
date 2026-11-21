//! 开机自动启动管理
//!
//! Windows 下通过当前用户的 `Run` 注册表键实现；
//! 非 Windows 平台暂不支持，保持 no-op（返回 Ok，便于跨平台编译与测试）。

/// 启用/禁用开机自动启动。
///
/// 写入注册表 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`，
/// 值名 `VivianDesktopPet`，值为当前可执行文件路径（带引号）。
/// 禁用时删除该值（值不存在也视为成功）。
pub fn set_auto_start(enabled: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;

        const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
        const VALUE_NAME: &str = "VivianDesktopPet";

        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let (key, _disp) = hkcu
            .create_subkey(RUN_KEY)
            .map_err(|e| format!("打开启动项注册表失败: {}", e))?;

        if enabled {
            let exe_path = std::env::current_exe()
                .map_err(|e| format!("获取当前程序路径失败: {}", e))?;
            let exe = exe_path.to_string_lossy().to_string();
            // 路径用引号包裹，防止空格路径被注册表/Shell 解析截断
            let value = format!("\"{}\"", exe);
            key.set_value(VALUE_NAME, &value)
                .map_err(|e| format!("写入启动项失败: {}", e))?;
            tracing::info!("[AutoStart] 已启用开机自动启动: {}", value);
        } else {
            match key.delete_value(VALUE_NAME) {
                Ok(()) => {
                    tracing::info!("[AutoStart] 已禁用开机自动启动");
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    tracing::info!("[AutoStart] 开机自动启动未启用，无需删除");
                }
                Err(e) => {
                    return Err(format!("删除启动项失败: {}", e));
                }
            }
        }
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = enabled;
        tracing::debug!("[AutoStart] 当前平台不支持开机自动启动，忽略");
        Ok(())
    }
}
