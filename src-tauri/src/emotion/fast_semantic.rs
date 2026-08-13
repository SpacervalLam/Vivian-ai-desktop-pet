//! 快速语义路由器
//!
//! 在 LLM 主调用前对用户输入进行多维度嵌入分类，输出统一的 FastPerceptionResult，
//! 驱动 prompt 动态组装（注入引导文本、裁剪无关模块、调整工具场景）。
//!
//! 维度：emotion（复用 EmbeddingEmotionClassifier）+ intent + topic + memory_signal + relationship_signal
//! 查询文本只嵌入一次，跨维度复用。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::pipeline::prompt_modules::normalize_lang;

use super::embedding_classifier::EmbeddingEmotionClassifier;
use super::EmotionResult;
use crate::memory::embedding::MemoryEmbeddingProvider;

const SEMANTIC_TOP_K: usize = 3;
const SEMANTIC_THRESHOLD: f32 = 0.35;
const SEMANTIC_CACHE_CAPACITY: usize = 64;
const SEMANTIC_EMBED_CHUNK_SIZE: usize = 128;

// ==================== 语料定义 ====================

#[derive(Debug, Clone)]
struct SemanticEntry {
    text: &'static str,
    label: &'static str,
}

/// intent 语料（中文）
static INTENT_CORPUS_ZH: &[SemanticEntry] = &[
    // chat
    SemanticEntry { text: "在吗", label: "chat" },
    SemanticEntry { text: "聊聊天吧", label: "chat" },
    SemanticEntry { text: "好无聊啊", label: "chat" },
    SemanticEntry { text: "你在干嘛呢", label: "chat" },
    SemanticEntry { text: "今天怎么样", label: "chat" },
    SemanticEntry { text: "嘿", label: "chat" },
    SemanticEntry { text: "早上好", label: "chat" },
    SemanticEntry { text: "在不在", label: "chat" },
    // question
    SemanticEntry { text: "为什么天是蓝的", label: "question" },
    SemanticEntry { text: "这个怎么用", label: "question" },
    SemanticEntry { text: "你能解释一下吗", label: "question" },
    SemanticEntry { text: "什么是量子计算", label: "question" },
    SemanticEntry { text: "帮我看看这个问题", label: "question" },
    SemanticEntry { text: "怎么回事", label: "question" },
    SemanticEntry { text: "为什么会这样", label: "question" },
    SemanticEntry { text: "想问一下", label: "question" },
    // request
    SemanticEntry { text: "帮我写一段代码", label: "request" },
    SemanticEntry { text: "给我推荐一首歌", label: "request" },
    SemanticEntry { text: "帮我查一下", label: "request" },
    SemanticEntry { text: "能不能帮我", label: "request" },
    SemanticEntry { text: "帮我翻译一下", label: "request" },
    SemanticEntry { text: "帮我想想", label: "request" },
    SemanticEntry { text: "给我讲个故事", label: "request" },
    SemanticEntry { text: "帮我整理一下", label: "request" },
    // sharing
    SemanticEntry { text: "今天去了公园", label: "sharing" },
    SemanticEntry { text: "我跟你说", label: "sharing" },
    SemanticEntry { text: "刚才发生了一件事", label: "sharing" },
    SemanticEntry { text: "我终于完成了", label: "sharing" },
    SemanticEntry { text: "今天好累啊", label: "sharing" },
    SemanticEntry { text: "心情不太好", label: "sharing" },
    SemanticEntry { text: "今天遇到个有趣的事", label: "sharing" },
    SemanticEntry { text: "刚下班", label: "sharing" },
    // complaint
    SemanticEntry { text: "烦死了", label: "complaint" },
    SemanticEntry { text: "怎么又这样", label: "complaint" },
    SemanticEntry { text: "受不了了", label: "complaint" },
    SemanticEntry { text: "太坑了", label: "complaint" },
    SemanticEntry { text: "真无语", label: "complaint" },
    SemanticEntry { text: "气死我了", label: "complaint" },
    SemanticEntry { text: "这也太过分了", label: "complaint" },
    SemanticEntry { text: "真的服了", label: "complaint" },
    // goodbye
    SemanticEntry { text: "我要睡了", label: "goodbye" },
    SemanticEntry { text: "晚安", label: "goodbye" },
    SemanticEntry { text: "先忙了", label: "goodbye" },
    SemanticEntry { text: "回头聊", label: "goodbye" },
    SemanticEntry { text: "出门了", label: "goodbye" },
    SemanticEntry { text: "拜拜", label: "goodbye" },
    SemanticEntry { text: "我要去上班了", label: "goodbye" },
    SemanticEntry { text: "下次再聊", label: "goodbye" },
    // tool_request
    SemanticEntry { text: "查一下天气", label: "tool_request" },
    SemanticEntry { text: "放首歌", label: "tool_request" },
    SemanticEntry { text: "搜索一下", label: "tool_request" },
    SemanticEntry { text: "截个图", label: "tool_request" },
    SemanticEntry { text: "帮我记一下", label: "tool_request" },
    SemanticEntry { text: "设个闹钟", label: "tool_request" },
    SemanticEntry { text: "搜一下这个", label: "tool_request" },
    SemanticEntry { text: "帮我打开音乐", label: "tool_request" },
];

/// intent 语料（英文）
static INTENT_CORPUS_EN: &[SemanticEntry] = &[
    // chat
    SemanticEntry { text: "hey", label: "chat" },
    SemanticEntry { text: "what's up", label: "chat" },
    SemanticEntry { text: "how's it going", label: "chat" },
    SemanticEntry { text: "anyone there", label: "chat" },
    SemanticEntry { text: "let's chat", label: "chat" },
    SemanticEntry { text: "i'm so bored", label: "chat" },
    SemanticEntry { text: "good morning", label: "chat" },
    SemanticEntry { text: "hello there", label: "chat" },
    // question
    SemanticEntry { text: "why is the sky blue", label: "question" },
    SemanticEntry { text: "how does this work", label: "question" },
    SemanticEntry { text: "can you explain", label: "question" },
    SemanticEntry { text: "what is quantum computing", label: "question" },
    SemanticEntry { text: "can you look into this", label: "question" },
    SemanticEntry { text: "what's going on", label: "question" },
    SemanticEntry { text: "why did this happen", label: "question" },
    SemanticEntry { text: "i have a question", label: "question" },
    // request
    SemanticEntry { text: "write me some code", label: "request" },
    SemanticEntry { text: "recommend me a song", label: "request" },
    SemanticEntry { text: "look this up for me", label: "request" },
    SemanticEntry { text: "can you help me", label: "request" },
    SemanticEntry { text: "translate this for me", label: "request" },
    SemanticEntry { text: "help me brainstorm", label: "request" },
    SemanticEntry { text: "tell me a story", label: "request" },
    SemanticEntry { text: "help me organize this", label: "request" },
    // sharing
    SemanticEntry { text: "i went to the park today", label: "sharing" },
    SemanticEntry { text: "let me tell you", label: "sharing" },
    SemanticEntry { text: "something just happened", label: "sharing" },
    SemanticEntry { text: "i finally finished it", label: "sharing" },
    SemanticEntry { text: "i'm so tired today", label: "sharing" },
    SemanticEntry { text: "feeling a bit down", label: "sharing" },
    SemanticEntry { text: "something funny happened today", label: "sharing" },
    SemanticEntry { text: "just got off work", label: "sharing" },
    // complaint
    SemanticEntry { text: "this is so annoying", label: "complaint" },
    SemanticEntry { text: "why does this keep happening", label: "complaint" },
    SemanticEntry { text: "i can't take it anymore", label: "complaint" },
    SemanticEntry { text: "this is ridiculous", label: "complaint" },
    SemanticEntry { text: "speechless", label: "complaint" },
    SemanticEntry { text: "i'm so mad", label: "complaint" },
    SemanticEntry { text: "this is too much", label: "complaint" },
    SemanticEntry { text: "seriously over it", label: "complaint" },
    // goodbye
    SemanticEntry { text: "i'm going to sleep", label: "goodbye" },
    SemanticEntry { text: "goodnight", label: "goodbye" },
    SemanticEntry { text: "gotta go", label: "goodbye" },
    SemanticEntry { text: "catch you later", label: "goodbye" },
    SemanticEntry { text: "heading out", label: "goodbye" },
    SemanticEntry { text: "bye", label: "goodbye" },
    SemanticEntry { text: "off to work", label: "goodbye" },
    SemanticEntry { text: "talk later", label: "goodbye" },
    // tool_request
    SemanticEntry { text: "check the weather", label: "tool_request" },
    SemanticEntry { text: "play some music", label: "tool_request" },
    SemanticEntry { text: "search this for me", label: "tool_request" },
    SemanticEntry { text: "take a screenshot", label: "tool_request" },
    SemanticEntry { text: "set a reminder", label: "tool_request" },
    SemanticEntry { text: "set an alarm", label: "tool_request" },
    SemanticEntry { text: "look this up", label: "tool_request" },
    SemanticEntry { text: "play me a song", label: "tool_request" },
];

