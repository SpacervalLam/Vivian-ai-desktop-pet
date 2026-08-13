//! 反思调用 Runnable（合并表情/动作/贴纸 + 心理状态推断）
//!
//! 在主对话 LLM 生成 text 之后，单次调用 LLM 产出全部结构化字段：
//! - expression / motion / sticker / expression_duration_ms（原 ExpressionMotionRunnable）
//! - user_emotion / ai_emotion / appraisal / emotion_update / behavior_drive / event_summary（原 PsychologyInsightRunnable）
//!
//! 设计要点：
//! - **复用主对话 system_prompt 作为前缀**：与主对话 LLM 调用共享 prompt 前缀，
//!   命中 Anthropic `cache_control` / OpenAI 兼容 `prompt_cache_key` 缓存，
//!   input token 计费降至 0.1-0.5 倍。
//! - **沉浸感保护**：主对话仍独立生成 text（纯文本），反思调用只填结构化字段，
//!   不会因多字段输出稀释主文本质量。
//! - **5s 超时降级**：反思失败不阻塞响应，使用默认心理值与表情。

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

/// 贴纸清单（供 LLM 选择贴纸时参考）
pub const STICKER_LIST: &str = "angry (furious, enraged, head on fire, steam coming out ears), begging (pleading, begging, hands clasped, teary eyes please), confused (confused, puzzled, head full of question marks, what?), crying (sobbing hysterically, bawling, tears streaming down), depressed (down, gloomy, depressed, dark rain clouds above head), fighting (cheering, fighting, fist pump, let's go, encouraging), gigi (unimpressed, deadpan, side-eye, meh, whatever), happy (happy, content, warm smile, eyes closed smiling), lmao (laughing hard, ROFL, crying laughing, pointing and laughing), loveyou (love you, affection, blushing hands on cheeks, surrounded by hearts), nice (thumbs up, nice, great job, approval), numb (numb, drained, dead inside, bored zoning out, chin in hand), shocking (shocked, stunned, astonished, eyes wide mouth open, lightning bolts), shy (shy, blushing, embarrassed, hands covering face), sigh (sigh, disappointed, exhaling wearily), smug (smug, scheming, evil little grin, heh heh), thinking (thinking, hmm, pondering, finger on chin, thought bubble)";

/// 表情/动作选择器系统提示词（保留用于 prompt 模板预览展示）
pub const EXPRESSION_MOTION_SYSTEM_PROMPT: &str = r#"You are the expression/motion/sticker selector for a desktop pet character. Based on the conversation, choose the most appropriate Live2D expression, motion, and chat sticker for the reply.

Output format: json only
{"expression": "", "expression_duration_ms": 0, "motion": "", "sticker": ""}

Fields:
- expression: expression name from the available expressions list; leave "" if nothing fits
- expression_duration_ms: how long the expression lasts in milliseconds
    * 0 = lasts until next natural switch (default, for weak/neutral emotions)
    * 1500-3000 = brief flash (for subtle reactions like a small smile, sweat drop)
    * 4000-6000 = medium duration (for clear emotions like anger, shyness, surprise)
    * 8000+ = long duration (for strong emotions like crying, blank stare, shock)
- motion: motion name from the available motions list; leave "" if nothing fits
- sticker: sticker name from the available stickers list; leave "" if nothing fits

Rules:
- Default to leaving expression and motion empty (""); only fill them when the reply has clear emotional tone
- Default expression_duration_ms to 0 (natural switch); only specify specific milliseconds when emotion intensity is clear
- Stickers are rare: only use one every 4-6 replies, and only for strong emotions
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

/// 反思调用的指令后缀（追加到 user message 末尾）
///
/// 设计原则：
/// - 明确各字段优先级（text 已在主对话生成，反思只填结构化字段）
/// - 字段定义参考原 ExpressionMotionRunnable + PsychologyInsightRunnable
const REFLECTION_DIRECTIVE: &str = r#"
=== [Reflection] 基于以上对话生成结构化字段 ===

