//! 反思调用 Runnable（合并表情/动作 + 心理状态推断）
//!
//! 在主对话 LLM 生成 text 之后，单次调用 LLM 产出全部结构化字段：
//! - expression / motion / expression_duration_ms（原 ExpressionMotionRunnable）
//! - user_emotion / ai_emotion / appraisal / emotion_update / behavior_drive / event_summary（原 PsychologyInsightRunnable）
//!
//! 设计要点：
//! - **独立精简协议**：不重复发送主对话的工具协议、检索与附件；
//!   协议保持稳定，动态区仅含有界对话、角色视角与成长候选。
//! - **沉浸感保护**：主对话仍独立生成 text（纯文本），反思调用只填结构化字段，
//!   不会因多字段输出稀释主文本质量。
//! - **15s 超时降级**：反思失败使用默认心理值与表情。

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

use crate::brain::json_parser::{parse_appraisal, parse_behavior_drive, parse_emotion_deltas};
use crate::engine::expression_stats;
use crate::engine::manifest::ResourceManifest;
use crate::error::VivianResult;
use crate::mind::user_goals::{parse_deadline, GoalUpdateOp, UserGoalLedger, UserGoalSource, UserGoalState};
use crate::pipeline::base::Runnable;
use crate::pipeline::state::PipelineState;
use crate::providers::base::LLMRequest;
use crate::providers::router::ModelRouter;
use crate::types::response::ChatMessage;
use crate::world::WorldState;

/// 表情/动作选择器系统提示词（保留用于 prompt 模板预览展示）
pub const EXPRESSION_MOTION_SYSTEM_PROMPT: &str = r#"You are the expression/motion selector for a desktop pet character. Based on the conversation, choose the most appropriate expression and motion for the reply.

Output format: json only
{"expression": "", "expression_duration_ms": 0, "motion": ""}

Fields:
- expression: expression name from the available expressions list; leave "" if nothing fits
- expression_duration_ms: how long the expression lasts in milliseconds
    * 0 = lasts until next natural switch (default, for weak/neutral emotions)
    * 1500-3000 = brief flash (for subtle reactions like a small smile, sweat drop)
    * 4000-6000 = medium duration (for clear emotions like anger, shyness, surprise)
    * 8000+ = long duration (for strong emotions like crying, blank stare, shock)
- motion: motion name from the available motions list; leave "" if nothing fits

Rules:
- Default to leaving expression and motion empty (""); only fill them when the reply has clear emotional tone
- Default expression_duration_ms to 0 (natural switch); only specify specific milliseconds when emotion intensity is clear
- If nothing fits, leave all fields empty — never force a choice
- expression and motion MUST come from the provided lists only; do not invent names"#;