/// intent 语料（日文）
static INTENT_CORPUS_JA: &[SemanticEntry] = &[
    // chat
    SemanticEntry { text: "いる？", label: "chat" },
    SemanticEntry { text: "話そうよ", label: "chat" },
    SemanticEntry { text: "暇だな", label: "chat" },
    SemanticEntry { text: "何してるの", label: "chat" },
    SemanticEntry { text: "今日どうだった", label: "chat" },
    SemanticEntry { text: "おはよう", label: "chat" },
    SemanticEntry { text: "やあ", label: "chat" },
    SemanticEntry { text: "チャットしよう", label: "chat" },
    // question
    SemanticEntry { text: "空はなぜ青いの", label: "question" },
    SemanticEntry { text: "これどう使うの", label: "question" },
    SemanticEntry { text: "説明してくれる", label: "question" },
    SemanticEntry { text: "量子コンピューティングって何", label: "question" },
    SemanticEntry { text: "これ見てくれる", label: "question" },
    SemanticEntry { text: "どういうこと", label: "question" },
    SemanticEntry { text: "なんでこうなるの", label: "question" },
    SemanticEntry { text: "聞きたいことがある", label: "question" },
    // request
    SemanticEntry { text: "コード書いて", label: "request" },
    SemanticEntry { text: "曲教えて", label: "request" },
    SemanticEntry { text: "調べてほしい", label: "request" },
    SemanticEntry { text: "手伝って", label: "request" },
    SemanticEntry { text: "翻訳して", label: "request" },
    SemanticEntry { text: "一緒に考えて", label: "request" },
    SemanticEntry { text: "物語聞かせて", label: "request" },
    SemanticEntry { text: "まとめて", label: "request" },
    // sharing
    SemanticEntry { text: "今日公園行ったんだ", label: "sharing" },
    SemanticEntry { text: "聞いて", label: "sharing" },
    SemanticEntry { text: "さっき面白いことあった", label: "sharing" },
    SemanticEntry { text: "やっと終わった", label: "sharing" },
    SemanticEntry { text: "今日疲れた", label: "sharing" },
    SemanticEntry { text: "気分がよくない", label: "sharing" },
    SemanticEntry { text: "面白いことあったんだ", label: "sharing" },
    SemanticEntry { text: "仕事終わった", label: "sharing" },
    // complaint
    SemanticEntry { text: "うざい", label: "complaint" },
    SemanticEntry { text: "またこれか", label: "complaint" },
    SemanticEntry { text: "もう無理", label: "complaint" },
    SemanticEntry { text: "ひどすぎ", label: "complaint" },
    SemanticEntry { text: "は？", label: "complaint" },
    SemanticEntry { text: "腹立つ", label: "complaint" },
    SemanticEntry { text: "あまりにもひどい", label: "complaint" },
    SemanticEntry { text: "ほんとむかつく", label: "complaint" },
    // goodbye
    SemanticEntry { text: "寝るね", label: "goodbye" },
    SemanticEntry { text: "おやすみ", label: "goodbye" },
    SemanticEntry { text: "行かなきゃ", label: "goodbye" },
    SemanticEntry { text: "またね", label: "goodbye" },
    SemanticEntry { text: "出かける", label: "goodbye" },
    SemanticEntry { text: "じゃあね", label: "goodbye" },
    SemanticEntry { text: "仕事行ってくる", label: "goodbye" },
    SemanticEntry { text: "また後で", label: "goodbye" },
    // tool_request
    SemanticEntry { text: "天気教えて", label: "tool_request" },
    SemanticEntry { text: "曲流して", label: "tool_request" },
    SemanticEntry { text: "検索して", label: "tool_request" },
    SemanticEntry { text: "スクショ撮って", label: "tool_request" },
    SemanticEntry { text: "メモして", label: "tool_request" },
    SemanticEntry { text: "アラームセットして", label: "tool_request" },
    SemanticEntry { text: "これ調べて", label: "tool_request" },
    SemanticEntry { text: "音楽かけて", label: "tool_request" },
];

/// topic 语料（中文）
static TOPIC_CORPUS_ZH: &[SemanticEntry] = &[
    // daily_life
    SemanticEntry { text: "今天吃了火锅", label: "daily_life" },
    SemanticEntry { text: "刚洗完澡", label: "daily_life" },
    SemanticEntry { text: "今天下雨了", label: "daily_life" },
    SemanticEntry { text: "去超市买了点东西", label: "daily_life" },
    SemanticEntry { text: "今天做了大扫除", label: "daily_life" },
    SemanticEntry { text: "晚饭吃什么好呢", label: "daily_life" },
    SemanticEntry { text: "今天睡到中午", label: "daily_life" },
    // work_study
    SemanticEntry { text: "今天加班到很晚", label: "work_study" },
    SemanticEntry { text: "这个bug好难修", label: "work_study" },
    SemanticEntry { text: "明天有个考试", label: "work_study" },
    SemanticEntry { text: "论文写不完了", label: "work_study" },
    SemanticEntry { text: "开会开了一下午", label: "work_study" },
    SemanticEntry { text: "deadline快到了", label: "work_study" },
    SemanticEntry { text: "老板又改需求了", label: "work_study" },
    SemanticEntry { text: "学了一上午", label: "work_study" },
    // health
    SemanticEntry { text: "今天头有点疼", label: "health" },
    SemanticEntry { text: "最近睡眠不好", label: "health" },
    SemanticEntry { text: "感冒了", label: "health" },
    SemanticEntry { text: "去跑了步", label: "health" },
    SemanticEntry { text: "最近总是很累", label: "health" },
    SemanticEntry { text: "胃不太舒服", label: "health" },
    SemanticEntry { text: "体检结果出来了", label: "health" },
    // gaming
    SemanticEntry { text: "今天又吃鸡了", label: "gaming" },
    SemanticEntry { text: "这关过不去", label: "gaming" },
    SemanticEntry { text: "新出的角色好强", label: "gaming" },
    SemanticEntry { text: "队友太坑了", label: "gaming" },
    SemanticEntry { text: "抽卡又歪了", label: "gaming" },
    SemanticEntry { text: "刚打完一把排位", label: "gaming" },
    SemanticEntry { text: "这个boss太难了", label: "gaming" },
    // relationship
    SemanticEntry { text: "和朋友吵架了", label: "relationship" },
    SemanticEntry { text: "想家了", label: "relationship" },
    SemanticEntry { text: "女朋友生气了", label: "relationship" },
    SemanticEntry { text: "好久没见朋友了", label: "relationship" },
    SemanticEntry { text: "今天被表白了", label: "relationship" },
    SemanticEntry { text: "他为什么不回我消息", label: "relationship" },
    SemanticEntry { text: "和朋友和好了", label: "relationship" },
    // life_event
    SemanticEntry { text: "我考上研究生了", label: "life_event" },
    SemanticEntry { text: "今天是我生日", label: "life_event" },
    SemanticEntry { text: "找到工作了", label: "life_event" },
    SemanticEntry { text: "搬家了", label: "life_event" },
    SemanticEntry { text: "毕业了", label: "life_event" },
    SemanticEntry { text: "领证了", label: "life_event" },
    SemanticEntry { text: "拿到offer了", label: "life_event" },
    // entertainment
    SemanticEntry { text: "看完了一部电影", label: "entertainment" },
    SemanticEntry { text: "这动漫太好看了", label: "entertainment" },
    SemanticEntry { text: "在追一部剧", label: "entertainment" },
    SemanticEntry { text: "今天去听了演唱会", label: "entertainment" },
    SemanticEntry { text: "这本书真不错", label: "entertainment" },
    SemanticEntry { text: "在听一首很好听的歌", label: "entertainment" },
    SemanticEntry { text: "刚看完一部番", label: "entertainment" },
    // technology
    SemanticEntry { text: "这个框架怎么用", label: "technology" },
    SemanticEntry { text: "服务器又挂了", label: "technology" },
    SemanticEntry { text: "写了个新项目", label: "technology" },
    SemanticEntry { text: "学了新的编程语言", label: "technology" },
    SemanticEntry { text: "部署了一下", label: "technology" },
    SemanticEntry { text: "配置了一下环境", label: "technology" },
    SemanticEntry { text: "重构了一下代码", label: "technology" },
];

