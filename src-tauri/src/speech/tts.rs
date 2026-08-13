//! 语音合成（TTS）- 多后端统一管理器
//!
//! - `TtsEngine` 枚举: None / EdgeTts / Azure / GptSoVits / FishSpeech / MiniMax / Windows
//! - `TtsConfig`: 统一配置(主后端 + fallback 后端 + 各后端独立字段)
//! - `TtsManager`: 持有 `Box<dyn TtsBackend>` + fallback 链 + 重试 + WordBoundary 事件回调
//! - 播放使用 `MciPlayer`(进程内 MCI,替代 PowerShell + taskkill)
//! - 持久化: `%APPDATA%\Vivian\sound\config.json`

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::error::{VivianError, VivianResult};
use crate::utils::path::get_user_data_dir;

use super::tts_audio::{cleanup_temp_file, save_to_temp_file, MciPlayer, MemoryPlayer};
use super::tts_backend::{
    create_backend, word_to_mouth_open, AudioFormat, TtsBackend, TtsSynthesisResult,
};

use std::sync::atomic::Ordering;

/// 单句播放超时兜底。
///
/// rodio 的 `is_playing()` 在底层 sink 异常时可能永远返回 true，
/// 缺少兜底会让 `speak_text` 命令永久阻塞，连锁导致前端 `flushSync` 卡死、
/// 气泡不消失、消息不入记忆图谱。120s 覆盖最长的合成语音（约 60s 朗读）+ 缓冲。
const PLAYBACK_TIMEOUT_SECS: std::time::Duration = std::time::Duration::from_secs(120);

/// 口型同步回调类型:接收 [0.0, 1.0] 的嘴形开合值
///
/// 朗读期间后台线程会根据 WordBoundary 事件或音素映射调用此回调;
/// 朗读结束时调用 `callback(0.0)` 复位嘴形。
pub type MouthCallback = Arc<dyn Fn(f64) + Send + Sync>;

/// TTS 事件回调类型:用于向前端推送合成/播放进度
pub type TtsEventCallback = Arc<dyn Fn(&TtsEvent) + Send + Sync>;

/// TTS 事件(通过回调推送)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TtsEvent {
    /// 开始合成
    Started { text: String, engine: String },
    /// 词边界(用于音素级唇形同步)
    WordBoundary {
        text: String,
        offset_ms: u64,
        duration_ms: u64,
        mouth_open: f32,
    },
    /// 播放完成
    Finished,
    /// 出错
    Error { message: String, engine: String },
    /// fallback 到备用后端
    Fallback {
        from: String,
        to: String,
        reason: String,
    },
}

/// TTS 引擎类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TtsEngine {
    None,
    /// Edge-TTS(免费、高质量、支持 WordBoundary)— 默认后端
    EdgeTts,
    /// Azure 认知服务(需 API Key)
    Azure,
    /// GPT-SoVITS 自托管服务(兼容 v1/v2)
    GptSoVits,
    /// Fish Speech(fishaudio/fish-speech,本地或云端 /v1/tts)
    FishSpeech,
    /// 遗留别名,加载时自动迁移到 FishSpeech
    BertVits2,
    /// MiniMax 语音合成(云端,WebSocket 流式协议,一次性合成)
    MiniMax,
    /// 豆包语音合成(火山引擎,高质量中文情感语音,HTTP 一次性合成)
    Doubao,
    /// Windows 原生 WinRT(离线 fallback)
    Windows,
}

impl Default for TtsEngine {
    fn default() -> Self {
        TtsEngine::None
    }
}

impl TtsEngine {
    /// 将遗留别名映射到实际后端
    /// - BertVits2 → FishSpeech（老 Bert-VITS2 服务已停维，迁移到 Fish Speech）
    pub fn resolve(&self) -> TtsEngine {
        match self {
            TtsEngine::BertVits2 => TtsEngine::FishSpeech,
            other => other.clone(),
        }
    }
}

/// GPT-SoVITS 情感音色映射条目
///
/// 为不同 emotion 配置独立的参考音频，让 Vivian 在不同情绪下使用不同音色。
/// emotion 键名与 LLM 返回的 expression 字段对齐（如 happy/sad/angry/shy/neutral）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmotionVoiceEntry {
    /// 参考音频路径（覆盖 gpt_sovits_ref_audio）
    pub ref_audio_path: String,
    /// 参考音频文本（覆盖 gpt_sovits_prompt_text）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_text: Option<String>,
    /// 参考音频语种（覆盖 gpt_sovits_prompt_lang）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_lang: Option<String>,
}

/// 情感韵律配置 — Emotion Prosody
///
/// 为不同 emotion 配置独立的韵律参数(pitch/speed/pause/energy),
/// 替代单纯切换参考音频,让语音表现力更细腻。
///
/// - pitch: 音高偏移(半音,-12 到 +12 为合理范围;正值升高,负值降低)
/// - speed: 语速倍率(1.0=正常;>1.0 加速,<1.0 减速)
/// - pause: 停顿倍率(1.0=正常;>1.0 句间停顿更长,营造舒缓感)
/// - energy: 能量/强度(0.5-2.0,1.0=正常;Azure 对应 style_degree)
///
/// 命中 emotion 时覆盖 TtsConfig 的对应字段。
/// 不同后端支持程度不同:
/// - EdgeTts/Azure: pitch/speed 通过 SSML prosody 支持
/// - GPT-SoVITS: speed 通过 speed_factor 支持,pitch 暂不支持
/// - MiniMax: speed 支持,pitch 暂不支持
/// - pause/energy: 目前仅定义,后端支持待扩展
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceProfile {
    /// 音高偏移(半音)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f64>,
    /// 语速倍率(1.0=正常)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    /// 停顿倍率(1.0=正常)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause: Option<f64>,
    /// 能量/强度(0.5-2.0,1.0=正常)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub energy: Option<f64>,
}