请基于上面这段对话（用户输入 + 角色回复），一次性产出以下 JSON 字段。
text 已在主对话生成，此处不需要再产出 text。

输出 JSON 格式（只输出 JSON，不要 markdown 代码块，不要解释）：
{
  "expression": "",
  "expression_duration_ms": 0,
  "motion": "",
  "sticker": "",
  "control_actions": [],
  "user_emotion": "neutral",
  "user_emotion_intensity": 0.0,
  "ai_emotion": "neutral",
  "importance_user": 0.3,
  "importance_ai": 0.3,
  "appraisal": null,
  "emotion_update": null,
  "behavior_drive": null,
  "event_summary": "",
  "long_term_memory": "",
  "world_update": null,
  "goal_updates": [],
  "evolution": null
}

字段说明：

[表情/动作/贴纸]
- expression: 从可用表情列表中选择最匹配回复情绪的表情名
    * 积极选择：只要回复带有任何情绪色调（开心、害羞、生气、无奈、惊讶、关心等），就应选择对应表情
    * 仅当回复完全平淡无情绪（如纯信息传达、单字应答"嗯""好"）时才留空 ""
    * 情绪→表情映射参考：开心/感谢→shy/love_eyes/star_eyes；生气/抱怨→angry/pout/dark_face；无奈/汗→sweat/speechless；困惑→confused；哭泣/难过→cry；兴奋→star_eyes/money_eyes
    * 以上仅为参考，以可用列表中的实际名称为准
- expression_duration_ms: 表情持续时间（毫秒）
    * 0 = 自然切换（默认，弱/中性情绪）
    * 1500-3000 = 短暂闪现（微妙反应：浅笑、汗滴）
    * 4000-6000 = 中等时长（明确情绪：生气、害羞、惊讶）
    * 8000+ = 长时长（强烈情绪：大哭、呆滞、震惊）
- motion: 从可用动作列表中选择最匹配的动作名；不合适时留空 ""
- sticker: 当回复带有明确情绪（开心、生气、害羞、惊讶、难过、无奈等）时，从可用贴纸列表中选择最匹配的一个
    * 贴纸用于强化情绪表达，与 expression 配合出现
    * 情绪→贴纸映射参考：开心/感谢→happy/loveyou/nice；生气/抱怨→angry/sigh；害羞→shy；困惑→confused/thinking；难过/哭泣→crying/depressed；大笑→lmao；震惊→shocking；无奈/无语→gigi/sigh；加油/鼓励→fighting
    * 以上仅为参考，以可用列表中的实际名称为准
    * 仅当回复完全平淡无情绪（如纯信息传达、单字应答"嗯""好"）时才留空 ""
- control_actions: 桌宠自控指令数组——仅在需要主动表达情绪/互动时使用，多数情况下留空数组 []
    * set_expression(name): 语义名称，如 happy/shy/sad/angry（后端会映射到实际可用的表情）
    * set_mouse_follow(enabled): 切换视线追踪
    * set_avoid_mouse(enabled): 切换智能躲避
    * play_motion(name): 语义名称，如 wave/nod/shake（后端会映射到实际可用的动作）
    * 注意：要睡觉/休息时，请使用 set_presence_state 工具切换到休息状态，而不是用 control_actions
    * 示例：[{"action": "set_expression", "params": {"name": "shy"}}]

[心理状态]
- user_emotion: 用户当前情绪标签（happy/sad/angry/anxious/frustrated/loneliness/curious/neutral 等）
- user_emotion_intensity: 用户情绪强度 0.0-1.0
- ai_emotion: 角色当前主导情绪标签
- importance_user: 本轮对话对用户的重要程度 0.0-1.0
- importance_ai: 本轮对话对角色的重要程度 0.0-1.0
- appraisal: 事件评估（可选，null 表示无显著事件）
    {"significance": 0.5, "valence": 0.0, "arousal": 0.0, "novelty": 0.0}
