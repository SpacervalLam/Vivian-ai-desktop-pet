//! 在场状态后台任务 — Busy 知识采集 / Rest 记忆沉淀
//!
//! 设计：
//! - **Busy 知识采集**：进入 Busy 状态时 spawn 一个 `run_knowledge_acquisition` 任务。
//!   调 LLM（task_type="knowledge_acquisition"）让它读自己最近的记忆，决定要补什么
//!   时效信息 → 调 `WebSearcher::search` 搜索 → LLM 汇总成结构化知识 → 写入
//!   `MemoryManager::add_knowledge_document`。任务结束时调用 `presence.finish_task()`，
//!   若期间用户请求过唤醒（pending_exit_to_online），由 finish_task 自动切回 Online。
//!
//! - **Rest 记忆沉淀**：进入 Rest 状态时 spawn 一个 `run_memory_consolidation` 任务，
//!   直接调用 `ConsolidationPipeline::run(&memory)` 跑完整三阶段（Stage 1/2/3 自带
//!   条件门控，不满足时直接返回 None，不调 LLM，因此 Rest 期间多次触发是安全的）。
//!   同样在结束时调 `presence.finish_task()` 收尾。
//!
//! 所有任务都通过 `tokio::spawn` 异步执行，不阻塞 transition 调用方。
//! `PresenceManager::begin_task()` 在任务体最前面调用，确保 task_in_progress 标记
//! 在 transition 完成、状态已切到 Busy/Rest 之后再设。

use std::sync::Arc;

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::config::WebSearchConfig;
use crate::memory::manager::MemoryManager;
use crate::memory::pipeline::ConsolidationPipeline;
use crate::memory::types::MemoryType;
use crate::mind::{Mind, UserCognitionEngine};
use crate::network::web_context::WebSearcher;
use crate::presence::PresenceManager;
use crate::proactive::ProactiveOrchestrator;
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::state::AppState;
use crate::tools::builtin::share_link_tool::share_link_to_wechat;
use crate::types::response::ChatMessage;
use crate::world::WorldState;

/// 单次知识采集最多尝试的主题数（内化 + 分享合计）
const MAX_TOPICS_PER_ACQUISITION: usize = 3;
/// 每个主题的搜索结果上限
const SEARCH_RESULTS_PER_TOPIC: usize = 5;

/// 启动 Busy 知识采集任务
///
/// 由 transition(Busy) 在切到 Busy 状态时通过 task_spawner 钩子调用。
/// `begin_task()` 已在 transition 内同步调用（消除 race），本函数仅 spawn 异步任务体。
pub fn spawn_knowledge_acquisition(
    char_id: String,
    app: AppHandle,
    presence: Arc<PresenceManager>,
    router: Arc<ModelRouter>,
    memory: Arc<MemoryManager>,
    proactive: Arc<ProactiveOrchestrator>,
) {
    tokio::spawn(async move {
        let cancel = crate::utils::cancel_token::cancel_token();
        if cancel.is_cancelled() {
            return;
        }
        // 通知前端：开始知识采集
        let _ = app.emit(
            "presence:task_started",
            json!({
                "character_id": &char_id,
                "task": "knowledge_acquisition",
            }),
        );

        // 从 AppState 读取 web_search 配置（按 provider 切换后端）
        let web_search_config = app
            .state::<std::sync::Arc<AppState>>()
            .config
            .read()
            .get_all()
            .web_search;

        // 检查开关：用户可在配置中关闭 Busy 状态知识采集
        if !web_search_config.enable_background_knowledge_fetch {
            tracing::info!("[Presence:{}] Busy 知识采集已禁用（enable_background_knowledge_fetch=false），跳过", char_id);
            // 任务结束：同样处理延迟退出
            if let Some(event) = presence.finish_task(&Default::default()) {
                emit_presence_changed(&app, &char_id, &event);
                write_presence_log(&memory, &char_id, &event).await;
            }
            let _ = app.emit(
                "presence:task_finished",
                json!({
                    "character_id": &char_id,
                    "task": "knowledge_acquisition",
                    "summary": "已禁用",
                    "topics": Vec::<String>::new(),
                    "acquired": 0,
                }),
            );
            return;
        }

        // 采集任务级冷却：距上次采集不足 30 分钟则跳过，避免每次 Busy 都触发采集
        if proactive.is_knowledge_acquisition_in_cooldown() {
            tracing::info!("[Presence:{}] Busy 知识采集冷却中（距上次不足 30 分钟），跳过", char_id);
            if let Some(event) = presence.finish_task(&Default::default()) {
                emit_presence_changed(&app, &char_id, &event);
                write_presence_log(&memory, &char_id, &event).await;
            }
            let _ = app.emit(
                "presence:task_finished",
                json!({
                    "character_id": &char_id,
                    "task": "knowledge_acquisition",
                    "summary": "冷却中",
                    "topics": Vec::<String>::new(),
                    "acquired": 0,
                }),
            );
            return;
        }

        // 关键 LLM 调用前再次检查取消信号
        if cancel.is_cancelled() {
            return;
        }
        let result = run_knowledge_acquisition(&char_id, &router, &memory, &web_search_config, &app, &proactive).await;

        // 注：want_to_share_knowledge 播种已由 run_knowledge_acquisition 内部完成
        // （仅对内化类主题播种，分享类已直接发给用户）

        // 任务结束：若期间用户请求过唤醒，finish_task 会自动 transition(Online)
        // 并返回 PresenceEvent，由我们这里负责写记忆 + emit presence:changed
        if let Some(event) = presence.finish_task(&Default::default()) {
            emit_presence_changed(&app, &char_id, &event);
            write_presence_log(&memory, &char_id, &event).await;
        }

        // 通知前端：知识采集结束（携带采集摘要）
        let _ = app.emit(
            "presence:task_finished",
            json!({
                "character_id": &char_id,
                "task": "knowledge_acquisition",
                "summary": result.summary,
                "topics": result.topics,
                "acquired": result.acquired_count,
                "shared": result.shared_count,
            }),
        );
    });
}

