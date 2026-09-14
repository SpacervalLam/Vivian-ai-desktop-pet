//! 工作智能体询问工具（独立功能，与陪伴智能体的自由文本 `ask_user` 分离）。
//!
//! 面向编程会话的**选择题**：模型给出 2-4 个候选方向，用户在编程面板点选
//! （也可以自己写、或直接跳过），答案以工具返回值回流，agent loop 随即继续。
//!
//! 工具 `call()` 里 `await`
//! 一个 oneshot，`Sender` 存在注册表中，前端回答时把它唤醒。等待期间不产生
//! 任何 token——loop 就停在这一行，不消耗预算、不空转。
//!
//! 答案走**工具返回值**而非伪装成新的用户消息，上下文保持 append-only，
//! 对 API 的 KV cache 前缀友好。

use once_cell::sync::Lazy;
use serde_json::{Value, json};
use std::sync::RwLock;

use async_trait::async_trait;
use tauri::{AppHandle, Emitter};

use crate::brain::work_question::{
    WorkQuestionOption, global_work_question_registry,
};
use crate::tools::types::{PermissionResult, Tool, ToolResult, ToolCategory, ToolRiskTier, ToolUseContext, ValidationResult};

/// 全局 AppHandle（由 lib.rs setup 注入，用于 emit 事件给前端）。
static APP_HANDLE: Lazy<RwLock<Option<AppHandle>>> = Lazy::new(|| RwLock::new(None));

/// 注入 AppHandle（lib.rs setup 调用）。
pub fn set_app_handle(handle: AppHandle) {
    *APP_HANDLE.write().unwrap() = Some(handle);
}

/// 候选项最少 / 最多条数。
const MIN_OPTIONS: usize = 2;
const MAX_OPTIONS: usize = 4;
/// 问题正文最大字数。
const MAX_QUESTION_CHARS: usize = 500;
/// 选项标签最大字数（标签是祈使短句，说明放 description）。
const MAX_LABEL_CHARS: usize = 120;
/// 补充背景最大字数。
const MAX_CONTEXT_CHARS: usize = 2000;
/// 等待用户回答的上限（与注册表 TTL 对齐）。
const WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30 * 60);

/// 工作智能体向用户提问（选择题）。
pub struct WorkAskUserTool;

impl WorkAskUserTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WorkAskUserTool {
    fn default() -> Self {
        Self::new()
    }
}

const DESC_EN: &str = "Ask the user to choose a direction when the way forward is genuinely \
ambiguous and you cannot settle it by reading code or running a command. Present 2-4 concrete \
options; execution SUSPENDS until the user answers, so use it only for decisions the user owns \
(which approach, how far the scope goes, which trade-off to accept) — never for facts you could \
discover yourself, and never to ask whether to keep going. If you favour one option, put it FIRST \
and append \" (Recommended)\" to its label. The user may type a free-form answer instead, or skip; \
on a skip, pick the most reasonable direction yourself and say which one you took.";

const DESC_ZH: &str = "当推进方向确实存在分叉、且读代码或跑命令都无法判断该走哪条时，用这个工具让用户\
选方向。给出 2-4 个具体选项；提问期间执行会**挂起等待**，所以只用于「用户才拥有的决策」（选哪种方案、\
范围到哪里、接受哪种权衡），不要问自己查得清的事实，也不要问「要不要继续」。如果你倾向某个选项，\
把它放第一个并在标签后加「（推荐）」。用户也可以自己写答案或跳过；用户跳过时，你自己选最合理的方向\
继续，并在总结里说明选了哪条。";

const DESC_JA: &str = "コードを読んでもコマンドを実行しても判断できない分岐に直面したとき、ユーザーに\
方向を選んでもらいます。2〜4個の具体的な選択肢を提示してください。質問中は実行が**一時停止**します。\
ユーザーが持っている判断（どの手法、どこまでの範囲、どのトレードオフを許容するか）にだけ使い、\
自分で調べられる事実や「続けてよいか」には使わないでください。推奨する選択肢がある場合は先頭に置き、\
ラベルの末尾に「（推奨）」を付けてください。ユーザーは自由入力やスキップもできます。スキップされた\
場合は自分で最も妥当な方向を選び、どれを選んだかを最後に伝えてください。";

