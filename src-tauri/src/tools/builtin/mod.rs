//! 内置工具注册 - 将所有内置工具注册到工具系统

use std::sync::Arc;

use super::registry::ToolSystem;

pub mod cross_character_tools;
pub mod coding_tools;
pub mod diary_tools;
pub mod discovery_tools;
pub mod bilibili_danmaku_tool;
pub mod extended_system_ops;
pub mod file_tools;
pub mod input_control_tools;
pub mod desktop_runtime;
pub mod computer_action;
pub mod jobs_tools;
pub mod lsp_tools;
pub mod media_tools;
pub mod memory_tools;
pub mod music_tools;
pub mod notebook_tools;
pub mod user_quick_notes;
pub mod perception_tools;
pub mod pet_tools;
pub mod pet_interaction_tools;
pub mod plan_tools;
/// 平台内检索：web_search 之外去 B站 / 番组 / 微博 / V2EX / Reddit / X 找可引用条目。
pub mod platform_search_policy;
pub mod platform_search_tool;
pub mod plugin_tools;
pub mod provider_preset_tools;
pub mod presence_tools;
pub mod question_tools;
pub mod research_tool;
/// 代码审查：review_diff（只读解析审查目标、变更文件与规约文件）。
pub mod review_tools;
pub mod scheduler_tools;
pub mod send_image_tool;
pub mod send_chat_message_tool;
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
pub(crate) mod web_platform_search;
pub(crate) mod web_platform_fetch;
pub(crate) mod web_tool_compat;
pub mod web_search_tool;
pub mod work_agent_tools;
pub mod work_todo_tools;
pub mod work_question_tools;
pub mod work_subagent_tools;
pub mod work_worktree_tools;
pub mod workflow_tools;
/// 会话元工具：目标（goal）与交付声明（present）。
pub mod agent_meta_tools;
/// 多智能体协作：队友管理、团队任务板、消息投递、目标迭代。
pub mod agent_control_tools;
/// 编辑与看图：edit_file（多动作编辑器，可撤销）、read_image（真正看见图）。
pub mod editor_tools;
/// 运行时环境：工作目录切换、依赖探测、运行时自省。
pub mod runtime_env_tools;
/// 会话历史检索（跨会话回忆）。
pub mod session_query_tools;
/// 持久终端：terminal_* 六件套 + pwsh_persistent。
pub mod terminal_tools;
/// 生图：文生图 + 参考图引导，结果落盘并回传。
pub mod image_gen_tool;
pub mod plugin_advisor_tools;

