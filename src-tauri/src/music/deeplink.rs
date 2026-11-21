//! 流媒体平台深链：把「搜索关键词」映射为可打开的链接 / URI。
//!
//! 深链只能打开平台的搜索结果页，不能触发具体曲目的播放；
//! 例外是 Spotify 的官方 `spotify:track:{id}` URI（已知曲目 id 时可直接播放）。
//! 因此 [`open_search`] 返回的 `PlayOutcome::auto_played` 恒为 `false`
//! （`play_track` 命中 Spotify URI 时为例外）。
//!
//! 桌面客户端私有协议（`orpheus://` / `qqmusic://`）各版本行为不一致，
//! 一律走网页搜索页 URL。

use super::{PlayOutcome, TrackSource};

/// 百分号编码（RFC 3986 的 unreserved 之外全部编码）。
///
/// 只用于拼接搜索关键词——中文歌名必须编码，否则部分客户端会解析失败。
fn encode(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len() * 3);
    for b in segment.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// 打开某平台的**搜索页**所需的目标字符串（URL 或 URI）
pub fn search_target(source: TrackSource, query: &str) -> String {
    let q = encode(query);
    match source {
        // 网页版搜索页（type=1 限定单曲）
        TrackSource::Netease => format!("https://music.163.com/#/search/m/?s={}&type=1", q),
        TrackSource::Qqmusic => format!("https://y.qq.com/n/ryqq/search?w={}&t=song", q),
        // Spotify 官方 URI：桌面客户端会直接跳到搜索
        TrackSource::Spotify => format!("spotify:search:{}", q),
        // 本地不走深链
        TrackSource::Local => String::new(),
    }
}

/// 已知曲目 id 时的**直接播放** URI（目前仅 Spotify 官方支持）
pub fn track_target(source: TrackSource, track_id: &str) -> Option<String> {
    match source {
        TrackSource::Spotify => Some(format!("spotify:track:{}", track_id)),
        // 网易云/QQ音乐无公开的直接播放 URI 契约
        _ => None,
    }
}

/// 用系统 shell 打开一个 URL / URI。
///
/// 走 `explorer.exe`（经 ShellExecute），因此**注册过的自定义协议**
/// （`spotify:` 等）也会交给对应客户端处理，与 `open_url` 工具同一机制。
fn open_target(target: &str) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        crate::utils::process::silent_command("explorer")
            .arg(target)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("打开失败: {e}"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = target;
        Err("当前平台不支持深链打开".to_string())
    }
}

/// 打开某平台的搜索结果页
pub fn open_search(source: TrackSource, query: &str) -> Result<PlayOutcome, String> {
    if matches!(source, TrackSource::Local) {
        return Err("本地音源不支持深链搜索".to_string());
    }
    let target = search_target(source, query);
    open_target(&target)?;

    Ok(PlayOutcome {
        message: format!(
            "已在「{}」打开搜索页（关键词：{}）。深链只能打开搜索页，需要你点一下具体曲目才会播放。",
            source.display_name(),
            query
        ),
        auto_played: false,
        source: source.as_str().to_string(),
        target,
    })
}

/// 已知曲目 id 时直接播放（仅 Spotify 支持；其余平台返回 Err 供调用方降级）
pub fn play_track(source: TrackSource, track_id: &str, title: &str) -> Result<PlayOutcome, String> {
    let target = track_target(source, track_id).ok_or_else(|| {
        format!(
            "「{}」没有公开的「直接播放指定曲目」URI 契约，无法跳过搜索页",
            source.display_name()
        )
    })?;
    open_target(&target)?;

    Ok(PlayOutcome {
        message: format!("已在「{}」开始播放：{}", source.display_name(), title),
        auto_played: true,
        source: source.as_str().to_string(),
        target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_cjk() {
        let t = search_target(TrackSource::Netease, "晴天");
        assert!(t.contains("%E6%99%B4%E5%A4%A9"), "got {t}");
        // 保留字符不被编码
        assert!(t.contains("type=1"));
    }

    #[test]
    fn only_spotify_has_direct_track_uri() {
        assert!(track_target(TrackSource::Spotify, "abc").is_some());
        assert!(track_target(TrackSource::Netease, "123").is_none());
        assert!(track_target(TrackSource::Qqmusic, "123").is_none());
    }

    #[test]
    fn search_target_empty_for_local() {
        assert!(search_target(TrackSource::Local, "x").is_empty());
    }
}
