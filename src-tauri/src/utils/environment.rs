//! 环境管理器 - 监测窗口、活动状态、系统资源
//!
//! - 当前活动窗口标题
//! - 鼠标位置
//! - 系统时间
//! - CPU/内存使用率
//! - 电池状态
//! - 用户活动状态（键盘/鼠标空闲时间）

use std::sync::Arc;
use std::time::Instant;

use chrono::Datelike;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

/// 环境信息快照
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EnvironmentInfo {
    pub current_window: String,
    pub window_class: String,
    pub mouse_position: (i32, i32),
    pub system_time: String,
    pub cpu_usage: f64,
    pub memory_usage: f64,
    pub battery_level: i32,
    pub is_plugged_in: bool,
    pub network_status: String,
    pub keyboard_idle_seconds: f64,
    pub mouse_idle_seconds: f64,
}

/// 用户活动状态
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserActivity {
    pub keyboard_active: bool,
    pub keyboard_idle_time: f64,
    pub mouse_active: bool,
    pub mouse_idle_time: f64,
    pub is_idle: bool,
}

/// 当前精简状态（用于主动交互决策）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CurrentState {
    pub active_window: String,
    pub hour: u32,
    pub day_of_week: u32,
    pub is_work_hours: bool,
    pub is_night: bool,
    pub user_activity: UserActivity,
}

/// 环境管理器
pub struct EnvironmentManager {
    last_info: Arc<RwLock<EnvironmentInfo>>,
    last_update: Arc<RwLock<Instant>>,
    last_mouse_pos: Arc<RwLock<(i32, i32)>>,
    last_mouse_move: Arc<RwLock<Instant>>,
}

impl EnvironmentManager {
    pub fn new() -> Self {
        Self {
            last_info: Arc::new(RwLock::new(EnvironmentInfo::default())),
            last_update: Arc::new(RwLock::new(Instant::now())),
            last_mouse_pos: Arc::new(RwLock::new((0, 0))),
            last_mouse_move: Arc::new(RwLock::new(Instant::now())),
        }
    }

    /// 获取环境信息快照
    pub fn get_environment_info(&self) -> EnvironmentInfo {
        self.last_info.read().clone()
    }

    /// 获取当前精简状态
    pub fn get_current_state(&self) -> CurrentState {
        let info = self.last_info.read();
        let now = chrono::Local::now();
        let hour = now.format("%H").to_string().parse::<u32>().unwrap_or(12);
        let day_of_week = now.weekday().num_days_from_monday();

        CurrentState {
            active_window: info.current_window.clone(),
            hour,
            day_of_week,
            is_work_hours: (9..=18).contains(&hour) && day_of_week < 5,
            is_night: hour >= 22 || hour < 6,
            user_activity: UserActivity {
                keyboard_active: info.keyboard_idle_seconds < 5.0,
                keyboard_idle_time: info.keyboard_idle_seconds,
                mouse_active: info.mouse_idle_seconds < 5.0,
                mouse_idle_time: info.mouse_idle_seconds,
                is_idle: info.keyboard_idle_seconds > 60.0
                    && info.mouse_idle_seconds > 60.0,
            },
        }
    }

    /// 获取用户活动状态
    pub fn get_user_activity(&self) -> UserActivity {
        let info = self.last_info.read();
        UserActivity {
            keyboard_active: info.keyboard_idle_seconds < 5.0,
            keyboard_idle_time: info.keyboard_idle_seconds,
            mouse_active: info.mouse_idle_seconds < 5.0,
            mouse_idle_time: info.mouse_idle_seconds,
            is_idle: info.keyboard_idle_seconds > 60.0
                && info.mouse_idle_seconds > 60.0,
        }
    }

    /// 更新环境信息（由前端定时调用，传入当前鼠标位置和活动窗口）
    pub fn update(&self, mouse_pos: (i32, i32), active_window: String) {
        let now = Instant::now();
        let mut info = self.last_info.write();

        // 检测鼠标是否移动
        let mouse_moved = {
            let mut last_pos = self.last_mouse_pos.write();
            let moved = *last_pos != mouse_pos;
            *last_pos = mouse_pos;
            moved
        };

        if mouse_moved {
            let mut last_move = self.last_mouse_move.write();
            *last_move = now;
            info.mouse_idle_seconds = 0.0;
        } else {
            let last_move = self.last_mouse_move.read();
            info.mouse_idle_seconds = now.duration_since(*last_move).as_secs_f64();
        }

        info.mouse_position = mouse_pos;
        info.current_window = active_window;
        info.system_time = chrono::Local::now().to_rfc3339();

        // 在 Windows 上获取电池信息（简化实现）
        #[cfg(target_os = "windows")]
        {
            if let Some(battery) = get_battery_info() {
                info.battery_level = battery.0;
                info.is_plugged_in = battery.1;
            }
        }

        let mut last_update = self.last_update.write();
        *last_update = now;
    }

    /// 更新键盘活动状态
    pub fn update_keyboard_activity(&self) {
        let mut info = self.last_info.write();
        info.keyboard_idle_seconds = 0.0;
    }
}

impl Default for EnvironmentManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "windows")]
fn get_battery_info() -> Option<(i32, bool)> {
    use crate::utils::process::silent_command;
    let output = silent_command("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Add-Type -AssemblyName System.Windows.Forms; $p = [System.Windows.Forms.SystemInformation]::PowerStatus; Write-Output ($p.BatteryLifePercent * 100); Write-Output ($p.PowerLineStatus -eq 'Online')",
        ])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines = stdout.lines();
    let level = lines.next()?.parse::<i32>().ok()?;
    let plugged = lines.next()?.trim() == "True";
    Some((level, plugged))
}
