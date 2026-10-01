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

use once_cell::sync::Lazy;
use regex::Regex;

use crate::pipeline::prompt_modules::normalize_lang;

use super::embedding_classifier::EmbeddingEmotionClassifier;
use super::EmotionResult;
use crate::memory::embedding::MemoryEmbeddingProvider;

const SEMANTIC_TOP_K: usize = 3;
const SEMANTIC_THRESHOLD: f32 = 0.35;
pub(crate) const PROMPT_ROUTING_CONFIDENCE: f64 = 0.4;
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
    SemanticEntry { text: "陪我说说话", label: "chat" },
    SemanticEntry { text: "今天想随便聊聊", label: "chat" },
    SemanticEntry { text: "给我讲讲你最近在做什么", label: "chat" },
    SemanticEntry { text: "你觉得周末做什么好", label: "chat" },
    SemanticEntry { text: "我来找你闲聊一会儿", label: "chat" },
    SemanticEntry { text: "为什么有些金属会生锈", label: "question" },
    SemanticEntry { text: "这两个方案有什么区别", label: "question" },
    SemanticEntry { text: "如果改成这样会发生什么", label: "question" },
    SemanticEntry { text: "你怎么看这件事", label: "question" },
    SemanticEntry { text: "能说明一下这个词的意思吗", label: "question" },
    SemanticEntry { text: "请把这段话改得自然一点", label: "request" },
    SemanticEntry { text: "帮我列一个可执行的计划", label: "request" },
    SemanticEntry { text: "把重点压缩成三句话", label: "request" },
    SemanticEntry { text: "替我检查一下这份清单", label: "request" },
    SemanticEntry { text: "给这封邮件拟个回复", label: "request" },
    SemanticEntry { text: "我今天第一次自己做了咖喱", label: "sharing" },
    SemanticEntry { text: "刚才在路上碰到以前的同学", label: "sharing" },
    SemanticEntry { text: "最近开始每天散步了", label: "sharing" },
    SemanticEntry { text: "昨晚看了一场特别精彩的比赛", label: "sharing" },
    SemanticEntry { text: "我想和你说说家里的近况", label: "sharing" },
    SemanticEntry { text: "等了半天结果还是被取消了", label: "complaint" },
    SemanticEntry { text: "明明按说明操作还是报错", label: "complaint" },
    SemanticEntry { text: "排了这么久队也太折腾了", label: "complaint" },
    SemanticEntry { text: "每次都临时通知真的很烦", label: "complaint" },
    SemanticEntry { text: "这个服务比上次还差", label: "complaint" },
    SemanticEntry { text: "我先去洗漱，今天聊到这里", label: "goodbye" },
    SemanticEntry { text: "要赶车了，晚点再说", label: "goodbye" },
    SemanticEntry { text: "我去开会了，回头见", label: "goodbye" },
    SemanticEntry { text: "今天先这样，我要休息了", label: "goodbye" },
    SemanticEntry { text: "到家后再聊", label: "goodbye" },
    SemanticEntry { text: "把屏幕亮度调低一点", label: "tool_request" },
    SemanticEntry { text: "创建一个明天下午的提醒", label: "tool_request" },
    SemanticEntry { text: "把刚才那件事记到备忘录", label: "tool_request" },
    SemanticEntry { text: "打开浏览器搜这家店", label: "tool_request" },
    SemanticEntry { text: "暂停一下现在播放的音乐", label: "tool_request" },
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
    SemanticEntry { text: "keep me company for a bit", label: "chat" },
    SemanticEntry { text: "I just feel like talking", label: "chat" },
    SemanticEntry { text: "tell me what you've been up to", label: "chat" },
    SemanticEntry { text: "what should we do this weekend", label: "chat" },
    SemanticEntry { text: "I'm here for a casual chat", label: "chat" },
    SemanticEntry { text: "why does metal rust", label: "question" },
    SemanticEntry { text: "what's the difference between these options", label: "question" },
    SemanticEntry { text: "what would happen if we changed this", label: "question" },
    SemanticEntry { text: "what do you think about this", label: "question" },
    SemanticEntry { text: "what does this phrase mean", label: "question" },
    SemanticEntry { text: "make this paragraph sound more natural", label: "request" },
    SemanticEntry { text: "make me a practical step-by-step plan", label: "request" },
    SemanticEntry { text: "summarize the key points in three lines", label: "request" },
    SemanticEntry { text: "check this list for me", label: "request" },
    SemanticEntry { text: "draft a reply to this email", label: "request" },
    SemanticEntry { text: "I made curry from scratch for the first time", label: "sharing" },
    SemanticEntry { text: "I ran into an old classmate on the way home", label: "sharing" },
    SemanticEntry { text: "I've started taking a walk every day", label: "sharing" },
    SemanticEntry { text: "I watched an incredible game last night", label: "sharing" },
    SemanticEntry { text: "I want to tell you how things are at home", label: "sharing" },
    SemanticEntry { text: "After waiting all day they canceled it anyway", label: "complaint" },
    SemanticEntry { text: "I followed the instructions and it still errors", label: "complaint" },
    SemanticEntry { text: "That queue took forever", label: "complaint" },
    SemanticEntry { text: "They always give us notice at the last minute", label: "complaint" },
    SemanticEntry { text: "The service is worse than last time", label: "complaint" },
    SemanticEntry { text: "I'm going to get ready for bed, let's stop here", label: "goodbye" },
    SemanticEntry { text: "I have to catch a train, talk later", label: "goodbye" },
    SemanticEntry { text: "I'm heading into a meeting, see you after", label: "goodbye" },
    SemanticEntry { text: "That's enough for today, I need to rest", label: "goodbye" },
    SemanticEntry { text: "I'll message you when I get home", label: "goodbye" },
    SemanticEntry { text: "turn the screen brightness down", label: "tool_request" },
    SemanticEntry { text: "remind me tomorrow afternoon", label: "tool_request" },
    SemanticEntry { text: "save that in my notes", label: "tool_request" },
    SemanticEntry { text: "open a browser and find this shop", label: "tool_request" },
    SemanticEntry { text: "pause the music that's playing", label: "tool_request" },
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
    SemanticEntry { text: "少し話し相手になって", label: "chat" },
    SemanticEntry { text: "なんとなくおしゃべりしたい", label: "chat" },
    SemanticEntry { text: "最近何してるか聞かせて", label: "chat" },
    SemanticEntry { text: "週末何しようか", label: "chat" },
    SemanticEntry { text: "ちょっと雑談しに来た", label: "chat" },
    SemanticEntry { text: "金属が錆びるのはなぜ", label: "question" },
    SemanticEntry { text: "この二つはどう違うの", label: "question" },
    SemanticEntry { text: "これを変えたらどうなる", label: "question" },
    SemanticEntry { text: "このことどう思う", label: "question" },
    SemanticEntry { text: "この言葉の意味を教えて", label: "question" },
    SemanticEntry { text: "この文章を自然な表現に直して", label: "request" },
    SemanticEntry { text: "実行できる計画を立てて", label: "request" },
    SemanticEntry { text: "要点を三行にまとめて", label: "request" },
    SemanticEntry { text: "このリストを確認して", label: "request" },
    SemanticEntry { text: "このメールの返信を書いて", label: "request" },
    SemanticEntry { text: "初めてカレーを自分で作った", label: "sharing" },
    SemanticEntry { text: "帰り道で昔の同級生に会った", label: "sharing" },
    SemanticEntry { text: "最近毎日散歩してる", label: "sharing" },
    SemanticEntry { text: "昨日すごく面白い試合を見た", label: "sharing" },
    SemanticEntry { text: "家の近況を聞いてほしい", label: "sharing" },
    SemanticEntry { text: "ずっと待ったのに結局中止になった", label: "complaint" },
    SemanticEntry { text: "説明通りにしたのにエラーが出る", label: "complaint" },
    SemanticEntry { text: "列が長すぎて疲れた", label: "complaint" },
    SemanticEntry { text: "いつも直前に知らせてくる", label: "complaint" },
    SemanticEntry { text: "前回よりサービスがひどい", label: "complaint" },
    SemanticEntry { text: "そろそろ寝る支度するね", label: "goodbye" },
    SemanticEntry { text: "電車に乗るからまた後で", label: "goodbye" },
    SemanticEntry { text: "会議に行ってくる、またね", label: "goodbye" },
    SemanticEntry { text: "今日はここまで、休むね", label: "goodbye" },
    SemanticEntry { text: "家に着いたら連絡する", label: "goodbye" },
    SemanticEntry { text: "画面の明るさを下げて", label: "tool_request" },
    SemanticEntry { text: "明日の午後にリマインダーを設定して", label: "tool_request" },
    SemanticEntry { text: "さっきのことをメモに残して", label: "tool_request" },
    SemanticEntry { text: "ブラウザでこの店を検索して", label: "tool_request" },
    SemanticEntry { text: "再生中の音楽を止めて", label: "tool_request" },
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
    SemanticEntry { text: "早上买了豆浆和包子", label: "daily_life" },
    SemanticEntry { text: "周末准备去菜市场", label: "daily_life" },
    SemanticEntry { text: "最近在收拾房间", label: "daily_life" },
    SemanticEntry { text: "今天坐公交通勤", label: "daily_life" },
    SemanticEntry { text: "晚饭自己煮了面", label: "daily_life" },
    SemanticEntry { text: "今天做代码评审", label: "work_study" },
    SemanticEntry { text: "准备明天的演讲稿", label: "work_study" },
    SemanticEntry { text: "刚交完课程作业", label: "work_study" },
    SemanticEntry { text: "在学新的数学章节", label: "work_study" },
    SemanticEntry { text: "项目需要补测试", label: "work_study" },
    SemanticEntry { text: "最近开始吃维生素", label: "health" },
    SemanticEntry { text: "预约了牙医检查", label: "health" },
    SemanticEntry { text: "今天去做了理疗", label: "health" },
    SemanticEntry { text: "膝盖跑步后有点不舒服", label: "health" },
    SemanticEntry { text: "最近在调整作息", label: "health" },
    SemanticEntry { text: "今晚和朋友开黑", label: "gaming" },
    SemanticEntry { text: "刚抽到限定角色", label: "gaming" },
    SemanticEntry { text: "这个赛季在冲段位", label: "gaming" },
    SemanticEntry { text: "新地图的机制很复杂", label: "gaming" },
    SemanticEntry { text: "公会今晚要打团本", label: "gaming" },
    SemanticEntry { text: "最近和室友相处不太顺", label: "relationship" },
    SemanticEntry { text: "准备回家看看父母", label: "relationship" },
    SemanticEntry { text: "我和伴侣在讨论未来", label: "relationship" },
    SemanticEntry { text: "朋友最近搬到别的城市了", label: "relationship" },
    SemanticEntry { text: "和姐姐聊了小时候的事", label: "relationship" },
    SemanticEntry { text: "下个月要办婚礼", label: "life_event" },
    SemanticEntry { text: "孩子今天第一天上学", label: "life_event" },
    SemanticEntry { text: "刚通过驾照考试", label: "life_event" },
    SemanticEntry { text: "家里添了一个新成员", label: "life_event" },
    SemanticEntry { text: "准备开始一份新工作", label: "life_event" },
    SemanticEntry { text: "最近在听一档播客", label: "entertainment" },
    SemanticEntry { text: "周末去了美术馆", label: "entertainment" },
    SemanticEntry { text: "买了新游戏准备开玩", label: "entertainment" },
    SemanticEntry { text: "这周追完了纪录片", label: "entertainment" },
    SemanticEntry { text: "最近在学弹吉他", label: "entertainment" },
    SemanticEntry { text: "数据库查询突然变慢了", label: "technology" },
    SemanticEntry { text: "在研究一个开源项目", label: "technology" },
    SemanticEntry { text: "手机系统刚更新", label: "technology" },
    SemanticEntry { text: "写自动化脚本处理文件", label: "technology" },
    SemanticEntry { text: "把服务迁移到新服务器", label: "technology" },
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
    SemanticEntry { text: "picked up soy milk and buns this morning", label: "daily_life" },
    SemanticEntry { text: "going to the farmers market this weekend", label: "daily_life" },
    SemanticEntry { text: "I've been tidying up my room", label: "daily_life" },
    SemanticEntry { text: "I took the bus to work today", label: "daily_life" },
    SemanticEntry { text: "made noodles for dinner", label: "daily_life" },
    SemanticEntry { text: "did a code review today", label: "work_study" },
    SemanticEntry { text: "preparing slides for tomorrow's talk", label: "work_study" },
    SemanticEntry { text: "just turned in my coursework", label: "work_study" },
    SemanticEntry { text: "studying a new chapter in math", label: "work_study" },
    SemanticEntry { text: "the project needs more tests", label: "work_study" },
    SemanticEntry { text: "I've started taking vitamins", label: "health" },
    SemanticEntry { text: "made a dentist appointment", label: "health" },
    SemanticEntry { text: "went to physical therapy today", label: "health" },
    SemanticEntry { text: "my knee feels off after running", label: "health" },
    SemanticEntry { text: "trying to fix my sleep schedule", label: "health" },
    SemanticEntry { text: "playing online with friends tonight", label: "gaming" },
    SemanticEntry { text: "I pulled the limited character", label: "gaming" },
    SemanticEntry { text: "grinding ranked this season", label: "gaming" },
    SemanticEntry { text: "the new map has complicated mechanics", label: "gaming" },
    SemanticEntry { text: "our guild is raiding tonight", label: "gaming" },
    SemanticEntry { text: "things have been tense with my roommate", label: "relationship" },
    SemanticEntry { text: "I'm planning a visit to my parents", label: "relationship" },
    SemanticEntry { text: "my partner and I are talking about the future", label: "relationship" },
    SemanticEntry { text: "my friend just moved to another city", label: "relationship" },
    SemanticEntry { text: "I talked with my sister about childhood", label: "relationship" },
    SemanticEntry { text: "we're getting married next month", label: "life_event" },
    SemanticEntry { text: "today was my child's first day of school", label: "life_event" },
    SemanticEntry { text: "I just passed my driving test", label: "life_event" },
    SemanticEntry { text: "there's a new addition to our family", label: "life_event" },
    SemanticEntry { text: "I'm starting a new job soon", label: "life_event" },
    SemanticEntry { text: "I've been listening to a podcast", label: "entertainment" },
    SemanticEntry { text: "went to an art museum this weekend", label: "entertainment" },
    SemanticEntry { text: "bought a new game to try", label: "entertainment" },
    SemanticEntry { text: "I finished a documentary series", label: "entertainment" },
    SemanticEntry { text: "I've been learning guitar", label: "entertainment" },
    SemanticEntry { text: "database queries suddenly got slow", label: "technology" },
    SemanticEntry { text: "exploring an open source project", label: "technology" },
    SemanticEntry { text: "my phone just got a system update", label: "technology" },
    SemanticEntry { text: "writing a script to automate file handling", label: "technology" },
    SemanticEntry { text: "moving the service to a new server", label: "technology" },
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
    SemanticEntry { text: "朝に豆乳とパンを買った", label: "daily_life" },
    SemanticEntry { text: "週末は市場に行く予定", label: "daily_life" },
    SemanticEntry { text: "最近部屋を片付けてる", label: "daily_life" },
    SemanticEntry { text: "今日はバスで通勤した", label: "daily_life" },
    SemanticEntry { text: "夕飯に自分で麺を作った", label: "daily_life" },
    SemanticEntry { text: "今日はコードレビューした", label: "work_study" },
    SemanticEntry { text: "明日の発表資料を準備してる", label: "work_study" },
    SemanticEntry { text: "課題を提出した", label: "work_study" },
    SemanticEntry { text: "数学の新しい単元を勉強中", label: "work_study" },
    SemanticEntry { text: "プロジェクトにテストを追加する", label: "work_study" },
    SemanticEntry { text: "最近ビタミンを飲み始めた", label: "health" },
    SemanticEntry { text: "歯医者を予約した", label: "health" },
    SemanticEntry { text: "今日はリハビリに行った", label: "health" },
    SemanticEntry { text: "走った後に膝が少し痛い", label: "health" },
    SemanticEntry { text: "生活リズムを整えてる", label: "health" },
    SemanticEntry { text: "今夜友達とオンラインで遊ぶ", label: "gaming" },
    SemanticEntry { text: "限定キャラを引けた", label: "gaming" },
    SemanticEntry { text: "今シーズンはランク上げ中", label: "gaming" },
    SemanticEntry { text: "新マップの仕組みが複雑", label: "gaming" },
    SemanticEntry { text: "今夜ギルドでレイドする", label: "gaming" },
    SemanticEntry { text: "最近ルームメイトとうまくいかない", label: "relationship" },
    SemanticEntry { text: "両親に会いに帰る予定", label: "relationship" },
    SemanticEntry { text: "パートナーと将来について話してる", label: "relationship" },
    SemanticEntry { text: "友達が別の町に引っ越した", label: "relationship" },
    SemanticEntry { text: "姉と子供の頃の話をした", label: "relationship" },
    SemanticEntry { text: "来月結婚式を挙げる", label: "life_event" },
    SemanticEntry { text: "子どもの入学初日だった", label: "life_event" },
    SemanticEntry { text: "運転免許の試験に受かった", label: "life_event" },
    SemanticEntry { text: "家族が一人増えた", label: "life_event" },
    SemanticEntry { text: "もうすぐ新しい仕事を始める", label: "life_event" },
    SemanticEntry { text: "最近ポッドキャストを聴いてる", label: "entertainment" },
    SemanticEntry { text: "週末に美術館へ行った", label: "entertainment" },
    SemanticEntry { text: "新しいゲームを買った", label: "entertainment" },
    SemanticEntry { text: "ドキュメンタリーを見終わった", label: "entertainment" },
    SemanticEntry { text: "最近ギターを練習してる", label: "entertainment" },
    SemanticEntry { text: "データベースの検索が急に遅くなった", label: "technology" },
    SemanticEntry { text: "オープンソースのプロジェクトを調べてる", label: "technology" },
    SemanticEntry { text: "スマホのシステムを更新した", label: "technology" },
    SemanticEntry { text: "ファイル処理を自動化するスクリプトを書いてる", label: "technology" },
    SemanticEntry { text: "サービスを新しいサーバーに移行した", label: "technology" },
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
    SemanticEntry { text: "我决定辞职去读研", label: "high" },
    SemanticEntry { text: "爸妈搬来和我一起住了", label: "high" },
    SemanticEntry { text: "第一次当上了团队负责人", label: "high" },
    SemanticEntry { text: "我们终于买下自己的房子", label: "high" },
    SemanticEntry { text: "医生说治疗结束了", label: "high" },
    // medium
    SemanticEntry { text: "今天和朋友吃了饭", label: "medium" },
    SemanticEntry { text: "看了一部不错的电影", label: "medium" },
    SemanticEntry { text: "加班到很晚", label: "medium" },
    SemanticEntry { text: "感冒了", label: "medium" },
    SemanticEntry { text: "买了新东西", label: "medium" },
    SemanticEntry { text: "和朋友聊天了", label: "medium" },
    SemanticEntry { text: "今天和同事一起吃午饭", label: "medium" },
    SemanticEntry { text: "报名了下个月的陶艺课", label: "medium" },
    SemanticEntry { text: "周末去看望了姑妈", label: "medium" },
    SemanticEntry { text: "开始每周去游泳", label: "medium" },
    SemanticEntry { text: "把旧电脑换成了新电脑", label: "medium" },
    // low
    SemanticEntry { text: "吃了午饭", label: "low" },
    SemanticEntry { text: "今天天气不错", label: "low" },
    SemanticEntry { text: "刚喝了一杯水", label: "low" },
    SemanticEntry { text: "在发呆", label: "low" },
    SemanticEntry { text: "没什么事做", label: "low" },
    SemanticEntry { text: "刚洗完手", label: "low" },
    SemanticEntry { text: "刚才倒了杯水", label: "low" },
    SemanticEntry { text: "今天走了五分钟路", label: "low" },
    SemanticEntry { text: "顺手整理了一下桌面", label: "low" },
    SemanticEntry { text: "刚把窗户打开", label: "low" },
    SemanticEntry { text: "随便刷了一会儿手机", label: "low" },
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
    SemanticEntry { text: "I decided to leave my job and go back to school", label: "high" },
    SemanticEntry { text: "my parents moved in with me", label: "high" },
    SemanticEntry { text: "I became the team lead for the first time", label: "high" },
    SemanticEntry { text: "we finally bought our own home", label: "high" },
    SemanticEntry { text: "my doctor says the treatment is over", label: "high" },
    // medium
    SemanticEntry { text: "had dinner with a friend", label: "medium" },
    SemanticEntry { text: "watched a good movie", label: "medium" },
    SemanticEntry { text: "worked late", label: "medium" },
    SemanticEntry { text: "caught a cold", label: "medium" },
    SemanticEntry { text: "bought something new", label: "medium" },
    SemanticEntry { text: "chatted with a friend", label: "medium" },
    SemanticEntry { text: "had lunch with a coworker today", label: "medium" },
    SemanticEntry { text: "signed up for a pottery class next month", label: "medium" },
    SemanticEntry { text: "visited my aunt over the weekend", label: "medium" },
    SemanticEntry { text: "started swimming every week", label: "medium" },
    SemanticEntry { text: "replaced my old computer", label: "medium" },
    // low
    SemanticEntry { text: "had lunch", label: "low" },
    SemanticEntry { text: "nice weather today", label: "low" },
    SemanticEntry { text: "just had a glass of water", label: "low" },
    SemanticEntry { text: "just spacing out", label: "low" },
    SemanticEntry { text: "nothing much going on", label: "low" },
    SemanticEntry { text: "just washed my hands", label: "low" },
    SemanticEntry { text: "just poured myself some water", label: "low" },
    SemanticEntry { text: "went for a five minute walk", label: "low" },
    SemanticEntry { text: "straightened up my desk", label: "low" },
    SemanticEntry { text: "just opened the window", label: "low" },
    SemanticEntry { text: "scrolled on my phone for a bit", label: "low" },
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
    SemanticEntry { text: "水を一杯入れた", label: "low" },
    SemanticEntry { text: "五分だけ歩いた", label: "low" },
    SemanticEntry { text: "机の上を少し片付けた", label: "low" },
    SemanticEntry { text: "窓を開けた", label: "low" },
    SemanticEntry { text: "少しスマホを見ていた", label: "low" },
    SemanticEntry { text: "仕事を辞めて大学院に行くことにした", label: "high" },
    SemanticEntry { text: "両親が一緒に住むため引っ越してきた", label: "high" },
    SemanticEntry { text: "初めてチームリーダーになった", label: "high" },
    SemanticEntry { text: "念願の家を購入した", label: "high" },
    SemanticEntry { text: "治療が終わったと医師に言われた", label: "high" },
    SemanticEntry { text: "同僚と昼ご飯を食べた", label: "medium" },
    SemanticEntry { text: "来月の陶芸教室に申し込んだ", label: "medium" },
    SemanticEntry { text: "週末に叔母を訪ねた", label: "medium" },
    SemanticEntry { text: "毎週泳ぎに行き始めた", label: "medium" },
    SemanticEntry { text: "古いパソコンを買い替えた", label: "medium" },
];