/// 注册所有内置工具到工具系统
pub fn register_builtin_tools(tool_system: &Arc<ToolSystem>) {
    let mut tools: Vec<Arc<dyn super::types::Tool>> = vec![
        // 系统工具
        Arc::new(system_ops::OpenApplicationTool::new()),
        Arc::new(system_ops::CloseApplicationTool::new()),
        Arc::new(system_ops::TakeScreenshotTool::new()),
        Arc::new(computer_action::ComputerActionTool),
        Arc::new(computer_action::ComputerStateTool),
        // 扩展系统工具（前台窗口/内存占用已并入 computer_state 的 action）
        Arc::new(extended_system_ops::OpenUrlTool::new()),
        // 供应商预设（盘点 + 统一变更入口：LLM/嵌入 upsert、按 id 删除；
        // 旧 update_provider_preset 的整行 upsert 语义并入 manage_provider_preset）
        Arc::new(provider_preset_tools::ListProviderPresetsTool::new()),
        Arc::new(provider_preset_tools::ManageProviderPresetTool::new()),
        // 记忆工具
        Arc::new(memory_tools::SaveMemoryTool::new()),
        Arc::new(memory_tools::SearchMemoryTool::new()),
        // 深查：模型一次给多组关键词/渠道，内部并行跨日记/笔记/摘要/事实/原文检索。
        Arc::new(memory_tools::DeepRecallTool::new()),
        Arc::new(user_quick_notes::ReadUserQuickNotes),
        Arc::new(memory_tools::MemoryMdTool::new()),
        Arc::new(pet_interaction_tools::GetPetInteractionsTool),
        // 时间回忆（最近互动/按日期/今日总结合并为一个 canonical 工具）
        Arc::new(memory_tools::RecallTimeTool::new()),
        // 智能日记工具（LLM 自主写日记，条件满足时可见）
        Arc::new(diary_tools::WriteDiaryTool::new()),
        // 宠物注视模式切换
        Arc::new(pet_tools::ToggleWatchModeTool::new()),
        // 待办工具（读用 list_todo，写统一走 manage_todo 的 action 分发）
        Arc::new(todo_tools::ListTodoTool::new()),
        Arc::new(todo_tools::ManageTodoTool::new()),
        // 工作智能体独立待办工具（与陪伴待办分离，仅工作侧使用；整表替换语义）
        Arc::new(work_todo_tools::WorkTodoWriteTool::new()),
        // 工作智能体询问工具（选择题，挂起 loop 等用户抉择；与陪伴侧自由文本 ask_user 分离）
        Arc::new(work_question_tools::WorkAskUserTool::new()),
        // 工作智能体委派工具（独立上下文的子 agent，只回传最终文本）
        Arc::new(work_subagent_tools::WorkDelegateTool::new()),
        // 后台子任务的收集 / 取消 / 列表
        Arc::new(work_subagent_tools::WorkJobTool::new()),
        // 工作侧会话元工具：目标（一句话终态，跨轮持久）与交付声明（登记已存在的产物）
        Arc::new(agent_meta_tools::GoalTool::new()),
        Arc::new(agent_meta_tools::PresentTool::new()),
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
        // 室友在场查询（只读；补上 prompt 侧"变化驱动注入"留下的按需缺口）
        Arc::new(cross_character_tools::GetRoommateStatusTool::new()),
        // 媒体控制（播放/暂停、音量/静音 + 读取当前播放 + 按名点歌，统一 action 分发）
        Arc::new(media_tools::MediaControlTool::new()),
        // 后台任务工具（run_job / manage_job：后台命令执行与轮询）
        Arc::new(jobs_tools::RunJobTool::new()),
        Arc::new(jobs_tools::ManageJobTool::new()),
        // LSP 语义查询工具（定义/引用/实现/hover）
        Arc::new(lsp_tools::LspQueryTool::new()),
        // Wallpaper Engine 工具（列出/设置/控制合并到 wallpaper_control）
        Arc::new(wallpaper_tools::WallpaperControlTool::new()),
        // 网络搜索（LLM 原生搜索的补充，支持 DuckDuckGo/SearXNG/Tavily）
        Arc::new(web_search_tool::WebSearchTool::new()),
        // 统一读取网页与平台正文、字幕、评论、弹幕。
        Arc::new(web_fetch_tool::WebFetchTool::new()),
        // 链接分享（搜索后分享有价值的链接卡片）
        Arc::new(share_link_tool::ShareLinkTool::new()),
        // 图片发送（智能体把本地图片发到微信面板 / 编程页聊天流）
        Arc::new(send_image_tool::SendImageTool::new()),
        Arc::new(send_chat_message_tool::SendChatMessageTool),
        // 可视化组件（智能体把 SVG 流程图/图表渲染成编程页卡片）
        Arc::new(show_widget_tool::ShowWidgetTool::new()),
        // 技能读侧统一入口（use 激活正文 / search 自然语言召回 / validate 形态校验；后两者旧名保留为别名）
        Arc::new(skill_tools::UseSkillTool::new()),
        Arc::new(plugin_advisor_tools::PluginAdvisorTool),
        // 技能创建（智能体自主沉淀可复用技能，写入即注册，自进化闭环）
        Arc::new(skill_tools::CreateSkillTool::new()),
        // 代码审查输入（只读解析审查目标：未提交/基准分支/提交，附变更文件与规约文件）
        Arc::new(review_tools::ReviewDiffTool::new()),
        // 笔记本工具（create_notebook 保留长写作工作流；其余查看/修改/分享/HTML 笔记合并为 notebook）
        Arc::new(notebook_tools::CreateNotebookTool::new()),
        Arc::new(notebook_tools::NotebookTool::new()),
        // 文件系统读取（按路径读本地文本/代码/HTML，受沙箱路径校验约束）
        Arc::new(file_tools::ReadFileTool::default()),
        Arc::new(file_tools::ReadSpilledResultTool),
        // 编程智能体工具集（读/写/编辑/搜索/列目录/执行命令，构成 coding agent 闭环）
        Arc::new(coding_tools::WriteFileTool::new()),
        Arc::new(coding_tools::EditFileTool::new()),
        Arc::new(coding_tools::RunCommandTool::new()),
        Arc::new(coding_tools::GrepSearchTool::new()),
        Arc::new(coding_tools::ListDirTool::new()),
        Arc::new(coding_tools::GlobTool::new()),
        // 看图（让模型真的看见图，而不是猜文件名）；编辑动作已并入 edit_file
        Arc::new(editor_tools::ReadImageTool::new()),
        // 生图（文生图 + 参考图引导；结果落盘并随工具结果回传）
        Arc::new(image_gen_tool::GenerateImageTool::new()),
        // 运行时环境：工作目录、依赖路径、能力自省
        Arc::new(runtime_env_tools::WorkingDirectoryTool::new()),
        Arc::new(runtime_env_tools::LoadWorkspaceDependenciesTool::new()),
        Arc::new(runtime_env_tools::RuntimeInspectListTool::new(Arc::downgrade(tool_system))),
        Arc::new(runtime_env_tools::RuntimeInspectQueryTool::new(Arc::downgrade(tool_system))),
        // 跨会话历史检索
        Arc::new(session_query_tools::SessionSearchTool::new()),
        Arc::new(session_query_tools::SessionTraceTool::new()),
        Arc::new(session_query_tools::SessionEventSearchTool::new()),
        Arc::new(session_query_tools::SessionEventReadTool::new()),
        Arc::new(session_query_tools::SessionEventTraceTool::new()),
        // 持久终端（跨调用保留状态）
        Arc::new(terminal_tools::TerminalOpenTool::new()),
        Arc::new(terminal_tools::TerminalSendTool::new()),
        Arc::new(terminal_tools::TerminalReadTool::new()),
        Arc::new(terminal_tools::TerminalSignalTool::new()),
        Arc::new(terminal_tools::TerminalListTool::new()),
        Arc::new(terminal_tools::TerminalCloseTool::new()),
        Arc::new(terminal_tools::PwshPersistentTool::new()),
        // 多智能体协作（team / team_task 两个统一入口；旧名保留为别名）
        Arc::new(agent_control_tools::TeamTool::new()),
        Arc::new(agent_control_tools::TeamTaskTool::new()),
        // 天气预报（Open-Meteo 免费 API，支持 1~16 天预报）
        Arc::new(weather_tools::GetWeatherForecastTool::new()),
        // 内容发现闭环（B 站推荐 / 反馈 / 兴趣探针）
        Arc::new(discovery_tools::RecommendContentTool::new()),
        Arc::new(discovery_tools::SubmitContentFeedbackTool::new()),
        Arc::new(discovery_tools::GetInterestProbesTool::new()),
        Arc::new(discovery_tools::AnswerInterestProbeTool::new()),
    ];

    // 内嵌浏览器工具（经 browser_bridge 调用进程内 WebView2，共享普通配置的登录态）
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
        ("screenshot_analyze", "take_screenshot"),
        ("set_wallpaper", "wallpaper_control"),
        ("set_wallpaper_from_file", "wallpaper_control"),
        ("wallpaper_list", "wallpaper_control"),
        ("wallpaper_set", "wallpaper_control"),
        ("music_now_playing", "media_control"),
        ("music_play", "media_control"),
        ("add_todo", "manage_todo"),
        ("complete_todo", "manage_todo"),
        ("update_todo", "manage_todo"),
        ("click_mouse", "computer_state"),
        ("hotkey", "computer_state"),
        ("type_text", "computer_state"),
        ("get_active_window", "computer_state"),
        ("get_foreground_app_context", "computer_state"),
        ("get_recent_interactions", "recall_time"),
        ("recall_by_date_time", "recall_time"),
        ("read_diary_by_date", "recall_time"),
        ("summarize_today_context", "recall_time"),
        ("list_agents", "team"),
        ("interrupt_agent", "team"),
        ("send_message", "team"),
        ("wait_agent", "team"),
        ("list_subagent_models", "team"),
        ("spawn_teammate", "team"),
        ("ralph", "team"),
        ("team_task_create", "team_task"),
        ("team_task_list", "team_task"),
        ("team_task_get", "team_task"),
        ("team_task_update", "team_task"),
        ("list_notebooks", "notebook"),
        ("get_notebook_detail", "notebook"),
        ("update_notebook", "notebook"),
        ("share_notebook", "notebook"),
        ("create_html_note", "notebook"),
        // 技能读侧合并：旧名按参数形状推断 use / search / validate
        ("search_skill", "use_skill"),
        ("validate_skill", "use_skill"),
        // 供应商预设：旧 update_provider_preset 的整行 upsert 已并入 manage_provider_preset
        ("update_provider_preset", "manage_provider_preset"),
        // 编辑器合并：str_replace_editor 的 view/create/insert/undo 已并入 edit_file
        ("str_replace_editor", "edit_file"),
        ("list_directory", "list_dir"),
        ("search_files", "grep_search"),
        ("grep", "grep_search"),
        ("cancel_scheduled", "manage_scheduled"),
    ];
    let alias_count = tool_system.register_aliases(alias_pairs);
    tracing::info!(
        "已注册 {} 条工具别名（共 {} 条候选，目标未注册的条目已忽略）",
        alias_count,
        alias_pairs.len()
    );

    // 注册 ToolSearchTool（延迟工具搜索元工具）
    // 快照作为兜底；同时持有 ToolSystem 弱引用——自建工具（create_tool）在
    // 运行时注册，搜索时优先查活注册表才能找到它们
    let snapshot = Arc::new(tools.clone());
    tool_system.register_tool(Arc::new(
        crate::tools::tool_call_manager::ToolSearchTool::new(snapshot, Arc::downgrade(tool_system)),
    ));
    // 可用工具清单（能力面目录）。与 tool_search 分工：这个回答"我有什么"，
    // 那个回答"某个工具怎么调"。二者共用同一份快照与弱引用。
    tool_system.register_tool(Arc::new(
        crate::tools::tool_call_manager::AvailableToolsTool::new(
            Arc::new(tools.clone()),
            Arc::downgrade(tool_system),
        ),
    ));

    tracing::info!("已注册 {} 个内置工具（含 tool_search / tool_list 元工具）", count + 2);
}