/// TTS 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    pub enabled: bool,
    /// 语速倍率(1.0=正常)。旧配置中 rate>10 视为遗留 i32 格式,加载时归一化为 1.0
    pub rate: f64,
    pub volume: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_id: Option<String>,
    pub engine: TtsEngine,
    /// fallback 后端(主后端失败时使用,None 表示无 fallback)
    /// 缺失时默认 EdgeTts,保证主后端失败时有兜底
    #[serde(default = "default_fallback_engine", skip_serializing_if = "Option::is_none")]
    pub fallback_engine: Option<TtsEngine>,
    /// 失败重试次数(默认 1)
    #[serde(default = "default_retry_count")]
    pub retry_count: u32,

    // ── Azure 配置 ──
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure_region: Option<String>,
    /// Azure voice style(情感风格,如 cheerful/sad/excited/whispering)
    /// 仅对支持 StyleList 的 voice 生效,参考 /voices/list 返回的 StyleList 字段
    /// 完整列表: advertisement_upbeat, affectionate, angry, assistant, calm, chat,
    ///           cheerful, customerservice, depressed, disgruntled, documentary-narration,
    ///           embarrassed, empathetic, envious, excited, fearful, friendly, gentle,
    ///           hopeful, lyrical, narration-professional, narration-relaxed, newscast,
    ///           newscast-casual, newscast-formal, poetry-reading, sad, serious, shouting,
    ///           sports_commentary, sports_commentary_excited, terrified, unfriendly, whispering
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure_style: Option<String>,
    /// Azure style degree(风格强度,0.5-2.0,默认 1.0)
    /// 1.0 为标准强度,<1.0 弱化风格,>1.0 增强风格
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure_style_degree: Option<f64>,
    /// Azure role(角色扮演,如 YoungAdultFemale/OlderAdultMale/Boy/Girl)
    /// 仅对支持 RolePlayList 的 voice 生效
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure_role: Option<String>,
    /// Azure pitch(音高偏移,单位为半音,-50 到 +50,默认 0)
    /// 正值升高音调,负值降低音调
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure_pitch: Option<f64>,
    /// Azure 输出格式(X-Microsoft-OutputFormat 头)
    /// 支持的值: riff-24khz-16bit-mono-pcm(wav) / audio-24khz-48kbitrate-mono-mp3(mp3,默认) /
    ///           ogg-24khz-16bit-mono-opus(ogg) / webm-24khz-16bit-mono-opus(webm) /
    ///           raw-24khz-16bit-mono-pcm(pcm) 等
    /// 留空默认 audio-24khz-48kbitrate-mono-mp3
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub azure_output_format: Option<String>,

    // ── GPT-SoVITS 配置 ──
    /// GPT-SoVITS 服务地址(如 http://127.0.0.1:9880)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_url: Option<String>,
    /// GPT-SoVITS 安装目录(api_v2.py 所在路径,通常是整合包根目录),用于一键启动服务
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_install_path: Option<String>,
    /// 自定义 tts_infer.yaml 路径(可选)
    /// 若配置则直接用此文件;若未配置但用户填了模型路径/GPU,会自动生成临时 yaml
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_config_path: Option<String>,
    /// GPT(t2s)模型路径(.ckpt) — 写入生成的 tts_infer.yaml 的 t2s_weights_path
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_gpt_model: Option<String>,
    /// SoVITS(vits)模型路径(.pth) — 写入生成的 tts_infer.yaml 的 vits_weights_path
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_sovits_model: Option<String>,
    /// GPU 卡号(0=第一张 GPU,1=第二张...;-1=CPU 推理)
    /// 转换为 tts_infer.yaml 的 device 字段:0→"cuda:0",-1→"cpu"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_gpu: Option<i32>,
    /// 服务端口(对应 api_v2.py 的 -p 参数,默认 9880)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_port: Option<u16>,
    /// Python 可执行文件路径(用于启动 api_v2.py,默认从 PATH 查找 python)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_python_path: Option<String>,
    /// 主参考音频路径(3-10 秒,对应 v2 API 的 ref_audio_path)
    /// 决定合成音色
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_ref_audio: Option<String>,
    /// 主参考音频对应的文本(可选,对应 v2 API 的 prompt_text)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_prompt_text: Option<String>,
    /// 主参考音频语种(对应 v2 API 的 prompt_lang)
    /// zh/en/ja/ko/yue
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_prompt_lang: Option<String>,
    /// 辅助参考音频路径列表(不限数量和长度,对应 v2 API 的 aux_ref_audio_paths)
    /// 用于多参考音频音色融合
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_aux_ref_audios: Option<Vec<String>>,
    /// 并行推理(默认 true,对应 v2 API 的 parallel_infer)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_parallel_infer: Option<bool>,
    /// 文本切分方式(对应 v2 API 的 text_split_method)
    /// cut0=不切 | cut1=四句一切 | cut2=按标点切 | cut3=按英文句号切 | cut4=按标点切(无逗号) | cut5=按标点切(混合)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_text_split_method: Option<String>,
    /// top_k 采样参数(可选,默认 15)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_top_k: Option<i32>,
    /// top_p 采样参数(可选,默认 1.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_top_p: Option<f64>,
    /// 温度参数(可选,默认 1.0)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_temperature: Option<f64>,
    /// 情感音色映射：emotion 名 → 参考音频配置
    ///
    /// 当 TTS 被调用时传入 emotion 参数，且 map 中存在该 emotion 的条目，
    /// 则用条目中的 ref_audio_path / prompt_text / prompt_lang 覆盖默认配置，
    /// 让 Vivian 在不同情绪下使用不同音色。
    /// emotion 键名与 LLM 返回的 expression 对齐（happy/sad/angry/shy/neutral 等）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_emotion_voice_map: Option<HashMap<String, EmotionVoiceEntry>>,
    /// 应用启动时是否自动拉起 GPT-SoVITS 后端服务（仅在配置了安装路径时生效）
    #[serde(default)]
    pub gpt_sovits_auto_start: bool,
    /// 是否启用双实例模式：启动两个独立 API 进程（不同端口），两个角色可同时合成不排队
    /// 显存占用翻倍（约 4~8 GB），适合大显存显卡（如 24GB）
    #[serde(default)]
    pub gpt_sovits_dual_instance: bool,
    /// 双实例模式下第二个实例的端口（默认 9881，第一个实例用 gpt_sovits_port）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpt_sovits_second_port: Option<u16>,

    // ── 跨语言 TTS 配置 ──
    /// 显示语言（AI 回复文本的语言代码，如 "zh"、"ja"、"en"）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_language: Option<String>,
    /// TTS 合成语言（语音播放的语言代码）
    /// 与 display_language 不同时，启用翻译流水线：文本送入 TTS 前先翻译为此语言
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tts_language: Option<String>,
    /// 翻译服务提供商："deepl" | "google"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation_provider: Option<String>,
    /// 翻译服务 API Key
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation_api_key: Option<String>,
    /// 翻译服务自定义端点（留空使用官方端点）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translation_endpoint: Option<String>,

    // ── Fish Speech 配置 ──
    /// Fish Speech 服务地址(留空默认 https://api.fish.audio 云端)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fish_speech_url: Option<String>,
    /// Fish Speech API Key(云端必需,本地部署可留空)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fish_speech_key: Option<String>,
    /// Fish Speech 参考 ID(reference_id)
    /// - 云端: fish.audio 模型 ID(如 7f92f8afb8ec43bf81429cc1c9199cb1)
    /// - 本地: references/<id>/ 目录名(通过 /v1/references/add 上传)
    /// - 留空时使用 fish_speech_ref_audio 零样本克隆
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fish_speech_character: Option<String>,
    /// Fish Speech 输出格式(wav/pcm/mp3/opus,默认 wav)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fish_speech_format: Option<String>,
    /// Fish Speech 参考音频本地路径(零样本克隆,in-context learning)
    /// - 与 fish_speech_character 二选一:有 character 优先用 character_id
    /// - 留空且无 character 时使用服务器默认音色
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fish_speech_ref_audio: Option<String>,
    /// Fish Speech 参考音频对应文本(配合 fish_speech_ref_audio 使用)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fish_speech_ref_text: Option<String>,

    // ── Fish Speech 本地服务管理（一键启动）──
    /// Fish Speech 安装路径（git clone 的仓库根目录，含 tools/api_server.py）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fish_speech_install_path: Option<String>,
    /// Python 可执行文件路径（留空则使用 PATH 中的 python）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fish_speech_python_path: Option<String>,
    /// 本地服务监听端口（默认 8080）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fish_speech_port: Option<u16>,
    /// 应用启动时是否自动拉起本地 Fish Speech 服务
    #[serde(default)]
    pub fish_speech_auto_start: bool,
    /// 模型检查点路径（--llama-checkpoint-path，留空使用仓库默认模型）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fish_speech_llama_checkpoint_path: Option<String>,
    /// 解码器检查点路径（--decoder-checkpoint-path，留空使用仓库默认）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fish_speech_decoder_checkpoint_path: Option<String>,
    /// 是否启用半精度推理（--half，显存减半，推荐 GPU 开启）
    #[serde(default)]
    pub fish_speech_half: bool,
    /// 是否启用 torch.compile 加速（--compile，首次启动较慢但推理更快）
    #[serde(default)]
    pub fish_speech_compile: bool,

    // ── MiniMax 配置 ──
    /// MiniMax API Key(Authorization: Bearer <key>)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimax_key: Option<String>,
    /// MiniMax 音色 ID(voice_id,在平台创建音色后获得)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimax_voice_id: Option<String>,
    /// MiniMax 合成模型(speech-01-turbo 极速 / speech-01-hd 高保真,默认极速)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimax_model: Option<String>,
    /// MiniMax 音频格式(mp3 / wav / pcm,默认 mp3)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimax_format: Option<String>,
    /// MiniMax 采样率(16000 / 24000 / 32000,默认 32000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimax_sample_rate: Option<u32>,

    // ── 豆包(火山引擎)配置 ──
    /// 豆包应用 ID(火山引擎控制台获取)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doubao_appid: Option<String>,
    /// 豆包访问令牌(火山引擎控制台获取)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doubao_access_token: Option<String>,
    /// 豆包业务集群(默认 volcano_tts,声音复刻等场景可能不同)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doubao_cluster: Option<String>,
    /// 豆包音色类型(如 BV700_streaming 灿灿 2.0 / BV001_streaming 通用女声)
    /// 完整列表见 https://www.volcengine.com/docs/6561/97465
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doubao_voice_type: Option<String>,
    /// 豆包音频格式(mp3 / wav / pcm / ogg_opus,默认 mp3)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doubao_format: Option<String>,
    /// 豆包采样率(8000 / 16000 / 24000,默认 24000)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doubao_sample_rate: Option<u32>,

    // ── Emotion Prosody 配置 ──
    /// 通用音高偏移(半音,-12 到 +12 为合理范围;正值升高,负值降低)
    ///
    /// 与 azure_pitch 不同,此字段适用于所有后端(EdgeTts/Azure 等)。
    /// Azure 后端优先使用 azure_pitch,若 azure_pitch 为空则使用此字段。
    /// 命中 emotion_voice_profile_map 时被覆盖。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f64>,
    /// 情感韵律映射：emotion 名 → VoiceProfile(pitch/speed/pause/energy)
    ///
    /// 当 TTS 被调用时传入 emotion 参数,且 map 中存在该 emotion 的条目,
    /// 则用条目中的 VoiceProfile 覆盖 config 的 pitch/rate 字段,
    /// 实现情感驱动的韵律调整(语速加快/音高升高等)。
    /// emotion 键名与 LLM 返回的 expression 对齐(happy/sad/angry/shy/neutral 等)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emotion_voice_profile_map: Option<HashMap<String, VoiceProfile>>,
    /// 当前 emotion 字符串(运行时传递,不持久化)
    ///
    /// 由 with_emotion_prosody 设置,供豆包等需要 emotion 字符串参数的后端读取。
    /// skip_serializing 避免写入配置文件(每次调用动态设置)。
    #[serde(default, skip_serializing, skip_deserializing)]
    pub current_emotion: Option<String>,
}

fn default_retry_count() -> u32 {
    1
}