#[async_trait]
impl Tool for WorkAskUserTool {
    fn name(&self) -> &str {
        "work_ask_user"
    }

    fn description(&self) -> &str {
        DESC_EN
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => DESC_ZH,
            "ja" => DESC_JA,
            _ => DESC_EN,
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "这个方向你来定\n需要你确认\n接下来往哪走",
            "en" => "you decide the direction\nneed your confirmation\nwhich way should we go",
            "ja" => "方向を決めて\n確認が必要\n次はどちらへ",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        Self::schema_for("en")
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        Self::schema_for(lang)
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let question_ok = input
            .get("question")
            .and_then(|v| v.as_str())
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
        let options_ok = input
            .get("options")
            .and_then(|v| v.as_array())
            .map(|a| (MIN_OPTIONS..=MAX_OPTIONS).contains(&a.len()))
            .unwrap_or(false);
        if question_ok && options_ok {
            ValidationResult::success(Some(input.clone()))
        } else {
            ValidationResult::failure(
                format!(
                    "question（非空字符串）与 options（{MIN_OPTIONS}-{MAX_OPTIONS} 个选项）均为必填"
                ),
                2,
            )
        }
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        PermissionResult::allow()
    }

    async fn call(&self, args: Value, context: &ToolUseContext) -> ToolResult {
        // 子 agent 面前没有人类应答者，挂起等待会永久阻塞整条委派链。
        // 这里不是硬失败就算了——错误文案要指明出路：把未决问题写进最终结果，
        // 交给上层去问人或自己拍板。
        if context.subagent_depth > 0 {
            return ToolResult::standard_error(
                "子 agent 不能向用户提问：你面前没有人类应答者，等待回答会永久阻塞。\
                 请自行选择最合理的方向继续，并在你的最终结果里写明——你选了哪条路、\
                 依据是什么、还有哪些未决问题需要上层判断。上层会看到这段说明。",
                Some("SubagentCannotAsk"),
                None,
            );
        }
        let session_id = context.session_id.clone();
        let question = args
            .get("question")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if question.is_empty() {
            return ToolResult::standard_error("question 不能为空", Some("WorkQuestion"), None);
        }
        if question.chars().count() > MAX_QUESTION_CHARS {
            return ToolResult::standard_error(
                &format!("question 过长（上限 {MAX_QUESTION_CHARS} 字）"),
                Some("WorkQuestion"),
                None,
            );
        }

        let Some(raw_options) = args.get("options").and_then(|v| v.as_array()) else {
            return ToolResult::standard_error(
                &format!("options 必填，需 {MIN_OPTIONS}-{MAX_OPTIONS} 个选项"),
                Some("WorkQuestion"),
                None,
            );
        };
        if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&raw_options.len()) {
            return ToolResult::standard_error(
                &format!(
                    "选项数量 {} 不合法，需 {MIN_OPTIONS}-{MAX_OPTIONS} 个（选择题要少而明确，不是穷举）",
                    raw_options.len()
                ),
                Some("WorkQuestion"),
                None,
            );
        }
        let mut options: Vec<WorkQuestionOption> = Vec::with_capacity(raw_options.len());
        let mut seen: Vec<String> = Vec::new();
        for item in raw_options {
            let Some(label) = item.get("label").and_then(|v| v.as_str()) else {
                return ToolResult::standard_error(
                    "每个选项都需要 label 字段（字符串）",
                    Some("WorkQuestion"),
                    None,
                );
            };
            let label = label.trim().to_string();
            if label.is_empty() {
                return ToolResult::standard_error("选项 label 不能为空", Some("WorkQuestion"), None);
            }
            if label.chars().count() > MAX_LABEL_CHARS {
                return ToolResult::standard_error(
                    &format!("选项「{label}」过长（上限 {MAX_LABEL_CHARS} 字）"),
                    Some("WorkQuestion"),
                    None,
                );
            }
            if seen.contains(&label) {
                return ToolResult::standard_error(
                    &format!("选项「{label}」重复"),
                    Some("WorkQuestion"),
                    None,
                );
            }
            seen.push(label.clone());
            let description = item
                .get("description")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            options.push(WorkQuestionOption { label, description });
        }

