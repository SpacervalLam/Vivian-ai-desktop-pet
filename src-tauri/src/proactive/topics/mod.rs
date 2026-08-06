//! 话题子模块

pub mod pool;
pub mod recall;
pub mod tree;

pub use pool::{DailyTopicPool, InterestExtender, Period, TopicPool, TopicUsageState};
pub use recall::MemoryRecall;
pub use tree::{TopicNode, TopicTree, TOPIC_TREE};

/// 根据角色 ID 生成兴趣搜索查询（用于内心独白生成前的网络搜索）
///
/// 返回 1-2 条搜索查询字符串，由调用方通过 LLM search grounding 获取结果。
/// 查询设计混合了日常话题和角色兴趣，避免每次都只搜单一兴趣导致内容刻意。
pub fn interest_search_queries(char_id: &str) -> Vec<&'static str> {
    match char_id {
        // Vivian：二次元 / 网络热梗 / 可爱事物 / 生活小趣事 —— 混合话题不局限于动漫
        "vivian" | "薇薇安" => vec![
            "今日有趣的小事 可爱新闻 生活趣事",
            "最近好看的动漫 新番推荐 网络热梗",
        ],
        // Nana：书籍 / 茶 / 花 / 生活美学
        "nana" | "娜娜" => vec![
            "生活美学 安静的小事 治愈瞬间",
            "好书推荐 茶文化 花艺 近期资讯",
        ],
        // 其他角色：通用话题
        _ => vec![
            "今日有趣的事 生活小确幸",
        ],
    }
}