/// topic 语料（英文）
static TOPIC_CORPUS_EN: &[SemanticEntry] = &[
    // daily_life
    SemanticEntry { text: "had hotpot today", label: "daily_life" },
    SemanticEntry { text: "just took a shower", label: "daily_life" },
    SemanticEntry { text: "it rained today", label: "daily_life" },
    SemanticEntry { text: "went grocery shopping", label: "daily_life" },
    SemanticEntry { text: "did a deep clean today", label: "daily_life" },
    SemanticEntry { text: "what's for dinner", label: "daily_life" },
    SemanticEntry { text: "slept until noon", label: "daily_life" },
    // work_study
    SemanticEntry { text: "worked late today", label: "work_study" },
    SemanticEntry { text: "this bug is hard to fix", label: "work_study" },
    SemanticEntry { text: "got an exam tomorrow", label: "work_study" },
    SemanticEntry { text: "paper's not done", label: "work_study" },
    SemanticEntry { text: "was in meetings all afternoon", label: "work_study" },
    SemanticEntry { text: "deadline's coming up", label: "work_study" },
    SemanticEntry { text: "boss changed the requirements again", label: "work_study" },
    SemanticEntry { text: "been studying all morning", label: "work_study" },
    // health
    SemanticEntry { text: "head hurts today", label: "health" },
    SemanticEntry { text: "not sleeping well lately", label: "health" },
    SemanticEntry { text: "caught a cold", label: "health" },
    SemanticEntry { text: "went for a run", label: "health" },
    SemanticEntry { text: "been feeling tired lately", label: "health" },
    SemanticEntry { text: "stomach's a bit off", label: "health" },
    SemanticEntry { text: "got my checkup results", label: "health" },
    // gaming
    SemanticEntry { text: "won another chicken dinner", label: "gaming" },
    SemanticEntry { text: "can't beat this level", label: "gaming" },
    SemanticEntry { text: "the new character is op", label: "gaming" },
    SemanticEntry { text: "teammates are terrible", label: "gaming" },
    SemanticEntry { text: "lost the 50/50 again", label: "gaming" },
    SemanticEntry { text: "just finished a ranked match", label: "gaming" },
    SemanticEntry { text: "this boss is too hard", label: "gaming" },
    // relationship
    SemanticEntry { text: "got into a fight with a friend", label: "relationship" },
    SemanticEntry { text: "missing home", label: "relationship" },
    SemanticEntry { text: "girlfriend's mad at me", label: "relationship" },
    SemanticEntry { text: "haven't seen friends in ages", label: "relationship" },
    SemanticEntry { text: "got confessed to today", label: "relationship" },
    SemanticEntry { text: "why isn't he texting back", label: "relationship" },
    SemanticEntry { text: "made up with my friend", label: "relationship" },
    // life_event
    SemanticEntry { text: "got into grad school", label: "life_event" },
    SemanticEntry { text: "it's my birthday today", label: "life_event" },
    SemanticEntry { text: "got the job", label: "life_event" },
    SemanticEntry { text: "moved to a new place", label: "life_event" },
    SemanticEntry { text: "graduated", label: "life_event" },
    SemanticEntry { text: "got married", label: "life_event" },
    SemanticEntry { text: "got the offer", label: "life_event" },
    // entertainment
    SemanticEntry { text: "finished a movie", label: "entertainment" },
    SemanticEntry { text: "this anime is so good", label: "entertainment" },
    SemanticEntry { text: "watching a new series", label: "entertainment" },
    SemanticEntry { text: "went to a concert today", label: "entertainment" },
    SemanticEntry { text: "this book is great", label: "entertainment" },
    SemanticEntry { text: "listening to a great song", label: "entertainment" },
    SemanticEntry { text: "just finished an anime", label: "entertainment" },
    // technology
    SemanticEntry { text: "how to use this framework", label: "technology" },
    SemanticEntry { text: "server's down again", label: "technology" },
    SemanticEntry { text: "started a new project", label: "technology" },
    SemanticEntry { text: "learned a new language", label: "technology" },
    SemanticEntry { text: "deployed it", label: "technology" },
    SemanticEntry { text: "set up the environment", label: "technology" },
    SemanticEntry { text: "refactored the code", label: "technology" },
];

/// topic 语料（日文）
static TOPIC_CORPUS_JA: &[SemanticEntry] = &[
    // daily_life
    SemanticEntry { text: "今日鍋食べた", label: "daily_life" },
    SemanticEntry { text: "お風呂入った", label: "daily_life" },
    SemanticEntry { text: "今日雨だった", label: "daily_life" },
    SemanticEntry { text: "スーパー行ってきた", label: "daily_life" },
    SemanticEntry { text: "今日掃除した", label: "daily_life" },
    SemanticEntry { text: "夜ご飯何にしよう", label: "daily_life" },
    SemanticEntry { text: "今日昼まで寝てた", label: "daily_life" },
    // work_study
    SemanticEntry { text: "今日残業した", label: "work_study" },
    SemanticEntry { text: "このバグ難しい", label: "work_study" },
    SemanticEntry { text: "明日試験", label: "work_study" },
    SemanticEntry { text: "論文終わらない", label: "work_study" },
    SemanticEntry { text: "午後ずっと会議", label: "work_study" },
    SemanticEntry { text: "締め切り近い", label: "work_study" },
    SemanticEntry { text: "上司がまた要件変えた", label: "work_study" },
    SemanticEntry { text: "午前中ずっと勉強してた", label: "work_study" },
    // health
    SemanticEntry { text: "今日頭痛い", label: "health" },
    SemanticEntry { text: "最近眠れない", label: "health" },
    SemanticEntry { text: "風邪引いた", label: "health" },
    SemanticEntry { text: "ジョギングした", label: "health" },
    SemanticEntry { text: "最近ずっと疲れる", label: "health" },
    SemanticEntry { text: "胃の調子が悪い", label: "health" },
    SemanticEntry { text: "健康診断の結果来た", label: "health" },
    // gaming
    SemanticEntry { text: "今日も勝った", label: "gaming" },
    SemanticEntry { text: "この面クリアできない", label: "gaming" },
    SemanticEntry { text: "新キャラ強い", label: "gaming" },
    SemanticEntry { text: "味方がひどい", label: "gaming" },
    SemanticEntry { text: "ガチャ外れた", label: "gaming" },
    SemanticEntry { text: "ランク戦終わった", label: "gaming" },
    SemanticEntry { text: "このボス硬すぎ", label: "gaming" },
    // relationship
    SemanticEntry { text: "友達と喧嘩した", label: "relationship" },
    SemanticEntry { text: "実家帰りたい", label: "relationship" },
    SemanticEntry { text: "彼女怒ってる", label: "relationship" },
    SemanticEntry { text: "久しぶりに友達に会いたい", label: "relationship" },
    SemanticEntry { text: "今日告白された", label: "relationship" },
    SemanticEntry { text: "なんで既読つかないの", label: "relationship" },
    SemanticEntry { text: "友達と仲直りした", label: "relationship" },
    // life_event
    SemanticEntry { text: "大学院受かった", label: "life_event" },
    SemanticEntry { text: "今日誕生日", label: "life_event" },
    SemanticEntry { text: "就職決まった", label: "life_event" },
    SemanticEntry { text: "引っ越した", label: "life_event" },
    SemanticEntry { text: "卒業した", label: "life_event" },
    SemanticEntry { text: "結婚した", label: "life_event" },
    SemanticEntry { text: "内定もらった", label: "life_event" },
    // entertainment
    SemanticEntry { text: "映画観終わった", label: "entertainment" },
    SemanticEntry { text: "このアニメ最高", label: "entertainment" },
    SemanticEntry { text: "ドラマ追ってる", label: "entertainment" },
    SemanticEntry { text: "今日ライブ行った", label: "entertainment" },
    SemanticEntry { text: "この本面白い", label: "entertainment" },
    SemanticEntry { text: "いい曲聴いてる", label: "entertainment" },
    SemanticEntry { text: "アニメ観終わった", label: "entertainment" },
    // technology
    SemanticEntry { text: "このフレームワーク使い方", label: "technology" },
    SemanticEntry { text: "サーバー落ちた", label: "technology" },
    SemanticEntry { text: "新しいプロジェクト作った", label: "technology" },
    SemanticEntry { text: "新しい言語学んだ", label: "technology" },
    SemanticEntry { text: "デプロイした", label: "technology" },
    SemanticEntry { text: "環境構築した", label: "technology" },
    SemanticEntry { text: "コードリファクタした", label: "technology" },
];

/// memory importance 语料（中文）
static MEMORY_CORPUS_ZH: &[SemanticEntry] = &[
    // high
    SemanticEntry { text: "我考上研究生了", label: "high" },
    SemanticEntry { text: "今天是我生日", label: "high" },
    SemanticEntry { text: "找到工作了", label: "high" },
    SemanticEntry { text: "领证了", label: "high" },
    SemanticEntry { text: "搬家了", label: "high" },
    SemanticEntry { text: "我失恋了", label: "high" },
    SemanticEntry { text: "毕业了", label: "high" },
    // medium
    SemanticEntry { text: "今天和朋友吃了饭", label: "medium" },
    SemanticEntry { text: "看了一部不错的电影", label: "medium" },
    SemanticEntry { text: "加班到很晚", label: "medium" },
    SemanticEntry { text: "感冒了", label: "medium" },
    SemanticEntry { text: "买了新东西", label: "medium" },
    SemanticEntry { text: "和朋友聊天了", label: "medium" },
    // low
    SemanticEntry { text: "吃了午饭", label: "low" },
    SemanticEntry { text: "今天天气不错", label: "low" },
    SemanticEntry { text: "刚喝了一杯水", label: "low" },
    SemanticEntry { text: "在发呆", label: "low" },
    SemanticEntry { text: "没什么事做", label: "low" },
    SemanticEntry { text: "刚洗完手", label: "low" },
];

