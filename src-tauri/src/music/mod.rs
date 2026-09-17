//! 音乐播放能力：把「按名字找歌并放出来」抽象成可插拔的音源。
//!
//! 音源与能力对照：
//!
//! | 源 | 能搜索 | 能自动播放 | 说明 |
//! | :--- | :--- | :--- | :--- |
//! | [`local`] | ✅ 按文件名 | ✅ 直接解码播放 | 要求用户有本地文件 |
//! | [`deeplink`] | ❌ | ❌ 只能打开搜索页 | 覆盖流媒体客户端（网易云/QQ音乐/Spotify） |
//!
//! 与 [`crate::world::music`] 的分工：本模块负责「按名字找歌并放出来」，
//! 那边负责系统播放感知与控制（SMTC）。

pub mod deeplink;
pub mod local;

use serde::{Deserialize, Serialize};

/// 音源标识
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackSource {
    /// 本地音乐文件
    Local,
    /// 网易云音乐（桌面客户端深链）
    Netease,
    /// QQ 音乐（桌面客户端深链）
    Qqmusic,
    /// Spotify（客户端深链，URI 方案最完整）
    Spotify,
}

impl TrackSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Netease => "netease",
            Self::Qqmusic => "qqmusic",
            Self::Spotify => "spotify",
        }
    }

    /// 解析模型传入的 source 参数；`auto` / 未知值返回 None（表示由调用方决定）
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "local" => Some(Self::Local),
            "netease" | "netease_cloud" | "cloudmusic" => Some(Self::Netease),
            "qqmusic" | "qq_music" | "qq" => Some(Self::Qqmusic),
            "spotify" => Some(Self::Spotify),
            _ => None,
        }
    }

    /// 面向用户的显示名
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Local => "本地",
            Self::Netease => "网易云音乐",
            Self::Qqmusic => "QQ 音乐",
            Self::Spotify => "Spotify",
        }
    }
}

/// 一首候选曲目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackCandidate {
    /// 本次搜索结果内的序号（1-based，供 `music_play` 按编号引用）
    pub index: usize,
    pub title: String,
    pub artist: String,
    /// 时长（秒），未知为 0
    pub duration_secs: u64,
    /// 音源标识（[`TrackSource::as_str`]）
    pub source: String,
    /// 播放定位标识：本地 = 文件绝对路径；流媒体 = 搜索关键词或曲目 id
    pub id: String,
}

impl TrackCandidate {
    /// 渲染成给模型看的一行
    pub fn render_line(&self) -> String {
        let artist = if self.artist.is_empty() {
            "未知艺术家".to_string()
        } else {
            self.artist.clone()
        };
        let dur = if self.duration_secs > 0 {
            format!(" [{}:{:02}]", self.duration_secs / 60, self.duration_secs % 60)
        } else {
            String::new()
        };
        format!(
            "{}. {} — {}{} ({})",
            self.index,
            self.title,
            artist,
            dur,
            TrackSource::parse(&self.source)
                .map(|s| s.display_name())
                .unwrap_or(&self.source)
        )
    }
}

/// 播放结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayOutcome {
    /// 面向用户的说明
    pub message: String,
    /// 是否**真正开始播放**了。
    /// 深链源只能打开搜索页，此处为 `false`。
    pub auto_played: bool,
    /// 实际使用的音源
    pub source: String,
    /// 本地播放时的文件路径 / 流媒体的定位标识
    pub target: String,
}

/// 音乐相关的配置读取（供工具层调用，避免工具直接依赖 config 结构）
#[derive(Debug, Clone, Default)]
pub struct MusicSettings {
    /// 本地音乐目录
    pub local_dirs: Vec<String>,
    /// 首选音源；`auto` 表示按可用性自动挑
    pub preferred_source: String,
    /// 搜索结果条数上限
    pub max_results: usize,
}

impl MusicSettings {
    /// 默认搜索结果上限
    pub const DEFAULT_MAX_RESULTS: usize = 8;

    pub fn max_results_or_default(&self) -> usize {
        if self.max_results == 0 {
            Self::DEFAULT_MAX_RESULTS
        } else {
            self.max_results.clamp(1, 50)
        }
    }
}
