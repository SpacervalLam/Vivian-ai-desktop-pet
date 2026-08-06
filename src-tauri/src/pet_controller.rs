//! 桌宠控制器
//!
//! 提供 7 种控制命令枚举（`ControlCommandType`）与窗口控制功能，
//! 集成新实现的 engine 模块（`AnimationManager`/`ExpressionManager`/`StateMachine`）。
//!
//! 线程安全：内部状态使用 `parking_lot::RwLock` 保护，管理器以 `Arc` 共享。
//! 序列化：命令类型与执行结果派生 `serde::Serialize`/`Deserialize`。

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tauri::WebviewWindow;

use crate::engine::animation::AnimationManager;
use crate::engine::expression::ExpressionManager;
use crate::engine::resource_loader::ResourceLoader;
use crate::engine::state_machine::StateMachine;

/// 窗口最小尺寸（像素）
pub const PET_MIN_SIZE: i32 = 100;
/// 窗口最大尺寸（像素）
pub const PET_MAX_SIZE: i32 = 2000;
/// 优先级上限
pub const PRIORITY_MAX: u32 = 200;

/// 控制命令类型枚举（6 种）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
#[allow(non_camel_case_types)]
pub enum ControlCommandType {
    /// 动作播放
    MOTION = 1,
    /// 表情设置
    EXPRESSION = 2,
    /// 鼠标跟随
    MOUSE_FOLLOW = 3,
    /// 窗口尺寸
    WINDOW_SIZE = 4,
    /// 窗口位置
    WINDOW_POSITION = 5,
    /// 透明度
    OPACITY = 6,
}

impl ControlCommandType {
    /// 从数值构造命令类型
    pub fn from_value(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::MOTION),
            2 => Some(Self::EXPRESSION),
            3 => Some(Self::MOUSE_FOLLOW),
            4 => Some(Self::WINDOW_SIZE),
            5 => Some(Self::WINDOW_POSITION),
            6 => Some(Self::OPACITY),
            _ => None,
        }
    }

    /// 获取命令名称
    pub fn name(&self) -> &'static str {
        match self {
            Self::MOTION => "MOTION",
            Self::EXPRESSION => "EXPRESSION",
            Self::MOUSE_FOLLOW => "MOUSE_FOLLOW",
            Self::WINDOW_SIZE => "WINDOW_SIZE",
            Self::WINDOW_POSITION => "WINDOW_POSITION",
            Self::OPACITY => "OPACITY",
        }
    }
}

/// 控制器执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlResult {
    /// 是否成功
    pub success: bool,
    /// 结果消息
    pub message: String,
    /// 附加数据（可选）
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<serde_json::Value>,
}

impl ControlResult {
    /// 构造成功结果
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            data: None,
        }
    }

    /// 构造失败结果
    pub fn fail(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            data: None,
        }
    }

    /// 附加数据
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// 控制器内部状态
#[derive(Debug, Clone, Default)]
struct PetControllerInner {
    /// 鼠标跟随是否启用
    mouse_follow_enabled: bool,
    /// 智能躲避鼠标是否启用
    avoid_mouse_enabled: bool,
}

/// 桌宠控制器
///
/// 线程安全：所有可变状态通过 `parking_lot::RwLock` 保护，
/// 管理器以 `Arc` 共享，窗口句柄 `WebviewWindow` 本身可跨线程传递。
pub struct PetController {
    /// 角色 Live2D 显示窗口句柄（label = character_id）
    main_window: RwLock<Option<WebviewWindow>>,
    /// 动画管理器
    animation_manager: RwLock<Option<Arc<AnimationManager>>>,
    /// 表情管理器
    expression_manager: RwLock<Option<Arc<ExpressionManager>>>,
    /// 状态机
    state_machine: RwLock<Option<Arc<StateMachine>>>,
    /// 资源加载器
    resource_loader: RwLock<Option<Arc<ResourceLoader>>>,
    /// 内部状态（鼠标跟随/睡眠/躲避鼠标）
    inner: RwLock<PetControllerInner>,
    /// 角色 ID（用于事件 payload 中标识来源角色）
    character_id: RwLock<String>,
}

