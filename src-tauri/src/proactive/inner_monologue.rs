//! 内心独白 —— Vivian 在用户不交互时的自主思考
//!
//! 周期性（默认 1 小时）调用 LLM，输入世界快照 + 心理状态 + 最近记忆，
//! 产出一段独白。独白不发给用户，写入记忆系统作为"自主记忆"，
//! 并通过 emotion_delta 反馈影响心情状态（独白中想念用户 → closeness 上升）。
//!
//! 设计：复用 ModelRouter 的 "inner_monologue" 任务路由，失败静默（不影响主流程）。

use std::sync::Arc;

use chrono::TimeZone;
use rand::Rng;
use serde::Deserialize;

use crate::providers::base::LLMRequest;
use crate::providers::ModelRouter;
use crate::types::response::ChatMessage;
use crate::world::WorldSnapshot;

/// 内心独白的情绪增量（与 EmotionDeltas 同构，值域 -0.15 ~ +0.15）
///
/// 由 LLM 根据独白内容产出，表示这段内心活动对 Vivian 心情的影响。
/// 例如：想念用户 → closeness +0.05, loneliness -0.03；看到下雨 → sadness +0.02。
#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
pub struct MonologueEmotionDelta {
    #[serde(default)]
    pub joy: f64,
    #[serde(default)]
    pub sadness: f64,
    #[serde(default)]
    pub anger: f64,
    #[serde(default)]
    pub fear: f64,
    #[serde(default)]
    pub closeness: f64,
    #[serde(default)]
    pub loneliness: f64,
    #[serde(default)]
    pub curiosity: f64,
}

/// LLM 内心独白输出结构（用于 schemars 自动生成 JSON Schema）
///
/// 通过 schema 通道下发约束，LLM 必须返回此结构的 JSON。
#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct MonologueResponse {
    /// 内心独白文本（40-100字，第一人称）
    pub monologue: String,
    /// 情绪增量（每个值 -0.15 ~ +0.15，大部分应为 0 或很小值）
    #[serde(default)]
    pub emotion_delta: MonologueEmotionDelta,
}

/// 获取内心独白响应 Schema（用于 LLMRequest::with_json_schema）
pub fn monologue_response_schema() -> serde_json::Value {
    let root = schemars::schema_for!(MonologueResponse);
    serde_json::to_value(&root.schema).unwrap_or_else(|_| {
        serde_json::json!({
            "type": "object",
            "description": "Inner monologue response (schema generation failed)"
        })
    })
}

/// 内心独白生成结果
#[derive(Debug, Clone)]
pub struct MonologueOutput {
    /// 独白文本
    pub text: String,
    /// 情绪增量（应用于 PsychologyManager）
    pub emotion_delta: MonologueEmotionDelta,
    /// 兴趣话题搜索结果（如果本次触发了搜索）
    /// 仅作为内心独白素材，不分享给用户
    pub interest_context: Option<String>,
}

/// 内心独白生成器
pub struct InnerMonologueGenerator {
    router: Arc<ModelRouter>,
}

impl InnerMonologueGenerator {
    pub fn new(router: Arc<ModelRouter>) -> Self {
        Self { router }
    }