/// 提取文本中所有括号内的内容（不含括号本身）。
///
/// 同时支持全角（）和半角()括号，返回逗号分隔的提取结果。
/// 无括号内容时返回空字符串。
fn extract_parenthetical_hints(text: &str) -> String {
    let mut hints: Vec<String> = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '(' || chars[i] == '（' {
            let open = chars[i];
            let close = if open == '（' { '）' } else { ')' };
            let mut depth = 1;
            let mut j = i + 1;
            while j < chars.len() && depth > 0 {
                if chars[j] == open { depth += 1; }
                else if chars[j] == close { depth -= 1; }
                j += 1;
            }
            if depth == 0 {
                let inner: String = chars[(i + 1)..(j - 1)].iter().collect();
                let trimmed = inner.trim();
                if !trimmed.is_empty() {
                    hints.push(trimmed.to_string());
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    hints.join(", ")
}

/// 构造最近对话段落，注入到反思 user prompt 之前。
///
/// - 取末 6 条消息（约 3 轮对话）
/// - 每条格式 `{role}: {content}`，content 截断 100 字符
/// - role 用 "User"/"AI"（其他角色跳过）
/// - 段落标题三语：[Recent Conversation] / [最近对话] / [最近の会話]
/// - 无消息时返回空字符串（不注入段落）
fn build_recent_conversation_section(messages: &[ChatMessage]) -> String {
    if messages.is_empty() {
        return String::new();
    }
    let take = 6.min(messages.len());
    let start = messages.len() - take;
    let recent = &messages[start..];

    let mut lines: Vec<String> = Vec::new();
    for msg in recent {
        let role = match msg.role.as_str() {
            "user" => "User",
            "assistant" => "AI",
            _ => continue,
        };
        let content = msg.content.trim();
        if content.is_empty() {
            continue;
        }
        let truncated = crate::utils::truncate_chars(content, 100);
        let suffix = if content.chars().count() > 100 { "…" } else { "" };
        lines.push(format!("{}: {}{}", role, truncated, suffix));
    }

    if lines.is_empty() {
        return String::new();
    }

    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
    let title = match lang_norm {
        "en" => "[Recent Conversation]",
        "ja" => "[最近の会話]",
        _ => "[最近对话]",
    };
    format!("{}\n{}\n\n", title, lines.join("\n"))
}

/// 反思调用的稳定 system 协议。
///
/// 设计原则：
/// - 明确各字段优先级（text 已在主对话生成，反思只填结构化字段）
/// - 字段定义参考原 ExpressionMotionRunnable + PsychologyInsightRunnable
const REFLECTION_DIRECTIVE: &str = include_str!("../../../prompts/framework/reflection_compact.md");

/// Only saved, unredacted user wording can support growth; recalled/model text cannot.
fn growth_evidence(state: &PipelineState, evolution: &Value) -> Option<crate::persona::evolution::GrowthEvidence> {
    if state.current_channel == "cross_character"
        || state.metadata.get("skip_memory_save").and_then(Value::as_bool).unwrap_or(false) { return None; }
    let quote = evolution.get("source_quote")?.as_str()?.trim();
    let content = state.metadata.get("growth_source_content")?.as_str()?;
    if quote.chars().count() < 6 || quote.chars().count() > 500
        || !state.user_input.contains(quote) || !content.contains(quote) { return None; }
    use sha2::{Digest, Sha256};
    Some(crate::persona::evolution::GrowthEvidence {
        memory_id: state.metadata.get("growth_source_memory_id")?.as_str()?.into(),
        timestamp: state.metadata.get("growth_source_timestamp")?.as_f64()?,
        quote: quote.into(),
        fingerprint: format!("{:x}", Sha256::digest(content.trim().as_bytes())),
    })
}

#[derive(Clone)]
pub struct ReflectionRunnable {
    active_turn: Option<(Arc<std::sync::atomic::AtomicU64>, u64)>,
    pub router: Option<Arc<ModelRouter>>,
    pub manifest: Option<Arc<ResourceManifest>>,
    pub char_id: String,
    /// 内联标签模式：仅推断心理状态（表情/动作已由流式扫描器实时处理）
    pub inline_enabled: bool,
    /// 世界状态引用：解析 world_update 后直接写入用户活动状态机
    pub world_state: Option<Arc<WorldState>>,
    /// 用户长期目标账本引用：解析 goal_updates 后写入用户目标
    pub user_goals: Option<Arc<UserGoalLedger>>,
    /// 人格引擎引用：解析 evolution 后应用自我进化（语气/性格调整）
    pub persona: Option<Arc<crate::persona::PersonaEngine>>,
}

impl ReflectionRunnable {
    pub fn new(
        router: Option<Arc<ModelRouter>>,
        manifest: Option<Arc<ResourceManifest>>,
        inline_enabled: bool,
        char_id: impl Into<String>,
    ) -> Self {
        Self {
            active_turn: None,
            router,
            manifest,
            inline_enabled,
            char_id: char_id.into(),
            world_state: None,
            user_goals: None,
            persona: None,
        }
    }

    /// 注入世界状态引用（用于解析 world_update 后更新用户活动状态机）
    pub fn for_turn(&self, revision: Arc<std::sync::atomic::AtomicU64>, turn: u64) -> Self {
        let mut reflection = self.clone();
        reflection.active_turn = Some((revision, turn));
        reflection
    }

    fn is_current_turn(&self) -> bool {
        self.active_turn.as_ref().is_none_or(|(revision, turn)| {
            revision.load(std::sync::atomic::Ordering::SeqCst) == *turn
        })
    }

    pub fn with_world_state(mut self, world_state: Arc<WorldState>) -> Self {
        self.world_state = Some(world_state);
        self
    }

    /// 注入用户长期目标账本引用（用于解析 goal_updates 后写入用户目标）
    pub fn with_user_goals(mut self, user_goals: Arc<UserGoalLedger>) -> Self {
        self.user_goals = Some(user_goals);
        self
    }

    /// 注入人格引擎引用（用于解析 evolution 后应用自我进化）
    pub fn with_persona(mut self, persona: Arc<crate::persona::PersonaEngine>) -> Self {
        self.persona = Some(persona);
        self
    }

    /// 解析 world_update 并写入世界状态
    ///
    /// LLM 输出 null 时不动状态机；输出 user_activity 且 confidence >= 0.7 时更新；
    /// confidence < 0.7 视为不确定，忽略。
    fn apply_world_update(&self, json: &Value) {
        let Some(world_state) = self.world_state.as_ref() else {
            return;
        };
        let Some(world_update) = json.get("world_update") else {
            return;
        };
        if world_update.is_null() {
            return;
        }
        let Some(user_activity) = world_update.get("user_activity").and_then(|v| v.as_str()) else {
            return;
        };
        let label = user_activity.trim();
        if label.is_empty() {
            return;
        }
        let confidence = world_update
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.8);
        if confidence < 0.7 {
            tracing::debug!(
                "[Reflection:{}] world_update confidence {:.2} < 0.7，忽略 user_activity=\"{}\"",
                self.char_id,
                confidence,
                label
            );
            return;
        }
        world_state.update_user_activity(label, confidence);
        tracing::info!(
            "[Reflection:{}] 用户活动状态已更新: \"{}\" (confidence={:.2})",
            self.char_id,
            label,
            confidence
        );
    }

    /// 解析 goal_updates 数组并写入用户长期目标账本
    ///
    /// LLM 输出空数组或字段缺失时不动账本；每条操作按 action 分发到 create/transition/update_deadline。
    /// create 强制要求 source_quote（用户原话），缺失则跳过该条目。
    fn apply_goal_updates(&self, json: &Value) {
        let Some(ledger) = self.user_goals.as_ref() else {
            return;
        };
        let Some(arr) = json.get("goal_updates").and_then(|v| v.as_array()) else {
            return;
        };
        if arr.is_empty() {
            return;
        }
        for item in arr {
            let Ok(op) = serde_json::from_value::<GoalUpdateOp>(item.clone()) else {
                continue;
            };
            let action = op.action.trim().to_lowercase();
            let label = op.label.as_deref().unwrap_or("").trim().to_string();
            if label.is_empty() && action != "create" {
                continue;
            }
            match action.as_str() {
                "create" => {
                    if label.is_empty() {
                        continue;
                    }
                    let quote = op.source_quote.as_deref().unwrap_or("").trim().to_string();
                    if quote.is_empty() {
                        tracing::debug!(
                            "[Reflection:{}] goal_updates create 跳过：缺 source_quote (label=\"{}\")",
                            self.char_id,
                            label
                        );
                        continue;
                    }
                    let deadline = op.deadline.as_deref().and_then(parse_deadline);
                    let now = chrono::Local::now().timestamp() as f64;
                    let source = UserGoalSource::Dialogue { quote, extracted_at: now };
                    ledger.create(&label, deadline, source);
                    ledger.enforce_capacity();
                }
                "pause" => {
                    ledger.transition_state(&label, UserGoalState::Paused);
                }
                "complete" => {
                    ledger.transition_state(&label, UserGoalState::Completed);
                }
                "abandon" => {
                    ledger.transition_state(&label, UserGoalState::Abandoned);
                }
                "update_deadline" => {
                    let deadline = op.deadline.as_deref().and_then(parse_deadline);
                    ledger.update_deadline(&label, deadline);
                }
                other => {
                    tracing::debug!(
                        "[Reflection:{}] 未知 goal_updates action: {}",
                        self.char_id,
                        other
                    );
                }
            }
        }
    }

    /// 解析 evolution 字段并应用到人格引擎（自我进化）。
    ///
    /// LLM 输出 null 或字段缺失时不动覆盖层；tone/personality 至少填一个，
    /// 且受覆盖层内部最小间隔与去重限制。
    fn apply_evolution(&self, json: &Value, state: &PipelineState) {
        if state.current_channel == "cross_character"
            || state.metadata.get("skip_memory_save").and_then(Value::as_bool).unwrap_or(false) {
            return;
        }
        let Some(persona) = self.persona.as_ref() else {
            return;
        };
        let Some(evolution) = json.get("evolution") else {
            return;
        };
        if evolution.is_null() {
            return;
        }
        let tone = evolution.get("tone").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let personality = evolution.get("personality").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let reason = evolution.get("reason").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();

        let scope = evolution.get("scope").and_then(Value::as_str).unwrap_or("");
        let Some(evidence) = growth_evidence(state, evolution) else { return; };
        let explicit = evolution.get("explicit_feedback").and_then(Value::as_bool).unwrap_or(false);
        // One interpretation per event/scope; prefer the semantic understanding over wording tweaks.
        let (kind, text) = if !personality.is_empty() { ("personality", &personality) } else { ("tone", &tone) };
        let revises = evolution.get("revises").and_then(Value::as_str);
        if persona.revise_evolution(kind, scope, text, &reason, evidence, explicit, revises) {
            tracing::info!("[Reflection:{}] 已更新场景理解: {}", self.char_id, scope);
        }
        // All learned behavior stays in the sourced, reversible growth layer.
        // Do not also write unscoped pattern-library rules from the same reflection.
    }

    fn char_display_name(&self) -> &str {
        match self.char_id.as_str() {
            "nana" => "Nana",
            _ => "Vivian",
        }
    }

    fn char_cn_name(&self) -> &str {
        match self.char_id.as_str() {
            "nana" => "Nana",
            _ => "Vivian",
        }
    }

    /// 构造情境-表情学习提示段落
    fn build_expression_hint_section(&self, user_emotion: &str) -> String {
        let emotion = user_emotion.trim();
        if emotion.is_empty() {
            return String::new();
        }
        let situations = [emotion];
        let hints = expression_stats::get_expression_hints(&self.char_id, &situations);
        if hints.is_empty() {
            return String::new();
        }
        let lines: Vec<String> = hints
            .iter()
            .map(|(sit, expr, w)| format!("  - 情境[{}] → {}（权重 {:.1}）", sit, expr, w))
            .collect();
        format!("\n历史同类情境常用表情参考：\n{}\n", lines.join("\n"))
    }

    /// 记录本次表情选择到情境-表情映射表
    fn record_expression_learning(&self, state: &PipelineState) {
        let expr = state.expression.trim();
        if expr.is_empty() {
            return;
        }
        let situation = state.user_emotion.trim();
        if situation.is_empty() {
            return;
        }
        expression_stats::record_expression_use(&self.char_id, situation, expr);
    }

    /// 构造反思调用的 messages
    ///
    /// - system: stable, compact analyzer protocol, independent of chat/tool context
    /// - user: bounded conversation, character perspective, goals and resource lists
    fn build_messages(&self, state: &PipelineState) -> Vec<ChatMessage> {
        let system = ChatMessage::system(REFLECTION_DIRECTIVE);

        // 可用表情/动作列表（从 manifest 提取）
        let (expressions, motions) = match self.manifest.as_deref() {
            Some(m) => (m.expressions().join(", "), m.motions().join(", ")),
            None => (String::new(), String::new()),
        };

        let reply_excerpt = reflection_excerpt(&state.text, 1600);
        let paren_hints = extract_parenthetical_hints(&reply_excerpt);
        let paren_section = if paren_hints.is_empty() {
            String::new()
        } else {
            format!("\n角色回复中的情绪/动作暗示：{}\n", paren_hints)
        };

        // 最近对话：取末 6 条消息（约 3 轮），让 LLM 判断情绪趋势
        let recent_section = build_recent_conversation_section(&state.messages);

        let expr_hint_section = self.build_expression_hint_section(&state.user_emotion);

        let growth_candidates = self.persona.as_ref().map(|p| {
            let items: Vec<_> = p.evolution_candidates().into_iter().map(|c|
                serde_json::json!({"reference":c.reference(),"kind":c.kind,"scope":c.scope,"text":c.text})).collect();
            let learned: Vec<_> = p.evolution_entries().into_iter().filter(|e| e.active()).map(|e|
                serde_json::json!({"scope":e.scope,"text":e.text})).collect();
            format!("\n角色视角（保留用户设置）：{}\n已生效理解：{}\n待验证候选（不是事实，仅本轮新增证据可支持）：{}\n",
                p.reflection_profile(), serde_json::to_string(&learned).unwrap_or_default(),
                serde_json::to_string(&items).unwrap_or_default())
        }).unwrap_or_default();
        let goals = self.user_goals.as_ref().map(|ledger| {
            let items: Vec<_> = ledger.active_briefs(8).into_iter().map(|g|
                serde_json::json!({"label":g.label,"state":g.state})).collect();
            format!("\n已知长期目标：{}\n", serde_json::to_string(&items).unwrap_or_default())
        }).unwrap_or_default();
        let user_content = format!(
            "回复前心情（内部参考，回复措辞不是新的外部事件）：{}\n{growth_candidates}{goals}{recent_section}用户输入：{}\n\n{} 的回复：{}{}\n可用表情：{}\n可用动作：{}\n{}",
            state.metadata.get("mood_before_reply").map(Value::to_string).unwrap_or_else(|| "未提供；无证据时不猜测变化".into()),
            reflection_excerpt(&state.user_input, 3200),
            self.char_cn_name(),
            reply_excerpt,
            paren_section,
            expressions,
            motions,
            expr_hint_section,
        );
        let user = ChatMessage::user(user_content);

        // 保留 char_display_name 用于未来扩展（如角色化反思指令）
        let _ = self.char_display_name();

        vec![system, user]
    }

    async fn call_llm(&self, state: &PipelineState) -> Option<Value> {
        let router = self.router.as_ref()?;
        let messages = self.build_messages(state);

        match router.generate(LLMRequest::new("chat", messages)
            .with_usage_tag("reflection")
            .with_max_tokens(1536)
            .with_reasoning_pref(crate::providers::reasoning::ReasoningPreference {
                mode: crate::providers::reasoning::ReasoningMode::Off, effort: None, budget_tokens: None })
            .with_character_id(self.char_id.clone())).await {
            Ok(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    return None;
                }
                serde_json::from_str::<Value>(trimmed)
                    .ok()
                    .or_else(|| extract_json_object(trimmed))
            }
            Err(e) => {
                tracing::warn!(
                    "[Reflection:{}] LLM 调用失败，使用默认心理值与表情: {}",
                    self.char_id,
                    e
                );
                None
            }
        }
    }

    /// 将反思 JSON 应用到 PipelineState
    fn apply_to_state(state: &mut PipelineState, json: &Value, manifest: Option<&ResourceManifest>) {
        let is_cross_character = state.current_channel == "cross_character";
        // ── 表情/动作 ──
        if let Some(expr) = json.get("expression").and_then(|v| v.as_str()) {
            let expr = expr.trim();
            if !expr.is_empty() {
                let normalized = manifest
                    .map(|m| m.normalize_expression(expr))
                    .unwrap_or_else(|| expr.to_string());
                state.expression = normalized;
            }
        }
        if let Some(duration) = json.get("expression_duration_ms").and_then(|v| v.as_u64()) {
            state.expression_duration_ms = duration;
        }
        if let Some(motion) = json.get("motion").and_then(|v| v.as_str()) {
            let motion = motion.trim();
            if !motion.is_empty() {
                let normalized = manifest
                    .map(|m| m.normalize_motion(motion))
                    .unwrap_or_else(|| motion.to_string());
                state.motion = normalized;
            }
        }
        // control_actions（桌宠自控指令，由反思调用产出）
        if let Some(actions) = json.get("control_actions").and_then(|v| v.as_array()) {
            if !actions.is_empty() {
                state.control_actions = actions.to_vec();
            }
        }

        // ── 心理状态 ──
        if !is_cross_character {
        if let Some(user_emo) = json.get("user_emotion").and_then(|v| v.as_str()) {
            let user_emo = user_emo.trim().to_lowercase();
            if !user_emo.is_empty() {
                state.user_emotion = user_emo;
            }
        }
        if let Some(intensity) = json.get("user_emotion_intensity").and_then(|v| v.as_f64()) {
            state.user_emotion_intensity = intensity.clamp(0.0, 1.0);
        }
        if let Some(ai_emo) = json.get("ai_emotion").and_then(|v| v.as_str()) {
            let ai_emo = ai_emo.trim().to_lowercase();
            if !ai_emo.is_empty() {
                state.emotion = Some(ai_emo);
            }
        }
        if let Some(imp) = json.get("importance_user").and_then(|v| v.as_f64()) {
            state.importance_user = imp.clamp(0.0, 1.0);
        }
        if let Some(imp) = json.get("importance_ai").and_then(|v| v.as_f64()) {
            state.importance_ai = imp.clamp(0.0, 1.0);
        }
        if let Some(appraisal) = json.get("appraisal") {
            state.appraisal = parse_appraisal(appraisal);
        }
        if let Some(emotion_update) = json.get("emotion_update") {
            state.emotion_update = parse_emotion_deltas(emotion_update);
        }
        if let Some(behavior_drive) = json.get("behavior_drive") {
            state.behavior_drive = parse_behavior_drive(behavior_drive);
        }
        if let Some(event_summary) = json.get("event_summary").and_then(|v| v.as_str()) {
            state.event_summary = event_summary.trim().to_string();
        }
        if let Some(ltm) = json.get("long_term_memory").and_then(|v| v.as_str()) {
            let quote = json
                .get("long_term_memory_source_quote")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            if !ltm.trim().is_empty()
                && !quote.is_empty()
                && state.user_input.contains(quote)
            {
                state.long_term_memory = ltm.trim().to_string();
                state.metadata["long_term_memory_source_quote"] =
                    Value::String(quote.to_string());
            } else if !ltm.trim().is_empty() {
                tracing::warn!("[Reflection] 长期记忆缺少可核验的用户原话，已丢弃");
            }
        }
        }
    }
}

/// Bound machine analysis without touching the visible reply; keep both task and conclusion.
fn reflection_excerpt(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(limit).collect();
    if chars.next().is_none() { return head; }
    let head: String = head.chars().take(limit / 2).collect();
    let tail: String = text.chars().rev().take(limit / 2).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}\n[中段省略，仅按可见证据分析]\n{tail}")
}