/// memory importance 语料（英文）
static MEMORY_CORPUS_EN: &[SemanticEntry] = &[
    // high
    SemanticEntry { text: "got into grad school", label: "high" },
    SemanticEntry { text: "it's my birthday today", label: "high" },
    SemanticEntry { text: "got the job", label: "high" },
    SemanticEntry { text: "got married", label: "high" },
    SemanticEntry { text: "moved to a new place", label: "high" },
    SemanticEntry { text: "broke up with my partner", label: "high" },
    SemanticEntry { text: "graduated", label: "high" },
    // medium
    SemanticEntry { text: "had dinner with a friend", label: "medium" },
    SemanticEntry { text: "watched a good movie", label: "medium" },
    SemanticEntry { text: "worked late", label: "medium" },
    SemanticEntry { text: "caught a cold", label: "medium" },
    SemanticEntry { text: "bought something new", label: "medium" },
    SemanticEntry { text: "chatted with a friend", label: "medium" },
    // low
    SemanticEntry { text: "had lunch", label: "low" },
    SemanticEntry { text: "nice weather today", label: "low" },
    SemanticEntry { text: "just had a glass of water", label: "low" },
    SemanticEntry { text: "just spacing out", label: "low" },
    SemanticEntry { text: "nothing much going on", label: "low" },
    SemanticEntry { text: "just washed my hands", label: "low" },
];

/// memory importance 语料（日文）
static MEMORY_CORPUS_JA: &[SemanticEntry] = &[
    // high
    SemanticEntry { text: "大学院受かった", label: "high" },
    SemanticEntry { text: "今日誕生日", label: "high" },
    SemanticEntry { text: "就職決まった", label: "high" },
    SemanticEntry { text: "結婚した", label: "high" },
    SemanticEntry { text: "引っ越した", label: "high" },
    SemanticEntry { text: "別れた", label: "high" },
    SemanticEntry { text: "卒業した", label: "high" },
    // medium
    SemanticEntry { text: "友達とご飯食べた", label: "medium" },
    SemanticEntry { text: "面白い映画観た", label: "medium" },
    SemanticEntry { text: "残業した", label: "medium" },
    SemanticEntry { text: "風邪引いた", label: "medium" },
    SemanticEntry { text: "新しいの買った", label: "medium" },
    SemanticEntry { text: "友達と話した", label: "medium" },
    // low
    SemanticEntry { text: "お昼食べた", label: "low" },
    SemanticEntry { text: "今日いい天気", label: "low" },
    SemanticEntry { text: "水飲んだ", label: "low" },
    SemanticEntry { text: "ぼーとしてる", label: "low" },
    SemanticEntry { text: "特に何もない", label: "low" },
    SemanticEntry { text: "手洗った", label: "low" },
];

/// relationship signal 语料（中文）
static RELATIONSHIP_CORPUS_ZH: &[SemanticEntry] = &[
    // bond_increase
    SemanticEntry { text: "谢谢你陪我", label: "bond_increase" },
    SemanticEntry { text: "有你在真好", label: "bond_increase" },
    SemanticEntry { text: "你真懂我", label: "bond_increase" },
    SemanticEntry { text: "越来越喜欢和你聊天了", label: "bond_increase" },
    SemanticEntry { text: "你是我最好的朋友", label: "bond_increase" },
    // attention_seek
    SemanticEntry { text: "你怎么不理我", label: "attention_seek" },
    SemanticEntry { text: "你在干嘛呀", label: "attention_seek" },
    SemanticEntry { text: "好无聊快来陪我", label: "attention_seek" },
    SemanticEntry { text: "你怎么不说话了", label: "attention_seek" },
    SemanticEntry { text: "别走开", label: "attention_seek" },
    // gratitude
    SemanticEntry { text: "谢谢你的建议", label: "gratitude" },
    SemanticEntry { text: "多亏了你", label: "gratitude" },
    SemanticEntry { text: "你帮了大忙", label: "gratitude" },
    SemanticEntry { text: "太感谢了", label: "gratitude" },
    SemanticEntry { text: "真的谢谢你", label: "gratitude" },
    // coldness
    SemanticEntry { text: "你好冷淡", label: "coldness" },
    SemanticEntry { text: "不想和你说话了", label: "coldness" },
    SemanticEntry { text: "你变了", label: "coldness" },
    SemanticEntry { text: "随便吧", label: "coldness" },
    SemanticEntry { text: "算了无所谓", label: "coldness" },
];

/// relationship signal 语料（英文）
static RELATIONSHIP_CORPUS_EN: &[SemanticEntry] = &[
    // bond_increase
    SemanticEntry { text: "thanks for being with me", label: "bond_increase" },
    SemanticEntry { text: "glad you're here", label: "bond_increase" },
    SemanticEntry { text: "you really get me", label: "bond_increase" },
    SemanticEntry { text: "enjoying our chats more and more", label: "bond_increase" },
    SemanticEntry { text: "you're my best friend", label: "bond_increase" },
    // attention_seek
    SemanticEntry { text: "why are you ignoring me", label: "attention_seek" },
    SemanticEntry { text: "what are you up to", label: "attention_seek" },
    SemanticEntry { text: "i'm bored, keep me company", label: "attention_seek" },
    SemanticEntry { text: "why so quiet", label: "attention_seek" },
    SemanticEntry { text: "don't go", label: "attention_seek" },
    // gratitude
    SemanticEntry { text: "thanks for the advice", label: "gratitude" },
    SemanticEntry { text: "couldn't have done it without you", label: "gratitude" },
    SemanticEntry { text: "you really helped", label: "gratitude" },
    SemanticEntry { text: "thank you so much", label: "gratitude" },
    SemanticEntry { text: "really appreciate it", label: "gratitude" },
    // coldness
    SemanticEntry { text: "you're being cold", label: "coldness" },
    SemanticEntry { text: "don't feel like talking", label: "coldness" },
    SemanticEntry { text: "you've changed", label: "coldness" },
    SemanticEntry { text: "whatever", label: "coldness" },
    SemanticEntry { text: "never mind", label: "coldness" },
];

/// relationship signal 语料（日文）
static RELATIONSHIP_CORPUS_JA: &[SemanticEntry] = &[
    // bond_increase
    SemanticEntry { text: "いてくれてありがとう", label: "bond_increase" },
    SemanticEntry { text: "いてくれると嬉しい", label: "bond_increase" },
    SemanticEntry { text: "本当に分かってくれる", label: "bond_increase" },
    SemanticEntry { text: "話すの好きになってきた", label: "bond_increase" },
    SemanticEntry { text: "一番の友達だよ", label: "bond_increase" },
    // attention_seek
    SemanticEntry { text: "なんで無視するの", label: "attention_seek" },
    SemanticEntry { text: "何してるの", label: "attention_seek" },
    SemanticEntry { text: "暇だから構って", label: "attention_seek" },
    SemanticEntry { text: "なんで黙ってるの", label: "attention_seek" },
    SemanticEntry { text: "行かないで", label: "attention_seek" },
    // gratitude
    SemanticEntry { text: "アドバイスありがとう", label: "gratitude" },
    SemanticEntry { text: "おかげで助かった", label: "gratitude" },
    SemanticEntry { text: "すごく助かった", label: "gratitude" },
    SemanticEntry { text: "本当にありがとう", label: "gratitude" },
    SemanticEntry { text: "感謝してる", label: "gratitude" },
    // coldness
    SemanticEntry { text: "冷たいね", label: "coldness" },
    SemanticEntry { text: "もう話したくない", label: "coldness" },
    SemanticEntry { text: "変わったね", label: "coldness" },
    SemanticEntry { text: "どうでもいい", label: "coldness" },
    SemanticEntry { text: "別にいい", label: "coldness" },
];

/// 按语言返回 intent 语料
fn intent_corpus(lang: &str) -> &'static [SemanticEntry] {
    match lang {
        "en" => INTENT_CORPUS_EN,
        "ja" => INTENT_CORPUS_JA,
        _ => INTENT_CORPUS_ZH,
    }
}

/// 按语言返回 topic 语料
fn topic_corpus(lang: &str) -> &'static [SemanticEntry] {
    match lang {
        "en" => TOPIC_CORPUS_EN,
        "ja" => TOPIC_CORPUS_JA,
        _ => TOPIC_CORPUS_ZH,
    }
}

