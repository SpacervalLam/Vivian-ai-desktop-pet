//! 推荐服务
//!
//! 话题推荐（复用 TopicTree + InterestExtender）+ 音乐推荐（兴趣→风格映射）。
//! 纯数据驱动映射，无状态。

use super::super::random_index;
use super::super::topics::{InterestExtender, TopicTree};

/// 兴趣标签 → 音乐风格推荐
#[derive(Clone, Copy)]
struct MusicEntry {
    text: &'static str,
}

fn interest_to_music() -> &'static [(&'static str, &'static [MusicEntry])] {
    static MUSIC_TABLE: &[(&str, &[MusicEntry])] = &[
        (
            "编程",
            &[
                MusicEntry {
                    text: "写代码的时候听 Lo-Fi 很专注哦~要不要试试？",
                },
                MusicEntry {
                    text: "编程时我喜欢听一些后摇滚~",
                },
            ],
        ),
        (
            "游戏",
            &[
                MusicEntry {
                    text: "很多游戏的 OST 超好听，你有关注的吗？",
                },
                MusicEntry {
                    text: "游戏玩累的时候，听听游戏原声放松一下也不错~",
                },
            ],
        ),
        (
            "音乐",
            &[
                MusicEntry {
                    text: "最近有听到什么好听的 J-Pop 吗？",
                },
                MusicEntry {
                    text: "独立音乐圈最近有不少宝藏~",
                },
            ],
        ),
        (
            "电影",
            &[MusicEntry {
                text: "电影配乐有时候比电影本身还让人难忘呢~",
            }],
        ),
        (
            "运动",
            &[MusicEntry {
                text: "跑步的时候听点节奏感强的歌，动力满满！",
            }],
        ),
        (
            "旅行",
            &[MusicEntry {
                text: "旅行的时候听民谣特别有感觉~",
            }],
        ),
    ];
    MUSIC_TABLE
}

const GENERAL_MUSIC: &[&str] = &[
    "最近有什么好听的歌可以分享给我吗？",
    "你喜欢什么风格的音乐？",
    "要不要一起听首歌？",
];

/// 推荐生成器
pub struct Recommender;

impl Recommender {
    /// 推荐一个话题
    ///
    /// 优先使用 InterestExtender 做深度扩展，兜底用 TopicTree 随机。
    pub fn recommend_topic(interest_tags: &[String]) -> Option<&'static str> {
        if !interest_tags.is_empty() {
            if let Some(p) = InterestExtender::random_extension(interest_tags) {
                return Some(p);
            }
        }
        TopicTree::random_prompt(if interest_tags.is_empty() {
            None
        } else {
            Some(interest_tags)
        })
    }

    /// 推荐一首音乐/风格
    pub fn recommend_music(interest_tags: &[String]) -> &'static str {
        if interest_tags.is_empty() {
            let idx = random_index(GENERAL_MUSIC.len());
            return GENERAL_MUSIC[idx];
        }
        let lower: Vec<String> = interest_tags.iter().map(|t| t.to_lowercase()).collect();
        let mut candidates: Vec<&MusicEntry> = Vec::new();
        for (tag, entries) in interest_to_music() {
            if lower.contains(&tag.to_lowercase()) {
                for e in entries.iter() {
                    candidates.push(e);
                }
            }
        }
        if candidates.is_empty() {
            let idx = random_index(GENERAL_MUSIC.len());
            return GENERAL_MUSIC[idx];
        }
        let idx = random_index(candidates.len());
        candidates[idx].text
    }
}
