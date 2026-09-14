//! 可视化组件工具 - 工作智能体把 SVG 流程图/图表渲染成编程页聊天流里的卡片
//!
//! 调用场景：
//! - 解释架构 / 模块依赖 / 调用链时画流程图或结构图
//! - 梳理状态流转 / 时序交互时画状态图或时序图
//! - 对比方案、呈现数据关系时画简单图表
//!
//! 设计要点（镜像 WorkBuddy 可视化通道的三条原则）：
//! - 强约束系统内嵌在 `description()` 里——约束随工具 schema 每轮注入，
//!   模型每次调用都会先看到规范，等效于「渲染前强制读一遍规范」
//! - 工具载荷隔离——SVG 作为工具产出单向推给前端（`coding:assistant_message`
//!   携带 widgets），不进 LLM 上下文，画图不污染后续对话 token
//! - 受控渲染——前端经 dompurify 白名单 sanitize 后内联渲染，失败降级为可展开
//!   的原始代码卡片；后端在此再拦一道防御性校验（拒绝 <script>/事件/foreignObject）

use async_trait::async_trait;
use once_cell::sync::Lazy;
use parking_lot::RwLock;
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::brain::coding_agent::CodingWidget;
use crate::commands::coding_agent::CODING_AGENT;
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext, ValidationResult,
};

/// 全局 AppHandle（由 lib.rs setup 注入，与 send_image_tool 同机制）。
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用一次）。
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write() = Some(handle);
}

/// 可视化组件工具。
pub struct ShowWidgetTool;

impl ShowWidgetTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ShowWidgetTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ShowWidgetTool {
    fn name(&self) -> &str {
        "show_widget"
    }