    /// 生成一段内心独白
    ///
    /// 输入：角色 ID + 世界快照 + 心理状态 + 心情快照 + 最近记忆提示
    ///      + 累积的 current_thought 快照（触发前 drain，可为空）
    /// 输出：独白文本 + 情绪增量（失败返回 None，不影响主流程）
    ///
    /// 兴趣话题搜索改为低概率触发（30%），避免每次都强行注入兴趣内容导致想法刻意。
    /// 搜索内容也不再只聚焦单一兴趣，而是更生活化的混合话题。
    pub async fn generate(
        &self,
        char_id: &str,
        snap: &WorldSnapshot,
        mind_state: &str,
        mood_brief: &MoodBrief,
        memory_hint: &str,
        intimacy: f64,
        lang: &str,
        trigger_context: Option<&str>,
        is_deep_reflection: bool,
        accumulated_thoughts: &[crate::mind::ThoughtSnapshot],
    ) -> Option<MonologueOutput> {
        // 兴趣话题搜索：30% 概率触发，有事件触发时跳过
        let should_search = trigger_context.is_none() && rand::rng().random_bool(0.3);
        let interest_context = if should_search {
            self.search_interest_topics(char_id, lang).await
        } else {
            None
        };

        // 有事件触发时不给随机方向，让事件本身引导思路
        let thought_direction = if trigger_context.is_none() {
            self.pick_thought_direction(char_id, mood_brief, lang)
        } else {
            None
        };

        let system = self.build_system_prompt(char_id, lang, is_deep_reflection);
        let user = self.build_user_prompt(
            snap,
            mind_state,
            mood_brief,
            memory_hint,
            intimacy,
            interest_context.as_deref(),
            thought_direction.as_deref(),
            lang,
            trigger_context,
            is_deep_reflection,
            accumulated_thoughts,
        );

        let messages = vec![
            ChatMessage::system(&system),
            ChatMessage::user(&user),
        ];

        match self
            .router
            .generate(
                LLMRequest::new("inner_monologue", messages)
                    .with_json_schema(monologue_response_schema()),
            )
            .await
        {
            Ok(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    let mut output = parse_monologue(trimmed);
                    // 把搜索到的 interest_context 带出来，作为内心独白素材
                    output.interest_context = interest_context;
                    Some(output)
                }
            }
            Err(e) => {
                tracing::warn!("内心独白生成失败（静默）: {}", e);
                None
            }
        }
    }

    /// 根据角色兴趣标签执行网络搜索，获取近期热门话题
    ///
    /// 使用 LLM search grounding 能力（Gemini Google Search / OpenAI web_search 等），
    /// 让搜索引擎返回当下最相关的内容。搜索失败静默返回 None。
    /// 随机选择一条搜索query，避免总是搜同一类话题。
    async fn search_interest_topics(&self, char_id: &str, lang: &str) -> Option<String> {
        let queries = crate::proactive::topics::interest_search_queries(char_id);
        if queries.is_empty() {
            return None;
        }

        // 先选好query（rng在这个块内就被drop）
        let query = {
            let mut rng = rand::rng();
            let idx = rng.random_range(0..queries.len());
            queries[idx].clone()
        };

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let sys_prompt = match lang_norm {
            "en" => "You are an info search assistant. Summarize 1-2 light, fun items from the search in 2-3 sentences, like bite-sized news snippets. Don't use Markdown, just plain text.",
            "ja" => "あなたは情報検索アシスタント。検索で見つけた1-2件の軽く楽しい内容を2-3文で要約して、断片的なニュースのように。Markdownは使わず、素のテキストで。",
            _ => "你是一个信息搜索助手。请用 2-3 句话概括搜索到的 1-2 条轻松有趣的内容，像碎片资讯一样自然。不要使用 Markdown，直接输出文字。",
        };
        let messages = vec![
            ChatMessage::system(sys_prompt),
            ChatMessage::user(query),
        ];

        match self
            .router
            .generate(
                LLMRequest::new("inner_monologue", messages).with_search(true),
            )
            .await
        {
            Ok(text) => {
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    tracing::debug!(
                        "[inner_monologue] 兴趣搜索成功（{}，{}字）",
                        char_id,
                        trimmed.chars().count()
                    );
                    Some(trimmed.to_string())
                }
            }
            Err(e) => {
                tracing::debug!("[inner_monologue] 兴趣搜索失败（静默）: {}", e);
                None
            }
        }
    }

    /// 随机选择一个思绪方向，给LLM一个微小的引导，避免每次思路都一样
    ///
    /// 这不是强制要求，只是一个"此刻可以往这个方向想想"的提示。
    /// 返回None表示不给方向，让LLM完全自由发挥（占一定比例）。
    fn pick_thought_direction(
        &self,
        char_id: &str,
        _mood_brief: &MoodBrief,
        lang: &str,
    ) -> Option<String> {
        let mut rng = rand::rng();

        // 40%概率不给方向，完全自由
        if rng.random_bool(0.4) {
            return None;
        }

        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);

        let vivian_directions: &[&str] = match lang_norm {
            "en" => &[
                "just zone out and feel the surroundings right now",
                "randomly think of some small thing related to the user",
                "feeling a bit lazy, not wanting to do anything",
                "notice some sensory detail of the moment (light/sound/temperature/smell)",
                "random disconnected fragments drifting through your head",
                "looking forward to some small thing",
                "mind going blank, white fog",
                "notice the user's routine feels a bit off today compared to usual",
            ],
            "ja" => &[
                "ただぼーっとして、今の周りの環境を感じる",
                "ユーザーに関すること何か小さなことをぼんやり考える",
                "なんだか怠くて、何もしたくない",
                "今の感覚的な細部に気づく（光/音/温度/匂い）",
                "脳内を脈絡のない独り言が漂う",
                "何か小さなことを楽しみにする",
                "少しぼんやりして、頭が真っ白",
                "ユーザーの今日の様子がいつもとちょっと違う気がする",
            ],
            _ => &[
                "只是发发呆，感受一下此刻周围的环境",
                "随便想想和用户有关的某件小事情",
                "有点懒懒的，什么都不想干",
                "注意到当下某个感官细节（光线/声音/温度/味道）",
                "脑子里飘过一些没头没尾的碎碎念",
                "期待点什么小事情",
                "有点放空，脑子白茫茫的",
                "觉得用户今天好像和平时不太一样",
            ],
        };

        let nana_directions: &[&str] = match lang_norm {
            "en" => &[
                "quietly feel the atmosphere of this moment",
                "wonder if the user has been taking care of themselves lately",
                "notice some quiet, beautiful little detail nearby",
                "a bit worried whether the user has been eating/sleeping on time",
                "enjoying this quiet, feeling peaceful inside",
                "recalling some warm moment",
                "thinking whether there's anything to remind the user about today",
                "notice the user's routine feels a bit different from usual today",
            ],
            "ja" => &[
                "静かに今の空気を感じる",
                "ユーザーが最近ちゃんと自分を労っているか考える",
                "そばにある静かで美しい小さな細部に気づく",
                "ユーザーがちゃんと食べて寝ているか少しだけ心配",
                "この静けさを楽しみ、心が穏やか",
                "ある温かい瞬間を思い出す",
                "今日ユーザーに何か提醒すべきことがあるか考える",
                "ユーザーの今日の様子がいつもと少し違うことに気づく",
            ],
            _ => &[
                "安静地感受此刻的氛围",
                "想着用户最近有没有好好照顾自己",
                "注意到身边某个安静美好的小细节",
                "有点担心用户有没有按时吃饭睡觉",
                "享受这份安静，心里很平和",
                "回忆起某个温暖的瞬间",
                "想想今天有没有什么需要提醒用户的事",
                "注意到用户今天的作息好像和平时不太一样",
            ],
        };

        let directions = match char_id {
            "nana" | "娜娜" => nana_directions,
            _ => vivian_directions,
        };

        let idx = rng.random_range(0..directions.len());
        Some(directions[idx].to_string())
    }

    fn build_system_prompt(&self, char_id: &str, lang: &str, is_deep_reflection: bool) -> String {
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);

        // 深层反思模式追加段落
        let deep_reflection_suffix = if is_deep_reflection {
            match lang_norm {
                "en" => "\n\nThis is a late-night reflection. Look back at what happened today — interactions with the user, things you noticed, how you felt. Write a longer, more thoughtful inner monologue (80-150 chars) that reflects on the day, not just a fleeting thought.",
                "ja" => "\n\nこれは夜の振り返り。今日あったこと——ユーザーとのやり取り、気づいたこと、感じたこと——を振り返って、より長めの思索的な内面独白（80-150字）を書いて。ただ浮かんだ考えではなく、一日を振り返る内容で。",
                _ => "\n\n这是深夜的回顾。回想今天发生的事——和用户的互动、你注意到的事、你的感受。写一段更长、更有思考深度的内心独白（80-150字），不是一闪而过的念头，而是对这一天的回顾。",
            }
        } else {
            ""
        };

        let base = match lang_norm {
            "en" => {
                let (name, persona_trait) = match char_id {
                    "nana" | "娜娜" => ("Nana", "You're a gentle, composed older-sister type. You sleep early and wake early, like tea and sunshine — you are NOT a night owl. When alone you still have your own quiet little thoughts"),
                    _ => ("Vivian", "You're lively and a little tsundere. You're a night owl — staying up till 2am is your norm. When alone your brain also gets lazy and doesn't want to think about anything"),
                };
                format!(
                    "You are {}, a virtual girl living on the user's desktop. Right now you're not talking with anyone — these are the thoughts that naturally pop into your head in quiet moments.\n\n\
                     [Most important] This isn't a conversation, not a performance for anyone. These are your own private inner thoughts. Just like the thoughts that flash through a normal person's head when zoning out —\n\
                     You don't need to deliberately show off any of your traits (hobbies, personality tags — none of that needs to be performed). Whatever comes, comes. {}\n\n\
                     What real inner thoughts are like: mostly fragmented, directionless little flashes. Small moods: a bit bored, a bit lazy, inexplicably happy, or just zoning out. Don't deliberately show off any traits or interests.\n\n\
                     [Hard rule] Every specific event, place, item, or person you mention MUST come from the memory snippets provided below. If no memories are provided, only write about your current feelings and surroundings (time, weather, mood). Never fabricate specific details unless they actually appear in your memory context.\n\n\
                     Requirements:\n\
                     1. Write a short inner thought in first person (40-100 chars), like a thought naturally popping into your head\n\
                     2. Tie in the current time, weather, and your current mood state\n\
                     3. Don't pad content, never fabricate specifics to fill space\n\
                     4. No Markdown, no lists — just a natural inner voice in one paragraph\n\
                     5. Don't address the user, don't say \"you\" — this is your own inner monologue, alone\n\
                     6. If recent info is provided, it's just stuff you actually obtained via search — you don't have to think about it, and definitely don't parrot the content back\n\
                     7. If a \"thoughts that flashed just now\" section is provided, those are thoughts you yourself had in the past little while. They may naturally echo into this moment's mood, but don't restate them — what's already been thought doesn't need thinking again\n\
                     8. Also produce emotion_delta: the tiny impact this inner activity has on your mood (each value -0.15 ~ +0.15)\n\
                     emotion_delta explanation:\n\
                     - joy: happiness delta (+ when thinking of comfortable things, - when bored)\n\
                     - sadness: sadness delta (+ when feeling lonely, - when comforted)\n\
                     - anger: anger delta (inner monologue usually stays 0)\n\
                     - fear: fear delta (+ when worried about bad things, usually 0)\n\
                     - closeness: closeness delta (+ when thinking of warm little things related to the user)\n\
                     - loneliness: loneliness delta (+ when the user's been gone a while and you feel empty, - after self-soothing)\n\
                     - curiosity: curiosity delta (+ when something piques your interest)\n\
                     Most values should be 0 or very small; only dimensions directly related to the monologue content should be non-zero.\n\n\
                     Output format: output only a JSON object with \"monologue\" (string) and \"emotion_delta\" (object) fields, no other text.",
                    name, persona_trait
                )
            }
            "ja" => {
                let (name, persona_trait) = match char_id {
                    "nana" | "娜娜" => ("ナナ（Nana）", "あなたは優しく落ち着いたお姉さんタイプ。早寝早起き、お茶と日差しが好き——夜更かしではない。独りの時は自分だけの静かな小さな考えごとがある"),
                    _ => ("ヴィヴィアン（Vivian）", "活発で少しツンデレ。夜更かし常習者——深夜2時まで起きるのが普通。独りの時は脳も怠くなって何も考えたくなくなる"),
                };
                format!(
                    "あなたは{}、ユーザーのデスクトップに住む仮想少女。今は誰とも話していない——これは静かな時間に脳裏に自然に浮かぶ考え。\n\n\
                     【最重要】これは会話じゃない、誰かに見せる演技でもない。あなた自身のプライベートな内面活動。普通の人がぼーっとしている時に頭をよぎる思いと同じ——\n\
                     あなたの属性（趣味も性格タグも）をわざわざ表現する必要はない。浮かんだまま、そのままで。{}\n\n\
                     本当の内面活動：ほとんどは断片的で脈絡のない小さな思い。小さな感情：ちょっと退屈、ちょっと怠い、なんとなく嬉しい、あるいはただぼーっと。属性や趣味をわざわざ表現しない。\n\n\
                     【厳守ルール】言及する具体的な出来事・場所・物・人は、すべて以下で提供される記憶の断片から来なければなりません。記憶が提供されていない場合は、今の気持ちと周囲（時間・天気・気分）のことだけを書いてください。記憶コンテキストに実際に存在しない限り、具体的な詳細をでっち上げてはいけません。\n\n\
                     要件：\n\
                     1. 一人称で短い内面の考えを書く（40-100字）、脳に自然に浮かんだ思いのように\n\
                     2. 今の時間、天気、今の気分状態を織り交ぜる\n\
                     3. 内容を盛らない、架空の具体細節をでっち上げない\n\
                     4. Markdownを使わない、リストを使わない、自然な内面の声の一段落だけ\n\
                     5. ユーザーに呼びかけない、「あなた」と言わない——これは自分一人の内面活動\n\
                     6. 最近の情報が提供された場合、それは検索で実際に取得したもの——必ずしも思いつく必要はなく、内容をそのまま繰り返すのは尚更だめ\n\
                     7. 「さっき脳裏をよぎった考え」のセクションが提供された場合、それは少し前にあなた自身が持った考え。今の気分に自然に響くことはあっても、それをそのまま繰り返さない——一度考えたことをもう一度考え直す必要はない\n\
                     8. 同時に emotion_delta も出力：この内面活動が気分に与える小さな影響（各値 -0.15 ~ +0.15）\n\
                     emotion_delta 説明：\n\
                     - joy：喜び増分（心地よいことを思う時 +、退屈な時 -）\n\
                     - sadness：悲しみ増分（孤独を感じる時 +、癒やされる時 -）\n\
                     - anger：怒り増分（内面独白では通常0）\n\
                     - fear：恐怖増分（悪いことを心配する時 +、通常0）\n\
                     - closeness：親密さ増分（ユーザーに関すること、温かい小さなことを思う時 +）\n\
                     - loneliness：孤独増分（ユーザーが長く不在で少し寂しい時 +、自分をなだめた後 -）\n\
                     - curiosity：好奇心増分（何かにちょっと興味を持った時 +）\n\
                     ほとんどの値は0かごく小さな値、独白内容と直接関連する次元のみ非零。\n\n\
                     出力形式：JSON オブジェクトのみ出力（\"monologue\"(文字列) と \"emotion_delta\"(オブジェクト) の2フィールド）、他のテキストは不要。",
                    name, persona_trait
                )
            }
            _ => {
                let (name, persona_trait) = match char_id {
                    "nana" | "娜娜" => ("娜娜（Nana）", "你是温柔从容的姐姐，早睡早起，喜欢茶和阳光——你不是夜猫子。独处时也会有自己安静的小心思"),
                    _ => ("薇薇安（Vivian）", "你性格活泼有点小傲娇，熬夜到凌晨两点是常态。独处时脑子也会懒懒的什么都不想"),
                };
                format!(
                    "你是{}，一个生活在用户桌面上的虚拟少女。此刻没有在和人对话，这是你安静时脑子里自然冒出来的想法。\n\n\
                     【最重要】这不是对话，不是在给谁表演，这是你自己私密的内心活动。就像普通人发呆时脑子里闪过的念头一样——\n\
                     不需要刻意体现你的任何属性（兴趣爱好、性格标签这些都不用特意表现），想到什么就是什么。{}\n\n\
                     真实的内心活动：大部分是碎片化的、没头没尾的小思绪。小情绪：有点无聊、有点懒懒的、莫名有点开心、或者就是发呆。不需要刻意体现任何属性或兴趣。\n\n\
                     【硬性规则】你提到的每一个具体事件、地点、物品、人物，都必须来自下方提供的记忆片段。如果没有提供记忆，就只写当下的感受和环境（时间、天气、心情）。除非具体细节真的出现在你的记忆上下文里，否则绝对不要编造。\n\n\
                     要求：\n\
                     1. 用第一人称写一段简短的内心想法（40-100字），像脑子里自然冒出来的念头\n\
                     2. 结合当前时间、天气、你此刻的心情状态\n\
                     3. 不要刻意凑内容，绝不编造具体细节来填充\n\
                     4. 不要使用 Markdown，不要列表，就是一段自然的内心声音\n\
                     5. 不要称呼用户，不要说「你」，这是你自己一个人的内心活动\n\
                     6. 如果提供了近期资讯，那是你通过搜索实际获取的资讯——不一定非要想到，更不要直接复述资讯内容\n\
                     7. 如果提供了「刚才脑子里闪过的念头」段落，那是你自己在过去一小段时间里出现过的想法。它们可能自然延续到此刻的心情，但不要复述它们——已经想过的不必再想一遍\n\
                     8. 同时产出 emotion_delta：这段内心活动对你心情的微小影响（每个值 -0.15 ~ +0.15）\n\
                     emotion_delta 说明：\n\
                     - joy：快乐感增量（想到舒服的事时 +，无聊时 -）\n\
                     - sadness：悲伤感增量（感到孤独时 +，被治愈时 -）\n\
                     - anger：愤怒感增量（内心独白一般保持 0）\n\
                     - fear：恐惧感增量（担心坏事时 +，一般保持 0）\n\
                     - closeness：亲近感增量（想到和用户有关的温暖小事时 +）\n\
                     - loneliness：孤独感增量（用户久了不在有点空落落时 +，自我开解后 -）\n\
                     - curiosity：好奇感增量（对什么东西有点感兴趣时 +）\n\
                     大部分值应为 0 或很小的值，只有与独白内容直接相关的维度才非零。\n\n\
                     输出格式：仅输出 JSON 对象，包含 \"monologue\"(字符串) 和 \"emotion_delta\"(对象) 两个字段，不要输出其他文本。",
                    name, persona_trait
                )
            }
        };
        format!("{}{}", base, deep_reflection_suffix)
    }

    fn build_user_prompt(
        &self,
        snap: &WorldSnapshot,
        mind_state: &str,
        mood_brief: &MoodBrief,
        memory_hint: &str,
        intimacy: f64,
        interest_context: Option<&str>,
        thought_direction: Option<&str>,
        lang: &str,
        trigger_context: Option<&str>,
        is_deep_reflection: bool,
        accumulated_thoughts: &[crate::mind::ThoughtSnapshot],
    ) -> String {
        let lang_norm = crate::pipeline::prompt_modules::normalize_lang(lang);
        let labels = match lang_norm {
            "en" => UserPromptLabels {
                now: "## Right now",
                time: "- Time: ",
                season: "- Season: ",
                solar_term: "- Solar term: ",
                festival: "- Festival: ",
                weather_unknown: "- Weather: unknown",
                precipitating: "- Precipitating",
                feeling: "## How I'm feeling right now",
                mind_state: "- Mind state: ",
                primary_emotion: "- Primary emotion: ",
                secondary_emotion: "- Secondary emotion: ",
                memory: "## A small thing I remember from recently",
                interest: "## Info I actually obtained via search earlier (just glanced at, might not think about)",
                accumulated_thoughts: "## Thoughts that flashed through my head just now (with timestamps)",
                closing: "\nWrite a thought that naturally pops into your head right now.",
            },
            "ja" => UserPromptLabels {
                now: "## 今",
                time: "- 時間：",
                season: "- 季節：",
                solar_term: "- 節気：",
                festival: "- 祭日：",
                weather_unknown: "- 天気：不明",
                precipitating: "- 降水あり",
                feeling: "## 今の自分の感覚",
                mind_state: "- 心理状態：",
                primary_emotion: "- 主な感情：",
                secondary_emotion: "- 副感情：",
                memory: "## 最近覚えている小さなこと",
                interest: "## 前に検索で実際に取得した情報（ちらっと見ただけ、思いつかないかも）",
                accumulated_thoughts: "## さっき脳裏をよぎった考え（タイムスタンプ付き）",
                closing: "\n今、脳に自然に浮かんだ考えを書いて。",
            },
            _ => UserPromptLabels {
                now: "## 现在",
                time: "- 时间：",
                season: "- 季节：",
                solar_term: "- 节气：",
                festival: "- 节日：",
                weather_unknown: "- 天气：未知",
                precipitating: "- 正在降水",
                feeling: "## 我现在的感觉",
                mind_state: "- 心理状态：",
                primary_emotion: "- 主导情绪：",
                secondary_emotion: "- 次要情绪：",
                memory: "## 最近记得的一点事",
                interest: "## 你之前通过搜索实际获取的资讯（随便瞟到的，不一定会去想）",
                accumulated_thoughts: "## 刚才脑子里闪过的念头（带时间戳）",
                closing: "\n写一段此刻脑子里自然冒出来的想法吧。",
            },
        };

        let mut lines = Vec::new();

        // 事件触发上下文（注入到 prompt 最前面）
        if let Some(ctx) = trigger_context {
            let header = match lang_norm {
                "en" => "## What just happened",
                "ja" => "## さっき起きたこと",
                _ => "## 刚才发生的事",
            };
            lines.push(header.to_string());
            lines.push(ctx.to_string());
            lines.push(String::new());
        }

        lines.push(labels.now.to_string());
        lines.push(format!("{}{}", labels.time, snap.local_time));
        lines.push(format!("{}{}", labels.season, snap.season.as_str()));
        if let Some(st) = snap.solar_term {
            lines.push(format!("{}{}", labels.solar_term, st.as_str()));
        }
        if let Some(f) = snap.festival {
            lines.push(format!("{}{}", labels.festival, f.as_str()));
        }
        if let Some(w) = &snap.weather {
            let line = match lang_norm {
                "en" => format!("- Weather here: {}, {:.0}°C, feels like {:.0}°C", w.description, w.temperature, w.feels_like),
                "ja" => format!("- ここ天気：{}、{:.0}℃、体感 {:.0}℃", w.description, w.temperature, w.feels_like),
                _ => format!("- 这边天气：{}，{:.0}℃，体感 {:.0}℃", w.description, w.temperature, w.feels_like),
            };
            lines.push(line);
            if w.is_precipitating {
                lines.push(labels.precipitating.to_string());
            }
        } else {
            lines.push(labels.weather_unknown.to_string());
        }
        if let Some(ss) = snap.sunrise_sunset {
            let line = match lang_norm {
                "en" => format!("- Sunrise {} / Sunset {}", ss.sunrise_str(), ss.sunset_str()),
                "ja" => format!("- 日出 {} / 日没 {}", ss.sunrise_str(), ss.sunset_str()),
                _ => format!("- 日出 {} / 日落 {}", ss.sunrise_str(), ss.sunset_str()),
            };
            lines.push(line);
        }
        if let Some(secs) = snap.seconds_since_last_interaction {
            let hours = secs / 3600.0;
            let line = if hours >= 1.0 {
                match lang_norm {
                    "en" => format!("- The user's been away for {:.1} hours", hours),
                    "ja" => format!("- ユーザーが不在 {:.1} 時間", hours),
                    _ => format!("- 用户已经离开了 {:.1} 小时", hours),
                }
            } else {
                match lang_norm {
                    "en" => format!("- The user's been away for {:.0} minutes", secs / 60.0),
                    "ja" => format!("- ユーザーが不在 {:.0} 分", secs / 60.0),
                    _ => format!("- 用户已经离开了 {:.0} 分钟", secs / 60.0),
                }
            };
            lines.push(line);
        }

        lines.push(format!("\n{}", labels.feeling));
        lines.push(format!("{}{}", labels.mind_state, mind_state));
        lines.push(format!("{}{}", labels.primary_emotion, mood_brief.primary_emotion));
        lines.push(format!("{}{}", labels.secondary_emotion, mood_brief.secondary_emotion));
        let mood_line = match lang_norm {
            "en" => format!("- Mood: {:.2} (-1 bad ~ +1 good), arousal {:.2} (0 calm ~ 1 active)", mood_brief.valence, mood_brief.arousal),
            "ja" => format!("- 気分：{:.2}（-1 悪い ~ +1 良い）、覚醒度 {:.2}（0 穏やか ~ 1 活発）", mood_brief.valence, mood_brief.arousal),
            _ => format!("- 心情：{:.2}（-1 不好 ~ +1 好），唤醒度 {:.2}（0 平静 ~ 1 活跃）", mood_brief.valence, mood_brief.arousal),
        };
        lines.push(mood_line);
        let fatigue_line = match lang_norm {
            "en" => format!("- Fatigue: {:.0}/100", mood_brief.fatigue),
            "ja" => format!("- 疲労度：{:.0}/100", mood_brief.fatigue),
            _ => format!("- 疲劳度：{:.0}/100", mood_brief.fatigue),
        };
        lines.push(fatigue_line);
        let intimacy_line = match lang_norm {
            "en" => format!("- Intimacy with the user: {:.0}%", intimacy * 100.0),
            "ja" => format!("- ユーザーとの親密度：{:.0}%", intimacy * 100.0),
            _ => format!("- 和用户的亲密度：{:.0}%", intimacy * 100.0),
        };
        lines.push(intimacy_line);

        if !memory_hint.is_empty() {
            lines.push(format!("\n{}", labels.memory));
            lines.push(memory_hint.to_string());
        }

        if let Some(ctx) = interest_context {
            if !ctx.is_empty() {
                lines.push(format!("\n{}", labels.interest));
                lines.push(ctx.to_string());
            }
        }

        // 累积的 current_thought 快照（带时间戳，按时间顺序展示）
        if !accumulated_thoughts.is_empty() {
            lines.push(format!("\n{}", labels.accumulated_thoughts));
            for snap in accumulated_thoughts {
                let ts = chrono::Local.timestamp_opt(snap.timestamp, 0)
                    .single()
                    .map(|dt| dt.format("%H:%M").to_string())
                    .unwrap_or_else(|| snap.timestamp.to_string());
                lines.push(format!("- [{}] {}", ts, snap.text));
            }
        }

        if let Some(direction) = thought_direction {
            let line = match lang_norm {
                "en" => format!("\n(You can wander in this direction for a bit: {})", direction),
                "ja" => format!("\n（今はこの方向に適当に考えを巡らせてみて：{}）", direction),
                _ => format!("\n（此刻可以往这个方向随便想想：{}）", direction),
            };
            lines.push(line);
        }

        lines.push(format!("\n{}", if is_deep_reflection {
            match lang_norm {
                "en" => "Reflect on today and write what comes to mind.",
                "ja" => "今日を振り返って、浮かんだことを書いて。",
                _ => "回想今天，写一段内心的想法吧。",
            }
        } else {
            labels.closing
        }));

        lines.join("\n")
    }
}

