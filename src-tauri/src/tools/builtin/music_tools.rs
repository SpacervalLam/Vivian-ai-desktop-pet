//! 音乐工具 —— 读取系统当前播放 / 按名字找歌并播放。
//!
//! | 工具 | 只读 | 风险档 | 说明 |
//! | :--- | :--- | :--- | :--- |
//! | [`MusicNowPlayingTool`] | ✅ | `Safe` | 读系统当前播放（SMTC） |
//! | [`MusicPlayTool`] | ❌ | `Shell` | 按名字找歌并播放 / 打开搜索页，需确认 |
//!
//! 检索与播放分工：
//! - 本地检索覆盖常见位置（见 `music::local::resolve_search_dirs`）；
//!   任意位置的扫描由陪伴侧 `run_command`（`Get-ChildItem`）或 `directory` 参数承担。
//! - 流媒体平台无公开搜索 API，`music_play` 经深链打开搜索页，
//!   `PlayOutcome::auto_played` 为 `false`，由调用方转述实际结果。

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::music::{deeplink, local, MusicSettings, TrackSource};
use crate::tools::types::{
    PermissionResult, Tool, ToolCategory, ToolResult, ToolRiskTier, ToolUseContext,
    ValidationResult,
};
use crate::world::MusicSource;

/// 把「来源偏好 + 本地可用性」解析为实际要用的音源
///
/// 返回 `None` 表示无可用音源，由调用方提示用户配置首选音源或提供 `directory`。
fn resolve_source(requested: &str, settings: &MusicSettings, local_hits: usize) -> Option<TrackSource> {
    // 显式指定优先
    if requested != "auto" && !requested.is_empty() {
        return TrackSource::parse(requested);
    }
    // 配置里的首选音源
    let preferred = settings.preferred_source.trim();
    if preferred != "auto" && !preferred.is_empty() {
        return TrackSource::parse(preferred);
    }
    // auto：本地有命中就用本地（唯一能自动播放的源）
    if local_hits > 0 {
        return Some(TrackSource::Local);
    }
    None
}

// ============================================================================
// music_now_playing
// ============================================================================

/// 读取系统当前正在播放的音乐
pub struct MusicNowPlayingTool;

#[async_trait]
impl Tool for MusicNowPlayingTool {
    fn name(&self) -> &str {
        "music_now_playing"
    }

    fn description(&self) -> &str {
        "Read what music is currently playing on this computer (title / artist / album / \
         playback state / source app), via the system media session. \
         Use when the user asks \"what am I listening to\" or before commenting on their music."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "读取这台电脑当前正在播放的音乐（曲名 / 艺术家 / 专辑 / 播放状态 / 来源应用），\
         数据来自系统媒体会话。\n\
         典型场景：用户问\"我在听什么\"，或你想对用户正在听的音乐发表评论之前。",
            "ja" => "この PC で現在再生中の音楽（曲名 / アーティスト / アルバム / 再生状態 / 再生元アプリ）を\
         システムメディアセッションから読み取る。\n\
         典型的なシナリオ：ユーザーが「何を聴いてる？」と聞いた時、または再生中の音楽に言及する前。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "我在听什么歌\n现在放的是什么\n这首歌叫什么\n谁唱的\n帮我看看在播什么",
            "en" => "what am I listening to\nwhat song is this\nwho sings this\nwhat's playing now",
            "ja" => "何を聴いてる\n今の曲は何\nこの曲の名前は\n誰が歌ってる",
            _ => "",
        }
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn validate_input(&self, _input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        ValidationResult::success(None)
    }

    async fn check_permissions(&self, _input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        // 纯读取系统播放状态，无副作用
        PermissionResult::allow()
    }