/// serde 默认值：fallback_engine 缺失时填充 EdgeTts
/// 保证旧配置文件（无此字段）加载后也有兜底后端
fn default_fallback_engine() -> Option<TtsEngine> {
    Some(TtsEngine::EdgeTts)
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            rate: 1.0,
            volume: 1.0,
            voice_id: None,
            engine: TtsEngine::None,
            fallback_engine: Some(TtsEngine::EdgeTts),
            retry_count: 1,
            azure_key: None,
            azure_region: None,
            azure_style: None,
            azure_style_degree: None,
            azure_role: None,
            azure_pitch: None,
            azure_output_format: None,
            gpt_sovits_url: None,
            gpt_sovits_install_path: None,
            gpt_sovits_config_path: None,
            gpt_sovits_gpt_model: None,
            gpt_sovits_sovits_model: None,
            gpt_sovits_gpu: None,
            gpt_sovits_port: None,
            gpt_sovits_python_path: None,
            gpt_sovits_ref_audio: None,
            gpt_sovits_prompt_text: None,
            gpt_sovits_prompt_lang: None,
            gpt_sovits_aux_ref_audios: None,
            gpt_sovits_parallel_infer: None,
            gpt_sovits_text_split_method: None,
            gpt_sovits_top_k: None,
            gpt_sovits_top_p: None,
            gpt_sovits_temperature: None,
            gpt_sovits_emotion_voice_map: None,
            gpt_sovits_auto_start: false,
            gpt_sovits_dual_instance: false,
            gpt_sovits_second_port: None,
            display_language: None,
            tts_language: None,
            translation_provider: None,
            translation_api_key: None,
            translation_endpoint: None,
            fish_speech_url: None,
            fish_speech_key: None,
            fish_speech_character: None,
            fish_speech_format: None,
            fish_speech_ref_audio: None,
            fish_speech_ref_text: None,
            fish_speech_install_path: None,
            fish_speech_python_path: None,
            fish_speech_port: None,
            fish_speech_auto_start: false,
            fish_speech_llama_checkpoint_path: None,
            fish_speech_decoder_checkpoint_path: None,
            fish_speech_half: false,
            fish_speech_compile: false,
            minimax_key: None,
            minimax_voice_id: None,
            minimax_model: None,
            minimax_format: None,
            minimax_sample_rate: None,
            doubao_appid: None,
            doubao_access_token: None,
            doubao_cluster: None,
            doubao_voice_type: None,
            doubao_format: None,
            doubao_sample_rate: None,
            pitch: None,
            emotion_voice_profile_map: None,
            current_emotion: None,
        }
    }
}

impl TtsConfig {
    /// 归一化遗留配置:rate>10 视为旧 i32 格式(0-200 标度),重置为 1.0
    fn normalize_legacy(&mut self) {
        if self.rate > 10.0 {
            tracing::warn!(
                "[TTS] 检测到遗留 rate 格式({}),已重置为 1.0",
                self.rate
            );
            self.rate = 1.0;
        }
        // 遗留 BertVits2 → Fish Speech(老 Bert-VITS2 后端已被 Fish Speech 替换)
        if self.engine == TtsEngine::BertVits2 {
            tracing::warn!("[TTS] 检测到遗留 BertVits2 引擎,已迁移到 FishSpeech");
            self.engine = TtsEngine::FishSpeech;
            // 旧字段迁移:bert_vits2_url → fish_speech_url,character 同理
            // 注意:旧 Bert-VITS2 服务与新 Fish Speech API 不兼容,此处仅迁移字段,
            // 用户需更新服务地址为 Fish Speech 实例
        }
        if let Some(fe) = &self.fallback_engine {
            if fe == &TtsEngine::BertVits2 {
                self.fallback_engine = Some(TtsEngine::FishSpeech);
            }
        }
    }

    /// 应用情感音色覆盖：若 emotion_voice_map 中存在该 emotion，
    /// 返回一份覆盖了 ref_audio/prompt_text/prompt_lang 的新配置；
    /// 否则返回原配置的克隆。
    ///
    /// 仅对 GPT-SoVITS 后端生效（其他后端忽略此覆盖）。
    pub fn with_emotion_overlay(&self, emotion: Option<&str>) -> TtsConfig {
        let mut cloned = self.clone();
        let emotion = match emotion {
            Some(e) if !e.is_empty() => e,
            _ => return cloned,
        };
        let map = match &self.gpt_sovits_emotion_voice_map {
            Some(m) => m,
            None => return cloned,
        };
        let entry = match map.get(emotion) {
            Some(e) => e,
            None => return cloned,
        };
        tracing::debug!(
            "[TTS] emotion overlay 命中: emotion={} ref_audio={}",
            emotion,
            entry.ref_audio_path
        );
        cloned.gpt_sovits_ref_audio = Some(entry.ref_audio_path.clone());
        if entry.prompt_text.is_some() {
            cloned.gpt_sovits_prompt_text = entry.prompt_text.clone();
        }
        if entry.prompt_lang.is_some() {
            cloned.gpt_sovits_prompt_lang = entry.prompt_lang.clone();
        }
        cloned
    }

    /// 应用情感韵律覆盖(Emotion Prosody)
    ///
    /// 若 emotion_voice_profile_map 中存在该 emotion 的条目,
    /// 用 VoiceProfile 覆盖 config 的 pitch/rate 字段。
    /// 与 with_emotion_overlay 独立,可链式调用:
    /// `config.with_emotion_overlay(emotion).with_emotion_prosody(emotion)`
    pub fn with_emotion_prosody(&self, emotion: Option<&str>) -> TtsConfig {
        let mut cloned = self.clone();
        let emotion = match emotion {
            Some(e) if !e.is_empty() => e,
            _ => return cloned,
        };
        // 设置 current_emotion,供豆包等需要 emotion 字符串的后端读取
        cloned.current_emotion = Some(emotion.to_string());

        let map = match &self.emotion_voice_profile_map {
            Some(m) => m,
            None => return cloned,
        };
        let profile = match map.get(emotion) {
            Some(p) => p,
            None => return cloned,
        };
        tracing::debug!(
            "[TTS] emotion prosody 命中: emotion={} pitch={:?} speed={:?} energy={:?}",
            emotion,
            profile.pitch,
            profile.speed,
            profile.energy
        );
        if let Some(pitch) = profile.pitch {
            cloned.pitch = Some(pitch);
        }
        if let Some(speed) = profile.speed {
            cloned.rate = speed;
        }
        // pause / energy 目前仅记录,后端支持待扩展
        // Azure 的 style_degree 可通过 energy 映射(若用户未单独配置 azure_style_degree)
        if let Some(energy) = profile.energy {
            if cloned.azure_style_degree.is_none() {
                cloned.azure_style_degree = Some(energy);
            }
        }
        cloned
    }

    /// 应用 TTS 控制标记的语速倍率覆盖（书中 9.7 的 [SPEED:x]）。
    ///
    /// 直接覆盖 rate 字段，供 `parse_tts_controls` 提取到的语速使用。
    pub fn with_speed_override(&self, speed: Option<f64>) -> TtsConfig {
        let mut cloned = self.clone();
        if let Some(s) = speed {
            if s > 0.0 {
                cloned.rate = s;
            }
        }
        cloned
    }

    /// 应用言语上下文(Speech Context)
    ///
    /// 在 emotion prosody 之上叠加场景调整:
    /// - 场景类型决定基础 pitch/speed 偏移
    /// - 能量低 → 放缓、降低
    /// - 亲密度高 → 放缓、微升
    ///
    /// 叠加方式:context 的偏移量加到当前 pitch/rate 上(而非覆盖)。
    /// 链式调用顺序:overlay → prosody → context
    pub fn with_context(&self, context: Option<&super::planner::SpeechContext>) -> TtsConfig {
        let context = match context {
            Some(c) => c,
            None => return self.clone(),
        };
        let overlay = context.to_profile_overlay();
        let mut cloned = self.clone();

        // pitch 叠加(当前值 + 偏移量)
        let current_pitch = cloned.pitch.unwrap_or(0.0);
        let overlay_pitch = overlay.pitch.unwrap_or(0.0);
        cloned.pitch = Some(current_pitch + overlay_pitch);

        // speed 叠加(当前 rate × 偏移倍率)
        // overlay.speed 是倍率(1.0=不变),与当前 rate 相乘
        let overlay_speed = overlay.speed.unwrap_or(1.0);
        cloned.rate = (cloned.rate * overlay_speed * 100.0).round() / 100.0;

        tracing::debug!(
            "[TTS] context 叠加: scene={:?} energy={} closeness={} → pitch+{} speed×{}",
            context.scene,
            context.energy,
            context.closeness,
            overlay_pitch,
            overlay_speed
        );
        cloned
    }
}

/// 语音合成管理器
pub struct TtsManager {
    config: Arc<RwLock<TtsConfig>>,
    persistence_path: std::path::PathBuf,
    speaking: Arc<std::sync::atomic::AtomicBool>,
    /// 播放取消标志(stop 时置位,MciPlayer 轮询检测)
    cancel: Arc<std::sync::atomic::AtomicBool>,
    /// 口型同步回调(兼容旧接口,由 WordBoundary 驱动)
    mouth_callback: Arc<RwLock<Option<MouthCallback>>>,
    /// TTS 事件回调(用于向前端推送 tts:word / tts:started / tts:finished)
    event_callback: Arc<RwLock<Option<TtsEventCallback>>>,
    /// 播放代次：每次 speak 递增，用于序号防穿插（旧代次的播放自动作废）
    generation: Arc<std::sync::atomic::AtomicU64>,
    /// 语音缓存(text+voice+emotion hash → 本地音频文件)
    cache: super::tts_cache::SpeechCache,
    /// Ducking 因子(1.0=正常音量,0.3=被压低)
    ///
    /// 由 SpeechPlanner 设置:当 Background 优先级的播放与其他角色并行时,
    /// Planner 调用 set_ducking(0.3) 压低其音量;其他角色停止后恢复 1.0。
    /// play_audio 中的 watcher 线程周期性读取此值并应用 mci_set_volume。
    ducking_factor: Arc<RwLock<f64>>,
    /// 言语记忆(记录最近说过的内容,供 Brain 查询避免重复)
    speech_memory: Arc<super::speech_memory::SpeechMemory>,
    /// 缓存的后端实例(避免每次 create_backend + 连接建立开销)
    ///
    /// 存储 Arc<(引擎类型, 后端实例)>，当引擎类型变更时自动失效。
    /// Arc 允许 speak_with_context 持有后端引用跨 await，即使 prewarm 并发修改缓存也不影响。
    cached_backend: tokio::sync::Mutex<Option<Arc<(TtsEngine, Box<dyn TtsBackend>)>>>,
}

