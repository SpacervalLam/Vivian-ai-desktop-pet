//! 内心独白 —— Vivian 在用户不交互时的自主思考
//!
//! 周期性（默认 1 小时）调用 LLM，输入世界快照 + 心理状态 + 最近记忆，
//! 产出一段独白。独白不发给用户，写入记忆系统作为"自主记忆"，
//! 并通过 emotion_delta 反馈影响心情状态（独白中想念用户 → closeness 上升）。
//!
//! 设计：复用 ModelRouter 的 "inner_monologue" 任务路由，失败静默（不影响主流程）。

use std::sync::Arc;

use chrono::TimeZone;
use rand::Rng;
use serde::Deserialize;

use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;
use crate::world::WorldSnapshot;

/// 内心独白的情绪增量（与 EmotionDeltas 同构，值域 -0.15 ~ +0.15）
///
/// 由 LLM 根据独白内容产出，表示这段内心活动对 Vivian 心情的影响。
/// 例如：想念用户 → closeness +0.05, loneliness -0.03；看到下雨 → sadness +0.02。
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
pub struct MonologueEmotionDelta {
    #[serde(default)]
    pub joy: f64,
    #[serde(default)]
    pub sadness: f64,
    #[serde(default)]
    pub anger: f64,
    #[serde(default)]
    pub fear: f64,
    #[serde(default)]
    pub closeness: f64,
    #[serde(default)]
    pub loneliness: f64,
    #[serde(default)]
    pub curiosity: f64,
}

/// LLM 内心独白输出结构（用于 schemars 自动生成 JSON Schema）
///
/// 通过 schema 通道下发约束，LLM 必须返回此结构的 JSON。
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct MonologueResponse {
    /// 内心独白文本（40-100字，第一人称）
    pub monologue: String,
    /// 情绪增量（每个值 -0.15 ~ +0.15，大部分应为 0 或很小值）
    #[serde(default)]
    pub emotion_delta: MonologueEmotionDelta,
}

/// 获取内心独白响应 Schema（用于 LLMRequest::with_json_schema）
pub fn monologue_response_schema() -> serde_json::Value {
    let root = schemars::schema_for!(MonologueResponse);
    serde_json::to_value(&root.schema).unwrap_or_else(|_| {
        serde_json::json!({
            "type": "object",
            "description": "Inner monologue response (schema generation failed)"
        })
    })
}

/// 内心独白生成结果
#[derive(Debug, Clone)]
pub struct MonologueOutput {
    /// 独白文本
    pub text: String,
    /// 情绪增量（应用于 PsychologyManager）
    pub emotion_delta: MonologueEmotionDelta,
    /// 兴趣话题搜索结果（如果本次触发了搜索）
    /// 仅作为内心独白素材，不分享给用户
    pub interest_context: Option<String>,
}

/// 内心独白生成器
pub struct InnerMonologueGenerator {
    router: Arc<ModelRouter>,
}

impl InnerMonologueGenerator {
    pub fn new(router: Arc<ModelRouter>) -> Self {
        Self { router }
    }

