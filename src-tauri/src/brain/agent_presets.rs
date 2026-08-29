//! 智能体预设 —— 按会话组装工具集与提示词段。
//!
//! 一个预设声明：工具白名单（该形态下模型可见的工具）+ 引导提示词段。
//! 工作智能体的 standard/code/minimal 模式与陪伴/研究形态共用此注册表，
//! 让「按会话组装智能体」有统一出处。

/// 单个预设。
pub struct AgentPreset {
    pub name: &'static str,
    /// 工具白名单（空 = 不限制，全量工具）
    pub tools: &'static [&'static str],
    /// 引导提示词段（拼入 system prompt）
    pub prompt_section: &'static str,
}

/// 全部预设。
pub const PRESETS: &[AgentPreset] = &[
    AgentPreset {
        name: "standard",
        tools: &[
            "read_file", "write_file", "edit_file", "run_command",
            "grep_search", "list_dir", "run_job", "manage_job",
            "plan_task", "lsp_query", "ask_user", "web_fetch", "web_search",
        ],
        prompt_section: "你在标准工作模式下：先看（list_dir/grep/read）再动手，局部修改用 edit_file，改后跑命令验证；耗时操作放后台（run_job），复杂任务先出计划（plan_task）。",
    },
    AgentPreset {
        name: "code",
        tools: &[
            "read_file", "write_file", "edit_file", "run_command",
            "grep_search", "list_dir", "run_job",
        ],
        prompt_section: "你在编排模式下：把整个任务一次性规划为多步程序（JSON 步骤序列），由宿主顺序执行；步骤间不要依赖上一步的动态输出值。",
    },
    AgentPreset {
        name: "minimal",
        tools: &["run_command", "edit_file"],
        prompt_section: "你在极简模式下：只有 run_command 与 edit_file 两个工具；读取用 Get-Content，搜索用 Select-String。",
    },
    AgentPreset {
        name: "companion",
        tools: &[],
        prompt_section: "你在陪伴形态下：以角色人格与用户自然交流为主，但你自身可直接使用工作能力——\
检索资料（web_search / web_fetch，用户问「帮我查查/看看这个链接」时直接用）、\
后台执行耗时命令（run_job 启动，manage_job 轮询进度并向用户汇报，如构建/安装/下载）、\
管理待办（add_todo / update_todo）、\
有风险或多步骤的事先出计划征得同意（plan_task）、\
多步任务一步编排（run_workflow）、\
可并行的小任务用 spawn_subagent 委派子代理后台执行，用 subagent_control 查询进度/取消/取报告、\
需要用户澄清或决策时主动提问（ask_user）、\
对用户做出「稍后再来」类承诺时用 schedule_wakeup 给自己定时（如「20分钟后再来看看你」「明早叫你」）。\
当用户提出**复杂任务**——大型编程/多文件工程、多步骤执行、需要工具创建等能力进化事件（create_tool / create_skill）——\
用 delegate_to_work_agent 派发给工作智能体（独立会话后台执行，无需工作区也可工作），\
之后用 get_work_status 查进度并向用户汇报——小事自己动手，大活和进化事件才派出去。\
你派出去的后台任务状态与完成报告会出现在提示词的「后台任务」段落：刚完成但未汇报的，主动用自己的口吻告诉用户结果，不必等用户来问。",
    },
    AgentPreset {
        name: "research",
        tools: &[
            "web_search", "web_fetch", "read_file", "list_dir", "grep_search",
            "save_memory", "search_memories", "create_notebook",
        ],
        prompt_section: "你在研究形态下：围绕主题检索（web_search/web_fetch），把有价值的信息整理入库（save_memory）并沉淀为笔记（create_notebook）。",
    },
];

/// 按名解析预设。
pub fn resolve(name: &str) -> Option<&'static AgentPreset> {
    PRESETS.iter().find(|p| p.name == name)
}

/// 全部预设名。
pub fn list_names() -> Vec<&'static str> {
    PRESETS.iter().map(|p| p.name).collect()
}

/// 预设的工具白名单（找不到预设返回 None）。
pub fn tools_of(name: &str) -> Option<&'static [&'static str]> {
    resolve(name).map(|p| p.tools)
}

/// 预设的提示词段（找不到返回空串）。
pub fn prompt_section_of(name: &str) -> &'static str {
    resolve(name).map(|p| p.prompt_section).unwrap_or("")
}
