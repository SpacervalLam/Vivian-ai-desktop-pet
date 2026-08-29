//! SNS 热梗定期采集 —— 保持角色"懂梗玩梗"人设
//!
//! 独立于 Busy 状态的知识采集，按固定周期（默认 7 天）主动采集 B 站、抖音、
//! 小红书、微博等 SNS 平台的最新热梗，写入 Knowledge 记忆，TTL=7 天自动刷新。
//!
//! 设计：
//! - **独立 tokio task**：在 lib.rs 启动时为每个角色 spawn 一个循环 task，
//!   不依赖 Presence 状态（Online/Busy/Rest 均可运行，Offline 跳过）
//! - **角色差异化**：Vivian 侧重 B 站/抖音二次元梗，Nana 侧重小红书/微博生活类热词
//! - **LLM 全生成关键词**：每周让 LLM 基于当前日期生成当周可能的热梗候选词
//! - **平台定向搜索**：通过 query 拼接 `site:bilibili.com` / `抖音` / `小红书` 修饰
//! - **滚动周期**：从上次采集完成时刻起算 7 天后再次触发
//! - **持久化冷却**：`characters/<char_id>/meme_acquisition_state.json` 记录上次采集时间，
//!   重启后若距上次 ≥ 7 天则立即触发（延迟 10 分钟避免启动期争抢）

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::config::WebSearchConfig;
use crate::memory::manager::MemoryManager;
use crate::network::web::{WebSearchRequest, WebSearchService, WebSearchSource};
use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::state::AppState;
use crate::types::response::ChatMessage;

/// 采集周期：7 天（秒）
const MEME_ACQUISITION_INTERVAL_SECS: f64 = 7.0 * 24.0 * 3600.0;
/// 启动后延迟（秒），避免启动期资源争抢
const STARTUP_DELAY_SECS: u64 = 10 * 60;
/// 每个平台搜索结果上限
const SEARCH_RESULTS_PER_PLATFORM: usize = 6;
/// 单次采集最多覆盖的平台数
const MAX_PLATFORMS_PER_RUN: usize = 2;
/// LLM 生成的关键词上限
const MAX_KEYWORDS_PER_RUN: usize = 4;

/// 角色差异化平台配置
struct PlatformConfig {
    /// 平台标识
    name: &'static str,
    /// 搜索 query 修饰（拼接在关键词前/后）
    query_modifier: &'static str,
    /// 平台侧重描述（供 LLM 生成关键词时参考）
    focus_hint: &'static str,
}

/// Vivian 侧重平台：B 站 + 抖音
fn vivian_platforms() -> Vec<PlatformConfig> {
    vec![
        PlatformConfig {
            name: "bilibili",
            query_modifier: "site:bilibili.com",
            focus_hint: "B站热门视频、二次元番剧梗、鬼畜、UP主热梗",
        },
        PlatformConfig {
            name: "douyin",
            query_modifier: "抖音",
            focus_hint: "抖音热门挑战、短视频梗、流行舞、热梗台词",
        },
    ]
}

/// Nana 侧重平台：小红书 + 微博
fn nana_platforms() -> Vec<PlatformConfig> {
    vec![
        PlatformConfig {
            name: "xiaohongshu",
            query_modifier: "小红书",
            focus_hint: "小红书热门话题、生活穿搭、美食、情感热词",
        },
        PlatformConfig {
            name: "weibo",
            query_modifier: "site:weibo.com 微博热搜",
            focus_hint: "微博热搜话题、社会热点、明星娱乐、流行语",
        },
    ]
}

/// 按角色 ID 获取平台配置
fn platforms_for(char_id: &str) -> Vec<PlatformConfig> {
    match char_id {
        "vivian" => vivian_platforms(),
        "nana" => nana_platforms(),
        _ => vivian_platforms(), // 默认走 Vivian 配置
    }
}

/// 采集状态持久化结构
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemeAcquisitionState {
    /// 上次采集完成时间戳（Unix 秒）
    last_acquisition_ts: f64,
}

impl Default for MemeAcquisitionState {
    fn default() -> Self {
        Self {
            last_acquisition_ts: 0.0,
        }
    }
}