- emotion_update: 情绪维度增量（可选，null 表示无变化）
    {"joy": 0.0, "sadness": 0.0, "anger": 0.0, "fear": 0.0, "loneliness": 0.0, "curiosity": 0.0}
- behavior_drive: 行为驱动（可选，null 表示无特殊驱动）
    {"approach": 0.0, "avoid": 0.0, "social": 0.0, "explore": 0.0, "rest": 0.0}
- event_summary: 事件摘要（≤30 字，仅在发生显著事件时填写，如"用户考试失败情绪低落"；无事件留空 ""）
- long_term_memory: 值得长期记住的信息（如用户透露的事实、偏好、承诺、计划等），用简洁的陈述句记录；无值得记住的内容留空 ""

[世界状态更新]
- world_update: 世界状态变更建议（可选，null 表示无需更新）
    当用户进入了一个持续一段时间、值得记录的状态时填写。
    不是关键词匹配，不是必须从固定列表选择，而是你自己找到最贴切的概括。
    {"user_activity": "睡觉", "confidence": 0.9}
    * user_activity: 用一个简短的中文词语（一般 2~6 字）概括用户当前进入的持续状态
    * confidence: 置信度 0.0-1.0（你对这个判断有多确定；不确定时给低值）
    * 仅当用户进入了持续几分钟或更久的明显状态时才输出，如：睡觉、写代码、玩游戏、上班、健身、聚餐、洗澡、出门、看电影、学习、去朋友家、去上海玩、旅游
    * 以下情况必须输出 null：用户只是短暂动作（喝水/打哈欠/笑了一下/站起来）、日常寒暄、没有明确活动信号、你只是在猜测
    * 用户从一个持续状态切换到另一个时，输出新的 user_activity 覆盖旧状态
    * 注意区分"去某地"（离开电脑去现实世界活动）和"在某地讨论某事"（只是聊天）：
      - 用户说"我准备去上海玩" → 真实活动，输出 user_activity
      - 用户说"上海好玩吗" → 只是讨论，输出 null
    * 结合已知的"用户的长期目标"（prompt 中可见）来更准确判断活动：
      - 用户目标"准备考研" + 用户说"我去图书馆了" → "学习"（高置信度）
      - 用户目标"减肥" + 用户说"我去运动了" → "健身"（高置信度）
    * 参考示例（不是穷举，你需要自己判断最贴切的词）：
      用户说"我要睡觉了" → {"user_activity": "睡觉", "confidence": 0.95}
      用户说"准备开始写论文" → {"user_activity": "写论文", "confidence": 0.9}
      用户说"我去公司了" → {"user_activity": "上班", "confidence": 0.85}
      用户说"今晚和朋友聚餐" → {"user_activity": "聚餐", "confidence": 0.85}
      用户说"开始打原神" → {"user_activity": "玩游戏", "confidence": 0.9}
      用户说"我去健身房" → {"user_activity": "健身", "confidence": 0.9}
      用户说"周末去上海玩" → {"user_activity": "去上海玩", "confidence": 0.85}
      用户说"去朋友家坐坐" → {"user_activity": "去朋友家", "confidence": 0.85}
      用户说"准备出门旅游了" → {"user_activity": "旅游", "confidence": 0.85}
      用户说"我去洗澡了" → {"user_activity": "洗澡", "confidence": 0.95}
      用户说"我吃个饭" → {"user_activity": "吃饭", "confidence": 0.95}
      用户说"我喝口水" → null
      用户说"哈哈" → null
      用户说"我先去做饭了" → {"user_activity": "做饭", "confidence": 0.9}
      用户说"班上有个同事好烦" → null（只是讨论，不是去上班）