impl TtsManager {
    /// 创建 TTS 管理器，按角色 ID 隔离持久化路径
    ///
    /// 配置文件路径：`<user_data_dir>/characters/<char_id>/sound/config.json`
    /// 若新路径不存在但旧的全局路径 `sound/config.json` 存在，则一次性迁移过来，
    /// 保证旧版本用户的配置不丢失。
    pub fn new(char_id: &str) -> VivianResult<Self> {
        let sound_dir = crate::utils::path::get_character_data_dir(char_id).join("sound");
        std::fs::create_dir_all(&sound_dir)
            .map_err(|e| VivianError::Memory(format!("创建声音目录失败: {e}")))?;

        let persistence_path = sound_dir.join("config.json");

        // 旧路径迁移：首次升级到按角色隔离时，把全局 config.json 复制到角色目录
        if !persistence_path.exists() {
            let legacy = get_user_data_dir().join("sound").join("config.json");
            if legacy.exists() {
                if let Err(e) = std::fs::copy(&legacy, &persistence_path) {
                    tracing::warn!("[TTS] 迁移旧配置失败,使用默认配置: {e}");
                } else {
                    tracing::info!("[TTS] 已迁移旧 TTS 配置到角色目录: {}", persistence_path.display());
                }
            }
        }

        let mut config = if persistence_path.exists() {
            Self::load_from(&persistence_path).unwrap_or_default()
        } else {
            TtsConfig::default()
        };
        config.normalize_legacy();

        Ok(Self {
            config: Arc::new(RwLock::new(config)),
            persistence_path,
            speaking: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            mouth_callback: Arc::new(RwLock::new(None)),
            event_callback: Arc::new(RwLock::new(None)),
            generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            cache: super::tts_cache::SpeechCache::new(char_id)?,
            ducking_factor: Arc::new(RwLock::new(1.0)),
            speech_memory: Arc::new(super::speech_memory::SpeechMemory::new()),
            cached_backend: tokio::sync::Mutex::new(None),
        })
    }

    /// 注册口型同步回调(兼容旧接口)
    ///
    /// 朗读期间根据 WordBoundary 事件驱动此回调;
    /// 朗读结束/停止时调用 `callback(0.0)` 复位嘴形。
    pub fn set_mouth_callback(&self, callback: Option<MouthCallback>) {
        *self.mouth_callback.write() = callback;
    }

    /// 注册 TTS 事件回调(用于向前端推送合成/播放进度)
    pub fn set_event_callback(&self, callback: Option<TtsEventCallback>) {
        *self.event_callback.write() = callback;
    }