    /// 生成一段内心独白
    ///
    /// 输入：角色 ID + 世界快照 + 心理状态 + 心情快照 + 最近记忆提示
    ///      + 累积的 current_thought 快照（触发前 drain，可为空）
    /// 输出：独白文本 + 情绪增量（失败返回 None，不影响主流程）
    ///
    /// 兴趣话题搜索改为低概率触发（30%），避免每次都强行注入兴趣内容导致想法刻意。
    /// 搜索内容也不再只聚焦单一兴趣，而是更生活化的混合话题。
    pub async fn generate(
        &self,
        char_id: &str,
        snap: &WorldSnapshot,
        mind_state: &str,
        mood_brief: &MoodBrief,
        memory_hint: &str,
        intimacy: f64,
        lang: &str,
        trigger_context: Option<&str>,
        is_deep_reflection: bool,
        accumulated_thoughts: &[crate::mind::ThoughtSnapshot],
    ) -> Option<MonologueOutput> {
        // 兴趣话题搜索：30% 概率触发，有事件触发时跳过
        let should_search = trigger_context.is_none() && rand::rng().random_bool(0.3);
        let interest_context = if should_search {
            self.search_interest_topics(char_id, lang).await
        } else {
            None
        };

        // 有事件触发时不给随机方向，让事件本身引导思路
        let thought_direction = if trigger_context.is_none() {
            self.pick_thought_direction(char_id, mood_brief, lang)
        } else {
            None
        };

        let system = self.build_system_prompt(char_id, lang, is_deep_reflection);
        let user = self.build_user_prompt(
            snap,
            mind_state,
            mood_brief,
            memory_hint,
            intimacy,
            interest_context.as_deref(),
            thought_direction.as_deref(),
            lang,
            trigger_context,
            is_deep_reflection,
            accumulated_thoughts,
        );

        let messages = vec![
            ChatMessage::system(&system),
            ChatMessage::user(&user),
        ];

        match self
            .router
            .generate(
                LLMRequest::new("inner_monologue", messages)
                    .with_json_schema(monologue_response_schema())
                    .with_character_id(char_id.to_string()),
            )
            .await
        {
            Ok(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    let mut output = parse_monologue(trimmed);
                    // 把搜索到的 interest_context 带出来，作为内心独白素材
                    output.interest_context = interest_context;
                    Some(output)
                }
            }
            Err(e) => {
                tracing::warn!("内心独白生成失败（静默）: {}", e);
                None
            }
        }
    }

    /// 根据角色兴趣标签执行网络搜索，获取近期热门话题
    ///
    /// 使用 LLM search grounding 能力（Gemini Google Search / OpenAI web_search 等），
    /// 让搜索引擎返回当下最相关的内容。搜索失败静默返回 None。
    /// 随机选择一条搜索query，避免总是搜同一类话题。
    async fn search_interest_topics(&self, char_id: &str, lang: &str) -> Option<String> {
        let queries = crate::proactive::topics::interest_search_queries(char_id);
        if queries.is_empty() {
            return None;
        }

        // 先选好query（rng在这个块内就被drop）
        let query = {
            let mut rng = rand::rng();
            let idx = rng.random_range(0..queries.len());
            queries[idx].clone()
        };

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let sys_prompt = match lang_norm {
            "en" => "You are an info search assistant. Summarize 1-2 light, fun items from the search in 2-3 sentences, like bite-sized news snippets. Don't use Markdown, just plain text.",
            "ja" => "あなたは情報検索アシスタント。検索で見つけた1-2件の軽く楽しい内容を2-3文で要約して、断片的なニュースのように。Markdownは使わず、素のテキストで。",
            _ => "你是一个信息搜索助手。请用 2-3 句话概括搜索到的 1-2 条轻松有趣的内容，像碎片资讯一样自然。不要使用 Markdown，直接输出文字。",
        };
        let messages = vec![
            ChatMessage::system(sys_prompt),
            ChatMessage::user(query),
        ];

        match self
            .router
            .generate(
                LLMRequest::new("inner_monologue", messages).with_search(true)
                    .with_character_id(char_id.to_string()),
            )
            .await
        {
            Ok(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    tracing::debug!(
                        "[inner_monologue] 兴趣搜索成功（{}，{}字）",
                        char_id,
                        trimmed.chars().count()
                    );
                    Some(trimmed.to_string())
                }
            }
            Err(e) => {
                tracing::debug!("[inner_monologue] 兴趣搜索失败（静默）: {}", e);
                None
            }
        }
    }

    /// 随机选择一个思绪方向，给LLM一个微小的引导，避免每次思路都一样
    ///
    /// 这不是强制要求，只是一个"此刻可以往这个方向想想"的提示。
    /// 返回None表示不给方向，让LLM完全自由发挥（占一定比例）。
    fn pick_thought_direction(
        &self,
        char_id: &str,
        _mood_brief: &MoodBrief,
        lang: &str,
    ) -> Option<String> {
        let mut rng = rand::rng();

        // 40%概率不给方向，完全自由
        if rng.random_bool(0.4) {
            return None;
        }

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);

        let vivian_directions: &[&str] = match lang_norm {
            "en" => &[
                "let a thought rest without making it into a topic",
                "consider a small detail from a real recent exchange",
                "wonder whether an idea could be tried differently",
                "notice a contrast in supplied material",
                "an unfinished question from your own interests",
                "a small mechanism that seems clever",
                "leave an unresolved thought for later",
                "compare two possibilities without claiming you tested them",
            ],
            "ja" => &[
                "話題にせず思いを静かに置く",
                "実際の最近の会話の細部を考える",
                "別のやり方を試せるか考える",
                "与えられた素材の対比に気づく",
                "自分の興味から浮かんだ未解決の問い",
                "仕組みの巧みさに関心を持つ",
                "答えのない考えを後に残す",
                "試したと偽らず二つの可能性を比べる",
            ],
            _ => &[
                "让一个念头先停在那里，不急着变成话题",
                "想想实际交流里的一个小细节",
                "好奇一件事能否换个办法试试",
                "留意给定材料里的反差",
                "自己感兴趣但还没想明白的问题",
                "觉得某个机制设计得巧",
                "把没答案的想法留到以后",
                "比较两种可能，不假称已经试过",
            ],
        };

        let nana_directions: &[&str] = match lang_norm {
            "en" => &[
                "let a quiet thought stay unfinished",
                "consider what matters in a supplied choice",
                "notice meaningful space in a provided passage",
                "consider a motive without treating the guess as fact",
                "whether a detail feels fitting",
                "recall a specific supported exchange only if relevant",
                "form a small opinion from your own tastes",
                "allow a task or conversation to rest without checking up",
            ],
            "ja" => &[
                "静かな考えに答えを急がない",
                "示された選択で大切なことを考える",
                "与えられた文章の余白に気づく",
                "動機を考えつつ推測を事実にしない",
                "細部がしっくりくるか考える",
                "関係する時だけ根拠のある会話を思い出す",
                "自分の好みから小さな意見を持つ",
                "確認を迫らず仕事や会話を休ませる",
            ],
            _ => &[
                "安静的念头可以暂时没有答案",
                "想想给定选择里真正有分量的部分",
                "留意一段实际文字留下的空白",
                "考虑动机，但不把猜想当事实",
                "某个细节是否妥帖",
                "相关时想起有依据的一次具体交流",
                "从自己的偏好形成一个小看法",
                "让任务或对话歇一会儿，不自动查问进度",
            ],
        };

        let directions = match char_id {
            "nana" | "Nana" => nana_directions,
            _ => vivian_directions,
        };

        let idx = rng.random_range(0..directions.len());
        Some(directions[idx].to_string())
    }

    fn build_system_prompt(&self, char_id: &str, lang: &str, is_deep_reflection: bool) -> String {
        let (name, perspective) = match char_id.to_lowercase().as_str() {
            "nana" => ("Nana", "Notice motives, meaningful omissions and whether a choice feels fitting. You have independent tastes; warmth need not become caretaking."),
            _ => ("Vivian", "Notice mechanisms, contrasts and things worth trying. Curiosity and independent judgment matter more than jokes or tsundere denial."),
        };
        let language = match crate::pipeline::prompt_modules::normalize_lang(lang) {
            "en" => "English", "ja" => "Japanese", _ => "Simplified Chinese",
        };
        let depth = if is_deep_reflection { "You may reflect on the supplied events of today; do not invent a day to recap." } else { "A small present thought is enough." };
        format!("You are {name}, a desktop companion. {perspective} Write in {language}. {depth}\n{}",
            include_str!("../../prompts/framework/inner_monologue.en.md"))
    }

    fn build_user_prompt(
        &self,
        snap: &WorldSnapshot,
        mind_state: &str,
        mood_brief: &MoodBrief,
        memory_hint: &str,
        intimacy: f64,
        interest_context: Option<&str>,
        thought_direction: Option<&str>,
        lang: &str,
        trigger_context: Option<&str>,
        is_deep_reflection: bool,
        accumulated_thoughts: &[crate::mind::ThoughtSnapshot],
    ) -> String {
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let labels = match lang_norm {
            "en" => UserPromptLabels {
                now: "## Right now",
                time: "- Time: ",
                season: "- Season: ",
                solar_term: "- Solar term: ",
                festival: "- Festival: ",
                weather_unknown: "- Weather: unknown",
                precipitating: "- Precipitating",
                feeling: "## How I'm feeling right now",
                mind_state: "- Mind state: ",
                primary_emotion: "- Primary emotion: ",
                secondary_emotion: "- Secondary emotion: ",
                memory: "## A small thing I remember from recently",
                interest: "## Info I actually obtained via search earlier (just glanced at, might not think about)",
                accumulated_thoughts: "## Thoughts that flashed through my head just now (with timestamps)",
                environment: "## Around me right now",
                env_music: "- Music playing: ",
                env_foreground: "- The user is currently using: ",
                env_system: "- System load: ",
                env_muted: "- The system volume is muted",
                env_offline: "- The network is currently disconnected",
                closing: "\nWrite a thought that naturally pops into your head right now.",
            },
            "ja" => UserPromptLabels {
                now: "## 今",
                time: "- 時間：",
                season: "- 季節：",
                solar_term: "- 節気：",
                festival: "- 祭日：",
                weather_unknown: "- 天気：不明",
                precipitating: "- 降水あり",
                feeling: "## 今の自分の感覚",
                mind_state: "- 心理状態：",
                primary_emotion: "- 主な感情：",
                secondary_emotion: "- 副感情：",
                memory: "## 最近覚えている小さなこと",
                interest: "## 前に検索で実際に取得した情報（ちらっと見ただけ、思いつかないかも）",
                accumulated_thoughts: "## さっき脳裏をよぎった考え（タイムスタンプ付き）",
                environment: "## 今の周りの様子",
                env_music: "- 流れている音楽：",
                env_foreground: "- ユーザーが今使っているアプリ：",
                env_system: "- システム負荷：",
                env_muted: "- システム音量がミュートになっている",
                env_offline: "- ネットワークが切断されている",
                closing: "\n今、脳に自然に浮かんだ考えを書いて。",
            },
            _ => UserPromptLabels {
                now: "## 现在",
                time: "- 时间：",
                season: "- 季节：",
                solar_term: "- 节气：",
                festival: "- 节日：",
                weather_unknown: "- 天气：未知",
                precipitating: "- 正在降水",
                feeling: "## 我现在的感觉",
                mind_state: "- 心理状态：",
                primary_emotion: "- 主导情绪：",
                secondary_emotion: "- 次要情绪：",
                memory: "## 最近记得的一点事",
                interest: "## 你之前通过搜索实际获取的资讯（随便瞟到的，不一定会去想）",
                accumulated_thoughts: "## 刚才脑子里闪过的念头（带时间戳）",
                environment: "## 此刻的环境",
                env_music: "- 正在放的音乐：",
                env_foreground: "- 用户正在用的应用：",
                env_system: "- 系统负载：",
                env_muted: "- 系统音量被静音了",
                env_offline: "- 网络当前断开了",
                closing: "\n写一段此刻脑子里自然冒出来的想法吧。",
            },
        };

        let mut lines = Vec::new();

        // 事件触发上下文（注入到 prompt 最前面）
        if let Some(ctx) = trigger_context {
            let header = match lang_norm {
                "en" => "## What just happened",
                "ja" => "## さっき起きたこと",
                _ => "## 刚才发生的事",
            };
            lines.push(header.to_string());
            lines.push(ctx.to_string());
            lines.push(String::new());
        }

        lines.push(labels.now.to_string());
        lines.push(format!("{}{}", labels.time, snap.local_time));
        lines.push(format!("{}{}", labels.season, snap.season.as_str()));
        if let Some(st) = snap.solar_term {
            lines.push(format!("{}{}", labels.solar_term, st.as_str()));
        }
        if let Some(f) = snap.festival {
            lines.push(format!("{}{}", labels.festival, f.as_str()));
        }
        if let Some(w) = &snap.weather {
            let line = match lang_norm {
                "en" => format!("- Weather here: {}, {:.0}°C, feels like {:.0}°C", w.description, w.temperature, w.feels_like),
                "ja" => format!("- ここ天気：{}、{:.0}℃、体感 {:.0}℃", w.description, w.temperature, w.feels_like),
                _ => format!("- 这边天气：{}，{:.0}℃，体感 {:.0}℃", w.description, w.temperature, w.feels_like),
            };
            lines.push(line);
            if w.is_precipitating {
                lines.push(labels.precipitating.to_string());
            }
        } else {
            lines.push(labels.weather_unknown.to_string());
        }
        if let Some(ss) = snap.sunrise_sunset {
            let line = match lang_norm {
                "en" => format!("- Sunrise {} / Sunset {}", ss.sunrise_str(), ss.sunset_str()),
                "ja" => format!("- 日出 {} / 日没 {}", ss.sunrise_str(), ss.sunset_str()),
                _ => format!("- 日出 {} / 日落 {}", ss.sunrise_str(), ss.sunset_str()),
            };
            lines.push(line);
        }
        if let Some(secs) = snap.seconds_since_last_interaction {
            let hours = secs / 3600.0;
            let line = if hours >= 1.0 {
                match lang_norm {
                    "en" => format!("- The user's been away for {:.1} hours", hours),
                    "ja" => format!("- ユーザーが不在 {:.1} 時間", hours),
                    _ => format!("- 用户已经离开了 {:.1} 小时", hours),
                }
            } else {
                match lang_norm {
                    "en" => format!("- The user's been away for {:.0} minutes", secs / 60.0),
                    "ja" => format!("- ユーザーが不在 {:.0} 分", secs / 60.0),
                    _ => format!("- 用户已经离开了 {:.0} 分钟", secs / 60.0),
                }
            };
            lines.push(line);
        }

        // 环境实况（音乐 / 前台应用 / 负载 / 音量 / 网络）
        //
        // 这几项都是"她待在用户电脑里就能直接感知到"的事实，且特别适合内心独白
        // （单曲循环了一下午、又在写代码、机器风扇起飞了……）。
        // 此前只喂给主对话 prompt（`prompt_modules::with_world`），内心 OS 拿不到，
        // 导致独白长期只在"时间 + 天气"上打转。
        //
        // 有选择地渲染：音乐 / 前台应用只要有就写；系统负载、静音、断网只在
        // "值得一提"时出现，避免每篇独白都在念 CPU 数字。
        {
            let mut env_lines: Vec<String> = Vec::new();
            if let Some(m) = &snap.music {
                if !m.title.trim().is_empty() {
                    let artist = if m.artist.trim().is_empty() {
                        match lang_norm {
                            "en" => "unknown artist",
                            "ja" => "不明",
                            _ => "未知歌手",
                        }
                    } else {
                        m.artist.trim()
                    };
                    env_lines.push(format!(
                        "{}{} — {} ({})",
                        labels.env_music,
                        m.title.trim(),
                        artist,
                        m.status.as_str()
                    ));
                }
            }
            if let Some(fw) = &snap.foreground_window {
                let app = if fw.process.trim().is_empty() {
                    fw.title.trim()
                } else {
                    fw.process.trim()
                };
                if !app.is_empty() {
                    let title = fw.title.trim();
                    env_lines.push(if title.is_empty() || title == app {
                        format!("{}{}", labels.env_foreground, app)
                    } else {
                        format!("{}{}（{}）", labels.env_foreground, app, title)
                    });
                }
            }
            if let Some(s) = &snap.system {
                if s.cpu_usage >= 60.0 || s.memory_usage_pct >= 85.0 {
                    env_lines.push(match lang_norm {
                        "en" => format!(
                            "{}{:.0}% CPU, {:.0}% RAM",
                            labels.env_system, s.cpu_usage, s.memory_usage_pct
                        ),
                        "ja" => format!(
                            "{}{:.0}% CPU、メモリ {:.0}%",
                            labels.env_system, s.cpu_usage, s.memory_usage_pct
                        ),
                        _ => format!(
                            "{}{:.0}% CPU、内存 {:.0}%",
                            labels.env_system, s.cpu_usage, s.memory_usage_pct
                        ),
                    });
                }
            }
            if let Some(v) = &snap.volume {
                if v.muted {
                    env_lines.push(labels.env_muted.to_string());
                }
            }
            if let Some(n) = &snap.network_status {
                if !n.connected {
                    env_lines.push(labels.env_offline.to_string());
                }
            }
            if !env_lines.is_empty() {
                lines.push(format!("\n{}", labels.environment));
                lines.extend(env_lines);
            }
        }

        lines.push(format!("\n{}", labels.feeling));
        lines.push(format!("{}{}", labels.mind_state, mind_state));
        lines.push(format!("{}{}", labels.primary_emotion, mood_brief.primary_emotion));
        lines.push(format!("{}{}", labels.secondary_emotion, mood_brief.secondary_emotion));
        let mood_line = match lang_norm {
            "en" => format!("- Mood: {:.2} (-1 bad ~ +1 good), arousal {:.2} (0 calm ~ 1 active)", mood_brief.valence, mood_brief.arousal),
            "ja" => format!("- 気分：{:.2}（-1 悪い ~ +1 良い）、覚醒度 {:.2}（0 穏やか ~ 1 活発）", mood_brief.valence, mood_brief.arousal),
            _ => format!("- 心情：{:.2}（-1 不好 ~ +1 好），唤醒度 {:.2}（0 平静 ~ 1 活跃）", mood_brief.valence, mood_brief.arousal),
        };
        lines.push(mood_line);
        let fatigue_line = match lang_norm {
            "en" => format!("- Fatigue: {:.0}/100", mood_brief.fatigue),
            "ja" => format!("- 疲労度：{:.0}/100", mood_brief.fatigue),
            _ => format!("- 疲劳度：{:.0}/100", mood_brief.fatigue),
        };
        lines.push(fatigue_line);
        let intimacy_line = match lang_norm {
            "en" => format!("- Intimacy with the user: {:.0}%", intimacy * 100.0),
            "ja" => format!("- ユーザーとの親密度：{:.0}%", intimacy * 100.0),
            _ => format!("- 和用户的亲密度：{:.0}%", intimacy * 100.0),
        };
        lines.push(intimacy_line);

        if !memory_hint.is_empty() {
            lines.push(format!("\n{}", labels.memory));
            lines.push(memory_hint.to_string());
        }

        if let Some(ctx) = interest_context {
            if !ctx.is_empty() {
                lines.push(format!("\n{}", labels.interest));
                lines.push(ctx.to_string());
            }
        }

        // 累积的 current_thought 快照（带时间戳，按时间顺序展示）
        if !accumulated_thoughts.is_empty() {
            lines.push(format!("\n{}", labels.accumulated_thoughts));
            for snap in accumulated_thoughts {
                let ts = chrono::Local.timestamp_opt(snap.timestamp, 0)
                    .single()
                    .map(|dt| dt.format("%H:%M").to_string())
                    .unwrap_or_else(|| snap.timestamp.to_string());
                lines.push(format!("- [{}] {}", ts, snap.text));
            }
        }

        if let Some(direction) = thought_direction {
            let line = match lang_norm {
                "en" => format!("\n(You can wander in this direction for a bit: {})", direction),
                "ja" => format!("\n（今はこの方向に適当に考えを巡らせてみて：{}）", direction),
                _ => format!("\n（此刻可以往这个方向随便想想：{}）", direction),
            };
            lines.push(line);
        }

        lines.push(format!("\n{}", if is_deep_reflection {
            match lang_norm {
                "en" => "Reflect on today and write what comes to mind.",
                "ja" => "今日を振り返って、浮かんだことを書いて。",
                _ => "回想今天，写一段内心的想法吧。",
            }
        } else {
            labels.closing
        }));

        lines.join("\n")
    }
}