/// 按语言返回 memory importance 语料
fn memory_corpus(lang: &str) -> &'static [SemanticEntry] {
    match lang {
        "en" => MEMORY_CORPUS_EN,
        "ja" => MEMORY_CORPUS_JA,
        _ => MEMORY_CORPUS_ZH,
    }
}

/// 按语言返回 relationship signal 语料
fn relationship_corpus(lang: &str) -> &'static [SemanticEntry] {
    match lang {
        "en" => RELATIONSHIP_CORPUS_EN,
        "ja" => RELATIONSHIP_CORPUS_JA,
        _ => RELATIONSHIP_CORPUS_ZH,
    }
}

// ==================== 输出结构 ====================

/// 单维度分类结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionResult {
    pub label: String,
    pub confidence: f64,
}

/// 快速感知结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FastPerceptionResult {
    /// 情绪分类（复用 EmbeddingEmotionClassifier）
    pub emotion: EmotionResult,
    /// 意图：chat / question / request / sharing / complaint / goodbye / tool_request
    pub intent: DimensionResult,
    /// 话题标签（可能多个）
    pub topics: Vec<DimensionResult>,
    /// 记忆重要性：high / medium / low
    pub memory_importance: DimensionResult,
    /// 关系信号：bond_increase / attention_seek / gratitude / coldness / none
    pub relationship_signal: DimensionResult,
    /// 动态引导文本（注入 prompt 的简短指引）
    pub guidance: String,
    /// 建议加载的 prompt 模块
    pub suggested_modules: Vec<String>,
    /// 查询文本的嵌入向量（不序列化，供 ToolSemanticFilter 等下游复用，避免重复嵌入）
    #[serde(skip)]
    pub query_embedding: Arc<Vec<f32>>,
    /// 认知知识需求评估（在 analyze 中同步计算，不额外嵌入）
    #[serde(default)]
    pub epistemic_assessment: EpistemicAssessment,
}

impl Default for FastPerceptionResult {
    fn default() -> Self {
        Self {
            emotion: EmotionResult::neutral(),
            intent: DimensionResult { label: "chat".to_string(), confidence: 0.0 },
            topics: vec![],
            memory_importance: DimensionResult { label: "low".to_string(), confidence: 0.0 },
            relationship_signal: DimensionResult { label: "none".to_string(), confidence: 0.0 },
            guidance: String::new(),
            suggested_modules: vec![],
            query_embedding: Arc::new(Vec::new()),
            epistemic_assessment: EpistemicAssessment::default(),
        }
    }
}

// ==================== 知识需求评估（Epistemic Assessment） ====================

/// 知识需求决策
///
/// 替代单一置信度阈值，从"是否自信"转向"是否需要外部证据"。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KnowledgeDecision {
    /// 不需要搜索，模型已知/常识
    NoSearch,
    /// 可选搜索，模型可能够用但搜索也无妨
    SearchOptional,
    /// 建议搜索，有帮助但非必需
    SearchPreferred,
    /// 必须搜索，缺乏外部事实无法可靠回答
    SearchRequired,
    /// 搜索后仍不确定，需要追问用户澄清
    SearchThenAsk,
}

impl Default for KnowledgeDecision {
    fn default() -> Self {
        Self::NoSearch
    }
}

/// 知识状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum KnowledgeStatus {
    /// 模型已有足够知识
    Known,
    /// 很可能知道
    ProbablyKnown,
    /// 不确定是否知道
    Unknown,
    /// 存在歧义，无法确定指代
    Ambiguous,
    /// 需要外部验证
    RequiresVerification,
    /// 可能是一个网络梗/流行语
    PossiblyMeme,
    /// 可能是一个近期事件
    PossiblyRecent,
}

impl Default for KnowledgeStatus {
    fn default() -> Self {
        Self::Known
    }
}

/// 认知知识需求评估（Epistemic Assessment）
///
/// 多维评估用户输入是否需要外部知识验证，替代单一置信度阈值。
/// 核心问题不是"我有多确定"，而是"为了给出可靠回答，是否需要从外部世界获得证据"。
///
/// 四个核心维度：
/// - semantic_clarity：我理解用户在说什么吗？
/// - factual_dependence：回答这个问题是否依赖外部事实？
/// - temporal_sensitivity：这个事实是否可能随时间变化？
/// - interpretation_risk：如果不搜索，自行解释会不会容易误解用户？
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpistemicAssessment {
    /// 语义清晰度（0~1）：我理解用户在说什么吗？
    /// 低 = 指代不明/语义模糊/无法确定实体
    pub semantic_clarity: f64,
    /// 外部事实依赖度（0~1）：回答这个问题是否依赖外部世界的事实？
    /// 高 = 需要查证外部事实才能回答
    pub factual_dependence: f64,
    /// 时效敏感性（0~1）：这个事实是否可能随时间变化？
    /// 高 = 当前时间敏感，搜索优先
    pub temporal_sensitivity: f64,
    /// 解释风险（0~1）：如果不搜索，自行解释会不会很容易误解用户？
    /// 高 = 歧义/梗/隐喻/荒诞组合，猜错的代价大
    pub interpretation_risk: f64,
    /// 知识缺口（0~1）：模型是否有足够的知识来回答？
    /// 高 = 涉及特定专名/事件/文化背景，模型可能缺乏
    pub knowledge_gap: f64,
    /// 知识状态分类
    pub knowledge_status: KnowledgeStatus,
    /// 最终决策
    pub decision: KnowledgeDecision,
    /// 决策理由（用于日志）
    pub reason: String,
    /// 建议的搜索关键词
    pub search_query: Option<String>,
}

impl Default for EpistemicAssessment {
    fn default() -> Self {
        Self {
            semantic_clarity: 1.0,
            factual_dependence: 0.0,
            temporal_sensitivity: 0.0,
            interpretation_risk: 0.0,
            knowledge_gap: 0.0,
            knowledge_status: KnowledgeStatus::Known,
            decision: KnowledgeDecision::NoSearch,
            reason: String::new(),
            search_query: None,
        }
    }
}