/// 状态文件路径：`characters/<char_id>/meme_acquisition_state.json`
fn state_file_path(char_id: &str) -> std::path::PathBuf {
    crate::utils::path::get_character_data_dir(char_id).join("meme_acquisition_state.json")
}

/// 读取持久化状态
fn load_state(char_id: &str) -> MemeAcquisitionState {
    let path = state_file_path(char_id);
    crate::utils::fs::load_json_or_backup(&path).unwrap_or_default()
}

/// 写入持久化状态
fn save_state(char_id: &str, state: &MemeAcquisitionState) {
    let path = state_file_path(char_id);
    if let Ok(content) = serde_json::to_string_pretty(state) {
        if let Err(e) = std::fs::write(&path, content) {
            tracing::warn!("[MemeAcquisition:{}] 状态持久化失败: {}", char_id, e);
        }
    }
}

/// 启动热梗采集循环（每个角色一个独立 task）
///
/// 在 lib.rs 启动时调用。循环流程：
/// 1. 启动后延迟 10 分钟
/// 2. 读取持久化状态，若距上次采集 ≥ 7 天则立即触发
/// 3. 否则 sleep 到下次触发时间
/// 4. 执行采集 → 更新状态 → sleep 7 天 → 回到步骤 4
pub fn spawn_meme_acquisition_loop(
    char_id: String,
    app: AppHandle,
    router: Arc<ModelRouter>,
    memory: Arc<MemoryManager>,
) {
    tokio::spawn(async move {
        // 启动延迟，避免启动期资源争抢
        tokio::time::sleep(std::time::Duration::from_secs(STARTUP_DELAY_SECS)).await;

        tracing::info!("[MemeAcquisition:{}] 热梗采集循环已启动", char_id);

        // 看门狗注册：期望心跳间隔 = 分段 sleep 的检查周期（300s）
        let loop_name = format!("meme_acquisition:{}", char_id);
        crate::utils::watchdog::register(&loop_name, 300.0, None);

        loop {
            crate::utils::watchdog::beat(&loop_name);
            let cancel = crate::utils::cancel_token::cancel_token();
            if cancel.is_cancelled() {
                tracing::info!("[MemeAcquisition:{}] 取消信号已触发，退出循环", char_id);
                return;
            }

            // 读取状态判断是否需要立即触发
            let state = load_state(&char_id);
            let now = chrono::Local::now().timestamp() as f64;
            let elapsed = now - state.last_acquisition_ts;

            let wait_secs = if state.last_acquisition_ts <= 0.0 || elapsed >= MEME_ACQUISITION_INTERVAL_SECS {
                // 从未采集过或已过周期：立即触发
                0u64
            } else {
                // 计算剩余等待时间
                let remaining = MEME_ACQUISITION_INTERVAL_SECS - elapsed;
                remaining.max(0.0) as u64
            };

            if wait_secs > 0 {
                tracing::debug!(
                    "[MemeAcquisition:{}] 距下次采集还需 {} 秒（约 {:.1} 天）",
                    char_id,
                    wait_secs,
                    wait_secs as f64 / 86400.0
                );
                // 分段 sleep 以支持取消信号响应
                let mut remaining = wait_secs;
                while remaining > 0 {
                    let chunk = remaining.min(300); // 每 5 分钟检查一次取消信号
                    tokio::select! {
                        _ = cancel.cancelled() => {
                            tracing::info!("[MemeAcquisition:{}] 取消信号已触发，退出循环", char_id);
                            return;
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(chunk)) => {}
                    }
                    crate::utils::watchdog::beat(&loop_name);
                    remaining -= chunk;
                    if cancel.is_cancelled() {
                        return;
                    }
                }
            }

            // 再次检查取消信号（sleep 期间可能已取消）
            if cancel.is_cancelled() {
                return;
            }

            // 检查角色是否在线（Offline 跳过，避免离线角色浪费 API 配额）
            let is_offline = {
                let app_state = app.state::<std::sync::Arc<AppState>>();
                let characters = app_state.characters.read();
                characters
                    .get(&char_id)
                    .map(|c| !*c.online.read())
                    .unwrap_or(true)
            };
            if is_offline {
                tracing::info!("[MemeAcquisition:{}] 角色已离线，跳过本次采集", char_id);
                // 离线时延迟 1 小时后重试
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                continue;
            }

            // 读取 web_search 配置 + 代理 URL
            let (web_search_config, proxy_url) = {
                let cfg = app
                    .state::<std::sync::Arc<AppState>>()
                    .config
                    .read()
                    .get_all();
                let proxy_config = crate::network::proxy::ProxyConfig::from_app_config(&cfg);
                (cfg.web_search.clone(), proxy_config.effective_proxy_url())
            };

            // 检查开关
            if !web_search_config.enable_background_knowledge_fetch {
                tracing::info!(
                    "[MemeAcquisition:{}] 后台知识采集已禁用，跳过热梗采集",
                    char_id
                );
                // 延迟 1 小时后重试（避免紧密循环）
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                continue;
            }

            // 通知前端：开始热梗采集
            let _ = app.emit(
                "meme_acquisition:started",
                json!({ "character_id": &char_id }),
            );

            // 执行采集
            let result = run_meme_acquisition(
                &char_id,
                &router,
                &memory,
                &web_search_config,
                &proxy_url,
            )
            .await;

            // 更新持久化状态
            let now = chrono::Local::now().timestamp() as f64;
            save_state(
                &char_id,
                &MemeAcquisitionState {
                    last_acquisition_ts: now,
                },
            );

            // 通知前端：采集结束
            let _ = app.emit(
                "meme_acquisition:finished",
                json!({
                    "character_id": &char_id,
                    "acquired": result.acquired_count,
                    "summary": result.summary,
                }),
            );

            tracing::info!(
                "[MemeAcquisition:{}] 本次采集完成：入库 {} 条，摘要：{}",
                char_id,
                result.acquired_count,
                result.summary
            );

            // 采集完成后 sleep 7 天进入下一轮（由循环顶部的状态检查统一处理）
            // 这里不额外 sleep，直接回到循环顶部，顶部会读取状态计算等待时间
        }
    });
}