/// 在所有内置和运行时工具注册完成后检查确认名单。
pub fn validate_confirmation_tools(tool_system: &crate::tools::ToolSystem) {
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

}

#[cfg(test)]
mod merged_family_tests {
    use super::*;

    #[test]
    fn legacy_family_names_resolve_to_the_merged_tool() {
        let system = Arc::new(crate::tools::ToolSystem::new());
        register_builtin_tools(&system);
        let cases = [
            ("music_play", "media_control"),
            ("music_now_playing", "media_control"),
            ("wallpaper_list", "wallpaper_control"),
            ("wallpaper_set", "wallpaper_control"),
            ("set_wallpaper", "wallpaper_control"),
            ("add_todo", "manage_todo"),
            ("complete_todo", "manage_todo"),
            ("update_todo", "manage_todo"),
            ("click_mouse", "computer_state"),
            ("hotkey", "computer_state"),
            ("type_text", "computer_state"),
            ("get_active_window", "computer_state"),
            ("get_foreground_app_context", "computer_state"),
            ("get_recent_interactions", "recall_time"),
            ("recall_by_date_time", "recall_time"),
            ("read_diary_by_date", "recall_time"),
            ("summarize_today_context", "recall_time"),
            ("list_agents", "team"),
            ("interrupt_agent", "team"),
            ("send_message", "team"),
            ("wait_agent", "team"),
            ("list_subagent_models", "team"),
            ("spawn_teammate", "team"),
            ("ralph", "team"),
            ("team_task_create", "team_task"),
            ("team_task_list", "team_task"),
            ("team_task_get", "team_task"),
            ("team_task_update", "team_task"),
            ("list_notebooks", "notebook"),
            ("get_notebook_detail", "notebook"),
            ("update_notebook", "notebook"),
            ("share_notebook", "notebook"),
            ("create_html_note", "notebook"),
            ("search_skill", "use_skill"),
            ("validate_skill", "use_skill"),
            ("update_provider_preset", "manage_provider_preset"),
            ("str_replace_editor", "edit_file"),
        ];
        for (legacy, canonical) in cases {
            assert_eq!(
                system.find_tool(legacy).expect("旧名应能解析").name(),
                canonical,
                "alias {legacy}"
            );
        }
        // 合并后的家族只保留 canonical 一个模型可见工具。
        for canonical in [
            "media_control",
            "wallpaper_control",
            "manage_todo",
            "computer_state",
            "recall_time",
            "team",
            "team_task",
            "notebook",
            "use_skill",
            "manage_provider_preset",
            "edit_file",
        ] {
            assert!(system.find_tool(canonical).is_some(), "{canonical} 应已注册");
        }
    }
}