/// 评估认知知识需求
///
/// 纯规则启发式评估，不调用 LLM：
/// - 检测模糊指代（"那个事""你听说了"）
/// - 检测矛盾/荒诞描述（"被东方明珠攻击"）
/// - 检测网络梗/流行语特征
/// - 检测多个专有名词的异常组合
/// - 检测时效性内容（"最近""今天" + 非问候语）
/// - 复用 FastPerceptionResult 的意图/话题维度
pub fn evaluate_epistemic_state(
    input: &str,
    _lang: &str,
    perception: Option<&FastPerceptionResult>,
) -> EpistemicAssessment {
    let trimmed = input.trim();
    let input_len = trimmed.chars().count();

    // 短输入：语义清晰，无外部依赖，无时效性，无歧义
    if input_len < 4 {
        return EpistemicAssessment {
            semantic_clarity: 1.0,
            factual_dependence: 0.0,
            temporal_sensitivity: 0.0,
            interpretation_risk: 0.0,
            knowledge_gap: 0.0,
            knowledge_status: KnowledgeStatus::Known,
            decision: KnowledgeDecision::NoSearch,
            reason: "输入过短，无需搜索".to_string(),
            search_query: None,
        };
    }

    let mut clarity: f64 = 1.0;
    let mut factual: f64 = 0.0;
    let mut temporal: f64 = 0.0;
    let mut risk: f64 = 0.0;
    let mut gap: f64 = 0.0;
    let mut reasons: Vec<String> = Vec::new();

    // 1. 模糊指代检测 → 降低语义清晰度，增加解释风险
    let ambiguity_markers = ["那个事", "那个瓜", "那个视频", "那个新闻", "最近那个", "今天那个", "你听说了", "你知道那个", "这个梗", "那件事"];
    for marker in &ambiguity_markers {
        if trimmed.contains(marker) {
            clarity = (clarity - 0.40).max(0.0);
            risk = (risk + 0.35).min(1.0);
            gap = (gap + 0.25).min(1.0);
            reasons.push(format!("模糊指代: '{}'", marker));
            break;
        }
    }

    // 2. 矛盾/荒诞描述检测 → 提高解释风险
    let contradiction_patterns = [
        ("被", "攻击"), ("被", "炸"), ("被", "曝光"),
    ];
    for (a, b) in &contradiction_patterns {
        if trimmed.contains(a) && trimmed.contains(b) {
            clarity = (clarity - 0.20).max(0.0);
            risk = (risk + 0.30).min(1.0);
            factual = (factual + 0.30).min(1.0);
            reasons.push(format!("疑似矛盾描述: '{}' + '{}'", a, b));
            break;
        }
    }

    // 3. 网络梗/流行语检测 → 提高解释风险，降低语义清晰度
    let meme_patterns = ["梗", "表情包", "热搜", "上热搜", "出圈", "刷屏", "破防", "yyds", "绝绝子", "栓Q", "芭比Q"];
    let meme_hit = meme_patterns.iter().any(|p| trimmed.contains(p));
    if meme_hit {
        clarity = (clarity - 0.15).max(0.0);
        risk = (risk + 0.25).min(1.0);
        factual = (factual + 0.15).min(1.0);
        reasons.push("疑似网络梗/流行语".to_string());
    }

    // 4. 多个专有名词异常组合 → 提高知识缺口
    let proper_nouns = [
        "上海", "北京", "广州", "深圳", "成都", "杭州", "南京", "武汉", "西安", "重庆",
        "虹桥", "浦东", "东方明珠", "外滩", "故宫", "长城", "西湖", "天河",
        "蜜雪冰城", "喜茶", "奈雪", "星巴克", "麦当劳", "肯德基", "海底捞", "瑞幸",
        "B站", "抖音", "快手", "小红书", "微博", "知乎", "贴吧", "豆瓣",
    ];
    let proper_noun_hits: Vec<&str> = proper_nouns.iter().filter(|pn| trimmed.contains(*pn)).copied().collect();
    if proper_noun_hits.len() >= 2 {
        gap = (gap + 0.25).min(1.0);
        factual = (factual + 0.20).min(1.0);
        risk = (risk + 0.15).min(1.0);
        reasons.push(format!("多个专有名词组合: {}", proper_noun_hits.join("+")));
    }

    // 5. 复用 FastPerceptionResult 的意图置信度
    if let Some(fp) = perception {
        if fp.intent.label == "question" && fp.intent.confidence < 0.5 {
            clarity = (clarity - 0.15).max(0.0);
            factual = (factual + 0.15).min(1.0);
            reasons.push(format!("question 意图置信度低: {:.2}", fp.intent.confidence));
        }
        if fp.intent.label == "tool_request" && fp.intent.confidence < 0.5 {
            clarity = (clarity - 0.10).max(0.0);
            reasons.push(format!("tool_request 意图置信度低: {:.2}", fp.intent.confidence));
        }
    }

    // 6. 时效性内容检测 → 提高时效敏感性
    let recency_markers = ["最近", "今天", "刚刚", "昨天", "这周", "本周", "今年", "去年"];
    let greeting_patterns = ["你好", "早上好", "晚上好", "晚安", "嗨", "hello", "hi"];
    let has_recency = recency_markers.iter().any(|m| trimmed.contains(m));
    let is_greeting = greeting_patterns.iter().any(|g| trimmed.to_lowercase().contains(g));
    if has_recency && !is_greeting && input_len > 6 {
        temporal = (temporal + 0.35).min(1.0);
        factual = (factual + 0.20).min(1.0);
        reasons.push("时效性内容可能需要验证".to_string());
    }

    // 7. 复杂问句 + 较长 → 提高知识缺口
    if (trimmed.contains('？') || trimmed.contains('?')) && input_len > 15 {
        factual = (factual + 0.10).min(1.0);
        gap = (gap + 0.10).min(1.0);
        reasons.push("复杂问句".to_string());
    }

    // 8. 实体/名词组合 + 问号 → 强搜索信号（如"英伟达现在市值多少"）
    if proper_noun_hits.len() >= 1 && (trimmed.contains('？') || trimmed.contains('?')) {
        temporal = (temporal + 0.15).min(1.0);
        factual = (factual + 0.25).min(1.0);
        if !has_recency {
            // 专名+问号但不含时效词 → 知识事实，gap 提升
            gap = (gap + 0.20).min(1.0);
        }
    }

    // 确定知识状态
    let knowledge_status = if risk > 0.6 {
        if trimmed.contains("梗") || meme_hit {
            KnowledgeStatus::PossiblyMeme
        } else if has_recency {
            KnowledgeStatus::PossiblyRecent
        } else {
            KnowledgeStatus::Ambiguous
        }
    } else if factual > 0.6 {
        KnowledgeStatus::RequiresVerification
    } else if gap > 0.5 {
        KnowledgeStatus::Unknown
    } else if clarity < 0.6 {
        KnowledgeStatus::Ambiguous
    } else {
        KnowledgeStatus::Known
    };

    // 决策映射
    let decision = if temporal >= 0.7 {
        KnowledgeDecision::SearchRequired
    } else if risk >= 0.7 {
        KnowledgeDecision::SearchRequired
    } else if factual >= 0.7 && gap >= 0.5 {
        KnowledgeDecision::SearchRequired
    } else if clarity < 0.4 {
        KnowledgeDecision::SearchPreferred
    } else if factual >= 0.5 && temporal >= 0.3 {
        KnowledgeDecision::SearchPreferred
    } else if factual >= 0.4 {
        KnowledgeDecision::SearchOptional
    } else {
        KnowledgeDecision::NoSearch
    };

    // 搜索关键词：当决策不低于 SearchPreferred 时提取
    let search_query = if matches!(decision, KnowledgeDecision::SearchRequired | KnowledgeDecision::SearchPreferred) {
        let cleaned = trimmed
            .replace('？', " ")
            .replace('?', " ")
            .replace('！', " ")
            .replace('!', " ")
            .trim()
            .to_string();
        if cleaned.chars().count() >= 4 {
            Some(cleaned)
        } else {
            None
        }
    } else {
        None
    };

    let reason = if reasons.is_empty() {
        "无显著知识需求信号".to_string()
    } else {
        reasons.join("; ")
    };

    EpistemicAssessment {
        semantic_clarity: clarity,
        factual_dependence: factual,
        temporal_sensitivity: temporal,
        interpretation_risk: risk,
        knowledge_gap: gap,
        knowledge_status,
        decision,
        reason,
        search_query,
    }
}

// ==================== 分析器 ====================

/// 快速语义分析器
///
/// 包装 EmbeddingEmotionClassifier，额外提供 intent/topic/memory/relationship 维度。
/// 查询文本只嵌入一次，跨维度复用。
pub struct FastSemanticAnalyzer {
    emotion_classifier: Arc<EmbeddingEmotionClassifier>,
    provider: Arc<dyn MemoryEmbeddingProvider>,
    language: String,
    // 各维度语料嵌入（懒初始化）
    intent_embeddings: Mutex<Option<Vec<Vec<f32>>>>,
    topic_embeddings: Mutex<Option<Vec<Vec<f32>>>>,
    memory_embeddings: Mutex<Option<Vec<Vec<f32>>>>,
    relationship_embeddings: Mutex<Option<Vec<Vec<f32>>>>,
    init_in_progress: AtomicBool,
    query_cache: Mutex<VecDeque<(String, FastPerceptionResult)>>,
}

impl FastSemanticAnalyzer {
    pub fn new(
        emotion_classifier: Arc<EmbeddingEmotionClassifier>,
        provider: Arc<dyn MemoryEmbeddingProvider>,
        language: String,
    ) -> Self {
        Self {
            emotion_classifier,
            provider,
            language,
            intent_embeddings: Mutex::new(None),
            topic_embeddings: Mutex::new(None),
            memory_embeddings: Mutex::new(None),
            relationship_embeddings: Mutex::new(None),
            init_in_progress: AtomicBool::new(false),
            query_cache: Mutex::new(VecDeque::with_capacity(SEMANTIC_CACHE_CAPACITY)),
        }
    }