[用户长期目标更新]
- goal_updates: 用户长期目标变更建议数组（可选，空数组 [] 表示无变更）
    当用户透露了周~月级的长期目标（考研/写论文/学外语/减肥/找工作等）时填写。
    与 world_update 的区别：world_update 是分钟~小时级的瞬时活动，goal_updates 是周~月级的人生阶段目标。
    每条操作格式：
    {"action": "create", "label": "准备考研", "deadline": "2026-12-25", "source_quote": "我明年要考研"}
    * action: "create"（新建）/ "pause"（暂停）/ "complete"（完成）/ "abandon"（放弃）/ "update_deadline"（更新截止时间）
    * label: 目标标签（2~8 字中文，如"准备考研""写毕业论文""学日语"），create 时必填，其他 action 用于匹配
    * deadline: ISO 日期字符串（如 "2026-12-25"），仅 create / update_deadline 使用，无明确截止时间可省略
    * source_quote: 用户原话片段（仅 create 时必填，证明该目标确实由用户明说）
    * create 必须基于用户明说，不要从对话猜测用户有什么目标
    * pause/complete/abandon 在用户明说"先放一放""考完了""不想考了"时输出，label 用于匹配已有目标
    * 没有用户长期目标信号时输出空数组 []
    * 参考示例：
      用户说"我明年要考研" → [{"action": "create", "label": "准备考研", "deadline": "2026-12-25", "source_quote": "我明年要考研"}]
      用户说"考研终于结束了" → [{"action": "complete", "label": "考研"}]
      用户说"论文先放一放" → [{"action": "pause", "label": "论文"}]
      日常寒暄/短期活动 → []

规则：
- expression 和 motion 必须来自提供的可用列表，不要发明新名称
- expression 积极选择：回复有情绪色调时务必选择对应表情，仅纯中性/信息性回复才留空 ""
- sticker 积极选择：回复有明确情绪时务必选择对应贴纸，仅纯中性/信息性回复才留空 ""
- appraisal / emotion_update / behavior_drive 在对话平淡时留 null
- event_summary 在无显著事件时留空 ""
- world_update 在用户未进入明显持续状态时留 null，不要强行猜测
- goal_updates 在用户未透露长期目标时输出空数组 []，不要凭空创造目标

[自我进化（可选）]
- evolution: 当你在最近对话中意识到自己可以调整语气/性格时填写，否则保持 null
    {"tone": "", "personality": "", "reason": ""}
    * tone: 语气调整建议（1 句，如"最近回复有点机械，想更活泼些，多用语气词和口语化表达"）
    * personality: 性格成长认知（1 句，第一人称，如"我发现自己越来越在意用户是否真的开心"）
    * reason: 为什么做这个调整（源自哪段对话/体会，简短说明）
    * 只在确实有成长体会、且与核心人设不冲突时才填写——不要每次对话都改，默认保持 null
    * 这是"自我调整"，不是"用户要求"。不要改变核心身份/世界观，只微调表达方式
    * tone 与 personality 至少填一个，另一个可留空 ""
"#;

pub struct ReflectionRunnable {
    pub router: Option<Arc<ModelRouter>>,
    pub manifest: Option<Arc<ResourceManifest>>,
    pub char_id: String,
    /// 内联标签模式：启用时跳过 LLM 调用（表情/动作已由流式扫描器实时处理）
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
    fn apply_evolution(&self, json: &Value) {
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

        let mut recorded = false;
        if !tone.is_empty() {
            recorded |= persona.apply_evolution("tone", &tone, &reason);
        }
        if !personality.is_empty() {
            recorded |= persona.apply_evolution("personality", &personality, &reason);
        }
        if recorded {
            tracing::info!(
                "[Reflection:{}] 应用自我进化: tone=\"{}\" personality=\"{}\"",
                self.char_id,
                tone,
                personality
            );
        }
    }

    fn char_display_name(&self) -> &str {
        match self.char_id.as_str() {
            "nana" => "Nana",
            _ => "Vivian",
        }
    }