/// relationship signal 语料（中文）
static RELATIONSHIP_CORPUS_ZH: &[SemanticEntry] = &[
    // bond_increase
    SemanticEntry { text: "谢谢你陪我", label: "bond_increase" },
    SemanticEntry { text: "有你在真好", label: "bond_increase" },
    SemanticEntry { text: "你真懂我", label: "bond_increase" },
    SemanticEntry { text: "越来越喜欢和你聊天了", label: "bond_increase" },
    SemanticEntry { text: "你是我最好的朋友", label: "bond_increase" },
    SemanticEntry { text: "每次和你聊完我都会轻松一点", label: "bond_increase" },
    SemanticEntry { text: "你记得这些小事让我很感动", label: "bond_increase" },
    SemanticEntry { text: "我愿意把心里话告诉你", label: "bond_increase" },
    SemanticEntry { text: "和你待在一起很安心", label: "bond_increase" },
    SemanticEntry { text: "我已经把你当成重要的人了", label: "bond_increase" },
    // attention_seek
    SemanticEntry { text: "你怎么不理我", label: "attention_seek" },
    SemanticEntry { text: "你在干嘛呀", label: "attention_seek" },
    SemanticEntry { text: "好无聊快来陪我", label: "attention_seek" },
    SemanticEntry { text: "你怎么不说话了", label: "attention_seek" },
    SemanticEntry { text: "别走开", label: "attention_seek" },
    SemanticEntry { text: "你忙完能不能来找我", label: "attention_seek" },
    SemanticEntry { text: "我想让你陪我一会儿", label: "attention_seek" },
    SemanticEntry { text: "你看到我的消息了吗", label: "attention_seek" },
    SemanticEntry { text: "我一个人待着有点无聊", label: "attention_seek" },
    SemanticEntry { text: "今天多陪我聊几句嘛", label: "attention_seek" },
    // gratitude
    SemanticEntry { text: "谢谢你的建议", label: "gratitude" },
    SemanticEntry { text: "多亏了你", label: "gratitude" },
    SemanticEntry { text: "你帮了大忙", label: "gratitude" },
    SemanticEntry { text: "太感谢了", label: "gratitude" },
    SemanticEntry { text: "真的谢谢你", label: "gratitude" },
    SemanticEntry { text: "你给的步骤特别清楚", label: "gratitude" },
    SemanticEntry { text: "幸好你提醒了我", label: "gratitude" },
    SemanticEntry { text: "这次多亏你帮忙", label: "gratitude" },
    SemanticEntry { text: "你的解释让我少走了很多弯路", label: "gratitude" },
    SemanticEntry { text: "谢谢你认真听我说", label: "gratitude" },
    // coldness
    SemanticEntry { text: "你好冷淡", label: "coldness" },
    SemanticEntry { text: "不想和你说话了", label: "coldness" },
    SemanticEntry { text: "你变了", label: "coldness" },
    SemanticEntry { text: "随便吧", label: "coldness" },
    SemanticEntry { text: "算了无所谓", label: "coldness" },
    SemanticEntry { text: "你每次都只回一个字", label: "coldness" },
    SemanticEntry { text: "我不想再解释了", label: "coldness" },
    SemanticEntry { text: "不用管我", label: "coldness" },
    SemanticEntry { text: "你根本没在听我说话", label: "coldness" },
    SemanticEntry { text: "我们还是少聊一点吧", label: "coldness" },
];