/// 启动 Rest 记忆沉淀任务
///
/// 由 transition(Rest) 在切到 Rest 状态时通过 task_spawner 钩子调用。
/// `begin_task()` 已在 transition 内同步调用，本函数仅 spawn 异步任务体。
pub fn spawn_memory_consolidation(
    char_id: String,
    app: AppHandle,
    presence: Arc<PresenceManager>,
    pipeline: Arc<ConsolidationPipeline>,
    memory: Arc<MemoryManager>,
) {
    tokio::spawn(async move {
        let cancel = crate::utils::cancel_token::cancel_token();
        if cancel.is_cancelled() {
            return;
        }
        let _ = app.emit(
            "presence:task_started",
            json!({
                "character_id": &char_id,
                "task": "memory_consolidation",
            }),
        );

        let mut report = crate::memory::pipeline::ConsolidationReport::default();
        // 直接调 pipeline.run，跳过 MemoryConsolidator 的 6h 冷却
        // Stage 1/2/3 内部各自带条件门控，不满足则返回 0 / None，不调 LLM
        // 关键 LLM 调用前再次检查取消信号
        if cancel.is_cancelled() {
            return;
        }
        match pipeline.run(&memory).await {
            Ok(r) => report = r,
            Err(e) => {
                tracing::warn!("[Presence:{}] Rest 记忆沉淀流水线失败: {}", char_id, e);
            }
        }

        // 任务结束：同样处理延迟退出
        if let Some(event) = presence.finish_task(&Default::default()) {
            emit_presence_changed(&app, &char_id, &event);
            write_presence_log(&memory, &char_id, &event).await;
        }

        let _ = app.emit(
            "presence:task_finished",
            json!({
                "character_id": &char_id,
                "task": "memory_consolidation",
                "stage1_summaries": report.stage1_summaries,
                "stage2_facts": report.stage2_facts,
                "stage3_insights": report.stage3_insights,
            }),
        );
    });
}

/// 启动 Rest 用户认知整理任务
///
/// 在记忆沉淀之后串行调用：从行为日志（UserBehaviorLog）提炼出用户习惯 Belief
/// （带 metric/value/match_labels），写入 Mind 的 BeliefStore。
///
/// 与 `spawn_memory_consolidation` 的关系：
/// - 记忆沉淀：把对话/事件压缩为 LongTerm/Insight/Fact（关于"发生过什么"）
/// - 用户认知整理：从行为时序提炼习惯 Belief（关于"用户是什么样的人"）
///
/// 两者数据源不同、目的不同，故独立任务，避免相互阻塞。
pub fn spawn_user_cognition_consolidation(
    char_id: String,
    app: AppHandle,
    router: Arc<ModelRouter>,
    mind: Arc<Mind>,
    world_state: Arc<WorldState>,
    recent_n: usize,
) {
    tokio::spawn(async move {
        let cancel = crate::utils::cancel_token::cancel_token();
        if cancel.is_cancelled() {
            return;
        }
        let _ = app.emit(
            "presence:task_started",
            json!({
                "character_id": &char_id,
                "task": "user_cognition_consolidation",
            }),
        );

        let engine = UserCognitionEngine::new(router);
        let behavior_log = world_state.behavior_log();
        // 关键 LLM 调用前再次检查取消信号
        if cancel.is_cancelled() {
            return;
        }
        let report = engine
            .consolidate_behaviors_to_beliefs(&behavior_log, &mind, recent_n)
            .await;

        let payload = match report {
            Ok(r) => json!({
                "character_id": &char_id,
                "task": "user_cognition_consolidation",
                "raw_count": r.raw_count,
                "beliefs_created": r.beliefs_created,
                "beliefs_reinforced": r.beliefs_reinforced,
            }),
            Err(e) => {
                tracing::warn!(
                    "[Presence:{}] 用户认知整理失败: {}",
                    char_id,
                    e
                );
                json!({
                    "character_id": &char_id,
                    "task": "user_cognition_consolidation",
                    "error": e.to_string(),
                })
            }
        };

        let _ = app.emit("presence:task_finished", payload);
    });
}