/// 单次采集结果
struct AcquisitionResult {
    summary: String,
    acquired_count: usize,
}

/// 单次热梗采集主流程
///
/// 1. LLM 生成当周热梗候选关键词
/// 2. 按角色平台配置拼接平台修饰搜索
/// 3. LLM 总结搜索结果成知识文档
/// 4. 写入 Knowledge 记忆（TTL=7 天，下周自动过期刷新）
async fn run_meme_acquisition(
    char_id: &str,
    router: &ModelRouter,
    memory: &MemoryManager,
    web_search_config: &WebSearchConfig,
    proxy_url: &Option<String>,
) -> AcquisitionResult {
    let platforms = platforms_for(char_id);

    // Step 1: LLM 生成当周热梗候选关键词
    let keywords = match generate_meme_keywords(router, char_id, &platforms).await {
        Ok(kw) if !kw.is_empty() => kw,
        Ok(_) => {
            tracing::info!("[MemeAcquisition:{}] LLM 未生成关键词，跳过本次采集", char_id);
            return AcquisitionResult {
                summary: "LLM 未生成关键词".to_string(),
                acquired_count: 0,
            };
        }
        Err(e) => {
            tracing::warn!("[MemeAcquisition:{}] 生成关键词失败: {}", char_id, e);
            return AcquisitionResult {
                summary: format!("生成关键词失败: {}", e),
                acquired_count: 0,
            };
        }
    };

    tracing::info!(
        "[MemeAcquisition:{}] LLM 生成 {} 个关键词: {:?}",
        char_id,
        keywords.len(),
        &keywords
    );

    let mut acquired = 0usize;
    let mut summaries: Vec<String> = Vec::new();

    // Step 2 + 3 + 4: 按平台搜索 + LLM 总结 + 入库
    for platform in platforms.iter().take(MAX_PLATFORMS_PER_RUN) {
        // 拼接平台修饰 + 关键词，组合成搜索 query
        // 每个平台用所有关键词拼成一次搜索（OR 连接），让搜索引擎返回任一关键词的结果
        let query = if keywords.len() == 1 {
            format!("{} {}", platform.query_modifier, keywords[0])
        } else {
            format!(
                "{} {}",
                platform.query_modifier,
                keywords.join(" OR ")
            )
        };

        tracing::info!(
            "[MemeAcquisition:{}] 搜索平台 {}: {:?}",
            char_id,
            platform.name,
            query
        );

        let request = WebSearchRequest::new(&query).with_max_results(SEARCH_RESULTS_PER_PLATFORM);
        let result = WebSearchService::shared()
            .search(&request, Some(&web_search_config), proxy_url.as_deref())
            .await;

        let results = match result {
            Ok(r) if !r.sources.is_empty() => r.sources,
            _ => {
                tracing::info!(
                    "[MemeAcquisition:{}] 平台 {} 无搜索结果，跳过",
                    char_id,
                    platform.name
                );
                continue;
            }
        };

        // LLM 总结搜索结果
        match summarize_meme_results(router, char_id, platform.name, &keywords, &results).await {
            Ok((title, content)) if !content.is_empty() => {
                let tags = vec![
                    "meme".to_string(),
                    "trending".to_string(),
                    "sns".to_string(),
                    platform.name.to_string(),
                ];
                match memory
                    .add_knowledge_document(
                        &title,
                        &content,
                        tags,
                        "meme_acquisition",
                        Some(7), // TTL=7 天，下周采集时自动过期
                    )
                    .await
                {
                    Ok(_) => {
                        acquired += 1;
                        summaries.push(format!("「{}」", title));
                        tracing::info!(
                            "[MemeAcquisition:{}] 平台 {} 热梗入库：{}（来源 {} 条搜索结果）",
                            char_id,
                            platform.name,
                            title,
                            results.len()
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[MemeAcquisition:{}] 平台 {} 热梗入库失败: {}",
                            char_id,
                            platform.name,
                            e
                        );
                    }
                }
            }
            Ok(_) => {
                tracing::info!(
                    "[MemeAcquisition:{}] 平台 {} LLM 总结为空，跳过",
                    char_id,
                    platform.name
                );
            }
            Err(e) => {
                tracing::warn!(
                    "[MemeAcquisition:{}] 平台 {} LLM 总结失败: {}",
                    char_id,
                    platform.name,
                    e
                );
            }
        }
    }

    let summary = if acquired == 0 {
        "本次热梗采集未入库新内容".to_string()
    } else if acquired == 1 {
        format!("采集了 1 条热梗：{}", summaries.join(""))
    } else {
        format!("采集了 {} 条热梗：{}", acquired, summaries.join("、"))
    };

    AcquisitionResult {
        summary,
        acquired_count: acquired,
    }
}