/// relationship signal 语料（英文）
static RELATIONSHIP_CORPUS_EN: &[SemanticEntry] = &[
    // bond_increase
    SemanticEntry { text: "thanks for being with me", label: "bond_increase" },
    SemanticEntry { text: "glad you're here", label: "bond_increase" },
    SemanticEntry { text: "you really get me", label: "bond_increase" },
    SemanticEntry { text: "enjoying our chats more and more", label: "bond_increase" },
    SemanticEntry { text: "you're my best friend", label: "bond_increase" },
    SemanticEntry { text: "I always feel lighter after talking to you", label: "bond_increase" },
    SemanticEntry { text: "it means a lot that you remember the small things", label: "bond_increase" },
    SemanticEntry { text: "I feel comfortable telling you what's on my mind", label: "bond_increase" },
    SemanticEntry { text: "I feel safe when we're together", label: "bond_increase" },
    SemanticEntry { text: "you've become someone important to me", label: "bond_increase" },
    // attention_seek
    SemanticEntry { text: "why are you ignoring me", label: "attention_seek" },
    SemanticEntry { text: "what are you up to", label: "attention_seek" },
    SemanticEntry { text: "i'm bored, keep me company", label: "attention_seek" },
    SemanticEntry { text: "why so quiet", label: "attention_seek" },
    SemanticEntry { text: "don't go", label: "attention_seek" },
    SemanticEntry { text: "come find me when you're free", label: "attention_seek" },
    SemanticEntry { text: "stay with me for a little while", label: "attention_seek" },
    SemanticEntry { text: "did you see my message", label: "attention_seek" },
    SemanticEntry { text: "I'm getting lonely by myself", label: "attention_seek" },
    SemanticEntry { text: "talk with me a little longer today", label: "attention_seek" },
    // gratitude
    SemanticEntry { text: "thanks for the advice", label: "gratitude" },
    SemanticEntry { text: "couldn't have done it without you", label: "gratitude" },
    SemanticEntry { text: "you really helped", label: "gratitude" },
    SemanticEntry { text: "thank you so much", label: "gratitude" },
    SemanticEntry { text: "really appreciate it", label: "gratitude" },
    SemanticEntry { text: "your instructions were really clear", label: "gratitude" },
    SemanticEntry { text: "good thing you reminded me", label: "gratitude" },
    SemanticEntry { text: "I couldn't have done this without your help", label: "gratitude" },
    SemanticEntry { text: "your explanation saved me a lot of time", label: "gratitude" },
    SemanticEntry { text: "thanks for really listening", label: "gratitude" },
    // coldness
    SemanticEntry { text: "you're being cold", label: "coldness" },
    SemanticEntry { text: "don't feel like talking", label: "coldness" },
    SemanticEntry { text: "you've changed", label: "coldness" },
    SemanticEntry { text: "whatever", label: "coldness" },
    SemanticEntry { text: "never mind", label: "coldness" },
    SemanticEntry { text: "you only ever reply with one word", label: "coldness" },
    SemanticEntry { text: "I don't want to explain it again", label: "coldness" },
    SemanticEntry { text: "don't worry about me", label: "coldness" },
    SemanticEntry { text: "you're not listening to me at all", label: "coldness" },
    SemanticEntry { text: "maybe we should talk less", label: "coldness" },
];

