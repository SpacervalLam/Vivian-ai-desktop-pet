//! 内置工具注册 - 将所有内置工具注册到工具系统

use std::sync::Arc;

use super::registry::ToolSystem;

pub mod cross_character_tools;
pub mod diary_tools;
pub mod extended_system_ops;
pub mod file_tools;
pub mod input_control_tools;
pub mod media_tools;
pub mod memory_tools;
pub mod notebook_tools;
pub mod perception_tools;
pub mod pet_tools;
pub mod presence_tools;
pub mod relationship_tools;
pub mod research_tool;
pub mod scheduler_tools;
pub mod share_link_tool;
pub mod system_ops;
pub mod todo_tools;
pub mod wallpaper_tools;
pub mod weather_tools;
pub mod web_search_tool;

/// 注册所有内置工具到工具系统
pub fn register_builtin_tools(tool_system: &ToolSystem) {
    let tools: Vec<Arc<dyn super::types::Tool>> = vec![
        // 系统工具
        Arc::new(system_ops::OpenApplicationTool::new()),
        Arc::new(system_ops::CloseApplicationTool::new()),
        Arc::new(system_ops::TakeScreenshotTool::new()),
        Arc::new(system_ops::ScreenshotAnalyzeTool::new()),
        // 扩展系统工具
        Arc::new(extended_system_ops::OpenUrlTool::new()),
        Arc::new(extended_system_ops::GetActiveWindowTool::new()),
        // 记忆工具
        Arc::new(memory_tools::SaveMemoryTool::new()),
        Arc::new(memory_tools::SearchMemoryTool::new()),
        Arc::new(memory_tools::GetRecentInteractionsTool::new()),
        Arc::new(memory_tools::SummarizeTodayContextTool::new()),
        Arc::new(memory_tools::ReadDiaryByDateTool::new()),
        Arc::new(memory_tools::RecallByDateTimeTool::new()),
        // 智能日记工具（LLM 自主写日记，条件满足时可见）
        Arc::new(diary_tools::WriteDiaryTool::new()),
        // 宠物注视模式切换
        Arc::new(pet_tools::ToggleWatchModeTool::new()),
        // 待办工具
        Arc::new(todo_tools::AddTodoTool::new()),
        Arc::new(todo_tools::ListTodoTool::new()),
        Arc::new(todo_tools::CompleteTodoTool::new()),
        Arc::new(todo_tools::ManageTodoTool::new()),
        // 定时任务工具
        Arc::new(scheduler_tools::ScheduleReminderTool::new()),
        Arc::new(scheduler_tools::ManageScheduledTool::new()),
        // 在场状态工具（LLM 自主上下线）
        Arc::new(presence_tools::SetPresenceStateTool::new()),
        // 用户研究工具（LLM 主动观察用户行为习惯）
        Arc::new(research_tool::ObserveUserTool::new()),
        // 跨角色对话工具
        Arc::new(cross_character_tools::TalkToCharacterTool::new()),
        // 媒体控制工具（合并播放/暂停/上下首/音量/静音为单工具）
        Arc::new(media_tools::MediaControlTool::new()),
        // 桌面感知工具
        Arc::new(perception_tools::GetForegroundAppContextTool::new()),
        // 输入控制工具
        Arc::new(input_control_tools::ClickMouseTool::new()),
        Arc::new(input_control_tools::HotkeyTool::new()),
        Arc::new(input_control_tools::TypeTextTool::new()),
        // Wallpaper Engine 工具（list/set 保留，控制类合并为 wallpaper_control）
        Arc::new(wallpaper_tools::WallpaperListTool::new()),
        Arc::new(wallpaper_tools::WallpaperSetTool::new()),
        Arc::new(wallpaper_tools::WallpaperControlTool::new()),
        // 网络搜索（LLM 原生搜索的补充，支持 DuckDuckGo/SearXNG/Tavily）
        Arc::new(web_search_tool::WebSearchTool::new()),
        // 链接分享（搜索后分享有价值的链接卡片）
        Arc::new(share_link_tool::ShareLinkTool::new()),
        // 笔记本工具（创建/查看/修改/分享卡片风格 HTML 笔记）
        Arc::new(notebook_tools::CreateNotebookTool::new()),
        Arc::new(notebook_tools::ListNotebooksTool::new()),
        Arc::new(notebook_tools::GetNotebookDetailTool::new()),
        Arc::new(notebook_tools::UpdateNotebookTool::new()),
        Arc::new(notebook_tools::ShareNotebookTool::new()),
        // 完整 HTML 笔记（LLM 直接撰写自包含 HTML，经 Shadow DOM 渲染）
        Arc::new(notebook_tools::CreateHtmlNoteTool::default()),
        // 文件系统读取（按路径读本地文本/代码/HTML，受沙箱路径校验约束）
        Arc::new(file_tools::ReadFileTool::default()),
        // 天气预报（Open-Meteo 免费 API，支持 1~16 天预报）
        Arc::new(weather_tools::GetWeatherForecastTool::new()),
    ];

    let count = tools.len();
    for tool in &tools {
        tool_system.register_tool(Arc::clone(tool));
    }

    // 注册 ToolSearchTool（延迟工具搜索元工具）
    // 传入已注册工具的快照——工具注册只在启动时做一次，之后不变，快照足够
    let snapshot = Arc::new(tools.clone());
    tool_system.register_tool(Arc::new(
        crate::tools::tool_call_manager::ToolSearchTool::new(snapshot),
    ));

    tracing::info!("已注册 {} 个内置工具（含 tool_search 元工具）", count + 1);
}
