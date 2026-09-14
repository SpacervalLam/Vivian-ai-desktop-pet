//! 异步反思（合并 Consciousness Update + Activity Extractor）
//!
//! 在 `brain.think()` 中通过 `tokio::spawn` 调用，fire-and-forget。
//! 合并原 `consciousness_update_async` 和 `activity_extractor` 两次独立 LLM 调用为单次。
//!
//! 设计原则：
//! - **节流触发**：5 轮对话 OR 30 分钟间隔 OR 触发（避免每轮都调）
//! - **激烈对话抑制**：连续两轮间隔 < 10s 时推迟触发
//! - **失败静默**：网络/解析/超时仅 `tracing::debug!`，不影响主路径
//! - **独立 system prompt**：与主对话人设无关，避免角色 prompt 干扰实体/活动提取

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::mind::Mind;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::psychology::PsychologyManager;
use crate::types::response::ChatMessage;

/// 节流阈值
const TURNS_THRESHOLD: u32 = 5;
const TIME_THRESHOLD: Duration = Duration::from_secs(30 * 60);
const INTENSE_DIALOG_GAP: Duration = Duration::from_secs(10);

/// 合并后的 system prompt（约 300 token）
fn async_reflection_prompt() -> String {
    let lang_norm = crate::pipeline::prompt_modules::normalize_lang(&crate::i18n::get_language());
    match lang_norm {
        "en" => r#"You are an attention analyzer. Analyze the user input and extract the following:

1. **entities**: Notable entities mentioned in the user input (people/things/activities/concepts), each with a weight 0.3-1.0
   - The user themselves → "user", weight 1.0
   - Current conversation partner → weight 0.8
   - Specific things (e.g. "exam", "work", "music") → weight 0.3-0.8, by importance
   - At most 5 entities

2. **emotion_hint**: A short tag for the user's current emotion (e.g. "happy"/"sad"/"anxious"/"neutral")

Output JSON (no markdown code blocks, no other text):
{
  "entities": [{"entity": "exam", "weight": 0.8}],
  "emotion_hint": "anxious"
}

Rules:
- All fields are optional; output empty array or empty string when there is no content
- If the input is too simple (e.g. "yeah", "ok", "oh"), output {"entities":[], "emotion_hint": "neutral"}
- Output only JSON, no explanation"#.to_string(),
        "ja" => r#"あなたは注意力アナライザーです。ユーザー入力を分析し、以下の情報を抽出してください：

1. **entities**: ユーザー入力に登場した注目すべきエンティティ（人名/事物/活動/概念）。各エンティティに重み 0.3-1.0 を付与
   - ユーザー自身 → "user"、重み 1.0
   - 現在の会話相手 → 重み 0.8
   - 具体的な事物（例：「試験」「仕事」「音楽」）→ 重み 0.3-0.8、重要度に応じて
   - 最大 5 つのエンティティ

2. **emotion_hint**: ユーザーの現在の感情の短いタグ（例："happy"/"sad"/"anxious"/"neutral"）

JSONを出力（markdownコードブロックなし、他のテキスト一切なし）：
{
  "entities": [{"entity": "試験", "weight": 0.8}],
  "emotion_hint": "anxious"
}

ルール：
- すべてのフィールドは省略可能。内容がない場合は空配列または空文字列を出力
- 入力が短すぎる場合（例：「うん」「いいよ」「ええ」）は {"entities":[], "emotion_hint": "neutral"} を出力
- JSONのみを出力し、説明は不要"#.to_string(),
        _ => r#"你是注意力分析器。分析用户输入，提炼以下信息：

1. **entities**: 用户输入中提到的值得关注的实体（人名/事物/活动/概念），每个实体附带权重 0.3-1.0
   - 用户自己 → "user"，权重 1.0
   - 当前对话对象 → 权重 0.8
   - 具体事物（如"考试"、"工作"、"音乐"）→ 权重 0.3-0.8，按重要性
   - 最多 5 个实体

2. **emotion_hint**: 用户当前情绪的简短标签（如 "happy"/"sad"/"anxious"/"neutral"）

输出 JSON（不要 markdown 代码块，不要任何其他文字）：
{
  "entities": [{"entity": "考试", "weight": 0.8}],
  "emotion_hint": "anxious"
}

规则：
- 字段全部可选，无内容时输出空数组或空字符串
- 输入太简单（如"嗯"、"好"、"哦"）时输出 {"entities":[], "emotion_hint": "neutral"}
- 只输出 JSON，不要解释"#.to_string(),
    }
}