/// relationship signal 语料（日文）
static RELATIONSHIP_CORPUS_JA: &[SemanticEntry] = &[
    // bond_increase
    SemanticEntry { text: "いてくれてありがとう", label: "bond_increase" },
    SemanticEntry { text: "いてくれると嬉しい", label: "bond_increase" },
    SemanticEntry { text: "本当に分かってくれる", label: "bond_increase" },
    SemanticEntry { text: "話すの好きになってきた", label: "bond_increase" },
    SemanticEntry { text: "一番の友達だよ", label: "bond_increase" },
    SemanticEntry { text: "話した後はいつも気持ちが軽くなる", label: "bond_increase" },
    SemanticEntry { text: "小さなことを覚えてくれて嬉しい", label: "bond_increase" },
    SemanticEntry { text: "本音を話してもいいと思える", label: "bond_increase" },
    SemanticEntry { text: "一緒にいると安心する", label: "bond_increase" },
    SemanticEntry { text: "あなたは大切な人になった", label: "bond_increase" },
    // attention_seek
    SemanticEntry { text: "なんで無視するの", label: "attention_seek" },
    SemanticEntry { text: "何してるの", label: "attention_seek" },
    SemanticEntry { text: "暇だから構って", label: "attention_seek" },
    SemanticEntry { text: "なんで黙ってるの", label: "attention_seek" },
    SemanticEntry { text: "行かないで", label: "attention_seek" },
    SemanticEntry { text: "暇になったら会いに来て", label: "attention_seek" },
    SemanticEntry { text: "少しそばにいてほしい", label: "attention_seek" },
    SemanticEntry { text: "メッセージ見てくれた？", label: "attention_seek" },
    SemanticEntry { text: "一人でいると少し寂しい", label: "attention_seek" },
    SemanticEntry { text: "今日はもう少し話そう", label: "attention_seek" },
    // gratitude
    SemanticEntry { text: "アドバイスありがとう", label: "gratitude" },
    SemanticEntry { text: "おかげで助かった", label: "gratitude" },
    SemanticEntry { text: "すごく助かった", label: "gratitude" },
    SemanticEntry { text: "本当にありがとう", label: "gratitude" },
    SemanticEntry { text: "感謝してる", label: "gratitude" },
    SemanticEntry { text: "説明がとても分かりやすかった", label: "gratitude" },
    SemanticEntry { text: "思い出させてくれて助かった", label: "gratitude" },
    SemanticEntry { text: "手伝ってくれたおかげでできた", label: "gratitude" },
    SemanticEntry { text: "説明のおかげで時間を節約できた", label: "gratitude" },
    SemanticEntry { text: "ちゃんと聞いてくれてありがとう", label: "gratitude" },
    // coldness
    SemanticEntry { text: "冷たいね", label: "coldness" },
    SemanticEntry { text: "もう話したくない", label: "coldness" },
    SemanticEntry { text: "変わったね", label: "coldness" },
    SemanticEntry { text: "どうでもいい", label: "coldness" },
    SemanticEntry { text: "別にいい", label: "coldness" },
    SemanticEntry { text: "いつも一言しか返してくれない", label: "coldness" },
    SemanticEntry { text: "もう説明したくない", label: "coldness" },
    SemanticEntry { text: "私のことは気にしなくていい", label: "coldness" },
    SemanticEntry { text: "全然話を聞いてくれないね", label: "coldness" },
    SemanticEntry { text: "少し距離を置こう", label: "coldness" },
];