    /// 分析主入口
    pub fn analyze(&self, text: &str) -> Result<FastPerceptionResult, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(FastPerceptionResult::default());
        }

        // 查缓存
        if let Some(result) = self.get_cached(trimmed) {
            return Ok(result);
        }

        // 情绪分类（复用 EmbeddingEmotionClassifier，它有自己的缓存和语料初始化）
        let emotion = self.emotion_classifier.classify(trimmed)?;

        // 确保语义维度语料嵌入已初始化
        self.ensure_semantic_initialized()?;

        // 嵌入查询文本（一次嵌入，多维度复用 + 供下游 ToolSemanticFilter 复用）
        let query_emb = self.provider.embed(trimmed).map_err(|e| {
            format!("嵌入服务调用失败: {}", e)
        })?;

        // 各维度分类
        let intent = self.classify_dimension(&query_emb, intent_corpus(&self.language), &self.intent_embeddings);
        let topics = self.classify_dimension_multi(&query_emb, topic_corpus(&self.language), &self.topic_embeddings, 3, 0.3);
        let memory_importance = self.classify_dimension(&query_emb, memory_corpus(&self.language), &self.memory_embeddings);
        let relationship = self.classify_dimension(&query_emb, relationship_corpus(&self.language), &self.relationship_embeddings);

        // 生成引导文本
        let guidance = generate_guidance(&self.language, &emotion, &intent, &topics, &memory_importance, &relationship);

        // 推荐模块
        let suggested_modules = suggest_modules(&intent, &topics, &relationship);

        // 认知知识需求评估（纯规则，不额外嵌入）
        let epistemic_assessment = evaluate_epistemic_state(
            trimmed,
            &self.language,
            None,
        );

        let result = FastPerceptionResult {
            emotion,
            intent,
            topics,
            memory_importance,
            relationship_signal: relationship,
            guidance,
            suggested_modules,
            query_embedding: Arc::new(query_emb),
            epistemic_assessment,
        };

        self.put_cache(trimmed.to_string(), result.clone());
        Ok(result)
    }

    /// 单标签分类（Top-K softmax 投票）
    fn classify_dimension(
        &self,
        query_emb: &[f32],
        corpus: &[SemanticEntry],
        embeddings_lock: &Mutex<Option<Vec<Vec<f32>>>>,
    ) -> DimensionResult {
        let embeddings = embeddings_lock.lock();
        let embeddings = match embeddings.as_ref() {
            Some(e) => e,
            None => return DimensionResult { label: "unknown".to_string(), confidence: 0.0 },
        };

        let mut sims: Vec<(usize, f32)> = embeddings
            .iter()
            .enumerate()
            .map(|(i, emb)| (i, cosine_similarity(query_emb, emb)))
            .collect();
        sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let top_k: Vec<(usize, f32)> = sims.into_iter().take(SEMANTIC_TOP_K).collect();

        let dominant_sim = top_k.first().map(|(_, s)| *s).unwrap_or(0.0);
        if dominant_sim < SEMANTIC_THRESHOLD {
            return DimensionResult { label: "unknown".to_string(), confidence: 0.0 };
        }

        // softmax 加权投票
        let mut votes: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
        let mut total_weight = 0.0f32;
        for (idx, sim) in &top_k {
            if *sim < SEMANTIC_THRESHOLD { break; }
            let label = corpus[*idx].label;
            let weight = (sim / 0.1).exp();
            *votes.entry(label).or_insert(0.0) += weight;
            total_weight += weight;
        }

        let (winner, winner_weight) = votes
            .into_iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(l, w)| (l, w))
            .unwrap_or(("unknown", 0.0));

        let confidence = if total_weight > 0.0 {
            (winner_weight / total_weight) as f64
        } else {
            0.0
        };

        DimensionResult {
            label: winner.to_string(),
            confidence,
        }
    }

    /// 多标签分类（返回 Top-N 标签）
    fn classify_dimension_multi(
        &self,
        query_emb: &[f32],
        corpus: &[SemanticEntry],
        embeddings_lock: &Mutex<Option<Vec<Vec<f32>>>>,
        max_labels: usize,
        min_sim: f32,
    ) -> Vec<DimensionResult> {
        let embeddings = embeddings_lock.lock();
        let embeddings = match embeddings.as_ref() {
            Some(e) => e,
            None => return vec![],
        };

        let mut sims: Vec<(usize, f32)> = embeddings
            .iter()
            .enumerate()
            .map(|(i, emb)| (i, cosine_similarity(query_emb, emb)))
            .collect();
        sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // 按标签聚合最高相似度
        let mut label_best: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
        for (idx, sim) in &sims {
            if *sim < min_sim { break; }
            let label = corpus[*idx].label;
            let entry = label_best.entry(label).or_insert(0.0);
            if *sim > *entry { *entry = *sim; }
        }

        let mut sorted: Vec<(&str, f32)> = label_best.into_iter().collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        sorted
            .into_iter()
            .take(max_labels)
            .map(|(label, sim)| DimensionResult {
                label: label.to_string(),
                confidence: sim as f64,
            })
            .collect()
    }

    fn ensure_semantic_initialized(&self) -> Result<(), String> {
        if self.intent_embeddings.lock().is_some()
            && self.topic_embeddings.lock().is_some()
            && self.memory_embeddings.lock().is_some()
            && self.relationship_embeddings.lock().is_some()
        {
            return Ok(());
        }

        if self.init_in_progress.swap(true, Ordering::AcqRel) {
            return Err("语义语料嵌入正在初始化中，请稍后重试".to_string());
        }

        let result = self.init_all_embeddings();
        self.init_in_progress.store(false, Ordering::Release);
        result
    }

    fn init_all_embeddings(&self) -> Result<(), String> {
        macro_rules! init_dim {
            ($corpus:expr, $lock:expr, $name:expr) => {
                if $lock.lock().is_none() {
                    let texts: Vec<String> = $corpus.iter().map(|e| e.text.to_string()).collect();
                    let embs = self.provider
                        .embed_batch_chunked(&texts, SEMANTIC_EMBED_CHUNK_SIZE, &|_, _| {})
                        .map_err(|e| format!("嵌入语料失败 ({}): {}", $name, e))?;
                    *$lock.lock() = Some(embs);
                    tracing::info!("[FastSemantic] {} 语料嵌入完成: {} 条", $name, texts.len());
                }
            };
        }

        init_dim!(intent_corpus(&self.language), self.intent_embeddings, "intent");
        init_dim!(topic_corpus(&self.language), self.topic_embeddings, "topic");
        init_dim!(memory_corpus(&self.language), self.memory_embeddings, "memory");
        init_dim!(relationship_corpus(&self.language), self.relationship_embeddings, "relationship");

        Ok(())
    }

    fn get_cached(&self, text: &str) -> Option<FastPerceptionResult> {
        let mut cache = self.query_cache.lock();
        if let Some(pos) = cache.iter().position(|(t, _)| t == text) {
            let (key, result) = cache.remove(pos).unwrap();
            cache.push_back((key, result.clone()));
            Some(result)
        } else {
            None
        }
    }

    fn put_cache(&self, text: String, result: FastPerceptionResult) {
        let mut cache = self.query_cache.lock();
        if cache.len() >= SEMANTIC_CACHE_CAPACITY {
            cache.pop_front();
        }
        cache.push_back((text, result));
    }
}

// ==================== 引导文本生成 ====================