impl Default for PetController {
    fn default() -> Self {
        Self::new()
    }
}

impl PetController {
    /// 创建新的控制器实例
    pub fn new() -> Self {
        Self {
            main_window: RwLock::new(None),
            animation_manager: RwLock::new(None),
            expression_manager: RwLock::new(None),
            state_machine: RwLock::new(None),
            resource_loader: RwLock::new(None),
            inner: RwLock::new(PetControllerInner::default()),
            character_id: RwLock::new(String::new()),
        }
    }

    /// 设置角色 ID（由 AppState 在创建角色实例时调用）
    pub fn set_character_id(&self, id: String) {
        *self.character_id.write() = id;
    }

    /// 获取角色 ID
    pub fn character_id(&self) -> String {
        self.character_id.read().clone()
    }

    /// 设置角色 Live2D 显示窗口
    ///
    /// 由 `lib.rs` 在创建角色窗口后调用，注入对应角色的 `WebviewWindow`。
    /// 未注入时，`set_window_position` / `set_window_size` / `set_opacity`
    /// 等全部因 `main_window=None` 静默失效。
    pub fn set_main_window(&self, window: WebviewWindow) {
        *self.main_window.write() = Some(window);
    }

    /// 注入管理器
    ///
    /// 传入 `None` 表示不修改对应管理器。
    pub fn set_managers(
        &self,
        animation_manager: Option<Arc<AnimationManager>>,
        expression_manager: Option<Arc<ExpressionManager>>,
        state_machine: Option<Arc<StateMachine>>,
    ) {
        if let Some(am) = animation_manager {
            *self.animation_manager.write() = Some(am);
        }
        if let Some(em) = expression_manager {
            *self.expression_manager.write() = Some(em);
        }
        if let Some(sm) = state_machine {
            *self.state_machine.write() = Some(sm);
        }
    }

    /// 注入资源加载器
    pub fn set_resource_loader(&self, resource_loader: Arc<ResourceLoader>) {
        *self.resource_loader.write() = Some(resource_loader);
    }

    /// 启动引擎（启动状态机空闲定时器等）
    ///
    /// 需在 tokio runtime 上下文中调用（如 Tauri 的 setup 回调内）。
    pub fn start(&self) {
        if let Some(sm) = self.state_machine.read().as_ref() {
            sm.start();
        }
    }

    /// 停止引擎（停止状态机空闲定时器）
    ///
    /// 角色离线时调用，避免后台空闲定时器向已隐藏的窗口 emit 事件。
    pub fn stop(&self) {
        if let Some(sm) = self.state_machine.read().as_ref() {
            sm.stop();
        }
    }

    /// 获取角色 Live2D 显示窗口的克隆
    fn get_window(&self) -> Option<WebviewWindow> {
        self.main_window.read().as_ref().cloned()
    }

    // ==================== 动作控制 ====================

