//! Tool discovery and catalog meta-tools, separate from execution orchestration.
use super::discovery::{DiscoverableTool, ToolSearchIndex};
use super::registry::{AgentSide, ToolSystem};
use super::tool_call_manager::resolve_visibility;
use super::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolScene, ToolUseContext, ValidationResult,
};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// ============================================================================
// ToolSearchTool - 延迟工具搜索元工具
// ============================================================================

/// 工具搜索元工具 - 让 LLM 按关键词或精确名称加载延迟工具的完整 schema
///
/// 行为：
/// - `select:A,B,C` — 按工具名精确加载（逗号分隔）
/// - 关键词搜索 — 匹配工具名 / search_hint / description，返回 top N
///
/// 返回结构是 `<functions>` 块，每行一个工具的完整 JSON schema，
/// LLM 拿到后即可像初始 prompt 中的工具一样调用。
///
/// 实现说明：持有 `Weak<ToolSystem>` 优先从活注册表搜索（自建工具在运行时注册，
/// 启动快照看不到），注册表已释放时回退到构造时快照；Weak 引用避免 Arc 循环。
pub struct ToolSearchTool {
    tools_snapshot: Arc<Vec<Arc<dyn Tool>>>,
    /// 工具系统弱引用：优先从活注册表搜索（自建工具运行时注册，快照看不到），
    /// Weak 避免 ToolSystem → tools map → ToolSearchTool 的 Arc 循环
    system: std::sync::Weak<ToolSystem>,
}

impl ToolSearchTool {
    pub fn new(
        tools_snapshot: Arc<Vec<Arc<dyn Tool>>>,
        system: std::sync::Weak<ToolSystem>,
    ) -> Self {
        Self {
            tools_snapshot,
            system,
        }
    }

    /// 搜索域：活注册表优先（含运行时注册的自建工具），回退启动快照
    fn live_tools(&self, side: AgentSide) -> Vec<Arc<dyn Tool>> {
        match self.system.upgrade() {
            Some(ts) => ts.list_tools_for_side(side),
            None => self
                .tools_snapshot
                .iter()
                .filter(|t| {
                    side == AgentSide::Work || !super::registry::is_work_agent_only(t.name())
                })
                .cloned()
                .collect(),
        }
    }

    /// 判断是否需要启用 ToolSearch（即是否存在 Deferred 或 Lazy 的工具）
    ///
    /// 注：此方法用 `ToolScene::Default` 作为基线判定，仅检查"是否存在可延迟工具"。
    /// 实际场景下的可见性由 `get_tools_for_ai_with_scene` 内的 `resolve_visibility(t, scene)` 动态判定。
    pub fn has_deferred_tools(tools: &[Arc<dyn Tool>]) -> bool {
        tools.iter().any(|t| {
            matches!(
                resolve_visibility(t, ToolScene::Default),
                crate::tools::types::ToolVisibility::Deferred
                    | crate::tools::types::ToolVisibility::Lazy
            )
        })
    }

    /// 生成 `<available-deferred-tools>` 块（仅工具名列表，每行一个）
    ///
    /// 注：此方法用 `ToolScene::Default` 作为基线。实际 prompt 中的 deferred 块
    /// 由 `get_tools_for_ai_with_scene` 按当前场景动态生成。
    pub fn format_deferred_tools_block(tools: &[Arc<dyn Tool>]) -> String {
        let deferred: Vec<String> = tools
            .iter()
            .filter(|t| {
                matches!(
                    resolve_visibility(t, ToolScene::Default),
                    crate::tools::types::ToolVisibility::Deferred
                        | crate::tools::types::ToolVisibility::Lazy
                )
            })
            .map(|t| t.name().to_string())
            .collect();
        if deferred.is_empty() {
            return String::new();
        }
        format!(
            "<available-deferred-tools>\n{}\n</available-deferred-tools>",
            deferred.join("\n")
        )
    }

    /// 执行搜索：返回匹配工具的完整 schema 字符串（`<functions>` 块）
    ///
    /// Search the caller's enabled tools, including deferred tools in every scene.
    /// Work-only tools remain excluded on the companion side. Exact selection and
    /// keyword search share the same limits and live per-side switches.
    pub fn search(
        &self,
        query: &str,
        max_results: usize,
        include_work_only: bool,
    ) -> (String, Vec<String>) {
        self.search_allowed(query, max_results, include_work_only, None)
    }

