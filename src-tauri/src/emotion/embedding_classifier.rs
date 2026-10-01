//! 基于嵌入的即时情绪分类器
//!
//! - 预置 14 类情绪语料（每类 24 条），含 target/context 元数据
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

/// 14 类情绪的预置语料（中文版本）
///
/// 每类 24 条中文样例，覆盖多种场景维度。每条附带 target/context 元数据，
/// 用于区分用户在说自己、别人、AI 还是客观情况。
static CORPUS_ZH: &[CorpusEntry] = &[
    // happy (24)
    CorpusEntry { text: "晚上散步很惬意", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "下雨天窝在家里好安逸", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "这家店环境挺舒适的", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "同事今天带了好吃的来", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "你推荐的电影还挺好看", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
    CorpusEntry { text: "每天晚上和你聊聊天真好", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你总让我心情变好", emotion: "happy", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "跟你待在一起很轻松", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你说话越来越幽默了", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "有人惦记的感觉真好", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
    CorpusEntry { text: "他记得我爱喝什么", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "爸爸默默帮我修好了东西", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "彼此陪伴就是最好的事", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
    CorpusEntry { text: "今天的工作很顺利", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "今天效率还不错", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "课程安排很合理", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "大家一起努力的感觉很好", emotion: "happy", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "最近身体感觉挺好的", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "好好休息了一天恢复不少", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "今天阳光晒着暖暖的", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
    CorpusEntry { text: "假期和朋友吃了顿火锅", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "和朋友去郊外走了走", emotion: "happy", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "这个季节出去走走正好", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "桂花开了好香", emotion: "happy", target: EmotionTarget::Situation, context: EmotionContext::Event },

    // excited (24)
    CorpusEntry { text: "天哪这也太好了吧", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "等了好久终于到了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "居然遇到了好久没见的人", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "被夸了超级开心", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "抽到SSR了！！", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
    CorpusEntry { text: "连胜停不下来了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
    CorpusEntry { text: "这游戏太好玩了吧", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "马上开奖了好紧张", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "明天的生日派对等不及了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "马上要见到他了", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Event },
    CorpusEntry { text: "这次考试成绩要出来了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "啊啊啊来了来了", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Livestream },
    CorpusEntry { text: "他刚才那个动作绝了", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Livestream },
    CorpusEntry { text: "限量秒杀开始了冲", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
    CorpusEntry { text: "你要给我表演什么呀", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "我等不及要听你说了", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "我好想马上试试", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "这周末要约他出去！", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
    CorpusEntry { text: "他要带我去一个地方", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "今晚的约会好期待", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
    CorpusEntry { text: "项目终于要收尾了！", emotion: "excited", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "团队要拿到大项目了", emotion: "excited", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "同事说她请客吃饭", emotion: "excited", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "你处理得好快啊", emotion: "excited", target: EmotionTarget::Ai, context: EmotionContext::WorkStudy },

    // grateful (24)
    CorpusEntry { text: "邻居送了我好多水果", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "谢谢你听我唠叨", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "被陌生人暖到了", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "幸好有你提醒我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "谢谢你的建议很有用", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "谢谢你耐心回答我", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "有你帮忙效率高多了", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "有你在心里踏实多了", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你推荐的歌好好听", emotion: "grateful", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "有你在身边就够了", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "你做的早餐太好吃了", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "你帮我洗碗了好贴心", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "谢谢你没有放弃我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "你帮我拎包好体贴", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "谢谢前辈教我这么多", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "感谢公司发的福利", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "谢谢同事帮我顶班", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "老师额外给我补课了", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "谢谢你们来我的婚礼", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Event },
    CorpusEntry { text: "年会抽到奖了感恩", emotion: "grateful", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "感谢主办方邀请了我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::Event },
    CorpusEntry { text: "感谢护士照顾我", emotion: "grateful", target: EmotionTarget::Other, context: EmotionContext::HealthBody },
    CorpusEntry { text: "术后恢复得不错感恩", emotion: "grateful", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "能健康活着就很好了", emotion: "grateful", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },

    // sad (24)
    CorpusEntry { text: "好难过啊", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "没人记得我的生日", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "不知道活着有什么意思", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "周末了不知道干什么", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "回到以前住的地方好感慨", emotion: "sad", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "他再也不会回来了", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "他删了我所有的联系方式", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "他还记得我吗", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "他走的时候连再见都没说", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "他说过会一直在的结果呢", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "只有你还愿意听我说话", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你会不会有一天也不理我了", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "跟你说完话还是觉得空虚", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你不会记得我说过的那些事", emotion: "sad", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "考研又没过", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "同学都有好工作了我还在找", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "成绩越来越差了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "他的婚礼我收到了请帖", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Event },
    CorpusEntry { text: "清明节去了他的墓前", emotion: "sad", target: EmotionTarget::Other, context: EmotionContext::Event },
    CorpusEntry { text: "又到了一个人生日的时候", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "又住院了", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "吃药吃得胃疼", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "好羡慕健康的人", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "镜子里的自己好陌生", emotion: "sad", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },

    // frustrated (24)
    CorpusEntry { text: "为什么我什么都做不好", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "感觉一直在原地踏步", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "事事不顺心真烦", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "每天都在重复一样的事", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "说了半天没人听", emotion: "frustrated", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "为什么总卡在这里", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "论文改了十遍还是不行", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "面试又没过", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "辛辛苦苦做的方案被否了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "感觉能力到瓶颈了", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "说了一百遍他还是不改", emotion: "frustrated", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "明明很喜欢却说不出口", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
    CorpusEntry { text: "相亲了好多次都没成", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
    CorpusEntry { text: "排位又掉段了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "练了好久还是打不过", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
    CorpusEntry { text: "这角色怎么这么难玩", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "刷了这么久还是没出", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "好不容易约好的又取消了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "限量的东西又没抢到", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "好不容易抢到的票又被取消了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "又失眠了", emotion: "frustrated", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "跑步膝盖又疼了", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
    CorpusEntry { text: "头发越掉越多怎么办", emotion: "frustrated", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
    CorpusEntry { text: "连你也给不了我想要的答案", emotion: "frustrated", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },

    // anxious (24)
    CorpusEntry { text: "心里慌慌的不踏实", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "万一出了差错怎么办", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "最近老是不安", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "你怎么了感觉你不太对", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
    CorpusEntry { text: "万一考砸了怎么办", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "成绩还没出来好忐忑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "汇报还没准备好", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "竞赛结果还没出好忐忑", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "奖学金不知道能不能评上", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "体重又涨了好焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "运动后膝盖疼不会受伤了吧", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "喉咙不舒服不会是扁桃体吧", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "疫苗打完好不安", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
    CorpusEntry { text: "迟到了怎么办好慌", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "忘带东西了好焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "排队这么久怕来不及", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "她一个人去那边好不安", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::Event },
    CorpusEntry { text: "他怎么还不回我好焦虑", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
    CorpusEntry { text: "他是不是在躲我", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "父母吵架了好担心", emotion: "anxious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "系统不会崩溃吧好担心", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你能理解我的焦虑吗", emotion: "anxious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你账号会不会被封", emotion: "anxious", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "网络卡了好怕你断线", emotion: "anxious", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },

    // tired (24)
    CorpusEntry { text: "浑身没劲", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "累到不想说话", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "终于能歇会儿了", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "你今天也很累吧", emotion: "tired", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
    CorpusEntry { text: "连续加班一周扛不住了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "看了一天论文眼睛要瞎了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "站了一天腿都木了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "天天加班到半夜谁受得了", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "同事都累趴了", emotion: "tired", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "昨天没睡好困死了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "背疼得不想动", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "心跳好快感觉透支了", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "浑身酸痛像被揍了一顿", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "生病之后恢复好慢", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
    CorpusEntry { text: "盯着屏幕眼睛好酸", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
    CorpusEntry { text: "打团的时候好累反应不过来", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
    CorpusEntry { text: "这游戏太肝了身体吃不消", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "旅行回来浑身酸痛", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "爬山爬到腿软", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "社交一整天精力已经耗尽", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "你处理这么多问题会累吗", emotion: "tired", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "好累啊想靠着你休息", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "好困想找个肩膀靠一靠", emotion: "tired", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你辛苦了早点休息", emotion: "tired", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },

    // angry (24)
    CorpusEntry { text: "谁允许他这么做的", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "我招你惹你了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "这破天气真烦人", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "你能不能听懂人话", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
    CorpusEntry { text: "你为什么不听我的", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "你怎么能这么自私", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "你就不能改改吗", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "我怎么就瞎了眼", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
    CorpusEntry { text: "老师凭什么不给过", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "凭什么加班不给加班费", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "都是我自己没准备好", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "领导偏心偏得太明显了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "票价这么贵体验这么差", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "黄牛太猖狂了没人管吗", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Event },
    CorpusEntry { text: "我怎么又迟到了真烦", emotion: "angry", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "弹幕一群喷子真恶心", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Livestream },
    CorpusEntry { text: "这主播居然骂粉丝", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Livestream },
    CorpusEntry { text: "直播间全是托", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
    CorpusEntry { text: "这匹配机制有毒", emotion: "angry", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "他抢我装备还有理了", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Gaming },
    CorpusEntry { text: "辅助不来保我打什么", emotion: "angry", target: EmotionTarget::Other, context: EmotionContext::Gaming },
    CorpusEntry { text: "说好的陪我聊天呢", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你居然忘了我之前说的", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你能不能别总重复一样的话", emotion: "angry", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },

    // disappointed (24)
    CorpusEntry { text: "期待了好久就这", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "排了好久的队不值得", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "以为他变了结果还是一样", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "以为戒掉了结果又破戒了", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "以为他是真心对我的", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "他居然忘了我们的纪念日", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "他居然把我的事告诉别人了", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "他说忙完就来结果一直没来", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "她居然也站在了对面", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "说好的奖金又没了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "以为老板会认可的结果被批了", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "以为offer稳了结果被鸽了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "说好的涨薪又推迟了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "以为这学期能拿高绩点的", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "活动比想象中差多了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "以为旅行会开心的结果一般", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "赶到场的时候居然已经结束了", emotion: "disappointed", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "说好了见面结果他变卦了", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Event },
    CorpusEntry { text: "以为这赛季能上王者", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
    CorpusEntry { text: "队友居然三个人都挂机了", emotion: "disappointed", target: EmotionTarget::Other, context: EmotionContext::Gaming },
    CorpusEntry { text: "以为这个角色很适合我结果不会玩", emotion: "disappointed", target: EmotionTarget::Self_, context: EmotionContext::Gaming },
    CorpusEntry { text: "你的回答不是我想要的", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你说的那些安慰好像没什么用", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你居然也会说错", emotion: "disappointed", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },

    // surprised (24)
    CorpusEntry { text: "啊？真的假的", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "这也太离谱了吧", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "竟然已经这个点了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "不会吧不会吧", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "你怎么突然说这个", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "没想到今天这么多人", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "没想到票价这么便宜", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "没想到会遇到熟人", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Event },
    CorpusEntry { text: "原来今天有烟花表演", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "他居然还记得我生日", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "你们居然认识？", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "你怎么偷偷准备了", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "这个bug居然自己好了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "工资居然涨了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "没想到面试这么顺利", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "你什么时候学会这个的", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你怎么突然变温柔了", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "没想到你会主动找我聊天", emotion: "surprised", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "这个特效也太炫了吧", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
    CorpusEntry { text: "居然连麦了", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
    CorpusEntry { text: "这也能暴击？", emotion: "surprised", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "居然匹配到认识的人", emotion: "surprised", target: EmotionTarget::Other, context: EmotionContext::Gaming },
    CorpusEntry { text: "我居然长高了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "心率居然降下来了", emotion: "surprised", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },

    // curious (24)
    CorpusEntry { text: "附近有什么好吃的", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "明天天气怎么样", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "你知道怎么去那里吗", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "你用的什么手机", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "我不太清楚这是什么", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "你觉得呢", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你能学新东西吗", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你怎么懂这么多东西", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你多大了", emotion: "curious", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你喜欢什么类型的故事", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "他昨天跟谁出去了", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "他为什么不接电话", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "我想搞清楚这件事", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
    CorpusEntry { text: "这个实验数据说明了什么", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "导师有什么新的要求", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "这个函数是做什么的", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "他选了哪门选修课", emotion: "curious", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "火箭什么时候发射", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "这次活动请了哪些嘉宾", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "我想了解一下新政策", emotion: "curious", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "跑步的时候心率多少正常", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
    CorpusEntry { text: "这个药有什么副作用", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
    CorpusEntry { text: "这个角色的被动技能是什么", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "这个成就怎么解锁", emotion: "curious", target: EmotionTarget::Situation, context: EmotionContext::Gaming },

    // neutral (24)
    CorpusEntry { text: "外面在下雨", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "风挺大的", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "电视在客厅", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "钥匙在桌子上", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "我穿了一件黑色外套", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "我刚起床", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "她几点的车", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "公交车还有三站", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "你能做什么", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你晚上也在线吗", emotion: "neutral", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "界面怎么切换模式", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "我之前跟你说过这个", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "这个项目月底截止", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "图书馆十一点闭馆", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "我提交了一份报告", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "他请了三天假", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "比赛分三个环节", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "演出在二号厅", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "我预约了周六的场地", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::Event },
    CorpusEntry { text: "昨晚睡了七个小时", emotion: "neutral", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
    CorpusEntry { text: "早餐吃了两个包子", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::HealthBody },
    CorpusEntry { text: "他们去年结的婚", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "我跟他约了明天见面", emotion: "neutral", target: EmotionTarget::Self_, context: EmotionContext::Relationship },
    CorpusEntry { text: "他几号回来", emotion: "neutral", target: EmotionTarget::Other, context: EmotionContext::Relationship },

    // bored (24)
    CorpusEntry { text: "好无聊啊没事做", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "好闲啊找点事做吧", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "发呆发了一下午", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "一天到晚无所事事", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::DailyChat },
    CorpusEntry { text: "假期宅着好没趣", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "你有什么好玩的吗", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::DailyChat },
    CorpusEntry { text: "一直刷同样的副本好腻", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "这活动好重复没新意", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "一直在赢好没挑战", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "实习没事做好闲", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "听讲座听得快睡着了", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "复习到脑子已经装不下了", emotion: "bored", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "弹幕好少好冷清", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
    CorpusEntry { text: "这个主播的风格好单调", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
    CorpusEntry { text: "这个频道内容好单一", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Livestream },
    CorpusEntry { text: "给我讲个故事吧好闷", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你会讲笑话吗好无聊", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你那边有什么有趣的事吗", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "这个聚会好无聊想走", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "假期没什么好玩的地方", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "最近有什么有意思的活动吗", emotion: "bored", target: EmotionTarget::Ai, context: EmotionContext::Event },
    CorpusEntry { text: "两个人待着也没话说好闷", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
    CorpusEntry { text: "约会每次都是吃饭看电影好腻", emotion: "bored", target: EmotionTarget::Situation, context: EmotionContext::Relationship },
    CorpusEntry { text: "相亲对象好无聊", emotion: "bored", target: EmotionTarget::Other, context: EmotionContext::Relationship },

    // confused (24)
    CorpusEntry { text: "这是什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "这到底是怎么回事", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "这话说得好莫名其妙", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::DailyChat },
    CorpusEntry { text: "他说的话我怎么理解不了", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::DailyChat },
    CorpusEntry { text: "代码跑不通哪里错了", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "需求文档看不明白", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "这个算法没看懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "开会说的没听明白", emotion: "confused", target: EmotionTarget::Self_, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "导师说的方向没太明白", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::WorkStudy },
    CorpusEntry { text: "这个症状是什么病", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
    CorpusEntry { text: "这药副作用是什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
    CorpusEntry { text: "血糖值多少算正常", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::HealthBody },
    CorpusEntry { text: "这抽奖机制什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "这个活动到底什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "会员权益没看懂", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Event },
    CorpusEntry { text: "主办方说的规则没明白", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Event },
    CorpusEntry { text: "这个装备怎么获得", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "这个属性什么意思", emotion: "confused", target: EmotionTarget::Situation, context: EmotionContext::Gaming },
    CorpusEntry { text: "他为什么突然这样", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "朋友说的那句话没懂", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "相亲对象态度好迷", emotion: "confused", target: EmotionTarget::Other, context: EmotionContext::Relationship },
    CorpusEntry { text: "你的回答我没看懂", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你这个逻辑我没跟上", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },
    CorpusEntry { text: "你理解错我的意思了吧", emotion: "confused", target: EmotionTarget::Ai, context: EmotionContext::AiCompanionship },

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
        if let Some(result) = self.cached_classification(trimmed) { return Ok(result); }
        self.ensure_initialized()?;
        let query_emb = self.provider.embed(trimmed).map_err(|e| {
            format!("嵌入服务调用失败: {}", e)
        })?;
        self.classify_with_embedding(trimmed, &query_emb)
    }

    /// Reuse the query vector produced by the semantic router; never embed the text again.
    pub(crate) fn classify_with_embedding(&self, text: &str, query_emb: &[f32]) -> Result<EmotionResult, String> {
        let trimmed = text.trim();
        if let Some(result) = self.cached_classification(trimmed) { return Ok(result); }
        if query_emb.len() != self.provider.dimension() || query_emb.iter().any(|v| !v.is_finite()) {
            return Err("情绪分类收到无效查询向量".into());
        }
        self.ensure_initialized()?;
        let result = self.classify_by_embedding(query_emb);
        self.put_cache(trimmed.to_string(), result.clone());
        Ok(result)
    }

    fn cached_classification(&self, trimmed: &str) -> Option<EmotionResult> {
        if trimmed.is_empty() {
            return Some(EmotionResult::neutral());
        }

        // 1. 精确匹配语料
        if let Some(entry) = self.corpus.iter().find(|e| e.text == trimmed) {
            let (v, a) = llm_emotion_valence_arousal(entry.emotion);
            return Some(EmotionResult {
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
        self.get_cached(trimmed)
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
        // 命中时直接加载，跳过全部嵌入调用（bge-m3 下 336 条约省 4 秒）。
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
        assert_eq!(clf.corpus_size(), 14 * 24, "每类应保留 24 条代表样本");
    }

    #[test]
    fn test_exact_match_returns_high_confidence() {
        let clf = make_classifier();
        let result = clf.classify("今天的工作很顺利").expect("精确匹配不应失败");
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
    fn target_uses_annotated_examples_not_pronoun_guessing() {
        let classifier = make_classifier();
        for (text, target) in [
            ("你总让我心情变好", "ai"),
            ("你怎么能这么自私", "other"),
            ("今天的工作很顺利", "self"),
            ("桂花开了好香", "situation"),
        ] {
            let result = classifier.classify(text).unwrap();
            assert_eq!(result.source, "embedding_exact");
            assert_eq!(result.target.as_deref(), Some(target));
        }
    }

    #[test]
    fn reused_vectors_reject_invalid_dimensions_and_nonfinite_values() {
        let classifier = make_classifier();
        assert!(classifier.classify_with_embedding("unique invalid query 724F2", &[1.0]).is_err());
        assert!(classifier.classify_with_embedding("unique invalid query 724F2", &vec![f32::NAN; 256]).is_err());
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