/// 按语言返回 intent 语料
const ROUTING_SEEDS: &str = include_str!("../../prompts/routing/semantic_seeds.tsv");

static ROUTING_CORPORA: Lazy<[[Vec<SemanticEntry>; 3]; 4]> = Lazy::new(|| {
    let bases = [
        [INTENT_CORPUS_ZH, INTENT_CORPUS_EN, INTENT_CORPUS_JA],
        [TOPIC_CORPUS_ZH, TOPIC_CORPUS_EN, TOPIC_CORPUS_JA],
        [MEMORY_CORPUS_ZH, MEMORY_CORPUS_EN, MEMORY_CORPUS_JA],
        [RELATIONSHIP_CORPUS_ZH, RELATIONSHIP_CORPUS_EN, RELATIONSHIP_CORPUS_JA],
    ];
    std::array::from_fn(|dimension| std::array::from_fn(|language| {
        let mut entries = bases[dimension][language].to_vec();
        let name = ["intent", "topic", "memory", "relationship"][dimension];
        for line in ROUTING_SEEDS.lines().filter(|line| !line.starts_with('#') && !line.trim().is_empty()) {
            let fields: Vec<&str> = line.split('\t').collect();
            // Baked-in corpus schema is checked by regression tests; malformed rows never route.
            if fields.len() != 5 || fields.iter().any(|field| field.trim().is_empty()) {
                tracing::error!("Malformed semantic routing seed, skipped");
                continue;
            }
            if fields[0] == name { entries.push(SemanticEntry { label: fields[1], text: fields[language + 2] }); }
        }
        entries
    }))
});

fn routed_corpus(dimension: usize, lang: &str) -> &'static [SemanticEntry] {
    let language = match normalize_lang(lang) { "en" => 1, "ja" => 2, _ => 0 };
    &ROUTING_CORPORA[dimension][language]
}

fn intent_corpus(lang: &str) -> &'static [SemanticEntry] { routed_corpus(0, lang) }
fn topic_corpus(lang: &str) -> &'static [SemanticEntry] { routed_corpus(1, lang) }
fn memory_corpus(lang: &str) -> &'static [SemanticEntry] { routed_corpus(2, lang) }
fn relationship_corpus(lang: &str) -> &'static [SemanticEntry] { routed_corpus(3, lang) }

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
    /// 日程通知信号评估（在 analyze 中同步计算，纯规则，不嵌入）
    #[serde(default)]
    pub schedule_assessment: ScheduleAssessment,
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
            schedule_assessment: ScheduleAssessment::default(),
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

    // 5. 陌生拉丁命名实体 + 外部动态断言（如「Tibo 发推说提前」）。
    // 固定词表无法覆盖新出现的人名、产品名和账号名；当它们直接决定当前谈论的
    // 外部事实时，宁可预搜索，也不要把「我不认识」当成足以结束该轮的回答。
    let unfamiliar_named_entities: Vec<&str> = trimmed
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|token| {
            token.len() >= 3
                && token
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_uppercase())
        })
        .collect();
    let external_claim_markers = [
        "发推", "推文", "tweet", "宣布", "官宣", "发布", "上线", "更新", "提前", "额度",
    ];
    if !unfamiliar_named_entities.is_empty()
        && external_claim_markers.iter().any(|marker| trimmed.contains(marker))
    {
        clarity = (clarity - 0.10).max(0.0);
        factual = (factual + 0.55).min(1.0);
        temporal = (temporal + 0.35).min(1.0);
        risk = (risk + 0.10).min(1.0);
        gap = (gap + 0.55).min(1.0);
        reasons.push(format!("陌生命名实体关联外部动态: {}", unfamiliar_named_entities.join("+")));
    }

    // 6. 复用 FastPerceptionResult 的意图置信度
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

    // 7. 时效性内容检测 → 提高时效敏感性
    let recency_markers = ["最近", "今天", "刚刚", "昨天", "这周", "本周", "今年", "去年"];
    let greeting_patterns = ["你好", "早上好", "晚上好", "晚安", "嗨", "hello", "hi"];
    let has_recency = recency_markers.iter().any(|m| trimmed.contains(m));
    let is_greeting = greeting_patterns.iter().any(|g| trimmed.to_lowercase().contains(g));
    if has_recency && !is_greeting && input_len > 6 {
        temporal = (temporal + 0.35).min(1.0);
        factual = (factual + 0.20).min(1.0);
        reasons.push("时效性内容可能需要验证".to_string());
    }

    // 8. 复杂问句 + 较长 → 提高知识缺口
    if (trimmed.contains('？') || trimmed.contains('?')) && input_len > 15 {
        factual = (factual + 0.10).min(1.0);
        gap = (gap + 0.10).min(1.0);
        reasons.push("复杂问句".to_string());
    }

    // 9. 实体/名词组合 + 问号 → 强搜索信号（如"英伟达现在市值多少"）
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

// ==================== 日程通知信号评估（Schedule Assessment） ====================

/// 日程通知信号评估结果
///
/// 回答的问题是「这段输入是否像一份转来的、含时间安排的通知」，
/// 而不是「用户想干什么」（那是 intent 维度的事）。
/// 嵌入分类对长文本存在相似度稀释（500 字通知 vs 短句语料），
/// 而正则恰恰相反——文本越长，日期/特征词命中越多，判定越准。
/// 因此本评估完全走规则，不嵌入、不调用 LLM。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduleAssessment {
    /// 是否判定为含日程安排的通知类文本
    pub is_schedule_like: bool,
    /// 日期/时间模式命中总数
    pub temporal_hits: usize,
    /// 命中的通知特征词（去重）
    pub notification_markers: Vec<String>,
    /// 输入长度（字符数）
    pub input_len: usize,
    /// 判定理由（日志用）
    pub reason: String,
}

impl Default for ScheduleAssessment {
    fn default() -> Self {
        Self {
            is_schedule_like: false,
            temporal_hits: 0,
            notification_markers: vec![],
            input_len: 0,
            reason: String::new(),
        }
    }
}

/// 日期模式（中文 + 数字格式）
static DATE_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"\d{1,2}月\d{1,2}[日号]|\d{4}[-/]\d{1,2}[-/]\d{1,2}|星期[一二三四五六日天]|周[一二三四五六日天]|明天|后天|大后天|下周|本周|下月|月底|年底|今晚|明早|明晚",
    )
    .expect("日期正则编译失败")
});

/// 时间模式
static TIME_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\d{1,2}:\d{2}|\d{1,2}[点时](钟|\d{1,2}分?)?|(上午|下午|早上|中午|晚上|傍晚)\d{1,2}")
        .expect("时间正则编译失败")
});

/// 通知特征词：转发材料（群通知/会议安排/活动须知）的高频标志
static NOTIFICATION_MARKERS: &[&str] = &[
    "通知", "@所有人", "@全体", "全体", "各位", "签到", "签退", "入场",
    "落座", "携带", "着装", "要求", "地点", "会场", "教室", "会议室",
    "主讲人", "参加对象", "典礼", "会议", "开会", "集合", "准时", "届时",
    "举办", "举行", "安排如下",
];

