// TypeScript 类型定义 - 与 Rust 后端结构对齐

/** AI 响应 - 对应 backend `AiResponse` */
export interface AiResponse {
  text: string;
  motion: string;
  expression: string;
  emotion_score: number;
  execution_result?: string | null;
  source?: string;
  importance_user?: number;
  importance_ai?: number;
  sticker?: string;
}

/** 链接卡片 - 类似微信分享链接预览 */
export interface LinkCard {
  url: string;
  title: string;
  description?: string;
  source?: string;
}

/** 聊天消息 - 对应 backend `ChatMessage` */
export interface ChatMessage {
  role: 'user' | 'assistant' | 'system';
  content: string;
  timestamp?: string;
  /** 链接卡片（AI分享网页时渲染） */
  linkCard?: LinkCard;
  /** 元数据（对应后端metadata） */
  metadata?: Record<string, any>;
}

/** 记忆类型 - 对应 backend `MemoryType` (snake_case) */
export type MemoryType =
  | 'short_term'
  | 'mid_term'
  | 'long_term'
  | 'user'
  | 'feedback'
  | 'project'
  | 'reference'
  | 'general'
  | 'preference'
  | 'identity'
  | 'important_event'
  | 'knowledge'
  | 'temporary_context'
  | 'casual_conversation'
  | 'observation_note'
  | 'session_summary'
  | 'insight'
  | 'inner_monologue'
  | 'event'
  | 'skill'
  | 'emotional'
  | 'contextual'
  | 'seed';

/** 记忆条目 - 对应 backend `MemoryItem` */
export interface MemoryItem {
  id: string;
  content: string;
  memory_type: MemoryType;
  importance: number;
  tags: string[];
  created_at: string;
  last_accessed: string;
  access_count: number;
  embedding?: number[] | null;
  expiration?: string | null;
  source: string;
  metadata?: Record<string, unknown> | null;
  /** 后端部分场景返回的统一时间戳（Unix 秒/毫秒或 ISO 字符串），缺失时回退到 created_at */
  timestamp?: number | string;
  /** 后端会话封包 ID（seal 时回填），与 metadata.session_id 互补 */
  episode_id?: string | null;
}

// ====================================================================
// 四层记忆架构 - 新增数据层类型
// ====================================================================

/** 统一事件账本 - 事件可见性分级 */
export type EventVisibility =
  | { Public: [] }
  | { Participants: [] }
  | { Private: [string] }; // owner_id

/** 统一事件账本条目 */
export interface UnifiedEvent {
  id: string;
  timestamp: number;
  sender: string;
  receiver: string;
  event_type: string; // "dialogue" | "observer_note" | ...
  content_preview: string;
  context_tags: string[];
  visibility: EventVisibility;
}

/** 共享世界知识分类 */
export type WorldFactCategory =
  | 'UserPreference'
  | 'HouseRule'
  | 'Environment'
  | 'SharedEvent';

/** 共享世界知识条目 */
export interface WorldFact {
  id: string;
  fact_text: string;
  category: WorldFactCategory;
  importance: number;
  contributors: string[];
  source_event_ids: string[];
  created_at: number;
  last_reinforced_at: number;
  reinforcement_count: number;
}

/** 关系认知事实分类 */
export type RelationshipFactCategory =
  | 'Personality'
  | 'Preference'
  | 'Habit'
  | 'Incident';

/** 关系认知事实（A 对 B 的认知） */
export interface RelationshipFact {
  id: string;
  owner_agent: string;
  target_agent: string;
  fact_text: string;
  category: RelationshipFactCategory;
  confidence: number;
  source_event_ids: string[];
  created_at: number;
  last_reinforced_at: number;
  reinforcement_count: number;
}

/** 记忆管理面板数据层 */
export type MemoryDataSource =
  | 'memory' // 角色私有记忆（已有）
  | 'events' // 统一事件账本
  | 'world' // 共享世界知识
  | 'facts'; // 关系认知事实

// ====================================================================
// Mind Inspector - 心智观察器数据结构
// ====================================================================

/** 实时心智快照 - 对应后端 get_mind_state */
export interface MindState {
  character_id: string;
  character_name: string;
  attention_top: Array<{ entity: string; weight: number }>;
  goals: Array<{ id: string; description: string; priority: number; active: boolean }>;
  cognition_mode: 'regular' | 'focus' | 'true_name';
  focus_charge: number;
  current_thought: string;
}