    async fn call(&self, _args: Value, _ctx: &ToolUseContext) -> ToolResult {
        // MusicSource 是无状态 unit struct，直接新建即可
        match MusicSource::new().fetch().await {
            Ok(Some(snap)) => {
                let status_zh = match snap.status {
                    crate::world::PlaybackStatus::Playing => "播放中",
                    crate::world::PlaybackStatus::Paused => "已暂停",
                    crate::world::PlaybackStatus::Changing => "切换中",
                    _ => "未知状态",
                };
                let album = if snap.album.is_empty() {
                    String::new()
                } else {
                    format!("，专辑《{}》", snap.album)
                };
                let source = if snap.source_app.is_empty() {
                    String::new()
                } else {
                    format!("（来源：{}）", snap.source_app)
                };
                ToolResult::standard_success(
                    &format!(
                        "当前{}：{} — {}{}{}",
                        status_zh, snap.artist, snap.title, album, source
                    ),
                    Some(json!({
                        "title": snap.title,
                        "artist": snap.artist,
                        "album": snap.album,
                        "status": snap.status.as_str(),
                        "source_app": snap.source_app,
                    })),
                )
            }
            Ok(None) => ToolResult::standard_success(
                "当前没有在播放音乐。",
                Some(json!({ "playing": false })),
            ),
            Err(e) => ToolResult::standard_error(
                &format!("读取系统播放状态失败：{}", e),
                Some("MusicReadFailed"),
                None,
            ),
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Media
    }

    fn risk(&self) -> ToolRiskTier {
        ToolRiskTier::Safe
    }
}

// ============================================================================
// music_play
// ============================================================================

/// 按名字找歌并播放
pub struct MusicPlayTool;

#[async_trait]
impl Tool for MusicPlayTool {
    fn name(&self) -> &str {
        "music_play"
    }

    fn description(&self) -> &str {
        "Find a song by name and play it. Tries the local library first (actually plays the \
         file); if the user's source is a streaming platform, it opens that platform's search \
         page instead — in that case the song is NOT auto-played and you must tell the user \
         to click the result. Never claim playback started when it did not.\n\
         If the song is not in the usual places, find the file yourself first — \
         run_command with PowerShell `Get-ChildItem -Recurse -Filter *.mp3 -Path D:\\` \
         (or list_dir) — then pass the path as track_id to skip searching."
    }

    fn description_in(&self, lang: &str) -> &str {
        match lang {
            "zh" => "按名字找歌并播放。优先用本地曲库（会真正播放文件）；\
         若用户配置的是流媒体平台，则打开该平台的搜索页——此时**并没有自动播放**，\
         你必须如实告诉用户\"已打开搜索页，需要你点一下\"，绝不能谎称已开始播放。\n\
         歌不在常见位置时，**你自己就有命令行**：用 run_command 跑 PowerShell，\
         例如 `Get-ChildItem -Recurse -Filter *.mp3 -Path D:\\ -ErrorAction SilentlyContinue`，\
         拿到路径后传给 track_id 即可跳过检索（list_dir 也能用）。\n\
         典型场景：用户说\"放首晴天\"、\"来点周杰伦\"时调用。\n\
         注意：只是暂停/切歌/调音量请用 media_control，不要用本工具。",
            "ja" => "名前で曲を探して再生する。まずローカルライブラリを試し（実際にファイルを再生）、\
         ユーザーの設定がストリーミング平台ならその検索ページを開く——この場合**自動再生はされない**ため、\
         ユーザーに「検索ページを開きました、クリックしてください」と正直に伝えること。\n\
         曲がよくある場所にない場合は、**自分でコマンドを実行できる**：run_command で PowerShell を走らせ、\
         例 `Get-ChildItem -Recurse -Filter *.mp3 -Path D:\\ -ErrorAction SilentlyContinue`、\
         得たパスを track_id に渡せば検索をスキップできる（list_dir も可）。",
            _ => self.description(),
        }
    }

    fn usage_corpus(&self, lang: &str) -> &'static str {
        match lang {
            "zh" => "放首歌\n来点音乐\n放周杰伦的晴天\n我想听歌\n给我放首歌\n换个歌听听",
            "en" => "play a song\nput on some music\nplay 晴天 by 周杰伦\nI want to listen to",
            "ja" => "音楽をかけて\n曲を再生して\nこの曲をかけて",
            _ => "",
        }
    }