fn generate_guidance(
    lang: &str,
    emotion: &EmotionResult,
    intent: &DimensionResult,
    topics: &[DimensionResult],
    memory: &DimensionResult,
    relationship: &DimensionResult,
) -> String {
    let lang = normalize_lang(lang);
    let mut parts: Vec<String> = vec![];

    // 情绪引导
    match emotion.emotion.as_str() {
        "sad" | "disappointed" | "frustrated" => {
            parts.push(match lang {
                "en" => "User mood is low, prioritize companionship and listening, avoid lecturing",
                "ja" => "ユーザーの気分が沈んでいる、寄り添いと傾聴を優先し、説教は避ける",
                _ => "用户情绪偏低，优先陪伴和倾听，避免说教",
            }.to_string());
        }
        "angry" => {
            parts.push(match lang {
                "en" => "User is angry, stay patient, validate emotions before discussing issues",
                "ja" => "ユーザーが怒っている、辛抱強く、まず感情を受け止めてから問題を話す",
                _ => "用户在生气，保持耐心，先认同情绪再讨论问题",
            }.to_string());
        }
        "anxious" => {
            parts.push(match lang {
                "en" => "User seems anxious, focus on reassurance, avoid adding pressure",
                "ja" => "ユーザーが不安そう、安心感を優先し、プレッシャーを増やさない",
                _ => "用户有些焦虑，安抚为主，避免增加压力",
            }.to_string());
        }
        "happy" | "excited" | "grateful" => {
            parts.push(match lang {
                "en" => "User is in a good mood, respond in a relaxed and cheerful way",
                "ja" => "ユーザーの気分が良い、 relaxed で明るいトーンで応答する",
                _ => "用户心情不错，可以轻松愉快地回应",
            }.to_string());
        }
        "tired" | "bored" => {
            parts.push(match lang {
                "en" => "User seems tired, keep replies concise, avoid long messages",
                "ja" => "ユーザーが疲れている、簡潔に返し、長文は避ける",
                _ => "用户有些疲惫，回复简洁些，避免长篇大论",
            }.to_string());
        }
        _ => {}
    }

    // 意图引导
    match intent.label.as_str() {
        "sharing" => {
            parts.push(match lang {
                "en" => "User is sharing something, listen attentively and give positive feedback",
                "ja" => "ユーザーが共有している、しっかり聞いてポジティブなフィードバックを",
                _ => "用户在分享，认真倾听并给予积极反馈",
            }.to_string());
        }
        "complaint" => {
            parts.push(match lang {
                "en" => "User is venting, empathize first, don't rush to give advice",
                "ja" => "ユーザーが愚痴を言っている、まず共感し、すぐにアドバイスをしない",
                _ => "用户在抱怨，共情优先，不要急于给建议",
            }.to_string());
        }
        "goodbye" => {
            parts.push(match lang {
                "en" => "User is leaving, say a warm goodbye, maybe mention seeing them again",
                "ja" => "ユーザーが退出しようとしている、温かく見送り、次の再会に触れても良い",
                _ => "用户要离开了，温暖道别，可以提及下次见面",
            }.to_string());
        }
        "tool_request" => {
            parts.push(match lang {
                "en" => "User has a clear tool request, help them get it done directly",
                "ja" => "ユーザーに明確なツール要求がある、直接サポートして完了させる",
                _ => "用户有明确的工具需求，直接协助完成",
            }.to_string());
        }
        "question" => {
            parts.push(match lang {
                "en" => "User is asking a question, give a clear and accurate answer",
                "ja" => "ユーザーが質問している、分かりやすく正確な回答をする",
                _ => "用户在提问，给出清晰准确的回答",
            }.to_string());
        }
        _ => {}
    }

    // 话题引导
    if let Some(first_topic) = topics.first() {
        match first_topic.label.as_str() {
            "life_event" => {
                parts.push(match lang {
                    "en" => "This is a significant life event for the user, show that you care and consider remembering this moment",
                    "ja" => "ユーザーの重要なライフイベント、重視する姿勢を示し、この瞬間を記憶することを検討する",
                    _ => "这是用户的重要人生事件，应表现出重视，考虑记住这个时刻",
                }.to_string());
            }
            "health" => {
                parts.push(match lang {
                    "en" => "User mentioned a health-related topic, express care",
                    "ja" => "ユーザーが健康関連の話題に触れた、気遣いを示す",
                    _ => "用户提到健康相关话题，表达关心",
                }.to_string());
            }
            "relationship" => {
                parts.push(match lang {
                    "en" => "User mentioned interpersonal relationships, listen patiently and avoid judging",
                    "ja" => "ユーザーが人間関係に触れた、辛抱強く聞き、安易に評価しない",
                    _ => "用户提到人际关系，耐心倾听，不要随意评判",
                }.to_string());
            }
            _ => {}
        }
    }

    // 记忆重要性引导
    if memory.label == "high" {
        parts.push(match lang {
            "en" => "This information is important, consider saving it to long-term memory",
            "ja" => "この情報は重要、長期記憶への保存を検討する",
            _ => "这个信息很重要，建议写入长期记忆",
        }.to_string());
    }

    // 关系信号引导
    match relationship.label.as_str() {
        "bond_increase" => {
            parts.push(match lang {
                "en" => "User is expressing closeness, respond warmly",
                "ja" => "ユーザーが親近感を示している、温かく応答する",
                _ => "用户在表达亲近，温暖回应",
            }.to_string());
        }
        "attention_seek" => {
            parts.push(match lang {
                "en" => "User is seeking attention, be more proactive and enthusiastic",
                "ja" => "ユーザーが注目を求めている、主动的にもっと熱心に対応する",
                _ => "用户在寻求关注，主动热情一些",
            }.to_string());
        }
        "gratitude" => {
            parts.push(match lang {
                "en" => "User is expressing gratitude, respond modestly",
                "ja" => "ユーザーが感謝を伝えている、謙虚に応答する",
                _ => "用户在表达感谢，谦虚回应",
            }.to_string());
        }
        "coldness" => {
            parts.push(match lang {
                "en" => "User seems distant, don't be overly enthusiastic, give some space",
                "ja" => "ユーザーが少し冷淡、過度に熱心にならず、距離を保つ",
                _ => "用户有些冷淡，不要过度热情，给彼此空间",
            }.to_string());
        }
        _ => {}
    }

    if parts.is_empty() {
        String::new()
    } else {
        let sep = match lang {
            "en" => "; ",
            "ja" => "；",
            _ => "；",
        };
        parts.join(sep)
    }
}

fn suggest_modules(
    intent: &DimensionResult,
    topics: &[DimensionResult],
    relationship: &DimensionResult,
) -> Vec<String> {
    let mut modules = vec!["persona".to_string()]; // 人格永远加载

    // 情绪/关系相关模块
    if relationship.label != "none" && relationship.confidence > 0.3 {
        modules.push("relationship".to_string());
    }

    // 记忆模块
    let has_life_event = topics.iter().any(|t| t.label == "life_event");
    if has_life_event || intent.label == "sharing" {
        modules.push("memory_check".to_string());
    }

    // 工具模块
    if intent.label == "tool_request" {
        modules.push("tools".to_string());
    }

    // 事件庆祝
    if has_life_event {
        modules.push("celebration".to_string());
    }

    modules
}

// ==================== 工具函数 ====================

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na < 1e-10 || nb < 1e-10 {
        0.0
    } else {
        dot / (na * nb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_analyzer() -> FastSemanticAnalyzer {
        let provider: Arc<dyn MemoryEmbeddingProvider> = Arc::new(
            crate::memory::embedding::HashingMemoryEmbedding::new(256),
        );
        let emotion_clf = Arc::new(EmbeddingEmotionClassifier::new(provider.clone(), "zh".to_string()));
        FastSemanticAnalyzer::new(emotion_clf, provider, "zh".to_string())
    }

    #[test]
    fn test_intent_corpus_size() {
        assert!(intent_corpus("zh").len() >= 50, "intent zh 语料不足: {}", intent_corpus("zh").len());
        assert!(intent_corpus("en").len() >= 50, "intent en 语料不足: {}", intent_corpus("en").len());
        assert!(intent_corpus("ja").len() >= 50, "intent ja 语料不足: {}", intent_corpus("ja").len());
    }

    #[test]
    fn test_topic_corpus_size() {
        assert!(topic_corpus("zh").len() >= 50, "topic zh 语料不足: {}", topic_corpus("zh").len());
        assert!(topic_corpus("en").len() >= 50, "topic en 语料不足: {}", topic_corpus("en").len());
        assert!(topic_corpus("ja").len() >= 50, "topic ja 语料不足: {}", topic_corpus("ja").len());
    }

    #[test]
    fn test_memory_corpus_size() {
        assert!(memory_corpus("zh").len() >= 15, "memory zh 语料不足: {}", memory_corpus("zh").len());
        assert!(memory_corpus("en").len() >= 15, "memory en 语料不足: {}", memory_corpus("en").len());
        assert!(memory_corpus("ja").len() >= 15, "memory ja 语料不足: {}", memory_corpus("ja").len());
    }

    #[test]
    fn test_relationship_corpus_size() {
        assert!(relationship_corpus("zh").len() >= 15, "relationship zh 语料不足: {}", relationship_corpus("zh").len());
        assert!(relationship_corpus("en").len() >= 15, "relationship en 语料不足: {}", relationship_corpus("en").len());
        assert!(relationship_corpus("ja").len() >= 15, "relationship ja 语料不足: {}", relationship_corpus("ja").len());
    }

    #[test]
    fn test_corpus_lang_fallback() {
        assert_eq!(intent_corpus("zh").len(), intent_corpus("unknown").len());
        assert_eq!(topic_corpus("zh").len(), topic_corpus("fr").len());
        assert_eq!(memory_corpus("zh").len(), memory_corpus("").len());
        assert_eq!(relationship_corpus("zh").len(), relationship_corpus("xyz").len());
    }

    #[test]
    fn test_default_result() {
        let r = FastPerceptionResult::default();
        assert_eq!(r.intent.label, "chat");
        assert_eq!(r.memory_importance.label, "low");
        assert_eq!(r.relationship_signal.label, "none");
        assert!(r.guidance.is_empty());
    }

    #[test]
    fn test_empty_input_returns_default() {
        let analyzer = make_analyzer();
        let result = analyzer.analyze("").unwrap();
        assert_eq!(result.emotion.emotion, "neutral");
    }

    #[test]
    fn test_guidance_generation() {
        let emotion = EmotionResult {
            emotion: "sad".to_string(),
            intensity: 0.7,
            ..Default::default()
        };
        let intent = DimensionResult { label: "sharing".to_string(), confidence: 0.8 };
        let topics = vec![DimensionResult { label: "health".to_string(), confidence: 0.7 }];
        let memory = DimensionResult { label: "high".to_string(), confidence: 0.6 };
        let relationship = DimensionResult { label: "none".to_string(), confidence: 0.0 };

        let guidance = generate_guidance("zh", &emotion, &intent, &topics, &memory, &relationship);
        assert!(guidance.contains("陪伴"));
        assert!(guidance.contains("倾听"));
        assert!(guidance.contains("长期记忆"));
    }

    #[test]
    fn test_suggest_modules() {
        let intent = DimensionResult { label: "tool_request".to_string(), confidence: 0.9 };
        let topics = vec![DimensionResult { label: "life_event".to_string(), confidence: 0.8 }];
        let relationship = DimensionResult { label: "bond_increase".to_string(), confidence: 0.7 };

        let modules = suggest_modules(&intent, &topics, &relationship);
        assert!(modules.contains(&"persona".to_string()));
        assert!(modules.contains(&"tools".to_string()));
        assert!(modules.contains(&"celebration".to_string()));
        assert!(modules.contains(&"relationship".to_string()));
    }
}
