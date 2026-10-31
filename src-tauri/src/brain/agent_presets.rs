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
            "plan_task", "lsp_query", "ask_user", "read_content", "web_search",
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
        prompt_section: include_str!("../../prompts/execution/companion-dispatch.en.md"),
    },
    AgentPreset {
        name: "research",
        tools: &[
            "web_search", "read_content", "read_file", "list_dir", "grep_search",
            "save_memory", "search_memories", "create_notebook",
        ],
        prompt_section: "你在研究形态下：围绕主题检索（web_search/read_content），把有价值的信息整理入库（save_memory）并沉淀为笔记（create_notebook）。",
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