    fn description(&self) -> &str {
        r#"Render an inline SVG visualization (flowchart, architecture diagram, sequence/state/class diagram, or simple chart) as a standalone card in the coding chat. Use it when a diagram would materially lower the reader's comprehension cost; do NOT draw for trivial answers.

Hard constraints (follow ALL of them; the renderer rejects violations):
- Output ONLY a raw <svg> element, nothing else. No <html>/<head>/<body>, no DOCTYPE, no markdown code fences.
- Root <svg> must set viewBox="0 0 680 H" (width is fixed at 680; H = lowest element bottom + 20) and width="100%". Keep all content inside x=40..640.
- Transparent background (the host provides the card background). No gradients, no drop shadows, no glow, no blur.
- Every shape and every <text> must set an explicit fill (and stroke where needed) inline; never rely on CSS classes.
- font-size must be >= 11px; use only font-weight 400 or 500.
- No <script>, no event-handler attributes (onclick/onload/...), no <foreignObject>, no <iframe>, no <style>, no position:fixed, no external images or fonts.
- No emoji in any text.
- Warm-paper palette: ink #3b3428, ink-soft #6b6158, ink-faint #9a8f7d, border #d8ccb4, accent #537d96, accent-deep #3f6179, success #5d8a5f, warning #b2822a, danger #b3402f, surface #fffcf3."#
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => r#"把一张 SVG 可视化（流程图 / 架构图 / 时序图 / 状态图 / 类图 / 简单图表）渲染成编程对话流里的独立卡片。当一张图能显著降低读者理解成本时使用；琐碎回答不要画图。

硬约束（全部遵守，渲染器会拒绝违规内容）：
- 只输出原始 <svg> 元素，其它什么都不带。不要 <html>/<head>/<body>，不要 DOCTYPE，不要 markdown 代码围栏。
- 根 <svg> 必须设 viewBox="0 0 680 H"（宽度固定 680；H = 最下方元素的底部 + 20）且 width="100%"。所有内容保持在 x=40..640 内。
- 背景透明（宿主提供卡片背景）。禁止渐变、投影、发光、模糊。
- 每个形状和每段 <text> 都必须内联显式填色（需要描边时也显式写），不得依赖 CSS 类。
- font-size 不得小于 11px；字重只用 400 或 500。
- 禁止 <script>、事件处理器属性（onclick/onload/…）、<foreignObject>、<iframe>、<style>、position:fixed、外链图片或字体。
- 文字里禁止 emoji。
- 暖纸色板：正文 #3b3428、次级 #6b6158、弱化 #9a8f7d、边框 #d8ccb4、主色 #537d96、主色深 #3f6179、成功 #5d8a5f、警告 #b2822a、危险 #b3402f、纸面 #fffcf3。"#,
            "ja" => self.description(),
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "画个流程图\n画一张架构图\n用图说明调用链\n把状态流转画出来",
            "en" => "draw a flowchart\ndraw an architecture diagram\nvisualize the call chain\ndiagram the state transitions",
            "ja" => "フローチャートを描いて\nアーキテクチャ図を描いて",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short card title, used for display and as the export filename"
                },
                "widget_code": {
                    "type": "string",
                    "description": "The raw <svg>...</svg> markup, following every hard constraint in the tool description"
                }
            },
            "required": ["title", "widget_code"]
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        match lang {
            "zh" => json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "卡片标题，用于展示与导出文件名"
                    },
                    "widget_code": {
                        "type": "string",
                        "description": "原始 <svg>...</svg> 代码，必须遵守工具描述里的全部硬约束"
                    }
                },
                "required": ["title", "widget_code"]
            }),
            "ja" => json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "カードのタイトル（表示・書き出しファイル名に使用）"
                    },
                    "widget_code": {
                        "type": "string",
                        "description": "生の <svg>...</svg> マークアップ（ツール説明の全ハード制約に従う）"
                    }
                },
                "required": ["title", "widget_code"]
            }),
            _ => self.parameters_schema(),
        }
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let title = input.get("title").and_then(|v| v.as_str()).unwrap_or("").trim();
        let code = input
            .get("widget_code")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();
        if title.is_empty() {
            return ValidationResult::failure("title 不能为空", 2);
        }
        if code.is_empty() {
            return ValidationResult::failure("widget_code 不能为空", 2);
        }
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, ctx: &ToolUseContext) -> ToolResult {
        let title = args
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let code = args
            .get("widget_code")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        // 防御性校验：后端先拦一道，前端 dompurify 是主防线
        if !looks_like_svg(&code) {
            return ToolResult::standard_error(
                "widget_code 必须是原始 <svg> 元素（以 <svg 开头，不含 <html>/<body>）",
                None,
                None,
            );
        }
        if let Some(bad) = contains_forbidden(&code) {
            return ToolResult::standard_error(
                &format!("widget_code 含禁止内容：{bad}"),
                None,
                None,
            );
        }

        // show_widget 是工作侧工具，仅在编程会话内渲染到聊天流
        if ctx.session_id.is_empty() || !CODING_AGENT.has_session(&ctx.session_id) {
            return ToolResult::standard_error(
                "show_widget 仅在编程会话内可用（当前不在编程会话）",
                None,
                None,
            );
        }

        let widget = CodingWidget {
            title: title.clone(),
            kind: "svg".to_string(),
            code,
        };
        let app_handle = match APP_HANDLE.read().clone() {
            Some(h) => h,
            None => return ToolResult::standard_error("AppHandle 未初始化", None, None),
        };

        match CODING_AGENT.push_agent_widget(&app_handle, &ctx.session_id, vec![widget], "") {
            Ok(()) => ToolResult::standard_success(
                &format!("已渲染可视化组件：{title}"),
                Some(json!({ "rendered": true, "title": title })),
            ),
            Err(e) => ToolResult::standard_error("渲染组件失败", Some(&e), None),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Media
    }

    fn always_load(&self) -> bool {
        false
    }

    fn should_defer(&self) -> bool {
        // 聊天侧按需经 tool_search 加载；编程侧由 CODING_TOOLS 白名单直接注入完整 schema
        true
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn search_hint(&self) -> &str {
        "show widget diagram chart flowchart svg render 画图 流程图 图表 架构图"
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Drawing for trivial answers that are clear in one sentence",
            "Emitting HTML instead of a raw <svg> element",
            "Emitting a gradient/shadow/glow or a <script>/<foreignObject> element",
            "Emitting SVG without explicit inline fill colors",
        ]
    }
}

/// 基本合法性：trim 后以 `<svg` 开头，且不夹带整页外壳。
fn looks_like_svg(code: &str) -> bool {
    let lower = code.to_lowercase();
    lower.starts_with("<svg") && !lower.contains("<html") && !lower.contains("<body")
}

/// 危险标签/属性校验（与前端 dompurify 白名单互为双保险）。
/// 命中即返回禁止项名称。
fn contains_forbidden(code: &str) -> Option<&'static str> {
    let lower = code.to_lowercase();
    if lower.contains("<script") {
        return Some("script");
    }
    if lower.contains("foreignobject") {
        return Some("foreignObject");
    }
    if lower.contains("<iframe") {
        return Some("iframe");
    }
    for evt in ["onload=", "onclick=", "onerror=", "onmouseover=", "onfocus="] {
        if lower.contains(evt) {
            return Some(evt);
        }
    }
    None
}