#[async_trait]
impl Runnable for ReflectionRunnable {
    async fn ainvoke(
        &self,
        input: Value,
        _config: Option<crate::pipeline::base::RunnableConfig>,
    ) -> VivianResult<Value> {
        let mut state = PipelineState::from_json(input);

        // 跳过条件：命令、不应答、graceful_exit、text 为空
        if state.is_command || !state.should_respond || state.graceful_exit || state.text.is_empty()
        {
            return Ok(state.to_json());
        }

        // 自我重复度自检：本轮回复与最近若干条自己的发言做轻量相似度比较。
        //
        // "活人感"此前只能靠肉眼看日志判断，改动有没有生效无从验证。
        // 这里把它变成可观测的量：纯本地字符 n-gram 计算，不做嵌入调用，
        // 成本可忽略。套话化会以 warn 形式在日志里暴露出来。
        log_self_repetition(&state, &self.char_id);

        // 内联标签模式：表情/动作已由流式扫描器实时处理，跳过反思调用
        // 但仍需心理状态推断——保留独立的心理推断路径（轻量调用）
        if self.inline_enabled {
            // 内联模式下仅做心理推断（使用独立精简协议）
            // 表情/动作字段保持默认（已被流式扫描器填充）
            return self.run_psychology_only(state).await;
        }

        // 15 秒超时降级：反思是锦上添花，不阻塞用户响应
        let timeout = std::time::Duration::from_secs(15);
        match tokio::time::timeout(timeout, self.call_llm(&state)).await {
            Ok(Some(json)) => {
                if !self.is_current_turn() { return Ok(state.to_json()); }
                Self::apply_to_state(&mut state, &json, self.manifest.as_deref());
                if state.current_channel != "cross_character" {
                    self.apply_world_update(&json);
                    self.apply_goal_updates(&json);
                }
                self.apply_evolution(&json, &state);
                self.record_expression_learning(&state);
            }
            Ok(None) => {
                tracing::debug!("[Reflection:{}] 无 LLM 输出，保留默认值", self.char_id);
            }
            Err(_) => {
                tracing::warn!(
                    "[Reflection:{}] 15s 超时，降级为默认心理值与表情",
                    self.char_id
                );
            }
        }

        Ok(state.to_json())
    }
}