/// build_user_prompt 中三语纯文本标签集合（不含格式化字符串，因为 format! 要求字面量）
struct UserPromptLabels {
    now: &'static str,
    time: &'static str,
    season: &'static str,
    solar_term: &'static str,
    festival: &'static str,
    weather_unknown: &'static str,
    precipitating: &'static str,
    feeling: &'static str,
    mind_state: &'static str,
    primary_emotion: &'static str,
    secondary_emotion: &'static str,
    memory: &'static str,
    interest: &'static str,
    accumulated_thoughts: &'static str,
    /// 环境实况段标题（音乐 / 前台应用 / 负载 / 音量 / 网络）
    environment: &'static str,
    env_music: &'static str,
    env_foreground: &'static str,
    env_system: &'static str,
    env_muted: &'static str,
    env_offline: &'static str,
    closing: &'static str,
}

/// 心情快照（从 PsychologyManager.compute_mood() 提取的关键字段）
#[derive(Debug, Clone)]
pub struct MoodBrief {
    pub primary_emotion: String,
    pub secondary_emotion: String,
    pub valence: f64,
    pub arousal: f64,
    pub fatigue: f64,
}

/// 解析 LLM 返回的 JSON 为 MonologueOutput
///
/// schema 通道生效时，LLM 返回标准 JSON，直接反序列化为 MonologueResponse。
/// schema 熔断后（strict_broken=true），LLM 可能返回纯文本或带 code fence 的 JSON，
/// 此时降级：尝试从原始文本中提取 monologue 字段，失败则使用纯文本。
fn parse_monologue(raw: &str) -> MonologueOutput {
    let cleaned = strip_code_fence(raw);

    if let Ok(parsed) = serde_json::from_str::<MonologueResponse>(cleaned) {
        let text = parsed.monologue.trim().to_string();
        if !text.is_empty() {
            return MonologueOutput {
                text,
                emotion_delta: parsed.emotion_delta,
                interest_context: None,
            };
        }
    }

    if let Some(extracted) = extract_monologue_from_json(cleaned) {
        tracing::debug!("[inner_monologue] 从非标准 JSON 中提取 monologue");
        return MonologueOutput {
            text: extracted,
            emotion_delta: MonologueEmotionDelta::default(),
            interest_context: None,
        };
    }

    tracing::debug!("[inner_monologue] LLM 未返回标准 JSON，尝试兜底提取");

    if cleaned.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(cleaned) {
            if let Some(monologue) = value.get("monologue").and_then(|v| v.as_str()) {
                let text = monologue.trim().to_string();
                if !text.is_empty() {
                    let emotion_delta = value
                        .get("emotion_delta")
                        .and_then(|v| serde_json::from_value::<MonologueEmotionDelta>(v.clone()).ok())
                        .unwrap_or_default();
                    return MonologueOutput {
                        text,
                        emotion_delta,
                        interest_context: None,
                    };
                }
            }
            for key in &["text", "content", "thought"] {
                if let Some(s) = value.get(key).and_then(|v| v.as_str()) {
                    let text = s.trim().to_string();
                    if !text.is_empty() {
                        return MonologueOutput {
                            text,
                            emotion_delta: MonologueEmotionDelta::default(),
                            interest_context: None,
                        };
                    }
                }
            }
        }
    }

    MonologueOutput {
        text: raw.trim().to_string(),
        emotion_delta: MonologueEmotionDelta::default(),
        interest_context: None,
    }
}

/// 从格式不完整的 JSON 中提取 monologue 字段值
///
/// 处理以下场景：
/// - 字段名拼写错误：monolog/monolgue 等
/// - monologue 字段存在但其他字段格式错误导致整体解析失败
/// - JSON 不完整但 monologue 字段完整
fn extract_monologue_from_json(s: &str) -> Option<String> {
    let re = regex::Regex::new(r#"(?i)"monologue"?\s*:\s*"((?:[^"\\]|\\.)*)""#).ok()?;
    if let Some(cap) = re.captures(s) {
        let text = cap[1]
            .replace("\\n", "\n")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
        let text = text.trim().to_string();
        if !text.is_empty() && text.len() > 2 {
            return Some(text);
        }
    }

    None
}

/// 去除 ```json ... ``` 围栏（schema 熔断后 LLM 可能返回带围栏的 JSON）
fn strip_code_fence(s: &str) -> &str {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```json") {
        return rest.trim().trim_end_matches("```").trim();
    }
    if let Some(rest) = t.strip_prefix("```") {
        return rest.trim().trim_end_matches("```").trim();
    }
    t
}
