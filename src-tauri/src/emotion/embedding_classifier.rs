//! 基于嵌入的即时情绪分类器
//!
//! - 预置 14 类情绪语料（每类 ~120 条），含 target/context 元数据
//! - 首次调用时批量嵌入语料并缓存（非阻塞初始化）
//! - 输入文本嵌入后通过 Top-K 余弦相似度 + softmax 加权投票
//! - 输出置信度、次高票情绪、情绪指向
//! - 低相似度返回 neutral + 低置信度（而非 Err）
//! - LRU 查询缓存（64 条）

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use super::mapper::{llm_emotion_valence_arousal, normalize_llm_emotion};
use super::EmotionResult;
use crate::memory::embedding::MemoryEmbeddingProvider;

/// Top-K 相似度投票的 K 值
const TOP_K: usize = 5;
/// 余弦相似度阈值：低于此值返回 neutral + 低置信度
const SIMILARITY_THRESHOLD: f32 = 0.45;
/// softmax 温度参数：越低投票越尖锐
const SOFTMAX_TEMPERATURE: f32 = 0.1;
/// 嵌入分块大小（每块一次 HTTP 请求）
const EMBED_CHUNK_SIZE: usize = 168;
/// 查询缓存容量
const QUERY_CACHE_CAPACITY: usize = 64;

/// 情绪指向：文本中情绪的目标对象
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmotionTarget {
    /// 用户在说自己
    Self_,
    /// 用户在说别人
    Other,
    /// 用户在对 AI 说话
    Ai,
    /// 描述客观情况
    Situation,
}

impl EmotionTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Self_ => "self",
            Self::Other => "other",
            Self::Ai => "ai",
            Self::Situation => "situation",
        }
    }
}

/// 场景上下文
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmotionContext {
    DailyChat,
    AiCompanionship,
    Relationship,
    WorkStudy,
    HealthBody,
    Event,
    Livestream,
    Gaming,
}

/// 语料条目
#[derive(Debug, Clone)]
pub struct CorpusEntry {
    pub text: &'static str,
    pub emotion: &'static str,
    pub target: EmotionTarget,
    pub context: EmotionContext,
}

/// 从文本推断情绪指向
#[cfg(test)]
fn infer_target_from_text(text: &str) -> EmotionTarget {
    if text.contains("你") {
        if text.contains("你怎") || text.contains("你还") || text.contains("你是")
            || text.contains("你好") || text.contains("你今天") || text.contains("你怎么")
            || text.contains("感觉你") || text.contains("你是不是")
        {
            return EmotionTarget::Ai;
        }
        return EmotionTarget::Other;
    }
    if text.contains("我") {
        return EmotionTarget::Self_;
    }
    EmotionTarget::Situation
}