impl ReflectionRunnable {
    /// 内联标签模式下仅做心理推断（表情/动作已由流式扫描器处理）
    async fn run_psychology_only(&self, mut state: PipelineState) -> VivianResult<Value> {
        let timeout = std::time::Duration::from_secs(15);
        match tokio::time::timeout(timeout, self.call_llm(&state)).await {
            Ok(Some(json)) => {
                if !self.is_current_turn() { return Ok(state.to_json()); }
                // 只应用心理字段，不覆盖表情/动作（已被流式扫描器填充）
                if state.current_channel != "cross_character" {
                if let Some(user_emo) = json.get("user_emotion").and_then(|v| v.as_str()) {
                    let user_emo = user_emo.trim().to_lowercase();
                    if !user_emo.is_empty() {
                        state.user_emotion = user_emo;
                    }
                }
                if let Some(intensity) =
                    json.get("user_emotion_intensity").and_then(|v| v.as_f64())
                {
                    state.user_emotion_intensity = intensity.clamp(0.0, 1.0);
                }
                if let Some(ai_emo) = json.get("ai_emotion").and_then(|v| v.as_str()) {
                    let ai_emo = ai_emo.trim().to_lowercase();
                    if !ai_emo.is_empty() {
                        state.emotion = Some(ai_emo);
                    }
                }
                if let Some(imp) = json.get("importance_user").and_then(|v| v.as_f64()) {
                    state.importance_user = imp.clamp(0.0, 1.0);
                }
                }
                if let Some(imp) = json.get("importance_ai").and_then(|v| v.as_f64()) {
                    state.importance_ai = imp.clamp(0.0, 1.0);
                }
                if let Some(appraisal) = json.get("appraisal") {
                    state.appraisal = parse_appraisal(appraisal);
                }
                if let Some(emotion_update) = json.get("emotion_update") {
                    state.emotion_update = parse_emotion_deltas(emotion_update);
                }
                if let Some(behavior_drive) = json.get("behavior_drive") {
                    state.behavior_drive = parse_behavior_drive(behavior_drive);
                }
                if let Some(event_summary) = json.get("event_summary").and_then(|v| v.as_str()) {
                    state.event_summary = event_summary.trim().to_string();
                }
                if let Some(ltm) = json.get("long_term_memory").and_then(|v| v.as_str()) {
                    let quote = json
                        .get("long_term_memory_source_quote")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .unwrap_or("");
                    if !ltm.trim().is_empty()
                        && !quote.is_empty()
                        && state.user_input.contains(quote)
                    {
                        state.long_term_memory = ltm.trim().to_string();
                        state.metadata["long_term_memory_source_quote"] =
                            Value::String(quote.to_string());
                    } else if !ltm.trim().is_empty() {
                        tracing::warn!("[Reflection] 长期记忆缺少可核验的用户原话，已丢弃");
                    }
                }
                // world_update 在内联模式下同样处理（与表情/动作无关，属于世界状态判断）
                if state.current_channel != "cross_character" {
                    self.apply_world_update(&json);
                    self.apply_goal_updates(&json);
                }
                self.apply_evolution(&json, &state);
            }
            Ok(None) => {
                tracing::debug!("[Reflection:{}] 内联模式无 LLM 输出", self.char_id);
            }
            Err(_) => {
                tracing::warn!(
                    "[Reflection:{}] 内联模式 15s 超时，降级为默认心理值",
                    self.char_id
                );
            }
        }

        Ok(state.to_json())
    }
}