/** 世界快照 - 对应后端 WorldSnapshot */
export interface WorldSnapshotView {
  timestamp: number;
  local_time: string;
  hour: number;
  weekday: number;
  is_weekend: boolean;
  season: string;
  solar_term: string | null;
  festival: string | null;
  sunrise_sunset: {
    sunrise_hour: number;
    sunset_hour: number;
    is_daytime: boolean;
  } | null;
  location: {
    latitude: number;
    longitude: number;
    city: string | null;
    region: string | null;
    country: string | null;
  } | null;
  weather: {
    temperature: number | null;
    description: string;
    weather_source: string;
    weather_code?: number;
    humidity?: number;
    wind_speed?: number;
  } | null;
  music: {
    title: string;
    artist: string;
    status: string;
    source: string;
  } | null;
  system: {
    cpu_usage: number;
    memory_total: number;
    memory_used: number;
    memory_usage_pct: number;
    net_download_bps: number;
    net_upload_bps: number;
  } | null;
  volume: {
    level: number;
    muted: boolean;
    device_name: string | null;
  } | null;
  network_status: {
    connected: boolean;
    name: string | null;
    interface_type: string | null;
  } | null;
  foreground_window: {
    title: string;
    process: string;
    pid: number;
  } | null;
  user_presence: {
    presence: 'present' | 'away';
    away_since: number | null;
    away_elapsed_secs: number;
    expected_return: {
      min_secs: number;
      max_secs: number;
      source: { type: string };
    } | null;
    last_active_at: number;
    current_activity: {
      label: string;
      started_at: number;
      confidence: number;
    } | null;
  } | null;
  seconds_since_last_interaction: number | null;
}

/** 研究任务状态 - 对应后端 TaskStatus */
export type TaskStatus = 'Active' | 'Paused' | 'Concluded';

/** 样本视图 - 对应后端 SampleView */
export interface SampleView {
  observation: string;
  data: Record<string, unknown> | null;
  source_text: string;
  timestamp: number;
}

/** 结论 - 对应后端 Conclusion */
export interface ConclusionView {
  summary: string;
  confidence: number;
  sample_count: number;
  mean_time: string | null;
  mean_duration: string | null;
  concluded_at: number;
}

/** 研究任务视图 - 对应后端 ResearchTaskView */
export interface ResearchTaskView {
  id: string;
  target: string;
  status: TaskStatus;
  samples: SampleView[];
  conclusion: ConclusionView | null;
  created_at: number;
  updated_at: number;
}

/** 世界快照响应 - 对应后端 get_world_snapshot */
export interface WorldSnapshotResponse {
  snapshot: WorldSnapshotView;
  research: ResearchTaskView[];
  /** 用户行为日志（最近 50 条，按时间降序） */
  behaviors?: UserBehaviorEntryView[];
  /** 用户认知 Belief（subject="user" 且未取代的，按 confidence 降序） */
  user_beliefs?: BeliefView[];
}

/** 用户行为事件 - 对应后端 UserBehaviorEntry */
export interface UserBehaviorEntryView {
  id: string;
  activity_label: string;
  started_at: number;
  ended_at: number;
  duration_secs: number;
  source: string;
  ended_by: 'UserReturn' | 'StateChange' | 'SystemClear' | 'Override';
  confidence: number;
}

/** 信念类别 - 对应后端 BeliefCategory */
export type BeliefCategory =
  | 'Trait'
  | 'Habit'
  | 'Preference'
  | 'State'
  | 'Relationship';

/** 信念状态 - 对应后端 BeliefStatus */
export type BeliefStatus = 'Stable' | 'Questioning' | 'Superseded';

/** 信念条目 - 对应后端 Belief */
export interface BeliefView {
  id: string;
  statement: string;
  subject: string;
  category: BeliefCategory;
  confidence: number;
  source_memory_ids: string[];
  source_episode_ids: string[];
  created_at: number;
  last_reinforced_at: number;
  reinforcement_count: number;
  contradiction_count?: number;
  status?: BeliefStatus;
  metric?: string | null;
  value?: number | null;
  match_labels?: string[];
  superseded_by?: string | null;
}

/** 内部情绪指标 */
export interface EmotionMetrics {
  joy: number;
  sadness: number;
  anger: number;
  fear: number;
  anxiety: number;
  surprise: number;
  curiosity: number;
  affection: number;
  loneliness: number;
  trust: number;
}

/** 需求驱动指标 */
export interface NeedMetrics {
  social: number;
  companionship: number;
  expression: number;
  curiosity: number;
  achievement: number;
  security: number;
  energy: number;
  novelty: number;
}

/** 认知状态指标 */
export interface CognitiveMetrics {
  attention: number;
  thinking: boolean;
  memory_load: number;
  confidence: number;
  uncertainty: number;
}

/** 关系指标 */
export interface RelationshipMetrics {
  intimacy: number;
  trust: number;
  dependency: number;
  comfort: number;
  respect: number;
}

/** 人格基线 */
export interface PersonalityTraits {
  extraversion: number;
  empathy: number;
  stability: number;
  curiosity: number;
  responsibility: number;
}