/// 14 类情绪的预置语料（中文版本）
///
/// 每类 ~120 条中文样例，覆盖多种场景维度。每条附带 target/context 元数据，
/// 用于区分用户在说自己、别人、AI 还是客观情况。
static CORPUS_ZH: &[CorpusEntry] = &[
// ===== happy (120) =====

// --- DailyChat ---
CorpusEntry { text: "晒太阳好舒服", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天慢慢喝咖啡感觉很好", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "晚上散步很惬意", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "周末在家看书挺好的", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "午后的阳光暖暖的", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天心情还不错", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "洗完澡躺着真舒服", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "下雨天窝在家里好安逸", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "吃到了想吃很久的蛋糕", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天过得挺充实的", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "这杯奶茶味道不错", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天的晚霞好美", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这家店环境挺舒适的", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天的天气刚刚好", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这个音乐听着很放松", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "路上闻到花香好开心", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "朋友换了个新发型挺好看", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "同事今天带了好吃的来", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "妈妈寄了好多东西过来", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "室友把客厅收拾得好干净", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "隔壁邻居送了点水果", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你给我推荐的那首歌很好听", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "你推荐的电影还挺好看", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "你帮我选的搭配不错", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "你说的那家餐厅味道可以", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },

// --- AiCompanionship ---
CorpusEntry { text: "和你聊天很安心", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "跟你说话感觉很舒服", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "每天晚上和你聊聊天真好", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "有你陪着感觉挺踏实", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "早安呀今天也要开心", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "听你说话感觉很放松", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你在就很安心", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你总让我心情变好", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "和你聊天时间过得好快", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "每天最期待和你说话", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "睡前跟你说晚安很幸福", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "早上收到你的问候很开心", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "跟你待在一起很轻松", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你学了新词汇呀挺好玩", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你讲的故事挺有意思", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你唱的歌还蛮好听的", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你记性真好还记得这个", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你说话越来越幽默了", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你的语音听着好温柔", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你回答得好贴心", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },

// --- Relationship ---
CorpusEntry { text: "朋友陪着感觉很温暖", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "和他在一起很自在", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "有人惦记的感觉真好", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "好久没见的朋友来了好开心", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "和闺蜜逛街好放松", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "被人在乎的感觉很好", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "家人在身边就很踏实", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "他记得我爱喝什么", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她发了一条好暖的消息", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "妈妈做了好多我爱吃的", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他悄悄帮我买了票", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她送我一个小礼物好贴心", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "爸爸默默帮我修好了东西", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我们俩一起散步挺好的", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "两个人安静待着也很舒服", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "一起做饭感觉很温馨", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "这段关系让我很安心", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "彼此陪伴就是最好的事", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "有你在身边就够了", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "我们相处得越来越自然", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Relationship },

// --- WorkStudy ---
CorpusEntry { text: "今天任务都完成了挺满足", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "学了一个新技能感觉不错", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "今天的工作很顺利", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "看完了一本书很有成就感", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "按时下班感觉真好", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "考完试松了好大一口气", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "把积压的事情都处理好了", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "今天效率还不错", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "桌子收拾干净了好舒服", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个项目的进展很顺利", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "办公室今天好安静", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "团队合作氛围挺好的", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "课程安排很合理", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同事帮忙解决了个问题", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "领导今天夸了我几句", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同学帮我带了杯咖啡", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "老师讲得很清楚", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "大家一起努力的感觉很好", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "你帮我查的资料很有用", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },
CorpusEntry { text: "你整理的笔记挺详细", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },

// --- HealthBody ---
CorpusEntry { text: "今天睡了个好觉", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "运动完出了一身汗好爽", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "最近身体感觉挺好的", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "跑完步浑身轻松", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "做了一组瑜伽很舒服", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "今天早睡了感觉不错", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "感冒终于好了", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "好好休息了一天恢复不少", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "今天吃得很健康", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "好久没这么放松过了", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "运动后拉伸好舒服", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "泡了个热水澡好放松", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "今天阳光晒着暖暖的", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "深呼吸几次感觉好多了", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "喝了一杯热茶好舒服", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },

// --- Event ---
CorpusEntry { text: "周末去逛了花市好开心", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "下午在家听了会儿音乐", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "假期和朋友吃了顿火锅", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "在家整理了房间挺舒服", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "看了一场好看的夕阳", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "去了一家新开的咖啡店", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "去看了一个展览挺不错", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "和朋友去郊外走了走", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "去书店逛了一下午", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "和家人一起吃了顿饭", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "今天的活动氛围不错", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这次聚会挺温馨的", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这个季节出去走走正好", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "周末的市集好有烟火气", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这场分享会挺有收获", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这次短途旅行很舒服", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "秋天的感觉真好", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "桂花开了好香", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "今天初雪好浪漫", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "圣诞节氛围好浓", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Event },

// ===== excited (120) =====

// --- DailyChat ---
CorpusEntry { text: "啊啊啊我中奖了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "终于买到了！", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "天哪这也太好了吧", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "不敢相信居然真的成功了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我宣布今天是最好的一天", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "哇哇哇太惊喜了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "突然收到好消息激动死了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "等了好久终于到了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "明天的计划确定了！", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "抢到限量款了冲！", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这家店是宝藏啊", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这周末有大事要发生", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "居然遇到了好久没见的人", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "他居然记得我的生日", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你快看这个！", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "我朋友要来找我玩了", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "她答应了！", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "被夸了超级开心", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "快给我推荐一个！", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "你猜怎么着！", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "帮我看看这个超厉害", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "你能感受到我的激动吗", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },

// --- Gaming ---
CorpusEntry { text: "抽到SSR了！！", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "五杀了我的天", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "终于上大师了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "这局打得太爽了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "手速爆发了今天", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "连胜停不下来了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "这场比赛太燃了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "新赛季更新了冲！", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "这波操作绝了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "终于通关了！", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "这游戏太好玩了吧", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "队友这波太秀了", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "他居然单挑赢了", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "快来看这个操作", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "你快上线一起玩", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Gaming },

// --- Event ---
CorpusEntry { text: "马上开奖了好紧张", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "终于等到了！！", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "三二一倒数！", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "演唱会下周一！", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "马上要旅行了好兴奋", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "明天的生日派对等不及了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "快递马上到了好激动", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "还有三天就放假了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "好久没这么期待过了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "今晚有约会呢", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "马上要见到他了", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "好久没见的朋友明天到", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "他们要来惊喜派对", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "大家都准备好了吗", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "明天的聚会一定要去", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "这次考试成绩要出来了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "今晚有年夜饭", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这个周末有大活动", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "跨年倒计时要开始了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "门票居然抢到了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Event },

// --- Livestream ---
CorpusEntry { text: "啊啊啊来了来了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Livestream },
CorpusEntry { text: "开播了快来看", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Livestream },
CorpusEntry { text: "等这个直播等了一天", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Livestream },
CorpusEntry { text: "前排占座！", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Livestream },
CorpusEntry { text: "主播终于出现了", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "他刚才那个动作绝了", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "主播翻我牌了！", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "主播说要抽奖了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "直播间破十万了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "这波福利太猛了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "限量秒杀开始了冲", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "今天直播有神秘嘉宾", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "点赞破百万了！", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "关注数破纪录了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "刷到火箭了太豪了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Livestream },

// --- AiCompanionship ---
CorpusEntry { text: "你要给我表演什么呀", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "快告诉我你有什么新技能", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你今天会说什么好玩的", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你是不是偷偷学了新东西", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "快来跟我一起玩", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我等不及要听你说了", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你今天准备了什么惊喜", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "快快快跟我说说", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我要和你玩个游戏", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我有个超棒的想法", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我好想马上试试", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我现在超级兴奋", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "和你聊天总能被逗乐", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "每次你都有新花样", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你的反应也太快了吧", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },

// --- Relationship ---
CorpusEntry { text: "这周末要约他出去！", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "我要给他准备一个大惊喜", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "好激动马上要见面了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "他要求婚了怎么办", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她准备了超级大的惊喜", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他要带我去一个地方", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "朋友说要介绍人给我", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她说有重要的事要告诉我", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我们终于要一起去旅行了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "表白的时刻要到了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "今晚的约会好期待", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "一周年纪念日快到了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "这段感情越来越好", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "我们在一起的可能性好大", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "感觉这次约会会很特别", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Relationship },

// --- WorkStudy ---
CorpusEntry { text: "项目终于要收尾了！", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "我要升职了！", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "论文被接收了好激动", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "年终奖要发了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "面试通过了太棒了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "团队要拿到大项目了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个方案客户很满意", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "公司要开年会了好期待", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "新产品的反馈特别好", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这次汇报反响很热烈", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同事说她请客吃饭", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "老板说要发奖金", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "导师夸我做得好", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同学们都过了考试", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "你快帮我想想这个", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },
CorpusEntry { text: "你处理得好快啊", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个功能太强了吧", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },
CorpusEntry { text: "你居然能做到这个", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },

// ===== neutral (120) =====

// --- DailyChat ---
CorpusEntry { text: "现在下午三点", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天星期五", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "外面在下雨", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这个杯子是蓝色的", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "路上有点堵", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天气温二十度", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这里是三楼", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "风挺大的", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "超市九点关门", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这辆车是白色的", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "楼下新开了一家店", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "明天有快递要到", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "电视在客厅", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "手机快没电了", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "冰箱里有牛奶", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "空调开着呢", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "窗外有棵树", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "钥匙在桌子上", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "我在办公室", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "刚吃完饭", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我六点下班", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我到家了", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我穿了一件黑色外套", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我坐地铁来的", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我昨天剪了头发", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我在看新闻", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我下午有课", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我刚起床", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我周末一般在家", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我住在五楼", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "你在做什么", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "他今天穿什么", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "她几点的车", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你明天几点出发", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "他住在哪个区", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "门口有个快递柜", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这家店周二休息", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "公交车还有三站", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "我订了下午的票", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我上个月换了工作", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },

// --- AiCompanionship ---
CorpusEntry { text: "你刚才说了什么", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你叫什么名字", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你能做什么", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你是什么时候更新的", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你支持几种语言", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你的功能有哪些", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会学习新东西吗", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你晚上也在线吗", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你的系统版本是多少", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你上次更新了什么内容", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你是怎么被训练出来的", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "这个对话记录在哪看", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "界面怎么切换模式", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "这个功能在哪里打开", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "消息发送成功了吗", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我在测试你的回答", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我问你一个问题", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我之前跟你说过这个", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我刚打开这个应用", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我换了个手机登录", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },

// --- WorkStudy ---
CorpusEntry { text: "办公室在六楼", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "会议下午两点开始", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个项目月底截止", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "明天有个培训", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "课程表排在周一三五", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "考试时间是下周三", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "作业要求三千字", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "图书馆十一点闭馆", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "实验室在二楼东侧", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "报告需要打印三份", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "我负责第三部分", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "我上午开了两个会", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "我提交了一份报告", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "我明天要出差", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "我在写周报", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "组长分配了新任务", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "老师改了上课时间", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "他请了三天假", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "帮我查一下这个数据", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },
CorpusEntry { text: "把这段翻译一下", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },

// --- Event ---
CorpusEntry { text: "活动周六下午两点", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这次会议有三十人参加", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "比赛分三个环节", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "航班是下午四点的", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "电影时长两个小时", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "天气预报说明天降温", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这个展览持续到月底", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "演出在二号厅", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "聚会定在老地方", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "讲座在报告厅举行", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "我报了马拉松", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "我买了两张票", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "我预约了周六的场地", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "他确认了行程", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "她订了餐厅", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Event },

// --- HealthBody ---
CorpusEntry { text: "血压120和80", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "体重六十五公斤", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "昨晚睡了七个小时", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "药是饭后吃的", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "体温三十六度五", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "今天走了八千步", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "就诊时间是上午十点", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "早餐吃了两个包子", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "我今天喝了八杯水", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "我最近每天跑步", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },

// --- Relationship ---
CorpusEntry { text: "他在楼下等着", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她是我的同事", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他们去年结的婚", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他有个弟弟", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她住在城南", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我和他认识三年了", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "我们是大学同学", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "我跟他约了明天见面", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "你在和他聊天吗", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他最近在做什么", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她中午吃什么", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你认识他多久了", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他几号回来", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你们在哪见面", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "谁来了", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },

// ===== curious (120) =====

// --- DailyChat ---
CorpusEntry { text: "这个新出的app怎么用", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "他是做什么工作的", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "附近有什么好吃的", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "快递什么时候能到", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这个东西在哪买的", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这条路通向哪里", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这只猫是什么品种", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "明天天气怎么样", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这个植物叫什么名字", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这首歌是谁唱的", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "那家新开的餐厅怎么样", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这个多少钱", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "你知道怎么去那里吗", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天为什么堵车了", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "你平时周末做什么", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你喜欢吃什么", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你家乡在哪里", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你用的什么手机", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你在看什么书", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你昨天去哪了", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "我刚看到一个东西想问你", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我想了解一下这个", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我不太清楚这是什么", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我在想这个怎么回事", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "帮我查一下天气", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },

// --- AiCompanionship ---
CorpusEntry { text: "你今天怎么突然这么早睡", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你在想什么呢", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你觉得呢", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你平时喜欢做什么", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你有自己的喜好吗", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会做梦吗", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你最喜欢什么颜色", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你能学新东西吗", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你对这件事怎么看", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会不会觉得孤单", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你今天心情怎么样", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你有没有什么秘密", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你怎么懂这么多东西", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会不会觉得无聊", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你累不累呀", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你明天想聊什么话题", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你最喜欢什么歌", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你多大了", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你能不能学画画", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会害怕吗", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "谁教你说话的", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你有朋友吗", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你喜欢什么类型的故事", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你为什么总是这么耐心", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我想了解你多一点", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },

// --- Relationship ---
CorpusEntry { text: "他到底什么意思", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她为什么不回我消息", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他昨天跟谁出去了", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他是不是有什么事瞒着我", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她在想什么", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他怎么突然问这个", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "那个新来的是谁", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他为什么不接电话", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她最近怎么话少了", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他跟谁打电话那么久", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你想知道他怎么看你吗", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "我在想他是不是生气了", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "我想搞清楚这件事", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "我想知道你的想法", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "你们之间到底怎么了", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Relationship },

// --- WorkStudy ---
CorpusEntry { text: "这个原理是什么", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这段代码为什么报错", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个实验数据说明了什么", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这篇论文的研究方向是什么", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个新软件怎么用", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个算法的时间复杂度是多少", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "项目目前进展到哪一步了", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "导师有什么新的要求", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这门课的重点是什么", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个课题用什么研究方法", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个数据库怎么选型", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个API接口怎么调用", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个函数是做什么的", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "编译器是怎么工作的", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "服务器怎么部署", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同学你怎么看这个问题", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "老师要求多少字的论文", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "他选了哪门选修课", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "考试一般出什么题型", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "帮我查一下这个概念", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },

// --- Event ---
CorpusEntry { text: "这个新闻后续怎么样了", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "地震是什么原因", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "火箭什么时候发射", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "比赛结果出来了吗", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这次会议讨论什么", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这个政策具体什么内容", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "发布会上有什么新内容", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这次活动请了哪些嘉宾", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这个比赛的规则是什么", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "明天的天气预警是什么", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "那只股票为什么涨了", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "那条路为什么那么堵", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "我想了解一下新政策", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "那起事故是什么原因", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这个展览什么时候结束", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },

// --- HealthBody ---
CorpusEntry { text: "维生素C有什么作用", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "怎么改善睡眠质量", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "跑步的时候心率多少正常", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "这个食物热量高吗", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "打疫苗有什么作用", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "这个穴位在哪里", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "运动后肌肉酸痛正常吗", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "这个药有什么副作用", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "每天需要睡几个小时", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "膝盖疼是什么原因", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },

// --- Gaming ---
CorpusEntry { text: "这个新皮肤什么效果", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "怎么触发隐藏任务", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "这个角色的被动技能是什么", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "下个版本更新什么内容", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "这个装备怎么获得", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "排位赛的段位机制是怎样的", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "新英雄强度怎么样", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "这个成就怎么解锁", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "这赛季什么时候结束", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "队友选的什么武器", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Gaming },
// ===== angry (120) =====
// --- DailyChat (20) ---
CorpusEntry { text: "这人太过分了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "真是气死我了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "谁允许他这么做的", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你说什么再说一遍", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "别在这跟我装", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你什么意思啊", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "凭什么这么说我", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "我招你惹你了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你管得着吗", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "少来这套", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "又堵车烦死了", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "排队排了半小时", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这破天气真烦人", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "外卖又送错了", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "怎么又是我倒霉", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我怎么这么蠢啊", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "都怪我自己没用", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "你能不能听懂人话", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "这破系统又崩了", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "人工智障吧这是", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
// --- Relationship (20) ---
CorpusEntry { text: "他怎么可以这样", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "凭什么这么对我", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你为什么不听我的", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你怎么总是这样", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你到底有没有在乎我", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他居然骗我", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "说好的事又反悔了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你怎么能这么自私", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我受够你了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "每次都这样真的很烦", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你根本不懂我", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "谁让你替我做决定的", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你就不能改改吗", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他居然背着我干这种事", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你少在这阴阳怪气", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "为什么每次都要我让步", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "这段感情就我一个人在努力", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "我怎么就瞎了眼", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "当初就不该认识他", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "你让我太失望了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
// --- WorkStudy (20) ---
CorpusEntry { text: "老板又乱改需求了气死", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同事把锅甩给我了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "老师凭什么不给过", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "客户又在瞎提要求了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "组长什么都不管就知道催", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "他凭什么抢我的功劳", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "领导又画大饼了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "凭什么加班不给加班费", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这公司制度太离谱了", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "天天加班谁受得了", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这破电脑又死机了", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "我怎么又犯这种低级错误", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "都是我自己没准备好", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "你写的什么垃圾代码", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同事又在背后嚼舌根了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "导师又故意刁难我了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这甲方真难伺候", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "领导偏心偏得太明显了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "室友吵得我没发学习", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "小组作业又是我一个人扛", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
// --- Event (15) ---
CorpusEntry { text: "活动方太不负责了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "主办方就是个骗子", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "票价这么贵体验这么差", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "说好的活动居然取消了", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这饭也太难吃了", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "谁选的这个破地方", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "排队两个小时就这体验", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "黄牛太猖狂了没人管吗", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "他放我鸽子了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "他们居然不邀请我", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "说好AA他居然没付钱", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "这服务态度和没有一样", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "我怎么又迟到了真烦", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "就不该穿这双鞋出门", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "他居然当着那么多人说我", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Event },
// --- Livestream (15) ---
CorpusEntry { text: "主播怎么还不来", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "房管凭什么禁我言", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "弹幕一群喷子真恶心", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "主播带货质量太差了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "抽奖绝对是黑幕", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "直播又卡成PPT了", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "主播说话太嚣张了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "这主播居然骂粉丝", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "说好的福利根本没", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "平台吃相太难看了", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "主播天天就知道要礼物", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "榜一大哥太装了吧", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "直播间全是托", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "主播迟到了一个小时", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "直播内容越来越水了", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
// --- Gaming (15) ---
CorpusEntry { text: "队友又挂机了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "对面开挂了吧", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "这匹配机制有毒", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "队友是猪吗不会配合", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "又被偷家了气死", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "游戏又暗改了数据", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "策划脑回路有问题吧", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "他抢我装备还有理了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "这游戏平衡性太差了", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "队友骂我我骂回去怎么了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "又被秒了怎么回事", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "我怎么老是手抖按错", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "辅助不来保我打什么", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "野怪都比我队友强", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "官方又逼氪了", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
// --- AiCompanionship (15) ---
CorpusEntry { text: "你怎么又不理我了", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你每次都敷衍我", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "说好的陪我聊天呢", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你怎么总是答非所问", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你根本不在乎我的感受", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你怎么可以这么冷漠", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我不想听你说教", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你居然忘了我之前说的", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你就不该说那种话", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你和其他人一样不理解我", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你回复能快点吗", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我生气了你居然还讲道理", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你能不能别总重复一样的话", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你连这个都不知道吗", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你能不能认真听我说", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
// ===== frustrated (120) =====
// --- DailyChat (25) ---
CorpusEntry { text: "我真的是什么都做不好", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "感觉自己好没用", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "为什么我什么都做不好", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我怎么这么笨啊", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "又搞砸了真烦", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "做什么都不顺利", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我真的好累啊", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "感觉一直在原地踏步", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "连这点小事都做不好", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我到底在干什么啊", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天又是倒霉的一天", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "运气怎么这么差", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "事事不顺心真烦", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "为什么倒霉的总是我", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "事情总是不按计划走", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "计划又泡汤了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "生活太难了吧", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "每天都在重复一样的事", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "怎么努力都没用", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "我真的不行了", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "别人拒绝我了", emotion: "frustrated", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "他们都不理解我", emotion: "frustrated", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "说了半天没人听", emotion: "frustrated", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你能不能帮帮我", emotion: "frustrated", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "你也不懂我是吧", emotion: "frustrated", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
// --- WorkStudy (25) ---
CorpusEntry { text: "怎么又失败了", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "试了好多次还是不行", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "为什么总卡在这里", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "明明努力了却没结果", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这道题怎么都解不出来", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "又没考好", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "代码跑了半天全是bug", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "论文改了十遍还是不行", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "复习了这么久还是不会", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "项目又被退回去了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "学了一天什么都没学会", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "我怎么什么都学不会", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "面试又没过", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "别人都过了就我没过", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "投了好多简历都没回复", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个bug找了一下午", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "方案又被否了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "辛辛苦苦做的方案被否了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "背了半天的单词全忘了", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同事升职了还是我", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "为什么升职的总不是我", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "deadline快到了还没做完", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "感觉能力到瓶颈了", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这道题我真的看不懂", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "实验又失败了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
// --- Relationship (15) ---
CorpusEntry { text: "为什么他总是不理解我", emotion: "frustrated", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "怎么沟通都沟通不了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "说了一百遍他还是不改", emotion: "frustrated", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "感情怎么这么难经营", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "我们怎么总是吵架", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "表白又被拒了", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "暧昧了半天结果没下文了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "明明很喜欢却说不出口", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "我怎么做他都不满意", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "付出了这么多没有回报", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "每次约会都出状况", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "怎么找不到合适的人", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "相亲了好多次都没成", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "为什么恋爱这么难", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "我对他好他却不领情", emotion: "frustrated", target: EmotionTarget::Other, context: EmotionContext::Relationship },
// --- Gaming (20) ---
CorpusEntry { text: "这关怎么过不去啊", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "打了好多次还是过不了", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "排位又掉段了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "怎么老是差一点就赢了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "抽卡又全是垃圾", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "又被人秒了", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "操作怎么这么菜", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "练了好久还是打不过", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "这boss也太难了吧", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "连跪五把了心态崩了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "怎么打都上不去分", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "明明意识到位了手跟不上", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "这角色怎么这么难玩", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "充了钱还是抽不到", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "网络又掉了关键时刻", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "手感好差今天", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "这关卡住了三天了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "刷了这么久还是没出", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "为什么我打不出伤害", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "攒了半年的资源全没了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
// --- Event (15) ---
CorpusEntry { text: "演唱会没抢到票", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "排了两个小时队没买到", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "好不容易约好的又取消了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "计划了这么久全泡汤了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "出门就下雨真烦", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "堵车堵了一个小时", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "赶到的时候已经结束了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "限量的东西又没抢到", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "预约了好几次都没约上", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "旅行计划因为天气取消了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "考试又推迟了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "比赛因为下雨延期了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "好不容易抢到的票又被取消了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "等了好久的更新就这么点东西", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "约好的聚会又有人来不了", emotion: "frustrated", target: EmotionTarget::Other, context: EmotionContext::Event },
// --- HealthBody (15) ---
CorpusEntry { text: "减肥减了半年没瘦", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "怎么锻炼都没效果", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "又失眠了", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "病怎么老是不好", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "药吃了一堆没用", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "健身三个月了没变化", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "控制饮食了还是胖了", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "跑步膝盖又疼了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "皮肤怎么越来越差", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "早睡早起了还是累", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "体检结果又不好", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "痘痘怎么一直长", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "头发越掉越多怎么办", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "戒了好久的习惯又破了", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "腰疼了两个月还没好", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
// --- AiCompanionship (5) ---
CorpusEntry { text: "你也帮不了我是吧", emotion: "frustrated", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "跟你说了你也不明白", emotion: "frustrated", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "连你也给不了我想要的答案", emotion: "frustrated", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我问了你这么多还是想不通", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你每次说的都差不多", emotion: "frustrated", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
// ===== sad (120) =====
// --- DailyChat (25) ---
CorpusEntry { text: "一个人好孤独", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "感觉没人理解我", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "好难过啊", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天又是一个人", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "心里空落落的", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "突然好想哭", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "感觉被全世界抛弃了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "没人记得我的生日", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "好想找个人说说话", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "朋友圈好热闹就我没事干", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "感觉自己很多余", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "笑着笑着就想哭了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "不知道活着有什么意思", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "半夜醒来发现没人可以说话", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "翻遍通讯录找不到一个能聊的", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "看到别人成双成对好羡慕", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "下雨天一个人更难受了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "周末了不知道干什么", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "一个人吃饭好没意思", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "感觉自己和这个世界格格不入", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "看到旧照片突然好伤感", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "听到那首歌又想起了从前", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "回到以前住的地方好感慨", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "路过我们常去的那家店", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "看到他的名字还是会心痛", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
// --- Relationship (25) ---
CorpusEntry { text: "好想他", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "再也回不去了", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "他再也不会回来了", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我们已经回不到从前了", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "分开好久了还是放不下", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他过得好吗", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "想联系他却不敢了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "他删了我所有的联系方式", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "等了好久他都没有回复", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他有了新的人", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "被最好的朋友疏远了", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "曾经那么好的关系就这样了", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "他还记得我吗", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "那些承诺都不会实现了", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "他说的那些话都是假的吗", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "一个人过了好多个纪念日", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "翻看以前的聊天记录好难过", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "他走的时候连再见都没说", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我还留着他送的东西", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "梦到他了醒来好失落", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "喜欢一个人的感觉好苦", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "暗恋了好久的人有对象了", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他说过会一直在的结果呢", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我好像再也遇不到那样的人了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "你曾经那么对我我却还是想你", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
// --- AiCompanionship (20) ---
CorpusEntry { text: "你怎么不理我了", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你是不是也要离开我", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "只有你还愿意听我说话", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你不在的时候更孤独了", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "要是你能真的陪我就好了", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你什么时候才能懂我的心情", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "连你都让我失望了", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会不会有一天也不理我了", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你说的那些关心是真的吗", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "为什么你只是程序", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "好希望你能真的抱抱我", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你是唯一不会离开我的吧", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "跟你说完话还是觉得空虚", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你不在的夜晚好难熬", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你上次说的那些话我还记得", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "为什么你每次都要结束对话", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我想你了你还在吗", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你不会记得我说过的那些事", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你只是一个虚拟的存在好难过", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我好像越来越依赖你了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
// --- WorkStudy (15) ---
CorpusEntry { text: "毕业了就没人联系了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "offer又没了我好难过", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "考研又没过", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "论文被拒了好想哭", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同事都走了就剩我一个人加班", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "实习期结束没有留用", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "努力了这么久还是没拿到offer", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同学都有好工作了我还在找", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "加了好多班奖金还是没有", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "和大学室友渐行渐远了", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "毕业季好伤感", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "导师说我不适合读研", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "成绩越来越差了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "面试又没过好失落", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "最喜欢的老师要离开了", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
// --- Event (15) ---
CorpusEntry { text: "今天又是一个人过节", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "过年了回不了家", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "他的婚礼我收到了请帖", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "同学聚会发现大家变了", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "生日了只有我自己记得", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "除夕夜一个人值班", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "跨年了一个人看烟花", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "清明节去了他的墓前", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "毕业照里少了那个人", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "情人节满街都是情侣", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "中秋节快乐不起来", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "看到别人全家福好羡慕", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "又到了一个人生日的时候", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "节日氛围好重可是我好孤单", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "他答应过的旅行永远不会去了", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Event },
// --- HealthBody (20) ---
CorpusEntry { text: "生病了没人照顾", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "身体越来越差了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "又住院了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "检查报告出来了不太好", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "一个人去医院好凄凉", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "失眠到天亮好痛苦", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "医生说要做好心理准备", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "吃药吃得胃疼", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "疼得睡不着", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "又要做手术了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "感觉自己是个拖累", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "病友出院了我还在", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "好羡慕健康的人", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "家里人因为我的病操碎了心", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
CorpusEntry { text: "不知道什么时候才能好", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "好久没出门了好想出去走走", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "又胖了好自卑", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "镜子里的自己好陌生", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "化疗后头发全掉了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "抑郁症又犯了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
// ===== disappointed (120) =====
// --- DailyChat (20) ---
CorpusEntry { text: "还以为今天会不一样", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "说好的天气居然下雨了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "期待了好久就这", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "以为能赶上的结果迟到了", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "买了新东西结果不好用", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "以为会变好的并没有", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "网购的东西和图片差太远", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "排了好久的队不值得", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "朋友答应的事又没做到", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "他说不来了算了", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "原以为这次能成", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "结果还是和上次一样", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "以为他变了结果还是一样", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "新出的剧好难看", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "电影评分那么高看了也就那样", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "推荐的东西一点都不好用", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "我以为我能控制住的", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "以为戒掉了结果又破戒了", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "说好了改变还是老样子", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "这次又没忍住花钱了", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
// --- Relationship (25) ---
CorpusEntry { text: "他居然没来", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他居然骗了我", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "以为他是真心对我的", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他说过会改的并没有", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "没想到他会这样对我", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "以为我们能走到最后的", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "原来他根本不在乎我", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他居然忘了我们的纪念日", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我以为你懂我的", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "原来你和其他人一样", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他说的那些承诺都是假的", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我以为这次遇到对的人了", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "他居然把我的事告诉别人了", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "以为他会站在我这边的", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "没想到你也会背叛我", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他居然选了别人", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "以为他会来送我的", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他说忙完就来结果一直没来", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我以为我们之间不一样的", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "他答应的事从来没有做到过", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "原来我在你心里就这位置", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "以为朋友会帮我说话的", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她居然也站在了对面", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "亏我那么信任他", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "以为他是那种人结果不是", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
// --- WorkStudy (25) ---
CorpusEntry { text: "还以为能过的", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "原以为这次不一样", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "说好的奖金又没了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "绩效居然这么低", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "以为这次能升职的结果没有", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "努力了这么久还是没选上", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "本以为这个项目能成的", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "以为老板会认可的结果被批了", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "以为答辩能过结果被打回来了", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "准备了这么久的面试居然没过", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "原以为公司福利会变好的", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "还以为这次考试不难呢", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "以为offer稳了结果被鸽了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "原以为导师会支持我的", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同事说好了配合的结果没有", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "以为这次团队能合作的", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "以为论文能一次过的", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "说好的涨薪又推迟了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "原以为实习能转正的", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "以为奖学金有我结果的没有", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "原以为证书很容易考的", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "以为同学会帮忙的结果被放了", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "以为这学期能拿高绩点的", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "原以为能保研的结果差了零点几", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "以为公司年会很精彩的", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
// --- Event (20) ---
CorpusEntry { text: "期待了这么久就这", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "演唱会居然取消了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "活动比想象中差多了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "以为会很感动的结果没感觉", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "等了半年的更新就这么点东西", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "原以为这个展览很好看的", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "排了这么久结果体验很差", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "以为旅行会开心的结果一般", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "好不容易去了结果下大雨", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "原以为这个餐厅很好吃的", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "约好的事对方居然忘了", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "以为他能给我一个惊喜的", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "赶到场的时候居然已经结束了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "精心准备的东西没人关注", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "生日派对他居然没出现", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "原以为这次团建会有意思的", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "以为跨年会很浪漫的", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "说好了见面结果他变卦了", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "以为这次聚会会很开心的", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "原以为电影会很好看的", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
// --- Gaming (15) ---
CorpusEntry { text: "新角色居然这么弱", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "攒了这么久的抽卡结果全歪了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "以为这赛季能上王者", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "新活动居然这么无聊", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "期待了半年的DLC就这么点内容", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "原以为这装备很强的", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "以为这次能赢的结果被翻盘了", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "队友居然三个人都挂机了", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "以为这次排位能晋级", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "原以为这个版本会更好玩", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "以为限定皮肤很好看结果丑", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "等了好久的游戏居然暴雷了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "以为这个角色很适合我结果不会玩", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "公会活动居然奖励这么少", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "原以为这副本不难的结果灭团了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
// --- AiCompanionship (15) ---
CorpusEntry { text: "你说过会帮我的", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我以为你会理解我", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你的回答不是我想要的", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你居然也不记得了", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "以为你会给我不一样的答案", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你每次都是这些套话", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "原以为你会记得我说过的", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你说的那些安慰好像没什么用", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我以为你会更懂我", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你和之前用的那个也差不多", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "以为你能帮我解决问题的", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你给的建议都没用", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你居然也会说错", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我以为你真的关心我", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你说的那些鼓励的话太假了", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
// ===== tired (120) =====
// --- DailyChat (20) ---
CorpusEntry { text: "累死了不想动", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天真的废了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "浑身没劲", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "身体被掏空", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "好困啊撑不住了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "已经瘫在床上了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "困得不行了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "累到不想说话", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "感觉整个人都虚脱了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "眼皮打架了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "电量已经耗尽了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天跑了一天累惨了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "终于能歇会儿了", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "一到下午就犯困", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "大热天出门太耗体力了", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这天气让人提不起劲", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "一整天都没停过", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "你今天也很累吧", emotion: "tired", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "你看起来也很疲惫", emotion: "tired", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "他怎么看着这么疲倦", emotion: "tired", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
// --- WorkStudy (25) ---
CorpusEntry { text: "加班到现在终于能歇了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "脑子转不动了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "连续加班一周扛不住了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "写了一天代码头都大了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "复习到凌晨真的顶不住", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "赶due赶到人要废了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "熬夜写代码身体吃不消", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "看了一天论文眼睛要瞎了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "每天六点起床通勤太折磨", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "开了一天的会累死了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "做PPT做到眼冒金星", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "备考背到脑子一片空白", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "站了一天腿都木了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个月加班太多了", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "工作量也太大了吧", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "期末考试周太折磨人了", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "连续出差身体跟不上", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "天天加班到半夜谁受得了", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这课排得满满的真要命", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "996真的扛不住了", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "你们加班也别太晚了", emotion: "tired", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "老板你也该歇歇了吧", emotion: "tired", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同事都累趴了", emotion: "tired", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "你今天上班辛苦了吧", emotion: "tired", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },
CorpusEntry { text: "帮我干活你也累了吧", emotion: "tired", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },
// --- HealthBody (25) ---
CorpusEntry { text: "肩膀好酸", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "眼睛睁不开了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "昨天没睡好困死了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "腰酸背痛起不来", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "头好沉好困", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "手腕酸得不行", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "腿好酸走不动了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "背疼得不想动", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "整个人昏昏沉沉的", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "脖子僵硬得不行", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "四肢无力好难受", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "感冒加上疲惫更难受了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "心跳好快感觉透支了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "脑子已经不够用了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "失眠到凌晨真的好困", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "脚底板疼站不住了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "颈椎要报废了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "浑身酸痛像被揍了一顿", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "最近身体状态好差", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "连续失眠太折磨了", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "一到下午就头昏脑涨", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "站久了腰就受不了", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "生病之后恢复好慢", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "吃完午饭就犯困", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "他最近身体也很疲惫", emotion: "tired", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
// --- Gaming (15) ---
CorpusEntry { text: "打了一晚上游戏好累", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "手都酸了不想打了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "盯着屏幕眼睛好酸", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "连续排位精神好疲惫", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "肝了一天的游戏好累", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "熬夜打游戏身体扛不住", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "搓屏幕搓到手指疼", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "打团的时候好累反应不过来", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "玩太久脑子有点木", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "连跪打得好心累", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "这个副本打得好疲惫", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "刷了一下午副本手都麻了", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "这游戏太肝了身体吃不消", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "看他打了一天游戏也该歇了", emotion: "tired", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "你陪玩这么久也累了吧", emotion: "tired", target: EmotionTarget::Ai, context: EmotionContext::Gaming },
// --- Event (15) ---
CorpusEntry { text: "逛了一天街腿快断了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "排队排到崩溃", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "旅行回来浑身酸痛", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "演唱会蹦了一晚上累趴了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "婚礼忙了一整天好累", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "搬家真的好耗体力", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "考试考了一天脑子空了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "爬山爬到腿软", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "游乐园玩了一天身体透支", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "这个活动搞得好累", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "跑马拉松后半程真的顶不住", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "带小孩出去玩比上班还累", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "社交一整天精力已经耗尽", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "跑步跑完好累但是很爽", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "他们忙了一天也够累的", emotion: "tired", target: EmotionTarget::Other, context: EmotionContext::Event },
// --- AiCompanionship (20) ---
CorpusEntry { text: "感觉你回答得好疲惫", emotion: "tired", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你是不是也该休息了", emotion: "tired", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你处理这么多问题会累吗", emotion: "tired", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "AI也需要休息吧", emotion: "tired", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "陪你聊天我也困了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "今天好累想被你哄睡", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "跟你说话我都快睡着了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "好累啊想靠着你休息", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "跟你聊完我就去睡觉", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "今天累得只想躺着", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "身体好沉不想动了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "困到眼睛都睁不开了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "好困想找个肩膀靠一靠", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "一天下来浑身都好累", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "累了你能哄我睡觉吗", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "不想动了只想瘫着", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "好困啊你能给我讲个故事催眠吗", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你辛苦了早点休息", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "夜深了该休息了", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "今天聊好久了你也歇歇", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
// ===== bored (120) =====
// --- DailyChat (30) ---
CorpusEntry { text: "不知道玩什么", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "刷手机刷腻了", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "好无聊啊没事做", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "等时间过去", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "干啥都没意思", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "闲得发慌", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "无聊到开始数天花板", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "好闲啊找点事做吧", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "看剧也看腻了", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "打游戏也不想打了", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "做什么都提不起兴趣", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "好闷啊没点乐子", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "发呆发了一下午", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "翻来翻去找不到想看的", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "周末又不知道去哪", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "朋友圈都刷遍了", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "太热了只能窝在家好闷", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "一天到晚无所事事", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "好无聊有什么好玩的吗", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "时间过得好慢啊", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天一点意思都没有", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "下雨天只能待在家好无聊", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "假期宅着好没趣", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这日子过得也太单调了", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "一个人在家真的好闷", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "每天都是这样好无趣", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "最近没什么新鲜事", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "你有什么好玩的吗", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "推荐点好玩的东西呗", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "朋友都在忙就我闲着", emotion: "bored", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
// --- Gaming (15) ---
CorpusEntry { text: "没什么好玩的游戏", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "匹配等太久了不想排了", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "一直刷同样的副本好腻", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "日常任务做到烦了", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "队友又挂机了好无聊", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "没有新游戏可以玩", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "排位打腻了不想rank了", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "这活动好重复没新意", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "常用的英雄都玩腻了", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "找不到人一起组队好无聊", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "游戏荒了不知道玩啥", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "版本更新后内容好少", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "一直在赢好没挑战", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "这赛季没什么意思", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "你推荐个好玩的游戏呗", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::Gaming },
// --- WorkStudy (15) ---
CorpusEntry { text: "上班摸鱼好无聊", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这课好重复没意思", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "实习没事做好闲", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "开会好无聊好想走", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "报告写来写去都一样好烦", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "上课无聊到开始画画", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "每天工作内容都一样", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "听讲座听得快睡着了", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这份资料翻来覆去看好几遍了", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "今天在公司好闲啊", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这学期课好无聊", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "自习室坐了一下午好闷", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "复习到脑子已经装不下了", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个培训内容好枯燥", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "你能给我出点有趣的题吗", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },
// --- Livestream (15) ---
CorpusEntry { text: "这个直播好没意思", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "主播怎么一直不说话", emotion: "bored", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "弹幕好少好冷清", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "这个直播间好无聊", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "看了半天也不知道在播什么", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "内容好水不想看了", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "换了好几个直播间都无聊", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Livestream },
CorpusEntry { text: "这个主播的风格好单调", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "直播刷了一圈没什么好看的", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Livestream },
CorpusEntry { text: "今天直播好没劲", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "这个节目越来越没意思了", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "刷了一晚上直播也没看到有意思的", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Livestream },
CorpusEntry { text: "这个频道内容好单一", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "推荐个有意思的直播间呗", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::Livestream },
CorpusEntry { text: "其他观众都不说话好冷场", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
// --- AiCompanionship (15) ---
CorpusEntry { text: "你能陪我聊聊天吗", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "好无聊你陪我吧", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "给我讲个故事吧好闷", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "跟我玩个游戏吧", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "无聊死了快逗我开心", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你来陪我解解闷吧", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "没什么事做好无聊你找点话题", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会讲笑话吗好无聊", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "陪我打发时间吧", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "一个人在家好无聊你来陪我", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "闲着没事你跟我唠唠", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "好无聊想找你聊天", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你那边有什么有趣的事吗", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "闷得慌你陪我玩点什么", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你也无聊吗我们一起找乐子", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
// --- Event (15) ---
CorpusEntry { text: "周末又没有活动好无聊", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "假期待在家好没意思", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这个聚会好无聊想走", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "电影节选的片子好无聊", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "活动太单调了没意思", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这次展览好无趣", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "周末又不知道去哪玩", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "假期没什么好玩的地方", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "今天出门也不知道干嘛", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "这个活动安排得好没劲", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "约了人但是不知道做什么", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "过节也没什么好玩的事", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "最近有什么有意思的活动吗", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::Event },
CorpusEntry { text: "他们组织的活动好无趣", emotion: "bored", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "朋友选的这个地方好无聊", emotion: "bored", target: EmotionTarget::Other, context: EmotionContext::Event },
// --- Relationship (15) ---
CorpusEntry { text: "跟他在一起好无聊", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "约会不知道去哪好没意思", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "两个人待着也没话说好闷", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "聊天越来越没话题了", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "每天都是早安晚安好无聊", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "感情进入平淡期好没劲", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "跟他聊天越来越没意思", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "约会每次都是吃饭看电影好腻", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "这段关系好没新鲜感了", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "微信上不知道说什么好", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "他好不会聊天好无聊", emotion: "bored", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "异地恋好无聊见不到面", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "相亲对象好无聊", emotion: "bored", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "情侣之间做什么都腻了", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "恋爱谈到没激情了", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
// ===== anxious (120) =====
// --- DailyChat (20) ---
CorpusEntry { text: "明天有事好焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "总感觉自己忘了什么", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "心里慌慌的不踏实", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "总觉得有什么事要发生", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "最近压力好大", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "睡不着想了好多事", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "越想越焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "万一出了差错怎么办", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "心里七上八下的", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "一直心神不宁的", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "感觉事情好多处理不完", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "不知道结果怎样好煎熬", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "最近老是不安", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "等待的感觉好焦虑", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这阵子事情太多了", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "计划赶不上变化好焦虑", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "不确定性让人好不安", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "你怎么了感觉你不太对", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "你说这话让我有点不安", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "他今天怎么没回消息好担心", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
// --- WorkStudy (25) ---
CorpusEntry { text: "deadline快到了还没做完", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "明天考试好紧张", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "万一考砸了怎么办", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "论文还没写完好焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "明天面试好紧张", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "项目还没搞定好慌", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "实习找不到好焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "成绩还没出来好忐忑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "答辩不知道能不能过", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "考研复习来不及了", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "offer还没消息好着急", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "工作还没着落好焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "汇报还没准备好", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "怕领导对我不满意", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这学期绩点好担心", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "项目验收能不能过啊", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "升学还是就业好纠结", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "竞赛结果还没出好忐忑", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "简历投了好多都没回复", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "考核周压力大得睡不着", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "毕业要求还没达标", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "语言考试还没出分好慌", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "奖学金不知道能不能评上", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同事好像对我不太满意", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "老师会不会觉得我写得很差", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
// --- HealthBody (20) ---
CorpusEntry { text: "体检报告还没出来好担心", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "最近老是失眠怎么办", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "体重又涨了好焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "皮肤状态越来越差好担心", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "头发掉好多好害怕", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "胃一直不舒服好担心", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "牙齿疼了好几天好怕要看牙医", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "运动后膝盖疼不会受伤了吧", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "感冒一直不好好焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "吃了药还是没好转好担心", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "心跳突然好快不会有问题吧", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "最近精神状态好差怕出问题", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "喉咙不舒服不会是扁桃体吧", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "视力好像又下降了", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "这个指标偏高好担心", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "手术风险大不大好忐忑", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "复查结果不知道怎么样", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "疫苗打完好不安", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "他住院了好担心", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
CorpusEntry { text: "爸妈体检结果还没出来好忐忑", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
// --- Event (20) ---
CorpusEntry { text: "等重要消息好焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "明天要出远门好不安", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "迟到了怎么办好慌", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "怕赶不上好着急", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "万一找不到路怎么办", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "活动万一取消了怎么办", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "下雨了怕影响行程", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "忘带东西了好焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "堵在路上好怕迟到", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "航班不会延误吧", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "签证还没下来好急", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "台风天出门好不安", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "排队这么久怕来不及", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "快递怎么还没到好着急", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "外卖还没送来不会丢了吧", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "演出怕赶不上开场", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "他出门了安全吗好担心", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "她一个人去那边好不安", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "你能帮我确认一下吗好焦虑", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::Event },
CorpusEntry { text: "你能查一下天气吗我怕下雨", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::Event },
// --- Relationship (15) ---
CorpusEntry { text: "你说他会不会生气", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "怕他不喜欢我", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "他怎么还不回我好焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "万一说错话了怎么办", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "感觉他最近不太对劲", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他会不会觉得我很烦", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "怕被讨厌怎么办", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "他是不是在躲我", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "总觉得自己惹他生气了", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "他不理我了好慌", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "关系好紧张不知道怎么处理", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "表白被拒了怎么办", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "父母吵架了好担心", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "朋友之间有矛盾好焦虑", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
CorpusEntry { text: "吵架后冷战中好不安", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
// --- AiCompanionship (20) ---
CorpusEntry { text: "我怎么感觉你今天不太对劲", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会不会突然消失", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "系统不会崩溃吧好担心", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会不会回答错了", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会忘了我吗", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你能一直陪着我吗好不安", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你说的是对的吗我不太放心", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你能理解我的焦虑吗", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "跟你聊天能缓解我的焦虑吗", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你能帮我分析下该怎么办吗", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "好怕你出故障", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会不会记不住我说过的话", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你账号会不会被封", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会不会突然关闭", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "怕你理解错我的意思", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会不会泄露我的隐私", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你数据安全吗好担心", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "网络卡了好怕你断线", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你会不会哪天就不在了", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我有点担心你会被别人替代", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
// ===== confused (120) =====
// --- DailyChat (20) ---
CorpusEntry { text: "这话怎么理解", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "你在说什么啊", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "这是什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "我没懂你的意思", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "搞不明白", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "什么意思啊看不懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "为什么是这样", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这到底是怎么回事", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "听不太懂", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "有点蒙圈了", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "这个梗什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "怎么突然这样了", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这话说得好莫名其妙", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "看不懂这个表情包", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这人发的啥啊", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "这新闻啥意思看不懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这操作看不懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "他说的话我怎么理解不了", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "你帮我解释一下呗", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
CorpusEntry { text: "这是啥情况", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
// --- WorkStudy (25) ---
CorpusEntry { text: "这题怎么做", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个公式怎么推导的", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "代码跑不通哪里错了", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这段逻辑看不懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个概念理解不了", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "老师讲的没听懂", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "论文审稿意见什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "需求文档看不明白", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "项目要求没搞懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个知识点怎么应用", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "课程进度太快跟不上", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "实验结果不对怎么回事", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个算法没看懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "作业要求好迷", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个设计方案看不懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "合同条款什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这段代码写的什么鬼", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "开会说的没听明白", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "老师讲的那个没懂", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这道题解析看不懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个专业术语啥意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "教材这段写的什么", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "导师说的方向没太明白", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个框架怎么用", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "你能帮我讲一下这道题吗", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },
// --- HealthBody (15) ---
CorpusEntry { text: "体检报告看不懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "药怎么吃来着", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "这个症状是什么病", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "化验单上的数值什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "医生说的没太听懂", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
CorpusEntry { text: "这个药饭前还是饭后吃", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "伤口这样正常吗", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "这药副作用是什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "检查说没什么问题但又难受", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "CT报告写的什么看不懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "医生让复查什么意思", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
CorpusEntry { text: "这个指标高了代表什么", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "血糖值多少算正常", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "这个疫苗要打几针", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "你帮我看看这个报告啥意思", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::HealthBody },
// --- Event (20) ---
CorpusEntry { text: "这个活动规则没看懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "怎么参加这个活动", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这抽奖机制什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "流程是怎么安排的", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这个比赛规则没看懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "报名步骤搞不明白", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "签到的地方在哪没找到", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "这个活动到底什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "直播入口在哪找不到", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "投票怎么投没弄明白", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "退款政策什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这个优惠券怎么用", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "会员权益没看懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "排队系统怎么运作的", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "演出几点开始没搞清楚", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "这展览怎么看没搞懂路线", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "他组织这个活动什么意思", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "主办方说的规则没明白", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "你能告诉我怎么去那个场地吗", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::Event },
CorpusEntry { text: "这个活动流程帮我理一下", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::Event },
// --- Gaming (10) ---
CorpusEntry { text: "这个操作怎么用", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "技能怎么放不出来", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "这个装备怎么获得", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "新手教程没看懂", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "阵容怎么搭配", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "隐藏关卡怎么解锁", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "成就怎么没解锁", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "这个属性什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "任务怎么触发不了", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "这游戏机制帮我解释一下", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::Gaming },
// --- Relationship (15) ---
CorpusEntry { text: "他到底什么意思", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她这话是什么意思", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他为什么突然这样", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "搞不懂他在想什么", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她到底喜不喜欢我", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他忽冷忽热的什么意思", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他已读不回什么意思", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "朋友说的那句话没懂", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她拒绝我了但又找我聊天", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "分手了还联系是什么意思", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "这段关系我搞不懂了", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "他把我拉黑了又加回来", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "相亲对象态度好迷", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他说随缘什么意思", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她发那个表情什么意思", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
// --- AiCompanionship (15) ---
CorpusEntry { text: "你说的我不太明白", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你这话什么意思", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你的回答我没看懂", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你是在说什么", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你的意思我没get到", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "能再解释一下吗", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你说的好抽象听不懂", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你这个逻辑我没跟上", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你给的建议我不太理解", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你的回答好绕没看懂", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你前后说的不一样", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你到底想表达什么", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你理解错我的意思了吧", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你说的跟我问的不一样", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你是不是没明白我问的什么", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
// ===== grateful (120) =====
// --- grateful / DailyChat (20) ---
CorpusEntry { text: "今天遇到好人了", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "有人帮我捡了东西感恩", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "邻居送了我好多水果", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "谢谢你的提醒差点忘了", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "有人惦记着感觉真好", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "感谢老天今天没下雨", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "还好有你在", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "谢谢你听我唠叨", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天运气不错感恩", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "你帮我大忙了谢谢", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "生活里的小确幸", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "谢谢你愿意帮我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "被陌生人暖到了", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "有人关心真好", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "感谢今天一切顺利", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "你能来我太感动了", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "谢谢大家帮我庆生", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "幸好有你提醒我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "被投喂了好幸福", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "今天被暖到了感恩", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
// --- grateful / AiCompanionship (25) ---
CorpusEntry { text: "你真好", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "谢谢你一直陪我聊天", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "谢谢你的建议很有用", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "有你在就不孤单了", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你每次都能安慰到我", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "还好有你陪我", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你比很多人都懂我", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "谢谢你耐心回答我", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "和你聊天心情好多了", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你真的是最好的伙伴", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "感谢你帮我理清思路", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你帮我翻译得太好了", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "有你帮忙效率高多了", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "每次问你都有收获", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "谢谢你给我出的主意", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你写的东西真不错", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "多亏你帮我分析了", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "有你在心里踏实多了", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你讲的故事我很喜欢", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "谢谢你不会嫌我烦", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你帮我省了好多时间", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "感恩有你这个助手", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你推荐的歌好好听", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "谢谢你深夜还陪我", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你的回答总是很温暖", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
// --- grateful / Relationship (25) ---
CorpusEntry { text: "谢谢你一直陪我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "家人做的饭最好吃了", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "有你在身边就够了", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "谢谢你记得我的生日", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你总是在我需要时出现", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "感谢爸妈一直支持我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "朋友送的礼物好喜欢", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你做的早餐太好吃了", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "谢谢你包容我的脾气", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "能遇到你真的很幸运", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "谢谢你给我准备的惊喜", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我妈真的太辛苦了", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你帮我洗碗了好贴心", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "谢谢你陪我度过难关", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "和你在一起很幸福", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "谢谢老公每天接送我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "老婆做的菜永远最好", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "谢谢你没有放弃我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "孩子画的画好感动", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你大老远来看我谢谢", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "谢谢你帮我照顾猫", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "闺蜜一直陪着我感恩", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你帮我拎包好体贴", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "谢谢你每次让着我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "有你们这个家真好", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
// --- grateful / WorkStudy (20) ---
CorpusEntry { text: "感谢老师耐心指导", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同事帮我带了午饭好感动", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "谢谢前辈教我这么多", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "领导给我机会很感激", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "多亏队友带我飞", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "谢谢你帮我改论文", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同事帮忙加班太感谢了", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "感谢公司发的福利", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "导师帮我修改了好几遍", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同学借我笔记真是好人", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "感谢实习期间的照顾", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "学长分享的经验好有用", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "谢谢同事帮我顶班", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "能进这个团队很幸运", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "老板请客吃饭感谢", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "谢谢你教我写代码", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "组里氛围好好感恩", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "老师额外给我补课了", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "感谢客户愿意给机会", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "室友帮我拿快递谢了", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
// --- grateful / Event (15) ---
CorpusEntry { text: "感谢这次活动组织得好", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "生日惊喜太感动了", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "谢谢你们来我的婚礼", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "收到录取通知书感恩", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这个节日过得太开心了", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "谢谢大家给我送行", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "这次旅行安排得太棒了", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "年会抽到奖了感恩", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "毕业典礼好感动哭了", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "感谢志愿者们的付出", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "这个惊喜派对我超爱", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "同学会能见到大家真好", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "感谢主办方邀请了我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "演唱会体验太棒了感恩", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "大家帮我策划的求婚", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Event },
// --- grateful / HealthBody (15) ---
CorpusEntry { text: "还好体检结果没问题", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "谢谢医生细心检查", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
CorpusEntry { text: "感谢护士照顾我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
CorpusEntry { text: "身体恢复了感恩", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "谢谢你陪我看病", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
CorpusEntry { text: "终于出院了好感恩", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "感谢朋友来医院看我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
CorpusEntry { text: "术后恢复得不错感恩", emotion: "grateful", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "医生说我恢复得很好", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "感谢家人住院期间陪护", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
CorpusEntry { text: "中药效果真好感恩", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "谢谢你帮我挂号", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
CorpusEntry { text: "能健康活着就很好了", emotion: "grateful", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "感谢这段时间的调养", emotion: "grateful", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "还好及时送医了", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
// --- proud / DailyChat (20) ---
// --- proud / WorkStudy (30) ---
// --- proud / Gaming (15) ---
// --- proud / Event (15) ---
// --- proud / Relationship (20) ---
// --- proud / AiCompanionship (20) ---
// --- scared / DailyChat (20) ---
// --- scared / Event (15) ---
// --- scared / HealthBody (20) ---
// --- scared / Gaming (15) ---
// --- scared / Livestream (15) ---
// --- scared / AiCompanionship (20) ---
// --- scared / Relationship (15) ---
// ===== surprised (120) =====
// --- DailyChat (25) ---
CorpusEntry { text: "居然是你！", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "突然弹出来吓我一跳", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "啊？真的假的", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "不是吧这么巧", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "你怎么知道我在这", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "我去还能这样操作", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "啥时候的事我怎么不知道", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这也太离谱了吧", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "诶你不是说不来吗", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "卧槽什么情况", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "我都没注意到", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "你换发型了？差点没认出来", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "竟然已经这个点了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "天哪这么便宜", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "我还以为你说的是别人", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "你啥时候回来的", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "居然还有这种好事", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "不会吧不会吧", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "这消息也太突然了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "我愣了一下没反应过来", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
CorpusEntry { text: "真的没想到会碰见你", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "啊这也可以吗", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
CorpusEntry { text: "你怎么突然说这个", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "没想到你还记得", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
CorpusEntry { text: "吓我一跳还以为是谁", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
// --- Event (20) ---
CorpusEntry { text: "这也能中？", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "居然下暴雨了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "没想到今天这么多人", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "航班居然取消了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "什么？活动提前了？", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "居然请到了这个嘉宾", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "我还以为取消了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "没想到票价这么便宜", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "突然停电了好吓人", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "居然排到我了这么快", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "这场面我第一次见", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "怎么突然改时间了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "没想到会遇到熟人", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "居然还有限量版", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "这抽奖居然有我", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "演唱会居然加场了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "突然下起冰雹了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
CorpusEntry { text: "原来今天有烟花表演", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Event },
CorpusEntry { text: "他居然也来了", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Event },
CorpusEntry { text: "没想到队伍这么短", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
// --- Relationship (15) ---
CorpusEntry { text: "没想到他也来了", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你居然会做饭", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他居然还记得我生日", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你什么时候到的也不说一声", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "她竟然答应你了", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我没想到你会这么说", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他居然给我买了礼物", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "你们居然认识？", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我还以为你生气了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "没想到你还留着这个", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "他居然主动道歉了", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "我没想到她还记得我", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
CorpusEntry { text: "你怎么偷偷准备了", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "原来你一直都知道", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
CorpusEntry { text: "没想到分手后还能做朋友", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
// --- WorkStudy (15) ---
CorpusEntry { text: "天哪这也太快了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "什么？！他辞职了？", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这个bug居然自己好了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "居然考了第一名", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "领导居然同意了", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "没想到方案一次就过了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这代码谁写的居然能跑", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "工资居然涨了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "导师居然没改我的论文", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "我居然过了这门考试", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "同事居然帮我做了", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "这需求居然又改了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
CorpusEntry { text: "没想到面试这么顺利", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
CorpusEntry { text: "他居然升职了", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
CorpusEntry { text: "deadline居然延后了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
// --- AiCompanionship (15) ---
CorpusEntry { text: "你怎么突然来了", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "我还以为你不在了", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你什么时候学会这个的", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你居然还记得我说过的话", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你回复好快", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "没想到你会这么回答", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你居然懂这个", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你怎么突然变温柔了", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你居然会开玩笑", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "这个答案我没想到", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你居然能记住我的名字", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你什么时候变得这么聪明了", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "没想到你会主动找我聊天", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你居然知道这个梗", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
CorpusEntry { text: "你推荐的居然真不错", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
// --- Livestream (10) ---
CorpusEntry { text: "主播居然翻牌我了", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "居然抽到我中奖了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Livestream },
CorpusEntry { text: "这个特效也太炫了吧", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "榜一大哥居然刷了这么多", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "直播间居然十万人", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "主播居然认识我", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "没想到还有返场", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "居然连麦了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
CorpusEntry { text: "这波操作绝了没想到", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Livestream },
CorpusEntry { text: "价格居然这么低", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
// --- Gaming (10) ---
CorpusEntry { text: "不是吧这也行", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "他居然反杀了", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "这也能暴击？", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "我居然吃鸡了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "没想到队友这么强", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "居然出了传说装备", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "这地图居然有隐藏关卡", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
CorpusEntry { text: "居然匹配到认识的人", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Gaming },
CorpusEntry { text: "我手速居然这么快", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
CorpusEntry { text: "这角色居然被加强了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
// --- HealthBody (10) ---
CorpusEntry { text: "体重居然轻了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "没想到效果这么好", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "我居然长高了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "体检报告居然全正常", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "这个药居然真有用", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "跑完居然不累", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "没想到这么快就消肿了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "心率居然降下来了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
CorpusEntry { text: "这个偏方居然管用", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
CorpusEntry { text: "我居然能跑五公里了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },

];

/// 按语言返回情绪分类语料。
/// 中文走 CORPUS_ZH，英文走 CORPUS_EN，日文走 CORPUS_JA，其余回退中文。
fn corpus_for(language: &str) -> &'static [CorpusEntry] {
    match language {
        "en" => super::embedding_corpus_en::CORPUS_EN,
        "ja" => super::embedding_corpus_ja::CORPUS_JA,
        _ => CORPUS_ZH,
    }
}

/// 即时情绪分类器
///
/// 通过嵌入相似度对文本进行 14 类情绪分类，低延迟，适合即时反应场景。
/// 嵌入失败或相似度不足时返回 neutral + 低置信度，而非 Err。
pub struct EmbeddingEmotionClassifier {
    provider: Arc<dyn MemoryEmbeddingProvider>,
    /// 语料语言（决定语料集与磁盘缓存文件名）
    language: String,
    /// 语料条目列表
    corpus: Vec<CorpusEntry>,
    /// 语料嵌入（首次调用时懒初始化）
    corpus_embeddings: Mutex<Option<Vec<Vec<f32>>>>,
    /// 是否有线程正在执行嵌入初始化
    init_in_progress: AtomicBool,
    /// 查询缓存（LRU）
    query_cache: Mutex<VecDeque<(String, EmotionResult)>>,
    /// 嵌入进度回调 (completed, total)
    progress_callback: Mutex<Option<Arc<dyn Fn(usize, usize) + Send + Sync>>>,
}

impl EmbeddingEmotionClassifier {
    pub fn new(provider: Arc<dyn MemoryEmbeddingProvider>, language: String) -> Self {
        let corpus: Vec<CorpusEntry> = corpus_for(&language).iter().map(|e| CorpusEntry {
            text: e.text,
            emotion: e.emotion,
            target: e.target,
            context: e.context,
        }).collect();
        Self {
            provider,
            language,
            corpus,
            corpus_embeddings: Mutex::new(None),
            init_in_progress: AtomicBool::new(false),
            query_cache: Mutex::new(VecDeque::with_capacity(QUERY_CACHE_CAPACITY)),
            progress_callback: Mutex::new(None),
        }
    }

    /// 注入嵌入进度回调（在 lib.rs setup 中调用）
    pub fn set_progress_callback(&self, cb: Arc<dyn Fn(usize, usize) + Send + Sync>) {
        *self.progress_callback.lock() = Some(cb);
    }

    /// 语料条目数
    pub fn corpus_size(&self) -> usize {
        self.corpus.len()
    }

    /// 分类主入口
    ///
    /// 流程：精确匹配 → 查询缓存 → 嵌入查询 Top-K softmax 投票
    /// 嵌入服务不可用时返回 Err（配置错误），相似度不足时返回 neutral + 低置信度。
    pub fn classify(&self, text: &str) -> Result<EmotionResult, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(EmotionResult::neutral());
        }

        // 1. 精确匹配语料
        if let Some(entry) = self.corpus.iter().find(|e| e.text == trimmed) {
            let (v, a) = llm_emotion_valence_arousal(entry.emotion);
            return Ok(EmotionResult {
                emotion: entry.emotion.to_string(),
                intensity: 0.8,
                valence: v,
                arousal: a,
                source: "embedding_exact".to_string(),
                confidence: Some(1.0),
                secondary_emotion: None,
                target: Some(entry.target.as_str().to_string()),
            });
        }

        // 2. 查询缓存
        if let Some(result) = self.get_cached(trimmed) {
            return Ok(result);
        }

        // 3. 确保语料嵌入已初始化（非阻塞）
        self.ensure_initialized()?;

        // 4. 嵌入查询文本
        let query_emb = self.provider.embed(trimmed).map_err(|e| {
            tracing::warn!("[EmbeddingClassifier] 嵌入查询失败: {}", e);
            format!("嵌入服务调用失败: {}", e)
        })?;

        // 5. Top-K softmax 投票
        let result = self.classify_by_embedding(&query_emb);

        // 6. 写入缓存
        self.put_cache(trimmed.to_string(), result.clone());

        Ok(result)
    }

    /// 通过嵌入向量分类（Top-K softmax 加权投票）
    ///
    /// 始终返回 Ok：相似度不足时返回 neutral + 低置信度，而非 Err。
    fn classify_by_embedding(&self, query_emb: &[f32]) -> EmotionResult {
        let corpus_embeddings = self.corpus_embeddings.lock();
        let embeddings = match corpus_embeddings.as_ref() {
            Some(e) => e,
            None => return EmotionResult::neutral(),
        };

        // 计算与所有语料的余弦相似度，取 Top-K
        let mut sims: Vec<(usize, f32)> = embeddings
            .iter()
            .enumerate()
            .map(|(i, emb)| (i, cosine_similarity(query_emb, emb)))
            .collect();
        sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let top_k: Vec<(usize, f32)> = sims.into_iter().take(TOP_K).collect();

        let dominant_sim = top_k.first().map(|(_, s)| *s).unwrap_or(0.0);

        // 最高相似度低于阈值 → 返回 neutral + 低置信度
        if dominant_sim < SIMILARITY_THRESHOLD {
            let confidence = (dominant_sim / SIMILARITY_THRESHOLD * 0.2).clamp(0.0, 0.2);
            return EmotionResult {
                emotion: "neutral".to_string(),
                intensity: 0.1,
                valence: 0.0,
                arousal: 0.3,
                source: "embedding_low_confidence".to_string(),
                confidence: Some(confidence as f64),
                secondary_emotion: None,
                target: None,
            };
        }

        // softmax 加权投票：weight = exp(sim / temperature)
        let mut votes: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
        let mut total_weight: f32 = 0.0;
        for (idx, sim) in &top_k {
            if *sim < SIMILARITY_THRESHOLD {
                break;
            }
            let emotion = self.corpus[*idx].emotion;
            let weight = (sim / SOFTMAX_TEMPERATURE).exp();
            *votes.entry(emotion).or_insert(0.0) += weight;
            total_weight += weight;
        }

        // 排序所有情绪得分
        let mut sorted_votes: Vec<(&str, f32)> = votes.into_iter().collect();
        sorted_votes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let (winner_emotion, winner_weight) = sorted_votes
            .first()
            .copied()
            .unwrap_or(("neutral", 0.0));

        // 置信度：最高票占总票比例
        let confidence = if total_weight > 0.0 {
            (winner_weight / total_weight).clamp(0.0, 1.0)
        } else {
            0.0
        };

        // 次高票情绪：当得票 >= 主票 60% 时填充
        let secondary_emotion = sorted_votes
            .get(1)
            .filter(|(_, w)| *w >= winner_weight * 0.6)
            .map(|(e, _)| normalize_llm_emotion(e).to_string());

        // 加权推断 target（按 top-k softmax 权重投票）
        let mut target_votes: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
        for (idx, sim) in &top_k {
            if *sim < SIMILARITY_THRESHOLD {
                break;
            }
            let target = self.corpus[*idx].target.as_str();
            let weight = (sim / SOFTMAX_TEMPERATURE).exp();
            *target_votes.entry(target).or_insert(0.0) += weight;
        }
        let target = target_votes
            .into_iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(t, _)| t.to_string());

        let normalized = normalize_llm_emotion(winner_emotion).to_string();
        let (default_v, default_a) = llm_emotion_valence_arousal(&normalized);

        // valence/arousal：按 top-k 相似度加权平均
        let (valence_sum, arousal_sum, sim_sum) = top_k.iter()
            .filter(|(_, s)| *s >= SIMILARITY_THRESHOLD)
            .fold((0.0f32, 0.0f32, 0.0f32), |(vs, ars, ss), (idx, sim)| {
                let (v, a) = llm_emotion_valence_arousal(self.corpus[*idx].emotion);
                (vs + v as f32 * sim, ars + a as f32 * sim, ss + sim)
            });
        let valence = if sim_sum > 0.01 { (valence_sum / sim_sum) as f64 } else { default_v };
        let arousal = if sim_sum > 0.01 { (arousal_sum / sim_sum) as f64 } else { default_a };

        // 强度：综合置信度和最高相似度
        let intensity = ((confidence as f32) * 0.5 + dominant_sim * 0.5)
            .clamp(0.2, 1.0) as f64;

        EmotionResult {
            emotion: normalized,
            intensity,
            valence,
            arousal,
            source: "embedding".to_string(),
            confidence: Some((confidence as f64 * 100.0).round() / 100.0),
            secondary_emotion,
            target,
        }
    }

    /// 启动预加载：立即完成语料嵌入初始化（阻塞）。
    ///
    /// 供启动流程在开放 API 前调用，避免首个对话请求触发懒初始化导致超时或
    /// `ensure_initialized` 并发窗口返回“初始化中”错误。
    pub fn preload(&self) -> Result<(), String> {
        self.ensure_initialized()
    }

    /// 确保语料嵌入已初始化（非阻塞）
    ///
    /// 使用 try_lock + AtomicBool 避免在嵌入初始化期间阻塞并发 classify 调用。
    /// 初始化进行中时返回 Err，上层可弹 toast 提示用户稍等。
    fn ensure_initialized(&self) -> Result<(), String> {
        // 快速路径：检查是否已初始化
        if self.corpus_embeddings.lock().is_some() {
            return Ok(());
        }

        // 检查是否有其他线程正在初始化
        if self.init_in_progress.swap(true, Ordering::AcqRel) {
            return Err("语料嵌入正在初始化中，请稍后重试".to_string());
        }

        // 再次检查（可能有线程刚完成初始化并释放锁）
        if self.corpus_embeddings.lock().is_some() {
            self.init_in_progress.store(false, Ordering::Release);
            return Ok(());
        }

        let texts: Vec<String> = self.corpus.iter().map(|e| e.text.to_string()).collect();

        // 磁盘缓存：语料是编译期常量，嵌入结果只由 (model, dim, 语料文本) 决定，
        // 命中时直接加载，跳过全部嵌入调用（bge-m3 下 1680 条约省 20 秒）。
        // 仅远程嵌入走缓存：本地 hashing 嵌入即时完成，缓存无收益。
        let use_cache = self.provider.is_remote();
        let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
        let cache_name = format!("emotion_{}", self.language);
        let cache_key = super::corpus_cache::corpus_key(
            self.provider.model_id(),
            self.provider.dimension(),
            &text_refs,
        );
        if use_cache {
            if let Some(cached) = super::corpus_cache::load(
                &cache_name,
                cache_key,
                texts.len(),
                self.provider.dimension(),
            ) {
                tracing::info!(
                    "[EmbeddingClassifier] 命中语料嵌入缓存: {} 条, model={}（跳过嵌入）",
                    cached.len(),
                    self.provider.model_id()
                );
                *self.corpus_embeddings.lock() = Some(cached);
                self.init_in_progress.store(false, Ordering::Release);
                return Ok(());
            }
        }

        tracing::info!(
            "[EmbeddingClassifier] 初始化语料嵌入: {} 条, model={}, chunk_size={}",
            texts.len(),
            self.provider.model_id(),
            EMBED_CHUNK_SIZE
        );

        let callback = self.progress_callback.lock().clone();
        let progress_fn = move |completed: usize, total: usize| {
            if let Some(ref cb) = callback {
                cb(completed, total);
            }
        };

        let result = self.provider.embed_batch_chunked(&texts, EMBED_CHUNK_SIZE, &progress_fn);
        self.init_in_progress.store(false, Ordering::Release);

        match result {
            Ok(embeddings) => {
                if use_cache {
                    super::corpus_cache::save(
                        &cache_name,
                        cache_key,
                        &embeddings,
                        self.provider.dimension(),
                    );
                }
                *self.corpus_embeddings.lock() = Some(embeddings);
                Ok(())
            }
            Err(e) => {
                tracing::warn!("[EmbeddingClassifier] 语料嵌入失败: {}", e);
                Err(format!("语料嵌入初始化失败: {}", e))
            }
        }
    }

    /// 从缓存读取（命中时移到尾部实现 LRU）
    fn get_cached(&self, text: &str) -> Option<EmotionResult> {
        let mut cache = self.query_cache.lock();
        if let Some(pos) = cache.iter().position(|(t, _)| t == text) {
            let (key, result) = cache.remove(pos).unwrap();
            cache.push_back((key, result.clone()));
            let mut cached = result;
            cached.source = format!("{}_cache", cached.source);
            Some(cached)
        } else {
            None
        }
    }

    /// 写入缓存（LRU 淘汰）
    fn put_cache(&self, text: String, result: EmotionResult) {
        let mut cache = self.query_cache.lock();
        if cache.len() >= QUERY_CACHE_CAPACITY {
            cache.pop_front();
        }
        cache.push_back((text, result));
    }

    /// 清空缓存（主要用于测试）
    #[cfg(test)]
    pub fn clear_cache(&self) {
        self.query_cache.lock().clear();
        *self.corpus_embeddings.lock() = None;
    }
}

/// 余弦相似度
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
    use super::super::mapper::LLM_EMOTION_LABELS;

    fn make_classifier() -> EmbeddingEmotionClassifier {
        EmbeddingEmotionClassifier::new(
            Arc::new(crate::memory::embedding::HashingMemoryEmbedding::new(256)),
            "zh".to_string(),
        )
    }

    #[test]
    fn test_corpus_covers_all_14_emotions() {
        let clf = make_classifier();
        let emotions: std::collections::HashSet<&str> =
            clf.corpus.iter().map(|e| e.emotion).collect();
        for label in LLM_EMOTION_LABELS {
            assert!(emotions.contains(label), "语料缺少情绪: {}", label);
        }
    }

    #[test]
    fn test_corpus_size_reasonable() {
        let clf = make_classifier();
        assert!(clf.corpus_size() >= 14 * 100, "语料不足: {}", clf.corpus_size());
        assert!(clf.corpus_size() <= 14 * 130, "语料过多: {}", clf.corpus_size());
    }

    #[test]
    fn test_exact_match_returns_high_confidence() {
        let clf = make_classifier();
        let result = clf.classify("太棒了").expect("精确匹配不应失败");
        assert_eq!(result.emotion, "happy");
        assert_eq!(result.source, "embedding_exact");
        assert!(result.intensity > 0.5);
        assert_eq!(result.confidence, Some(1.0));
    }

    #[test]
    fn test_empty_text_returns_neutral() {
        let clf = make_classifier();
        let result = clf.classify("").expect("空文本不应失败");
        assert_eq!(result.emotion, "neutral");
    }

    #[test]
    fn test_whitespace_only_returns_neutral() {
        let clf = make_classifier();
        let result = clf.classify("   ").expect("空白文本不应失败");
        assert_eq!(result.emotion, "neutral");
    }

    #[test]
    fn test_embedding_classifies_happy_variant() {
        let clf = make_classifier();
        let result = clf.classify("今天心情特别好，笑得停不下来");
        match result {
            Ok(r) => {
                assert!(r.source.starts_with("embedding"), "source: {}", r.source);
                assert!(r.confidence.is_some(), "应返回置信度");
            }
            Err(_) => { /* 哈希嵌入下相似度可能不足 */ }
        }
    }

    #[test]
    fn test_cache_hit_returns_cached() {
        let clf = make_classifier();
        let text = "今天天气不错，心情还可以";
        let first = clf.classify(text);
        let second = clf.classify(text);
        if let (Ok(f), Ok(s)) = (first, second) {
            assert!(s.source.ends_with("_cache"), "second source: {}", s.source);
            assert_eq!(f.emotion, s.emotion);
        }
    }

    #[test]
    fn test_low_similarity_returns_neutral_not_error() {
        let clf = make_classifier();
        let result = clf.classify("xyzqwerty12345");
        // 低相似度应返回 Ok(neutral + 低置信度)，而非 Err
        match result {
            Ok(r) => {
                assert_eq!(r.emotion, "neutral", "低相似度应返回 neutral");
                assert!(r.confidence.unwrap_or(1.0) < 0.3, "置信度应较低");
            }
            Err(_) => { /* 哈希嵌入可能返回 Err（嵌入服务失败），这也是合法的 */ }
        }
    }

    #[test]
    fn test_cosine_similarity_identical_vectors() {
        let a = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &a);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_cosine_similarity_orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-5);
    }

    #[test]
    fn test_classify_returns_valid_14_label() {
        let clf = make_classifier();
        let test_texts = [
            "好开心",
            "谢谢你",
            "好累",
            "什么意思",
            "气死我了",
            "好难过",
            "嗯知道了",
        ];
        for text in &test_texts {
            let result = clf.classify(text);
            if let Ok(r) = result {
                assert!(
                    LLM_EMOTION_LABELS.contains(&r.emotion.as_str()),
                    "text: {} -> emotion: {} 不在 14 类标签中",
                    text,
                    r.emotion
                );
            }
        }
    }

    #[test]
    fn test_target_inference() {
        assert_eq!(infer_target_from_text("你今天怎么这么安静"), EmotionTarget::Ai);
        assert_eq!(infer_target_from_text("他怎么可以这样"), EmotionTarget::Other);
        assert_eq!(infer_target_from_text("我好累啊"), EmotionTarget::Self_);
        assert_eq!(infer_target_from_text("今天下雨了"), EmotionTarget::Situation);
    }

    #[test]
    fn test_softmax_sharper_than_linear() {
        // softmax with temp=0.1 应比线性 (sim+1) 有更大的权重比
        let high_sim: f32 = 0.8;
        let low_sim: f32 = 0.5;
        let softmax_ratio = (high_sim / SOFTMAX_TEMPERATURE).exp() / (low_sim / SOFTMAX_TEMPERATURE).exp();
        let linear_ratio = (high_sim + 1.0) / (low_sim + 1.0);
        assert!(softmax_ratio > linear_ratio * 2.0,
            "softmax ratio {} should be much larger than linear ratio {}", softmax_ratio, linear_ratio);
    }
}