    fn search_allowed(&self, query: &str, max_results: usize, include_work_only: bool,
        allowed: Option<&[String]>) -> (String, Vec<String>) {
        let query = query.trim();
        let max_results = max_results.min(20);
        if max_results == 0 {
            return ("No matching deferred tools found".into(), Vec::new());
        }
        let mut live = self.live_tools(if include_work_only {
            AgentSide::Work
        } else {
            AgentSide::Companion
        });
        if let Some(allowed) = allowed { live.retain(|tool| allowed.iter().any(|name| name == tool.name())); }
        let all_tools = &live;

        // 1. select: 前缀精确加载（在全量工具中查找）
        if let Some(rest) = query
            .split_once(':')
            .filter(|(prefix, _)| prefix.eq_ignore_ascii_case("select"))
            .map(|(_, rest)| rest)
        {
            let requested: Vec<String> = rest
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let exact: HashMap<&str, Arc<dyn Tool>> = all_tools
                .iter()
                .map(|t| (t.name(), Arc::clone(t)))
                .collect();
            let catalog = super::catalog::ToolCatalog::from_values(
                all_tools
                    .iter()
                    .map(|tool| (tool.name().to_string(), Arc::clone(tool))),
            );
            let mut found: Vec<String> = Vec::new();
            let mut seen = HashSet::new();
            for name in &requested {
                // Use the registry resolver so aliases and ambiguity behave as at execution.
                let resolved = self
                    .system
                    .upgrade()
                    .and_then(|system| system.find_tool(name))
                    .or_else(|| catalog.find(name).cloned());
                if let Some(tool) = resolved.filter(|tool| exact.contains_key(tool.name())) {
                    if seen.insert(tool.name().to_string()) {
                        found.push(tool.name().to_string());
                    }
                    if found.len() == max_results {
                        break;
                    }
                }
            }
            return (self.render_functions_block(&found, all_tools), found);
        }

        // 2. 关键词 / 自然语言搜索：BM25 多字段加权 + jieba 中英分词（tools::discovery 索引），
        //    在全量工具中检索，按相关性降序取 top max_results。索引每次实时构建，含运行时注册的自建工具。
        let corpus = crate::tools::usage_corpus::ToolUsageCorpus::shared(
            crate::utils::path::get_user_data_dir().join("memory/tool_usage_corpus.json"),
        );
        let descriptors: Vec<DiscoverableTool> = all_tools
            .iter()
            .map(|tool| {
                let mut descriptor = DiscoverableTool::from_tool(tool.as_ref());
                let learned = corpus.search_hints(tool.as_ref());
                if !learned.is_empty() {
                    descriptor.search_hint.push_str(&format!("\n{learned}"));
                }
                descriptor
            })
            .collect();
        let index = ToolSearchIndex::build(descriptors);
        let matches: Vec<String> = index
            .search(query, max_results)
            .into_iter()
            .map(|(d, _score)| d.name.clone())
            .collect();
        (self.render_functions_block(&matches, all_tools), matches)
    }

    /// 把匹配的工具渲染为 `<functions>` 块（每行一个工具的完整 schema）
    fn render_functions_block(&self, names: &[String], all_tools: &[Arc<dyn Tool>]) -> String {
        if names.is_empty() {
            return "No matching deferred tools found".to_string();
        }
        let by_name: HashMap<&str, Arc<dyn Tool>> = all_tools
            .iter()
            .map(|t| (t.name(), Arc::clone(t)))
            .collect();
        let mut lines: Vec<String> = vec!["<functions>".to_string()];
        for name in names {
            if let Some(tool) = by_name.get(name.as_str()) {
                let schema = serde_json::json!({
                    "description": tool.description(),
                    "name": tool.name(),
                    "parameters": tool.parameters_schema(),
                });
                lines.push(format!("<function>{}</function>", schema));
            }
        }
        lines.push("</functions>".to_string());
        lines.join("\n")
    }
}

/// 可用工具清单（`tool_list`）。
///
/// 与 `tool_search` 的分工：`tool_search` 是「我知道名字或关键词，去拿完整 schema」；
/// `tool_list` 是「我根本不知道有什么，先看一眼目录」。开放式任务开头最该做的就是
/// 后者——先确认自己的能力面，再决定打法，而不是凭印象挑一个工具硬上。
///
/// 范围由 [`ToolUseContext::allowed_tools`] 决定：只列**本轮真的注入了 schema** 的工具。
/// 列出一个调不通的名字比不列更糟：模型会照着调，然后吃一个「未提供」的错误。
pub struct AvailableToolsTool {
    tools_snapshot: Arc<Vec<Arc<dyn Tool>>>,
    /// 同 ToolSearchTool：优先读活注册表，Weak 避免 Arc 循环
    system: std::sync::Weak<ToolSystem>,
}

