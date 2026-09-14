//! 话题子模块

pub mod pool;
pub mod recall;
pub mod tree;

pub use pool::{DailyTopicPool, InterestExtender, Period, TopicPool, TopicUsageState};
pub use recall::MemoryRecall;
pub use tree::{TopicNode, TopicTree, TOPIC_TREE};

/// 根据角色 ID 生成兴趣搜索查询（用于内心独白生成前的网络搜索）
///
/// 查询词优先取兴趣画像顶层兴趣 + 活跃兴趣探针域（随用户画像与
/// 探针演化而动态变化）；画像尚未建立时回退角色预设的混合话题，
/// 避免每次都只搜单一兴趣导致内容刻意。
pub fn interest_search_queries(char_id: &str) -> Vec<String> {
    let hints = crate::discovery::interest_search_hints(char_id);
    if !hints.is_empty() {
        return hints;
    }
    fallback_queries(char_id).to_vec()
}

/// 画像为空时的角色预设查询（混合日常话题和角色兴趣）
fn fallback_queries(char_id: &str) -> Vec<String> {
    match char_id {
        // Vivian：二次元 / 网络热梗 / 可爱事物 / 生活小趣事 —— 混合话题不局限于动漫
        "vivian" | "薇薇安" => vec![
            "今日有趣的小事 可爱新闻 生活趣事".to_string(),
            "最近好看的动漫 新番推荐 网络热梗".to_string(),
        ],
        // Nana：书籍 / 茶 / 花 / 生活美学
        "nana" | "娜娜" => vec![
            "生活美学 安静的小事 治愈瞬间".to_string(),
            "好书推荐 茶文化 花艺 近期资讯".to_string(),
        ],
        // 其他角色：通用话题
        _ => vec!["今日有趣的事 生活小确幸".to_string()],
    }
}