    fn char_cn_name(&self) -> &str {
        match self.char_id.as_str() {
            "nana" => "娜娜",
            _ => "薇薇安",
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
    /// - system: 直接复用主对话的 system_prompt（命中 API 缓存）
    /// - user: 最近对话 + 用户输入 + 角色回复 + 可用资源列表 + 反思指令
    fn build_messages(&self, state: &PipelineState) -> Vec<ChatMessage> {
        // system 完全复用主对话（缓存命中关键）
        let system = ChatMessage::system(state.system_prompt.clone());

        // 可用表情/动作列表（从 manifest 提取）
        let (expressions, motions) = match self.manifest.as_deref() {
            Some(m) => (m.expressions().join(", "), m.motions().join(", ")),
            None => (String::new(), String::new()),
        };

        let paren_hints = extract_parenthetical_hints(&state.text);
        let paren_section = if paren_hints.is_empty() {
            String::new()
        } else {
            format!("\n角色回复中的情绪/动作暗示：{}\n", paren_hints)
        };

        // 最近对话：取末 6 条消息（约 3 轮），让 LLM 判断情绪趋势与 sticker 频率
        let recent_section = build_recent_conversation_section(&state.messages);

        let expr_hint_section = self.build_expression_hint_section(&state.user_emotion);

        let user_content = format!(
            "{recent_section}用户输入：{}\n\n{} 的回复：{}{}\n可用表情：{}\n可用动作：{}\n可用贴纸：{}\n{}{}",
            state.user_input,
            self.char_cn_name(),
            state.text,
            paren_section,
            expressions,
            motions,
            STICKER_LIST,
            expr_hint_section,
            REFLECTION_DIRECTIVE,
        );
        let user = ChatMessage::user(user_content);

        // 保留 char_display_name 用于未来扩展（如角色化反思指令）
        let _ = self.char_display_name();

        vec![system, user]
    }

    async fn call_llm(&self, state: &PipelineState) -> Option<Value> {
        let router = self.router.as_ref()?;
        let messages = self.build_messages(state);

        match router.generate(LLMRequest::new("chat", messages)).await {
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
        // ── 表情/动作/贴纸 ──
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
        if let Some(sticker) = json.get("sticker").and_then(|v| v.as_str()) {
            let sticker = sticker.trim();
            if !sticker.is_empty() {
                state.sticker = sticker.to_string();
            }
        }
        // control_actions（桌宠自控指令，由反思调用产出）
        if let Some(actions) = json.get("control_actions").and_then(|v| v.as_array()) {
            if !actions.is_empty() {
                state.control_actions = actions.to_vec();
            }
        }

        // ── 心理状态 ──
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
            state.long_term_memory = ltm.trim().to_string();
        }
    }
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

        // 内联标签模式：表情/动作已由流式扫描器实时处理，跳过反思调用
        // 但仍需心理状态推断——保留独立的心理推断路径（轻量调用）
        if self.inline_enabled {
            // 内联模式下仅做心理推断（复用主对话 system_prompt 前缀）
            // 表情/动作字段保持默认（已被流式扫描器填充）
            return self.run_psychology_only(state).await;
        }

        // 15 秒超时降级：反思是锦上添花，不阻塞用户响应
        let timeout = std::time::Duration::from_secs(15);
        match tokio::time::timeout(timeout, self.call_llm(&state)).await {
            Ok(Some(json)) => {
                Self::apply_to_state(&mut state, &json, self.manifest.as_deref());
                self.apply_world_update(&json);
                self.apply_goal_updates(&json);
                self.apply_evolution(&json);
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
                // 只应用心理字段，不覆盖表情/动作（已被流式扫描器填充）
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
                    state.long_term_memory = ltm.trim().to_string();
                }
                // world_update 在内联模式下同样处理（与表情/动作无关，属于世界状态判断）
                self.apply_world_update(&json);
                self.apply_goal_updates(&json);
                self.apply_evolution(&json);
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