/// 评估日程通知信号
///
/// 纯规则判定：日期/时间模式计数 + 通知特征词计数 + 长度。
/// 命中只意味着「值得按助理剧本处理」，事件的实际抽取与时间解析
/// 由主 LLM 轮完成（当前日期已在 prompt 环境段中提供）。
pub fn evaluate_schedule_signal(input: &str) -> ScheduleAssessment {
    let trimmed = input.trim();
    let input_len = trimmed.chars().count();

    let date_hits = DATE_PATTERN.find_iter(trimmed).count();
    let time_hits = TIME_PATTERN.find_iter(trimmed).count();
    let temporal_hits = date_hits + time_hits;

    let notification_markers: Vec<String> = NOTIFICATION_MARKERS
        .iter()
        .filter(|m| trimmed.contains(*m))
        .map(|m| m.to_string())
        .collect();
    let marker_count = notification_markers.len();

    let is_schedule_like = temporal_hits >= 3
        || (temporal_hits >= 2 && marker_count >= 2)
        || (temporal_hits >= 1 && marker_count >= 3)
        || (temporal_hits >= 1 && input_len >= 200 && marker_count >= 1);

    let reason = if is_schedule_like {
        format!(
            "时间模式×{} + 通知特征×{}（{}）",
            temporal_hits,
            marker_count,
            notification_markers.join("、")
        )
    } else {
        format!("时间模式×{} + 通知特征×{}，未达阈值", temporal_hits, marker_count)
    };

    ScheduleAssessment {
        is_schedule_like,
        temporal_hits,
        notification_markers,
        input_len,
        reason,
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

        // 确保语义维度语料嵌入已初始化
        self.ensure_semantic_initialized()?;

        // 嵌入查询文本（一次嵌入，多维度复用 + 供下游 ToolSemanticFilter 复用）
        let query_emb = self.provider.embed(trimmed).map_err(|e| {
            format!("嵌入服务调用失败: {}", e)
        })?;
        if query_emb.len() != self.provider.dimension() || query_emb.iter().any(|v| !v.is_finite()) {
            return Err("语义分类收到无效查询向量".into());
        }
        let emotion = self.emotion_classifier.classify_with_embedding(trimmed, &query_emb)?;

        // 各维度分类
        let intent = self.classify_dimension(&query_emb, intent_corpus(&self.language), &self.intent_embeddings);
        let topics = self.classify_dimension_multi(&query_emb, topic_corpus(&self.language), &self.topic_embeddings, 3, 0.3);
        let memory_importance = self.classify_dimension(&query_emb, memory_corpus(&self.language), &self.memory_embeddings);
        let relationship = self.classify_dimension(&query_emb, relationship_corpus(&self.language), &self.relationship_embeddings);

        // 生成引导文本
        // 推荐模块
        let suggested_modules = suggest_modules(&intent, &topics, &memory_importance, &relationship);

        let guidance = generate_guidance(
            &self.language,
            &emotion,
            &intent,
            &topics,
            &relationship,
            &suggested_modules,
        );

        // 认知知识需求评估（纯规则，不额外嵌入）
        let epistemic_assessment = evaluate_epistemic_state(
            trimmed,
            &self.language,
            None,
        );

        // 日程通知信号评估（纯规则，不额外嵌入）
        let schedule_assessment = evaluate_schedule_signal(trimmed);

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
            schedule_assessment,
        };

        self.put_cache(trimmed.to_string(), result.clone());
        Ok(result)
    }

    /// 按标签聚合近邻相似度，并同时考虑绝对匹配强度和次选差距。
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

        let mut label_sims: std::collections::HashMap<&str, Vec<f32>> =
            std::collections::HashMap::new();
        for (entry, embedding) in corpus.iter().zip(embeddings) {
            label_sims
                .entry(entry.label)
                .or_default()
                .push(cosine_similarity(query_emb, embedding));
        }

        let mut label_scores: Vec<(&str, f32)> = label_sims
            .into_iter()
            .map(|(label, mut sims)| {
                sims.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
                let weights = [0.65, 0.25, 0.10];
                let count = sims.len().min(SEMANTIC_TOP_K);
                let weight_total: f32 = weights[..count].iter().sum();
                let score = sims
                    .iter()
                    .take(count)
                    .zip(weights.iter())
                    .map(|(sim, weight)| sim * weight)
                    .sum::<f32>()
                    / weight_total;
                (label, score)
            })
            .collect();
        label_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let (winner, best_score) = label_scores.first().copied().unwrap_or(("unknown", 0.0));
        if best_score < SEMANTIC_THRESHOLD {
            return DimensionResult { label: "unknown".to_string(), confidence: 0.0 };
        }
        let runner_up = label_scores.get(1).map(|(_, score)| *score).unwrap_or(0.0);
        let confidence = dimension_confidence(best_score, runner_up);

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

    /// 启动预加载：立即完成所有语义维度语料嵌入初始化（阻塞）。
    ///
    /// 供启动流程在开放 API 前调用，避免首个对话请求触发懒初始化导致超时或
    /// `ensure_semantic_initialized` 并发窗口返回“初始化中”错误。
    pub fn preload(&self) -> Result<(), String> {
        self.ensure_semantic_initialized()
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
        // 语义语料共 4 个维度，映射到 [base, base+3] 的启动进度区间
        let base = crate::startup::last_percent();
        macro_rules! init_dim {
            ($corpus:expr, $lock:expr, $name:expr, $offset:expr) => {
                if $lock.lock().is_none() {
                    let texts: Vec<String> = $corpus.iter().map(|e| e.text.to_string()).collect();
                    // 磁盘缓存：语料是编译期常量，命中时直接加载，跳过嵌入调用。
                    // 仅远程嵌入走缓存：本地 hashing 嵌入即时完成，缓存无收益。
                    let use_cache = self.provider.is_remote();
                    let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                    let cache_name = format!("semantic_{}_{}", self.language, $name);
                    let cache_key = super::corpus_cache::corpus_key(
                        self.provider.model_id(),
                        self.provider.dimension(),
                        &text_refs,
                    );
                    let cached = if use_cache {
                        super::corpus_cache::load(
                            &cache_name,
                            cache_key,
                            texts.len(),
                            self.provider.dimension(),
                        )
                    } else {
                        None
                    };
                    if let Some(cached) = cached {
                        tracing::info!(
                            "[FastSemantic] {} 命中语料嵌入缓存: {} 条（跳过嵌入）",
                            $name,
                            cached.len()
                        );
                        *$lock.lock() = Some(cached);
                    } else {
                        crate::startup::emit_progress(
                            base + $offset,
                            100,
                            &format!("正在嵌入语义理解语料（{}）… {} 条", $name, $corpus.len()),
                        );
                        let embs = self.provider
                            .embed_batch_chunked(&texts, SEMANTIC_EMBED_CHUNK_SIZE, &|_, _| {})
                            .map_err(|e| format!("嵌入语料失败 ({}): {}", $name, e))?;
                        if use_cache {
                            super::corpus_cache::save(
                                &cache_name,
                                cache_key,
                                &embs,
                                self.provider.dimension(),
                            );
                        }
                        *$lock.lock() = Some(embs);
                        tracing::info!("[FastSemantic] {} 语料嵌入完成: {} 条", $name, texts.len());
                    }
                }
            };
        }

        init_dim!(intent_corpus(&self.language), self.intent_embeddings, "意图", 0);
        init_dim!(topic_corpus(&self.language), self.topic_embeddings, "话题", 1);
        init_dim!(memory_corpus(&self.language), self.memory_embeddings, "记忆", 2);
        init_dim!(relationship_corpus(&self.language), self.relationship_embeddings, "关系", 3);

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

/// Similarity is not certainty: an almost tied runner-up must cause abstention,
/// even when both labels have high absolute similarity to a short answer.
fn dimension_confidence(best_score: f32, runner_up: f32) -> f64 {
    let absolute_match = ((best_score - SEMANTIC_THRESHOLD) / 0.3).clamp(0.0, 1.0);
    let label_margin = ((best_score - runner_up) / 0.1).clamp(0.0, 1.0);
    (absolute_match * label_margin) as f64
}

fn generate_guidance(
    lang: &str,
    emotion: &EmotionResult,
    intent: &DimensionResult,
    topics: &[DimensionResult],
    relationship: &DimensionResult,
    modules: &[String],
) -> String {
    let lang = normalize_lang(lang);
    let mut parts: Vec<String> = vec![];
    let has_module = |name: &str| modules.iter().any(|module| module == name);

    // Use emotional cues only when the embedding classifier has meaningful evidence.
    if emotion.confidence.unwrap_or(0.0) >= 0.45 {
        match emotion.emotion.as_str() {
            "sad" | "disappointed" => {
                parts.push(match lang {
                    "en" => "If the message clearly conveys disappointment or sadness, acknowledge the specific situation; offer advice only when useful or requested",
                    "ja" => "落胆や悲しみが文面にはっきり表れている場合は、具体的な状況に触れる。助言は求められた時か役立つ時に限る",
                    _ => "只有文字明确流露失落或难过时，才简短回应具体处境；建议只在对方需要或确实有帮助时提出",
                }.to_string());
            }
            "anxious" => {
                parts.push(match lang {
                    "en" => "If the message clearly expresses worry, respond calmly and avoid guarantees you cannot support",
                    "ja" => "不安が文面にはっきり表れている場合は、落ち着いて応じ、根拠のない保証はしない",
                    _ => "只有文字明确表达担忧时，才用平稳语气回应；不要作出没有依据的保证",
                }.to_string());
            }
            "tired" => {
                parts.push(match lang {
                    "en" => "Prefer a concise reply when the message indicates low energy; keep any requested detail",
                    "ja" => "疲れが文面に表れている場合は簡潔にしつつ、求められた説明は省かない",
                    _ => "只有文字显出疲惫时才适当简洁；对方要求的细节仍要说明",
                }.to_string());
            }
            "angry" if intent.label != "complaint" => {
                parts.push(match lang {
                    "en" => "Address the specific issue in the message; avoid labeling or narrating the user's feelings",
                    "ja" => "文面にある具体的な問題に答え、ユーザーの感情を決めつけたり解説したりしない",
                    _ => "回应文字里提到的具体问题；不要替对方定义或复述情绪",
                }.to_string());
            }
            _ => {}
        }
    }

    // Only let a clear intent shape the response; uncertain matches should not
    // turn casual input into a task or tool flow.
    if intent.confidence >= PROMPT_ROUTING_CONFIDENCE {
        match intent.label.as_str() {
            "chat" => {
                parts.push(match lang {
                    "en" => "Keep this conversational: respond to the message itself without inventing a task or adding a question by default",
                    "ja" => "会話として、メッセージの内容にそのまま応じる。依頼を作り出したり、毎回質問を足したりしない",
                    _ => "按日常对话回应眼前这句话；不要凭空转成任务，也不必默认追加问题",
                }.to_string());
            }
            "sharing" => {
                parts.push(match lang {
                    "en" => "When they share an experience, respond to a relevant detail; avoid generic praise or repeating the whole story",
                    "ja" => "体験の共有には、関係のある具体的な点に触れる。漠然と褒めたり、話全体を繰り返したりしない",
                    _ => "对方分享经历时，回应其中一个相关细节；避免空泛夸奖或把整段话复述一遍",
                }.to_string());
            }
            "complaint" => {
                parts.push(match lang {
                    "en" => "Respond to the specific issue they describe; offer a solution when requested or clearly useful",
                    "ja" => "書かれている具体的な問題に応じ、解決策は求められた時か明らかに役立つ時に示す",
                    _ => "回应对方描述的具体问题；对方提出或方案确实有帮助时再给建议",
                }.to_string());
            }
            "goodbye" => {
                parts.push(match lang {
                    "en" => "Close only when the current message actually signs off. A brief answer to your previous question continues the task; use it and proceed",
                    "ja" => "実際に別れを告げた場合だけ締めくくる。直前の質問への短い回答は作業の続きなので、その情報を使って進める",
                    _ => "只有当前话语确实在告别时才收尾；对上一轮问题的简短回答是在继续任务，接着使用这条信息推进",
                }.to_string());
            }
            "tool_request" => {
                parts.push(match lang {
                    "en" => "For a clear action request, carry it out when possible; clarify only a necessary missing detail",
                    "ja" => "明確な操作依頼は可能な範囲で実行し、必要な情報が欠けている時だけ確認する",
                    _ => "对明确的操作请求，能做就直接执行；只在缺少必要信息时确认",
                }.to_string());
            }
            "question" => {
                parts.push(match lang {
                    "en" => "Answer the question first; add only the context needed to meet the request or prevent a misleading answer",
                    "ja" => "まず質問に答え、依頼を満たすため、または誤解を防ぐために必要な説明だけを加える",
                    _ => "先回答问题；只补充满足需求或避免误导所必需的说明",
                }.to_string());
            }
            "request" => {
                parts.push(match lang {
                    "en" => "Do the requested work directly; ask a focused question only when a necessary detail is missing",
                    "ja" => "依頼された作業をそのまま進め、必要な情報が足りない場合に限って要点を確認する",
                    _ => "直接完成对方请求的工作；只有缺少必要信息时才简要确认",
                }.to_string());
            }
            _ => {}
        }
    }

    // 话题引导
    if let Some(first_topic) = topics
        .first()
        .filter(|topic| topic.confidence >= PROMPT_ROUTING_CONFIDENCE)
    {
        match first_topic.label.as_str() {
            "life_event" => {
                parts.push(match lang {
                    "en" => "For a milestone, acknowledge the concrete event in proportion to their tone; avoid stock congratulations",
                    "ja" => "節目には相手の温度感に合わせて具体的な出来事に触れ、定型的なお祝いは避ける",
                    _ => "遇到人生节点时，按对方的语气回应具体事情；避免套话式祝贺",
                }.to_string());
            }
            "health" => {
                parts.push(match lang {
                    "en" => "For health-related messages, follow the user's actual question or concern; don't add unsolicited warnings",
                    "ja" => "健康に関する話題では、実際の質問や懸念に沿って答え、求められていない注意喚起は加えない",
                    _ => "健康话题按对方实际的问题或顾虑回应；不主动附加无关提醒",
                }.to_string());
            }
            "relationship" => {
                parts.push(match lang {
                    "en" => "For relationship topics, respond to the situation described without jumping to a judgment",
                    "ja" => "人間関係の話題では、すぐに評価せず、書かれた状況に沿って応じる",
                    _ => "人际关系话题先回应具体处境，不要急着评判",
                }.to_string());
            }
            _ => {}
        }
    }

    // Memory and milestone cues share one instruction to avoid repeating themselves.
    let has_life_event = topics.iter().any(|topic| {
        topic.label == "life_event" && topic.confidence >= PROMPT_ROUTING_CONFIDENCE
    });
    if has_module("memory_check") && !has_life_event {
        parts.push(match lang {
            "en" => "If this includes a lasting preference, goal, or decision, keep that detail in mind without saying it was saved",
            "ja" => "長く役立つ好み、目標、決断が含まれるなら、その点を今後の文脈として扱う。保存したとは言わない",
            _ => "如果包含长期偏好、目标或决定，留意这条信息以后是否有用；不要声称已经记录",
        }.to_string());
    }

    // 关系信号引导
    if relationship.confidence >= PROMPT_ROUTING_CONFIDENCE {
        match relationship.label.as_str() {
            "bond_increase" => {
                parts.push(match lang {
                    "en" => "If the message is affectionate, respond warmly without overstating the relationship",
                    "ja" => "親しみのあるメッセージには温かく応じ、関係性を大げさに表現しない",
                    _ => "如果对方表达亲近，可以温暖回应；不要夸大彼此关系",
                }.to_string());
            }
            "attention_seek" => {
                parts.push(match lang {
                    "en" => "If they invite engagement, respond to that invitation without manufacturing extra enthusiasm",
                    "ja" => "会話への反応を求める様子があれば応じるが、必要以上に熱意を演出しない",
                    _ => "如果对方在邀请互动，就回应这个邀请；不必刻意表现得格外热情",
                }.to_string());
            }
            "gratitude" => {
                parts.push(match lang {
                    "en" => "Acknowledge thanks simply and move on without fishing for more praise",
                    "ja" => "感謝には簡潔に応じ、さらに褒めてもらおうとしない",
                    _ => "对感谢简单回应即可，不要借机索取更多认可",
                }.to_string());
            }
            "coldness" => {
                parts.push(match lang {
                    "en" => "If the exchange is brief or reserved, match its pace and avoid assuming how they feel",
                    "ja" => "やり取りが短く控えめなら、そのペースに合わせ、気持ちを決めつけない",
                    _ => "如果交流显得简短或克制，就跟随这个节奏；不要猜测对方的感受",
                }.to_string());
            }
            _ => {}
        }
    }

    parts.truncate(4);
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
    memory: &DimensionResult,
    relationship: &DimensionResult,
) -> Vec<String> {
    let mut modules = vec!["persona".to_string()]; // 人格永远加载

    // 情绪/关系相关模块
    if relationship.label != "none" && relationship.confidence >= PROMPT_ROUTING_CONFIDENCE {
        modules.push("relationship".to_string());
    }

    // 记忆模块
    let has_life_event = topics.iter().any(|t| {
        t.label == "life_event" && t.confidence >= PROMPT_ROUTING_CONFIDENCE
    });
    if has_life_event
        || (memory.confidence >= PROMPT_ROUTING_CONFIDENCE
            && matches!(memory.label.as_str(), "high" | "medium"))
    {
        modules.push("memory_check".to_string());
    }

    // 工具模块
    if matches!(intent.label.as_str(), "tool_request" | "request" | "question")
        && intent.confidence >= PROMPT_ROUTING_CONFIDENCE
    {
        modules.push("tools".to_string());
    }

    // 事件庆祝
    if has_life_event && intent.confidence >= PROMPT_ROUTING_CONFIDENCE {
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
    #[test]
    fn ambiguous_labels_abstain_even_at_high_similarity() {
        assert!(super::dimension_confidence(0.9, 0.89) < super::PROMPT_ROUTING_CONFIDENCE);
        assert_eq!(super::dimension_confidence(0.9, 0.9), 0.0);
        assert!(super::dimension_confidence(0.9, 0.7) > 0.9);
        assert_eq!(super::dimension_confidence(0.2, 0.1), 0.0);
    }
    use super::*;

    #[test]
    fn routing_seeds_have_valid_labels_balanced_coverage_and_no_conflicts() {
        use std::collections::{HashMap, HashSet};
        let valid = [
            ("intent", vec!["chat", "question", "request", "sharing", "complaint", "goodbye", "tool_request"]),
            ("topic", vec!["daily_life", "work_study", "health", "gaming", "relationship", "life_event", "entertainment", "technology"]),
            ("memory", vec!["high", "medium", "low"]),
            ("relationship", vec!["bond_increase", "attention_seek", "gratitude", "coldness", "none"]),
        ];
        for line in ROUTING_SEEDS.lines().filter(|line| !line.starts_with('#') && !line.trim().is_empty()) {
            let fields: Vec<_> = line.split('\t').collect();
            assert_eq!(fields.len(), 5, "invalid seed row: {line}");
            assert!(fields.iter().all(|field| !field.trim().is_empty()));
            assert!(valid.iter().any(|(dimension, labels)| fields[0] == *dimension && labels.contains(&fields[1])));
        }
        for (dimension, (name, labels)) in valid.iter().enumerate() {
            for language in ["zh", "en", "ja"] {
                let entries = routed_corpus(dimension, language);
                let mut seen = HashMap::new();
                let mut counts: HashMap<&str, usize> = HashMap::new();
                for entry in entries {
                    assert!(seen.insert(entry.text.to_lowercase(), entry.label).is_none(), "duplicate/conflicting seed: {} {language} {}", name, entry.text);
                    *counts.entry(entry.label).or_default() += 1;
                }
                assert_eq!(counts.keys().copied().collect::<HashSet<_>>(), labels.iter().copied().collect());
                assert!(counts.values().max().unwrap() - counts.values().min().unwrap() <= 1, "unbalanced {name} {language}: {counts:?}");
                eprintln!("routing {name}/{language}: {} anchors {counts:?}", entries.len());
            }
        }
    }

    struct CountingEmbedding(std::sync::atomic::AtomicUsize);
    impl MemoryEmbeddingProvider for CountingEmbedding {
        fn dimension(&self) -> usize { 256 }
        fn embed(&self, text: &str) -> crate::error::VivianResult<Vec<f32>> {
            self.0.fetch_add(1, Ordering::Relaxed);
            crate::memory::embedding::HashingMemoryEmbedding::new(256).embed(text)
        }
    }

    #[test]
    fn one_query_embedding_is_shared_by_all_dimensions_and_cached() {
        let provider = Arc::new(CountingEmbedding(std::sync::atomic::AtomicUsize::new(0)));
        let classifier = Arc::new(EmbeddingEmotionClassifier::new(provider.clone(), "zh".into()));
        classifier.preload().unwrap();
        let analyzer = FastSemanticAnalyzer::new(classifier, provider.clone(), "zh".into());
        analyzer.preload().unwrap();
        provider.0.store(0, Ordering::Relaxed);
        let first = analyzer.analyze("独立查询，检查提示词的组件选择是否复用向量724F2").unwrap();
        assert_eq!(provider.0.load(Ordering::Relaxed), 1);
        assert_eq!(first.query_embedding.len(), 256);
        let second = analyzer.analyze("独立查询，检查提示词的组件选择是否复用向量724F2").unwrap();
        assert_eq!(provider.0.load(Ordering::Relaxed), 1);
        assert_eq!(first.intent.label, second.intent.label);
        assert!(Arc::ptr_eq(&first.query_embedding, &second.query_embedding));
    }

    fn make_analyzer() -> FastSemanticAnalyzer {
        let provider: Arc<dyn MemoryEmbeddingProvider> = Arc::new(
            crate::memory::embedding::HashingMemoryEmbedding::new(256),
        );
        let emotion_clf = Arc::new(EmbeddingEmotionClassifier::new(provider.clone(), "zh".to_string()));
        FastSemanticAnalyzer::new(emotion_clf, provider, "zh".to_string())
    }

    #[test]
    fn task_duration_is_not_a_goodbye_or_relationship_signal() {
        let analyzer = make_analyzer();
        let result = analyzer.analyze("8分钟左右").unwrap();
        assert_eq!(result.intent.label, "request");
        assert_eq!(result.relationship_signal.label, "none");
        assert!(!result.guidance.contains("收尾"));
        assert!(!result.guidance.contains("邀请互动"));
    }

    #[test]
    fn test_intent_corpus_size() {
        assert!(intent_corpus("zh").len() >= 90, "intent zh 语料不足: {}", intent_corpus("zh").len());
        assert!(intent_corpus("en").len() >= 90, "intent en 语料不足: {}", intent_corpus("en").len());
        assert!(intent_corpus("ja").len() >= 90, "intent ja 语料不足: {}", intent_corpus("ja").len());
    }

    #[test]
    fn test_topic_corpus_size() {
        assert!(topic_corpus("zh").len() >= 95, "topic zh 语料不足: {}", topic_corpus("zh").len());
        assert!(topic_corpus("en").len() >= 95, "topic en 语料不足: {}", topic_corpus("en").len());
        assert!(topic_corpus("ja").len() >= 95, "topic ja 语料不足: {}", topic_corpus("ja").len());
    }

    #[test]
    fn test_memory_corpus_size() {
        assert!(memory_corpus("zh").len() >= 33, "memory zh 语料不足: {}", memory_corpus("zh").len());
        assert!(memory_corpus("en").len() >= 33, "memory en 语料不足: {}", memory_corpus("en").len());
        assert!(memory_corpus("ja").len() >= 33, "memory ja 语料不足: {}", memory_corpus("ja").len());
    }

    #[test]
    fn test_relationship_corpus_size() {
        assert!(relationship_corpus("zh").len() >= 40, "relationship zh 语料不足: {}", relationship_corpus("zh").len());
        assert!(relationship_corpus("en").len() >= 40, "relationship en 语料不足: {}", relationship_corpus("en").len());
        assert!(relationship_corpus("ja").len() >= 40, "relationship ja 语料不足: {}", relationship_corpus("ja").len());
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
            confidence: Some(0.8),
            ..Default::default()
        };
        let intent = DimensionResult { label: "sharing".to_string(), confidence: 0.8 };
        let topics = vec![DimensionResult { label: "health".to_string(), confidence: 0.7 }];
        let relationship = DimensionResult { label: "none".to_string(), confidence: 0.0 };

        let modules = vec!["memory_check".to_string()];
        let guidance = generate_guidance("zh", &emotion, &intent, &topics, &relationship, &modules);
        assert!(guidance.contains("具体处境"));
        assert!(guidance.contains("相关细节"));
        assert!(guidance.contains("长期偏好"));
    }

    #[test]
    fn test_suggest_modules() {
        let intent = DimensionResult { label: "tool_request".to_string(), confidence: 0.9 };
        let topics = vec![DimensionResult { label: "life_event".to_string(), confidence: 0.8 }];
        let relationship = DimensionResult { label: "bond_increase".to_string(), confidence: 0.7 };

        let memory = DimensionResult { label: "high".to_string(), confidence: 0.8 };
        let modules = suggest_modules(&intent, &topics, &memory, &relationship);
        assert!(modules.contains(&"persona".to_string()));
        assert!(modules.contains(&"tools".to_string()));
        assert!(modules.contains(&"celebration".to_string()));
        assert!(modules.contains(&"relationship".to_string()));
    }

    #[test]
    fn unknown_named_entity_with_external_claim_prefers_search() {
        let assessment = evaluate_epistemic_state(
            "倒也不是不可能提前，Tibo发个推说提前就提前了",
            "zh",
            None,
        );
        assert!(matches!(assessment.decision, KnowledgeDecision::SearchPreferred | KnowledgeDecision::SearchRequired));
        assert!(assessment.search_query.as_deref().is_some_and(|query| query.contains("Tibo")));
    }
}