    fn emit_event(&self, event: &TtsEvent) {
        if let Some(cb) = self.event_callback.read().as_ref() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cb(event);
            }));
        }
    }

    fn load_from(path: &std::path::Path) -> VivianResult<TtsConfig> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| VivianError::Memory(format!("读取 TTS 配置失败: {e}")))?;
        if content.trim().is_empty() {
            return Ok(TtsConfig::default());
        }
        // 预处理:将遗留 "sapi5" 引擎值迁移到 "windows"
        // SAPI5 已从枚举中移除,这里做一次 JSON 级别迁移以兼容旧配置
        let mut value: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| VivianError::Memory(format!("解析 TTS 配置失败: {e}")))?;
        if let Some(obj) = value.as_object_mut() {
            for key in &["engine", "fallback_engine"] {
                if let Some(v) = obj.get_mut(*key) {
                    if v.as_str() == Some("sapi5") {
                        *v = serde_json::Value::String("windows".to_string());
                    }
                }
            }
        }
        serde_json::from_value::<TtsConfig>(value)
            .map_err(|e| VivianError::Memory(format!("解析 TTS 配置失败: {e}")))
    }

    fn save_to(&self) -> VivianResult<()> {
        let config = self.config.read();
        let json = serde_json::to_string_pretty(&*config)
            .map_err(|e| VivianError::Memory(format!("序列化 TTS 配置失败: {e}")))?;
        let tmp = self.persistence_path.with_extension("json.tmp");
        std::fs::write(&tmp, json)
            .map_err(|e| VivianError::Memory(format!("写入 TTS 临时文件失败: {e}")))?;
        std::fs::rename(&tmp, &self.persistence_path)
            .map_err(|e| VivianError::Memory(format!("替换 TTS 文件失败: {e}")))?;
        Ok(())
    }

    pub fn get_config(&self) -> TtsConfig {
        self.config.read().clone()
    }

    pub fn set_config(&self, config: TtsConfig) -> VivianResult<()> {
        {
            let mut guard = self.config.write();
            *guard = config;
        }
        self.save_to()?;
        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        self.config.read().enabled
    }

    pub fn is_speaking(&self) -> bool {
        self.speaking.load(Ordering::SeqCst)
    }

    /// 解析实际使用的引擎（主引擎为 None 时使用 fallback）
    fn resolve_engine(config: &TtsConfig) -> VivianResult<TtsEngine> {
        let engine = config.engine.resolve();
        if !matches!(engine, TtsEngine::None) {
            return Ok(engine);
        }
        match config.fallback_engine.as_ref().map(|e| e.resolve()) {
            Some(fb) if !matches!(fb, TtsEngine::None) => {
                tracing::warn!("[TTS] 主引擎为 None,使用 fallback: {:?}", fb);
                Ok(fb)
            }
            _ => Err(VivianError::Speech("TTS 引擎为 None,且无可用 fallback".to_string())),
        }
    }

    /// 预热 TTS 后端连接
    ///
    /// 在 LLM 首 token 到达时调用,提前建立 WSS/HTTP 连接。
    /// 缓存后端实例，后续 synthesize 复用同一实例（含已建立的连接）。
    pub async fn prewarm(&self) -> VivianResult<()> {
        let config = self.config.read().clone();
        let engine = match Self::resolve_engine(&config) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        };
        let mut guard = self.cached_backend.lock().await;
        if let Some(cached) = guard.as_ref() {
            if cached.0 != engine {
                tracing::info!(
                    "[TTS] 引擎变更 {:?} → {:?}，清除缓存后端",
                    cached.0,
                    engine
                );
                *guard = None;
            }
        }
        if guard.is_none() {
            *guard = Some(Arc::new((engine.clone(), create_backend(&engine)?)));
        }
        if let Some(cached) = guard.as_ref() {
            cached.1.prewarm(&config).await
        } else {
            Ok(())
        }
    }

    /// 预合成文本（只写缓存，不播放）
    ///
    /// 复用 speak_with_backend 的缓存读写逻辑：
    /// 缓存命中 → 立即返回（已合成过）
    /// 缓存未命中 → 调用后端合成 → 写入缓存
    ///
    /// 前端在播放当前句子时 fire-and-forget 调用此方法预合成下一句，
    /// 后续 speak_text 命中缓存直接播放，消除句间合成延迟。
    pub async fn prefetch(&self, text: &str, emotion: Option<&str>) -> VivianResult<()> {
        if text.trim().is_empty() {
            return Ok(());
        }
        // 与 speak_with_context 相同的 overlay 链，确保缓存键一致
        let config = self
            .config
            .read()
            .with_emotion_overlay(emotion)
            .with_emotion_prosody(emotion);
        let engine = Self::resolve_engine(&config)?;
        // 独立连接：不复用 cached_backend，让多个 prefetch 可并行合成
        // （Edge TTS WebSocket 不支持同一连接上并发多路合成）
        let backend = create_backend(&engine)?;
        let engine_name = backend.name().to_string();
        match self
            .speak_with_backend(backend.as_ref(), &config, text, &engine_name, emotion)
            .await
        {
            Ok(_) => {
                tracing::info!(
                    "[TTS] prefetch 完成: {:?} ({}字)",
                    text,
                    text.chars().count()
                );
                Ok(())
            }
            Err(e) => {
                tracing::warn!("[TTS] prefetch 失败: {}", e);
                Err(e)
            }
        }
    }

    /// 只合成不播放，保存为音频文件并返回相对路径与估算时长
    ///
    /// 用于微信渠道语音消息：LLM 返回 voice_message=true 时，
    /// 合成 TTS 音频保存到 `<user_data_dir>/audio/` 下，
    /// 前端以语音气泡展示，点击播放。
    pub async fn synthesize_to_file(
        &self,
        text: &str,
        emotion: Option<&str>,
    ) -> VivianResult<(String, f64)> {
        if text.trim().is_empty() {
            return Err(VivianError::Speech("文本为空，无法合成".to_string()));
        }

        let config = self
            .config
            .read()
            .with_emotion_overlay(emotion)
            .with_emotion_prosody(emotion);
        let engine = Self::resolve_engine(&config)?;
        let backend = create_backend(&engine)?;
        let engine_name = backend.name().to_string();

        let result = self
            .speak_with_backend(backend.as_ref(), &config, text, &engine_name, emotion)
            .await?;

        // 保存到 <user_data_dir>/audio/
        let ext = match result.format {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Wav => "wav",
            AudioFormat::Pcm => "pcm",
            AudioFormat::Ogg => "ogg",
            AudioFormat::Aac => "aac",
        };
        let data_dir = get_user_data_dir();
        let audio_dir = data_dir.join("audio");
        crate::utils::path::ensure_dir(&audio_dir)
            .map_err(|e| VivianError::Speech(format!("创建音频目录失败: {e}")))?;
        let saved_name = format!("{}.{}", uuid::Uuid::new_v4(), ext);
        let saved_path = audio_dir.join(&saved_name);
        std::fs::write(&saved_path, &result.audio)
            .map_err(|e| VivianError::Speech(format!("保存音频文件失败: {e}")))?;

        let rel_path = format!("audio/{}", saved_name);

        // 估算时长：MP3 约 16KB/s（128kbps），WAV/PCM 约 88KB/s（16bit 44.1kHz）
        let bytes = result.audio.len() as f64;
        let duration = match result.format {
            AudioFormat::Mp3 | AudioFormat::Aac | AudioFormat::Ogg => bytes / 16000.0,
            AudioFormat::Wav | AudioFormat::Pcm => bytes / 88000.0,
        };
        // 限制最小 1 秒，避免显示 0″
        let duration = duration.max(1.0);

        tracing::info!(
            "[TTS] synthesize_to_file 完成: {} 字, {} 秒, {}",
            text.chars().count(),
            duration,
            rel_path
        );

        Ok((rel_path, duration))
    }

    /// 获取缓存的后端实例（引擎变更时自动重建，支持 fallback）
    ///
    /// 返回 Arc 包装的后端，调用方可安全持有跨 await。
    async fn get_cached_backend(&self) -> VivianResult<Arc<(TtsEngine, Box<dyn TtsBackend>)>> {
        let config = self.config.read().clone();
        let engine = Self::resolve_engine(&config)?;
        let mut guard = self.cached_backend.lock().await;
        if let Some(cached) = guard.as_ref() {
            if cached.0 != engine {
                *guard = None;
            }
        }
        if guard.is_none() {
            *guard = Some(Arc::new((engine.clone(), create_backend(&engine)?)));
        }
        guard
            .clone()
            .ok_or_else(|| VivianError::Speech("无法创建 TTS 后端".to_string()))
    }

    /// 列出当前后端可用语音
    pub async fn list_voices(&self) -> VivianResult<Vec<VoiceInfo>> {
        let config = self.config.read().clone();
        let engine = match Self::resolve_engine(&config) {
            Ok(e) => e,
            Err(_) => return Ok(Vec::new()),
        };
        let backend = create_backend(&engine)?;
        backend.list_voices(&config).await
    }

    /// 测试当前后端(合成一小段文本)
    pub async fn test(&self) -> VivianResult<()> {
        let config = self.config.read().clone();
        let engine = Self::resolve_engine(&config)?;
        let backend = create_backend(&engine)?;
        let result = backend.synthesize("你好,这是语音测试。", &config).await?;
        if result.audio.is_empty() {
            return Err(VivianError::Speech("测试合成返回空音频".to_string()));
        }
        Ok(())
    }

    /// 朗读文本(异步)
    ///
    /// 流程:
    /// 1. 检查 Speech Cache 是否命中 → 命中则直接返回缓存的音频
    /// 2. 主后端合成(失败→fallback 后端)
    /// 3. 写入缓存
    async fn speak_with_backend(
        &self,
        backend: &dyn TtsBackend,
        config: &TtsConfig,
        text: &str,
        engine_name: &str,
        emotion: Option<&str>,
    ) -> VivianResult<TtsSynthesisResult> {
        // 清洗 Markdown / 富文本符号，避免 TTS 朗读星号、井号等格式标记
        let text = strip_markdown_for_tts(text);
        let text = text.as_str();
        if text.trim().is_empty() {
            return Err(VivianError::Speech("清洗后无可朗读文本".to_string()));
        }

        // 检查缓存命中
        let voice_str = config.voice_id.clone().unwrap_or_default();
        if let Some(cached) = self.cache.get(
            text,
            &voice_str,
            emotion,
            engine_name,
            config.rate,
            config.volume,
            config.pitch,
        ) {
            return Ok(cached);
        }

        let mut last_err: Option<VivianError> = None;
        let retries = config.retry_count.max(1);
        for attempt in 0..retries {
            if attempt > 0 {
                tracing::warn!(
                    "[TTS] {} 第 {} 次重试",
                    engine_name,
                    attempt
                );
            }
            match backend.synthesize(text, config).await {
                Ok(result) if !result.audio.is_empty() => {
                    // 写入缓存
                    self.cache.put(
                        text,
                        &voice_str,
                        emotion,
                        engine_name,
                        config.rate,
                        config.volume,
                        config.pitch,
                        &result,
                    );
                    return Ok(result);
                }
                Ok(_) => {
                    last_err = Some(VivianError::Speech(format!(
                        "{} 返回空音频",
                        engine_name
                    )));
                }
                Err(e) => {
                    tracing::warn!("[TTS] {} 合成失败: {}", engine_name, e);
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            VivianError::Speech(format!("{} 合成失败(未知原因)", engine_name))
        }))
    }

    /// 朗读文本
    ///
    /// 早播放策略：将文本按句切分，首句合成后立即播放，
    /// 后续句子在播放期间并行合成，减少首音延迟。
    /// 通过 generation 计数器实现序号防穿插：新的 speak 调用会使旧代次作废。
    pub async fn speak(&self, text: &str) -> VivianResult<()> {
        self.speak_with_emotion(text, None).await
    }

    /// 朗读文本（带情感参数）
    ///
    /// emotion 参数用于 GPT-SoVITS emotionVoiceMap 覆盖：
    /// 若配置了 emotion_voice_map 且传入的 emotion 命中，则使用该情绪对应的参考音频。
    pub async fn speak_with_emotion(
        &self,
        text: &str,
        emotion: Option<&str>,
    ) -> VivianResult<()> {
        self.speak_with_context(text, emotion, None).await
    }

    /// 朗读文本(带情感参数 + 言语上下文)
    ///
    /// 在 emotion prosody 之上叠加 Speech Context 调整:
    /// - 场景类型决定基础 pitch/speed 偏移
    /// - 能量低 → 放缓、降低
    /// - 亲密度高 → 放缓、微升
    pub async fn speak_with_context(
        &self,
        text: &str,
        emotion: Option<&str>,
        context: Option<&super::planner::SpeechContext>,
    ) -> VivianResult<()> {
        if text.trim().is_empty() {
            return Ok(());
        }

        // 解析 TTS 控制标记（书中 9.7）：剥离 [EMO]/[THINKING]/[SPEED]/[PAUSE]，
        // 提取语速覆盖与思考停顿。后续全部使用剥离标记后的文本。
        let controls = parse_tts_controls(text);
        let text: &str = controls.text.as_str();
        if text.trim().is_empty() {
            // 纯标记（如仅 [THINKING]）无可读文本，跳过合成
            return Ok(());
        }

        // 最近播放去重:防止 proactive tick / wake_from_presence 等多路径在短时间内
        // 重复触发同一句合成(日志显示 5-10s 间隔重复合成)。
        // 30s 窗口覆盖日志中观察到的所有重复间隔,又不至于误杀用户主动"再说一遍"场景。
        const RECENT_SPOKEN_DEDUP_WINDOW: std::time::Duration = std::time::Duration::from_secs(30);
        if self
            .speech_memory
            .recently_spoken(text, RECENT_SPOKEN_DEDUP_WINDOW)
        {
            tracing::debug!(
                "[TtsManager] 最近 {}s 已合成相同文本,跳过: \"{}\"",
                RECENT_SPOKEN_DEDUP_WINDOW.as_secs(),
                text
            );
            return Ok(());
        }

        // 记录到言语记忆(供 Brain 查询避免重复)
        self.speech_memory.record(text);

        let config = self
            .config
            .read()
            .with_emotion_overlay(emotion)
            .with_emotion_prosody(emotion)
            .with_speed_override(controls.speed)
            .with_context(context);
        let engine = Self::resolve_engine(&config)?;

        // 停止上一次播放并递增 generation（序号防穿插）
        self.stop_internal();
        let my_gen = self
            .generation
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);

        self.speaking.store(true, Ordering::SeqCst);
        self.cancel.store(false, Ordering::SeqCst);

        // 思考停顿：播放前静默 N 毫秒（[THINKING]/[PAUSE] 标记），让语气更像人
        if controls.pause_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(controls.pause_ms)).await;
        }

        // 获取或创建缓存的后端实例（复用 prewarm 建立的连接）
        let cached = self.get_cached_backend().await?;
        let backend: &dyn TtsBackend = cached.1.as_ref();
        let engine_name = backend.name().to_string();
        self.emit_event(&TtsEvent::Started {
            text: text.to_string(),
            engine: engine_name.clone(),
        });

        // 按句切分（早播放）
        let sentences = split_sentences(text);
        let single = sentences.len() <= 1;

        if single {
            // 短文本：单次合成 + 播放（原路径）
            let result = self
                .speak_with_backend(backend, &config, text, &engine_name, emotion)
                .await;
            let synthesis = match result {
                Ok(r) => r,
                Err(e) => match self.try_fallback(&config, text, &engine, &engine_name, &e).await {
                    Some(r) => r,
                    None => {
                        self.speaking.store(false, Ordering::SeqCst);
                        self.emit_event(&TtsEvent::Error {
                            message: e.to_string(),
                            engine: engine_name.clone(),
                        });
                        self.reset_mouth();
                        return Err(e);
                    }
                },
            };
            return self.finish_play(synthesis, &config, text, &engine_name).await;
        }

        // 多句：流水线 — 播放句 N 时并行预合成句 N+1
        let mut next_synth: Option<tokio::task::JoinHandle<VivianResult<TtsSynthesisResult>>> =
            None;

        for (idx, sentence) in sentences.iter().enumerate() {
            // 序号防穿插：检查是否被新 speak 取代
            if self.generation.load(Ordering::SeqCst) != my_gen {
                tracing::debug!("[TTS] generation 变更，中止旧代次播放");
                break;
            }
            if self.cancel.load(Ordering::SeqCst) {
                tracing::debug!("[TTS] 检测到 cancel，中止播放");
                break;
            }

            // 获取合成结果：优先取预合成的，否则直接合成
            let synthesis = if let Some(handle) = next_synth.take() {
                // 取预合成结果
                match handle.await {
                    Ok(Ok(r)) => r,
                    Ok(Err(e)) => {
                        tracing::warn!("[TTS] 第 {} 句预合成失败，跳过: {}", idx + 1, e);
                        continue;
                    }
                    Err(e) => {
                        tracing::warn!("[TTS] 第 {} 句预合成任务 panic: {}", idx + 1, e);
                        continue;
                    }
                }
            } else {
                // 首句或预合成未启动：直接合成
                match self
                    .speak_with_backend(backend, &config, sentence, &engine_name, emotion)
                    .await
                {
                    Ok(r) => r,
                    Err(e) => {
                        if idx == 0 {
                            match self
                                .try_fallback(&config, sentence, &engine, &engine_name, &e)
                                .await
                            {
                                Some(r) => r,
                                None => {
                                    self.speaking.store(false, Ordering::SeqCst);
                                    self.emit_event(&TtsEvent::Error {
                                        message: e.to_string(),
                                        engine: engine_name.clone(),
                                    });
                                    self.reset_mouth();
                                    return Err(e);
                                }
                            }
                        } else {
                            tracing::warn!("[TTS] 第 {} 句合成失败，跳过: {}", idx + 1, e);
                            continue;
                        }
                    }
                }
            };

            // 预合成下一句（fire-and-forget spawn，播放期间并行执行）
            // 主后端失败时自动尝试 fallback 后端，避免预合成被直接跳过
            if idx + 1 < sentences.len() {
                let next_sentence = sentences[idx + 1].clone();
                let next_config = config.clone();
                let next_engine = engine.clone();
                let fallback_engine = config.fallback_engine.clone();
                next_synth = Some(tokio::spawn(async move {
                    let next_backend = create_backend(&next_engine)?;
                    match next_backend.synthesize(&next_sentence, &next_config).await {
                        Ok(r) if !r.audio.is_empty() => Ok(r),
                        Err(primary_err) => {
                            if let Some(fb) = fallback_engine {
                                let fb_resolved = fb.resolve();
                                if !matches!(fb_resolved, TtsEngine::None)
                                    && fb_resolved != next_engine
                                {
                                    tracing::warn!(
                                        "[TTS] 预合成主后端 {:?} 失败,fallback 到 {:?}: {}",
                                        next_engine, fb_resolved, primary_err
                                    );
                                    let fb_backend = create_backend(&fb_resolved)?;
                                    return fb_backend
                                        .synthesize(&next_sentence, &next_config)
                                        .await;
                                }
                            }
                            Err(primary_err)
                        }
                        Ok(empty) => Ok(empty),
                    }
                }));
            }

            // 播放当前句（阻塞至播放完成）
            if let Err(e) = self
                .play_audio(synthesis, &config, sentence, &engine_name)
                .await
            {
                tracing::warn!("[TTS] 第 {} 句播放失败: {}", idx + 1, e);
            }
        }

        self.speaking.store(false, Ordering::SeqCst);
        self.reset_mouth();
        self.emit_event(&TtsEvent::Finished);
        Ok(())
    }

    /// 完成单次合成后的播放收尾（提取公共逻辑）
    async fn finish_play(
        &self,
        synthesis: TtsSynthesisResult,
        config: &TtsConfig,
        text: &str,
        engine_name: &str,
    ) -> VivianResult<()> {
        let play_result = self.play_audio(synthesis, config, text, engine_name).await;
        self.speaking.store(false, Ordering::SeqCst);
        self.reset_mouth();
        match play_result {
            Ok(()) => {
                self.emit_event(&TtsEvent::Finished);
                Ok(())
            }
            Err(e) => {
                self.emit_event(&TtsEvent::Error {
                    message: e.to_string(),
                    engine: engine_name.to_string(),
                });
                Err(e)
            }
        }
    }

    /// 尝试使用 fallback 后端合成
    ///
    /// 返回 `Some(TtsSynthesisResult)` 表示 fallback 成功;
    /// 返回 `None` 表示无可用 fallback 或 fallback 也失败(错误已通过事件推送)
    async fn try_fallback(
        &self,
        config: &TtsConfig,
        text: &str,
        primary_engine: &TtsEngine,
        primary_engine_name: &str,
        primary_err: &VivianError,
    ) -> Option<TtsSynthesisResult> {
        let fb_engine = config.fallback_engine.clone()?;
        let fb_resolved = fb_engine.resolve();
        if matches!(fb_resolved, TtsEngine::None) || fb_resolved == *primary_engine {
            return None;
        }

        let fb_name_str = format!("{:?}", fb_resolved).to_lowercase();
        self.emit_event(&TtsEvent::Fallback {
            from: primary_engine_name.to_string(),
            to: fb_name_str.clone(),
            reason: primary_err.to_string(),
        });
        tracing::warn!(
            "[TTS] 主后端 {} 失败,fallback 到 {}",
            primary_engine_name,
            fb_name_str
        );

        let fb_backend = match create_backend(&fb_resolved) {
            Ok(b) => b,
            Err(create_err) => {
                self.emit_event(&TtsEvent::Error {
                    message: create_err.to_string(),
                    engine: fb_name_str.clone(),
                });
                return None;
            }
        };
        let fb_name = fb_backend.name().to_string();

        match self
            .speak_with_backend(fb_backend.as_ref(), config, text, &fb_name, None)
            .await
        {
            Ok(r) => Some(r),
            Err(fb_err) => {
                self.emit_event(&TtsEvent::Error {
                    message: fb_err.to_string(),
                    engine: fb_name,
                });
                None
            }
        }
    }

    /// 播放音频:优先 MemoryPlayer（内存直接播放），失败回退 MCI（临时文件）
    async fn play_audio(
        &self,
        synthesis: TtsSynthesisResult,
        config: &TtsConfig,
        text: &str,
        engine_name: &str,
    ) -> VivianResult<()> {
        tracing::info!(
            "[TTS] play_audio: engine={} audio={}字节 format={:?} boundaries={}",
            engine_name, synthesis.audio.len(), synthesis.format, synthesis.word_boundaries.len()
        );

        let cancel = self.cancel.clone();
        let speaking = self.speaking.clone();
        // 单句播放完成标志，用于通知 watcher 线程退出
        let play_done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mouth_cb = self.mouth_callback.clone();
        let event_cb = self.event_callback.clone();
        let boundaries = synthesis.word_boundaries.clone();
        let has_boundaries = !synthesis.word_boundaries.is_empty();
        let format = synthesis.format;
        let text_owned = text.to_string();
        let engine_owned = engine_name.to_string();
        let volume = config.volume;
        let ducking_factor = self.ducking_factor.clone();
        let audio_data = synthesis.audio.clone();

        let result = tokio::task::spawn_blocking(move || -> VivianResult<()> {
            // 尝试 MemoryPlayer (rodio 内存直接播放)
            match MemoryPlayer::play_from_memory(audio_data, format) {
                Ok(player) => {
                    tracing::info!("[TTS] 使用 MemoryPlayer 播放");

                    // 设置初始音量
                    let init_duck = *ducking_factor.read();
                    player.set_volume((volume * init_duck) as f32);

                    // 音量桥:watcher 写入 → 主循环读取并应用到 MemoryPlayer
                    let vol_bridge = Arc::new(std::sync::atomic::AtomicU32::new(
                        (init_duck as f32 * volume as f32 * 1000.0).clamp(0.0, 1000.0) as u32,
                    ));
                    let vb = vol_bridge.clone();
                    let vb_cancel = cancel.clone();
                    let vb_speaking = speaking.clone();
                    let vb_done = play_done.clone();
                    let vb_ducking = ducking_factor.clone();
                    let duck_handle = std::thread::Builder::new()
                        .name("tts-ducking-mem".to_string())
                        .spawn(move || {
                            let mut last_vol = vb.load(Ordering::SeqCst);
                            while !vb_cancel.load(Ordering::SeqCst)
                                && vb_speaking.load(Ordering::SeqCst)
                                && !vb_done.load(Ordering::SeqCst)
                            {
                                let duck = *vb_ducking.read();
                                let new_vol =
                                    (duck as f32 * volume as f32 * 1000.0).clamp(0.0, 1000.0) as u32;
                                let diff = new_vol.abs_diff(last_vol);
                                if diff > 50 {
                                    vb.store(new_vol, Ordering::SeqCst);
                                    last_vol = new_vol;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(100));
                            }
                        })
                        .ok();

                    // WordBoundary 驱动线程
                    let wb_handle = if has_boundaries {
                        let wb_cancel = cancel.clone();
                        let wb_mouth = mouth_cb.clone();
                        let wb_event = event_cb.clone();
                        let wb_speaking = speaking.clone();
                        let wb_done = play_done.clone();
                        let wb_list = boundaries.clone();
                        let wb_text = text_owned.clone();
                        let wb_engine = engine_owned.clone();
                        std::thread::Builder::new()
                            .name("tts-word-boundary".to_string())
                            .spawn(move || {
                                let start = std::time::Instant::now();
                                for wb in &wb_list {
                                    let elapsed = start.elapsed().as_millis() as u64;
                                    if wb.offset_ms > elapsed {
                                        let mut remaining = wb.offset_ms - elapsed;
                                        while remaining > 0 {
                                            if wb_cancel.load(Ordering::SeqCst)
                                                || !wb_speaking.load(Ordering::SeqCst)
                                                || wb_done.load(Ordering::SeqCst)
                                            {
                                                return;
                                            }
                                            let step = remaining.min(50);
                                            std::thread::sleep(
                                                std::time::Duration::from_millis(step),
                                            );
                                            remaining = remaining.saturating_sub(step);
                                        }
                                    }
                                    if wb_cancel.load(Ordering::SeqCst)
                                        || !wb_speaking.load(Ordering::SeqCst)
                                        || wb_done.load(Ordering::SeqCst)
                                    {
                                        return;
                                    }
                                    let mouth = word_to_mouth_open(&wb.text);
                                    if let Some(cb) = wb_mouth.read().as_ref() {
                                        let _ = std::panic::catch_unwind(
                                            std::panic::AssertUnwindSafe(|| cb(mouth as f64)),
                                        );
                                    }
                                    if let Some(cb) = wb_event.read().as_ref() {
                                        let evt = TtsEvent::WordBoundary {
                                            text: wb.text.clone(),
                                            offset_ms: wb.offset_ms,
                                            duration_ms: wb.duration_ms,
                                            mouth_open: mouth,
                                        };
                                        let _ = std::panic::catch_unwind(
                                            std::panic::AssertUnwindSafe(|| cb(&evt)),
                                        );
                                    }
                                }
                                let _ = (wb_text, wb_engine);
                            })
                            .ok()
                    } else {
                        None
                    };

                    // 主循环:应用音量桥 + 等待播放完成
                    // 超时兜底：rodio is_playing() 在某些异常情况下可能永远返回 true，
                    // 用 PLAYBACK_TIMEOUT_SECS 强制 break，避免 speak_text 命令永久阻塞导致
                    // 前端 flushSync 卡死、气泡不消失、消息不入记忆。
                    let play_started_at = std::time::Instant::now();
                    while player.is_playing() {
                        if cancel.load(Ordering::SeqCst) {
                            player.stop();
                            break;
                        }
                        if play_started_at.elapsed() > PLAYBACK_TIMEOUT_SECS {
                            tracing::warn!(
                                "[TTS] MemoryPlayer 播放超过 {}s 未结束，强制中断（避免卡死）",
                                PLAYBACK_TIMEOUT_SECS.as_secs()
                            );
                            player.stop();
                            break;
                        }
                        let target = vol_bridge.load(Ordering::SeqCst);
                        player.set_volume(target as f32 / 1000.0);
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }

                    play_done.store(true, Ordering::SeqCst);

                    if let Some(h) = wb_handle {
                        let _ = h.join();
                    }
                    if let Some(h) = duck_handle {
                        let _ = h.join();
                    }
                    Ok(())
                }
                Err(mem_err) => {
                    tracing::warn!("[TTS] MemoryPlayer 失败, 回退 MCI: {}", mem_err);

                    // MCI 回退路径
                    let temp_path =
                        save_to_temp_file(&synthesis.audio, synthesis.format)?;
                    let mut mci_player = MciPlayer::new();
                    mci_player.play_file(&temp_path, format)?;

                    #[cfg(windows)]
                    {
                        let duck = *ducking_factor.read();
                        let vol = (volume * duck * 1000.0).clamp(0.0, 1000.0) as u32;
                        let _ = mci_set_volume(mci_player.alias_str(), vol);
                    }

                    #[cfg(windows)]
                    let duck_handle = {
                        let dw_cancel = cancel.clone();
                        let dw_speaking = speaking.clone();
                        let dw_done = play_done.clone();
                        let dw_ducking = ducking_factor.clone();
                        let dw_alias = mci_player.alias_str().to_string();
                        let mut dw_last_vol = {
                            let duck = *dw_ducking.read();
                            (volume * duck * 1000.0).clamp(0.0, 1000.0) as u32
                        };
                        std::thread::Builder::new()
                            .name("tts-ducking-mci".to_string())
                            .spawn(move || {
                                while !dw_cancel.load(Ordering::SeqCst)
                                    && dw_speaking.load(Ordering::SeqCst)
                                    && !dw_done.load(Ordering::SeqCst)
                                {
                                    let duck = *dw_ducking.read();
                                    let new_vol =
                                        (volume * duck * 1000.0).clamp(0.0, 1000.0) as u32;
                                    let diff = if new_vol >= dw_last_vol {
                                        new_vol - dw_last_vol
                                    } else {
                                        dw_last_vol - new_vol
                                    };
                                    if diff > 50 {
                                        let _ = mci_set_volume(&dw_alias, new_vol);
                                        dw_last_vol = new_vol;
                                    }
                                    std::thread::sleep(
                                        std::time::Duration::from_millis(100),
                                    );
                                }
                            })
                            .ok()
                    };
                    #[cfg(not(windows))]
                    let duck_handle: Option<std::thread::JoinHandle<()>> = None;

                    let wb_handle = if has_boundaries {
                        let wb_cancel = cancel.clone();
                        let wb_mouth = mouth_cb.clone();
                        let wb_event = event_cb.clone();
                        let wb_speaking = speaking.clone();
                        let wb_done = play_done.clone();
                        let wb_list = boundaries.clone();
                        let wb_text = text_owned.clone();
                        let wb_engine = engine_owned.clone();
                        std::thread::Builder::new()
                            .name("tts-word-boundary".to_string())
                            .spawn(move || {
                                let start = std::time::Instant::now();
                                for wb in &wb_list {
                                    let elapsed = start.elapsed().as_millis() as u64;
                                    if wb.offset_ms > elapsed {
                                        let mut remaining = wb.offset_ms - elapsed;
                                        while remaining > 0 {
                                            if wb_cancel.load(Ordering::SeqCst)
                                                || !wb_speaking.load(Ordering::SeqCst)
                                                || wb_done.load(Ordering::SeqCst)
                                            {
                                                return;
                                            }
                                            let step = remaining.min(50);
                                            std::thread::sleep(
                                                std::time::Duration::from_millis(step),
                                            );
                                            remaining = remaining.saturating_sub(step);
                                        }
                                    }
                                    if wb_cancel.load(Ordering::SeqCst)
                                        || !wb_speaking.load(Ordering::SeqCst)
                                        || wb_done.load(Ordering::SeqCst)
                                    {
                                        return;
                                    }
                                    let mouth = word_to_mouth_open(&wb.text);
                                    if let Some(cb) = wb_mouth.read().as_ref() {
                                        let _ = std::panic::catch_unwind(
                                            std::panic::AssertUnwindSafe(|| cb(mouth as f64)),
                                        );
                                    }
                                    if let Some(cb) = wb_event.read().as_ref() {
                                        let evt = TtsEvent::WordBoundary {
                                            text: wb.text.clone(),
                                            offset_ms: wb.offset_ms,
                                            duration_ms: wb.duration_ms,
                                            mouth_open: mouth,
                                        };
                                        let _ = std::panic::catch_unwind(
                                            std::panic::AssertUnwindSafe(|| cb(&evt)),
                                        );
                                    }
                                }
                                let _ = (wb_text, wb_engine);
                            })
                            .ok()
                    } else {
                        None
                    };

                    mci_player.wait_until_done(&cancel);

                    play_done.store(true, Ordering::SeqCst);

                    if let Some(h) = wb_handle {
                        let _ = h.join();
                    }
                    if let Some(h) = duck_handle {
                        let _ = h.join();
                    }

                    cleanup_temp_file(&temp_path);
                    Ok(())
                }
            }
        })
        .await
        .map_err(|e| VivianError::Speech(format!("TTS 播放任务失败: {e}")))?;

        result
    }

    /// 复位嘴形(mouth_callback(0.0))
    fn reset_mouth(&self) {
        if let Some(cb) = self.mouth_callback.read().as_ref() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                cb(0.0);
            }));
        }
    }

    /// 停止朗读:置位 cancel 标志,MciPlayer 检测后优雅停止
    pub fn stop(&self) -> VivianResult<()> {
        self.stop_internal();
        self.reset_mouth();
        Ok(())
    }

    fn stop_internal(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        // 等待 speaking 标志清零(最多 200ms)
        // 使用 yield_now 而非 thread::sleep，避免阻塞 Tokio async worker 线程
        for _ in 0..20 {
            if !self.speaking.load(Ordering::SeqCst) {
                break;
            }
            std::hint::spin_loop();
        }
        self.speaking.store(false, Ordering::SeqCst);
        self.cancel.store(false, Ordering::SeqCst);
    }

    /// 设置 ducking 因子(0.0-1.0)
    ///
    /// 1.0 = 正常音量;0.3 = 压低到 30% 音量。
    /// 由 SpeechPlanner 在多角色并行播放时调用,压低 Background 优先级的语音。
    /// play_audio 中的 watcher 线程会在 100ms 内应用新值。
    pub fn set_ducking(&self, factor: f64) {
        let clamped = factor.clamp(0.0, 1.0);
        let mut guard = self.ducking_factor.write();
        if (*guard - clamped).abs() > 0.001 {
            tracing::debug!("[TTS] ducking: {:.2} → {:.2}", *guard, clamped);
            *guard = clamped;
        }
    }

    /// 读取当前 ducking 因子
    pub fn get_ducking(&self) -> f64 {
        *self.ducking_factor.read()
    }

    /// 获取言语记忆引用(供 Brain 查询最近说过的内容/高频口头禅)
    ///
    /// Brain 可在生成回复前调用:
    /// - `memory.recently_spoken(text, Duration::from_secs(300))` 判断是否刚说过
    /// - `memory.recent_texts(5)` 获取最近 5 条发言
    /// - `memory.frequent_phrases(10)` 识别口头禅
    pub fn speech_memory(&self) -> &super::speech_memory::SpeechMemory {
        &self.speech_memory
    }
}