// ============================================================================
// 知识采集实现
// ============================================================================

/// 主题意图：内化为知识 vs 分享给用户
///
/// 知识采集分两类（用户明确区分）：
/// - Internalize：智能体内化知识，存入自己的知识库，不需要 URL，
///   为以后的回复提供材料（像人记住看到的内容，不一定马上分享）
/// - Share：想要分享给用户的链接，包装好后通过微信面板立即发送
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopicIntent {
    Internalize,
    Share,
}

struct TopicWithIntent {
    topic: String,
    intent: TopicIntent,
}

/// 单次知识采集结果
struct AcquisitionResult {
    summary: String,
    topics: Vec<String>,
    /// 内化入库数
    acquired_count: usize,
    /// 通过微信面板分享的链接数
    shared_count: usize,
}

/// 知识采集主流程
///
/// 区分两类主题：
/// - internalize：搜索 → LLM 总结成知识文档（不含 URL） → 写入知识库
/// - share：搜索 → 选最佳链接 → LLM 生成 follow_up → 通过微信面板立即发送
///
/// 分享类直接走微信面板，在记忆图谱中作为 wechat 节点（信封图标）出现；
async fn run_knowledge_acquisition(
    char_id: &str,
    router: &ModelRouter,
    memory: &MemoryManager,
    web_search_config: &WebSearchConfig,
    app: &AppHandle,
    proactive: &ProactiveOrchestrator,
) -> AcquisitionResult {
    // Step -1: 扫描已过 TTL 的知识文档，提取主题用于刷新（内化类）
    let refresh_topics = collect_expired_knowledge_topics(memory).await;
    if !refresh_topics.is_empty() {
        tracing::info!(
            "[Presence:{}] 知识采集：发现 {} 条过期知识文档，将刷新: {:?}",
            char_id,
            refresh_topics.len(),
            &refresh_topics
        );
    }

    // Step 0: 优先消费对话中 web_search 留下的主题提示（默认走内化）
    let hint_topics = memory.drain_topic_hints();
    if !hint_topics.is_empty() {
        tracing::info!(
            "[Presence:{}] 知识采集：从对话搜索提示中获取 {} 个优先主题: {:?}",
            char_id,
            hint_topics.len(),
            &hint_topics
        );
    }

    // Step 1: 让 LLM 决定要查什么 + 标注意图（内化/分享）
    let llm_topics = if hint_topics.len() < MAX_TOPICS_PER_ACQUISITION {
        match decide_topics_with_intent(router, memory, char_id).await {
            Ok(t) if !t.is_empty() => t,
            Ok(_) => {
                tracing::info!("[Presence:{}] 知识采集：LLM 决定本次无主题", char_id);
                Vec::new()
            }
            Err(e) => {
                tracing::warn!("[Presence:{}] 知识采集决定主题失败: {}", char_id, e);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    // 合并主题列表（刷新主题与提示主题默认内化，LLM 主题带意图标注）
    let mut topics: Vec<TopicWithIntent> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // 刷新主题 + 提示主题：均为内化
    for t in refresh_topics.iter().chain(hint_topics.iter()) {
        let key = t.to_lowercase();
        if seen.insert(key) {
            topics.push(TopicWithIntent {
                topic: t.clone(),
                intent: TopicIntent::Internalize,
            });
        }
        if topics.len() >= MAX_TOPICS_PER_ACQUISITION {
            break;
        }
    }
    // LLM 主题：带意图标注
    for tw in llm_topics {
        let key = tw.topic.to_lowercase();
        if seen.insert(key) {
            topics.push(tw);
        }
        if topics.len() >= MAX_TOPICS_PER_ACQUISITION {
            break;
        }
    }

    if topics.is_empty() {
        return AcquisitionResult {
            summary: "本次没有需要补充的知识".to_string(),
            topics: vec![],
            acquired_count: 0,
            shared_count: 0,
        };
    }

    let mut acquired = 0usize;
    let mut shared = 0usize;
    let mut topic_summaries: Vec<String> = Vec::new();
    let mut share_summaries: Vec<String> = Vec::new();

    for tw in topics.iter().take(MAX_TOPICS_PER_ACQUISITION) {
        // Step 2: 搜索（按 web_search 配置选择 provider）
        let results = WebSearcher::search_with_config(
            &tw.topic,
            SEARCH_RESULTS_PER_TOPIC,
            Some(web_search_config),
        )
        .await;
        if results.is_empty() {
            tracing::info!("[Presence:{}] 主题「{}」无搜索结果，跳过", char_id, tw.topic);
            continue;
        }

        match tw.intent {
            TopicIntent::Internalize => {
                // 内化路径：搜索 → LLM 总结成知识文档（不含 URL） → 写入知识库
                match summarize_search_results(router, &tw.topic, &results).await {
                    Ok((title, content, ttl_days)) if !content.is_empty() => {
                        let tags = vec![
                            "auto_acquired".to_string(),
                            "busy_task".to_string(),
                            tw.topic.clone(),
                        ];
                        match memory
                            .add_knowledge_document(&title, &content, tags, "web", Some(ttl_days))
                            .await
                        {
                            Ok(_) => {
                                acquired += 1;
                                topic_summaries.push(format!("「{}」", title));
                                tracing::info!(
                                    "[Presence:{}] 知识入库：{}（来源 {} 条搜索结果，TTL={}天）",
                                    char_id,
                                    title,
                                    results.len(),
                                    if ttl_days < 0 {
                                        "永不过期".to_string()
                                    } else {
                                        ttl_days.to_string()
                                    }
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "[Presence:{}] 主题「{}」知识入库失败: {}",
                                    char_id,
                                    tw.topic,
                                    e
                                );
                            }
                        }
                    }
                    Ok(_) => {
                        tracing::info!(
                            "[Presence:{}] 主题「{}」LLM 总结为空，跳过",
                            char_id,
                            tw.topic
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[Presence:{}] 主题「{}」LLM 总结失败: {}",
                            char_id,
                            tw.topic,
                            e
                        );
                    }
                }
            }
            TopicIntent::Share => {
                // 分享冷却：30 分钟内已分享过则跳过，避免频繁推送链接给用户
                if proactive.is_knowledge_share_in_cooldown() {
                    tracing::info!(
                        "[Presence:{}] 知识分享冷却中，主题「{}」的链接分享推迟",
                        char_id, tw.topic
                    );
                    continue;
                }
                // 分享路径：搜索 → LLM 选最佳链接 + 生成 follow_up → 立即通过微信面板发送
                match prepare_share_payload(router, &tw.topic, &results).await {
                    Ok(payload) => {
                        share_link_to_wechat(
                            app,
                            char_id,
                            &payload.url,
                            &payload.title,
                            &payload.description,
                            &payload.source,
                            &payload.follow_up,
                        )
                        .await;
                        proactive.mark_knowledge_share_expressed();
                        shared += 1;
                        share_summaries.push(format!("「{}」", payload.title));
                        tracing::info!(
                            "[Presence:{}] 链接已通过微信面板分享：{}（url={}）",
                            char_id,
                            payload.title,
                            payload.url
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[Presence:{}] 主题「{}」分享链接准备失败: {}",
                            char_id,
                            tw.topic,
                            e
                        );
                    }
                }
            }
        }
    }

    // 通知 ProactiveOrchestrator：仅内化类主题播种 want_to_share_knowledge
    // （分享类已直接发给用户，不需要再触发主动分享）
    let internalized_topics: Vec<String> = topics
        .iter()
        .filter(|t| t.intent == TopicIntent::Internalize)
        .map(|t| t.topic.clone())
        .collect();
    if acquired > 0 && !internalized_topics.is_empty() {
        // 注：此处仍通知 ProactiveOrchestrator 播种 want_to_share_knowledge，
        // 让 LLM 在后续 Spontaneous 触发器中可能主动提到这些知识（口语化提及，非链接卡片）。
        proactive.signal_knowledge_acquired(internalized_topics);
    }

    let mut parts: Vec<String> = Vec::new();
    if acquired > 0 {
        if acquired == 1 {
            parts.push(format!("补充了 1 条新知识：{}", topic_summaries.join("")));
        } else {
            parts.push(format!(
                "补充了 {} 条新知识：{}",
                acquired,
                topic_summaries.join("、")
            ));
        }
    }
    if shared > 0 {
        if shared == 1 {
            parts.push(format!("分享了 1 条链接：{}", share_summaries.join("")));
        } else {
            parts.push(format!(
                "分享了 {} 条链接：{}",
                shared,
                share_summaries.join("、")
            ));
        }
    }
    let summary = if parts.is_empty() {
        "本次知识采集未入库新内容".to_string()
    } else {
        parts.join("；")
    };

    AcquisitionResult {
        summary,
        topics: topics.into_iter().map(|t| t.topic).collect(),
        acquired_count: acquired,
        shared_count: shared,
    }
}

/// 分享链接 payload
struct SharePayload {
    url: String,
    title: String,
    description: String,
    source: String,
    follow_up: String,
}

/// 让 LLM 根据最近话题总结+近期记忆决定要查询的主题列表（带意图标注）
///
/// 设计原则（用户反馈"忙碌状态知识采集太机械"）：
/// - 数据源用记忆系统已提炼的话题总结（SessionSummary）+ 最近5条短期记忆
///   单条对话消息太短/太口语化，不能稳定代表兴趣；SessionSummary 是 LLM 提炼过的话题级压缩，
///   是用户兴趣更稳定的锚点
/// - 不是每次 Busy 都需要采集：如果最近话题和记忆都缺乏明确兴趣锚点，LLM 可返回 `[none]` 表示本次不采集
/// - share 必须克制：要求 LLM 在 [share] 标签后附加理由（如 `[share:用户刚提到X]`），无理由自动降级为 internalize
/// - 一次最多 1 个 share，避免给用户连续推送链接
///
/// LLM 输出格式（每行一个）：
/// - `[internalize] 关键词` 或 `[i] 关键词`：内化为知识（默认）
/// - `[share:理由] 关键词`：分享链接，必须带理由
/// - `[none]`：本次无明确兴趣锚点，不采集
async fn decide_topics_with_intent(
    router: &ModelRouter,
    memory: &MemoryManager,
    char_id: &str,
) -> Result<Vec<TopicWithIntent>, String> {
    // 数据源 1：最近话题总结（SessionSummary，Stage 1 把多轮 ShortTerm 摘要成的话题级压缩）
    // 这是用户兴趣最稳定的锚点——已经是 LLM 提炼过的话题，比单条对话消息更可靠
    let recent_topics = memory.recent_by_type(MemoryType::SessionSummary, 3);
    let topic_block = if recent_topics.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> = recent_topics.iter()
            .filter_map(|m| {
                let content = m.content.trim();
                if content.is_empty() { return None; }
                Some(format!("- {}", content.chars().take(150).collect::<String>()))
            })
            .collect();
        if lines.is_empty() {
            String::new()
        } else {
            format!("## 最近话题总结（近期与用户聊过的核心话题，搜索兴趣的首要锚点）\n{}", lines.join("\n"))
        }
    };

    // 数据源 2：最近5条短期记忆（含对话/事件，补充话题总结之外的近期上下文）
    let recent_memories = memory.recent_by_tags(&["short_term", "casual_conversation"], 5);
    let memory_block = if recent_memories.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> = recent_memories.iter()
            .filter_map(|m| {
                let content = m.content.trim();
                if content.is_empty() { return None; }
                Some(format!("- {}", content.chars().take(120).collect::<String>()))
            })
            .collect();
        if lines.is_empty() {
            String::new()
        } else {
            format!("## 最近记忆（最近5条短期记忆，提供近期上下文）\n{}", lines.join("\n"))
        }
    };

    // 拼接上下文
    let context_block = if topic_block.is_empty() && memory_block.is_empty() {
        "（最近没有话题总结和记忆——你可以选择休息不采集，或根据自己的兴趣自由发挥）".to_string()
    } else {
        let mut parts = Vec::new();
        if !topic_block.is_empty() { parts.push(topic_block); }
        if !memory_block.is_empty() { parts.push(memory_block); }
        parts.join("\n\n")
    };

    let system = format!(
        "你是智能体角色 {}，现在有 Busy 空闲时间，可以主动学习一些新知识或发现值得分享给用户的链接。\n\n\
         ## 核心原则：不是每次 Busy 都需要采集\n\
         如果最近话题总结中没有明确的话题锚点，且记忆中也缺乏明确的兴趣点，\
         请直接返回一行 `[none]`，本次不采集。这是合理的——像人一样，没事做的时候不必硬找事做。\n\n\
         ## 话题来源优先级\n\
         1. 优先围绕「最近话题总结」中提到的具体话题延伸——如果用户近期聊过某个具体话题（如某部番剧/某项技术/某个新闻），可以深入搜索相关知识\n\
         2. 其次参考「最近记忆」中的上下文补充\n\
         3. 都没有明确锚点时，返回 `[none]`，不要硬编造话题\n\n\
         ## 分享（share）必须克制\n\
         - 分享是少数情况：仅当搜索结果能让用户「立即想看」时才分享\n\
         - 必须带理由前缀：`[share:用户刚提到X]` 或 `[share:相关历史兴趣]`\n\
         - 没有理由前缀的 share 自动按 internalize 处理\n\
         - 一次最多 1 个 share，避免给用户连续推送链接\n\n\
         ## 内化（internalize）是常态\n\
         你平时上网看到的内容大部分是自己记住的，不一定立刻分享给别人。\n\
         - 内化：技术趋势、百科知识、长期背景知识、用户提到过但你了解不深的话题\n\
         - 分享：有趣的文章、实用工具、新闻热点、用户可能立即想看的内容\n\n\
         ## 输出格式\n\
         每行一个主题，带前缀：\n\
         - `[internalize] 关键词` 或 `[i] 关键词`\n\
         - `[share:理由] 关键词`（注意 share 必须带冒号+理由）\n\
         - 或单独一行 `[none]` 表示本次不采集\n\n\
         ## 数量\n\
         - 1-2 个为宜，最多 3 个\n\
         - 关键词应适合搜索引擎查询，不要写成完整问句\n\
         - 不要输出多余解释、序号或 markdown 标记",
        char_id
    );

    let user = format!("{}\n\n请决定你这次想查询的话题，或返回 `[none]` 表示本次不采集。", context_block);

    let messages = vec![
        ChatMessage::system(&system),
        ChatMessage::user(&user),
    ];

    let resp = router
        .generate(LLMRequest::new("knowledge_acquisition", messages))
        .await
        .map_err(|e| e.to_string())?;

    // 检测 [none] 信号：LLM 主动表示本次无采集意愿
    let trimmed_resp = resp.trim();
    if trimmed_resp == "[none]" || trimmed_resp.lines().any(|l| l.trim() == "[none]") {
        tracing::info!("[Presence:{}] 知识采集：LLM 判定本次无明确兴趣锚点，主动跳过", char_id);
        return Ok(Vec::new());
    }

    // 按行解析，提取前缀标注的意图
    let mut topics: Vec<TopicWithIntent> = Vec::new();
    let mut share_count = 0;
    for line in resp.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // 解析前缀 [internalize]/[i]/[share:理由]/[share]/[s]
        let (intent, rest) = if let Some(rest) = trimmed.strip_prefix("[internalize]") {
            (TopicIntent::Internalize, rest.trim())
        } else if let Some(rest) = trimmed.strip_prefix("[i]") {
            (TopicIntent::Internalize, rest.trim())
        } else if let Some(rest) = trimmed.strip_prefix("[share:") {
            // [share:理由] 关键词 —— 提取理由后剩余部分
            // rest 形如 "理由] 关键词"
            if let Some(close_pos) = rest.find(']') {
                let keyword = rest[close_pos + 1..].trim();
                (TopicIntent::Share, keyword)
            } else {
                // 格式异常，降级为 internalize
                (TopicIntent::Internalize, rest.trim())
            }
        } else if let Some(rest) = trimmed.strip_prefix("[share]") {
            // 无理由的 [share] —— 降级为 internalize（用户反馈：分享必须克制）
            tracing::info!("[Presence:{}] 知识采集：LLM 标注了无理由 [share]，降级为 internalize", char_id);
            (TopicIntent::Internalize, rest.trim())
        } else if let Some(rest) = trimmed.strip_prefix("[s]") {
            // 无理由的 [s] —— 同样降级
            tracing::info!("[Presence:{}] 知识采集：LLM 标注了无理由 [s]，降级为 internalize", char_id);
            (TopicIntent::Internalize, rest.trim())
        } else {
            // 无前缀默认内化
            (TopicIntent::Internalize, trimmed)
        };

        // 去掉行首序号/符号
        let topic = rest
            .trim_start_matches(|c: char| c.is_numeric() || c == '.' || c == '、' || c == '-' || c == '*')
            .trim()
            .to_string();
        if topic.is_empty() || topic.len() > 80 {
            continue;
        }

        // share 数量限制：一次最多 1 个 share，多余的降级为 internalize
        let final_intent = if intent == TopicIntent::Share {
            if share_count >= 1 {
                tracing::info!("[Presence:{}] 知识采集：LLM 标注了多个 share，第 {} 个降级为 internalize", char_id, share_count + 1);
                TopicIntent::Internalize
            } else {
                share_count += 1;
                TopicIntent::Share
            }
        } else {
            TopicIntent::Internalize
        };

        topics.push(TopicWithIntent { topic, intent: final_intent });

        if topics.len() >= MAX_TOPICS_PER_ACQUISITION {
            break;
        }
    }

    Ok(topics)
}