    fn anti_use_cases(&self) -> &[&str] {
        &[
            "Pause, resume, skip to next/previous track, or change volume (use media_control instead)",
            "Just checking what is playing (use music_now_playing instead)",
        ]
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Song title, artist, or both (e.g. \"周杰伦 晴天\"). Required unless track_id is given."
                },
                "track_id": {
                    "type": "string",
                    "description": "Exact local file path, taken found with file tools such as list_dir. Skips searching. Required unless query is given."
                },
                "source": {
                    "type": "string",
                    "enum": ["auto", "local", "netease", "qqmusic", "spotify"],
                    "description": "Which source to use. Defaults to the user's configured preference."
                },
                "directory": {
                    "type": "string",
                    "description": "Optional folder to search in. Use when the song is not in a usual place (e.g. \"D:\\\\\"). Ignored when track_id is given."
                },
                "index": {
                    "type": "number",
                    "description": "Which search result to play, 1-based. Defaults to 1 (best match). Ignored when track_id is given."
                }
            }
        })
    }

    fn parameters_schema_in(&self, lang: &str) -> Value {
        if lang != "zh" && lang != "ja" {
            return self.parameters_schema();
        }
        let (q, tid, s, d, i) = if lang == "zh" {
            (
                "曲名、艺术家，或两者（如「周杰伦 晴天」）。未给 track_id 时必填",
                "本地文件的完整路径，用文件工具（如 list_dir）找到的路径。给了它就跳过检索。未给 query 时必填",
                "使用哪个音源，默认取用户配置的偏好",
                "可选，指定在哪个文件夹里找。歌不在常见位置时用它（如 \"D:\\\\\"）。给了 track_id 时忽略",
                "播放第几条搜索结果（从 1 开始），默认第 1 条（最匹配）。给了 track_id 时忽略",
            )
        } else {
            (
                "曲名、アーティスト、または両方（例「周杰伦 晴天」）。track_id 未指定時は必須",
                "ローカルファイルの完全パス。ファイルツール（list_dir など）で見つけたパスから取得。指定すると検索をスキップ。query 未指定時は必須",
                "使用する音源。既定はユーザー設定の優先順位",
                "任意。検索するフォルダを指定。曲がよくある場所にない場合に使う（例 \"D:\\\\\"）。track_id 指定時は無視",
                "何番目の検索結果を再生するか（1 始まり）。既定は 1（最適一致）。track_id 指定時は無視",
            )
        };
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": q },
                "track_id": { "type": "string", "description": tid },
                "source": {
                    "type": "string",
                    "enum": ["auto", "local", "netease", "qqmusic", "spotify"],
                    "description": s
                },
                "directory": { "type": "string", "description": d },
                "index": { "type": "number", "description": i }
            }
        })
    }

    async fn validate_input(&self, input: &Value, _ctx: &ToolUseContext) -> ValidationResult {
        let has = |k: &str| {
            input
                .get(k)
                .and_then(Value::as_str)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false)
        };
        if has("query") || has("track_id") {
            ValidationResult::success(None)
        } else {
            ValidationResult::failure("必须提供 query 或 track_id 之一", 2)
        }
    }

    async fn check_permissions(&self, input: &Value, _ctx: &ToolUseContext) -> PermissionResult {
        let q = input.get("query").and_then(Value::as_str).unwrap_or("?");
        PermissionResult::ask(format!("是否允许播放音乐（{}）？", q))
    }

    async fn call(&self, args: Value, _ctx: &ToolUseContext) -> ToolResult {
        let settings = MusicSettings::default();

        let query = args.get("query").and_then(Value::as_str).unwrap_or("").trim();
        let requested = args
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or("auto")
            .trim();
        let index = args.get("index").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;

        // track_id 快路径：直接播放给定的文件路径，跳过检索与排序
        if let Some(track_id) = args
            .get("track_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let path = std::path::PathBuf::from(track_id);
            return match tokio::task::spawn_blocking(move || local::play_path(&path)).await {
                Ok(Ok(outcome)) => ToolResult::standard_success(
                    &outcome.message,
                    Some(json!({
                        "auto_played": outcome.auto_played,
                        "source": outcome.source,
                        "target": outcome.target,
                    })),
                ),
                Ok(Err(e)) => ToolResult::standard_error(
                    &format!("播放失败：{}", e),
                    Some("MusicPlayFailed"),
                    None,
                ),
                Err(e) => ToolResult::standard_error(
                    &format!("播放任务执行失败：{}", e),
                    Some("MusicPlayFailed"),
                    None,
                ),
            };
        }

        // 先看本地能否命中，供 auto 决策。
        // 搜索范围：显式 directory > 配置曲库 > 常见位置兜底
        let explicit_dir = args.get("directory").and_then(Value::as_str);
        let dirs = local::resolve_search_dirs(explicit_dir, &settings.local_dirs);
        let q = query.to_string();
        let limit = settings.max_results_or_default();
        let local_hits = if dirs.is_empty() {
            Vec::new()
        } else {
            match tokio::task::spawn_blocking(move || {
                let tracks = local::scan(&dirs);
                local::search(&tracks, &q, limit)
            })
            .await
            {
                Ok(hits) => hits,
                Err(e) => {
                    return ToolResult::standard_error(
                        &format!("检索本地曲库失败：{}", e),
                        Some("MusicSearchFailed"),
                        None,
                    )
                }
            }
        };

        let Some(source) = resolve_source(requested, &settings, local_hits.len()) else {
            return ToolResult::standard_error(
                &format!(
                    "没法播放「{}」：已搜过的位置里没有这个文件，也没有配置首选音源，\
                     所以不知道该用哪个平台去找。三个办法——\
                     ① **自己用 run_command 找**（推荐）：\
                     `Get-ChildItem -Recurse -Filter *{}*.mp3 -Path D:\\ -ErrorAction SilentlyContinue`，\
                     拿到路径后传给 track_id；\
                     ② 告诉我这首歌在哪个文件夹（用 directory 参数，如 \"D:\\\\\"）；\
                     ③ 在「设置 → 音乐」里指定首选音源（网易云 / QQ 音乐 / Spotify）。",
                    query, query
                ),
                Some("MusicSourceUnresolved"),
                None,
            );
        };

        match source {
            TrackSource::Local => {
                let Some(candidate) = local_hits.get(index - 1) else {
                    let lines: Vec<String> = local_hits.iter().map(|t| t.render_line()).collect();
                    return ToolResult::standard_error(
                        &format!(
                            "只匹配到 {} 首，取不到第 {} 首。候选是：\n{}",
                            local_hits.len(),
                            index,
                            lines.join("\n")
                        ),
                        Some("MusicTrackIndexOutOfRange"),
                        None,
                    );
                };
                // 多结果时把候选清单附在结果里，供调用方按需用 index 切换
                let alternatives: Vec<String> = if local_hits.len() > 1 {
                    local_hits
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| *i != index - 1)
                        .map(|(_, t)| t.render_line())
                        .collect()
                } else {
                    Vec::new()
                };
                let cand = candidate.clone();
                match tokio::task::spawn_blocking(move || local::play(&cand)).await {
                    Ok(Ok(outcome)) => {
                        let message = if alternatives.is_empty() {
                            outcome.message.clone()
                        } else {
                            format!(
                                "{}。另外还有 {} 首匹配：\n{}\n（想换其中某首就带上 index 参数）",
                                outcome.message,
                                alternatives.len(),
                                alternatives.join("\n")
                            )
                        };
                        ToolResult::standard_success(
                            &message,
                            Some(json!({
                                "auto_played": outcome.auto_played,
                                "source": outcome.source,
                                "target": outcome.target,
                                "alternatives": alternatives,
                            })),
                        )
                    }
                    Ok(Err(e)) => ToolResult::standard_error(
                        &format!("播放失败：{}", e),
                        Some("MusicPlayFailed"),
                        None,
                    ),
                    Err(e) => ToolResult::standard_error(
                        &format!("播放任务执行失败：{}", e),
                        Some("MusicPlayFailed"),
                        None,
                    ),
                }
            }
            // 流媒体：无搜索 API，经深链打开搜索页（auto_played = false）
            streaming => match deeplink::open_search(streaming, query) {
                Ok(outcome) => ToolResult::standard_success(
                    &outcome.message,
                    Some(json!({
                        "auto_played": outcome.auto_played,
                        "source": outcome.source,
                        "target": outcome.target,
                    })),
                ),
                Err(e) => ToolResult::standard_error(
                    &format!("打开搜索页失败：{}", e),
                    Some("MusicDeepLinkFailed"),
                    None,
                ),
            },
        }
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Media
    }

    fn risk(&self) -> ToolRiskTier {
        // 可能启动客户端 / 打开网页 / 输出声音，与 open_application 同级
        ToolRiskTier::Shell
    }
}