impl AvailableToolsTool {
    pub fn new(
        tools_snapshot: Arc<Vec<Arc<dyn Tool>>>,
        system: std::sync::Weak<ToolSystem>,
    ) -> Self {
        Self {
            tools_snapshot,
            system,
        }
    }

    fn live_tools(&self, side: AgentSide) -> Vec<Arc<dyn Tool>> {
        match self.system.upgrade() {
            Some(ts) => ts.list_tools_for_side(side),
            None => self
                .tools_snapshot
                .iter()
                .filter(|t| {
                    side == AgentSide::Work || !super::registry::is_work_agent_only(t.name())
                })
                .cloned()
                .collect(),
        }
    }
}

#[async_trait]
impl Tool for AvailableToolsTool {
    fn name(&self) -> &str {
        "tool_list"
    }

    fn description(&self) -> &str {
        "List what you can actually do this turn: every tool currently exposed to you, one line each (name + one-line purpose).\n\
         Use it at the start of open-ended work to see your capability surface before committing to an approach, \
         and before concluding \"I can't do that\" — the capability may exist under a name you didn't think of.\n\
         Returns names and purposes only. For a tool's full parameter schema, call tool_search with \"select:<name>\"."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "列出**本轮真实可用**的全部工具（每行：名称 + 一句话用途）。\n\
                     开放式任务开头先用它看一遍自己的能力面再定打法；得出「这个我做不到」的结论之前也先看一眼——\
                     能力可能就存在，只是名字跟你想的不一样。\n\
                     这里只给名称和用途；要某个工具的完整参数 schema，用 tool_search 传 \"select:<名字>\"。",
            "ja" => "このターンで実際に使えるツールを一覧表示する（1 行 1 件：名前 + 一行の用途）。\n\
                     自由形式の作業を始める前に、まず自分の能力面を確認してから方針を決める。\
                     「これはできない」と結論づける前にも確認すること——名前が違うだけで存在する場合がある。\n\
                     ここでは名前と用途のみ。完全なパラメータ schema は tool_search に \"select:<名前>\" を渡して取得する。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "可选：按名称/用途关键词过滤（不区分大小写的子串匹配）" },
                "category": { "type": "string", "description": "可选：按类别过滤（file / system / memory / network / mcp 等）" },
                "offset": { "type": "integer", "description": "分页起点，默认 0", "minimum": 0 },
                "limit": { "type": "integer", "description": "本页最多返回多少条，默认 60，上限 200", "minimum": 1 }
            },
            "required": []
        })
    }

    fn parameters_schema_in(&self, _lang: &str) -> serde_json::Value {
        self.parameters_schema()
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase());
        let category = args
            .get("category")
            .and_then(|v| v.as_str())
            .map(|s| s.to_lowercase());
        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(60)
            .clamp(1, 200) as usize;

        // allowed_tools 存在时以它为准——那是本轮 schema 的真实内容，
        // 不掺任何"注册表里有就算"的推断。
        let allowed: Option<std::collections::HashSet<&str>> = ctx
            .allowed_tools
            .as_ref()
            .map(|list| list.iter().map(|s| s.as_str()).collect());

        let mut rows: Vec<(String, String, String)> = self
            .live_tools(AgentSide::from_agent_kind(&ctx.agent_kind))
            .into_iter()
            .filter(|t| allowed.as_ref().map_or(true, |set| set.contains(t.name())))
            .filter_map(|t| {
                let name = t.name().to_string();
                let desc = t
                    .description()
                    .lines()
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let cat = format!("{:?}", t.category()).to_lowercase();
                if let Some(q) = &query {
                    if !name.to_lowercase().contains(q) && !desc.to_lowercase().contains(q) {
                        return None;
                    }
                }
                if let Some(c) = &category {
                    if &cat != c {
                        return None;
                    }
                }
                Some((name, desc, cat))
            })
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0));

        let total = rows.len();
        if total == 0 {
            let payload =
                serde_json::json!({"total": 0, "offset": offset, "returned": 0, "tools": []});
            return ToolResult::standard_success(
                "没有匹配的工具（可去掉 query/category 再试）。",
                Some(payload),
            );
        }

        let page: Vec<_> = rows.iter().skip(offset).take(limit).collect();
        let mut lines = vec![format!(
            "本轮可用工具：共 {total} 个，本页 {} 个（offset={offset}）。",
            page.len()
        )];
        for (name, desc, cat) in &page {
            let d: String = desc.chars().take(120).collect();
            lines.push(format!("- {name} [{cat}] — {d}"));
        }
        if offset + page.len() < total {
            lines.push(format!(
                "还有 {} 个未显示：用 offset={} 继续，或用 query/category 过滤。",
                total - offset - page.len(),
                offset + page.len()
            ));
        }
        lines.push("要某个工具的完整参数 schema：tool_search 传 \"select:<名字>\"。".to_string());

        let payload = serde_json::json!({
            "total": total,
            "offset": offset,
            "returned": page.len(),
            "tools": page.iter().map(|(n, d, c)| serde_json::json!({
                "name": n, "purpose": d, "category": c
            })).collect::<Vec<_>>(),
        });
        ToolResult::standard_success(&lines.join("\n"), Some(payload))
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> crate::tools::types::ToolRiskTier {
        crate::tools::types::ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "tool list available capability 工具清单 能用什么 有哪些能力"
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "tool_search"
    }

    fn description(&self) -> &str {
        "Search and load the full schema of deferred tools.\
         When you need to call a tool that appears in the <available-deferred-tools> block but has no full definition at the top of the prompt,\
         call this tool first to get its complete parameter schema so you can invoke it correctly.\n\
         Query forms:\n\
         - \"select:wallpaper_list,wallpaper_set\" — Load tools by exact name (comma-separated, recommended when you know the name)\n\
         - \"wallpaper\" — Keyword search, returns the top N most relevant results\n\
         The return structure is a <functions> block where each <function> contains description / name / parameters fields.\
         Once retrieved, you can call it like any tool defined in the initial prompt."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "搜索并加载延迟工具的完整 schema。当需要调用 <available-deferred-tools> 中列出但没有完整定义的工具时，\
                     先调用此工具获取其完整参数 schema，然后才能正确调用。\n\
                     查询形式：\n\
                     - \"select:wallpaper_list,wallpaper_set\" — 按名称精确加载（逗号分隔，推荐在已知名称时使用）\n\
                     - \"wallpaper\" — 关键词搜索，返回最相关的前 N 个结果\n\
                     返回结构为 <functions> 块，每个 <function> 包含 description / name / parameters 字段。\
                     获取后即可像初始提示中定义的工具一样调用。",
            "ja" => "遅延ツールの完全スキーマを検索して読み込む。<available-deferred-tools> に表示されているが完全な定義がないツールを呼び出す必要がある場合、\
                     まずこのツールで完全なパラメータスキーマを取得してから正しく呼び出してください。\n\
                     クエリ形式：\n\
                     - \"select:wallpaper_list,wallpaper_set\" — 名前で正確に読み込む（カンマ区切り、名前が分かっている場合に推奨）\n\
                     - \"wallpaper\" — キーワード検索、最も関連性の高い上位 N 件の結果を返す\n\
                     戻り値は <functions> ブロックで、各 <function> に description / name / parameters フィールドが含まれる。\
                     取得後は初期プロンプトで定義されたツールと同様に呼び出せる。",
            _ => self.description(),
        }
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Query string. \"select:A,B,C\" loads tools by exact name; otherwise pass keywords or a natural-language phrase (auto-tokenized, Chinese and English both work), ranked by BM25 over tool name/search_hint/description."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum tools returned by keyword search or select, default 5, at most 20",
                    "minimum": 1,
                    "maximum": 20
                }
            },
            "required": ["query"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> serde_json::Value {
        match lang {
            "zh" => serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "查询字符串。\"select:A,B,C\" 按名称精确加载工具；否则传关键词或自然语言短语（自动分词，中英皆可），按 BM25 在工具名/搜索提示/描述上排序。"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "关键词或 select 加载的最大工具数，默认 5，上限 20",
                    "minimum": 1,
                    "maximum": 20
                    }
                },
                "required": ["query"]
            }),
            "ja" => serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "クエリ文字列。\"select:A,B,C\" は名前で正確にツールを読み込む；それ以外はキーワードまたは自然言語の句（自動分かち書き、中英対応）を BM25 でツール名/検索ヒント/説明に対してランキングする。"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "キーワード検索または select の最大件数。既定 5、上限 20",
                        "minimum": 1
                    }
                },
                "required": ["query"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
        if query.trim().is_empty() {
            return ValidationResult::failure("query 不能为空", 2);
        }
        ValidationResult::success(Some(input.clone()))
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let max_results = args
            .get("max_results")
            .and_then(|v| v.as_u64())
            .unwrap_or(5)
            .clamp(1, 20) as usize;
        // 工作智能体专属工具只在 agent_kind == "work" 的检索域中出现
        let include_work_only = ctx.agent_kind == "work";
        let (block, matches) = self.search_allowed(query, max_results, include_work_only, ctx.allowed_tools.as_deref());
        ToolResult::standard_success(
            &format!("ToolSearch 返回 {} 个工具", matches.len()),
            Some(serde_json::json!({
                "matches": matches,
                "query": query,
                "max_results": max_results,
                "functions_block": block,
            })),
        )
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    // ToolSearch 自己永不延迟（否则鸡生蛋）
    fn always_load(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Named(&'static str);
    #[async_trait]
    impl Tool for Named {
        fn name(&self) -> &str {
            self.0
        }
        fn description(&self) -> &str {
            "catalog test tool"
        }
        fn parameters_schema(&self) -> Value {
            json!({"type":"object"})
        }
        async fn validate_input(&self, _: &Value, _: &ToolUseContext) -> ValidationResult {
            ValidationResult::success(None)
        }
        async fn check_permissions(&self, _: &Value, _: &ToolUseContext) -> PermissionResult {
            PermissionResult::allow()
        }
        async fn call(&self, _: Value, _: &ToolUseContext) -> ToolResult {
            ToolResult::success(Value::Null)
        }
        fn is_read_only(&self) -> bool {
            true
        }
        fn category(&self) -> ToolCategory {
            ToolCategory::System
        }
    }

    fn setup() -> (Arc<ToolSystem>, ToolSearchTool) {
        let system = Arc::new(ToolSystem::new());
        for name in ["alpha", "beta", "create_tool", "a_b", "ab"] {
            system.register_tool(Arc::new(Named(name)));
        }
        let search = ToolSearchTool::new(Arc::new(system.list_tools()), Arc::downgrade(&system));
        (system, search)
    }

    #[test]
    fn exact_loading_uses_aliases_and_limits_unique_visible_tools() {
        let (system, search) = setup();
        system.register_alias("old_alpha", "alpha");
        let (_, names) = search.search(" SeLeCt:old_alpha,alpha,beta,create_tool ", 1, false);
        assert_eq!(names, ["alpha"]);
        let (_, names) = search.search("select:alpha,old_alpha,beta,create_tool", 20, false);
        assert_eq!(names, ["alpha", "beta"]);
        assert!(search.search("select:A-B", 20, false).1.is_empty());
        assert!(search.search("select:alpha", 0, false).1.is_empty());
    }

    #[test]
    fn discovery_respects_live_per_side_switches_and_runtime_registration() {
        let (system, search) = setup();
        system.set_disabled_tools(vec!["alpha".into()], vec!["beta".into()]);
        assert_eq!(
            search.search("select:alpha,beta,create_tool", 20, false).1,
            ["beta"]
        );
        assert_eq!(
            search.search("select:alpha,beta,create_tool", 20, true).1,
            ["alpha", "create_tool"]
        );
        system.register_tool(Arc::new(Named("late")));
        assert_eq!(search.search("select:late", 20, false).1, ["late"]);
        system.unregister_tool("late");
        assert!(search.search("select:late", 20, false).1.is_empty());
    }

    #[test]
    fn discovery_does_not_offer_tools_outside_the_callers_allowlist() {
        let (_system, search) = setup();
        let allowed = vec!["alpha".into()];
        let (_, found) = search.search_allowed("select:alpha,beta", 10, true, Some(&allowed));
        assert_eq!(found, ["alpha"]);
    }

    #[tokio::test]
    async fn tool_list_intersects_actual_exposure_and_live_side_switches() {
        let (system, _) = setup();
        system.set_disabled_tools(vec!["alpha".into()], Vec::new());
        let list = AvailableToolsTool::new(Arc::new(system.list_tools()), Arc::downgrade(&system));
        let ctx = ToolUseContext {
            allowed_tools: Some(vec!["alpha".into(), "beta".into(), "create_tool".into()]),
            ..Default::default()
        };
        let result = list.call(json!({}), &ctx).await;
        let rows = &result.data.unwrap()["data"]["tools"];
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["name"], "beta");
    }
}