        let ctx_text = args
            .get("context")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        if let Some(c) = &ctx_text {
            if c.chars().count() > MAX_CONTEXT_CHARS {
                return ToolResult::standard_error(
                    &format!("context 过长（上限 {MAX_CONTEXT_CHARS} 字）"),
                    Some("WorkQuestion"),
                    None,
                );
            }
        }
        let multi_select = args
            .get("multi_select")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // 拿不到 AppHandle 就没法把问题送到用户面前——直接失败，
        // 绝不能让 loop 挂在一个永远没人回答的 await 上。
        let handle = match APP_HANDLE.read().unwrap().clone() {
            Some(h) => h,
            None => {
                return ToolResult::standard_error(
                    "无法向用户提问（后端未初始化）",
                    Some("WorkQuestionUnavailable"),
                    None,
                )
            }
        };

        let registry = global_work_question_registry();
        let (question_id, rx) = registry.create_question(
            session_id.clone(),
            question.clone(),
            ctx_text.clone(),
            options.clone(),
            multi_select,
        );
        let request = crate::brain::work_question::WorkQuestionRequest {
            question_id,
            session_id: session_id.clone(),
            question,
            context: ctx_text,
            options,
            multi_select,
        };
        if let Err(e) = handle.emit("coding:question", json!(request)) {
            registry.cancel_question(question_id);
            return ToolResult::standard_error(
                &format!("广播提问失败：{e}"),
                Some("WorkQuestionUnavailable"),
                None,
            );
        }

        // 挂起等待。等待期间不产生 token、不消耗轮次预算。
        match tokio::time::timeout(WAIT_TIMEOUT, rx).await {
            Ok(Ok(answer)) => {
                let text = answer.render();
                ToolResult::standard_success(
                    &text,
                    Some(json!({
                        "selected": answer.selected,
                        "custom": answer.custom,
                        "skipped": answer.is_skipped(),
                    })),
                )
            }
            // sender 被 drop：会话被取消，或问题被清理
            Ok(Err(_)) => ToolResult::standard_success(
                "提问已被取消（用户取消了本次任务）。请停止继续推进，等待用户的下一条指示。",
                Some(json!({ "cancelled": true })),
            ),
            Err(_) => ToolResult::standard_success(
                "等待用户回答超时（30 分钟）。请自行选择最合理的方向继续，并在最终总结里说明你选了哪条路。",
                Some(json!({ "timeout": true })),
            ),
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }

    fn always_load(&self) -> bool {
        true
    }

    fn search_hint(&self) -> &str {
        "work ask user question choice option direction clarify"
    }
}

impl WorkAskUserTool {
    /// 参数 schema（三语）。
    fn schema_for(lang: &str) -> Value {
        let (q, opts, label, desc, multi, ctx) = match lang {
            "zh" => (
                "要问用户的问题。",
                "候选方向，2-4 个。若你倾向其中一个，放第一个并在标签后加「（推荐）」。",
                "选项标签：一个简短的祈使句。",
                "该方向的一句话说明，帮用户判断。",
                "是否允许多选（默认单选）。",
                "可选：你目前的判断依据 / 已尝试过什么，帮用户做决定。",
            ),
            "ja" => (
                "ユーザーへの質問。",
                "2〜4個の候補。推奨するものがあれば先頭に置き、ラベル末尾に「（推奨）」を付けてください。",
                "選択肢ラベル：短い命令文。",
                "その方向の一言説明。",
                "複数選択を許可するか（既定は単一選択）。",
                "任意：判断材料や試したことを補足します。",
            ),
            _ => (
                "The question to ask the user.",
                "The candidate directions, 2-4 of them. If you favour one, put it first and append \" (Recommended)\" to its label.",
                "Option label: a short imperative line.",
                "A one-line explanation of this direction, to help the user choose.",
                "Whether several options may be selected at once (default: single choice).",
                "Optional: the reasoning behind your question — what you found, what you already tried.",
            ),
        };
        json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": q },
                "options": {
                    "type": "array",
                    "description": opts,
                    "minItems": MIN_OPTIONS,
                    "maxItems": MAX_OPTIONS,
                    "items": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "label": { "type": "string", "description": label },
                            "description": { "type": "string", "description": desc }
                        },
                        "required": ["label"]
                    }
                },
                "multi_select": { "type": "boolean", "description": multi },
                "context": { "type": "string", "description": ctx }
            },
            "required": ["question", "options"]
        })
    }
}