/// 让 LLM 从搜索结果中挑选最佳链接并生成 follow_up 评论
///
/// 与 `summarize_search_results` 的区别：
/// - summarize_search_results：把多条搜索结果融合成知识文档（不含 URL）
/// - prepare_share_payload：选一条最佳链接 + 生成分享给用户的 follow_up
async fn prepare_share_payload(
    router: &ModelRouter,
    topic: &str,
    results: &[crate::network::web_context::SearchResult],
) -> Result<SharePayload, String> {
    let context: String = results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "【{}】\n标题: {}\n摘要: {}\nURL: {}",
                i + 1,
                r.title,
                r.snippet,
                r.url
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let system = "你是分享链接助手。从用户给定的搜索结果中挑选一条最值得分享给用户的链接，并生成一句简短的 follow_up 评论。\n\n\
        要求：\n\
        - 第一行输出选中条目的 URL（必须来自搜索结果中实际存在的 URL）\n\
        - 第二行输出标题（不超过 30 字，可基于搜索结果标题改写）\n\
        - 第三行输出简短描述（1-2 句话，80 字以内）\n\
        - 第四行输出来源名称或域名（如 'Bilibili'、'知乎'、'GitHub'）\n\
        - 第五行输出 follow_up 评论（1-2 句自然口语化评论，如 '这个讲得挺清楚的'、'感觉这篇分析很到位'）\n\n\
        - 不要输出多余解释、markdown 标记或前缀标签\n\
        - 不要输出多条链接，只挑最值得分享的一条";

    let user = format!(
        "话题：{}\n\n搜索结果：\n{}\n\n请挑选一条最值得分享的链接并生成 follow_up。",
        topic, context
    );

    let messages = vec![
        ChatMessage::system(system),
        ChatMessage::user(&user),
    ];

    let resp = router
        .generate(LLMRequest::new("knowledge_acquisition", messages))
        .await
        .map_err(|e| e.to_string())?;

    // 按行解析
    let mut lines = resp.lines();
    let url = lines.next().unwrap_or("").trim().to_string();
    let title = lines.next().unwrap_or("").trim().to_string();
    let description = lines.next().unwrap_or("").trim().to_string();
    let source = lines.next().unwrap_or("").trim().to_string();
    let follow_up = lines
        .next()
        .unwrap_or("")
        .trim()
        .to_string();

    // 校验 URL 必须来自搜索结果（防止 LLM 编造）
    let url_valid = results.iter().any(|r| r.url == url);
    if !url_valid || url.is_empty() {
        return Err("LLM 返回的 URL 不在搜索结果中".to_string());
    }
    if title.is_empty() || follow_up.is_empty() {
        return Err("LLM 返回的标题或 follow_up 为空".to_string());
    }

    Ok(SharePayload {
        url,
        title,
        description,
        source,
        follow_up,
    })
}