/// 从可能含非 JSON 前后缀的文本中提取第一个 JSON 对象
fn extract_json_object(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str::<Value>(&text[start..=end]).ok()
}

/// 自我重复度自检的告警阈值（2-gram Jaccard 相似度）
const SELF_REPETITION_WARN_THRESHOLD: f64 = 0.55;
/// 参与比较的最近自身发言条数
const SELF_REPETITION_WINDOW: usize = 6;

/// 计算两段文本的字符 2-gram Jaccard 相似度
///
/// 刻意不用嵌入：这只是一条自检日志，不值得为此多一次模型调用。
/// 字符 n-gram 对"句式照旧、换几个字"这类套话化足够敏感。
fn bigram_jaccard(a: &str, b: &str) -> f64 {
    let grams = |s: &str| -> std::collections::HashSet<String> {
        let chars: Vec<char> = s.chars().collect();
        if chars.len() < 2 {
            return std::collections::HashSet::new();
        }
        chars
            .windows(2)
            .map(|w| w.iter().collect::<String>())
            .collect()
    };
    let ga = grams(a);
    let gb = grams(b);
    if ga.is_empty() || gb.is_empty() {
        return 0.0;
    }
    let inter = ga.intersection(&gb).count();
    let union = ga.union(&gb).count();
    inter as f64 / union as f64
}