/// LLM 返回的实体条目
#[derive(Debug, Clone, Serialize, Deserialize)]
struct EntityBoost {
    entity: String,
    weight: f32,
}

/// LLM 返回的完整结构
#[derive(Debug, Clone, Deserialize)]
struct AsyncReflectionResult {
    #[serde(default)]
    entities: Vec<EntityBoost>,
    #[serde(default)]
    emotion_hint: Option<String>,
}

/// 节流状态（每个角色独立）
#[derive(Debug, Clone)]
struct ThrottleState {
    turns_since_last: u32,
    last_update_at: Instant,
    last_user_msg_at: Instant,
}

impl Default for ThrottleState {
    fn default() -> Self {
        Self {
            turns_since_last: 0,
            last_update_at: Instant::now(),
            last_user_msg_at: Instant::now(),
        }
    }
}

/// 全局节流状态（按 char_id 索引）
static THROTTLE_STATES: once_cell::sync::Lazy<Mutex<std::collections::HashMap<String, ThrottleState>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

/// 判断是否应触发异步反思
///
/// 触发条件（OR 关系）：
/// 1. 累计对话轮数 ≥ 5 轮
/// 2. 距上次触发 > 30 分钟
///
/// 抑制条件：
/// - 激烈对话（连续两轮间隔 < 10s）→ 推迟到对话间隙
pub fn should_trigger(char_id: &str) -> bool {
    let mut states = THROTTLE_STATES.lock();
    let state = states.entry(char_id.to_string()).or_default();

    let now = Instant::now();
    let turns_ok = state.turns_since_last >= TURNS_THRESHOLD;
    let time_ok = now.duration_since(state.last_update_at) > TIME_THRESHOLD;
    let not_intense = now.duration_since(state.last_user_msg_at) > INTENSE_DIALOG_GAP;

    // 更新 last_user_msg_at（本轮用户消息时间）
    state.last_user_msg_at = now;
    state.turns_since_last += 1;

    (turns_ok || time_ok) && not_intense
}

/// 重置节流计数器（触发成功后调用）
fn reset_throttle(char_id: &str) {
    let mut states = THROTTLE_STATES.lock();
    if let Some(state) = states.get_mut(char_id) {
        state.turns_since_last = 0;
        state.last_update_at = Instant::now();
    }
}