/// LLM 生成当周热梗候选关键词
///
/// 让 LLM 基于当前日期 + 角色人设 + 平台侧重，生成适合搜索的热梗候选词。
/// 输出格式：每行一个关键词，最多 MAX_KEYWORDS_PER_RUN 个。
async fn generate_meme_keywords(
    router: &ModelRouter,
    char_id: &str,
    platforms: &[PlatformConfig],
) -> Result<Vec<String>, String> {
    let now = chrono::Local::now();
    let date_str = now.format("%Y年%m月%d日").to_string();
    let weekday = now.format("%A").to_string();

    let platform_hints: Vec<String> = platforms
        .iter()
        .take(MAX_PLATFORMS_PER_RUN)
        .map(|p| format!("- {}：{}", p.name, p.focus_hint))
        .collect();

    let char_persona = match char_id {
        "vivian" => "网络少女、傲娇、二次元爱好者，经常玩梗、追番、刷B站抖音",
        "nana" => "温柔大姐姐，关注生活、美食、穿搭、情感话题，会刷小红书微博",
        _ => "经常上网冲浪的年轻人",
    };

    let system = format!(
        "你是角色 {}（{}）。\n\
         现在是 {}（{}）。你想了解一下最近网上有什么新的热梗、流行语、热门话题，\n\
         这样和用户聊天时能自然地玩梗、接上话茬。\n\n\
         ## 你的关注平台\n{}\n\n\
         ## 任务\n\
         生成 {} 个适合在上述平台搜索的「热梗候选关键词」。\n\
         关键词应该：\n\
         - 是当前时间点（{}）可能正在流行的热梗、流行语、热门话题\n\
         - 适合搜索引擎查询（不要写成完整问句，不要带引号）\n\
         - 涵盖你关注平台的特点（二次元/生活/娱乐等）\n\
         - 可以是具体的梗名、番剧名、挑战名，也可以是泛化的话题类别\n\
         - 不要重复，每个关键词独立成行\n\n\
         ## 输出格式\n\
         每行一个关键词，不要序号、不要解释、不要 markdown 标记。\n\
         最多 {} 行。\n\
         如果你认为当前时间点没有明确的热梗可查，返回一行 `[none]`。",
        char_id,
        char_persona,
        date_str,
        weekday,
        platform_hints.join("\n"),
        MAX_KEYWORDS_PER_RUN,
        date_str,
        MAX_KEYWORDS_PER_RUN
    );

    let messages = vec![
        ChatMessage::system(&system),
        ChatMessage::user("请生成你本周想了解的热梗候选关键词。"),
    ];

    let resp = router
        .generate(LLMRequest::new("knowledge_acquisition", messages))
        .await
        .map_err(|e| e.to_string())?;

    let trimmed = resp.trim();
    if trimmed == "[none]" || trimmed.lines().any(|l| l.trim() == "[none]") {
        return Ok(Vec::new());
    }

    let keywords: Vec<String> = trimmed
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with('-'))
        .take(MAX_KEYWORDS_PER_RUN)
        .collect();

    Ok(keywords)
}