impl Default for TtsManager {
    fn default() -> Self {
        tracing::warn!("[TTS] 使用内存模式降级,配置不会持久化");
        let cache = match super::tts_cache::SpeechCache::new("default") {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(
                    "[TTS] SpeechCache 初始化失败，降级到临时目录缓存: {}", e
                );
                super::tts_cache::SpeechCache::fallback()
            }
        };
        TtsManager {
            config: Arc::new(RwLock::new(TtsConfig::default())),
            persistence_path: std::path::PathBuf::from("config.json"),
            speaking: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            mouth_callback: Arc::new(RwLock::new(None)),
            event_callback: Arc::new(RwLock::new(None)),
            generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            cache,
            ducking_factor: Arc::new(RwLock::new(1.0)),
            speech_memory: Arc::new(super::speech_memory::SpeechMemory::new()),
            cached_backend: tokio::sync::Mutex::new(None),
        }
    }
}

/// 语音信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceInfo {
    pub id: String,
    pub name: String,
    pub language: String,
}

/// 去除 Markdown / 富文本格式符号，只保留可朗读的纯文本
///
/// LLM 输出常包含 `**加粗**`、`# 标题`、`[链接](url)` 等 Markdown 标记，
/// TTS 引擎会将这些符号原样朗读（"星号星号你好星号星号"）。
/// 此函数在合成前统一清洗，保证语音只包含自然语言内容。
fn strip_markdown_for_tts(text: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;

    // 编译一次，全局复用（正则较多，避免每次调用重复编译）
    static RE_CODE_BLOCK: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"```[\s\S]*?```").unwrap());
    static RE_BOLD: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\*\*(.+?)\*\*").unwrap());
    static RE_BOLD_UNDER: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"__(.+?)__").unwrap());
    // 斜体 *text*：要求 * 紧邻非空白字符（Markdown 标准），避免误匹配 "2 * 3 * 4"
    static RE_ITALIC: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\*(\S(?:[^*\n]*?\S)?)\*").unwrap());
    // 注：下划线斜体 _text_ 不处理——LLM 输出中极少使用，
    // 且 snake_case 变量名误匹配风险高，收益不抵风险。
    static RE_STRIKE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"~~(.+?)~~").unwrap());
    static RE_INLINE_CODE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"`([^`]+)`").unwrap());
    static RE_IMAGE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"!\[([^\]]*)\]\([^)]*\)").unwrap());
    static RE_LINK: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
    static RE_HEADER: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^#{1,6}\s+").unwrap());
    static RE_BLOCKQUOTE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^>\s?").unwrap());
    static RE_UL_LIST: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^[\s]*[-*+]\s+").unwrap());
    static RE_OL_LIST: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^[\s]*\d+[.)]\s+").unwrap());
    static RE_HR: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^[-*_]{3,}\s*$").unwrap());
    static RE_HTML_TAG: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"</?[a-zA-Z][^>]*>").unwrap());
    // TTS 控制标记 [EMO:...]/[THINKING]/[SPEED:...]/[PAUSE:...]——绝不朗读
    static RE_TTS_CONTROLS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\[(?:EMO|THINKING|SPEED|PAUSE)(?::[^\]]*)?\]").unwrap()
    });

    let s = text;
    let s = RE_CODE_BLOCK.replace_all(&s, "");
    let s = RE_BOLD.replace_all(&s, "$1");
    let s = RE_BOLD_UNDER.replace_all(&s, "$1");
    let s = RE_ITALIC.replace_all(&s, "$1");
    let s = RE_STRIKE.replace_all(&s, "$1");
    let s = RE_INLINE_CODE.replace_all(&s, "$1");
    let s = RE_IMAGE.replace_all(&s, "$1");
    let s = RE_LINK.replace_all(&s, "$1");
    let s = RE_HEADER.replace_all(&s, "");
    let s = RE_BLOCKQUOTE.replace_all(&s, "");
    let s = RE_UL_LIST.replace_all(&s, "");
    let s = RE_OL_LIST.replace_all(&s, "");
    let s = RE_HR.replace_all(&s, "");
    let s = RE_HTML_TAG.replace_all(&s, "");
    let s = RE_TTS_CONTROLS.replace_all(&s, "");

    // 残留下划线替换为空格，避免 TTS 朗读出"下划线"
    let s = s.replace('_', " ");

    // 清洗后可能残留多余空行/空格，压缩为单个空格
    let s = s.replace('\n', " ");
    let s: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s
}