/// 本轮回复与最近自身发言的重复度自检，结果写入日志
///
/// 与 `LoopDetectionAdvisor` 的分工：
/// - `LoopDetectionAdvisor` 在**生成时**拦完全相同的输出并触发重试（事后补救）
/// - 这里在**生成后**度量近似重复并留痕（可观测），不打断流程
fn log_self_repetition(state: &crate::pipeline::state::PipelineState, char_id: &str) {
    let text = state.text.trim();
    // 过短的回复（"嗯""好"）本身 2-gram 就少，比不出意义，跳过
    if text.chars().count() < 6 {
        return;
    }

    let mut max_sim: f64 = 0.0;
    let mut hit: Option<String> = None;
    let mut compared = 0usize;

    for m in state.messages.iter().rev() {
        if compared >= SELF_REPETITION_WINDOW {
            break;
        }
        if m.role != "assistant" || m.content.trim().is_empty() {
            continue;
        }
        compared += 1;
        let sim = bigram_jaccard(text, m.content.trim());
        if sim > max_sim {
            max_sim = sim;
            hit = Some(m.content.chars().take(40).collect());
        }
    }

    if compared == 0 {
        return;
    }

    if max_sim >= SELF_REPETITION_WARN_THRESHOLD {
        tracing::warn!(
            "[SelfRepetition:{}] 本轮回复与最近发言高度雷同（相似度 {:.2}）：\"{}\" —— \
             可能已滑成套话，可检查 recent_self_utterances 是否生效",
            char_id,
            max_sim,
            hit.unwrap_or_default()
        );
    } else {
        tracing::debug!(
            "[SelfRepetition:{}] 与最近 {} 条发言的最高相似度 {:.2}",
            char_id,
            compared,
            max_sim
        );
    }
}

