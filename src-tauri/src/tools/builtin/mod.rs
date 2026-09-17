//! 内置工具注册 - 将所有内置工具注册到工具系统

use std::sync::Arc;

use super::registry::ToolSystem;

pub mod cross_character_tools;
pub mod coding_tools;
pub mod diary_tools;
pub mod discovery_tools;
pub mod extended_system_ops;
pub mod file_tools;
pub mod input_control_tools;
pub mod jobs_tools;
pub mod lsp_tools;
pub mod media_tools;
pub mod memory_tools;
pub mod music_tools;
pub mod notebook_tools;
pub mod perception_tools;
pub mod pet_tools;
pub mod plan_tools;
pub mod plugin_tools;
pub mod provider_preset_tools;
pub mod presence_tools;
pub mod question_tools;
pub mod relationship_tools;
pub mod research_tool;
pub mod scheduler_tools;
pub mod send_image_tool;
pub mod share_link_tool;
pub mod show_widget_tool;
pub mod skill_tools;
pub mod subagent_tools;
pub mod system_ops;
pub mod thinking_tools;
pub mod todo_tools;
pub mod tool_tools;
pub mod wallpaper_tools;
pub mod wakeup_tool;
pub mod weather_tools;
pub mod web_fetch_tool;
pub mod web_search_tool;
pub mod work_agent_tools;
pub mod work_todo_tools;
pub mod work_question_tools;
pub mod work_subagent_tools;
pub mod work_worktree_tools;
pub mod workflow_tools;