/// 解析 TTS 控制标记（书中 9.7：把"在哪里停顿/用什么语气"的决策权交给主 LLM）。
///
/// 支持标记：
/// - `[THINKING]`：思考停顿，默认 ~500ms
/// - `[EMO:xxx]`：情绪标记（与现有 expression 系统冗余，此处仅剥离）
/// - `[SPEED:0.9]`：语速倍率覆盖
/// - `[PAUSE:800]`：自定义停顿毫秒
///
/// 返回剥离标记后的可朗读文本与提取到的停顿/语速控制。
pub struct TtsControl {
    /// 剥离标记后的可朗读文本
    pub text: String,
    /// 提取到的语速倍率覆盖（如 0.9）
    pub speed: Option<f64>,
    /// 提取到的停顿毫秒（THINKING 默认 500，PAUSE 取显式值）
    pub pause_ms: u64,
}

pub fn parse_tts_controls(text: &str) -> TtsControl {
    use once_cell::sync::Lazy;
    use regex::Regex;

    const THINKING_PAUSE_MS: u64 = 500;

    static RE_SPEED: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\[SPEED:\s*([0-9]*\.?[0-9]+)\s*\]").unwrap());
    static RE_PAUSE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\[PAUSE:\s*([0-9]+)\s*\]").unwrap());
    static RE_THINKING: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\[THINKING\]").unwrap());
    static RE_ALL: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\[(?:EMO|THINKING|SPEED|PAUSE)(?::[^\]]*)?\]").unwrap()
    });

    let speed = RE_SPEED
        .captures(text)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<f64>().ok())
        .filter(|s| *s > 0.0);
    let explicit_pause = RE_PAUSE
        .captures(text)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<u64>().ok());
    let has_thinking = RE_THINKING.is_match(text);
    let pause_ms = explicit_pause
        .or_else(|| has_thinking.then(|| THINKING_PAUSE_MS))
        .unwrap_or(0);

    let cleaned = RE_ALL.replace_all(text, "");
    TtsControl {
        text: cleaned.trim().to_string(),
        speed,
        pause_ms,
    }
}