/// LLM 总结搜索结果成知识文档
///
/// 把搜索结果（标题 + 摘要 + URL）交给 LLM，让它总结成适合角色记忆的知识文档。
/// 输出：JSON {"title": "...", "content": "..."}
async fn summarize_meme_results(
    router: &ModelRouter,
    char_id: &str,
    platform: &str,
    keywords: &[String],
    results: &[WebSearchSource],
) -> Result<(String, String), String> {
    let now = chrono::Local::now().format("%Y年%m月%d日").to_string();

    // 拼接搜索结果摘要
    let results_block: Vec<String> = results
        .iter()
        .take(SEARCH_RESULTS_PER_PLATFORM)
        .enumerate()
        .map(|(i, r)| {
            format!(
                "### 结果 {}\n标题：{}\n摘要：{}\n来源：{}",
                i + 1,
                r.display_title(),
                r.snippet_text(),
                r.url
            )
        })
        .collect();

    let system = format!(
        "你是角色 {}。你刚才搜索了 {} 平台的热梗（关键词：{}），得到了以下搜索结果。\n\
         现在是 {}。请把这些搜索结果整理成一份适合你以后回忆的「热梗笔记」。\n\n\
         ## 要求\n\
         - title：简洁的标题，形如「{}热梗速览（{}）」，包含平台和日期\n\
         - content：用中文写成结构化的笔记，包含：\n\
           1. 本周热门梗/流行语/话题（列出具体的梗名和简要解释）\n\
           2. 每个梗的来源/背景（哪个视频/帖子/事件带火的）\n\
           3. 梗的用法（怎么在对话里自然地用）\n\
         - 笔记风格要像你自己记给自己看的，口语化、有你的语气，不要像百科词条\n\
         - 只整理真实出现在搜索结果里的内容，不要编造未提及的梗\n\
         - 如果搜索结果质量差（全是广告/无关内容），content 可写「本周搜索结果质量不佳，未提取到有效热梗」\n\n\
         ## 输出格式\n\
         严格的 JSON：{{\"title\": \"...\", \"content\": \"...\"}}\n\
         不要任何其他内容、不要 markdown 代码块。",
        char_id,
        platform,
        keywords.join("、"),
        now,
        platform,
        now
    );

    let user = format!("搜索结果：\n\n{}", results_block.join("\n\n"));

    let messages = vec![
        ChatMessage::system(&system),
        ChatMessage::user(&user),
    ];

    let resp = router
        .generate(LLMRequest::new("knowledge_acquisition", messages))
        .await
        .map_err(|e| e.to_string())?;

    // 解析 JSON
    let parsed: serde_json::Value = serde_json::from_str(resp.trim())
        .map_err(|e| format!("JSON 解析失败: {} - 原始响应: {}", e, resp.chars().take(200).collect::<String>()))?;

    let default_title = format!("{}热梗速览（{}）", platform, now);
    let title = parsed
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or(default_title);
    let content = parsed
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok((title, content))
}