/// 注册所有内置工具到工具系统
pub fn register_builtin_tools(tool_system: &Arc<ToolSystem>) {
    let mut tools: Vec<Arc<dyn super::types::Tool>> = vec![
        // 系统工具
        Arc::new(system_ops::OpenApplicationTool::new()),
        Arc::new(system_ops::CloseApplicationTool::new()),
        Arc::new(system_ops::TakeScreenshotTool::new()),
        Arc::new(system_ops::ScreenshotAnalyzeTool::new()),
        // 扩展系统工具
        Arc::new(extended_system_ops::OpenUrlTool::new()),
        Arc::new(extended_system_ops::GetActiveWindowTool::new()),
        // 系统内存查询（内存占用概况 + Top 进程明细，只读按需采集）
        Arc::new(extended_system_ops::GetMemoryUsageTool::new()),
        // 供应商预设更新（联网核对官方 API 文档后的结构化落点：整行 upsert +
        // 系统时钟写核对日期 + 自动递增插件版本防播种覆盖）
        Arc::new(provider_preset_tools::UpdateProviderPresetTool::new()),
        Arc::new(provider_preset_tools::ListProviderPresetsTool::new()),
        Arc::new(provider_preset_tools::ManageProviderPresetTool::new()),
        // 记忆工具
        Arc::new(memory_tools::SaveMemoryTool::new()),
        Arc::new(memory_tools::SearchMemoryTool::new()),
        Arc::new(memory_tools::MemoryMdTool::new()),
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
        Arc::new(todo_tools::UpdateTodoTool::new()),
        // 工作智能体独立待办工具（与陪伴待办分离，仅工作侧使用；整表替换语义）
        Arc::new(work_todo_tools::WorkTodoWriteTool::new()),
        // 工作智能体询问工具（选择题，挂起 loop 等用户抉择；与陪伴侧自由文本 ask_user 分离）
        Arc::new(work_question_tools::WorkAskUserTool::new()),
        // 工作智能体委派工具（独立上下文的子 agent，只回传最终文本）
        Arc::new(work_subagent_tools::WorkDelegateTool::new()),
        // 后台子任务的收集 / 取消 / 列表
        Arc::new(work_subagent_tools::WorkJobTool::new()),
        // 工作智能体隔离执行工具（工作树生命周期：列出 / 新建 / 移除）
        Arc::new(work_worktree_tools::WorkIsolateTool::new()),
        // 定时任务工具
        Arc::new(scheduler_tools::ScheduleReminderTool::new()),
        Arc::new(scheduler_tools::ManageScheduledTool::new()),
        // 在场状态工具（LLM 自主上下线）
        Arc::new(presence_tools::SetPresenceStateTool::new()),
        // 自主唤醒工具（LLM 给自己安排稍后回来的日程）
        Arc::new(wakeup_tool::ScheduleWakeupTool::new()),
        // 用户提问工具（模型主动向用户提问并等待回答）
        Arc::new(question_tools::AskUserTool::new()),
        // 计划模式工具（plan_task：产出计划等待用户批准）
        Arc::new(plan_tools::PlanTaskTool::new()),
        // 陪伴侧内部续思考信号：无外部副作用，驱动 ReAct 再运行一轮。
        Arc::new(thinking_tools::ContinueThinkingTool::new()),
        // 工作智能体桥（陪伴侧以用户身份派发工作任务 + 查询进度）
        Arc::new(work_agent_tools::DelegateToWorkAgentTool::new()),
        Arc::new(work_agent_tools::GetWorkStatusTool::new()),
        // 阶段成果播报（工作智能体 → 陪伴角色 → 以人设口吻向用户说话）
        Arc::new(work_agent_tools::NotifyCompanionTool::new()),
        // 子代理报告（子任务循环内向父级回传结果）
        Arc::new(subagent_tools::SubagentReportTool::new()),
        // 子代理委派与控制（spawn_subagent / subagent_control）
        Arc::new(subagent_tools::SpawnSubagentTool::new()),
        Arc::new(subagent_tools::SubagentControlTool::new()),
        // 工作流编排（多步工具脚本 + 并行扇出）
        Arc::new(workflow_tools::RunWorkflowTool::new()),
        // 用户研究工具（LLM 主动观察用户行为习惯）
        Arc::new(research_tool::ObserveUserTool::new()),
        // 跨角色对话工具
        Arc::new(cross_character_tools::TalkToCharacterTool::new()),
        // 媒体控制工具（合并播放/暂停/上下首/音量/静音为单工具）
        Arc::new(media_tools::MediaControlTool::new()),
        // 音乐（读当前播放 / 按名字找歌并播放；不单独暴露「搜索」工具，
        // 检索内嵌在 music_play 里，避免同一件事拆成两个工具）
        Arc::new(music_tools::MusicNowPlayingTool),
        Arc::new(music_tools::MusicPlayTool),
        // 桌面感知工具
        Arc::new(perception_tools::GetForegroundAppContextTool::new()),
        // 输入控制工具
        Arc::new(input_control_tools::ClickMouseTool::new()),
        Arc::new(input_control_tools::HotkeyTool::new()),
        Arc::new(input_control_tools::TypeTextTool::new()),
        // 后台任务工具（run_job / manage_job：后台命令执行与轮询）
        Arc::new(jobs_tools::RunJobTool::new()),
        Arc::new(jobs_tools::ManageJobTool::new()),
        // LSP 语义查询工具（定义/引用/实现/hover）
        Arc::new(lsp_tools::LspQueryTool::new()),
        // Wallpaper Engine 工具（list/set 保留，控制类合并为 wallpaper_control）
        Arc::new(wallpaper_tools::WallpaperListTool::new()),
        Arc::new(wallpaper_tools::WallpaperSetTool::new()),
        Arc::new(wallpaper_tools::WallpaperControlTool::new()),
        // 网络搜索（LLM 原生搜索的补充，支持 DuckDuckGo/SearXNG/Tavily）
        Arc::new(web_search_tool::WebSearchTool::new()),
        // 网页抓取（web_fetch：从具体 URL 提取正文）
        Arc::new(web_fetch_tool::WebFetchTool::new()),
        // 链接分享（搜索后分享有价值的链接卡片）
        Arc::new(share_link_tool::ShareLinkTool::new()),
        // 图片发送（智能体把本地图片发到微信面板 / 编程页聊天流）
        Arc::new(send_image_tool::SendImageTool::new()),
        // 可视化组件（智能体把 SVG 流程图/图表渲染成编程页卡片）
        Arc::new(show_widget_tool::ShowWidgetTool::new()),
        // 技能激活（按名称获取目录化技能正文，按需加载不常驻上下文）
        Arc::new(skill_tools::UseSkillTool::new()),
        // 技能创建（智能体自主沉淀可复用技能，写入即注册，自进化闭环）
        Arc::new(skill_tools::CreateSkillTool::new()),
        // 技能召回（按自然语言 BM25 检索名称/描述/关键词，返回候选供 use_skill 加载正文）
        Arc::new(skill_tools::SearchSkillTool::new()),
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
        // 编程智能体工具集（读/写/编辑/搜索/列目录/执行命令，构成 coding agent 闭环）
        Arc::new(coding_tools::WriteFileTool::new()),
        Arc::new(coding_tools::EditFileTool::new()),
        Arc::new(coding_tools::RunCommandTool::new()),
        Arc::new(coding_tools::GrepSearchTool::new()),
        Arc::new(coding_tools::ListDirTool::new()),
        // 天气预报（Open-Meteo 免费 API，支持 1~16 天预报）
        Arc::new(weather_tools::GetWeatherForecastTool::new()),
        // 内容发现闭环（B 站推荐 / 反馈 / 兴趣探针）
        Arc::new(discovery_tools::RecommendContentTool::new()),
        Arc::new(discovery_tools::SubmitContentFeedbackTool::new()),
        Arc::new(discovery_tools::GetInterestProbesTool::new()),
        Arc::new(discovery_tools::AnswerInterestProbeTool::new()),
    ];

    // 浏览器自动化工具（经精简 Chrome 扩展控制用户真实浏览器，保留登录态）
    tools.extend(crate::browser_bridge::tools::all_browser_tools());

    let count = tools.len();
    for tool in &tools {
        tool_system.register_tool(Arc::clone(tool));
    }

    // 历史名 → 真实名别名。
    //
    // 工具改名后旧名仍可能出现在 few-shot 示例、用户配置、历史对话与模型记忆中，
    // 别名表把旧名解析到新名，避免查不到工具。`normalize_tool_name` 只去分隔符、
    // 不重排词序（`setwallpaper` ≠ `wallpaperset`），无法覆盖该场景。
    //
    // 目标未注册的条目被 `register_alias` 忽略并 warn，表内可保留暂未注册的名字而不报错。
    let alias_pairs: &[(&str, &str)] = &[
        ("set_wallpaper", "wallpaper_set"),
        ("set_wallpaper_from_file", "wallpaper_set"),
        ("list_directory", "list_dir"),
        ("search_files", "grep_search"),
        ("grep", "grep_search"),
        ("cancel_scheduled", "manage_scheduled"),
        ("delete_todo", "manage_todo"),
        ("take_screenshot", "screenshot_analyze"),
    ];
    let alias_count = tool_system.register_aliases(alias_pairs);
    tracing::info!(
        "已注册 {} 条工具别名（共 {} 条候选，目标未注册的条目已忽略）",
        alias_count,
        alias_pairs.len()
    );

    // 确认名单自检：名单里的名字必须对应真实注册的工具，否则强制确认会静默失效
    // （不报错、无测试覆盖，表现为该问的没问）。
    let unresolved = crate::tools::permission::unresolved_confirmation_names(|name| {
        tool_system.has_tool(name)
    });
    if !unresolved.is_empty() {
        tracing::error!(
            "⚠️ 确认名单里以下名字没有对应已注册工具，这些工具的强制确认**不会生效**：{:?}。\
             请检查 permission.rs 的 CONFIRM_AT_ACTION_TOOLS / DISPATCHER_SAFE_ACTIONS \
             是否与 Tool::name() 的真实返回值一致。",
            unresolved
        );
    }

    // 注册 ToolSearchTool（延迟工具搜索元工具）
    // 快照作为兜底；同时持有 ToolSystem 弱引用——自建工具（create_tool）在
    // 运行时注册，搜索时优先查活注册表才能找到它们
    let snapshot = Arc::new(tools.clone());
    tool_system.register_tool(Arc::new(
        crate::tools::tool_call_manager::ToolSearchTool::new(snapshot, Arc::downgrade(tool_system)),
    ));

    tracing::info!("已注册 {} 个内置工具（含 tool_search 元工具）", count + 1);
}