/** 心情状态 - 对应 backend `MoodState` */
export interface MoodState {
  mood_label: string;
  mood_score: number;
  mood_emotion: string;
  mood_secondary: string;
  emotion_metrics: EmotionMetrics;
  need_metrics: NeedMetrics;
  cognitive_metrics: CognitiveMetrics;
  relationship_metrics: RelationshipMetrics;
  personality_traits: PersonalityTraits;
  primary_emotion: string;
  secondary_emotion: string;
  intimacy: number;
  trust: number;
  energy: number;
  stress: number;
  focus: number;
  valence: number;
  arousal: number;
  positive_affect: number;
  negative_affect: number;
  fatigue?: number;
  relationship_score?: number;
}

/** 工具信息 - 对应 backend tool schema */
export interface ToolInfo {
  name: string;
  description: string;
  category: string;
  parameters_schema: Record<string, unknown>;
  is_read_only?: boolean;
}

/** 系统信息 - 对应 backend `get_system_info` 返回 */
export interface SystemInfo {
  cpu_usage: number;
  memory_usage: number;
  cpu_count: number;
  total_memory: number;
  used_memory?: number;
  available_memory?: number;
  uptime?: number;
  host_name?: string;
  os_name?: string;
  os_version?: string;
}

export type ModelKind = 'live2d' | 'mmd' | 'vrm' | 'pngtuber';

/** 模型信息 - 对应 backend `get_model_info` 返回 */
export interface ModelInfo {
  model_name: string;
  version: string;
  expressions: string[];
  motions: string[];
  model_kind?: string;
  display_scale?: number;
}

/** 流式聊天事件载荷 */
export interface ChatChunkPayload {
  text: string;
}

export interface ChatDonePayload {
  text: string;
  motion: string;
  expression: string;
  emotion_score: number;
}

export interface ChatErrorPayload {
  error: string;
}

/** 应用配置根结构（部分关键字段） */
export interface AppConfig {
  base: {
    model_path: string;
    language: string;
  };
  window: {
    width: number;
    height: number;
    title: string;
    max_size: [number, number];
  };
  ai: {
    provider: string;
    model: string;
    temperature: number;
    max_tokens: number;
  };
  [key: string]: unknown;
}

/** TTS 引擎 - 对应 backend `TtsEngine` */
export type TtsEngine = 'none' | 'edgetts' | 'azure' | 'gptsovits' | 'fishspeech' | 'bertvits2' | 'minimax' | 'doubao';

/** TTS 配置 - 对应 backend `TtsConfig` */
export interface TtsConfig {
  enabled: boolean;
  rate: number;
  volume: number;
  voice_id?: string | null;
  engine: TtsEngine;
  fallback_engine?: TtsEngine | null;
  retry_count?: number;
  // Azure
  azure_key?: string | null;
  azure_region?: string | null;
  azure_style?: string | null;
  azure_style_degree?: number | null;
  azure_role?: string | null;
  azure_pitch?: number | null;
  azure_output_format?: string | null;
  // GPT-SoVITS
  gpt_sovits_url?: string | null;
  gpt_sovits_install_path?: string | null;
  gpt_sovits_config_path?: string | null;
  gpt_sovits_gpt_model?: string | null;
  gpt_sovits_sovits_model?: string | null;
  gpt_sovits_gpu?: number | null;
  gpt_sovits_port?: number | null;
  gpt_sovits_python_path?: string | null;
  gpt_sovits_ref_audio?: string | null;
  gpt_sovits_prompt_text?: string | null;
  gpt_sovits_prompt_lang?: string | null;
  gpt_sovits_aux_ref_audios?: string[] | null;
  gpt_sovits_parallel_infer?: boolean | null;
  gpt_sovits_text_split_method?: string | null;
  gpt_sovits_top_k?: number | null;
  gpt_sovits_top_p?: number | null;
  gpt_sovits_temperature?: number | null;
  gpt_sovits_auto_start?: boolean;
  gpt_sovits_dual_instance?: boolean;
  gpt_sovits_second_port?: number | null;
  // Fish Speech
  fish_speech_url?: string | null;
  fish_speech_key?: string | null;
  fish_speech_character?: string | null;
  fish_speech_format?: string | null;
  fish_speech_ref_audio?: string | null;
  fish_speech_ref_text?: string | null;
  // Fish Speech 本地服务管理（一键启动）
  fish_speech_install_path?: string | null;
  fish_speech_python_path?: string | null;
  fish_speech_port?: number | null;
  fish_speech_auto_start?: boolean;
  fish_speech_llama_checkpoint_path?: string | null;
  fish_speech_decoder_checkpoint_path?: string | null;
  fish_speech_half?: boolean;
  fish_speech_compile?: boolean;
  // MiniMax
  minimax_key?: string | null;
  minimax_voice_id?: string | null;
  minimax_model?: string | null;
  minimax_format?: string | null;
  minimax_sample_rate?: number | null;
  // 豆包(火山引擎)
  doubao_appid?: string | null;
  doubao_access_token?: string | null;
  doubao_cluster?: string | null;
  doubao_voice_type?: string | null;
  doubao_format?: string | null;
  doubao_sample_rate?: number | null;
}