/// 构造全部音乐工具（供 builtin 注册）
pub fn all_music_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(MusicNowPlayingTool),
        Arc::new(MusicPlayTool),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(preferred: &str, dirs: Vec<String>) -> MusicSettings {
        MusicSettings {
            local_dirs: dirs,
            preferred_source: preferred.to_string(),
            max_results: 8,
        }
    }

    #[test]
    fn explicit_source_wins() {
        let s = settings("local", vec!["D:/music".into()]);
        assert_eq!(
            resolve_source("spotify", &s, 3),
            Some(TrackSource::Spotify)
        );
    }

    #[test]
    fn auto_prefers_local_when_hits_exist() {
        let s = settings("auto", vec!["D:/music".into()]);
        assert_eq!(resolve_source("auto", &s, 2), Some(TrackSource::Local));
    }

    #[test]
    fn auto_without_hits_and_no_preference_is_unresolved() {
        // 无本地命中且无偏好时返回 None，由调用方提示用户配置音源
        let s = settings("auto", vec!["D:/music".into()]);
        assert_eq!(resolve_source("auto", &s, 0), None);
    }

    #[test]
    fn unknown_source_string_is_unresolved() {
        let s = settings("auto", Vec::new());
        assert_eq!(resolve_source("tidal", &s, 0), None);
    }
}