/// 异步反思入口
///
/// 合并 consciousness_update + activity_extractor，单次 LLM 调用同时产出：
/// - 实体 + 情绪预反应（原 consciousness_update_async）
/// - 活动事件（原 activity_extractor）
///
/// `ai_reply`：当前轮 AI 回复文本（用于辅助实体抽取与情绪识别；为空时降级）
/// `recent_context`：最近对话上下文（已格式化好的多行文本；为空时降级）
pub async fn run_async_reflection(
    router: Arc<ModelRouter>,
    mind: Arc<Mind>,
    user_input: String,
    ai_reply: &str,
    recent_context: &str,
    char_id: String,
    psychology: Option<Arc<PsychologyManager>>,
) {
    // 节流检查
    if !should_trigger(&char_id) {
        return;
    }

    let trimmed = user_input.trim();
    if trimmed.len() < 4 {
        return;
    }

    let ai_reply_trimmed = ai_reply.trim();
    let recent_context_trimmed = recent_context.trim();

    // 拼接 user prompt：用户输入 + AI 回复 + 最近对话（非空字段才注入）
    let mut user_prompt = format!("用户输入：{}", trimmed);
    if !ai_reply_trimmed.is_empty() {
        user_prompt.push_str(&format!("\nAI回复：{}", ai_reply_trimmed));
    }
    if !recent_context_trimmed.is_empty() {
        user_prompt.push_str(&format!("\n最近对话：\n{}", recent_context_trimmed));
    }

    let messages = vec![
        ChatMessage::system(async_reflection_prompt()),
        ChatMessage::user(user_prompt),
    ];

    // 超时：直连 10s，代理链路 20s
    let timeout_secs = if router.uses_proxy() { 20u64 } else { 10u64 };
    let result = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        router.generate(LLMRequest::new("reflection", messages)),
    )
    .await;

    let response_text = match result {
        Ok(Ok(text)) => text,
        Ok(Err(e)) => {
            tracing::debug!("[AsyncReflection:{}] LLM 调用失败: {}", char_id, e);
            return;
        }
        Err(_) => {
            tracing::debug!("[AsyncReflection:{}] LLM 超时（{}s）", char_id, timeout_secs);
            return;
        }
    };

    let parsed: AsyncReflectionResult = match parse_json(&response_text) {
        Some(p) => p,
        None => {
            tracing::debug!(
                "[AsyncReflection:{}] JSON 解析失败，原始: {}",
                char_id,
                response_text.chars().take(200).collect::<String>()
            );
            return;
        }
    };

    // 应用实体 boost
    let now = chrono::Utc::now().timestamp();
    let mut boosted_count = 0;
    for entity_boost in &parsed.entities {
        let entity = entity_boost.entity.trim().to_lowercase();
        if entity.is_empty() {
            continue;
        }
        let weight = entity_boost.weight.clamp(0.1, 1.0);
        mind.boost_attention(&entity, weight, now);
        boosted_count += 1;
    }
    if boosted_count > 0 {
        tracing::debug!(
            "[AsyncReflection:{}] 补充 boost {} 个实体",
            char_id,
            boosted_count
        );
    }

    // 应用 emotion_hint 到心理学系统
    if let Some(ref hint) = parsed.emotion_hint {
        if let Some(psy) = &psychology {
            apply_emotion_hint(psy, hint, &char_id);
        }
    }

    // 触发成功，重置节流计数器
    reset_throttle(&char_id);
}

/// 应用 LLM 识别的情绪标签作为微增量（±0.05）
fn apply_emotion_hint(psy: &PsychologyManager, hint: &str, char_id: &str) {
    let hint_lower = hint.trim().to_lowercase();
    let delta = match hint_lower.as_str() {
        "happy" | "joy" | "excited" => crate::psychology::EmotionDeltas {
            joy: 0.05,
            ..Default::default()
        },
        "sad" | "depressed" => crate::psychology::EmotionDeltas {
            sadness: 0.05,
            ..Default::default()
        },
        "anxious" | "worried" | "nervous" => crate::psychology::EmotionDeltas {
            fear: 0.05,
            ..Default::default()
        },
        "angry" | "frustrated" => crate::psychology::EmotionDeltas {
            anger: 0.05,
            ..Default::default()
        },
        "lonely" | "isolated" => crate::psychology::EmotionDeltas {
            loneliness: 0.05,
            ..Default::default()
        },
        "curious" | "interested" => crate::psychology::EmotionDeltas {
            curiosity: 0.05,
            ..Default::default()
        },
        _ => return,
    };

    let output = crate::psychology::PsychologyOutput {
        appraisal: None,
        emotion_update: Some(delta),
        behavior_drive: None,
        need_update: None,
    };
    psy.apply_llm_output(&output);
    tracing::debug!(
        "[AsyncReflection:{}] 情绪预反应已注入: {}",
        char_id,
        hint_lower
    );
}

/// 从 LLM 响应中解析 JSON（容忍 markdown 代码块包裹）
fn parse_json(text: &str) -> Option<AsyncReflectionResult> {
    let trimmed = text.trim();
    if let Ok(u) = serde_json::from_str::<AsyncReflectionResult>(trimmed) {
        return Some(u);
    }
    let cleaned = extract_json_object(trimmed)?;
    serde_json::from_str::<AsyncReflectionResult>(&cleaned).ok()
}

fn extract_json_object(text: &str) -> Option<String> {
    let cleaned = text
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let start = cleaned.find('{')?;
    let end = cleaned.rfind('}')?;
    if end > start {
        Some(cleaned[start..=end].to_string())
    } else {
        None
    }
}