/** GPT-SoVITS 服务运行状态 - 对应 backend `ServiceState` */
export type GptSoVitsServiceStatus =
  | 'stopped'
  | 'starting'
  | 'running'
  | 'stopping'
  | 'crashed';

export interface GptSoVitsServiceState {
  status: GptSoVitsServiceStatus;
  pid?: number | null;
  error?: string | null;
  command_line?: string | null;
  endpoint?: string | null;
  instances?: Array<{
    port: number;
    status: GptSoVitsServiceStatus;
    pid?: number | null;
    endpoint: string;
    error?: string | null;
  }>;
  dual_instance?: boolean;
}

/** Whisper 本地 ASR 服务状态 - 对应 backend `WhisperServiceState` */
export type WhisperServiceStatus =
  | 'stopped'
  | 'starting'
  | 'running'
  | 'stopping'
  | 'crashed';

export interface WhisperServiceState {
  status: WhisperServiceStatus;
  pid?: number | null;
  error?: string | null;
  command_line?: string | null;
  endpoint?: string | null;
  port?: number | null;
}

/** Fish Speech 本地 TTS 服务状态 */
export type FishSpeechServiceStatus =
  | 'stopped'
  | 'starting'
  | 'running'
  | 'stopping'
  | 'crashed';

export interface FishSpeechServiceState {
  status: FishSpeechServiceStatus;
  pid?: number | null;
  error?: string | null;
  command_line?: string | null;
  endpoint?: string | null;
  port?: number | null;
}

export type OllamaServiceStatus =
  | 'stopped'
  | 'starting'
  | 'running'
  | 'stopping'
  | 'crashed';

/** Ollama 本地嵌入服务状态 - 对应 backend `OllamaServiceState` */
export interface OllamaServiceState {
  status: OllamaServiceStatus;
  pid?: number | null;
  error?: string | null;
  endpoint?: string | null;
}

/** 关系信息 - 对应 backend `get_relationship` 返回 */
export interface RelationshipInfo {
  stage: string;
  intimacy: number;
  trust: number;
  respect: number;
  dependency: number;
  familiarity: number;
  interaction_count: number;
  consecutive_positive: number;
  consecutive_negative: number;
  permanent_stage: string;
  temporary_stage?: string | null;
  effective_stage_label: string;
  last_interaction_time: number;
  milestones: Array<{
    description: string;
    intimacy: number;
    timestamp: string;
  }>;
}

/** 主动对话 tick 上下文 - 传给 `proactive_tick` */
export interface ProactiveTickContext {
  idle_seconds: number;
  away_seconds: number;
  user_present: boolean;
  interaction_count_today: number;
  active_window: string;
  window_changed: boolean;
  last_topic_relevant: boolean;
  has_relevant_memory: boolean;
  drag_distance: number;
  user_emotion: string;
}

/** 主动对话消息 */
export interface ProactiveMessage {
  /** 消息文本内容（对应后端 PendingMessage.content） */
  content: string;
  trigger: string;
  timestamp: number;
  priority: number;
  /** 投递渠道：bubble=桌宠气泡，chat_window=微信风格聊天窗口 */
  delivery_channel?: 'bubble' | 'chat_window';
}

/** 主动对话 tick 响应 */
export interface ProactiveTickResponse {
  produced: boolean;
  messages: ProactiveMessage[];
  /** LLM 推荐的下次 tick 间隔（毫秒），后端 adaptive_tick 启用时根据空闲时间动态调整 */
  recommended_next_interval_ms?: number;
  /** 跨角色冷却时长（毫秒），后端按角色 reluctance 差异化下发 */
  effective_cross_cooldown_ms?: number;
}

/** 环境信息 */
export interface EnvironmentInfo {
  active_window: string;
  mouse_position: [number, number];
  system_time: string;
  battery?: {
    is_charging: boolean;
    percent: number;
  } | null;
}

/** 用户活动状态 */
export interface UserActivity {
  idle_seconds: number;
  away_seconds: number;
  user_present: boolean;
  last_input_time: number;
}

/** 启动问候响应 */
export interface StartupGreeting {
  greeting: string;
  /** LLM 调用失败时的错误信息（greeting 为空时可能携带） */
  error?: string | null;
}