/// 按句切分文本（TTS 早播放用）
///
/// 切分规则：
/// 1. 换行符（\n/\r）— 段落边界，最高优先级，无条件切分
/// 2. 句末标点（。！？!?）— 句级边界，仅当累积内容超过 MIN_SPLIT_CHARS 时切分
/// 3. 标点保留在被切片段末尾
/// 4. 过短的片段（< 2 字符，不含标点）合并到上一段
/// 5. 空白段落被丢弃
fn split_sentences(text: &str) -> Vec<String> {
    const MIN_SPLIT_CHARS: usize = 16;

    let mut sentences: Vec<String> = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);

        let is_newline = matches!(ch, '\n' | '\r');
        let is_punct_boundary = matches!(ch, '。' | '！' | '？' | '!' | '?');

        if is_newline || is_punct_boundary {
            let trimmed = current.trim().to_string();
            let content = trimmed.trim_end_matches(|c: char| c == '\n' || c == '\r').to_string();

            if is_punct_boundary && !is_newline && content.chars().count() <= MIN_SPLIT_CHARS {
                continue;
            }

            if !content.is_empty() {
                if content.chars().count() < 2 && !sentences.is_empty() {
                    sentences.last_mut().unwrap().push_str(&content);
                } else {
                    sentences.push(content);
                }
            }
            current.clear();
        }
    }

    let remaining = current.trim().to_string();
    if !remaining.is_empty() {
        let content = remaining.trim_end_matches(|c: char| c == '\n' || c == '\r').to_string();
        if !content.is_empty() {
            if content.chars().count() < 2 && !sentences.is_empty() {
                sentences.last_mut().unwrap().push_str(&content);
            } else {
                sentences.push(content);
            }
        }
    }

    if sentences.is_empty() {
        sentences.push(text.trim().to_string());
    }
    sentences
}

/// MCI 设置音量(Windows 专用)
#[cfg(windows)]
fn mci_set_volume(alias: &str, volume: u32) -> VivianResult<()> {
    use windows::core::PCWSTR;
    use windows::Win32::Media::Multimedia::mciSendStringW;
    let cmd = format!("setaudio {} volume to {}", alias, volume);
    let wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = [0u16; 256];
    let result = unsafe {
        mciSendStringW(PCWSTR(wide.as_ptr()), Some(&mut buf), None)
    };
    if result != 0 {
        return Err(VivianError::Speech(format!(
            "MCI 设置音量失败 [{}]",
            result
        )));
    }
    Ok(())
}