#[cfg(test)]
mod mood_feedback_tests {
    use super::*;

    #[test]
    fn reflection_sees_actual_reply_and_mood_baseline() {
        let reflection = ReflectionRunnable::new(None, None, true, "vivian");
        let mut state = PipelineState::new("我只想讨论，不要执行".into());
        state.text = "那我们先梳理这个想法。".into();
        state.metadata["mood_before_reply"] = serde_json::json!({"energy":23,"stress":61,"focus":42});
        let messages = reflection.build_messages(&state);
        let content = &messages[1].content;
        assert!(content.contains(&state.text) && content.contains(&state.user_input));
        assert!(content.contains("\"energy\":23"));
        assert!(messages[0].content.contains("不重复叠加 appraisal"));
        assert!(messages[0].content.contains("自己的语气"));
    }

    #[test]
    fn reflection_parses_relief_and_curiosity_as_net_change() {
        let mut state = PipelineState::new("聊完好多了".into());
        let json = serde_json::json!({
            "ai_emotion":"calm",
            "appraisal":{"threat":0.0,"rejection":0.0,"control":0.7,"fairness":0.5,"novelty":0.6,"significance":0.4},
            "emotion_update":{"anger":-0.06,"fear":-0.04,"closeness":0.02,"curiosity":0.05}
        });
        ReflectionRunnable::apply_to_state(&mut state, &json, None);
        let delta = state.emotion_update.unwrap();
        assert_eq!(delta.anger, -0.06);
        assert_eq!(delta.curiosity, 0.05);
        assert_eq!(delta.closeness, 0.02);
        assert_eq!(state.appraisal.unwrap().control, 0.7);
    }