    /// 播放指定动作
    ///
    /// # 参数
    /// - `name`: 动作名称
    /// - `priority`: 优先级 (0-200)，值越高优先级越高
    /// - `interruptible`: 是否可被打断
    /// - `loop`: 是否循环播放
    ///
    /// # 校验
    /// - 动作名称非空
    /// - 优先级在 0-200 范围内
    pub fn play_motion(
        &self,
        name: &str,
        priority: u32,
        interruptible: bool,
        r#loop: bool,
    ) -> ControlResult {
        let am = match self.animation_manager.read().as_ref() {
            Some(am) => am.clone(),
            None => return ControlResult::fail("AnimationManager未初始化"),
        };

        if name.trim().is_empty() {
            return ControlResult::fail("动作名称无效");
        }

        // u32 无负数，仅校验上限
        if priority > PRIORITY_MAX {
            return ControlResult::fail("优先级必须在0-200范围内");
        }

        // 通过 manifest 归一化动作名（语义名 → model3.json Name）
        let mapped_name = {
            let char_id = self.character_id.read().clone();
            crate::character_registry::get_manifest(&char_id)
                .map(|m| m.normalize_motion(name))
                .unwrap_or_else(|| name.to_string())
        };

        let result = am.play_motion(&mapped_name, priority, interruptible, r#loop);

        match result {
            Some(_) => ControlResult::ok(format!("动作 '{}' 已开始播放", name))
                .with_data(serde_json::json!({
                    "motion_name": mapped_name,
                    "priority": priority
                })),
            None => ControlResult::fail(format!("未找到动作 '{}'", name)),
        }
    }

    /// 停止当前播放的动作
    pub fn stop_motion(&self, force: bool) -> ControlResult {
        let am = match self.animation_manager.read().as_ref() {
            Some(am) => am.clone(),
            None => return ControlResult::fail("AnimationManager未初始化"),
        };

        let success = am.stop_motion(force);
        ControlResult {
            success,
            message: if success {
                "动作已停止".to_string()
            } else {
                "无法停止当前动作".to_string()
            },
            data: None,
        }
    }

    /// 停止所有动作
    pub fn stop_all_motions(&self) -> ControlResult {
        let am = match self.animation_manager.read().as_ref() {
            Some(am) => am.clone(),
            None => return ControlResult::fail("AnimationManager未初始化"),
        };

        am.stop_all_motions();
        ControlResult::ok("所有动作已停止")
    }

    // ==================== 表情控制 ====================

    /// 设置表情
    ///
    /// # 参数
    /// - `name`: 表情名称
    /// - `duration_ms`: 表情持续时间（毫秒），`None` 表示永久
    /// - `force`: 是否强制覆盖当前表情
    pub fn set_expression(
        &self,
        name: &str,
        duration_ms: Option<u64>,
        force: bool,
    ) -> ControlResult {
        let em = match self.expression_manager.read().as_ref() {
            Some(em) => em.clone(),
            None => return ControlResult::fail("ExpressionManager未初始化"),
        };

        if name.trim().is_empty() {
            return ControlResult::fail("表情名称无效");
        }

        // u64 已是非负整数，无需额外校验

        // engine 的 set_expression 多了一个 priority 参数，传 0 表示普通优先级
        let success = em.set_expression(name, duration_ms, force, 0);

        ControlResult {
            success,
            message: if success {
                format!("表情 '{}' 已设置", name)
            } else {
                format!("无法设置表情 '{}'", name)
            },
            data: Some(serde_json::json!({ "expression_name": name })),
        }
    }

    /// 重置表情为默认状态
    pub fn reset_expression(&self) -> ControlResult {
        let em = match self.expression_manager.read().as_ref() {
            Some(em) => em.clone(),
            None => return ControlResult::fail("ExpressionManager未初始化"),
        };

        em.reset_expression();
        ControlResult::ok("表情已重置为默认")
    }

    // ==================== 鼠标跟随 ====================

    /// 设置鼠标跟随状态
    ///
    /// Rust 版没有 `Live2DWidget`，状态保存在控制器内部。
    pub fn mouse_follow(&self, enabled: bool) -> ControlResult {
        {
            let mut inner = self.inner.write();
            inner.mouse_follow_enabled = enabled;
        }

        ControlResult::ok(format!(
            "鼠标跟随已{}",
            if enabled { "开启" } else { "关闭" }
        ))
        .with_data(serde_json::json!({ "enabled": enabled }))
    }

    /// 获取鼠标跟随状态
    pub fn get_mouse_follow(&self) -> ControlResult {
        let enabled = self.inner.read().mouse_follow_enabled;
        ControlResult::ok("获取鼠标跟随状态成功")
            .with_data(serde_json::json!({ "enabled": enabled }))
    }

    // ==================== 窗口控制 ====================

    /// 设置窗口尺寸
    ///
    /// # 校验
    /// - 宽高必须为正整数
    /// - 宽高钳制到 [PET_MIN_SIZE, PET_MAX_SIZE] 范围
    /// - 多显示器边界钳制：获取窗口所在屏幕几何信息，确保窗口不会超出当前屏幕的右/下边界，
    ///   并保留 10px 边距
    pub fn set_window_size(&self, width: i32, height: i32) -> ControlResult {
        let window = match self.get_window() {
            Some(w) => w,
            None => return ControlResult::fail("角色窗口未初始化"),
        };

        if width <= 0 {
            return ControlResult::fail("宽度必须为正整数");
        }
        if height <= 0 {
            return ControlResult::fail("高度必须为正整数");
        }

        // 钳制到 [PET_MIN_SIZE, PET_MAX_SIZE]
        let mut width = width.clamp(PET_MIN_SIZE, PET_MAX_SIZE);
        let mut height = height.clamp(PET_MIN_SIZE, PET_MAX_SIZE);

        // 多显示器边界钳制：获取窗口当前所在屏幕，失败则回退到主屏幕
        if let Ok(Some(monitor)) = window.current_monitor() {
            // 屏幕物理坐标（多显示器场景下可能为负值）
            let screen_pos = monitor.position();
            let screen_size = monitor.size();
            let screen_x = screen_pos.x;
            let screen_y = screen_pos.y;
            let screen_w = screen_size.width as i32;
            let screen_h = screen_size.height as i32;

            // 窗口当前物理位置
            if let Ok(win_pos) = window.outer_position() {
                // 相对于屏幕左上角的窗口坐标
                let rel_x = win_pos.x - screen_x;
                let rel_y = win_pos.y - screen_y;

                // 确保窗口不会超出屏幕右侧或底部
                let max_available_width = screen_w - rel_x;
                let max_available_height = screen_h - rel_y;

                // 保持至少 10px 的边距，避免完全超出屏幕
                let capped_w = max_available_width - 10;
                let capped_h = max_available_height - 10;

                if capped_w > 0 {
                    width = width.min(capped_w);
                }
                if capped_h > 0 {
                    height = height.min(capped_h);
                }

                // 确保最小尺寸仍然得到尊重
                width = width.max(PET_MIN_SIZE);
                height = height.max(PET_MIN_SIZE);
            }
        }

        use tauri::PhysicalSize;
        match window.set_size(PhysicalSize::new(width as u32, height as u32)) {
            Ok(_) => ControlResult::ok(format!("窗口尺寸已设置为 {}x{}", width, height))
                .with_data(serde_json::json!({ "width": width, "height": height })),
            Err(e) => ControlResult::fail(format!("设置窗口尺寸失败: {}", e)),
        }
    }

    /// 设置窗口位置
    pub fn set_window_position(&self, x: i32, y: i32) -> ControlResult {
        let window = match self.get_window() {
            Some(w) => w,
            None => return ControlResult::fail("角色窗口未初始化"),
        };

        use tauri::PhysicalPosition;
        match window.set_position(PhysicalPosition::new(x, y)) {
            Ok(_) => ControlResult::ok(format!("窗口位置已设置为 ({}, {})", x, y))
                .with_data(serde_json::json!({ "x": x, "y": y })),
            Err(e) => ControlResult::fail(format!("设置窗口位置失败: {}", e)),
        }
    }

    /// 获取窗口位置
    pub fn get_window_position(&self) -> ControlResult {
        let window = match self.get_window() {
            Some(w) => w,
            None => return ControlResult::fail("角色窗口未初始化"),
        };

        match window.outer_position() {
            Ok(pos) => ControlResult::ok("获取窗口位置成功")
                .with_data(serde_json::json!({ "x": pos.x, "y": pos.y })),
            Err(e) => ControlResult::fail(format!("获取窗口位置失败: {}", e)),
        }
    }

    /// 获取窗口尺寸
    pub fn get_window_size(&self) -> ControlResult {
        let window = match self.get_window() {
            Some(w) => w,
            None => return ControlResult::fail("角色窗口未初始化"),
        };

        match window.outer_size() {
            Ok(size) => ControlResult::ok("获取窗口尺寸成功").with_data(serde_json::json!({
                "width": size.width,
                "height": size.height
            })),
            Err(e) => ControlResult::fail(format!("获取窗口尺寸失败: {}", e)),
        }
    }

    // ==================== 透明度 ====================

    /// 设置窗口透明度
    ///
    /// # 参数
    /// - `opacity`: 透明度值 (0.0-1.0)，0 为完全透明，1 为完全不透明
    #[cfg(windows)]
    pub fn set_opacity(&self, opacity: f64) -> ControlResult {
        let window = match self.get_window() {
            Some(w) => w,
            None => return ControlResult::fail("角色窗口未初始化"),
        };

        // 钳制到 [0.0, 1.0]
        let opacity = opacity.clamp(0.0, 1.0);

        // 获取窗口句柄（Tauri 的 hwnd() 返回其内部 windows crate 版本的 HWND）
        let hwnd_tauri = match window.hwnd() {
            Ok(h) => h,
            Err(e) => return ControlResult::fail(format!("获取窗口句柄失败: {}", e)),
        };

        use windows::Win32::Foundation::{COLORREF, HWND};
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE,
            LWA_ALPHA, WS_EX_LAYERED,
        };

        // 跨版本构造本项目 windows crate（0.58）的 HWND：
        // Tauri 内部使用 windows 0.61，两者 HWND 内部表示均为 *mut c_void，可直接转换
        let hwnd = HWND(hwnd_tauri .0);

        unsafe {
            // 添加 WS_EX_LAYERED 扩展样式以启用分层窗口
            let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style | (WS_EX_LAYERED.0 as isize));

            // 设置透明度（alpha 值 0-255，LWA_ALPHA 表示按 alpha 通道混合）
            let alpha = (opacity * 255.0) as u8;
            match SetLayeredWindowAttributes(hwnd, COLORREF(0), alpha, LWA_ALPHA) {
                Ok(()) => ControlResult::ok(format!("窗口透明度已设置为 {:.2}", opacity))
                    .with_data(serde_json::json!({ "opacity": opacity })),
                Err(e) => ControlResult::fail(format!("设置窗口透明度失败: {}", e)),
            }
        }
    }

    /// 设置窗口透明度（非 Windows 平台回退实现）
    #[cfg(not(windows))]
    pub fn set_opacity(&self, opacity: f64) -> ControlResult {
        let _window = self.get_window();
        if _window.is_none() {
            return ControlResult::fail("角色窗口未初始化");
        }
        let _ = opacity;
        ControlResult::fail("当前平台不支持设置窗口透明度")
    }

    /// 获取窗口透明度
    ///
    /// 注：Tauri 2.x 未提供读取透明度的 API，返回默认值 1.0。
    pub fn get_opacity(&self) -> ControlResult {
        let _window = self.get_window();
        if _window.is_none() {
            return ControlResult::fail("角色窗口未初始化");
        }
        ControlResult::ok("获取窗口透明度成功")
            .with_data(serde_json::json!({ "opacity": 1.0 }))
    }

    // ==================== 智能躲避鼠标 ====================

    /// 设置智能躲避鼠标模式
    pub fn set_avoid_mouse(&self, enabled: bool) -> ControlResult {
        {
            let mut inner = self.inner.write();
            inner.avoid_mouse_enabled = enabled;
        }

        ControlResult::ok(format!(
            "智能躲避模式已{}",
            if enabled { "开启" } else { "关闭" }
        ))
        .with_data(serde_json::json!({ "enabled": enabled }))
    }

    /// 获取智能躲避鼠标模式状态
    pub fn get_avoid_mouse(&self) -> ControlResult {
        let enabled = self.inner.read().avoid_mouse_enabled;
        ControlResult::ok("获取智能躲避模式状态成功")
            .with_data(serde_json::json!({ "enabled": enabled }))
    }

    // ==================== 资源列表 ====================

    /// 获取可用的动作列表
    pub fn list_available_motions(&self) -> ControlResult {
        let am = match self.animation_manager.read().as_ref() {
            Some(am) => am.clone(),
            None => return ControlResult::fail("AnimationManager未初始化"),
        };

        let stats = am.get_statistics();
        ControlResult::ok("获取动作列表成功").with_data(serde_json::json!({
            "last_motion": stats.last_motion,
            "total_played": stats.total_motions_played,
            "queue_length": stats.queue_length,
        }))
    }

    /// 获取可用的表情列表
    pub fn list_available_expressions(&self) -> ControlResult {
        let em = match self.expression_manager.read().as_ref() {
            Some(em) => em.clone(),
            None => return ControlResult::fail("ExpressionManager未初始化"),
        };

        let expressions = em.list_expressions();
        ControlResult::ok("获取表情列表成功")
            .with_data(serde_json::json!({ "expressions": expressions }))
    }

    /// 获取资源加载器的引用（供命令层读取模型目录等）
    pub fn resource_loader(&self) -> Option<Arc<ResourceLoader>> {
        self.resource_loader.read().as_ref().cloned()
    }

    /// 获取当前模型信息（动作列表、表情列表、模型路径等）
    pub fn get_model_info(&self) -> serde_json::Value {
        let rl = match self.resource_loader.read().as_ref() {
            Some(rl) => rl.clone(),
            None => {
                return serde_json::json!({
                    "model_name": "unknown",
                    "error": "ResourceLoader未初始化",
                    "motions": [],
                    "expressions": [],
                })
            }
        };

        // 优先从当前角色的 manifest 获取模型显示名和表情列表
        let char_id = self.character_id.read().clone();
        let manifest_opt = if !char_id.is_empty() {
            crate::character_registry::get_manifest(&char_id)
        } else {
            None
        };
        // 从 manifest 获取 display_scale（留白补偿系数，默认 1.0）
        let display_scale = manifest_opt
            .as_ref()
            .and_then(|m| m.model_manifest())
            .map(|mf| mf.display_scale)
            .unwrap_or(1.0);
        let model_kind = manifest_opt
            .as_ref()
            .and_then(|m| m.model_manifest())
            .map(|mf| mf.model_kind.clone())
            .unwrap_or_else(|| "live2d".to_string());

        let (model_name, expressions, motions) =
            match manifest_opt {
                Some(m) => {
                    let name = m.model_manifest()
                        .map(|mf| mf.display_name.clone())
                        .unwrap_or_else(|| {
                            rl.get_preset("model")
                                .map(|p| p.name)
                                .unwrap_or_else(|| "unknown".to_string())
                        });
                    let exprs = m.expressions().to_vec();
                    let motions = m.motions().to_vec();
                    (name, exprs, motions)
                }
                None => {
                    let exprs = rl.list_expression_names();
                    let motions = rl.list_motion_names();
                    let name = rl
                        .get_preset("model")
                        .map(|p| p.name)
                        .unwrap_or_else(|| "unknown".to_string());
                    (name, exprs, motions)
                }
            };

        let model_preset = rl.get_preset("model");
        let model_path = model_preset
            .as_ref()
            .map(|p| p.path.clone())
            .unwrap_or_default();

        serde_json::json!({
            "model_name": model_name,
            "model_path": model_path,
            "model_dir": rl.model_dir().to_string_lossy(),
            "is_loaded": rl.is_loaded(),
            "motions": motions,
            "expressions": expressions,
            "motion_count": motions.len(),
            "expression_count": expressions.len(),
            "display_scale": display_scale,
            "model_kind": model_kind,
        })
    }

    /// 触发闲置动作
    ///
    /// 供前端命令直接调用，随机播放一个动作或临时表情。
    pub fn trigger_idle_action(&self) -> ControlResult {
        let sm = match self.state_machine.read().as_ref() {
            Some(sm) => sm.clone(),
            None => return ControlResult::fail("StateMachine未初始化"),
        };
        sm.trigger_random_idle_action();
        ControlResult::ok("已触发待机动作")
    }

    // ==================== 状态信息 ====================

    /// 获取桌宠当前状态的完整信息
    pub fn get_status(&self) -> ControlResult {
        let mut status = serde_json::Map::new();

        if let Some(am) = self.animation_manager.read().as_ref() {
            status.insert(
                "motion".to_string(),
                serde_json::to_value(am.get_statistics()).unwrap_or_default(),
            );
        }

        if let Some(em) = self.expression_manager.read().as_ref() {
            status.insert(
                "expression".to_string(),
                serde_json::to_value(em.get_statistics()).unwrap_or_default(),
            );
        }

        if let Some(sm) = self.state_machine.read().as_ref() {
            status.insert(
                "state".to_string(),
                serde_json::to_value(sm.get_statistics()).unwrap_or_default(),
            );
        }

        // 窗口位置
        let pos_result = self.get_window_position();
        if pos_result.success {
            if let Some(data) = pos_result.data {
                status.insert("window_position".to_string(), data);
            }
        }

        // 窗口尺寸
        let size_result = self.get_window_size();
        if size_result.success {
            if let Some(data) = size_result.data {
                status.insert("window_size".to_string(), data);
            }
        }

        // 鼠标跟随状态
        let inner = self.inner.read();
        status.insert(
            "mouse_follow".to_string(),
            serde_json::Value::Bool(inner.mouse_follow_enabled),
        );

        ControlResult::ok("获取状态成功")
            .with_data(serde_json::Value::Object(status))
    }

    // ==================== 命令分发 ====================

    /// 执行控制命令
    ///
    /// `command` 为 JSON 对象，需包含 `action` 字段和可选的 `params` 字段。
    pub fn execute_command(&self, command: &serde_json::Value) -> ControlResult {
        let obj = match command.as_object() {
            Some(obj) => obj,
            None => return ControlResult::fail("命令格式无效，需要JSON对象"),
        };

        let action = match obj.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return ControlResult::fail("命令格式无效，缺少action字段"),
        };

        let params = obj.get("params").cloned().unwrap_or(serde_json::Value::Null);

        match action {
            "play_motion" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let priority = params
                    .get("priority")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50) as u32;
                let interruptible = params
                    .get("interruptible")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                let r#loop = params.get("loop").and_then(|v| v.as_bool()).unwrap_or(false);
                self.play_motion(name, priority, interruptible, r#loop)
            }
            "stop_motion" => {
                let force = params.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                self.stop_motion(force)
            }
            "stop_all_motions" => self.stop_all_motions(),
            "set_expression" => {
                let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let duration_ms = params.get("duration_ms").and_then(|v| v.as_u64());
                let force = params.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                self.set_expression(name, duration_ms, force)
            }
            "reset_expression" => self.reset_expression(),
            "set_mouse_follow" | "mouse_follow" => {
                let enabled = params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                self.mouse_follow(enabled)
            }
            "get_mouse_follow" => self.get_mouse_follow(),
            "get_window_position" => self.get_window_position(),
            "get_window_size" => self.get_window_size(),
            "get_opacity" => self.get_opacity(),
            "set_avoid_mouse" => {
                let enabled = params.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                self.set_avoid_mouse(enabled)
            }
            "get_avoid_mouse" => self.get_avoid_mouse(),
            "list_motions" => self.list_available_motions(),
            "list_expressions" => self.list_available_expressions(),
            "get_status" => self.get_status(),
            _ => ControlResult::fail(format!("未知动作: {}", action)),
        }
    }

    /// 批量执行多个命令
    pub fn batch_execute(&self, commands: &[serde_json::Value]) -> Vec<ControlResult> {
        commands
            .iter()
            .map(|cmd| self.execute_command(cmd))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_type_values() {
        assert_eq!(ControlCommandType::MOTION as u8, 1);
        assert_eq!(ControlCommandType::EXPRESSION as u8, 2);
        assert_eq!(ControlCommandType::MOUSE_FOLLOW as u8, 3);
        assert_eq!(ControlCommandType::WINDOW_SIZE as u8, 4);
        assert_eq!(ControlCommandType::WINDOW_POSITION as u8, 5);
        assert_eq!(ControlCommandType::OPACITY as u8, 6);
    }

    #[test]
    fn test_command_type_from_value() {
        assert_eq!(ControlCommandType::from_value(1), Some(ControlCommandType::MOTION));
        assert_eq!(ControlCommandType::from_value(6), Some(ControlCommandType::OPACITY));
        assert_eq!(ControlCommandType::from_value(0), None);
        assert_eq!(ControlCommandType::from_value(7), None);
    }

    #[test]
    fn test_command_type_names() {
        assert_eq!(ControlCommandType::MOTION.name(), "MOTION");
        assert_eq!(ControlCommandType::OPACITY.name(), "OPACITY");
    }

    #[test]
    fn test_play_motion_no_manager() {
        let ctrl = PetController::new();
        let result = ctrl.play_motion("test", 50, true, false);
        assert!(!result.success);
        assert!(result.message.contains("AnimationManager"));
    }

    #[test]
    fn test_mouse_follow_toggle() {
        let ctrl = PetController::new();
        let result = ctrl.mouse_follow(true);
        assert!(result.success);
        let get_result = ctrl.get_mouse_follow();
        assert!(get_result.success);
        assert_eq!(get_result.data.unwrap()["enabled"], serde_json::json!(true));
    }

    #[test]
    fn test_avoid_mouse() {
        let ctrl = PetController::new();
        let result = ctrl.set_avoid_mouse(true);
        assert!(result.success);
        let state = ctrl.get_avoid_mouse();
        assert!(state.success);
        assert_eq!(state.data.unwrap()["enabled"], serde_json::json!(true));
    }

    #[test]
    fn test_execute_command_unknown_action() {
        let ctrl = PetController::new();
        let cmd = serde_json::json!({"action": "unknown_action", "params": {}});
        let result = ctrl.execute_command(&cmd);
        assert!(!result.success);
        assert!(result.message.contains("未知动作"));
    }

    #[test]
    fn test_execute_command_missing_action() {
        let ctrl = PetController::new();
        let cmd = serde_json::json!({"params": {}});
        let result = ctrl.execute_command(&cmd);
        assert!(!result.success);
        assert!(result.message.contains("action"));
    }

    #[test]
    fn test_execute_command_set_mouse_follow() {
        let ctrl = PetController::new();
        let cmd = serde_json::json!({"action": "set_mouse_follow", "params": {"enabled": true}});
        let result = ctrl.execute_command(&cmd);
        assert!(result.success);
        assert!(ctrl.get_mouse_follow().data.unwrap()["enabled"] == serde_json::json!(true));
    }

    #[test]
    fn test_batch_execute() {
        let ctrl = PetController::new();
        let commands = vec![
            serde_json::json!({"action": "set_mouse_follow", "params": {"enabled": true}}),
            serde_json::json!({"action": "set_avoid_mouse", "params": {"enabled": true}}),
        ];
        let results = ctrl.batch_execute(&commands);
        assert_eq!(results.len(), 2);
        assert!(results[0].success);
        assert!(results[1].success);
    }

    #[test]
    fn test_control_result_serialization() {
        let result = ControlResult::ok("测试").with_data(serde_json::json!({"key": "value"}));
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("测试"));
        assert!(json.contains("key"));
    }
}
