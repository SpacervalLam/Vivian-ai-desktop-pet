//! 话题树
//!
//! 预定义多层话题树，按分类/兴趣标签查询，随机选取话题问句。
//! 使用 `once_cell` 缓存静态数据，零运行时分配。

use once_cell::sync::Lazy;

use super::super::random_index;

/// 话题节点（展平后的叶子节点）
#[derive(Debug, Clone)]
pub struct TopicNode {
    pub category: &'static str,
    pub name: &'static str,
    pub prompts: &'static [&'static str],
    pub interest_tags: &'static [&'static str],
}

/// 全量话题树
pub static TOPIC_TREE: Lazy<Vec<TopicNode>> = Lazy::new(build_topic_tree);

fn build_topic_tree() -> Vec<TopicNode> {
    vec![
        TopicNode {
            category: "日常",
            name: "天气",
            prompts: &[
                "今天天气真好呢~你喜欢晴天还是雨天？",
                "外面下雨了，在家做什么呢？",
                "最近天气变化好大，注意别感冒了~",
            ],
            interest_tags: &["生活"],
        },
        TopicNode {
            category: "日常",
            name: "美食",
            prompts: &[
                "今天吃了什么好吃的呀？",
                "有没有特别想吃的菜？我帮你想想~",
                "你平时会自己做饭吗？",
            ],
            interest_tags: &["美食", "生活"],
        },
        TopicNode {
            category: "日常",
            name: "睡眠",
            prompts: &[
                "昨晚睡得好吗？",
                "你一般几点睡呀？别熬夜哦~",
                "最近睡眠质量怎么样？",
            ],
            interest_tags: &["健康", "生活"],
        },
        TopicNode {
            category: "日常",
            name: "节日",
            prompts: &["最近有什么节日吗？要不要一起庆祝一下~"],
            interest_tags: &["生活"],
        },
        TopicNode {
            category: "兴趣",
            name: "音乐",
            prompts: &[
                "最近在听什么歌呀？",
                "你最喜欢哪种风格的音乐？",
                "有没有喜欢的歌手推荐给我？",
            ],
            interest_tags: &["音乐", "娱乐"],
        },
        TopicNode {
            category: "兴趣",
            name: "电影",
            prompts: &[
                "最近有看什么好看的电影吗？",
                "你喜欢什么类型的电影？",
                "有想推荐的剧吗~",
            ],
            interest_tags: &["电影", "娱乐"],
        },
        TopicNode {
            category: "兴趣",
            name: "游戏",
            prompts: &[
                "最近在玩什么游戏呀？",
                "你喜欢什么类型的游戏？",
                "有在玩什么新出的游戏吗？",
            ],
            interest_tags: &["游戏", "娱乐"],
        },
        TopicNode {
            category: "兴趣",
            name: "阅读",
            prompts: &[
                "最近有在读什么书吗？",
                "你最喜欢的一本书是什么？",
                "有想推荐的书吗~",
            ],
            interest_tags: &["阅读", "学习"],
        },
        TopicNode {
            category: "兴趣",
            name: "运动",
            prompts: &[
                "最近有运动吗？",
                "你喜欢什么运动呀？",
                "要不要一起动一动~",
            ],
            interest_tags: &["运动", "健康"],
        },
        TopicNode {
            category: "兴趣",
            name: "旅行",
            prompts: &[
                "有没有想去旅行的地方？",
                "你最喜欢去过哪里？",
                "最近有出行计划吗？",
            ],
            interest_tags: &["旅行", "生活"],
        },
        TopicNode {
            category: "科技",
            name: "编程",
            prompts: &[
                "最近在写什么代码呢？",
                "你最喜欢用什么编程语言？",
                "有没有遇到什么有趣的技术问题？",
            ],
            interest_tags: &["技术", "编程"],
        },
        TopicNode {
            category: "科技",
            name: "AI",
            prompts: &["最近 AI 发展好快，你怎么看？", "你有在用 AI 工具吗？"],
            interest_tags: &["技术", "AI"],
        },
        TopicNode {
            category: "科技",
            name: "数码",
            prompts: &[
                "最近有入手什么新 gadget 吗？",
                "你喜欢什么数码产品？",
            ],
            interest_tags: &["技术", "数码"],
        },
        TopicNode {
            category: "宠物",
            name: "猫",
            prompts: &["你喜欢猫吗？我也好想养一只~", "你养过猫吗？"],
            interest_tags: &["宠物", "动物"],
        },
        TopicNode {
            category: "宠物",
            name: "狗",
            prompts: &["你喜欢狗吗？", "你养过狗狗吗？"],
            interest_tags: &["宠物", "动物"],
        },
    ]
}

/// 话题树查询接口
pub struct TopicTree;

impl TopicTree {
    /// 所有一级分类
    pub fn categories() -> Vec<&'static str> {
        let mut cats: Vec<&'static str> = Vec::new();
        for node in TOPIC_TREE.iter() {
            if !cats.contains(&node.category) {
                cats.push(node.category);
            }
        }
        cats
    }

    /// 指定分类下的子话题名
    pub fn subcategories(category: &str) -> Vec<&'static str> {
        TOPIC_TREE
            .iter()
            .filter(|n| n.category == category)
            .map(|n| n.name)
            .collect()
    }

    /// 展平的全部话题
    pub fn all_topics() -> &'static [TopicNode] {
        &TOPIC_TREE
    }

    /// 按兴趣标签筛选话题（任一标签命中即入选）
    pub fn filter_by_interest(interest_tags: &[String]) -> Vec<&'static TopicNode> {
        if interest_tags.is_empty() {
            return TOPIC_TREE.iter().collect();
        }
        let lower: Vec<String> = interest_tags.iter().map(|t| t.to_lowercase()).collect();
        TOPIC_TREE
            .iter()
            .filter(|n| {
                n.interest_tags
                    .iter()
                    .any(|t| lower.contains(&t.to_lowercase()))
            })
            .collect()
    }

    /// 随机选一个话题节点（可按兴趣筛选）
    pub fn random_topic(interest_tags: Option<&[String]>) -> Option<&'static TopicNode> {
        let candidates: Vec<&'static TopicNode> = match interest_tags {
            Some(tags) if !tags.is_empty() => Self::filter_by_interest(tags),
            _ => TOPIC_TREE.iter().collect(),
        };
        if candidates.is_empty() {
            return None;
        }
        let idx = random_index(candidates.len());
        Some(candidates[idx])
    }

    /// 随机选一句话题问句
    pub fn random_prompt(interest_tags: Option<&[String]>) -> Option<&'static str> {
        let topic = Self::random_topic(interest_tags)?;
        if topic.prompts.is_empty() {
            return None;
        }
        let idx = random_index(topic.prompts.len());
        Some(topic.prompts[idx])
    }
}