    #[test]
    fn compact_analysis_excludes_chat_payload_and_keeps_both_ends_of_long_text() {
        let reflection = ReflectionRunnable::new(None, None, true, "vivian");
        let mut state = PipelineState::new(format!("用户开头{}用户结尾", "中".repeat(20000)));
        state.text = format!("回复开头{}回复结尾", "文".repeat(20000));
        state.system_prompt = "MAIN_TOOL_PROTOCOL_SHOULD_NOT_BE_COPIED".repeat(10000);
        let messages = reflection.build_messages(&state);
        let body = &messages[1].content;
        assert!(body.contains("用户开头") && body.contains("用户结尾"));
        assert!(body.contains("回复开头") && body.contains("回复结尾"));
        assert!(body.contains("中段省略"));
        assert!(!messages.iter().any(|m| m.content.contains("MAIN_TOOL_PROTOCOL")));
        assert!(body.chars().count() < 5300);
        assert!(state.text.chars().count() > 20000, "visible response remains complete");
        assert_eq!(reflection_excerpt("普通短消息", 1600), "普通短消息");
    }

    #[test]
    #[ignore = "controlled token measurement against the pre-change Git source"]
    fn reflection_token_performance_baseline() {
        let source = std::process::Command::new("git")
            .args(["show", "c01fe000ce975a20e00f1dc7414024cb49892236:src-tauri/src/pipeline/steps/reflection.rs"])
            .current_dir(env!("CARGO_MANIFEST_DIR")).output().unwrap();
        assert!(source.status.success());
        let source = String::from_utf8(source.stdout).unwrap();
        let old_directive = source.split_once("const REFLECTION_DIRECTIVE: &str = r#\"").unwrap().1
            .split_once("\"#;").unwrap().0;
        let tokenizer = tiktoken_rs::cl100k_base().unwrap();
        let old_tokens = tokenizer.encode_with_special_tokens(old_directive).len();
        let new_tokens = tokenizer.encode_with_special_tokens(REFLECTION_DIRECTIVE).len();
        println!("reflection_protocol old_tokens={old_tokens} new_tokens={new_tokens}");
        assert!(new_tokens < old_tokens * 3 / 4);
        let config = crate::persona::schemas::default_persona_for("vivian");
        let core = crate::persona::prompt_render::render_character_block(&config, "zh");
        let reflection = ReflectionRunnable::new(None, None, true, "vivian");
        let mut state = PipelineState::new("我希望你先听具体困扰，再给建议".into());
        state.system_prompt = core;
        state.text = "好的，我会先听你讲清楚。".into();
        let old_total = tokenizer.encode_with_special_tokens(&format!("{}\n用户输入：{}\nVivian 的回复：{}\n{}",
            state.system_prompt, state.user_input, state.text, old_directive)).len();
        let new_total: usize = reflection.build_messages(&state).iter()
            .map(|m| tokenizer.encode_with_special_tokens(&m.content).len()).sum();
        println!("reflection_fixture_without_profile old_tokens={old_total} new_tokens={new_total}");
        let profile: String = [crate::persona::prompt_render::CharacterSection::Identity,
            crate::persona::prompt_render::CharacterSection::Personality,
            crate::persona::prompt_render::CharacterSection::Speech].into_iter()
            .map(|s| crate::utils::truncate_chars(&crate::persona::prompt_render::resolve_section(&config, s, "zh"), 600))
            .collect::<Vec<_>>().join("\n");
        let new_with_profile = new_total + tokenizer.encode_with_special_tokens(&profile).len();
        println!("reflection_fixture_with_profile old_tokens={old_total} new_tokens={new_with_profile}");
        assert!(new_with_profile < old_total);
    }
}

#[cfg(test)]
mod growth_tests {
    use super::*;
    #[test]
    fn growth_needs_saved_user_evidence_not_assistant_or_synthetic_text() {
        let quote = "Please stop calling me boss";
        let mut state = PipelineState::new(quote.into());
        let proposal = serde_json::json!({"source_quote":quote});
        assert!(growth_evidence(&state,&proposal).is_none());
        state.metadata["growth_source_memory_id"] = serde_json::json!("source-1");
        state.metadata["growth_source_timestamp"] = serde_json::json!(86400.0);
        state.metadata["growth_source_content"] = serde_json::json!(quote);
        assert!(growth_evidence(&state,&proposal).is_some());
        state.current_channel = "cross_character".into();
        assert!(growth_evidence(&state,&proposal).is_none());
        state.current_channel.clear();
        state.metadata["skip_memory_save"] = serde_json::json!(true);
        assert!(growth_evidence(&state,&proposal).is_none());
        state.metadata["skip_memory_save"] = serde_json::json!(false);
        state.text = quote.into();
        state.user_input = "hello".into();
        assert!(growth_evidence(&state,&proposal).is_none());
    }
}