/// 让 LLM 把搜索结果总结成结构化知识文档
///
/// 返回 (标题, 正文, ttl_days)
/// - ttl_days: 知识时效天数（7=短期热点, 30=中期趋势, -1=长期知识/不过期）
async fn summarize_search_results(
    router: &ModelRouter,
    topic: &str,
    results: &[crate::network::web_context::SearchResult],
) -> Result<(String, String, i64), String> {
    let context: String = results
        .iter()
        .enumerate()
        .map(|(i, r)| {
            format!(
                "【{}】\n标题: {}\n摘要: {}\nURL: {}",
                i + 1,
                r.title,
                r.snippet,
                r.url
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    let system = "你是知识整理助手。根据用户给定的搜索结果，整理出一份结构化、可长期保存的知识文档。要求：\n\
        - 第一行输出标题（不超过 30 字，不带「标题:」前缀）\n\
        - 空一行\n\
        - 接下来输出正文（200-600 字），按要点分段，融合多条搜索结果的信息\n\
        - 不要列出 URL，不要重复搜索结果原文\n\
        - 客观陈述事实，避免主观评价\n\
        - 最后一行单独输出时效标签，三选一：[short]（短期热点，7天过期）、[mid]（中期趋势，30天过期）、[long]（长期知识，不过期）\n\
        - 判断依据：新闻/热搜/股价/赛事 → short；技术趋势/产品动态/季节性话题 → mid；百科知识/历史/科学原理 → long";

    let user = format!(
        "话题：{}\n\n搜索结果：\n{}\n\n请整理成知识文档。",
        topic, context
    );

    let messages = vec![
        ChatMessage::system(system),
        ChatMessage::user(&user),
    ];

    let resp = router
        .generate(LLMRequest::new("knowledge_acquisition", messages))
        .await
        .map_err(|e| e.to_string())?;

    // 从最后一行提取时效标签
    let ttl_days = extract_ttl_from_response(&resp);

    // 去掉末尾的时效标签行后再解析标题和正文
    let resp_clean = strip_ttl_line(&resp);

    // 第一行作为标题，其余作为正文
    let mut lines = resp_clean.lines();
    let title = lines
        .next()
        .unwrap_or(topic)
        .trim()
        .trim_start_matches("标题：")
        .trim_start_matches("标题:")
        .trim()
        .to_string();
    let title = if title.is_empty() {
        format!("关于「{}」", topic)
    } else {
        title
    };

    let content: String = lines
        .skip_while(|s| s.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    if content.len() < 20 {
        return Ok((title, String::new(), ttl_days));
    }

    Ok((title, content, ttl_days))
}

/// 从 LLM 响应中提取时效标签，返回 TTL 天数
fn extract_ttl_from_response(resp: &str) -> i64 {
    let last_line = resp.lines().last().unwrap_or("").trim();
    let lower = last_line.to_lowercase();
    if lower.contains("[short]") {
        7
    } else if lower.contains("[mid]") {
        30
    } else if lower.contains("[long]") {
        -1
    } else {
        // 未识别到标签时默认 30 天
        30
    }
}

/// 去掉 LLM 响应末尾的时效标签行
fn strip_ttl_line(resp: &str) -> String {
    let lines: Vec<&str> = resp.lines().collect();
    if lines.is_empty() {
        return resp.to_string();
    }
    let last = lines.last().unwrap().trim().to_lowercase();
    if last.contains("[short]") || last.contains("[mid]") || last.contains("[long]") {
        lines[..lines.len() - 1].join("\n")
    } else {
        resp.to_string()
    }
}

/// 扫描已过 TTL 的知识文档，删除旧文档并返回其主题用于刷新。
///
/// 过期判定：metadata.expires_at 存在且 <= 当前时间。
/// 删除旧文档后返回其 metadata.title 作为刷新主题。
async fn collect_expired_knowledge_topics(memory: &MemoryManager) -> Vec<String> {
    let docs = match memory.list_knowledge_documents().await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("[KnowledgeRefresh] 列出知识文档失败: {}", e);
            return Vec::new();
        }
    };

    let now = chrono::Utc::now().timestamp() as f64;
    let mut topics = Vec::new();

    for doc in docs {
        let is_expired = doc
            .metadata
            .get("expires_at")
            .and_then(|v| v.as_f64())
            .map(|expires_at| now >= expires_at)
            .unwrap_or(false);

        if !is_expired {
            continue;
        }

        // 提取标题作为刷新主题
        let title = doc
            .metadata
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or(&doc.content)
            .to_string();

        // 删除旧文档（含向量索引）
        match memory.delete_knowledge_document(&doc.id).await {
            Ok(true) => {
                tracing::info!(
                    "[KnowledgeRefresh] 过期知识已删除，将刷新: {}",
                    title
                );
                topics.push(title);
            }
            Ok(false) => {
                tracing::warn!(
                    "[KnowledgeRefresh] 过期知识未找到，可能已被删除: {}",
                    title
                );
            }
            Err(e) => {
                tracing::warn!(
                    "[KnowledgeRefresh] 删除过期知识失败: {} - {}",
                    title,
                    e
                );
            }
        }
    }

    topics
}

// ============================================================================
// 共享工具
// ============================================================================

/// emit presence:changed 事件给前端
fn emit_presence_changed(
    app: &AppHandle,
    char_id: &str,
    event: &crate::presence::PresenceEvent,
) {
    let _ = app.emit(
        "presence:changed",
        json!({
            "character_id": char_id,
            "from": event.from,
            "to": event.to,
            "reason": event.reason,
        }),
    );
}

/// 写入 presence_log 行为日志记忆（fire-and-forget）
async fn write_presence_log(
    memory: &MemoryManager,
    char_id: &str,
    event: &crate::presence::PresenceEvent,
) {
    // 复用 PresenceManager 的 memory_text 逻辑：from→to + reason
    let text = format!(
        "（在场状态从{}变为{}，原因：{}）",
        event.from, event.to, event.reason
    );
    let meta = serde_json::json!({
        "channel": "presence",
        "speaker": char_id,
        "listener": char_id,
        "perspective": "speaker",
    });
    let _ = memory
        .add_memory_with_metadata(
            &text,
            MemoryType::ShortTerm,
            0.4,
            vec!["presence_log".to_string(), "assistant".to_string()],
            meta,
        )
        .await;
}