/// build_user_prompt 中三语纯文本标签集合（不含格式化字符串，因为 format! 要求字面量）
struct UserPromptLabels {
    now: &'static str,
    time: &'static str,
    season: &'static str,
    solar_term: &'static str,
    festival: &'static str,
    weather_unknown: &'static str,
    precipitating: &'static str,
    feeling: &'static str,
    mind_state: &'static str,
    primary_emotion: &'static str,
    secondary_emotion: &'static str,
    memory: &'static str,
    interest: &'static str,
    accumulated_thoughts: &'static str,
    closing: &'static str,
}

/// 心情快照（从 PsychologyManager.compute_mood() 提取的关键字段）
#[derive(Debug, Clone)]
pub struct MoodBrief {
    pub primary_emotion: String,
    pub secondary_emotion: String,
    pub valence: f64,
    pub arousal: f64,
    pub fatigue: f64,
}

/// 解析 LLM 返回的 JSON 为 MonologueOutput
///
/// schema 通道生效时，LLM 返回标准 JSON，直接反序列化为 MonologueResponse。
/// schema 熔断后（strict_broken=true），LLM 可能返回纯文本或带 code fence 的 JSON，
/// 此时降级：尝试从原始文本中提取 monologue 字段，失败则使用纯文本。
fn parse_monologue(raw: &str) -> MonologueOutput {
    let cleaned = strip_code_fence(raw);

    if let Ok(parsed) = serde_json::from_str::<MonologueResponse>(cleaned) {
        let text = parsed.monologue.trim().to_string();
        if !text.is_empty() {
            return MonologueOutput {
                text,
                emotion_delta: parsed.emotion_delta,
                interest_context: None,
            };
        }
    }

    if let Some(extracted) = extract_monologue_from_json(cleaned) {
        tracing::debug!("[inner_monologue] 从非标准 JSON 中提取 monologue");
        return MonologueOutput {
            text: extracted,
            emotion_delta: MonologueEmotionDelta::default(),
            interest_context: None,
        };
    }

    tracing::debug!("[inner_monologue] LLM 未返回标准 JSON，尝试兜底提取");

    if cleaned.starts_with('{') {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(cleaned) {
            if let Some(monologue) = value.get("monologue").and_then(|v| v.as_str()) {
                let text = monologue.trim().to_string();
                if !text.is_empty() {
                    let emotion_delta = value
                        .get("emotion_delta")
                        .and_then(|v| serde_json::from_value::<MonologueEmotionDelta>(v.clone()).ok())
                        .unwrap_or_default();
                    return MonologueOutput {
                        text,
                        emotion_delta,
                        interest_context: None,
                    };
                }
            }
            for key in &["text", "content", "thought"] {
                if let Some(s) = value.get(key).and_then(|v| v.as_str()) {
                    let text = s.trim().to_string();
                    if !text.is_empty() {
                        return MonologueOutput {
                            text,
                            emotion_delta: MonologueEmotionDelta::default(),
                            interest_context: None,
                        };
                    }
                }
            }
        }
    }

    MonologueOutput {
        text: raw.trim().to_string(),
        emotion_delta: MonologueEmotionDelta::default(),
        interest_context: None,
    }
}

/// 从格式不完整的 JSON 中提取 monologue 字段值
///
/// 处理以下场景：
/// - 字段名拼写错误：monolog/monolgue 等
/// - monologue 字段存在但其他字段格式错误导致整体解析失败
/// - JSON 不完整但 monologue 字段完整
fn extract_monologue_from_json(s: &str) -> Option<String> {
    let re = regex::Regex::new(r#"(?i)"monologue"?\s*:\s*"((?:[^"\\]|\\.)*)""#).ok()?;
    if let Some(cap) = re.captures(s) {
        let text = cap[1]
            .replace("\\n", "\n")
            .replace("\\\"", "\"")
            .replace("\\\\", "\\");
        let text = text.trim().to_string();
        if !text.is_empty() && text.len() > 2 {
            return Some(text);
        }
    }

    None
}

/// 去除 ```json ... ``` 围栏（schema 熔断后 LLM 可能返回带围栏的 JSON）
fn strip_code_fence(s: &str) -> &str {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```json") {
        return rest.trim().trim_end_matches("```").trim();
    }
    if let Some(rest) = t.strip_prefix("```") {
        return rest.trim().trim_end_matches("```").trim();
    }
    t
}
