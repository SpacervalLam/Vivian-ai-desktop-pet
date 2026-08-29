import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import ReactDOM from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { platform, version as osVersion, arch } from '@tauri-apps/plugin-os';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { emit, listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { open as openShell } from '@tauri-apps/plugin-shell';
import { useTranslation } from 'react-i18next';
import { changeLanguage } from '../i18n';
import { getCharacterId } from '../characterContext';
import TtsHelpDrawer, { TtsBackendKey } from './TtsHelpDrawer';
import AsrHelpDrawer, { AsrBackendKey } from './AsrHelpDrawer';
import ShortcutRecorder, { type ConflictResult, formatForDisplay } from './ShortcutRecorder';
import ClearConfirmDialog from './ClearConfirmDialog';
import SetupGuideModal from './SetupGuideModal';
import type { FishSpeechServiceState, GptSoVitsServiceState, GptSoVitsServiceStatus, OllamaServiceState, WhisperServiceState } from '../types';
import PluginsPanel from './plugins/PluginsPanel';
import BrowserPanel from './BrowserPanel';
import { Settings, Cpu, Wrench, Database, Mic, Wifi, Compass, Puzzle, Info, Trash2, Sparkles, ExternalLink } from 'lucide-react';

type TabKey =
  | 'general'
  | 'ai'
  | 'tools'
  | 'memory'
  | 'voice'
  | 'network'
  | 'browser'
  | 'plugins'
  | 'about';

interface TtsConfigState {
  enabled: boolean;
  rate: number;
  volume: number;
  voice_id: string | null;
  engine: 'none' | 'edgetts' | 'azure' | 'gptsovits' | 'fishspeech' | 'bertvits2' | 'minimax' | 'doubao' | 'mimo';
  fallback_engine: 'none' | 'edgetts' | 'azure' | 'gptsovits' | 'fishspeech' | 'bertvits2' | 'minimax' | 'doubao' | 'mimo' | null;
  retry_count: number;
  // Azure
  azure_key: string | null;
  azure_region: string | null;
  azure_style: string | null;
  azure_style_degree: number | null;
  azure_role: string | null;
  azure_pitch: number | null;
  azure_output_format: string | null;
  // GPT-SoVITS
  gpt_sovits_url: string | null;
  gpt_sovits_install_path: string | null;
  gpt_sovits_config_path: string | null;
  gpt_sovits_gpt_model: string | null;
  gpt_sovits_sovits_model: string | null;
  gpt_sovits_gpu: number | null;
  gpt_sovits_port: number | null;
  gpt_sovits_python_path: string | null;
  gpt_sovits_ref_audio: string | null;
  gpt_sovits_prompt_text: string | null;
  gpt_sovits_prompt_lang: string | null;
  gpt_sovits_aux_ref_audios: string[] | null;
  gpt_sovits_parallel_infer: boolean | null;
  gpt_sovits_text_split_method: string | null;
  gpt_sovits_top_k: number | null;
  gpt_sovits_top_p: number | null;
  gpt_sovits_temperature: number | null;
  gpt_sovits_auto_start: boolean;
  gpt_sovits_dual_instance: boolean;
  gpt_sovits_second_port: number | null;
  // 跨语言 TTS
  display_language: string | null;
  tts_language: string | null;
  translation_provider: string | null;
  translation_api_key: string | null;
  translation_endpoint: string | null;
  // Fish Speech
  fish_speech_url: string | null;
  fish_speech_key: string | null;
  fish_speech_character: string | null;
  fish_speech_format: string | null;
  fish_speech_ref_audio: string | null;
  fish_speech_ref_text: string | null;
  // Fish Speech 本地服务管理（一键启动）
  fish_speech_install_path: string | null;
  fish_speech_python_path: string | null;
  fish_speech_port: number | null;
  fish_speech_auto_start: boolean;
  fish_speech_llama_checkpoint_path: string | null;
  fish_speech_decoder_checkpoint_path: string | null;
  fish_speech_half: boolean;
  fish_speech_compile: boolean;
  // MiniMax
  minimax_key: string | null;
  minimax_voice_id: string | null;
  minimax_model: string | null;
  minimax_format: string | null;
  minimax_sample_rate: number | null;
  // 豆包(火山引擎)
  doubao_appid: string | null;
  doubao_access_token: string | null;
  doubao_cluster: string | null;
  doubao_voice_type: string | null;
  doubao_format: string | null;
  doubao_sample_rate: number | null;
  // 小米 MiMo（语音克隆）
  mimo_key: string | null;
  mimo_voice_audio_path: string | null;
  mimo_model: string | null;
  mimo_endpoint: string | null;
  mimo_style_prompt: string | null;
}

/**
 * 日记配置状态 - 对齐后端 `diary::DiaryConfig`
 * 字段：enable_auto_diary / min_interaction_threshold / max_diary_length
 */
interface DiaryConfigState {
  enable_auto_diary: boolean;
  min_interaction_threshold: number;
  max_diary_length: number;
}

const tabs: { key: TabKey; labelKey: string; icon: React.ElementType }[] = [
  { key: 'general', labelKey: 'config.tab_general', icon: Settings },
  { key: 'ai', labelKey: 'config.tab_ai', icon: Cpu },
  { key: 'tools', labelKey: 'config.tab_tools', icon: Wrench },
  { key: 'memory', labelKey: 'config.tab_memory', icon: Database },
  { key: 'voice', labelKey: 'config.tab_voice', icon: Mic },
  { key: 'network', labelKey: 'config.tab_network', icon: Wifi },
  { key: 'browser', labelKey: 'config.tab_browser', icon: Compass },
  { key: 'plugins', labelKey: 'config.tab_plugins', icon: Puzzle },
  { key: 'about', labelKey: 'config.tab_about', icon: Info },
];

type ConfigValue = string | number | boolean | ConfigObject | string[] | null;
interface ConfigObject {
  [key: string]: ConfigValue;
}

/**
 * 路由矩阵任务定义 - 14 个真实启用的任务，每个任务独立配置完整模型
 *
 * 任务职责说明：
 * - chat:                日常对话与问答（高频，人格核心，可用便宜模型）
 * - reasoning:           长输入/工具调用/动作决策深度推理（自动从 chat 升级，需强模型）
 * - vision_describe:     图片理解（用户发图时使用，必须配置支持视觉的多模态模型）
 * - diary:               智能日记内容生成
 * - memory:              写入时记忆抽取（enrich：关键词/重要性/语义类型分类，高频，建议便宜模型，并供 LLM 记忆路由/校验/用户画像复用）
 * - consolidation:       离线记忆巩固与精修（三阶段流水线、相似记忆精修、冲突仲裁，低频，需深度推理模型）
 * - reflection:          异步反思（每5轮或30分钟触发，合并意识更新与活动抽取，fire-and-forget，失败静默）
 * - inner_monologue:     离线内心独白（用户不交互时自主思考，含兴趣话题联网搜索，建议廉价快速模型）
 * - emotion_analysis:    情绪分类（用户/角色情绪效价与唤醒度，LLM 分类器，建议便宜快速模型）
 * - knowledge_acquisition: 空闲时知识搜索学习（后台低频，建议便宜模型）
 * - translation:         跨语言 TTS 文本翻译（仅翻译服务选 LLM 时使用，简单任务，便宜模型即可）
 * - bystander_judge:     旁观插话判断（用户对话时轻量判断旁观者是否插话，建议便宜快速模型）
 * - intent_judge:        会话关闭意图判断（每轮对话后判断是否应关闭及关闭原因，建议便宜快速模型）
 * - asr_polish:          语音识别结果整理（识别结束后修正同音字/语气词/标点，建议便宜快速模型）
 */
const ROUTING_TASKS: { labelKey: string; taskType: string; helpKey: string }[] = [
  { labelKey: 'config.routing_chat', taskType: 'chat', helpKey: 'config.routing_chat_help' },
  { labelKey: 'config.routing_reasoning', taskType: 'reasoning', helpKey: 'config.routing_reasoning_help' },
  { labelKey: 'config.routing_vision_describe', taskType: 'vision_describe', helpKey: 'config.routing_vision_describe_help' },
  { labelKey: 'config.routing_diary', taskType: 'diary', helpKey: 'config.routing_diary_help' },
  { labelKey: 'config.routing_memory', taskType: 'memory', helpKey: 'config.routing_memory_help' },
  { labelKey: 'config.routing_consolidation', taskType: 'consolidation', helpKey: 'config.routing_consolidation_help' },
  { labelKey: 'config.routing_reflection', taskType: 'reflection', helpKey: 'config.routing_reflection_help' },
  { labelKey: 'config.routing_inner_monologue', taskType: 'inner_monologue', helpKey: 'config.routing_inner_monologue_help' },
  { labelKey: 'config.routing_emotion_analysis', taskType: 'emotion_analysis', helpKey: 'config.routing_emotion_analysis_help' },
  { labelKey: 'config.routing_knowledge_acquisition', taskType: 'knowledge_acquisition', helpKey: 'config.routing_knowledge_acquisition_help' },
  { labelKey: 'config.routing_translation', taskType: 'translation', helpKey: 'config.routing_translation_help' },
  { labelKey: 'config.routing_bystander_judge', taskType: 'bystander_judge', helpKey: 'config.routing_bystander_judge_help' },
  { labelKey: 'config.routing_intent_judge', taskType: 'intent_judge', helpKey: 'config.routing_intent_judge_help' },
  { labelKey: 'config.routing_asr_polish', taskType: 'asr_polish', helpKey: 'config.routing_asr_polish_help' },
];

/**
 * 服务商预设 - 选中后自动填充 provider_type / endpoint / 默认 model
 *
 * 数据来源：2026-07 各服务商官方 API 文档实测
 * - OpenAI: https://api.openai.com/v1
 * - Anthropic: https://api.anthropic.com（原生 /v1/messages，非 OpenAI 兼容）
 * - Gemini: https://generativelanguage.googleapis.com（原生 REST）
 * - DeepSeek: https://api.deepseek.com（官方文档 base_url；/v1 仅为 OpenAI SDK 兼容后缀，
 *             与模型版本无关，两种写法均可用，这里取官方文档写法）
 * - 通义千问 Qwen: DashScope OpenAI 兼容模式 https://dashscope.aliyuncs.com/compatible-mode/v1
 * - 智谱 GLM: https://open.bigmodel.cn/api/paas/v4（OpenAI 兼容）
 * - Moonshot Kimi: https://api.moonshot.cn/v1（OpenAI 兼容）
 * - 豆包 Doubao: 火山方舟 https://ark.cn-beijing.volces.com/api/v3（OpenAI 兼容）
 * - SiliconFlow: https://api.siliconflow.cn/v1（OpenAI 兼容）
 * - 文心一言: https://aip.baidubce.com（原生 OAuth + access_token）
 *
 * 注：讯飞星火因 WebSocket + HMAC 鉴权复杂且预设实用性低，未提供预设；
 *     用户仍可通过手动选择 provider=spark 进行配置。
 */
interface ProviderPreset {
  /** 稳定标识，用作 provider_cache 的 key（不随 i18n 变化） */
  id: string;
  /** 下拉显示用的服务商名称 i18n key（仅服务商名，不含模型名） */
  labelKey: string;
  providerType: string;
  endpoint: string;
  defaultModel: string;
  /** 模型名输入建议列表（datalist 下拉建议，仍可自由输入） */
  mainModels: string[];
  /** 该预设的上下文窗口（tokens），用于自动压缩阈值判定 */
  contextWindow?: number;
  /** 该厂商的建议单次输出上限（tokens），切换主 LLM 预设时自动填入 max_tokens */
  suggestedMaxTokens?: number;
  /** 是否需要 api_secret（文心等 OAuth/HMAC 鉴权） */
  needsSecret?: boolean;
  /** 是否需要 app_id */
  needsAppId?: boolean;
  /** 供应商 API 控制台/官网（获取 API Key 的页面），有值时显示跳转按钮 */
  consoleUrl?: string;
  /** 该厂商支持的 API 协议变体（≥2 时显示协议选择器）。
   *
   *  每项是 (provider_type, endpoint) 组合；缺省时仅有默认
   *  providerType + endpoint 一种（无选择器）。切换协议只覆盖
   *  provider_type 与 endpoint，不动 model / api_key。 */
  protocols?: ProviderProtocol[];
}

/** 单个协议变体：后端 provider_type + 该协议的端点 */
interface ProviderProtocol {
  /** 后端 provider_type 值（openai / chat_completions / anthropic / …） */
  providerType: string;
  /** 协议显示名 i18n key */
  labelKey: string;
  /** 该协议的接口端点 */
  endpoint: string;
}

/**
 * 预设是否包含指定的 (provider_type, endpoint) 组合 ——
 * 匹配默认协议或任一协议变体。endpoint 为空时仅按 provider_type 匹配。
 */
const presetMatches = (p: ProviderPreset, type: string, endpoint: string): boolean =>
  (p.providerType === type && (endpoint === '' || p.endpoint === endpoint)) ||
  (p.protocols ?? []).some(
    (pr) => pr.providerType === type && (endpoint === '' || pr.endpoint === endpoint),
  );

const PROVIDER_PRESETS: ProviderPreset[] = [
  { id: 'openai', labelKey: 'config.preset_openai', providerType: 'openai', endpoint: 'https://api.openai.com/v1', defaultModel: 'gpt-5.5', mainModels: ['gpt-5.5', 'gpt-5.6', 'gpt-5', 'o3', 'o4-mini'], contextWindow: 400_000, suggestedMaxTokens: 32768, consoleUrl: 'https://platform.openai.com/api-keys', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.openai.com/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.openai.com/v1' },
  ] },
  { id: 'anthropic', labelKey: 'config.preset_anthropic', providerType: 'anthropic', endpoint: 'https://api.anthropic.com', defaultModel: 'claude-sonnet-4', mainModels: ['claude-sonnet-4-6', 'claude-sonnet-4', 'claude-opus-4-8', 'claude-haiku-4'], contextWindow: 200_000, suggestedMaxTokens: 64000, consoleUrl: 'https://console.anthropic.com/settings/keys' },
  { id: 'gemini', labelKey: 'config.preset_gemini', providerType: 'gemini', endpoint: 'https://generativelanguage.googleapis.com', defaultModel: 'gemini-3-pro', mainModels: ['gemini-3-pro', 'gemini-3-flash', 'gemini-2.5-pro'], contextWindow: 1_000_000, suggestedMaxTokens: 65536, consoleUrl: 'https://aistudio.google.com/apikey' },
  { id: 'deepseek', labelKey: 'config.preset_deepseek', providerType: 'openai', endpoint: 'https://api.deepseek.com', defaultModel: 'deepseek-chat', mainModels: ['deepseek-v4-pro', 'deepseek-v4-flash', 'deepseek-chat', 'deepseek-reasoner'], contextWindow: 128_000, suggestedMaxTokens: 8192, consoleUrl: 'https://platform.deepseek.com/api_keys', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.deepseek.com' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.deepseek.com' },
    { providerType: 'anthropic', labelKey: 'config.proto_anthropic', endpoint: 'https://api.deepseek.com/anthropic' },
  ] },
  { id: 'qwen', labelKey: 'config.preset_qwen', providerType: 'openai', endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1', defaultModel: 'qwen3-max', mainModels: ['qwen3-max', 'qwen-plus', 'qwen-turbo', 'qwen3-235b-a22b'], contextWindow: 131_072, suggestedMaxTokens: 32768, consoleUrl: 'https://bailian.console.aliyun.com/?apiKey=1', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1' },
  ] },
  { id: 'glm', labelKey: 'config.preset_glm', providerType: 'zhipu', endpoint: 'https://open.bigmodel.cn/api/paas/v4', defaultModel: 'glm-5', mainModels: ['glm-5.3', 'glm-5.2', 'glm-5', 'glm-5-turbo', 'glm-4.7'], contextWindow: 131_072, suggestedMaxTokens: 32768, consoleUrl: 'https://open.bigmodel.cn/usercenter/apikeys', protocols: [
    { providerType: 'zhipu', labelKey: 'config.proto_zhipu', endpoint: 'https://open.bigmodel.cn/api/paas/v4' },
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://open.bigmodel.cn/api/paas/v4' },
    { providerType: 'anthropic', labelKey: 'config.proto_anthropic', endpoint: 'https://open.bigmodel.cn/api/anthropic' },
  ] },
  { id: 'moonshot', labelKey: 'config.preset_moonshot', providerType: 'openai', endpoint: 'https://api.moonshot.cn/v1', defaultModel: 'kimi-k2.6', mainModels: ['kimi-k2.6', 'kimi-k2.5', 'kimi-k2-thinking'], contextWindow: 131_072, suggestedMaxTokens: 16384, consoleUrl: 'https://platform.moonshot.cn/console/api-keys', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.moonshot.cn/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.moonshot.cn/v1' },
  ] },
  { id: 'doubao', labelKey: 'config.preset_doubao', providerType: 'openai', endpoint: 'https://ark.cn-beijing.volces.com/api/v3', defaultModel: 'doubao-seed-1.6', mainModels: ['doubao-seed-2-1-pro-260628', 'doubao-seed-2-0-pro-260215', 'doubao-seed-1.6'], contextWindow: 256_000, suggestedMaxTokens: 16384, consoleUrl: 'https://console.volcengine.com/ark', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://ark.cn-beijing.volces.com/api/v3' },
    { providerType: 'doubao', labelKey: 'config.proto_doubao_responses', endpoint: 'https://ark.cn-beijing.volces.com/api/v3' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://ark.cn-beijing.volces.com/api/v3' },
    { providerType: 'anthropic', labelKey: 'config.proto_anthropic', endpoint: 'https://ark.cn-beijing.volces.com/api/v3/anthropic' },
  ] },
  { id: 'minimax', labelKey: 'config.preset_minimax', providerType: 'openai', endpoint: 'https://api.minimaxi.com/v1', defaultModel: 'MiniMax-M3', mainModels: ['MiniMax-M3', 'MiniMax-M2.7', 'MiniMax-M2.5'], contextWindow: 200_000, suggestedMaxTokens: 16384, consoleUrl: 'https://platform.minimaxi.com/user-center/basic-information/interface-key', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.minimaxi.com/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.minimaxi.com/v1' },
    { providerType: 'anthropic', labelKey: 'config.proto_anthropic', endpoint: 'https://api.minimaxi.com/anthropic' },
  ] },
  { id: 'mimo', labelKey: 'config.preset_mimo', providerType: 'openai', endpoint: 'https://api.xiaomimimo.com/v1', defaultModel: 'mimo-v2.5-pro', mainModels: ['mimo-v2.5-pro', 'mimo-v2.5'], contextWindow: 131_072, suggestedMaxTokens: 16384, consoleUrl: 'https://www.xiaomimimo.com/', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.xiaomimimo.com/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.xiaomimimo.com/v1' },
    { providerType: 'anthropic', labelKey: 'config.proto_anthropic', endpoint: 'https://api.xiaomimimo.com/anthropic' },
  ] },
  { id: 'siliconflow', labelKey: 'config.preset_siliconflow', providerType: 'openai', endpoint: 'https://api.siliconflow.cn/v1', defaultModel: 'deepseek-ai/DeepSeek-V3.1', mainModels: ['deepseek-ai/DeepSeek-V3.1', 'Qwen/Qwen2.5-72B-Instruct'], contextWindow: 131_072, suggestedMaxTokens: 8192, consoleUrl: 'https://cloud.siliconflow.cn/account/ak', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.siliconflow.cn/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.siliconflow.cn/v1' },
  ] },
  { id: 'grok', labelKey: 'config.preset_grok', providerType: 'openai', endpoint: 'https://api.x.ai/v1', defaultModel: 'grok-4.5', mainModels: ['grok-4.5', 'grok-4', 'grok-3'], contextWindow: 131_072, suggestedMaxTokens: 32768, consoleUrl: 'https://console.x.ai', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.x.ai/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.x.ai/v1' },
  ] },
  { id: 'openrouter', labelKey: 'config.preset_openrouter', providerType: 'chat_completions', endpoint: 'https://openrouter.ai/api/v1', defaultModel: 'openai/gpt-4o', mainModels: ['openai/gpt-4o', 'anthropic/claude-sonnet-4', 'deepseek/deepseek-chat'], contextWindow: 131_072, suggestedMaxTokens: 8192, consoleUrl: 'https://openrouter.ai/settings/keys' },
  { id: 'groq', labelKey: 'config.preset_groq', providerType: 'chat_completions', endpoint: 'https://api.groq.com/openai/v1', defaultModel: 'llama-3.3-70b-versatile', mainModels: ['llama-3.3-70b-versatile', 'llama-3.1-8b-instant'], contextWindow: 131_072, suggestedMaxTokens: 8192, consoleUrl: 'https://console.groq.com/keys' },
  { id: 'ollama', labelKey: 'config.preset_ollama', providerType: 'chat_completions', endpoint: 'http://localhost:11434/v1', defaultModel: 'llama3.2', mainModels: ['llama3.2', 'qwen2.5', 'deepseek-r1'], contextWindow: 131_072, suggestedMaxTokens: 8192, consoleUrl: 'https://ollama.com' },
  { id: 'mistral', labelKey: 'config.preset_mistral', providerType: 'chat_completions', endpoint: 'https://api.mistral.ai/v1', defaultModel: 'mistral-large-latest', mainModels: ['mistral-large-latest', 'mistral-small-latest'], contextWindow: 131_072, suggestedMaxTokens: 16384, consoleUrl: 'https://console.mistral.ai/api-keys' },
  { id: 'together', labelKey: 'config.preset_together', providerType: 'chat_completions', endpoint: 'https://api.together.xyz/v1', defaultModel: 'meta-llama/Llama-3.3-70B-Instruct-Turbo', mainModels: ['meta-llama/Llama-3.3-70B-Instruct-Turbo', 'deepseek-ai/DeepSeek-V3'], contextWindow: 131_072, suggestedMaxTokens: 8192, consoleUrl: 'https://api.together.ai/settings/api-keys' },
  { id: 'wenxin', labelKey: 'config.preset_wenxin', providerType: 'wenxin', endpoint: 'https://aip.baidubce.com', defaultModel: 'ernie-4.5-8k-latest', mainModels: ['ernie-4.5-8k-latest', 'ernie-4.0-8k-latest'], needsSecret: true, consoleUrl: 'https://console.bce.baidu.com/iam/#/iam/apikey/list' },
  { id: 'custom', labelKey: 'config.preset_custom', providerType: 'chat_completions', endpoint: '', defaultModel: '', mainModels: [] },
];

/**
 * 厂商 logo 资源映射（public/icons/providers/）
 *
 * 映射缺失的厂商（无 logo 文件）在卡片中以首字母徽标兜底。
 */
const PROVIDER_LOGOS: Record<string, string> = {
  openai: 'icons/providers/openai.svg',
  anthropic: 'icons/providers/claude.svg',
  deepseek: 'icons/providers/deepseek.svg',
  gemini: 'icons/providers/gemini.svg',
  qwen: 'icons/providers/qwen.svg',
  glm: 'icons/providers/glm.svg',
  moonshot: 'icons/providers/kimi.svg',
  doubao: 'icons/providers/volcengine.svg',
  minimax: 'icons/providers/minimax.svg',
  mimo: 'icons/providers/xiaomimimo.svg',
  siliconflow: 'icons/providers/siliconflow.svg',
  grok: 'icons/providers/grok.svg',
  openrouter: 'icons/providers/openrouter.svg',
  groq: 'icons/providers/groq.svg',
  ollama: 'icons/providers/ollama.svg',
  mistral: 'icons/providers/mistral.svg',
  together: 'icons/providers/together.svg',
  wenxin: 'icons/providers/wenxin.svg',
  custom: 'icons/providers/custom-endpoint.svg',
};

/** 当前 provider_type 是否需要 api_secret 字段 */
const needsSecretFor = (providerType: string): boolean => {
  const t = providerType.toLowerCase();
  return t === 'wenxin' || t === 'spark';
};

/** 当前 provider_type 是否需要 app_id 字段 */
const needsAppIdFor = (providerType: string): boolean => {
  const t = providerType.toLowerCase();
  return t === 'spark';
};

/** 预设卡片选中态的贴纸轮换色 */
const STICKER_COLORS = [
  'var(--sticker-pink-soft)',
  'var(--sticker-lilac-soft)',
  'var(--sticker-sky-soft)',
  'var(--sticker-mint-soft)',
  'var(--sticker-butter-soft)',
];

/**
 * 厂商预设卡片横滚行（共享组件）—— 主配置与工作模型选择器复用
 *
 * 隐藏原生横向滚动条，改为左右两侧竖向感应块（矩形块内三角箭头）：
 * 悬停缓慢滚动、按住快速滚动；滚轮在整行任意位置可横向滚动。
 * 直接操作 scrollLeft（requestAnimationFrame 循环），不触发 React 重渲染。
 */
const ProviderPresetRow: React.FC<{
  presets: ProviderPreset[];
  activeId: string;
  onSelect: (presetId: string) => void;
  t: (key: string) => string;
}> = ({ presets, activeId, onSelect, t }) => {
  const scrollRowRef = useRef<HTMLDivElement>(null);
  const zoneLeft = useRef(0); // 0=off / 1=slow / 2=fast
  const zoneRight = useRef(0);
  const rafRef = useRef<number>(0);

  useEffect(() => {
    const tick = () => {
      const el = scrollRowRef.current;
      if (el) {
        if (zoneLeft.current > 0 || zoneRight.current > 0) {
          const fast = zoneLeft.current === 2 || zoneRight.current === 2;
          const speed = fast ? 8 : 1.8;
          let dir = 0;
          if (zoneLeft.current > 0) dir -= 1;
          if (zoneRight.current > 0) dir += 1;
          el.scrollLeft += dir * speed;
        }
      }
      rafRef.current = requestAnimationFrame(tick);
    };
    rafRef.current = requestAnimationFrame(tick);

    // 滚轮：纵向滚轮增量映射为横向滚动，任意位置可用
    const el = scrollRowRef.current;
    const onWheel = (e: WheelEvent) => {
      if (!el || (e.deltaX === 0 && e.deltaY === 0)) return;
      e.preventDefault();
      el.scrollLeft += e.deltaX || e.deltaY;
    };
    el?.addEventListener('wheel', onWheel, { passive: false });

    return () => {
      cancelAnimationFrame(rafRef.current);
      el?.removeEventListener('wheel', onWheel);
    };
  }, []);

  const enterLeft = () => { if (zoneLeft.current < 1) zoneLeft.current = 1; };
  const leaveLeft = () => { zoneLeft.current = 0; };
  const pressLeft = (e: React.MouseEvent) => {
    e.preventDefault();
    zoneLeft.current = 2;
    const up = () => { zoneLeft.current = 1; window.removeEventListener('mouseup', up); };
    window.addEventListener('mouseup', up);
  };
  const enterRight = () => { if (zoneRight.current < 1) zoneRight.current = 1; };
  const leaveRight = () => { zoneRight.current = 0; };
  const pressRight = (e: React.MouseEvent) => {
    e.preventDefault();
    zoneRight.current = 2;
    const up = () => { zoneRight.current = 1; window.removeEventListener('mouseup', up); };
    window.addEventListener('mouseup', up);
  };

  return (
    <>
      <style>
        {`.provider-hscroll{scrollbar-width:none;-ms-overflow-style:none}
.provider-hscroll::-webkit-scrollbar{width:0;height:0;display:none}
.provider-zone{position:absolute;top:0;bottom:0;width:28px;display:flex;align-items:center;justify-content:center;z-index:3;cursor:pointer;user-select:none;background:rgba(112,112,128,0.10);transition:background .15s ease}
.provider-zone::after{content:'';width:0;height:0;border-top:6px solid transparent;border-bottom:6px solid transparent;opacity:.7;transition:opacity .15s ease,transform .15s ease}
.provider-zone-left{left:0;border-radius:8px 0 0 8px}
.provider-zone-left::after{border-right:10px solid var(--panel-text-secondary)}
.provider-zone-right{right:0;border-radius:0 8px 8px 0}
.provider-zone-right::after{border-left:10px solid var(--panel-text-secondary)}
.provider-zone:hover{background:rgba(112,112,128,0.22)}
.provider-zone:hover::after{opacity:1;transform:scale(1.14)}
.provider-zone-left:hover::after{border-right-color:var(--panel-accent)}
.provider-zone-right:hover::after{border-left-color:var(--panel-accent)}`}
      </style>
      <div style={{ position: 'relative' }}>
        <div
          ref={scrollRowRef}
          className="provider-hscroll"
          style={{
            display: 'flex',
            gap: 8,
            overflowX: 'auto',
            overflowY: 'hidden',
            padding: '4px 2px 8px',
          }}
        >
          {presets.map((p, idx) => {
            const active = p.id === activeId;
            const bg = active ? STICKER_COLORS[idx % STICKER_COLORS.length] : 'var(--panel-bg)';
            const logo = PROVIDER_LOGOS[p.id];
            return (
              <button
                key={p.id}
                type="button"
                onClick={() => onSelect(p.id)}
                title={p.endpoint || t('config.preset_custom')}
                style={{
                  flex: '0 0 auto',
                  display: 'flex',
                  flexDirection: 'column',
                  alignItems: 'center',
                  justifyContent: 'center',
                  gap: 5,
                  width: 84,
                  padding: '10px 6px 8px',
                  borderRadius: 12,
                  border: active
                    ? '1.5px solid var(--panel-accent)'
                    : '1.5px solid var(--panel-border)',
                  background: bg,
                  color: 'var(--panel-text)',
                  fontSize: 11,
                  fontWeight: active ? 700 : 500,
                  cursor: 'pointer',
                  textAlign: 'center',
                  transition: 'all 0.15s ease',
                  fontFamily: 'inherit',
                  lineHeight: 1.2,
                }}
                onMouseEnter={(e) => {
                  if (!active) {
                    e.currentTarget.style.borderColor = 'var(--panel-accent)';
                    e.currentTarget.style.transform = 'translateY(-2px)';
                  }
                }}
                onMouseLeave={(e) => {
                  if (!active) {
                    e.currentTarget.style.borderColor = 'var(--panel-border)';
                    e.currentTarget.style.transform = 'translateY(0)';
                  }
                }}
              >
                {logo ? (
                  p.id === 'moonshot' ? (
                    // Moonshot(Kimi) 的浅色/白色 logo 在浅色模式下对比不足，
                    // 增加一块中心深、向四周渐隐的径向渐变底提升辨识度
                    <div
                      style={{
                        width: 24,
                        height: 24,
                        borderRadius: 7,
                        flex: '0 0 auto',
                        display: 'inline-flex',
                        alignItems: 'center',
                        justifyContent: 'center',
                        background:
                          'radial-gradient(circle at 55% 45%, rgba(22,24,32,0.9) 0%, rgba(32,34,44,0.45) 55%, transparent 78%)',
                      }}
                    >
                      <img
                        src={logo}
                        alt=""
                        style={{ width: 15, height: 15, objectFit: 'contain' }}
                        draggable={false}
                      />
                    </div>
                  ) : (
                    <img
                      src={logo}
                      alt=""
                      style={{ width: 22, height: 22, objectFit: 'contain' }}
                      draggable={false}
                    />
                  )
                ) : (
                  <span
                    style={{
                      width: 22,
                      height: 22,
                      borderRadius: 6,
                      display: 'inline-flex',
                      alignItems: 'center',
                      justifyContent: 'center',
                      background: 'var(--panel-toggle-off)',
                      color: 'var(--panel-text-secondary)',
                      fontSize: 11,
                      fontWeight: 700,
                    }}
                  >
                    {t(p.labelKey).slice(0, 1)}
                  </span>
                )}
                <span
                  style={{
                    maxWidth: '100%',
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {t(p.labelKey)}
                </span>
              </button>
            );
          })}
        </div>
        <div
          className="provider-zone provider-zone-left"
          aria-hidden="true"
          onMouseEnter={enterLeft}
          onMouseLeave={leaveLeft}
          onMouseDown={pressLeft}
        />
        <div
          className="provider-zone provider-zone-right"
          aria-hidden="true"
          onMouseEnter={enterRight}
          onMouseLeave={leaveRight}
          onMouseDown={pressRight}
        />
      </div>
    </>
  );
};

/**
 * 服务商预设选择卡片网格 —— 替代原下拉选择
 *
 * 卡片网格（3 列、贴纸风格轮换色）展示全部服务商预设；
 * 选中预设 → 自动填充 provider_type / endpoint / model / context_window。
 *
 * 切换缓存机制：
 *   - 切换前把当前槽位的 provider_type/endpoint/model/api_key/api_secret/app_id
 *     快照到 config.provider_cache[当前preset.id]
 *   - 切换后从 config.provider_cache[目标preset.id] 恢复敏感字段；
 *     无缓存则清空 api_key/api_secret/app_id（防粘滞，避免上一家 key 误存到下一家）
 *   - 主配置与路由矩阵共享同一份 cache（同一家厂商的凭据应一致）
 *
 * pathPrefix 决定写入的配置路径前缀：
 *   - 主配置：'ai'（ai.provider / ai.endpoint / ai.model）
 *   - 路由任务：'routing_matrix.{taskType}'（routing_matrix.{taskType}.provider_type / .endpoint / .model）
 */
const ProviderSelector: React.FC<{
  pathPrefix: string;
  get: <T extends ConfigValue>(path: string, fallback: T) => T;
  setNested: (path: string, value: ConfigValue) => void;
  t: (key: string) => string;
}> = ({ pathPrefix, get, setNested, t }) => {
  const isMain = pathPrefix === 'ai';
  const providerTypePath = isMain ? 'ai.provider' : `${pathPrefix}.provider_type`;
  const endpointPath = `${pathPrefix}.endpoint`;
  const modelPath = `${pathPrefix}.model`;
  const apiKeyPath = `${pathPrefix}.api_key`;
  const apiSecretPath = `${pathPrefix}.api_secret`;
  const appIdPath = `${pathPrefix}.app_id`;
  const contextWindowPath = `${pathPrefix}.context_window`;

  const currentType = get(providerTypePath, 'openai') as string;
  const currentEndpoint = get(endpointPath, '') as string;

  // 匹配当前配置对应的预设（用于回显当前选中项）
  // 优先按 provider_type + endpoint 双重匹配（含协议变体）；endpoint 为空时回退到 provider_type 匹配
  const matchingPreset = PROVIDER_PRESETS.find((p) => presetMatches(p, currentType, currentEndpoint));
  const currentPresetId = matchingPreset?.id ?? 'custom';
  // 当前预设的协议变体与当前生效协议
  const presetProtocols = matchingPreset?.protocols ?? [];
  const activeProtocol = presetProtocols.find(
    (pr) => pr.providerType === currentType && (currentEndpoint === '' || pr.endpoint === currentEndpoint),
  );

  /** 切换 API 协议：仅覆盖 provider_type 与 endpoint，不动 model / api_key */
  const applyProtocol = (pr: ProviderProtocol) => {
    if (pr.providerType === currentType && pr.endpoint === currentEndpoint) return;
    setNested(providerTypePath, pr.providerType);
    setNested(endpointPath, pr.endpoint);
  };

  const applyPreset = (presetId: string) => {
    const preset = PROVIDER_PRESETS.find((p) => p.id === presetId);
    if (!preset || presetId === currentPresetId) return;

    // ① 切换前快照当前槽位配置到 provider_cache[当前preset.id]
    //    保留用户已填的 api_key/api_secret/app_id，切回来时自动恢复
    const currentApiKey = (get(apiKeyPath, '') as string) ?? '';
    const currentApiSecret = (get(apiSecretPath, '') as string) ?? '';
    const currentAppId = (get(appIdPath, '') as string) ?? '';
    const currentModel = (get(modelPath, '') as string) ?? '';
    setNested(`provider_cache.${currentPresetId}`, {
      provider_type: currentType,
      endpoint: currentEndpoint,
      model: currentModel,
      api_key: currentApiKey,
      api_secret: currentApiSecret,
      app_id: currentAppId,
    });

    // ② 切换预设：覆盖 provider_type / endpoint / model / context_window / max_tokens
    setNested(providerTypePath, preset.providerType);
    setNested(endpointPath, preset.endpoint);
    if (preset.defaultModel) {
      setNested(modelPath, preset.defaultModel);
    }
    if (preset.contextWindow) {
      setNested(contextWindowPath, preset.contextWindow);
    }
    // 主 LLM 配置：切换厂商时自动填入该厂商的建议输出上限（2048 默认对代码/长回复过小）
    if (isMain && preset.suggestedMaxTokens) {
      setNested('ai.max_tokens', preset.suggestedMaxTokens);
    }

    // ③ 从 provider_cache[目标preset.id] 恢复敏感字段；无缓存则清空（防粘滞）
    const cached = get(`provider_cache.${preset.id}`, '' as ConfigValue) as
      | { api_key?: string; api_secret?: string; app_id?: string }
      | string
      | null;
    const cachedProfile =
      cached && typeof cached === 'object' ? cached : null;
    if (cachedProfile) {
      setNested(apiKeyPath, cachedProfile.api_key ?? '');
      setNested(apiSecretPath, cachedProfile.api_secret ?? '');
      setNested(appIdPath, cachedProfile.app_id ?? '');
    } else {
      setNested(apiKeyPath, '');
      setNested(apiSecretPath, '');
      setNested(appIdPath, '');
    }
  };

  return (
    <div style={fieldStyle}>
      <label style={labelStyle}>{t('config.field_provider')}</label>
      <ProviderPresetRow presets={PROVIDER_PRESETS} activeId={currentPresetId} onSelect={applyPreset} t={t} />

      {/* API 协议选择（厂商支持多种协议时）+ 跳转供应商控制台获取 API Key */}
      {(presetProtocols.length > 1 || !!matchingPreset?.consoleUrl) && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap', marginTop: -2, marginBottom: 14 }}>
          {presetProtocols.length > 1 && (
            <>
              <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', flexShrink: 0 }}>
                {t('config.field_api_protocol')}
              </span>
              <div style={{ display: 'flex', gap: 5, flexWrap: 'wrap' }}>
                {presetProtocols.map((pr) => {
                  const active = pr === activeProtocol;
                  return (
                    <button
                      key={`${pr.providerType}:${pr.endpoint}`}
                      type="button"
                      onClick={() => applyProtocol(pr)}
                      style={{
                        padding: '4px 10px',
                        borderRadius: 8,
                        border: active
                          ? '1.5px solid var(--panel-accent)'
                          : '1.5px solid var(--panel-border)',
                        background: active ? 'var(--panel-bg-hover)' : 'var(--panel-surface)',
                        color: active ? 'var(--panel-accent)' : 'var(--panel-text-secondary)',
                        fontSize: 11,
                        fontWeight: active ? 700 : 500,
                        cursor: 'pointer',
                        fontFamily: 'inherit',
                        transition: 'all 0.15s ease',
                      }}
                    >
                      {t(pr.labelKey)}
                    </button>
                  );
                })}
              </div>
            </>
          )}
          {matchingPreset?.consoleUrl && (
            <button
              type="button"
              onClick={() => {
                const url = matchingPreset.consoleUrl;
                if (!url) return;
                void openShell(url).catch(() => window.open(url, '_blank', 'noopener,noreferrer'));
              }}
              title={matchingPreset.consoleUrl}
              style={{
                marginLeft: 'auto',
                display: 'inline-flex',
                alignItems: 'center',
                gap: 4,
                padding: '4px 10px',
                borderRadius: 8,
                border: '1px solid var(--panel-border)',
                background: 'transparent',
                color: 'var(--panel-accent)',
                fontSize: 11,
                fontWeight: 600,
                cursor: 'pointer',
                fontFamily: 'inherit',
                transition: 'border-color 0.15s ease, background 0.15s ease',
                flexShrink: 0,
              }}
              onMouseEnter={(e) => { e.currentTarget.style.borderColor = 'var(--panel-accent)'; e.currentTarget.style.background = 'var(--panel-bg-hover)'; }}
              onMouseLeave={(e) => { e.currentTarget.style.borderColor = 'var(--panel-border)'; e.currentTarget.style.background = 'transparent'; }}
            >
              <ExternalLink size={11} strokeWidth={2} />
              {t('config.get_api_key')}
            </button>
          )}
        </div>
      )}
    </div>
  );
};

/**
 * 工作模型服务商选择器 —— 复用主配置的 PROVIDER_PRESETS（完整厂商预设列表）
 *
 * 行为与主配置的 ProviderSelector 保持一致：
 *  - 选中预设自动填充 provider_type / endpoint / 默认 model；
 *  - 切换前把当前工作模型的凭据快照到 provider_cache[当前预设 id]，
 *    切换后从 provider_cache[目标预设 id] 恢复；无缓存则清空（防粘滞）；
 *  - 与主配置 / 路由矩阵共享同一份 provider_cache（同一家厂商的凭据应一致）。
 *
 * 额外保留"讯飞星火"选项（预设列表按主配置约定不含星火，但工作模型表单
 * 已支持 app_id / api_secret 字段，故单独列出以免丢失该能力）。
 */
const WorkModelProviderSelector: React.FC<{
  model: {
    provider_type: string;
    model: string;
    endpoint: string;
    api_key: string;
    api_secret?: string;
    app_id?: string;
  };
  onPatch: (patch: {
    provider_type: string;
    endpoint: string;
    model?: string;
    api_key: string;
    api_secret: string;
    app_id: string;
  }) => void;
  get: <T extends ConfigValue>(path: string, fallback: T) => T;
  setNested: (path: string, value: ConfigValue) => void;
  t: (key: string) => string;
}> = ({ model, onPatch, get, setNested, t }) => {
  const currentType = model.provider_type || 'openai';
  const currentEndpoint = model.endpoint || '';
  const matchingPreset = PROVIDER_PRESETS.find((p) => presetMatches(p, currentType, currentEndpoint));
  const currentPresetId = matchingPreset?.id ?? 'custom';
  // 当前预设的协议变体与当前生效协议
  const presetProtocols = matchingPreset?.protocols ?? [];
  const activeProtocol = presetProtocols.find(
    (pr) => pr.providerType === currentType && (currentEndpoint === '' || pr.endpoint === currentEndpoint),
  );

  /** 选中厂商预设卡片：快照当前凭据 → 覆盖 provider/endpoint/model → 恢复目标厂商缓存凭据 */
  const applyPresetById = (presetId: string) => {
    if (presetId === currentPresetId) return;
    // ① 切换前快照当前工作模型配置到 provider_cache[当前preset.id]
    setNested(`provider_cache.${currentPresetId}`, {
      provider_type: currentType,
      endpoint: currentEndpoint,
      model: model.model ?? '',
      api_key: model.api_key ?? '',
      api_secret: model.api_secret ?? '',
      app_id: model.app_id ?? '',
    });

    const preset = PROVIDER_PRESETS.find((p) => p.id === presetId);
    if (!preset) return;

    // ② 从 provider_cache[目标preset.id] 恢复敏感字段；无缓存则清空（防粘滞）
    const cached = get(`provider_cache.${preset.id}`, '' as ConfigValue) as
      | { api_key?: string; api_secret?: string; app_id?: string }
      | string
      | null;
    const cachedProfile = cached && typeof cached === 'object' ? cached : null;

    // ③ 覆盖 provider_type / endpoint / model，并写入（或清空）凭据
    const patch: {
      provider_type: string;
      endpoint: string;
      model?: string;
      api_key: string;
      api_secret: string;
      app_id: string;
    } = {
      provider_type: preset.providerType,
      endpoint: preset.endpoint,
      api_key: cachedProfile?.api_key ?? '',
      api_secret: cachedProfile?.api_secret ?? '',
      app_id: cachedProfile?.app_id ?? '',
    };
    if (preset.defaultModel) {
      patch.model = preset.defaultModel;
    }
    onPatch(patch);
  };

  return (
    <>
      <div style={fieldStyle}>
        <label style={labelStyle}>{t('config.field_provider')}</label>
        <ProviderPresetRow presets={PROVIDER_PRESETS} activeId={currentPresetId} onSelect={applyPresetById} t={t} />
      </div>

      {/* API 协议选择（厂商支持多种协议时）+ 跳转供应商控制台获取 API Key */}
      {(presetProtocols.length > 1 || !!matchingPreset?.consoleUrl) && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap', marginTop: -10, marginBottom: 14 }}>
          {presetProtocols.length > 1 && (
            <>
              <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', flexShrink: 0 }}>
                {t('config.field_api_protocol')}
              </span>
              <div style={{ display: 'flex', gap: 5, flexWrap: 'wrap' }}>
                {presetProtocols.map((pr) => {
                  const active = pr === activeProtocol;
                  return (
                    <button
                      key={`${pr.providerType}:${pr.endpoint}`}
                      type="button"
                      onClick={() => {
                        if (pr.providerType === currentType && pr.endpoint === currentEndpoint) return;
                        // 切换协议：仅覆盖 provider_type / endpoint，保留 model 与凭据
                        onPatch({
                          provider_type: pr.providerType,
                          endpoint: pr.endpoint,
                          api_key: model.api_key ?? '',
                          api_secret: model.api_secret ?? '',
                          app_id: model.app_id ?? '',
                        });
                      }}
                      style={{
                        padding: '4px 10px',
                        borderRadius: 8,
                        border: active
                          ? '1.5px solid var(--panel-accent)'
                          : '1.5px solid var(--panel-border)',
                        background: active ? 'var(--panel-bg-hover)' : 'var(--panel-surface)',
                        color: active ? 'var(--panel-accent)' : 'var(--panel-text-secondary)',
                        fontSize: 11,
                        fontWeight: active ? 700 : 500,
                        cursor: 'pointer',
                        fontFamily: 'inherit',
                        transition: 'all 0.15s ease',
                      }}
                    >
                      {t(pr.labelKey)}
                    </button>
                  );
                })}
              </div>
            </>
          )}
          {matchingPreset?.consoleUrl && (
            <button
              type="button"
              onClick={() => {
                const url = matchingPreset.consoleUrl;
                if (!url) return;
                void openShell(url).catch(() => window.open(url, '_blank', 'noopener,noreferrer'));
              }}
              title={matchingPreset.consoleUrl}
              style={{
                marginLeft: 'auto',
                display: 'inline-flex',
                alignItems: 'center',
                gap: 4,
                padding: '4px 10px',
                borderRadius: 8,
                border: '1px solid var(--panel-border)',
                background: 'transparent',
                color: 'var(--panel-accent)',
                fontSize: 11,
                fontWeight: 600,
                cursor: 'pointer',
                fontFamily: 'inherit',
                transition: 'border-color 0.15s ease, background 0.15s ease',
                flexShrink: 0,
              }}
              onMouseEnter={(e) => { e.currentTarget.style.borderColor = 'var(--panel-accent)'; e.currentTarget.style.background = 'var(--panel-bg-hover)'; }}
              onMouseLeave={(e) => { e.currentTarget.style.borderColor = 'var(--panel-border)'; e.currentTarget.style.background = 'transparent'; }}
            >
              <ExternalLink size={11} strokeWidth={2} />
              {t('config.get_api_key')}
            </button>
          )}
        </div>
      )}
    </>
  );
};

/**
 * 推理偏好控件 —— 三态选择（自动 / 关闭 / 开启）+ 档位下拉
 *
 * 值结构（与后端 ReasoningPreference 对齐）：
 *   { mode: 'auto' | 'off' | 'on', effort?: 'minimal'|'low'|'medium'|'high'|'xhigh'|'max' }
 *
 * - mode=auto：不干预，交由服务端默认（思考爆炸防护模型会映射到安全档位）
 * - mode=off：关闭思考（不支持关闭的模型由后端折叠为开启）
 * - mode=on：开启思考，可选档位（档位经后端按模型能力校验，不支持时回退默认档）
 */
const ReasoningPrefField: React.FC<{
  label: string;
  help?: string;
  value: { mode: string; effort?: string | null } | null | undefined;
  onChange: (v: { mode: string; effort?: string | null } | null) => void;
  t: (key: string) => string;
}> = ({ label, help, value, onChange, t }) => {
  const mode = value?.mode ?? 'auto';
  const effort = value?.effort ?? '';

  const modes: { key: string; labelKey: string }[] = [
    { key: 'auto', labelKey: 'config.reasoning_mode_auto' },
    { key: 'off', labelKey: 'config.reasoning_mode_off' },
    { key: 'on', labelKey: 'config.reasoning_mode_on' },
  ];

  const efforts = ['minimal', 'low', 'medium', 'high', 'xhigh', 'max'];

  return (
    <div style={fieldStyle}>
      <label style={labelStyle}>{label}</label>
      <div style={{ display: 'flex', gap: 6, marginBottom: mode === 'on' ? 8 : 0 }}>
        {modes.map((m) => {
          const active = mode === m.key;
          return (
            <button
              key={m.key}
              type="button"
              onClick={() => {
                if (m.key === 'auto') {
                  onChange(null);
                } else if (m.key === 'off') {
                  onChange({ mode: 'off' });
                } else {
                  onChange({ mode: 'on', effort: effort || 'medium' });
                }
              }}
              style={{
                flex: 1,
                padding: '8px 10px',
                borderRadius: 10,
                border: active
                  ? '1.5px solid var(--panel-accent)'
                  : '1.5px solid var(--panel-border)',
                background: active ? 'var(--panel-bg-hover)' : 'var(--panel-surface)',
                color: active ? 'var(--panel-accent)' : 'var(--panel-text-secondary)',
                fontSize: 12,
                fontWeight: active ? 700 : 500,
                cursor: 'pointer',
                fontFamily: 'inherit',
                transition: 'all 0.15s ease',
              }}
            >
              {t(m.labelKey)}
            </button>
          );
        })}
      </div>
      {mode === 'on' && (
        <select
          value={effort}
          onChange={(e) => onChange({ mode: 'on', effort: e.target.value })}
          style={{ ...inputStyle, padding: '7px 10px', fontSize: 12 }}
        >
          <option value="">{t('config.reasoning_effort_default')}</option>
          {efforts.map((e) => (
            <option key={e} value={e}>
              {t(`config.reasoning_effort_${e}`)}
            </option>
          ))}
        </select>
      )}
      {help && (
        <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: 6, lineHeight: 1.5 }}>
          {help}
        </div>
      )}
    </div>
  );
};

const fieldStyle: React.CSSProperties = {
  marginBottom: 18,
};
const labelStyle: React.CSSProperties = {
  display: 'block',
  fontSize: 12,
  fontWeight: 600,
  color: 'var(--panel-text-secondary)',
  marginBottom: 6,
  paddingLeft: 2,
};
const inputStyle: React.CSSProperties = {
  width: '100%',
  padding: '9px 12px',
  border: '1.5px solid var(--panel-border)',
  borderRadius: 12,
  background: 'var(--panel-surface)',
  color: 'var(--panel-text)',
  fontSize: 13,
  fontFamily: 'inherit',
  outline: 'none',
  boxSizing: 'border-box',
  boxShadow: 'var(--panel-shadow-subtle)',
  transition: 'border-color 0.15s ease, box-shadow 0.15s ease',
};
const selectStyle: React.CSSProperties = {
  ...inputStyle,
  appearance: 'none',
  cursor: 'pointer',
  paddingRight: 30,
};
const sectionTitleStyle: React.CSSProperties = {
  fontSize: 14,
  fontWeight: 700,
  color: 'var(--panel-text)',
  marginBottom: 14,
  paddingBottom: 8,
  paddingLeft: 12,
  borderLeft: '4px solid var(--panel-accent)',
  borderBottom: '2px dashed var(--panel-border)',
  letterSpacing: 0.3,
};

// 已知 Ollama 嵌入模型 → 向量维度映射（选择预设模型时自动填充 dimension 字段）
const OLLAMA_MODEL_DIMS: Record<string, number> = {
  'bge-m3': 1024,
  'nomic-embed-text': 768,
};

const TextField: React.FC<{
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  type?: 'text' | 'password' | 'number';
  disabled?: boolean;
  list?: string;
  style?: React.CSSProperties;
  help?: string;
}> = ({ label, value, onChange, placeholder, type = 'text', disabled = false, list, style, help }) => (
  <div style={{ ...fieldStyle, ...style }}>
    <label style={{ ...labelStyle, ...(disabled ? { opacity: 0.5 } : {}) }}>{label}</label>
    <input
      type={type}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      disabled={disabled}
      list={list}
      style={{
        ...inputStyle,
        ...(disabled
          ? {
              cursor: 'not-allowed',
              opacity: 0.5,
            }
          : {}),
      }}
    />
    {help && (
      <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: 6, lineHeight: 1.5 }}>
        {help}
      </div>
    )}
  </div>
);

/// 带浏览按钮的文本输入框：输入框与按钮在同一行水平对齐
/// （用 alignItems: 'flex-end' 让按钮底部与 input 底部齐平，
///   按钮的 padding 与 input 完全一致以保持高度相同）
const BrowseTextField: React.FC<{
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  onBrowse: () => void;
  browseLabel: string;
  disabled?: boolean;
}> = ({ label, value, onChange, placeholder, onBrowse, browseLabel, disabled = false }) => (
  <div style={{ ...fieldStyle, marginBottom: 18 }}>
    <label style={{ ...labelStyle, ...(disabled ? { opacity: 0.5 } : {}) }}>{label}</label>
    <div style={{ display: 'flex', gap: 6, alignItems: 'stretch' }}>
      <input
        type="text"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        disabled={disabled}
        style={{
          ...inputStyle,
          flex: 1,
          ...(disabled ? { cursor: 'not-allowed', opacity: 0.5 } : {}),
        }}
      />
      <button
        type="button"
        onClick={onBrowse}
        disabled={disabled}
        style={{
          padding: '8px 12px',
          border: '1.5px solid var(--panel-border)',
          borderRadius: 12,
          background: 'var(--panel-sticker-soft, var(--panel-surface))',
          color: 'var(--panel-text-secondary)',
          fontSize: 11,
          cursor: disabled ? 'not-allowed' : 'pointer',
          fontFamily: 'inherit',
          whiteSpace: 'nowrap',
          boxSizing: 'border-box',
          flexShrink: 0,
          opacity: disabled ? 0.5 : 1,
          boxShadow: 'var(--panel-shadow-subtle)',
        }}
      >
        {browseLabel}
      </button>
    </div>
  </div>
);

/// 分组小标题（GPT-SoVITS 面板内部使用）
const subsectionTitleStyle: React.CSSProperties = {
  marginTop: 20,
  marginBottom: 10,
  fontSize: 11,
  color: 'var(--panel-text-tertiary)',
  fontWeight: 600,
  letterSpacing: 0.5,
  textTransform: 'uppercase',
};

const SelectField: React.FC<{
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string; disabled?: boolean }[];
  labelExtra?: React.ReactNode;
}> = ({ label, value, onChange, options, labelExtra }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const selectRef = React.useRef<HTMLDivElement>(null);
  const dropdownRef = React.useRef<HTMLDivElement>(null);
  const buttonRef = React.useRef<HTMLButtonElement>(null);
  const [dropdownPos, setDropdownPos] = React.useState<{ top: number; left: number; width: number; upward: boolean } | null>(null);

  const updateDropdownPosition = React.useCallback(() => {
    if (buttonRef.current) {
      const rect = buttonRef.current.getBoundingClientRect();
      const dropdownHeight = Math.min(options.length * 40 + 2, 200);
      const spaceBelow = window.innerHeight - rect.bottom;
      const upward = spaceBelow < dropdownHeight + 8 && rect.top > dropdownHeight + 8;
      setDropdownPos({
        top: upward ? rect.top - 4 : rect.bottom + 4,
        left: rect.left,
        width: rect.width,
        upward,
      });
    }
  }, [options.length]);

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      const target = e.target as Node;
      if (
        (selectRef.current && selectRef.current.contains(target)) ||
        (dropdownRef.current && dropdownRef.current.contains(target))
      ) {
        return;
      }
      setOpen(false);
    };
    if (open) {
      document.addEventListener('mousedown', handleClickOutside);
      updateDropdownPosition();
      window.addEventListener('scroll', updateDropdownPosition, true);
      window.addEventListener('resize', updateDropdownPosition);
    }
    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
      window.removeEventListener('scroll', updateDropdownPosition, true);
      window.removeEventListener('resize', updateDropdownPosition);
    };
  }, [open, updateDropdownPosition]);

  const selectedOption = options.find((o) => o.value === value);

  return (
    <div style={fieldStyle} ref={selectRef}>
      <div style={{ ...labelStyle, display: 'flex', alignItems: 'center', gap: 6 }}>
        <span>{label}</span>
        {labelExtra}
      </div>
      <div>
        <button
          ref={buttonRef}
          type="button"
          onClick={() => setOpen(!open)}
          style={{
            ...selectStyle,
            textAlign: 'left',
            position: 'relative',
            background: 'var(--panel-bg-surface-elevated)',
            width: '100%',
          }}
        >
          <span style={{ color: selectedOption ? 'var(--panel-text)' : 'var(--panel-text-tertiary)' }}>
            {selectedOption ? selectedOption.label : t('common.please_select')}
          </span>
          <span
            style={{
              position: 'absolute',
              right: 10,
              top: '50%',
              transform: `translateY(-50%) ${open ? 'rotate(180deg)' : 'rotate(0deg)'}`,
              transition: 'transform 0.2s ease',
              color: 'var(--panel-text-tertiary)',
              fontSize: 10,
            }}
          >
            ▾
          </span>
        </button>
        {open && dropdownPos && ReactDOM.createPortal(
          <div
            ref={dropdownRef}
            style={{
              position: 'fixed',
              top: dropdownPos.top,
              left: dropdownPos.left,
              width: dropdownPos.width,
              background: 'var(--panel-surface)',
              border: '1.5px solid var(--panel-border-strong)',
              borderRadius: 10,
              boxShadow: 'var(--panel-shadow-elevated)',
              zIndex: 10000,
              maxHeight: 200,
              overflowY: 'auto',
              animation: 'fadeIn 0.15s ease-out',
            }}
          >
            {options.map((o) => (
              <button
                key={o.value}
                type="button"
                disabled={o.disabled}
                onClick={() => {
                  if (o.disabled) return;
                  onChange(o.value);
                  setOpen(false);
                }}
                style={{
                  width: '100%',
                  textAlign: 'left',
                  padding: '10px 14px',
                  background: o.value === value ? 'var(--panel-selected-bg)' : 'transparent',
                  color: o.disabled
                    ? 'var(--panel-text-tertiary)'
                    : o.value === value
                      ? 'var(--panel-selected-text)'
                      : 'var(--panel-text)',
                  border: 'none',
                  cursor: o.disabled ? 'not-allowed' : 'pointer',
                  fontSize: 13,
                  fontFamily: 'inherit',
                  opacity: o.disabled ? 0.5 : 1,
                  transition: 'background 0.1s ease',
                }}
                onMouseEnter={(e) => {
                  if (o.disabled || o.value === value) return;
                  e.currentTarget.style.background = 'var(--panel-bg-hover)';
                }}
                onMouseLeave={(e) => {
                  if (o.disabled || o.value === value) return;
                  e.currentTarget.style.background = 'transparent';
                }}
              >
                {o.label}
                {o.disabled && (
                  <span style={{ marginLeft: 6, fontSize: 11, color: 'var(--panel-text-tertiary)' }}>
                    {t('common.coming_soon')}
                  </span>
                )}
              </button>
            ))}
          </div>,
          document.body
        )}
      </div>
    </div>
  );
};

const NumberField: React.FC<{
  label: string;
  value: number;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
  step?: number;
  help?: string;
}> = ({ label, value, onChange, min, max, step, help }) => (
  <div style={fieldStyle}>
    <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
      <label style={labelStyle}>{label}</label>
      {help && (
        <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', lineHeight: 1.4 }}>
          {help}
        </span>
      )}
    </div>
    <input
      type="number"
      value={value}
      onChange={(e) => onChange(Number(e.target.value))}
      min={min}
      max={max}
      step={step}
      style={inputStyle}
    />
  </div>
);

const SliderField: React.FC<{
  label: string;
  value: number;
  onChange: (v: number) => void;
  min: number;
  max: number;
  step: number;
  format?: (v: number) => string;
  help?: string;
}> = ({ label, value, onChange, min, max, step, format, help }) => (
  <div style={fieldStyle}>
    <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 6 }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 2, flex: 1, minWidth: 0 }}>
        <label style={{ ...labelStyle, marginBottom: 0 }}>{label}</label>
        {help && (
          <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', lineHeight: 1.4 }}>
            {help}
          </span>
        )}
      </div>
      <span style={{ fontSize: 12, color: 'var(--panel-text)', fontVariantNumeric: 'tabular-nums' }}>
        {format ? format(value) : value}
      </span>
    </div>
    <input
      type="range"
      value={value}
      onChange={(e) => onChange(Number(e.target.value))}
      min={min}
      max={max}
      step={step}
      style={{ width: '100%', accentColor: 'var(--panel-accent)' }}
    />
  </div>
);

const ToggleField: React.FC<{
  label: string;
  value: boolean;
  onChange: (v: boolean) => void;
  help?: string;
}> = ({ label, value, onChange, help }) => (
  <div style={{ ...fieldStyle, display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
    <div style={{ display: 'flex', flexDirection: 'column', gap: 2, flex: 1, minWidth: 0 }}>
      <label style={{ ...labelStyle, marginBottom: 0 }}>{label}</label>
      {help && (
        <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', lineHeight: 1.4 }}>
          {help}
        </span>
      )}
    </div>
    <button
      onClick={() => onChange(!value)}
      style={{
        width: 40,
        height: 22,
        borderRadius: 11,
        border: 'none',
        background: value ? 'var(--panel-accent)' : 'var(--panel-toggle-off)',
        position: 'relative',
        cursor: 'pointer',
        transition: 'background 0.2s ease',
      }}
    >
      <span
        style={{
          position: 'absolute',
          top: 2,
          left: value ? 20 : 2,
          width: 18,
          height: 18,
          borderRadius: '50%',
          background: 'var(--panel-surface)',
          transition: 'left 0.2s ease',
          boxShadow: 'var(--panel-shadow-subtle)',
        }}
      />
    </button>
  </div>
);

/**
 * 工具开关卡片 —— 设置-工具页的工具级启用/禁用开关
 *
 * 卡片布局：工具名 + 类别徽标 + 描述 + 右侧胶囊开关；
 * 禁用态整体降为半透明。开关状态写入
 * `config.tools.disabled_tools`，保存后由后端同步到 ToolSystem 即时生效。
 */
const ToolSwitchCard: React.FC<{
  name: string;
  description: string;
  categoryLabel: string;
  enabled: boolean;
  custom: boolean;
  customBadge: string;
  onToggle: () => void;
}> = ({ name, description, categoryLabel, enabled, custom, customBadge, onToggle }) => (
  <div
    style={{
      display: 'flex',
      alignItems: 'center',
      gap: 10,
      minWidth: 0,
      padding: '10px 12px',
      borderRadius: 10,
      border: custom
        ? '1.5px dashed var(--panel-accent)'
        : '1px solid var(--panel-border)',
      background: custom
        ? 'linear-gradient(135deg, rgba(124, 92, 255, 0.08), transparent 55%)'
        : 'var(--panel-card)',
      opacity: enabled ? 1 : 0.55,
      transition: 'opacity 0.2s ease, border-color 0.2s ease',
    }}
  >
    <div style={{ flex: 1, minWidth: 0 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6, minWidth: 0 }}>
        {custom && (
          <Sparkles
            size={12}
            strokeWidth={2}
            style={{ flexShrink: 0, color: 'var(--panel-accent)' }}
          />
        )}
        <span
          style={{
            fontSize: 12.5,
            fontWeight: 600,
            color: custom ? 'var(--panel-accent)' : 'var(--panel-text)',
            overflow: 'hidden',
            whiteSpace: 'nowrap',
            textOverflow: 'ellipsis',
            fontFamily: 'ui-monospace, SFMono-Regular, Consolas, monospace',
          }}
        >
          {name}
        </span>
        {custom ? (
          <span
            style={{
              flexShrink: 0,
              padding: '1px 7px',
              fontSize: 10,
              fontWeight: 600,
              color: 'var(--panel-accent)',
              background: 'rgba(124, 92, 255, 0.12)',
              border: '1px solid var(--panel-accent)',
              borderRadius: 999,
            }}
          >
            {customBadge}
          </span>
        ) : (
          <span
            style={{
              flexShrink: 0,
              padding: '1px 7px',
              fontSize: 10,
              fontWeight: 500,
              color: 'var(--panel-text-tertiary)',
              background: 'var(--panel-bg-hover)',
              border: '1px solid var(--panel-border)',
              borderRadius: 999,
            }}
          >
            {categoryLabel}
          </span>
        )}
      </div>
      <div
        title={description}
        style={{
          marginTop: 3,
          fontSize: 11,
          lineHeight: 1.45,
          color: 'var(--panel-text-tertiary)',
          overflow: 'hidden',
          display: '-webkit-box',
          WebkitBoxOrient: 'vertical',
          WebkitLineClamp: 2,
          wordBreak: 'break-word',
        }}
      >
        {description || '—'}
      </div>
    </div>
    <button
      type="button"
      role="switch"
      aria-checked={enabled}
      onClick={onToggle}
      style={{
        flexShrink: 0,
        width: 36,
        height: 20,
        borderRadius: 10,
        border: 'none',
        background: enabled ? 'var(--panel-accent)' : 'var(--panel-toggle-off)',
        position: 'relative',
        cursor: 'pointer',
        transition: 'background 0.2s ease',
        padding: 0,
      }}
    >
      <span
        style={{
          position: 'absolute',
          top: 2,
          left: enabled ? 18 : 2,
          width: 16,
          height: 16,
          borderRadius: '50%',
          background: 'var(--panel-surface)',
          transition: 'left 0.2s ease',
          boxShadow: 'var(--panel-shadow-subtle)',
        }}
      />
    </button>
  </div>
);

/// 高级设置折叠按钮 — 可复用的"展开/收起"切换器
///
/// - `open`：当前是否展开
/// - `onToggle`：切换回调
/// - `label`：按钮文字（通常为"高级设置"）
const AdvancedToggle: React.FC<{
  open: boolean;
  onToggle: () => void;
  label: string;
}> = ({ open, onToggle, label }) => (
  <button
    type="button"
    onClick={onToggle}
    style={{
      width: '100%',
      display: 'flex',
      alignItems: 'center',
      gap: 8,
      padding: '8px 0',
      background: 'transparent',
      border: 'none',
      borderTop: '1px solid var(--panel-border)',
      cursor: 'pointer',
      fontFamily: 'inherit',
      color: 'var(--panel-text-secondary)',
      fontSize: 12,
      marginTop: 8,
    }}
  >
    <span
      style={{
        display: 'inline-block',
        transition: 'transform 0.15s',
        transform: open ? 'rotate(90deg)' : 'rotate(0deg)',
        color: 'var(--panel-text-tertiary)',
        fontSize: 10,
      }}
    >
      ▸
    </span>
    <span>{label}</span>
  </button>
);

/// 多选字段（checkbox 组）—— 用于"多引擎混用"等需要同时启用多个选项的场景
///
/// - `values`：当前已选中的值列表
/// - `options`：所有可选项目
/// - `minSelected`：最少需选中数量（达到时不允许取消最后一项）
const MultiCheckboxField: React.FC<{
  label: string;
  values: string[];
  options: { value: string; label: string }[];
  onChange: (next: string[]) => void;
  help?: string;
  minSelected?: number;
}> = ({ label, values, options, onChange, help, minSelected = 0 }) => {
  const { t } = useTranslation();
  const toggle = (val: string) => {
    if (values.includes(val)) {
      // 取消选中 —— 受 minSelected 约束
      if (values.length <= minSelected) return;
      onChange(values.filter((v) => v !== val));
    } else {
      // 选中 —— 追加到末尾以保留用户优先级顺序
      onChange([...values, val]);
    }
  };

  return (
    <div style={fieldStyle}>
      <div style={{ ...labelStyle, display: 'flex', alignItems: 'center', gap: 6 }}>
        <span>{label}</span>
      </div>
      {help && (
        <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', lineHeight: 1.4, marginBottom: 8, display: 'block' }}>
          {help}
        </span>
      )}
      <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
        {options.map((opt) => {
          const checked = values.includes(opt.value);
          const atMin = values.length <= minSelected;
          return (
            <label
              key={opt.value}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 10,
                padding: '8px 10px',
                border: `1.5px solid ${checked ? 'var(--panel-border-strong)' : 'var(--panel-border)'}`,
                borderRadius: 8,
                background: checked ? 'var(--panel-tag-bg)' : 'var(--panel-surface)',
                cursor: 'pointer',
                opacity: checked && atMin ? 0.65 : 1,
                transition: 'all 0.15s ease',
                fontSize: 13,
                color: 'var(--panel-text)',
              }}
              onMouseEnter={(e) => {
                if (!checked) {
                  e.currentTarget.style.borderColor = 'var(--panel-border-hover)';
                  e.currentTarget.style.background = 'var(--panel-tag-bg)';
                }
              }}
              onMouseLeave={(e) => {
                if (!checked) {
                  e.currentTarget.style.borderColor = 'var(--panel-border)';
                  e.currentTarget.style.background = 'var(--panel-surface)';
                }
              }}
            >
              <input
                type="checkbox"
                checked={checked}
                onChange={() => toggle(opt.value)}
                disabled={checked && atMin}
                title={checked && atMin ? t('config.web_search_min_required') : undefined}
                style={{
                  width: 16,
                  height: 16,
                  accentColor: 'var(--panel-accent)',
                  cursor: 'pointer',
                  margin: 0,
                }}
              />
              <span>{opt.label}</span>
            </label>
          );
        })}
      </div>
    </div>
  );
};

/** 可折叠分组容器 - 用于路由矩阵按任务折叠 */
const CollapsibleSection: React.FC<{
  title: string;
  subtitle?: string;
  defaultOpen?: boolean;
  tone?: 'default' | 'danger';
  /** 标题行右侧的附件区域（如已配置模型名），固定单行、过长时水平滚动 */
  titleAccessory?: React.ReactNode;
  children: React.ReactNode;
}> = ({ title, subtitle, defaultOpen = false, tone = 'default', titleAccessory, children }) => {
  const [open, setOpen] = useState(defaultOpen);
  const isDanger = tone === 'danger';
  const accent = isDanger ? '#E53935' : 'var(--panel-border)';
  return (
    <div
      style={{
        marginBottom: 10,
      }}
    >
      <div
        style={{
          border: `1.5px solid ${accent}`,
          borderRadius: open ? '10px 10px 0 0' : 10,
          overflow: 'hidden',
          background: isDanger ? 'rgba(229, 57, 53, 0.04)' : 'var(--panel-surface)',
        }}
      >
        <button
          onClick={() => setOpen((v) => !v)}
          style={{
            width: '100%',
            display: 'flex',
            flexDirection: 'column',
            gap: 4,
            padding: '10px 14px',
            background: 'transparent',
            cursor: 'pointer',
            fontFamily: 'inherit',
            textAlign: 'left' as const,
          }}
        >
          <span style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
            <span style={{ fontSize: 13, fontWeight: 700, color: isDanger ? accent : 'var(--panel-text)', whiteSpace: 'nowrap', flexShrink: 0 }}>{title}</span>
            {titleAccessory && (
              <span
                style={{
                  flex: '1 1 auto',
                  minWidth: 0,
                  overflowX: 'auto',
                  overflowY: 'hidden',
                  whiteSpace: 'nowrap',
                  textAlign: 'right',
                  scrollbarWidth: 'none',
                  msOverflowStyle: 'none',
                }}
                className="title-accessory-scroll"
              >
                {titleAccessory}
              </span>
            )}
            <span style={{ flex: '0 0 auto', fontSize: 12, color: isDanger ? accent : 'var(--panel-text-tertiary)' }}>{open ? '▾' : '▸'}</span>
          </span>
          {subtitle && (
            <span style={{
              fontSize: 11,
              color: 'var(--panel-text-tertiary)',
              whiteSpace: open ? 'pre-wrap' : 'nowrap',
              overflow: 'hidden',
              textOverflow: open ? undefined : 'ellipsis',
              lineHeight: 1.5,
            }}>{subtitle}</span>
          )}
        </button>
      </div>
      {open && (
        <div
          style={{
            border: `1.5px solid ${accent}`,
            borderTop: 'none',
            borderRadius: '0 0 10px 10px',
            padding: '12px 14px',
            overflow: 'visible',
            background: 'var(--panel-surface)',
          }}
        >
          {children}
        </div>
      )}
    </div>
  );
};

/// 辅助参考音频抽屉 — 可折叠展开/收缩列表，收缩时只显示标题和数量徽章
const AuxRefAudiosDrawer: React.FC<{
  value: string[] | null;
  onChange: (arr: string[]) => void;
  label: string;
  addLabel: string;
}> = ({ value, onChange, label, addLabel }) => {
  const [expanded, setExpanded] = useState(false);
  const items = value ?? [];
  const count = items.length;

  return (
    <div style={{ marginBottom: 18 }}>
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        style={{
          width: '100%',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          padding: '8px 0',
          background: 'transparent',
          border: 'none',
          cursor: 'pointer',
          fontFamily: 'inherit',
          color: 'var(--panel-text)',
          fontSize: 13,
        }}
      >
        <span style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <span
            style={{
              display: 'inline-block',
              transition: 'transform 0.15s',
              transform: expanded ? 'rotate(90deg)' : 'rotate(0deg)',
              color: 'var(--panel-text-tertiary)',
              fontSize: 10,
            }}
          >
            ▸
          </span>
          <span style={{ opacity: 0.85 }}>{label}</span>
          {count > 0 && (
            <span
              style={{
                padding: '1px 8px',
                borderRadius: 10,
                background: 'var(--panel-tag-bg)',
                color: 'var(--panel-text-secondary)',
                fontSize: 10,
                minWidth: 18,
                textAlign: 'center',
              }}
            >
              {count}
            </span>
          )}
        </span>
      </button>
      {expanded && (
        <div style={{ marginTop: 4 }}>
          {items.map((p, i) => (
            // 无唯一 ID 且无重排，用 index 作 key
            <div key={i} style={{ display: 'flex', alignItems: 'center', gap: 4, marginBottom: 4 }}>
              <span style={{ fontSize: 10, color: 'var(--panel-text-tertiary)', minWidth: 20 }}>
                #{i + 1}
              </span>
              <input
                type="text"
                value={p}
                onChange={(e) => {
                  const arr = [...items];
                  arr[i] = e.target.value;
                  onChange(arr);
                }}
                style={{
                  flex: 1,
                  padding: '6px 8px',
                  border: '1.5px solid var(--panel-border)',
                  borderRadius: 6,
                  background: 'var(--panel-surface)',
                  color: 'var(--panel-text)',
                  fontSize: 11,
                  fontFamily: 'inherit',
                }}
              />
              <button
                type="button"
                onClick={() => {
                  const arr = [...items];
                  arr.splice(i, 1);
                  onChange(arr);
                }}
                style={{
                  padding: '4px 8px',
                  border: '1.5px solid var(--panel-border)',
                  borderRadius: 6,
                  background: 'var(--panel-surface)',
                  color: '#E53935',
                  fontSize: 10,
                  cursor: 'pointer',
                  fontFamily: 'inherit',
                  whiteSpace: 'nowrap',
                }}
              >
                ✕
              </button>
            </div>
          ))}
          <button
            type="button"
            onClick={async () => {
              const selected = await open({
                multiple: true,
                filters: [{ name: 'Audio', extensions: ['wav', 'mp3', 'flac'] }],
              });
              if (selected) {
                const paths = Array.isArray(selected) ? selected : [selected];
                onChange([...items, ...paths]);
              }
            }}
            style={{
              padding: '4px 10px',
              border: '1.5px dashed var(--panel-border)',
              borderRadius: 6,
              background: 'transparent',
              color: 'var(--panel-text-secondary)',
              fontSize: 10,
              cursor: 'pointer',
              fontFamily: 'inherit',
            }}
          >
            + {addLabel}
          </button>
        </div>
      )}
    </div>
  );
};

/// 快捷键配置抽屉 — 收纳通用页的 6 个快捷键录制器，折叠时只显示标题与已配置数量徽章
const ShortcutsDrawer: React.FC<{
  label: string;
  expanded: boolean;
  onToggle: () => void;
  configuredCount: number;
  children: React.ReactNode;
}> = ({ label, expanded, onToggle, configuredCount, children }) => (
  <div style={{ marginBottom: 18 }}>
    <button
      type="button"
      onClick={onToggle}
      style={{
        width: '100%',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        padding: '8px 0',
        background: 'transparent',
        border: 'none',
        cursor: 'pointer',
        fontFamily: 'inherit',
        color: 'var(--panel-text)',
        fontSize: 13,
      }}
    >
      <span style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <span
          style={{
            display: 'inline-block',
            transition: 'transform 0.15s',
            transform: expanded ? 'rotate(90deg)' : 'rotate(0deg)',
            color: 'var(--panel-text-tertiary)',
            fontSize: 10,
          }}
        >
          ▸
        </span>
        <span style={{ opacity: 0.85 }}>{label}</span>
        {configuredCount > 0 && (
          <span
            style={{
              padding: '1px 8px',
              borderRadius: 10,
              background: 'var(--panel-tag-bg)',
              color: 'var(--panel-text-secondary)',
              fontSize: 10,
              minWidth: 18,
              textAlign: 'center',
            }}
          >
            {configuredCount}
          </span>
        )}
      </span>
    </button>
    {expanded && <div style={{ marginTop: 4 }}>{children}</div>}
  </div>
);

const ConfigWindow: React.FC = () => {
  const { t, i18n } = useTranslation();
  const [activeTab, setActiveTab] = useState<TabKey>('general');
  // 主 LLM 未配置时显示初始配置引导弹窗
  const [setupGuideOpen, setSetupGuideOpen] = useState(false);
  const [config, setConfig] = useState<ConfigObject>({});
  // 应用版本号（来自 tauri.conf.json 的 version）
  const [appVersion, setAppVersion] = useState<string>('');
  // 操作系统信息（platform + version + arch）
  const [osInfo, setOsInfo] = useState<string>('');
  const [saving, setSaving] = useState(false);
  const [savedFlash, setSavedFlash] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  // 快捷键配置抽屉展开状态
  const [shortcutsExpanded, setShortcutsExpanded] = useState(false);
  const [ttsConfig, setTtsConfig] = useState<TtsConfigState | null>(null);
  // 语音页签当前正在编辑的角色 ID（默认为窗口所属角色）
  // 允许在同一配置窗口内切换编辑 Vivian / Nana 的 TTS 配置
  const [ttsEditCharId, setTtsEditCharId] = useState<string | null>(null);
  // 所有角色列表（用于语音页签的角色切换器）
  const [characters, setCharacters] = useState<Array<{ id: string; name: string; online: boolean }>>([]);
  // 所有角色的 TTS 引擎（用于判断双实例开关是否显示）
  const [charTtsEngines, setCharTtsEngines] = useState<Record<string, string>>({});
  // 未保存的 TTS 配置草稿（切换角色时保留编辑中内容，不丢失）
  const [ttsDrafts, setTtsDrafts] = useState<Record<string, TtsConfigState>>({});
  // GPT-SoVITS 服务子进程状态(一键启动/停止)
  const [gptsovitsService, setGptsovitsService] = useState<GptSoVitsServiceState | null>(null);
  const [gptsovitsServiceBusy, setGptsovitsServiceBusy] = useState(false);
  // Fish Speech 服务子进程状态(一键启动/停止)
  const [fishSpeechService, setFishSpeechService] = useState<FishSpeechServiceState | null>(null);
  const [fishSpeechServiceBusy, setFishSpeechServiceBusy] = useState(false);
  // GPT-SoVITS 模型列表(扫描安装目录) + 整合包 runtime 检测
  const [gptSovitsModels, setGptSovitsModels] = useState<{
    gpt_models: Array<{ name: string; path: string }>;
    sovits_models: Array<{ name: string; path: string }>;
    has_runtime: boolean;
  }>({ gpt_models: [], sovits_models: [], has_runtime: false });
  // Ollama 本地嵌入服务状态(一键启动/停止) + 已安装模型列表 + 拉取中标志
  const [ollamaService, setOllamaService] = useState<OllamaServiceState | null>(null);
  const [ollamaServiceBusy, setOllamaServiceBusy] = useState(false);
  const [ollamaModels, setOllamaModels] = useState<string[]>([]);
  const [ollamaPulling, setOllamaPulling] = useState(false);
  // 内置嵌入模型注册表（来自后端 embedding_registry，用于云端模型维度自动填充）
  const [embeddingModels, setEmbeddingModels] = useState<{ id: string; dimension: number; source: string }[]>([]);
  // Whisper 本地 ASR 服务状态(一键启动/停止 faster-whisper-server)
  const [whisperService, setWhisperService] = useState<WhisperServiceState | null>(null);
  const [whisperServiceBusy, setWhisperServiceBusy] = useState(false);
  // Whisper 高级设置折叠
  const [whisperAdvancedOpen, setWhisperAdvancedOpen] = useState(false);
  // 各 ASR/TTS provider 高级设置折叠
  const [azureAsrAdvancedOpen, setAzureAsrAdvancedOpen] = useState(false);
  const [aliyunAsrAdvancedOpen, setAliyunAsrAdvancedOpen] = useState(false);
  const [azureTtsAdvancedOpen, setAzureTtsAdvancedOpen] = useState(false);
  const [fishSpeechAdvancedOpen, setFishSpeechAdvancedOpen] = useState(false);
  const [minimaxAdvancedOpen, setMinimaxAdvancedOpen] = useState(false);
  const [doubaoAdvancedOpen, setDoubaoAdvancedOpen] = useState(false);
  // TTS 帮助说明书抽屉
  const [ttsHelpOpen, setTtsHelpOpen] = useState(false);
  const [ttsHelpBackend, setTtsHelpBackend] = useState<TtsBackendKey>('edgetts');
  // EdgeTTS 语音列表
  const [edgeTtsVoices, setEdgeTtsVoices] = useState<Array<{ id: string; name: string; language: string }>>([]);
  // ASR 帮助说明书抽屉
  const [asrHelpOpen, setAsrHelpOpen] = useState(false);
  const [asrHelpBackend, setAsrHelpBackend] = useState<AsrBackendKey>('winrt');

  // 日记配置
  const [diaryConfig, setDiaryConfig] = useState<DiaryConfigState | null>(null);
  const [diaryLoading, setDiaryLoading] = useState(false);

  // 网络连接测试状态
  const [networkTesting, setNetworkTesting] = useState(false);

  // 地理定位自动检测状态
  const [detectingLocation, setDetectingLocation] = useState(false);
  const [networkTestResult, setNetworkTestResult] = useState<string | null>(null);
  const [networkTestSuccess, setNetworkTestSuccess] = useState<boolean | null>(null);

  // MCP server 管理
  const [mcpServers, setMcpServers] = useState<Array<{
    id: string; name: string; enabled: boolean; tool_count: number; alive: boolean;
  }>>([]);
  const [mcpEditing, setMcpEditing] = useState<{
    id: string; name: string; command: string; args: string; enabled: boolean;
  } | null>(null);
  const [mcpSaving, setMcpSaving] = useState(false);
  // 工具页签：全部注册工具清单（后端 list_tools，名称/描述/类别；
  // 描述按当前界面语言返回；is_custom 标记智能体自进化创造的自建工具）
  const [toolList, setToolList] = useState<Array<{ name: string; description: string; category: string; is_custom: boolean }>>([]);
  // 工具开关卡片的搜索过滤词
  const [toolSearch, setToolSearch] = useState('');
  const [clearMemoriesOpen, setClearMemoriesOpen] = useState(false);
  const [clearingMemories, setClearingMemories] = useState(false);

  // 数据备份与迁移
  const [backingUp, setBackingUp] = useState(false);
  const [restoreConfirmOpen, setRestoreConfirmOpen] = useState(false);
  const [restoreSource, setRestoreSource] = useState<string | null>(null);
  const [restoring, setRestoring] = useState(false);

  // 路由矩阵：每个任务最近一次请求状态（'ok' 绿色 / 'error' 红色）
  // 由后端 chat:route_status 事件驱动，仅在路由矩阵开启时有意义
  const [routeStatus, setRouteStatus] = useState<Record<string, 'ok' | 'error'>>({});

  // LLM 一键检测：主配置 + 各路由的 API 可用性测试结果
  // key = 'main'（主 LLM 配置）或路由 taskType
  const [llmTesting, setLlmTesting] = useState(false);
  const [llmTestResults, setLlmTestResults] = useState<
    Record<string, {
      state: 'testing' | 'ok' | 'error' | 'skipped';
      error?: string;
      elapsedMs?: number;
      reply?: string;
    }>
  >({});

  const get = <T extends ConfigValue>(path: string, fallback: T): T => {
    const parts = path.split('.');
    let cur: ConfigValue = config;
    for (const p of parts) {
      if (cur && typeof cur === 'object' && p in cur) {
        cur = (cur as ConfigObject)[p];
      } else return fallback;
    }
    return (cur as T) ?? fallback;
  };

  const setNested = (path: string, value: ConfigValue) => {
    const parts = path.split('.');
    setConfig((prev) => {
      const next: ConfigObject = JSON.parse(JSON.stringify(prev));
      let cur: ConfigObject = next;
      for (let i = 0; i < parts.length - 1; i++) {
        const p = parts[i];
        if (!cur[p] || typeof cur[p] !== 'object') cur[p] = {};
        cur = cur[p] as ConfigObject;
      }
      cur[parts[parts.length - 1]] = value;
      return next;
    });
  };

  // ===== 工作智能体模型预置（编程页列表切换用）=====
  type WorkModelProfile = {
    id: string;
    name: string;
    provider_type: string;
    model: string;
    api_key: string;
    endpoint: string;
    api_secret?: string;
    app_id?: string;
    temperature?: number | null;
    max_tokens?: number | null;
    context_window?: number | null;
    reasoning?: { mode: string; effort?: string | null } | null;
  };
  const workModels = (get('work_models', []) as WorkModelProfile[]);
  const workModelsActiveId = (config?.active_work_model ?? null) as string | null;
  const updateWorkModels = (next: WorkModelProfile[]) =>
    setNested('work_models', next as unknown as ConfigValue);
  const patchWorkModel = (idx: number, patch: Partial<WorkModelProfile>) =>
    // 注意：id 保持稳定（作为激活/移除的标识），但 name（模型别名）必须允许 patch 覆盖
    updateWorkModels(workModels.map((m, i) => (i === idx ? { ...m, ...patch, id: m.id } : m)));
  const addWorkModel = () => {
    updateWorkModels([
      ...workModels,
      {
        id: `wm_${Date.now()}`,
        name: `模型${workModels.length + 1}`,
        provider_type: 'openai',
        model: '',
        api_key: '',
        endpoint: '',
        api_secret: '',
        app_id: '',
        temperature: null,
        max_tokens: null,
      },
    ]);
  };
  const removeWorkModel = (id: string) => {
    updateWorkModels(workModels.filter((m) => m.id !== id));
    if (workModelsActiveId === id) {
      setNested('active_work_model', null as unknown as ConfigValue);
      void invoke('clear_work_model').catch((e) => console.warn(e));
    }
  };

  // 自动检测地理位置（Windows 系统定位优先，IP 定位兜底）
  const handleAutoDetectLocation = async () => {
    setDetectingLocation(true);
    try {
      const result = await invoke<[number, number] | null>('auto_detect_location');
      if (result) {
        const [lat, lon] = result;
        setNested('world.latitude', lat);
        setNested('world.longitude', lon);
        void emit('toast:show', { message: `(${lat.toFixed(4)}, ${lon.toFixed(4)})`, type: 'success', duration: 4000, key: Date.now() });
      } else {
        void emit('toast:show', { message: t('config.world_auto_detect_failed'), type: 'warning', duration: 4000, key: Date.now() });
      }
    } catch (e) {
      console.warn('自动定位失败:', e);
      void emit('toast:show', { message: t('config.world_auto_detect_failed'), type: 'warning', duration: 4000, key: Date.now() });
    } finally {
      setDetectingLocation(false);
    }
  };

  const loadConfig = async () => {
    try {
      const all = await invoke<ConfigObject>('get_all_config');

      setConfig(all ?? {});
    } catch (e) {
      console.warn('加载配置失败:', e);
    }
  };

  // 加载指定角色的 TTS 配置（charId 为空时回退到窗口所属角色）
  const loadTtsConfig = async (charId?: string) => {
    try {
      const id = charId ?? getCharacterId() ?? undefined;
      const tts = await invoke<TtsConfigState>('get_tts_config', {
        characterId: id,
      });
      const normalized = {
        ...tts,
        gpt_sovits_dual_instance: tts.gpt_sovits_dual_instance ?? false,
        gpt_sovits_second_port: tts.gpt_sovits_second_port ?? 9881,
        fish_speech_auto_start: tts.fish_speech_auto_start ?? false,
        fish_speech_half: tts.fish_speech_half ?? false,
        fish_speech_compile: tts.fish_speech_compile ?? false,
      };
      setTtsConfig(normalized);
      // 加载后作为初始草稿存入，确保未编辑时草稿与持久化配置一致
      if (id) {
        setTtsDrafts((prev) => ({ ...prev, [id]: normalized }));
      }
      // 如果当前引擎是 EdgeTTS，加载语音列表
      if (tts.engine === 'edgetts') {
        await loadEdgeTtsVoices(id);
      }
    } catch (e) {
      console.warn('加载 TTS 配置失败:', e);
      setTtsConfig(null);
    }
  };

  const loadEdgeTtsVoices = async (charId?: string) => {
    try {
      const voices = await invoke<Array<{ id: string; name: string; language: string }>>(
        'list_tts_voices',
        { characterId: charId ?? undefined }
      );
      setEdgeTtsVoices(voices ?? []);
    } catch (e) {
      console.warn('加载 EdgeTTS 语音列表失败:', e);
      setEdgeTtsVoices([]);
    }
  };

  // 切换语音页签正在编辑的角色：保存当前编辑内容到草稿，优先从草稿恢复目标角色
  const switchTtsEditChar = useCallback(async (charId: string) => {
    if (charId === ttsEditCharId) return;
    // 1. 将当前角色未保存的编辑内容存入草稿
    if (ttsEditCharId && ttsConfig) {
      setTtsDrafts((prev) => ({ ...prev, [ttsEditCharId]: ttsConfig }));
    }
    setTtsEditCharId(charId);
    // 2. 优先使用草稿，没有草稿才从后端加载
    const draft = ttsDrafts[charId];
    if (draft) {
      setTtsConfig(draft);
    } else {
      await loadTtsConfig(charId);
    }
  }, [ttsEditCharId, ttsConfig, ttsDrafts]);

  // 拉取 GPT-SoVITS 服务状态(单次)
  // 记录上一次的服务状态,用于检测 Running → Crashed 的变化
  const prevServiceStatusRef = useRef<string | null>(null);

  const refreshGptsovitsService = useCallback(async () => {
    try {
      const st = await invoke<GptSoVitsServiceState>('get_gpt_sovits_service_status');
      setGptsovitsService(st);
      const prev = prevServiceStatusRef.current;
      // 启动成功：从 starting 变 running 时提示
      if (st.status === 'running' && prev === 'starting') {
        void emit('toast:show', {
          message: t('config.toast_gptsovits_started'),
          type: 'success',
          duration: 3000,
          key: Date.now(),
        });
      }
      // 启动失败/运行中崩溃：进入 crashed 时显示错误
      if (st.status === 'crashed' && prev !== 'crashed' && st.error) {
        void emit('toast:show', {
          message: `${t('config.toast_gptsovits_crashed')}: ${st.error}`,
          type: 'error',
          duration: 8000,
          key: Date.now(),
        });
      }
      prevServiceStatusRef.current = st.status;
    } catch (e) {
      console.warn('查询 GPT-SoVITS 服务状态失败:', e);
    }
  }, [t]);

  // 引擎为 gptsovits 时定时轮询服务状态(2s 一次),停止/启动中时也持续轮询直到稳定
  useEffect(() => {
    if (ttsConfig?.engine !== 'gptsovits') {
      setGptsovitsService(null);
      return;
    }
    refreshGptsovitsService();
    const id = window.setInterval(refreshGptsovitsService, 2000);
    return () => window.clearInterval(id);
  }, [ttsConfig?.engine, refreshGptsovitsService]);

  // 同步当前编辑角色的 TTS 引擎到 charTtsEngines，用于双实例开关显示条件判断
  useEffect(() => {
    if (ttsEditCharId && ttsConfig) {
      setCharTtsEngines((prev) => {
        if (prev[ttsEditCharId] === ttsConfig.engine) return prev;
        return { ...prev, [ttsEditCharId]: ttsConfig.engine };
      });
    }
  }, [ttsConfig?.engine, ttsEditCharId]);

  // 当不再满足"所有角色都用 GPT-SoVITS"条件时，自动关闭双实例开关
  useEffect(() => {
    if (!ttsConfig) return;
    const allUseGptsovits = characters.length >= 2
      && characters.every((c) => charTtsEngines[c.id] === 'gptsovits');
    if (!allUseGptsovits && ttsConfig.gpt_sovits_dual_instance) {
      setTtsConfig((prev) => prev ? {
        ...prev,
        gpt_sovits_dual_instance: false,
        gpt_sovits_second_port: null,
      } : prev);
    }
  }, [charTtsEngines, characters.length]);

  // 安装路径变化时刷新模型列表，扫描结果为空或缺失 runtime 时通过 toast 提示
  useEffect(() => {
    if (ttsConfig?.engine !== 'gptsovits' || !ttsConfig?.gpt_sovits_install_path) {
      setGptSovitsModels({ gpt_models: [], sovits_models: [], has_runtime: false });
      return;
    }
    let cancelled = false;
    const installPath = ttsConfig.gpt_sovits_install_path;
    invoke<{
      gpt_models: Array<{ name: string; path: string }>;
      sovits_models: Array<{ name: string; path: string }>;
      has_runtime: boolean;
    }>('list_gpt_sovits_models', { installPath })
      .then((res) => {
        if (cancelled) return;
        setGptSovitsModels(res);
        const total = res.gpt_models.length + res.sovits_models.length;
        if (total === 0) {
          void emit('toast:show', {
            message: t('config.gptsovits_toast_no_models'),
            type: 'warning',
            duration: 5000,
            key: Date.now(),
          });
        }
        if (!res.has_runtime && !ttsConfig.gpt_sovits_python_path) {
          void emit('toast:show', {
            message: t('config.gptsovits_toast_no_runtime'),
            type: 'warning',
            duration: 5000,
            key: Date.now() + 1,
          });
        }
      })
      .catch((e) => {
        if (cancelled) return;
        setGptSovitsModels({ gpt_models: [], sovits_models: [], has_runtime: false });
        void emit('toast:show', {
          message: `${t('config.gptsovits_toast_scan_failed')}: ${e}`,
          type: 'error',
          duration: 5000,
          key: Date.now(),
        });
      });
    return () => {
      cancelled = true;
    };
  }, [ttsConfig?.engine, ttsConfig?.gpt_sovits_install_path, ttsConfig?.gpt_sovits_python_path]);

  // 一键启动 / 停止 GPT-SoVITS 服务
  const toggleGptsovitsService = async () => {
    if (gptsovitsServiceBusy) return;
    const cur = gptsovitsService?.status;
    if (cur === 'running' || cur === 'starting') {
      // 停止
      setGptsovitsServiceBusy(true);
      try {
        const st = await invoke<GptSoVitsServiceState>('stop_gpt_sovits_service');
        setGptsovitsService(st);
      } catch (e) {
        console.error('停止 GPT-SoVITS 服务失败:', e);
      } finally {
        setGptsovitsServiceBusy(false);
      }
    } else {
      // 启动前自动补齐字段：python 路径、端口默认值、服务 URL
      if (ttsConfig) {
        const port = ttsConfig.gpt_sovits_port ?? 9880;
        const dualInstance = ttsConfig.gpt_sovits_dual_instance ?? false;
        const secondPort = ttsConfig.gpt_sovits_second_port ?? 9881;
        const patched: TtsConfigState = {
          ...ttsConfig,
          gpt_sovits_port: port,
          gpt_sovits_dual_instance: dualInstance,
          gpt_sovits_second_port: dualInstance ? secondPort : null,
          // 若用户未填 Python 路径，且安装目录下检测到 runtime/python.exe，则自动补齐
          gpt_sovits_python_path:
            ttsConfig.gpt_sovits_python_path ||
            (gptSovitsModels.has_runtime && ttsConfig.gpt_sovits_install_path
              ? `${ttsConfig.gpt_sovits_install_path.replace(/\\/g, '/')}/runtime/python.exe`
              : null),
          // 服务 URL 自动指向本地端口（仅当用户未填时）
          gpt_sovits_url:
            ttsConfig.gpt_sovits_url || `http://127.0.0.1:${port}`,
        };
        setTtsConfig(patched);
        setGptsovitsServiceBusy(true);
        try {
          await invoke('set_tts_config', { config: patched, characterId: ttsEditCharId ?? undefined });
          // 模型列表由 install_path/python_path 变化触发的 useEffect 自动刷新，此处无需重复调用
          const st = await invoke<GptSoVitsServiceState>('start_gpt_sovits_service', { characterId: ttsEditCharId ?? undefined });
          setGptsovitsService(st);
          // 同步 prevServiceStatusRef 为 starting，让后续轮询能正确检测 starting → running 转换并弹 toast
          prevServiceStatusRef.current = st.status;
          // 启动是异步的（后端 spawn wait_for_health），立即再刷新一次拉取最新状态
          await refreshGptsovitsService();
        } catch (e) {
          console.error('启动 GPT-SoVITS 服务失败:', e);
          void emit('toast:show', {
            message: `${t('config.toast_gptsovits_start_failed')}: ${e}`,
            type: 'error',
            duration: 6000,
            key: Date.now(),
          });
          // 立即刷新状态以读取后端 error 信息
          await refreshGptsovitsService();
        } finally {
          setGptsovitsServiceBusy(false);
        }
      }
    }
  };

  // ── Fish Speech 本地 TTS 服务管理 ─────────────────────────
  // 拉取 Fish Speech 服务状态(单次)
  const prevFishServiceStatusRef = useRef<string | null>(null);

  const refreshFishSpeechService = useCallback(async () => {
    try {
      const st = await invoke<FishSpeechServiceState>('get_fish_speech_service_status');
      setFishSpeechService(st);
      const prev = prevFishServiceStatusRef.current;
      if (st.status === 'running' && prev === 'starting') {
        void emit('toast:show', {
          message: t('config.toast_fishspeech_started'),
          type: 'success',
          duration: 3000,
          key: Date.now(),
        });
      }
      if (st.status === 'crashed' && prev !== 'crashed' && st.error) {
        void emit('toast:show', {
          message: `${t('config.toast_fishspeech_crashed')}: ${st.error}`,
          type: 'error',
          duration: 8000,
          key: Date.now(),
        });
      }
      prevFishServiceStatusRef.current = st.status;
    } catch (e) {
      console.warn('查询 Fish Speech 服务状态失败:', e);
    }
  }, [t]);

  // 引擎为 fishspeech 时定时轮询服务状态(2s 一次)
  useEffect(() => {
    if (ttsConfig?.engine !== 'fishspeech') {
      setFishSpeechService(null);
      return;
    }
    refreshFishSpeechService();
    const id = window.setInterval(refreshFishSpeechService, 2000);
    return () => window.clearInterval(id);
  }, [ttsConfig?.engine, refreshFishSpeechService]);

  // 一键启动 / 停止 Fish Speech 服务
  const toggleFishSpeechService = async () => {
    if (fishSpeechServiceBusy) return;
    const cur = fishSpeechService?.status;
    if (cur === 'running' || cur === 'starting') {
      setFishSpeechServiceBusy(true);
      try {
        const st = await invoke<FishSpeechServiceState>('stop_fish_speech_service');
        setFishSpeechService(st);
      } catch (e) {
        console.error('停止 Fish Speech 服务失败:', e);
      } finally {
        setFishSpeechServiceBusy(false);
      }
    } else {
      if (ttsConfig) {
        const port = ttsConfig.fish_speech_port ?? 8080;
        const patched: TtsConfigState = {
          ...ttsConfig,
          fish_speech_port: port,
          // 服务 URL 自动指向本地端口（仅当用户未填时）
          fish_speech_url:
            ttsConfig.fish_speech_url || `http://127.0.0.1:${port}`,
        };
        setTtsConfig(patched);
        setFishSpeechServiceBusy(true);
        try {
          await invoke('set_tts_config', { config: patched, characterId: ttsEditCharId ?? undefined });
          const st = await invoke<FishSpeechServiceState>('start_fish_speech_service', { characterId: ttsEditCharId ?? undefined });
          setFishSpeechService(st);
          prevFishServiceStatusRef.current = st.status;
          await refreshFishSpeechService();
        } catch (e) {
          console.error('启动 Fish Speech 服务失败:', e);
          void emit('toast:show', {
            message: `${t('config.toast_fishspeech_start_failed')}: ${e}`,
            type: 'error',
            duration: 6000,
            key: Date.now(),
          });
          await refreshFishSpeechService();
        } finally {
          setFishSpeechServiceBusy(false);
        }
      }
    }
  };

  // ── Ollama 本地嵌入服务管理 ──────────────────────────────
  // 拉取 Ollama 服务状态(单次)
  const refreshOllamaService = useCallback(async () => {
    try {
      const st = await invoke<OllamaServiceState>('get_ollama_status');
      setOllamaService(st);
    } catch (e) {
      console.warn('查询 Ollama 服务状态失败:', e);
    }
  }, []);

  // 刷新已安装的 Ollama 模型列表
  const refreshOllamaModels = useCallback(async () => {
    try {
      const res = await invoke<{ models: string[] }>('list_ollama_models');
      setOllamaModels(res.models ?? []);
    } catch (e) {
      console.warn('查询 Ollama 模型列表失败:', e);
    }
  }, []);

  // 拉取内置嵌入模型注册表（用于云端模型选择时自动填充维度）
  const refreshEmbeddingModels = useCallback(async () => {
    try {
      const models = await invoke<{ id: string; dimension: number; source: string }[]>('get_embedding_models');
      setEmbeddingModels(Array.isArray(models) ? models : []);
    } catch (e) {
      console.warn('查询嵌入模型注册表失败:', e);
    }
  }, []);

  // 挂载时拉取一次嵌入模型注册表
  useEffect(() => {
    refreshEmbeddingModels();
  }, [refreshEmbeddingModels]);

  // 嵌入来源为 local 且处于记忆页签时定时轮询服务状态(2s 一次)
  useEffect(() => {
    const isLocal = get<string>('memory.embedding.source', 'cloud') === 'local';
    if (activeTab !== 'memory' || !isLocal) {
      return;
    }
    refreshOllamaService();
    refreshOllamaModels();
    const id = window.setInterval(refreshOllamaService, 2000);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTab, config, refreshOllamaService, refreshOllamaModels]);

  // 监听 ollama:ready 事件：启动自动拉取完成后刷新模型列表，
  // 使面板状态从"未加载"更新为"已加载"
  useEffect(() => {
    const unlisten = listen<{ model_installed: boolean; model: string | null }>(
      'ollama:ready',
      () => {
        refreshOllamaModels();
        refreshOllamaService();
      },
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [refreshOllamaModels, refreshOllamaService]);

  // 一键启动 / 停止 Ollama 服务
  const toggleOllamaService = async () => {
    if (ollamaServiceBusy) return;
    const cur = ollamaService?.status;
    setOllamaServiceBusy(true);
    try {
      if (cur === 'running' || cur === 'starting') {
        const st = await invoke<OllamaServiceState>('stop_ollama');
        setOllamaService(st);
      } else {
        const st = await invoke<OllamaServiceState>('start_ollama');
        setOllamaService(st);
        await refreshOllamaService();
      }
    } catch (e) {
      console.error('切换 Ollama 服务失败:', e);
      void emit('toast:show', {
        message: `${t('config.toast_ollama_toggle_failed')}: ${e}`,
        type: 'error',
        duration: 6000,
        key: Date.now(),
      });
      await refreshOllamaService();
    } finally {
      setOllamaServiceBusy(false);
    }
  };

  // ── Whisper 本地 ASR 服务管理 ──────────────────────────────
  // 拉取 Whisper 服务状态(单次)
  const prevWhisperStatusRef = useRef<string | null>(null);
  const refreshWhisperService = useCallback(async () => {
    try {
      const st = await invoke<WhisperServiceState>('get_whisper_service_status');
      setWhisperService(st);
      const prev = prevWhisperStatusRef.current;
      // starting → running: 启动成功提示
      if (st.status === 'running' && prev === 'starting') {
        void emit('toast:show', {
          message: t('config.toast_whisper_started'),
          type: 'success',
          duration: 3000,
          key: Date.now(),
        });
      }
      // 进入 crashed: 显示错误
      if (st.status === 'crashed' && prev !== 'crashed' && st.error) {
        void emit('toast:show', {
          message: `${t('config.toast_whisper_crashed')}: ${st.error}`,
          type: 'error',
          duration: 8000,
          key: Date.now(),
        });
      }
      prevWhisperStatusRef.current = st.status;
    } catch (e) {
      console.warn('查询 Whisper 服务状态失败:', e);
    }
  }, [t]);

  // 引擎为 whisper 时定时轮询服务状态(2s 一次)
  useEffect(() => {
    if (get<string>('speech_recognition.engine', 'winrt') !== 'whisper') {
      setWhisperService(null);
      prevWhisperStatusRef.current = null;
      return;
    }
    refreshWhisperService();
    const id = window.setInterval(refreshWhisperService, 2000);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [config, refreshWhisperService]);

  // 一键启动 / 停止 Whisper 服务
  const toggleWhisperService = async () => {
    if (whisperServiceBusy) return;
    const cur = whisperService?.status;
    if (cur === 'installing') return; // 安装中不响应点击
    if (cur === 'running' || cur === 'starting') {
      // 停止
      setWhisperServiceBusy(true);
      try {
        const st = await invoke<WhisperServiceState>('stop_whisper_service');
        setWhisperService(st);
        prevWhisperStatusRef.current = st.status;
      } catch (e) {
        console.error('停止 Whisper 服务失败:', e);
      } finally {
        setWhisperServiceBusy(false);
      }
    } else {
      // 启动：仅持久化 whisper service_* 字段，不触发整体保存/重初始化/关闭窗口
      setWhisperServiceBusy(true);
      try {
        await persistWhisperServiceConfig();
        const st = await invoke<WhisperServiceState>('start_whisper_service');
        setWhisperService(st);
        prevWhisperStatusRef.current = st.status;
        // 启动是异步的（后端 spawn wait_for_health），立即再刷新一次拉取最新状态
        await refreshWhisperService();
      } catch (e) {
        console.error('启动 Whisper 服务失败:', e);
        void emit('toast:show', {
          message: `${t('config.toast_whisper_start_failed')}: ${e}`,
          type: 'error',
          duration: 6000,
          key: Date.now(),
        });
        await refreshWhisperService();
      } finally {
        setWhisperServiceBusy(false);
      }
    }
  };

  // 仅持久化 whisper service_* 配置项到后端（不关闭窗口、不触发 reinitialize、不弹保存 toast）
  // 目的：让 start_whisper_service 命令读到表单中的最新值，避免触发整体保存副作用
  const persistWhisperServiceConfig = async () => {
    const fields: Array<[string, ConfigValue]> = [
      ['speech_recognition.whisper.service_model', get('speech_recognition.whisper.service_model', 'small')],
      ['speech_recognition.whisper.service_device', get('speech_recognition.whisper.service_device', 'auto')],
      ['speech_recognition.whisper.service_compute_type', get('speech_recognition.whisper.service_compute_type', 'auto')],
      ['speech_recognition.whisper.service_port', get('speech_recognition.whisper.service_port', 8000)],
      ['speech_recognition.whisper.service_python_path', get('speech_recognition.whisper.service_python_path', '') ?? ''],
      ['speech_recognition.whisper.service_install_path', get('speech_recognition.whisper.service_install_path', '') ?? ''],
      ['speech_recognition.whisper.service_auto_start', get('speech_recognition.whisper.service_auto_start', false)],
    ];
    for (const [key, value] of fields) {
      try {
        await invoke('set_config', { key, value });
      } catch (e) {
        console.warn(`写入 ${key} 失败:`, e);
      }
    }
    try {
      await invoke('save_config');
    } catch (e) {
      console.warn('save_config 失败:', e);
    }
  };

  // 拉取 Ollama 模型；权限不足时直接触发 UAC 提权修复目录后重试
  const pullOllamaModel = async () => {
    const model = get('memory.embedding.ollama_model', 'bge-m3');
    if (!model.trim() || ollamaPulling) return;
    setOllamaPulling(true);
    const doPull = async () =>
      invoke<{ success: boolean; error: string | null; permission_denied: boolean }>(
        'pull_ollama_model',
        { model },
      );
    try {
      const res = await doPull();
      if (res.success) {
        void emit('toast:show', {
          message: t('config.toast_ollama_pull_ok'),
          type: 'success',
          duration: 3000,
          key: Date.now(),
        });
        await refreshOllamaModels();
      } else if (res.permission_denied) {
        // 权限不足：直接触发 UAC（UAC 本身即权限确认），无需前置 window.confirm
        void emit('toast:show', {
          message: t('config.toast_ollama_fix_requesting'),
          type: 'info',
          duration: 4000,
          key: Date.now(),
        });
        try {
          await invoke('fix_ollama_permission');
          const retry = await doPull();
          if (retry.success) {
            void emit('toast:show', {
              message: t('config.toast_ollama_pull_ok'),
              type: 'success',
              duration: 3000,
              key: Date.now(),
            });
            await refreshOllamaModels();
          } else {
            void emit('toast:show', {
              message: `${t('config.toast_ollama_pull_failed')}: ${retry.error ?? ''}`,
              type: 'error',
              duration: 6000,
              key: Date.now(),
            });
          }
        } catch (fe) {
          // UAC 被取消或修复失败
          void emit('toast:show', {
            message: `${t('config.toast_ollama_fix_failed')}: ${fe}`,
            type: 'error',
            duration: 6000,
            key: Date.now(),
          });
        }
      } else {
        void emit('toast:show', {
          message: `${t('config.toast_ollama_pull_failed')}: ${res.error ?? ''}`,
          type: 'error',
          duration: 6000,
          key: Date.now(),
        });
      }
    } catch (e) {
      void emit('toast:show', {
        message: `${t('config.toast_ollama_pull_failed')}: ${e}`,
        type: 'error',
        duration: 6000,
        key: Date.now(),
      });
    } finally {
      setOllamaPulling(false);
    }
  };

  // 选择本地路径(文件夹或文件)
  const pickPath = async (
    field: keyof TtsConfigState,
    isDirectory: boolean,
    extensions?: string[],
  ) => {
    try {
      const selected = await open({
        directory: isDirectory,
        multiple: false,
        filters: extensions ? [{ name: '', extensions }] : undefined,
      });
      if (typeof selected === 'string' && selected) {
        setTtsConfig((prev) => (prev ? { ...prev, [field]: selected } : prev));
      }
    } catch (e) {
      console.warn('选择路径失败:', e);
    }
  };

  // 加载日记配置（通过 get_diary_config 命令读取当前角色的 diary/config.json）
  const loadDiaryConfig = async () => {
    setDiaryLoading(true);
    try {
      const cfg = await invoke<DiaryConfigState>('get_diary_config', {
        characterId: getCharacterId() ?? undefined,
      });
      setDiaryConfig(cfg);
    } catch (e) {
      console.warn('加载日记配置失败:', e);
    } finally {
      setDiaryLoading(false);
    }
  };

  useEffect(() => {
    // 后端启动预检要求展示配置说明（URL 带 guide=1）
    if (new URLSearchParams(window.location.search).get('guide') === '1') {
      setSetupGuideOpen(true);
    }
    // 主 LLM 未配置时作为兜底：即使已看过指引仍弹出
    void invoke<boolean>('is_main_api_configured')
      .then((ok) => {
        if (!ok) setSetupGuideOpen(true);
      })
      .catch(() => {});
    // 加载配置 + 角色列表 + TTS 配置；完成后按需弹出配置指引（等数据就绪，状态徽章才准确）
    void (async () => {
      try {
        await loadConfig();
        const resp = await invoke<{ characters: Array<{ id: string; name: string; online: boolean }> }>('list_characters');
        const list = resp?.characters ?? [];
        setCharacters(list);
        const winCharId = getCharacterId();
        const initial = winCharId && list.some(c => c.id === winCharId)
          ? winCharId
          : (list[0]?.id ?? null);
        setTtsEditCharId(initial);
        await loadTtsConfig(initial ?? undefined);
        // 加载所有角色的 TTS 引擎（用于判断双实例开关是否显示）
        const engines: Record<string, string> = {};
        for (const c of list) {
          try {
            const cfg = await invoke<TtsConfigState>('get_tts_config', { characterId: c.id });
            engines[c.id] = cfg.engine;
          } catch {
            engines[c.id] = 'none';
          }
        }
        setCharTtsEngines(engines);
      } catch (e) {
        console.warn('加载角色列表失败:', e);
        await loadTtsConfig();
      } finally {
        // 首次打开设置窗口时自动弹出配置指引
        if (!localStorage.getItem('vivian-setup-guide-seen')) {
          localStorage.setItem('vivian-setup-guide-seen', '1');
          setSetupGuideOpen(true);
        }
      }
    })();
    void loadDiaryConfig();
    void getVersion().then(setAppVersion).catch(() => { /* 忽略 */ });
    void Promise.all([platform(), osVersion(), arch()])
      .then(([p, v, a]) => setOsInfo(`${p} ${v} (${a})`))
      .catch(() => { /* 忽略 */ });
  }, []);

  // 后端启动预检未通过时，若设置窗口已打开也立即弹出配置说明
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      unlisten = await listen('setup-guide:show', () => {
        setSetupGuideOpen(true);
      });
    })();
    return () => { unlisten?.(); };
  }, []);

  // 切换到工具页签时加载 MCP server 列表与注册工具清单
  useEffect(() => {
    if (activeTab === 'tools') {
      invoke<Array<{ id: string; name: string; enabled: boolean; tool_count: number; alive: boolean }>>('list_mcp_servers')
        .then(setMcpServers)
        .catch(() => { /* 忽略 */ });
      invoke<{ tools: Array<{ name: string; description: string; category: string; is_custom: boolean }> }>('list_tools')
        .then((res) => setToolList(res?.tools ?? []))
        .catch(() => { /* 忽略 */ });
    }
  }, [activeTab]);

  // 监听主窗口语言切换事件，实时同步本窗口的 i18n 语言
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ language: string }>('config:language-changed', (e) => {
          if (e.payload?.language) void changeLanguage(e.payload.language);
        });
        if (cancelled) { unlisten(); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // 主题：读取 base.theme 配置设置根节点 data-theme，并监听实时变更
  useEffect(() => {
    const applyTheme = (theme: string | null | undefined) => {
      document.documentElement.setAttribute('data-theme', theme === 'light' || theme === 'dark' ? theme : 'system');
    };
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const theme = await invoke<string | null>('get_config', { key: 'base.theme' });
        if (!cancelled) applyTheme(theme);
        unlisten = await listen<{ theme: string }>('config:theme-changed', (e) => {
          applyTheme(e.payload?.theme);
        });
        if (cancelled) unlisten();
      } catch { /* ignore */ }
    })();
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  // 标识本窗口刚刚触发了 config:saved（用于跳过自身保存引发的事件，避免循环重载）
  const selfSaveRef = useRef(false);
  // 监听其他窗口（右键菜单 / 系统托盘菜单）保存配置后触发的 config:saved 事件，
  // 实时同步智能避让开关显示，避免本窗口显示的旧草稿与最新落盘值不一致。
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen('config:saved', async () => {
          if (selfSaveRef.current) {
            selfSaveRef.current = false;
            return;
          }
          try {
            const enabled = await invoke<boolean>('get_config', {
              key: 'window.smart_positioning_enabled',
            }).catch(() => true);
            setNested('window.smart_positioning_enabled', enabled);
          } catch {
            /* ignore */
          }
        });
        if (cancelled) { unlisten(); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // 监听路由矩阵任务状态事件（任务专属 provider 调用成功/失败）
  // 仅用于 UI 颜色标记：失败时模型名变红，下次成功时恢复绿色
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ task_type: string; status: 'ok' | 'error' }>(
          'chat:route_status',
          (event) => {
            const { task_type: taskType, status } = event.payload ?? {};
            if (!taskType || !status) return;
            setRouteStatus((prev) => {
              if (prev[taskType] === status) return prev;
              return { ...prev, [taskType]: status };
            });
          },
        );
        if (cancelled) { unlisten(); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  const handleTestTts = async () => {
    try {
      if (!ttsConfig) return;
      // GPT-SoVITS 引擎:试听前检查服务地址与服务运行状态
      if (ttsConfig.engine === 'gptsovits') {
        if (!ttsConfig.gpt_sovits_url) {
          void emit('toast:show', {
            message: t('config.toast_gptsovits_test_no_url'),
            type: 'warning',
            duration: 4000,
            key: Date.now(),
          });
          return;
        }
        if (gptsovitsService?.status !== 'running') {
          void emit('toast:show', {
            message: t('config.toast_gptsovits_test_not_running'),
            type: 'warning',
            duration: 4000,
            key: Date.now(),
          });
          return;
        }
      }
      // 先持久化当前编辑中的配置，确保后端使用最新选中的引擎而非旧配置
      await invoke('set_tts_config', { config: ttsConfig, characterId: ttsEditCharId ?? undefined });
      // 试听文案随界面语言切换，以便检验目标语种语音效果
      const lang = i18n.language;
      const charName = characters.find((c) => c.id === ttsEditCharId)?.name ?? 'Vivian';
      const sampleText =
        lang?.startsWith('en')
          ? `Hello, I'm ${charName}. Nice to meet you!`
          : lang?.startsWith('ja')
            ? `こんにちは、私は${charName}です。お会いできて嬉しいです〜`
            : `你好，我是${charName}，很高兴见到你。`;
      await invoke('speak_text', { text: sampleText, characterId: ttsEditCharId ?? undefined });
    } catch (e) {
      console.warn('TTS 测试失败:', e);
      void emit('toast:show', {
        message: t('toast.tts_test_failed', { error: String(e) }),
        type: 'error',
        duration: 5000,
        key: Date.now(),
      });
    }
  };

  // 打开 TTS 帮助说明书，并定位到指定后端页签
  const openTtsHelp = (backend: TtsBackendKey) => {
    setTtsHelpBackend(backend);
    setTtsHelpOpen(true);
  };

  // 打开 ASR 帮助说明书，并定位到指定后端页签
  const openAsrHelp = (backend: AsrBackendKey) => {
    setAsrHelpBackend(backend);
    setAsrHelpOpen(true);
  };

  const handleLanguageChange = async (lang: string) => {
    setNested('base.language', lang);
    void changeLanguage(lang);
    try {
      await invoke('set_config', { key: 'base.language', value: lang });
      await invoke('save_config');
      await emit('config:language-changed', { language: lang });
    } catch (e) {
      console.warn('即时保存语言设置失败:', e);
    }
  };

  const handleThemeChange = async (theme: string) => {
    setNested('base.theme', theme);
    try {
      await invoke('set_config', { key: 'base.theme', value: theme });
      await invoke('save_config');
      await emit('config:theme-changed', { theme });
    } catch (e) {
      console.warn('即时保存主题设置失败:', e);
    }
  };


  /** 快捷键变化处理：即时保存 + 通知后端重新注册所有文字快捷键
   *  冲突检测已在 ShortcutRecorder 内部完成（通过 register/unregister 试探），
   *  此处仅负责持久化与事件通知。返回 ConflictResult 让组件决定是否显示错误。 */
  const handleShortcutChange = useCallback(async (shortcut: string): Promise<ConflictResult> => {
    setNested('base.shortcut', shortcut);
    try {
      await invoke('set_config', { key: 'base.shortcut', value: shortcut });
      await invoke('save_config');
      // 通知后端重新注册所有文字快捷键
      await invoke('update_text_shortcuts');
      void emit('toast:show', {
        message: shortcut
          ? t('toast.shortcut_applied', { shortcut: formatForDisplay(shortcut) })
          : t('config.shortcut_recorder_idle'),
        type: 'success',
        duration: 4000,
        key: Date.now(),
      });
      return { ok: true };
    } catch (e) {
      void emit('toast:show', {
        message: t('toast.shortcut_register_failed', { shortcut: formatForDisplay(shortcut) }),
        type: 'error',
        duration: 4000,
        key: Date.now(),
      });
      return { ok: false, reason: 'conflict' };
    }
  }, [t]);

  /** 恢复出厂设置确认：调用后端原子化命令，锁死行为 → 清空数据 → 重启应用 */
  const handleClearMemoriesConfirm = useCallback(async () => {
    setClearingMemories(true);
    try {
      // 后端 factory_reset 命令会：
      // 1. 立即锁死所有桌面宠物行为（tick 命令拒绝执行）
      // 2. 停止所有后台子系统（proactive / scheduler / speech / activity_journal / pet_controller）
      // 3. 清空所有角色记忆 + 共同记忆
      // 4. 写入清扫标记并重启整个应用；重启后在 AppState 构造前按保留清单
      //    删除用户数据目录中的全部使用期数据与自进化内容（含 screenshots / images /
      //    skills / plugins / mcp 等），配置 / 凭据保留
      // 命令返回即意味着即将重启，前端无需后续处理
      await invoke('factory_reset');
    } catch (e) {
      void emit('toast:show', {
        message: String(e),
        type: 'error',
        duration: 4000,
        key: Date.now(),
      });
      setClearingMemories(false);
      setClearMemoriesOpen(false);
    }
  }, [t]);

  /** 一键备份：选择保存位置 → 后端复制用户数据目录（记忆/自进化/心理/配置等）到时间戳备份文件夹 */
  const handleBackup = useCallback(async () => {
    const dest = await open({ directory: true, multiple: false, title: t('config.backup_pick_folder') });
    if (!dest || typeof dest !== 'string') return;
    setBackingUp(true);
    try {
      const backupPath = await invoke<string>('backup_user_data', { destDir: dest });
      void emit('toast:show', {
        message: `${t('config.backup_done')}: ${backupPath}`,
        type: 'success',
        duration: 6000,
        key: Date.now(),
      });
    } catch (e) {
      void emit('toast:show', {
        message: `${t('config.backup_failed')}: ${String(e)}`,
        type: 'error',
        duration: 6000,
        key: Date.now(),
      });
    } finally {
      setBackingUp(false);
    }
  }, [t]);

  /** 恢复备份第一步：选择 .altn 备份文件（后端校验），通过后弹确认框 */
  const handleRestorePick = useCallback(async () => {
    const selected = await open({
      multiple: false,
      title: t('config.restore_pick_file'),
      filters: [
        { name: t('config.backup_file_type'), extensions: ['altn'] },
        { name: t('config.backup_all_files'), extensions: ['*'] },
      ],
    });
    if (!selected || typeof selected !== 'string') return;
    setRestoreSource(selected);
    setRestoreConfirmOpen(true);
  }, [t]);

  /** 恢复备份确认：后端写入恢复标记并自动重启应用，重启后完成数据回填 */
  const handleRestoreConfirm = useCallback(async () => {
    if (!restoreSource) return;
    setRestoring(true);
    try {
      // 后端 restore_user_data 会校验备份文件、写入恢复标记并重启应用
      await invoke('restore_user_data', { backupPath: restoreSource });
      void emit('toast:show', {
        message: t('config.restore_started'),
        type: 'success',
        duration: 4000,
        key: Date.now(),
      });
    } catch (e) {
      void emit('toast:show', {
        message: `${t('config.restore_failed')}: ${String(e)}`,
        type: 'error',
        duration: 6000,
        key: Date.now(),
      });
      setRestoring(false);
      setRestoreConfirmOpen(false);
    }
  }, [restoreSource, t]);

  /** Nana 私聊快捷键变化处理：保存配置 + 通知后端重新注册所有文字快捷键 */
  const handleNanaShortcutChange = useCallback(async (shortcut: string): Promise<ConflictResult> => {
    setNested('base.shortcut_nana', shortcut);
    try {
      await invoke('set_config', { key: 'base.shortcut_nana', value: shortcut });
      await invoke('save_config');
      // 通知后端重新注册所有文字快捷键
      await invoke('update_text_shortcuts');
      void emit('toast:show', {
        message: shortcut
          ? t('toast.shortcut_applied', { shortcut: formatForDisplay(shortcut) })
          : t('config.shortcut_recorder_idle'),
        type: 'success',
        duration: 4000,
        key: Date.now(),
      });
      return { ok: true };
    } catch (e) {
      void emit('toast:show', {
        message: t('toast.shortcut_register_failed', { shortcut: formatForDisplay(shortcut) }),
        type: 'error',
        duration: 4000,
        key: Date.now(),
      });
      return { ok: false, reason: 'conflict' };
    }
  }, [t]);

  /** 群发快捷键变化处理：保存配置 + 通知后端重新注册所有文字快捷键 */
  const handleBroadcastShortcutChange = useCallback(async (shortcut: string): Promise<ConflictResult> => {
    setNested('base.shortcut_broadcast', shortcut);
    try {
      await invoke('set_config', { key: 'base.shortcut_broadcast', value: shortcut });
      await invoke('save_config');
      await invoke('update_text_shortcuts');
      void emit('toast:show', {
        message: shortcut
          ? t('toast.shortcut_applied', { shortcut: formatForDisplay(shortcut) })
          : t('config.shortcut_recorder_idle'),
        type: 'success',
        duration: 4000,
        key: Date.now(),
      });
      return { ok: true };
    } catch (e) {
      void emit('toast:show', {
        message: t('toast.shortcut_register_failed', { shortcut: formatForDisplay(shortcut) }),
        type: 'error',
        duration: 4000,
        key: Date.now(),
      });
      return { ok: false, reason: 'conflict' };
    }
  }, [t]);

  /** 微信快捷键变化处理 */
  const handleChatShortcutChange = useCallback(async (shortcut: string): Promise<ConflictResult> => {
    setNested('base.shortcut_chat', shortcut);
    try {
      await invoke('set_config', { key: 'base.shortcut_chat', value: shortcut });
      await invoke('save_config');
      await invoke('update_text_shortcuts');
      void emit('toast:show', {
        message: shortcut
          ? t('toast.shortcut_applied', { shortcut: formatForDisplay(shortcut) })
          : t('config.shortcut_recorder_idle'),
        type: 'success',
        duration: 4000,
        key: Date.now(),
      });
      return { ok: true };
    } catch (e) {
      void emit('toast:show', {
        message: t('toast.shortcut_register_failed', { shortcut: formatForDisplay(shortcut) }),
        type: 'error',
        duration: 4000,
        key: Date.now(),
      });
      return { ok: false, reason: 'conflict' };
    }
  }, [t]);

  /** 设置快捷键变化处理 */
  const handleSettingsShortcutChange = useCallback(async (shortcut: string): Promise<ConflictResult> => {
    setNested('base.shortcut_settings', shortcut);
    try {
      await invoke('set_config', { key: 'base.shortcut_settings', value: shortcut });
      await invoke('save_config');
      await invoke('update_text_shortcuts');
      void emit('toast:show', {
        message: shortcut
          ? t('toast.shortcut_applied', { shortcut: formatForDisplay(shortcut) })
          : t('config.shortcut_recorder_idle'),
        type: 'success',
        duration: 4000,
        key: Date.now(),
      });
      return { ok: true };
    } catch (e) {
      void emit('toast:show', {
        message: t('toast.shortcut_register_failed', { shortcut: formatForDisplay(shortcut) }),
        type: 'error',
        duration: 4000,
        key: Date.now(),
      });
      return { ok: false, reason: 'conflict' };
    }
  }, [t]);

  /** 笔记本快捷键变化处理 */
  const handleMemoryShortcutChange = useCallback(async (shortcut: string): Promise<ConflictResult> => {
    setNested('base.shortcut_memory', shortcut);
    try {
      await invoke('set_config', { key: 'base.shortcut_memory', value: shortcut });
      await invoke('save_config');
      await invoke('update_text_shortcuts');
      void emit('toast:show', {
        message: shortcut
          ? t('toast.shortcut_applied', { shortcut: formatForDisplay(shortcut) })
          : t('config.shortcut_recorder_idle'),
        type: 'success',
        duration: 4000,
        key: Date.now(),
      });
      return { ok: true };
    } catch (e) {
      void emit('toast:show', {
        message: t('toast.shortcut_register_failed', { shortcut: formatForDisplay(shortcut) }),
        type: 'error',
        duration: 4000,
        key: Date.now(),
      });
      return { ok: false, reason: 'conflict' };
    }
  }, [t]);

  // 测试网络连接 —— 通过当前网络设置访问 Google 主页验证代理可用性
  const handleTestConnection = async () => {
    setNetworkTesting(true);
    setNetworkTestResult(null);
    setNetworkTestSuccess(null);
    try {
      // 先把当前 UI 中（可能尚未保存）的网络设置同步到后端内存配置，
      // 确保手动模式下使用的是用户刚填写的代理地址，而非上次保存的值
      const proxyMode = get<string>('network.proxy_mode', 'direct');
      const proxyUrl = get<string>('network.proxy_url', '');
      const timeout = get<number>('network.timeout', 30);
      await invoke('set_config', { key: 'network.proxy_mode', value: proxyMode });
      await invoke('set_config', { key: 'network.proxy_url', value: proxyUrl });
      await invoke('set_config', { key: 'network.timeout', value: timeout });

      const result = await invoke<{
        success: boolean;
        status_code: number | null;
        elapsed_ms: number;
        proxy_mode: string;
        effective_proxy: string | null;
        error: string | null;
      }>('test_network_connection');
      if (result.success) {
        setNetworkTestSuccess(true);
        setNetworkTestResult(
          t('config.test_connection_success', {
            status: result.status_code ?? 0,
            elapsed: result.elapsed_ms,
          })
        );
      } else {
        setNetworkTestSuccess(false);
        setNetworkTestResult(
          t('config.test_connection_failed', { error: result.error ?? 'Unknown' })
        );
      }
    } catch (e) {
      setNetworkTestSuccess(false);
      setNetworkTestResult(
        t('config.test_connection_failed', { error: String(e) })
      );
    } finally {
      setNetworkTesting(false);
    }
  };

  // ===== LLM 一键检测 =====
  // 收集测试目标：主 LLM 配置 + 工作智能体模型预置 + 路由矩阵全部任务
  // （读取当前 UI 值，含未保存的修改）
  const collectLlmTestTargets = () => {
    const s = (path: string, fallback = '') => ((get(path, fallback) as string) ?? '');
    return [
      {
        key: 'main',
        label: t('config.llm_test_main_label'),
        providerType: s('ai.provider', 'openai') || 'openai',
        model: s('ai.model').trim(),
        apiKey: s('ai.api_key').trim(),
        endpoint: s('ai.endpoint').trim(),
        apiSecret: s('ai.api_secret'),
        appId: s('ai.app_id'),
      },
      ...workModels.map((m) => ({
        key: `work:${m.id}`,
        label: `${t('config.llm_test_work_model_label')}${m.name}`,
        providerType: m.provider_type || 'openai',
        model: (m.model ?? '').trim(),
        apiKey: (m.api_key ?? '').trim(),
        endpoint: (m.endpoint ?? '').trim(),
        apiSecret: m.api_secret ?? '',
        appId: m.app_id ?? '',
      })),
      ...ROUTING_TASKS.map((task) => ({
        key: task.taskType,
        label: t(task.labelKey),
        providerType: s(`routing_matrix.${task.taskType}.provider_type`) || 'openai',
        model: s(`routing_matrix.${task.taskType}.model`).trim(),
        apiKey: s(`routing_matrix.${task.taskType}.api_key`).trim(),
        endpoint: s(`routing_matrix.${task.taskType}.endpoint`).trim(),
        apiSecret: s(`routing_matrix.${task.taskType}.api_secret`),
        appId: s(`routing_matrix.${task.taskType}.app_id`),
      })),
    ];
  };

  // 一键检测：给主配置和每个已配置路由的 API 发送最小请求（"ping"，temperature=0、max_tokens=16）
  const handleTestLlmRoutes = async () => {
    if (llmTesting) return;
    const targets = collectLlmTestTargets();
    setLlmTesting(true);
    setLlmTestResults(
      Object.fromEntries(targets.map((tg) => [tg.key, { state: 'testing' as const }]))
    );
    await Promise.all(
      targets.map(async (tg) => {
        // 本地服务（Ollama 等）允许空 API Key，仅模型/端点必填
        if (!tg.model || !tg.endpoint) {
          setLlmTestResults((prev) => ({ ...prev, [tg.key]: { state: 'skipped' } }));
          return;
        }
        try {
          const res = await invoke<{
            success: boolean;
            elapsed_ms: number;
            error: string | null;
            reply: string | null;
          }>('test_llm_route', {
            params: {
              provider_type: tg.providerType,
              model: tg.model,
              api_key: tg.apiKey,
              endpoint: tg.endpoint,
              api_secret: tg.apiSecret,
              app_id: tg.appId,
            },
          });
          setLlmTestResults((prev) => ({
            ...prev,
            [tg.key]: res.success
              ? { state: 'ok', elapsedMs: res.elapsed_ms, reply: res.reply ?? undefined }
              : { state: 'error', error: res.error ?? 'Unknown', elapsedMs: res.elapsed_ms },
          }));
        } catch (e) {
          setLlmTestResults((prev) => ({ ...prev, [tg.key]: { state: 'error', error: String(e) } }));
        }
      })
    );
    setLlmTesting(false);
  };

  const handleSave = async () => {
    setSaving(true);
    setSaveError(null);
    // 任何失败都收集为错误信息显示给用户；主配置保存失败则不关闭窗口
    let criticalError: string | null = null;

    // 保存前先抓取旧的嵌入配置，用于保存后检测是否切换了嵌入模型/来源
    let oldEmbedding: ConfigObject | undefined;
    try {
      const cur = await invoke<ConfigObject>('get_all_config');
      oldEmbedding = (cur?.memory as ConfigObject | undefined)?.embedding as ConfigObject | undefined;
    } catch {
      /* ignore */
    }

    // 深拷贝 config，避免修改 UI 状态
    const configCopy: ConfigObject = JSON.parse(JSON.stringify(config));

    // routing_matrix 整体设置（前后端均为 HashMap<String, TaskRouteConfig>）
    if (configCopy.routing_matrix && typeof configCopy.routing_matrix === 'object') {
      try {
        await invoke('set_config', { key: 'routing_matrix', value: configCopy.routing_matrix });
      } catch (e) {
        console.warn('保存 routing_matrix 失败:', e);
      }
      delete configCopy.routing_matrix;
    }

    // provider_cache 整体设置（前后端均为 HashMap<String, CachedProviderProfile>）
    if (configCopy.provider_cache && typeof configCopy.provider_cache === 'object') {
      try {
        await invoke('set_config', { key: 'provider_cache', value: configCopy.provider_cache });
      } catch (e) {
        console.warn('保存 provider_cache 失败:', e);
      }
      delete configCopy.provider_cache;
    }

    // 递归写入所有配置项 —— 单项失败不影响其他项
    const setDeep = async (obj: ConfigObject, prefix = '') => {
      for (const key of Object.keys(obj)) {
        const fullKey = prefix ? `${prefix}.${key}` : key;
        const v = obj[key];
        if (v && typeof v === 'object' && !Array.isArray(v)) {
          await setDeep(v as ConfigObject, fullKey);
        } else {
          try {
            await invoke('set_config', { key: fullKey, value: v });
          } catch (e) {
            console.warn(`保存配置项 ${fullKey} 失败:`, e);
          }
        }
      }
    };
    try {
      await setDeep(configCopy);
    } catch (e) {
      criticalError = `保存配置项失败: ${e}`;
    }

    // 写入磁盘 —— 关键步骤，失败则不关闭窗口
    try {
      await invoke('save_config');
    } catch (e) {
      criticalError = `保存到磁盘失败: ${e}`;
    }

    // 子配置保存（失败不阻止主流程）
    if (ttsConfig) {
      try {
        await invoke('set_tts_config', { config: ttsConfig, characterId: ttsEditCharId ?? undefined });
        // 保存成功后清除该角色的草稿（草稿已与后端一致）
        if (ttsEditCharId) {
          setTtsDrafts((prev) => {
            const next = { ...prev };
            delete next[ttsEditCharId];
            return next;
          });
        }
        // 通知主窗口同步语音开关状态
        try {
          await emit('tts:config-changed', { enabled: ttsConfig.enabled });
        } catch {
          /* ignore */
        }
      } catch (e) {
        console.warn('保存 TTS 配置失败:', e);
      }
    }
    if (diaryConfig) {
      try {
        await invoke('set_diary_config', {
          enable_auto_diary: diaryConfig.enable_auto_diary,
          min_interaction_threshold: diaryConfig.min_interaction_threshold,
          max_diary_length: diaryConfig.max_diary_length,
          characterId: getCharacterId() ?? undefined,
        });
      } catch (e) {
        console.warn('保存日记配置失败:', e);
      }
    }

    // 通知主窗口同步语言切换
    const newLang = (configCopy?.base as ConfigObject | undefined)?.language;
    if (typeof newLang === 'string' && newLang) {
      try {
        await emit('config:language-changed', { language: newLang });
      } catch {
        /* ignore */
      }
    }

    // 通知主窗口配置已保存（用于同步非语言配置，如智能避让开关）
    // 标记自身保存：避免本窗口监听到自己 emit 的事件后重复重载
    selfSaveRef.current = true;
    try {
      await emit('config:saved', {});
    } catch {
      selfSaveRef.current = false;
      /* ignore */
    }

    if (criticalError) {
      // 主配置保存失败 —— 不关闭窗口，显示错误让用户看到
      setSaveError(criticalError);
      setSaving(false);
      return;
    }

    // 重新初始化 Brain / ModelRouter，让新的 LLM 配置立即生效（无需重启应用）
    let reinitOk = false;
    try {
      await invoke('reinitialize');
      reinitOk = true;
    } catch (e) {
      console.warn('重新初始化 Brain 失败（可能需要重启应用）:', e);
      // 初始化失败通常意味着主 LLM / 嵌入服务配置仍不完整，
      // 保持设置窗口打开，方便用户继续调整。
      setSaveError(`重新初始化失败: ${e}`);
      setSaving(false);
      return;
    }

    // 让主动对话配置即时生效 + 通知主窗口更新 tick 间隔与 start/stop 状态
    // 放在 reinitialize 之后：reinitialize 成功时新 Brain 已用新 config 构造，
    // 此命令更新新 Brain 的 proactive config；reinitialize 失败时更新旧 Brain。
    try {
      await invoke('update_proactive_config');
    } catch (e) {
      console.warn('更新主动对话配置失败:', e);
    }
    try {
      await emit('proactive:config-changed');
    } catch {
      /* ignore */
    }

    // 让世界感知配置即时生效（天气/内心独白/记忆巩固开关）
    try {
      await invoke('update_world_config');
    } catch (e) {
      console.warn('更新世界感知配置失败:', e);
    }

    // 让 ASR 配置即时生效（无需重启应用）
    try {
      await invoke('update_asr_config');
    } catch (e) {
      console.warn('更新 ASR 配置失败:', e);
    }

    // 检测嵌入模型/来源是否变化：变化则旧向量索引已在 reinitialize 时失效，
    // 提示用户是否后台重建。rebuild_memory_embeddings 秒回（后台执行），不阻塞关窗。
    const newEmb = (config.memory as ConfigObject | undefined)?.embedding as ConfigObject | undefined;
    const embeddingChanged = reinitOk && !!oldEmbedding && !!newEmb && (
      oldEmbedding.source !== newEmb.source ||
      oldEmbedding.model !== newEmb.model ||
      oldEmbedding.ollama_model !== newEmb.ollama_model ||
      oldEmbedding.dimension !== newEmb.dimension
    );
    if (embeddingChanged) {
      const ok = window.confirm(t('config.embedding_changed_rebuild_confirm'));
      if (ok) {
        try {
          await invoke('rebuild_memory_embeddings');
        } catch (e) {
          console.warn('启动向量重建失败:', e);
          try {
            await emit('toast:show', {
              message: t('config.toast_rebuild_start_failed'),
              type: 'error',
              duration: 5000,
              key: Date.now(),
            });
          } catch {
            /* ignore */
          }
        }
      }
    }

    setSavedFlash(true);
    setSaving(false);
    // 通过独立 Toast 窗口提示用户配置已保存（设置窗口关闭后仍可见）
    try {
      await emit('toast:show', {
        message: t('config_saved'),
        type: 'success',
        duration: 3000,
        key: Date.now(),
      });
    } catch {
      /* ignore */
    }
    // 保存成功后关闭设置界面（单独 try，避免 close 失败影响 savedFlash 状态）
    try {
      await getCurrentWindow().close();
    } catch (e) {
      console.warn('关闭窗口失败:', e);
      setSaveError(`保存成功，但关闭窗口失败: ${e}`);
    }
  };

  const handleReset = async () => {
    try {
      await invoke('reload_config');
      await loadConfig();
    } catch (e) {
      console.warn('重置配置失败:', e);
    }
  };

  const clearNetworkTest = () => {
    setNetworkTestResult(null);
    setNetworkTestSuccess(null);
  };

  const handleTabChange = (tab: TabKey) => {
    // 离开网络页签时清空测试结果
    if (activeTab === 'network' && tab !== 'network') {
      clearNetworkTest();
    }
    setActiveTab(tab);
  };

  const closeWindow = async () => {
    // 关闭窗口前清空测试结果
    clearNetworkTest();
    try {
      await getCurrentWindow().close();
    } catch {
      // ignore
    }
  };

  const tabContent = useMemo(() => {
    switch (activeTab) {
      case 'general':
        return (
          <>
            <div style={sectionTitleStyle}>{t('config.section_general')}</div>
            <SelectField
              label={t('config.field_language')}
              value={get('base.language', 'zh-CN')}
              onChange={(v) => void handleLanguageChange(v)}
              options={[
                { value: 'zh-CN', label: '简体中文' },
                { value: 'en', label: 'English' },
                { value: 'ja', label: '日本語' },
              ]}
            />
            <SelectField
              label={t('config.field_theme')}
              value={get('base.theme', 'system')}
              onChange={(v) => void handleThemeChange(v)}
              options={[
                { value: 'system', label: t('config.theme_option_system') },
                { value: 'light', label: t('config.theme_option_light') },
                { value: 'dark', label: t('config.theme_option_dark') },
              ]}
            />
            <ToggleField
              label={t('config.field_smart_positioning')}
              help={t('config.smart_positioning_help')}
              value={get('window.smart_positioning_enabled', true)}
              onChange={(v) => setNested('window.smart_positioning_enabled', v)}
            />
            <ToggleField
              label={t('config.field_mouse_follow')}
              help={t('config.mouse_follow_help')}
              value={get('live2d_render.always_follow_mouse', false)}
              onChange={(v) => setNested('live2d_render.always_follow_mouse', v)}
            />
            <ToggleField
              label={t('config.field_auto_start')}
              help={t('config.auto_start_help')}
              value={get('base.auto_start', false)}
              onChange={(v) => setNested('base.auto_start', v)}
            />
            <ShortcutsDrawer
              label={t('config.section_shortcuts')}
              expanded={shortcutsExpanded}
              onToggle={() => setShortcutsExpanded((v) => !v)}
              configuredCount={[
                get<string>('base.shortcut', ''),
                get<string>('base.shortcut_nana', ''),
                get<string>('base.shortcut_broadcast', ''),
                get<string>('base.shortcut_chat', ''),
                get<string>('base.shortcut_settings', ''),
                get<string>('base.shortcut_memory', ''),
              ].filter((v) => !!v).length}
            >
              <ShortcutRecorder
                value={get<string>('base.shortcut', 'CommandOrControl+Shift+A')}
                defaultValue="CommandOrControl+Shift+A"
                onChange={handleShortcutChange}
              />
              <ShortcutRecorder
                value={get<string>('base.shortcut_nana', 'CommandOrControl+Shift+Q')}
                defaultValue="CommandOrControl+Shift+Q"
                onChange={handleNanaShortcutChange}
                labelKey="config.field_shortcut_nana"
                helpKey="config.shortcut_nana_help"
              />
              <ShortcutRecorder
                value={get<string>('base.shortcut_broadcast', 'CommandOrControl+Shift+Z')}
                defaultValue="CommandOrControl+Shift+Z"
                onChange={handleBroadcastShortcutChange}
                labelKey="config.field_shortcut_broadcast"
                helpKey="config.shortcut_broadcast_help"
              />
              <ShortcutRecorder
                value={get<string>('base.shortcut_chat', 'CommandOrControl+Shift+W')}
                defaultValue="CommandOrControl+Shift+W"
                onChange={handleChatShortcutChange}
                labelKey="config.field_shortcut_chat"
                helpKey="config.shortcut_chat_help"
              />
              <ShortcutRecorder
                value={get<string>('base.shortcut_settings', 'CommandOrControl+Shift+S')}
                defaultValue="CommandOrControl+Shift+S"
                onChange={handleSettingsShortcutChange}
                labelKey="config.field_shortcut_settings"
                helpKey="config.shortcut_settings_help"
              />
              <ShortcutRecorder
                value={get<string>('base.shortcut_memory', 'CommandOrControl+Shift+N')}
                defaultValue="CommandOrControl+Shift+N"
                onChange={handleMemoryShortcutChange}
                labelKey="config.field_shortcut_memory"
                helpKey="config.shortcut_memory_help"
              />
            </ShortcutsDrawer>

            {/* ── 真实世界感知（原独立页签合并）── */}
            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>
              {t('config.section_world')}
            </div>
            <ToggleField
              label={t('config.field_world_enable')}
              value={get('world.enable', true)}
              onChange={(v) => setNested('world.enable', v)}
            />
            <ToggleField
              label={t('config.field_world_inject_prompt')}
              help={t('config.world_inject_prompt_help')}
              value={get('world.inject_into_prompt', true)}
              onChange={(v) => setNested('world.inject_into_prompt', v)}
            />
            <div style={{ ...sectionTitleStyle, marginTop: 24 }}>
              {t('config.section_world_weather')}
            </div>
            <ToggleField
              label={t('config.field_world_weather')}
              value={get('world.enable_weather', true)}
              onChange={(v) => setNested('world.enable_weather', v)}
            />
            <NumberField
              label={t('config.field_world_weather_ttl')}
              value={get('world.weather_cache_ttl_secs', 3600)}
              onChange={(v) => setNested('world.weather_cache_ttl_secs', v)}
              min={300}
              step={300}
              help={t('config.world_weather_ttl_help')}
            />
            <NumberField
              label={t('config.field_world_latitude')}
              value={get('world.latitude', 0)}
              onChange={(v) => setNested('world.latitude', v)}
              step={0.01}
              help={t('config.world_latitude_help')}
            />
            <NumberField
              label={t('config.field_world_longitude')}
              value={get('world.longitude', 0)}
              onChange={(v) => setNested('world.longitude', v)}
              step={0.01}
              help={t('config.world_longitude_help')}
            />
            <div style={{ display: 'flex', justifyContent: 'flex-end', marginTop: -4, marginBottom: 4 }}>
              <button
                onClick={handleAutoDetectLocation}
                disabled={detectingLocation}
                style={{
                  padding: '4px 12px',
                  fontSize: 12,
                  background: 'var(--panel-bg-surface-elevated)',
                  border: '1px solid var(--panel-border)',
                  borderRadius: 6,
                  color: 'var(--panel-text)',
                  cursor: detectingLocation ? 'wait' : 'pointer',
                  opacity: detectingLocation ? 0.6 : 1,
                }}
              >
                {detectingLocation ? t('config.world_auto_detect_loading') : t('config.world_auto_detect')}
              </button>
            </div>

            {/* ── 内心独白 + 主动问候（合并分组，便于联动）── */}
            <div style={{ ...sectionTitleStyle, marginTop: 24 }}>
              {t('config.section_world_monologue')}
            </div>
            <ToggleField
              label={t('config.field_world_monologue')}
              value={get('world.enable_inner_monologue', true)}
              onChange={(v) => setNested('world.enable_inner_monologue', v)}
            />

            {/* ── 主动对话（从独立页签合并）── */}
            <div style={{ ...sectionTitleStyle, marginTop: 24 }}>
              {t('config.section_proactive')}
            </div>
            <ToggleField
              label={t('config.field_enable_proactive')}
              value={get('proactive.enabled', true)}
              onChange={(v) => setNested('proactive.enabled', v)}
            />

            {/* 内心独白优化的主动问候：仅当内心独白和主动对话都开启时显示 */}
            {get('world.enable_inner_monologue', true) && get('proactive.enabled', true) && (
              <ToggleField
                label={t('config.field_enable_social_urge_gating')}
                value={get('proactive.enable_social_urge_gating', true)}
                onChange={(v) => setNested('proactive.enable_social_urge_gating', v)}
                help={t('config.monologue_greeting_help')}
              />
            )}
            <NumberField
              label={t('config.field_check_interval')}
              value={get('proactive.tick_interval', 10)}
              onChange={(v) => setNested('proactive.tick_interval', v)}
              min={5}
              step={5}
            />
            <NumberField
              label={t('config.field_idle_threshold')}
              value={get('proactive.idle_threshold', 300)}
              onChange={(v) => setNested('proactive.idle_threshold', v)}
              min={30}
              step={30}
            />
            <NumberField
              label={t('config.field_min_trigger_interval')}
              value={get('proactive.min_trigger_interval', 180)}
              onChange={(v) => setNested('proactive.min_trigger_interval', v)}
              min={60}
              step={60}
            />
            <SliderField
              label={t('config.field_proactivity')}
              value={get('proactive.proactivity', 0.5)}
              onChange={(v) => setNested('proactive.proactivity', v)}
              min={0}
              max={1}
              step={0.05}
              format={(v) => v.toFixed(2)}
            />
            <ToggleField
              label={t('config.field_enable_idle_trigger')}
              value={get('proactive.enable_idle_trigger', true)}
              onChange={(v) => setNested('proactive.enable_idle_trigger', v)}
            />
            <ToggleField
              label={t('config.field_enable_window_change')}
              value={get('proactive.enable_window_change_trigger', false)}
              onChange={(v) => setNested('proactive.enable_window_change_trigger', v)}
            />
            <ToggleField
              label={t('config.field_enable_away_reminder')}
              value={get('proactive.enable_away_reminder', true)}
              onChange={(v) => setNested('proactive.enable_away_reminder', v)}
            />
            <ToggleField
              label={t('config.field_enable_system_pressure_trigger')}
              value={get('proactive.enable_system_pressure_trigger', true)}
              onChange={(v) => setNested('proactive.enable_system_pressure_trigger', v)}
            />
            <ToggleField
              label={t('config.field_enable_screen_peek_trigger')}
              value={get('proactive.enable_screen_peek_trigger', true)}
              onChange={(v) => setNested('proactive.enable_screen_peek_trigger', v)}
            />
            <ToggleField
              label={t('config.field_enable_app_duration_trigger')}
              value={get('proactive.enable_app_duration_trigger', true)}
              onChange={(v) => setNested('proactive.enable_app_duration_trigger', v)}
            />
            <ToggleField
              label={t('config.field_enable_late_night_trigger')}
              value={get('proactive.enable_late_night_trigger', true)}
              onChange={(v) => setNested('proactive.enable_late_night_trigger', v)}
            />
            <ToggleField
              label={t('config.field_enable_music_trigger')}
              value={get('proactive.enable_music_trigger', true)}
              onChange={(v) => setNested('proactive.enable_music_trigger', v)}
            />

            {/* ── 日记（仅保留启用开关，其余由智能体自主决定）── */}
            <div style={{ ...sectionTitleStyle, marginTop: 24 }}>
              {t('config.section_diary')}
            </div>
            {diaryLoading ? (
              <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)' }}>{t('common.loading')}</div>
            ) : diaryConfig ? (
              <ToggleField
                label={t('config.field_enable_auto_diary')}
                value={diaryConfig.enable_auto_diary}
                onChange={(v) => setDiaryConfig({ ...diaryConfig, enable_auto_diary: v })}
              />
            ) : null}

            {/* ── 数据备份与整体操作 ── */}
            <div style={{ ...sectionTitleStyle, marginTop: 24 }}>
              {t('config.section_backup')}
            </div>
            <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', lineHeight: 1.6, marginBottom: 12 }}>
              {t('config.backup_help')}
            </div>
            <CollapsibleSection
              title={t('config.section_operations')}
              tone="danger"
              defaultOpen={false}
            >
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8, padding: '2px 2px 4px' }}>
                {/* 导出备份 */}
                <button
                  onClick={() => void handleBackup()}
                  disabled={backingUp}
                  style={{
                    width: '100%',
                    padding: '11px 14px',
                    borderRadius: 12,
                    background: backingUp ? 'var(--panel-selected-bg)' : 'var(--panel-accent)',
                    border: 'none',
                    color: 'var(--panel-selected-text)',
                    fontSize: 13,
                    fontWeight: 600,
                    cursor: backingUp ? 'not-allowed' : 'pointer',
                    opacity: backingUp ? 0.7 : 1,
                    fontFamily: 'inherit',
                    transition: 'opacity 0.2s',
                    boxShadow: 'var(--panel-shadow-subtle)',
                  }}
                >
                  {backingUp ? t('common.saving') : t('config.backup_btn')}
                </button>
                {/* 导入备份（经二次确认弹窗，与恢复出厂设置一致） */}
                <button
                  onClick={() => void handleRestorePick()}
                  disabled={restoring}
                  style={{
                    width: '100%',
                    padding: '11px 14px',
                    borderRadius: 12,
                    background: 'transparent',
                    border: '1px solid var(--panel-accent)',
                    color: 'var(--panel-accent)',
                    fontSize: 13,
                    fontWeight: 600,
                    cursor: restoring ? 'not-allowed' : 'pointer',
                    opacity: restoring ? 0.7 : 1,
                    fontFamily: 'inherit',
                    transition: 'opacity 0.2s',
                  }}
                >
                  {restoring ? t('config.restore_btn_loading') : t('config.restore_btn')}
                </button>
                {/* 恢复出厂设置 */}
                <button
                  onClick={() => setClearMemoriesOpen(true)}
                  style={{
                    width: '100%',
                    padding: '11px 14px',
                    borderRadius: 12,
                    background: 'rgba(255, 69, 58, 0.12)',
                    border: '1px solid rgba(255, 69, 58, 0.3)',
                    color: '#E53935',
                    fontSize: 13,
                    fontWeight: 600,
                    cursor: 'pointer',
                    transition: 'background 0.2s',
                    fontFamily: 'inherit',
                  }}
                  onMouseEnter={(e) => { e.currentTarget.style.background = 'rgba(255, 69, 58, 0.2)'; }}
                  onMouseLeave={(e) => { e.currentTarget.style.background = 'rgba(255, 69, 58, 0.12)'; }}
                >
                  {t('config.clear_memories_btn')}
                </button>
              </div>
            </CollapsibleSection>
          </>
        );
      case 'ai':
        return (
          <>
            <div style={sectionTitleStyle}>{t('config.section_ai')}</div>
            <ProviderSelector pathPrefix="ai" get={get} setNested={setNested} t={t} />
            <datalist id="ai-model-suggestions">
              {(PROVIDER_PRESETS.find(
                (p) => presetMatches(p, get('ai.provider', 'openai') as string, get('ai.endpoint', '') as string),
              )?.mainModels ?? []).map((m) => (
                <option key={m} value={m} />
              ))}
            </datalist>
            <TextField
              label={t('config.field_model_name')}
              value={get('ai.model', 'gpt-5.5')}
              onChange={(v) => setNested('ai.model', v)}
              placeholder={t('config.ph_model')}
              list="ai-model-suggestions"
            />
            <TextField
              label={t('config.field_api_key')}
              type="password"
              value={get('ai.api_key', '')}
              onChange={(v) => setNested('ai.api_key', v)}
              placeholder={t('config.ph_api_key')}
            />
            {needsSecretFor(get('ai.provider', 'openai')) && (
              <TextField
                label={t('config.field_api_secret')}
                type="password"
                value={get('ai.api_secret', '')}
                onChange={(v) => setNested('ai.api_secret', v)}
                placeholder={t('config.ph_api_secret')}
              />
            )}
            {needsAppIdFor(get('ai.provider', 'openai')) && (
              <TextField
                label={t('config.field_app_id')}
                value={get('ai.app_id', '')}
                onChange={(v) => setNested('ai.app_id', v)}
                placeholder={t('config.ph_app_id')}
              />
            )}
            <TextField
              label={t('config.field_endpoint')}
              value={get('ai.endpoint', 'https://api.openai.com/v1')}
              onChange={(v) => setNested('ai.endpoint', v)}
              placeholder={t('config.ph_endpoint')}
            />
            <SliderField
              label={t('config.field_temperature')}
              value={get('ai.temperature', 0.70)}
              onChange={(v) => setNested('ai.temperature', v)}
              min={0}
              max={2}
              step={0.05}
              format={(v) => v.toFixed(2)}
            />
            <NumberField
              label={t('config.field_max_tokens')}
              value={get('ai.max_tokens', 2048)}
              onChange={(v) => setNested('ai.max_tokens', v)}
              min={64}
              step={64}
            />
            <NumberField
              label={t('config.field_context_window')}
              value={get('ai.context_window', 1000000)}
              onChange={(v) => setNested('ai.context_window', v)}
              min={8192}
              step={4096}
              help={t('config.context_window_help')}
            />
            <ReasoningPrefField
              label={t('config.field_reasoning_pref')}
              help={t('config.reasoning_pref_help')}
              value={get('ai.reasoning', null) as { mode: string; effort?: string | null } | null}
              onChange={(v) => setNested('ai.reasoning', v as ConfigValue)}
              t={t}
            />

            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>{t('config.section_multimodal')}</div>
            <ToggleField
              label={t('config.field_enable_vision')}
              help={t('config.field_enable_vision_help')}
              value={get('ai.enable_vision', false)}
              onChange={(v) => setNested('ai.enable_vision', v)}
            />
            <SelectField
              label={t('config.field_image_detail')}
              value={get('ai.image_detail', 'auto')}
              onChange={(v) => setNested('ai.image_detail', v)}
              options={[
                { value: 'auto', label: t('config.opt_image_detail_auto') },
                { value: 'low', label: t('config.opt_image_detail_low') },
                { value: 'high', label: t('config.opt_image_detail_high') },
              ]}
            />

            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>{t('config.section_routing')}</div>
            <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', marginBottom: 14 }}>
              {t('config.routing_description')}
            </div>
            <ToggleField
              label={t('config.field_enable_routing')}
              value={get('enable_routing_matrix', false)}
              onChange={(v) => setNested('enable_routing_matrix', v)}
            />
            <div style={{ display: 'flex', alignItems: 'center', gap: 12, margin: '14px 0' }}>
              <button
                onClick={handleTestLlmRoutes}
                disabled={llmTesting}
                style={{
                  padding: '9px 18px',
                  border: 'none',
                  background: llmTesting ? 'var(--panel-toggle-off)' : 'var(--panel-accent)',
                  color: 'var(--panel-selected-text)',
                  borderRadius: 12,
                  fontSize: 13,
                  fontWeight: 600,
                  fontFamily: 'inherit',
                  cursor: llmTesting ? 'not-allowed' : 'pointer',
                  opacity: llmTesting ? 0.7 : 1,
                  whiteSpace: 'nowrap',
                  transition: 'background 0.2s ease',
                  boxShadow: llmTesting ? 'none' : 'var(--panel-shadow-subtle)',
                }}
              >
                {llmTesting ? t('config.llm_test_testing') : t('config.llm_test_btn')}
              </button>
            </div>
            {Object.keys(llmTestResults).length > 0 && (() => {
              const targets = collectLlmTestTargets();
              const okCount = targets.filter((tg) => llmTestResults[tg.key]?.state === 'ok').length;
              const testedCount = targets.filter((tg) => {
                const st = llmTestResults[tg.key]?.state;
                return st === 'ok' || st === 'error';
              }).length;
              return (
                <div
                  style={{
                    border: '1px solid var(--panel-border)',
                    borderRadius: 8,
                    padding: '10px 14px',
                    marginBottom: 14,
                    background: 'var(--panel-card)',
                    fontSize: 12,
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 4,
                  }}
                >
                  <div style={{ fontWeight: 600, marginBottom: 4 }}>
                    {llmTesting
                      ? t('config.llm_test_testing')
                      : t('config.llm_test_summary', { ok: okCount, total: testedCount })}
                  </div>
                  {targets.map((tg) => {
                    const r = llmTestResults[tg.key];
                    if (!r) return null;
                    const statusText =
                      r.state === 'ok'
                        ? t('config.llm_test_ok')
                        : r.state === 'error'
                          ? t('config.llm_test_failed')
                          : r.state === 'testing'
                            ? t('config.llm_test_testing')
                            : t('config.llm_test_skipped');
                    const color =
                      r.state === 'ok'
                        ? '#4caf50'
                        : r.state === 'error'
                          ? '#f44336'
                          : r.state === 'testing'
                            ? 'var(--panel-text-tertiary)'
                            : 'var(--panel-text-quaternary)';
                    return (
                      <div key={tg.key} style={{ display: 'flex', gap: 8, alignItems: 'baseline' }}>
                        <span style={{ color, fontWeight: 600, minWidth: 56 }}>{statusText}</span>
                        <span style={{ color: 'var(--panel-text-secondary)' }}>{tg.label}</span>
                        {r.state === 'ok' && (
                          <span style={{ color: 'var(--panel-text-tertiary)' }}>
                            {tg.model}
                            {typeof r.elapsedMs === 'number' ? ` · ${r.elapsedMs}ms` : ''}
                            {r.reply ? ` · ${r.reply}` : ''}
                          </span>
                        )}
                        {r.state === 'error' && (
                          <span style={{ color: '#f44336', wordBreak: 'break-all' }}>{r.error}</span>
                        )}
                      </div>
                    );
                  })}
                </div>
              );
            })()}
            {ROUTING_TASKS.map((task) => {
              const prefix = `routing_matrix.${task.taskType}`;
              const providerType = get(`${prefix}.provider_type`, '') as string;
              const modelVal = (get(`${prefix}.model`, '') as string).trim();
              const apiKeyVal = (get(`${prefix}.api_key`, '') as string).trim();
              const endpointVal = (get(`${prefix}.endpoint`, '') as string).trim();
              const apiSecretVal = (get(`${prefix}.api_secret`, '') as string).trim();
              const appIdVal = (get(`${prefix}.app_id`, '') as string).trim();
              const needsSecret = needsSecretFor(providerType || 'openai');
              const needsAppId = needsAppIdFor(providerType || 'openai');
              const isComplete = !!modelVal
                && !!apiKeyVal
                && !!endpointVal
                && (!needsSecret || !!apiSecretVal)
                && (!needsAppId || !!appIdVal);
              return (
              <CollapsibleSection
                key={task.taskType}
                title={t(task.labelKey)}
                subtitle={t(task.helpKey)}
                defaultOpen={false}
                titleAccessory={
                  isComplete && modelVal ? (() => {
                    const test = llmTestResults[task.taskType];
                    const errored = test?.state === 'error' || routeStatus[task.taskType] === 'error';
                    const color = errored
                      ? '#F44336'
                      : test?.state === 'testing'
                        ? 'var(--panel-text-tertiary)'
                        : '#4CAF50';
                    return (
                      <span
                        style={{
                          color,
                          fontSize: 12,
                          fontWeight: 500,
                          display: 'inline-block',
                          maxWidth: '100%',
                        }}
                        title={test?.error ?? modelVal}
                      >
                        {modelVal}
                      </span>
                    );
                  })() : undefined
                }
              >
                <ProviderSelector
                  pathPrefix={`routing_matrix.${task.taskType}`}
                  get={get}
                  setNested={setNested}
                  t={t}
                />
                <TextField
                  label={t('config.field_model_name')}
                  value={get(`routing_matrix.${task.taskType}.model`, '')}
                  onChange={(v) => setNested(`routing_matrix.${task.taskType}.model`, v)}
                  placeholder={t('config.ph_model')}
                />
                <TextField
                  label={t('config.field_api_key')}
                  type="password"
                  value={get(`routing_matrix.${task.taskType}.api_key`, '')}
                  onChange={(v) => setNested(`routing_matrix.${task.taskType}.api_key`, v)}
                  placeholder={t('config.ph_api_key')}
                />
                {needsSecretFor(get(`routing_matrix.${task.taskType}.provider_type`, 'openai')) && (
                  <TextField
                    label={t('config.field_api_secret')}
                    type="password"
                    value={get(`routing_matrix.${task.taskType}.api_secret`, '')}
                    onChange={(v) => setNested(`routing_matrix.${task.taskType}.api_secret`, v)}
                    placeholder={t('config.ph_api_secret')}
                  />
                )}
                {needsAppIdFor(get(`routing_matrix.${task.taskType}.provider_type`, 'openai')) && (
                  <TextField
                    label={t('config.field_app_id')}
                    value={get(`routing_matrix.${task.taskType}.app_id`, '')}
                    onChange={(v) => setNested(`routing_matrix.${task.taskType}.app_id`, v)}
                    placeholder={t('config.ph_app_id')}
                  />
                )}
                <TextField
                  label={t('config.field_endpoint')}
                  value={get(`routing_matrix.${task.taskType}.endpoint`, '')}
                  onChange={(v) => setNested(`routing_matrix.${task.taskType}.endpoint`, v)}
                  placeholder={t('config.ph_endpoint')}
                />
                <SliderField
                  label={t('config.field_temperature')}
                  value={get(`routing_matrix.${task.taskType}.temperature`, get('ai.temperature', 0.70))}
                  onChange={(v) => setNested(`routing_matrix.${task.taskType}.temperature`, v)}
                  min={0}
                  max={2}
                  step={0.05}
                  format={(v) => v.toFixed(2)}
                  help={t('config.field_route_temperature_help')}
                />
                <NumberField
                  label={t('config.field_max_tokens')}
                  value={get(`routing_matrix.${task.taskType}.max_tokens`, get('ai.max_tokens', 2048))}
                  onChange={(v) => setNested(`routing_matrix.${task.taskType}.max_tokens`, v)}
                  min={64}
                  step={64}
                  help={t('config.field_route_max_tokens_help')}
                />
              </CollapsibleSection>
              );
            })}

            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>{t('config.section_work_models')}</div>
            <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', marginBottom: 14 }}>
              {t('config.work_models_description')}
            </div>
            {workModels.length === 0 && (
              <div style={{ fontSize: 12, color: 'var(--panel-text-quaternary)', padding: '10px 0 2px' }}>
                {t('config.work_models_empty')}
              </div>
            )}
            {workModels.map((m, idx) => {
              const isActive = workModelsActiveId === m.id;
              const needsSecret = needsSecretFor(m.provider_type || 'openai');
              const needsAppId = needsAppIdFor(m.provider_type || 'openai');
              return (
                <div
                  key={m.id}
                  style={{
                    border: `1px solid ${isActive ? 'var(--panel-accent)' : 'var(--panel-border)'}`,
                    borderRadius: 8,
                    padding: 12,
                    marginBottom: 12,
                    background: 'var(--panel-card)',
                  }}
                >
                  <div style={{ display: 'flex', alignItems: 'flex-end', gap: 8, marginBottom: 8 }}>
                  <TextField
                    label={t('config.field_work_model_name')}
                    value={m.name}
                    onChange={(v) => patchWorkModel(idx, { name: v })}
                    placeholder={t('config.work_models_name_ph')}
                    style={{ flex: 1, minWidth: 0, marginBottom: 0 }}
                  />
                    <button
                      type="button"
                      onClick={() => removeWorkModel(m.id)}
                      style={{
                        flexShrink: 0,
                        display: 'inline-flex',
                        alignItems: 'center',
                        justifyContent: 'center',
                        width: 32,
                        height: 32,
                        border: '1px solid var(--panel-border)',
                        background: 'transparent',
                        color: 'var(--panel-danger)',
                        cursor: 'pointer',
                        borderRadius: 8,
                        transition: 'background 0.15s ease, border-color 0.15s ease, opacity 0.15s ease',
                        opacity: 0.75,
                      }}
                      onMouseEnter={(e) => { e.currentTarget.style.background = 'rgba(229, 57, 53, 0.12)'; e.currentTarget.style.borderColor = 'rgba(229, 57, 53, 0.4)'; e.currentTarget.style.opacity = '1'; }}
                      onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; e.currentTarget.style.borderColor = 'var(--panel-border)'; e.currentTarget.style.opacity = '0.75'; }}
                      title={t('config.work_models_remove')}
                    >
                      <Trash2 size={15} strokeWidth={2} />
                    </button>
                  </div>
                  <WorkModelProviderSelector
                    model={m}
                    onPatch={(patch) => patchWorkModel(idx, patch)}
                    get={get}
                    setNested={setNested}
                    t={t}
                  />
                  <datalist id={`wm-model-suggestions-${m.id}`}>
                    {(PROVIDER_PRESETS.find(
                      (p) => presetMatches(p, m.provider_type || 'openai', m.endpoint || ''),
                    )?.mainModels ?? []).map((mm) => (
                      <option key={mm} value={mm} />
                    ))}
                  </datalist>
                  <TextField
                    label={t('config.field_model_name')}
                    value={m.model}
                    onChange={(v) => {
                      // 别名未填时，自动用模型名称补全别名
                      if (v && !m.name) patchWorkModel(idx, { model: v, name: v });
                      else patchWorkModel(idx, { model: v });
                    }}
                    placeholder={t('config.ph_model')}
                    list={`wm-model-suggestions-${m.id}`}
                  />
                  <TextField
                    label={t('config.field_endpoint')}
                    value={m.endpoint}
                    onChange={(v) => patchWorkModel(idx, { endpoint: v })}
                    placeholder={t('config.ph_endpoint')}
                  />
                  <TextField
                    label={t('config.field_api_key')}
                    type="password"
                    value={m.api_key}
                    onChange={(v) => patchWorkModel(idx, { api_key: v })}
                    placeholder={t('config.ph_api_key')}
                  />
                  {needsSecret && (
                    <TextField
                      label={t('config.field_api_secret')}
                      type="password"
                      value={m.api_secret ?? ''}
                      onChange={(v) => patchWorkModel(idx, { api_secret: v })}
                      placeholder={t('config.ph_api_secret')}
                    />
                  )}
                  {needsAppId && (
                    <TextField
                      label={t('config.field_app_id')}
                      value={m.app_id ?? ''}
                      onChange={(v) => patchWorkModel(idx, { app_id: v })}
                      placeholder={t('config.ph_app_id')}
                    />
                  )}
                  {/* 不提供 temperature 配置：工作智能体请求统一省略该参数（服务端默认）。
                      编程任务对确定性要求高，且推理模型对非默认温度敏感
                      （o 系列仅接受默认值、reasoner 忽略），参考 codex / Claude Code
                      的做法不让用户关心；后端构建 override provider 时已置
                      omit_temperature，旧配置残留值不会生效。 */}
                  <NumberField
                    label={t('config.field_context_window')}
                    value={m.context_window ?? 1000000}
                    onChange={(v) => patchWorkModel(idx, { context_window: v })}
                    min={8192}
                    step={4096}
                    help={t('config.context_window_help')}
                  />
                  <ReasoningPrefField
                    label={t('config.field_reasoning_pref')}
                    help={t('config.reasoning_pref_help')}
                    value={(m as { reasoning?: { mode: string; effort?: string | null } | null }).reasoning ?? null}
                    onChange={(v) => patchWorkModel(idx, { reasoning: v } as Partial<WorkModelProfile>)}
                    t={t}
                  />
                  {/* 不提供 max_tokens 配置：编程输出预算由后端统一给予充足默认值
                      （WORK_MODEL_DEFAULT_MAX_TOKENS），参考 codex / Claude Code 的做法，
                      不要求用户关心单次输出上限。 */}
                </div>
              );
            })}
            <button
              type="button"
              onClick={addWorkModel}
              style={{
                width: '100%',
                marginTop: 4,
                padding: '8px',
                border: '1px dashed var(--panel-border)',
                borderRadius: 8,
                background: 'transparent',
                color: 'var(--panel-accent)',
                cursor: 'pointer',
                fontSize: 13,
              }}
            >
              + {t('config.work_models_add')}
            </button>
          </>
        );
      case 'tools':
        return (
          <>
            <div style={sectionTitleStyle}>{t('config.section_tools_native_fc')}</div>
            <ToggleField
              label={t('config.field_enable_native_fc')}
              value={get('tools.enable_native_function_calling', true)}
              onChange={(v) => setNested('tools.enable_native_function_calling', v)}
            />

            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>{t('config.section_tools_switches')}</div>
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginBottom: 12, lineHeight: 1.5 }}>
              {t('config.tools_switches_help')}
            </div>
            {toolList.length === 0 ? (
              <div style={{ fontSize: 12, color: 'var(--panel-text-quaternary)', padding: '10px 0 2px' }}>
                {t('config.tools_switches_empty')}
              </div>
            ) : (
              (() => {
                const disabledSet = new Set((get('tools.disabled_tools', []) as string[]));
                const kw = toolSearch.trim().toLowerCase();
                const match = (tl: { name: string; description: string; category: string; is_custom: boolean }) =>
                  !kw ||
                  tl.name.toLowerCase().includes(kw) ||
                  (tl.description ?? '').toLowerCase().includes(kw);
                const filtered = toolList.filter(match);
                const enabledCount = filtered.filter((tl) => !disabledSet.has(tl.name)).length;

                // 工具按类型归类，固定顺序展示
                const CATEGORY_ORDER = ['file', 'web', 'system', 'memory', 'media', 'pet', 'mcp'];
                const grouped = CATEGORY_ORDER
                  .map((cat) => ({ cat, tools: filtered.filter((tl) => tl.category === cat) }))
                  .filter((g) => g.tools.length > 0);

                const cardGrid = (tools: typeof filtered) => (
                  <div style={{ display: 'grid', gridTemplateColumns: 'repeat(2, minmax(0, 1fr))', gap: 8 }}>
                    {tools.map((tl) => {
                      const enabled = !disabledSet.has(tl.name);
                      return (
                        <ToolSwitchCard
                          key={tl.name}
                          name={tl.name}
                          description={tl.description}
                          categoryLabel={t(`config.tool_cat_${tl.category}`)}
                          custom={!!tl.is_custom}
                          customBadge={t('config.tool_badge_custom')}
                          enabled={enabled}
                          onToggle={() => {
                            const next = new Set(disabledSet);
                            if (enabled) next.add(tl.name);
                            else next.delete(tl.name);
                            setNested('tools.disabled_tools', Array.from(next));
                          }}
                        />
                      );
                    })}
                  </div>
                );

                return (
                  <>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 10 }}>
                      <input
                        type="text"
                        value={toolSearch}
                        onChange={(e) => setToolSearch(e.target.value)}
                        placeholder={t('config.tools_search_ph')}
                        style={{ ...inputStyle, flex: 1, padding: '7px 10px', fontSize: 12 }}
                      />
                      <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', whiteSpace: 'nowrap' }}>
                        {t('config.tools_enabled_count', { enabled: enabledCount, total: filtered.length })}
                      </span>
                    </div>
                    {/* 搜索时平铺展示全部命中项；默认按类别收纳展开 */}
                    {kw ? (
                      cardGrid(filtered)
                    ) : (
                      grouped.map((g) => (
                        <CollapsibleSection
                          key={g.cat}
                          defaultOpen={false}
                          title={`${t(`config.tool_cat_${g.cat}`)} (${
                            g.tools.filter((tl) => !disabledSet.has(tl.name)).length
                          }/${g.tools.length})`}
                        >
                          {cardGrid(g.tools)}
                        </CollapsibleSection>
                      ))
                    )}
                  </>
                );
              })()
            )}

            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>{t('config.section_tools_execution')}</div>
            <NumberField
              label={t('config.field_tool_max_iterations')}
              value={(get('tools.max_iterations', 10) as number) === 0 ? -1 : (get('tools.max_iterations', 10) as number)}
              onChange={(v) => setNested('tools.max_iterations', v === -1 ? 0 : v)}
              min={-1}
              max={50}
              step={1}
              help={t('config.field_unlimited_hint')}
            />
            <NumberField
              label={t('config.field_tool_max_rounds')}
              value={(get('tools.max_rounds', 20) as number) === 0 ? -1 : (get('tools.max_rounds', 20) as number)}
              onChange={(v) => setNested('tools.max_rounds', v === -1 ? 0 : v)}
              min={-1}
              max={20}
              step={1}
              help={t('config.field_unlimited_hint')}
            />
            <NumberField
              label={t('config.field_tool_max_coding_rounds')}
              value={(get('tools.max_coding_rounds', 48) as number) === 0 ? -1 : (get('tools.max_coding_rounds', 48) as number)}
              onChange={(v) => setNested('tools.max_coding_rounds', v === -1 ? 0 : v)}
              min={-1}
              max={100}
              step={8}
              help={t('config.field_unlimited_hint')}
            />
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
              {t('config.field_tool_max_coding_rounds_help')}
            </div>
            <NumberField
              label={t('config.field_tool_max_result_chars')}
              value={get('tools.max_result_chars', 4000)}
              onChange={(v) => setNested('tools.max_result_chars', v)}
              min={500}
              step={500}
            />
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
              {t('config.field_tool_max_result_chars_help')}
            </div>
            <NumberField
              label={t('config.field_tool_feedback_history_chars')}
              value={get('tools.feedback_history_chars', 2000)}
              onChange={(v) => setNested('tools.feedback_history_chars', v)}
              min={200}
              step={200}
            />
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
              {t('config.field_tool_feedback_history_chars_help')}
            </div>
            <NumberField
              label={t('config.field_tool_default_timeout')}
              value={get('tools.default_tool_timeout_secs', 120)}
              onChange={(v) => setNested('tools.default_tool_timeout_secs', v)}
              min={5}
              step={5}
            />
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
              {t('config.field_tool_default_timeout_help')}
            </div>
            <NumberField
              label={t('config.field_tool_confirmation_timeout')}
              value={get('tools.confirmation_timeout_secs', 600)}
              onChange={(v) => setNested('tools.confirmation_timeout_secs', v)}
              min={30}
              step={30}
            />
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
              {t('config.field_tool_confirmation_timeout_help')}
            </div>

            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>{t('config.section_tools_cache')}</div>
            <ToggleField
              label={t('config.field_tool_enable_cache')}
              value={get('tools.enable_cache', true)}
              onChange={(v) => setNested('tools.enable_cache', v)}
            />
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
              {t('config.field_tool_enable_cache_help')}
            </div>
            <NumberField
              label={t('config.field_tool_cache_ttl')}
              value={get('tools.cache_ttl_secs', 300)}
              onChange={(v) => setNested('tools.cache_ttl_secs', v)}
              min={0}
              step={30}
            />
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
              {t('config.field_tool_cache_ttl_help')}
            </div>
            <NumberField
              label={t('config.field_tool_cache_max_size')}
              value={get('tools.cache_max_size', 1000)}
              onChange={(v) => setNested('tools.cache_max_size', v)}
              min={0}
              step={100}
            />
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
              {t('config.field_tool_cache_max_size_help')}
            </div>

            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>{t('config.section_tools_permission')}</div>
            <SelectField
              label={t('config.field_tool_access_level')}
              value={get('tools.access_level', 'full-control')}
              onChange={(v) => setNested('tools.access_level', v)}
              options={[
                { value: 'read-only', label: t('config.access_level_readonly') },
                { value: 'fs-read', label: t('config.access_level_fsread') },
                { value: 'fs-write', label: t('config.access_level_fswrite') },
                { value: 'full-control', label: t('config.access_level_fullcontrol') },
              ]}
            />
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
              {t('config.field_tool_access_level_help')}
            </div>

            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>{t('config.section_mcp')}</div>
            <div style={{
              fontSize: 12,
              color: 'var(--panel-text-secondary)',
              lineHeight: 1.7,
              padding: '10px 12px',
              marginBottom: 12,
              background: 'var(--panel-bg-hover)',
              borderRadius: 8,
              border: '1px solid var(--panel-border)',
            }}>
              {t('config.mcp_description')}
            </div>

            {/* 已连接 server 列表 */}
            {mcpServers.length > 0 && (
              <div style={{ marginBottom: 12 }}>
                {mcpServers.map((s) => (
                  <div key={s.id} style={{
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'space-between',
                    padding: '8px 12px',
                    marginBottom: 4,
                    background: 'var(--panel-bg-hover)',
                    borderRadius: 6,
                    border: '1px solid var(--panel-border)',
                  }}>
                    <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
                      <span style={{ fontSize: 13, color: 'var(--panel-text)' }}>
                        {s.name} <span style={{ color: 'var(--panel-text-tertiary)', fontSize: 11 }}>({s.id})</span>
                      </span>
                      <span style={{ fontSize: 11, color: s.alive ? '#4ade80' : '#f87171' }}>
                        {s.alive ? t('config.mcp_alive') : t('config.mcp_dead')} · {s.tool_count} {t('config.mcp_tools')}
                      </span>
                    </div>
                    <button
                      onClick={async () => {
                        try {
                          await invoke('remove_mcp_server', { serverId: s.id });
                          const refreshed = await invoke<Array<typeof s>>('list_mcp_servers');
                          setMcpServers(refreshed);
                          void emit('toast:show', { message: t('config.mcp_removed'), type: 'success', duration: 4000, key: Date.now() });
                        } catch (e) {
                          void emit('toast:show', { message: String(e), type: 'error', duration: 4000, key: Date.now() });
                        }
                      }}
                      style={{
                        padding: '4px 10px',
                        fontSize: 12,
                        background: 'rgba(248, 113, 113, 0.15)',
                        border: '1px solid rgba(248, 113, 113, 0.3)',
                        borderRadius: 6,
                        color: '#f87171',
                        cursor: 'pointer',
                      }}
                    >
                      {t('config.mcp_remove')}
                    </button>
                  </div>
                ))}
              </div>
            )}

            {/* 添加新 server */}
            {mcpEditing ? (
              <div style={{
                padding: '12px',
                marginBottom: 12,
                background: 'var(--panel-bg-hover)',
                borderRadius: 8,
                border: '1px solid var(--panel-border)',
              }}>
                <TextField
                  label={t('config.mcp_field_id')}
                  value={mcpEditing.id}
                  onChange={(v) => setMcpEditing({ ...mcpEditing, id: v })}
                  placeholder="filesystem"
                />
                <TextField
                  label={t('config.mcp_field_name')}
                  value={mcpEditing.name}
                  onChange={(v) => setMcpEditing({ ...mcpEditing, name: v })}
                  placeholder="Filesystem MCP"
                />
                <TextField
                  label={t('config.mcp_field_command')}
                  value={mcpEditing.command}
                  onChange={(v) => setMcpEditing({ ...mcpEditing, command: v })}
                  placeholder="npx"
                />
                <TextField
                  label={t('config.mcp_field_args')}
                  value={mcpEditing.args}
                  onChange={(v) => setMcpEditing({ ...mcpEditing, args: v })}
                  placeholder="-y @modelcontextprotocol/server-filesystem /tmp"
                />
                <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
                  {t('config.mcp_field_args_help')}
                </div>
                <ToggleField
                  label={t('config.mcp_field_enabled')}
                  value={mcpEditing.enabled}
                  onChange={(v) => setMcpEditing({ ...mcpEditing, enabled: v })}
                />
                <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
                  <button
                    onClick={async () => {
                      setMcpSaving(true);
                      try {
                        const args = mcpEditing.args.trim().split(/\s+/).filter(Boolean);
                        await invoke('add_mcp_server', {
                          config: {
                            id: mcpEditing.id,
                            name: mcpEditing.name,
                            transport: 'stdio',
                            command: mcpEditing.command,
                            args,
                            env: {},
                            cwd: null,
                            enabled: mcpEditing.enabled,
                          },
                        });
                        const refreshed = await invoke<Array<typeof mcpServers[number]>>('list_mcp_servers');
                        setMcpServers(refreshed);
                        setMcpEditing(null);
                        void emit('toast:show', { message: t('config.mcp_added'), type: 'success', duration: 4000, key: Date.now() });
                      } catch (e) {
                        void emit('toast:show', { message: String(e), type: 'error', duration: 4000, key: Date.now() });
                      } finally {
                        setMcpSaving(false);
                      }
                    }}
                    disabled={mcpSaving || !mcpEditing.id || !mcpEditing.command}
                    style={{
                      padding: '6px 16px',
                      fontSize: 13,
                      background: mcpSaving ? 'rgba(74, 222, 128, 0.3)' : 'rgba(74, 222, 128, 0.15)',
                      border: '1px solid rgba(74, 222, 128, 0.3)',
                      borderRadius: 6,
                      color: '#4ade80',
                      cursor: mcpSaving ? 'wait' : 'pointer',
                      opacity: (mcpSaving || !mcpEditing.id || !mcpEditing.command) ? 0.6 : 1,
                    }}
                  >
                    {mcpSaving ? t('config.mcp_connecting') : t('config.mcp_add')}
                  </button>
                  <button
                    onClick={() => setMcpEditing(null)}
                    style={{
                      padding: '6px 16px',
                      fontSize: 13,
                      background: 'var(--panel-bg-surface-elevated)',
                      border: '1px solid var(--panel-border)',
                      borderRadius: 6,
                      color: 'var(--panel-text-secondary)',
                      cursor: 'pointer',
                    }}
                  >
                    {t('config.mcp_cancel')}
                  </button>
                </div>
              </div>
            ) : (
              <button
                onClick={() => setMcpEditing({ id: '', name: '', command: '', args: '', enabled: true })}
                style={{
                  padding: '6px 16px',
                  fontSize: 13,
                  background: 'var(--panel-bg-surface-elevated)',
                  border: '1px solid var(--panel-border)',
                  borderRadius: 6,
                  color: 'var(--panel-text)',
                  cursor: 'pointer',
                }}
              >
                + {t('config.mcp_add_server')}
              </button>
            )}
          </>
        );
      case 'memory':
        return (
          <>
            <div style={sectionTitleStyle}>{t('config.section_memory')}</div>
            <NumberField
              label={t('config.field_short_term_limit')}
              help={t('config.field_short_term_limit_help')}
              value={get('memory.max_short_term_memory', 20)}
              onChange={(v) => setNested('memory.max_short_term_memory', v)}
              min={1}
              step={1}
            />
            <SelectField
              label={t('config.field_retrieval_strategy')}
              value={get('memory.retrieval_strategy', 'auto')}
              onChange={(v) => setNested('memory.retrieval_strategy', v)}
              options={[
                { value: 'auto', label: t('config.opt_auto') },
                { value: 'keyword', label: t('config.opt_keyword') },
                { value: 'vector', label: t('config.opt_vector') },
                { value: 'hybrid', label: t('config.opt_hybrid') },
                { value: 'graph', label: t('config.opt_graph') },
              ]}
            />
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
              {t('config.field_retrieval_strategy_help')}
            </div>
            <ToggleField
              label={t('config.field_enable_expiration')}
              value={get('memory.enable_expiration', true)}
              onChange={(v) => setNested('memory.enable_expiration', v)}
            />

            {/* 巩固流水线配置 */}
            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>
              {t('config.section_consolidation')}
            </div>
            <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', marginBottom: 14, lineHeight: 1.6 }}>
              {t('config.consolidation_description')}
            </div>
            <ToggleField
              label={t('config.field_world_consolidation')}
              help={t('config.world_consolidation_help')}
              value={get('world.enable_memory_consolidation', true)}
              onChange={(v) => setNested('world.enable_memory_consolidation', v)}
            />
            <NumberField
              label={t('config.field_stage1_threshold')}
              help={t('config.field_stage1_threshold_help')}
              value={get('memory.consolidation.stage1_short_term_threshold', 20)}
              onChange={(v) => setNested('memory.consolidation.stage1_short_term_threshold', v)}
              min={3}
              step={1}
            />
            <NumberField
              label={t('config.field_stage1_idle_sec')}
              help={t('config.field_stage1_idle_sec_help')}
              value={get('memory.consolidation.stage1_idle_timeout_sec', 1800)}
              onChange={(v) => setNested('memory.consolidation.stage1_idle_timeout_sec', v)}
              min={60}
              step={60}
            />

            {/* 检索三因子加权 */}
            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>
              {t('config.section_retrieval_weights')}
            </div>
            <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', marginBottom: 14, lineHeight: 1.6 }}>
              {t('config.retrieval_weights_description')}
            </div>
            <SliderField
              label={t('config.field_weight_recency')}
              help={t('config.field_weight_recency_help')}
              value={get('memory.retrieval_weights.recency', 0.25)}
              onChange={(v) => setNested('memory.retrieval_weights.recency', v)}
              min={0}
              max={1}
              step={0.05}
              format={(v) => v.toFixed(2)}
            />
            <SliderField
              label={t('config.field_weight_relevance')}
              help={t('config.field_weight_relevance_help')}
              value={get('memory.retrieval_weights.relevance', 0.4)}
              onChange={(v) => setNested('memory.retrieval_weights.relevance', v)}
              min={0}
              max={1}
              step={0.05}
              format={(v) => v.toFixed(2)}
            />
            <SliderField
              label={t('config.field_weight_importance')}
              help={t('config.field_weight_importance_help')}
              value={get('memory.retrieval_weights.importance', 0.15)}
              onChange={(v) => setNested('memory.retrieval_weights.importance', v)}
              min={0}
              max={1}
              step={0.05}
              format={(v) => v.toFixed(2)}
            />
            <NumberField
              label={t('config.field_recency_tau')}
              help={t('config.field_recency_tau_help')}
              value={get('memory.retrieval_weights.recency_tau_hours', 24)}
              onChange={(v) => setNested('memory.retrieval_weights.recency_tau_hours', v)}
              min={1}
              step={1}
            />

            {/* 嵌入服务配置（独立路由 fallback） */}
            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>
              {t('config.section_embedding')}
            </div>
            <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', marginBottom: 14, lineHeight: 1.6 }}>
              {t('config.embedding_description')}
            </div>
            <ToggleField
              label={t('config.field_enable_embedding')}
              value={get('memory.embedding.enabled', false)}
              onChange={(v) => setNested('memory.embedding.enabled', v)}
            />
            <SelectField
              label={t('config.field_embedding_source')}
              value={get('memory.embedding.source', 'cloud')}
              onChange={(v) => setNested('memory.embedding.source', v)}
              options={[
                { value: 'cloud', label: t('config.opt_embedding_cloud') },
                { value: 'local', label: t('config.opt_embedding_local') },
              ]}
            />

            {get('memory.embedding.source', 'cloud') === 'cloud' ? (
              <>
                <TextField
                  label={t('config.field_embedding_endpoint')}
                  value={get('memory.embedding.endpoint', '')}
                  onChange={(v) => setNested('memory.embedding.endpoint', v)}
                  placeholder={t('config.ph_embedding_endpoint')}
                />
                <TextField
                  label={t('config.field_api_key')}
                  type="password"
                  value={get('memory.embedding.api_key', '')}
                  onChange={(v) => setNested('memory.embedding.api_key', v)}
                  placeholder={t('config.ph_api_key')}
                />
                <TextField
                  label={t('config.field_embedding_model')}
                  value={get('memory.embedding.model', 'BAAI/bge-m3')}
                  onChange={(v) => {
                    setNested('memory.embedding.model', v);
                    // 命中内置云端模型注册表时自动填充维度
                    const known = embeddingModels.find(
                      (m) => m.source === 'cloud' && m.id === v,
                    );
                    if (known) setNested('memory.embedding.dimension', known.dimension);
                  }}
                  placeholder={t('config.ph_embedding_model')}
                  list="embedding-cloud-models"
                />
                <datalist id="embedding-cloud-models">
                  {embeddingModels
                    .filter((m) => m.source === 'cloud')
                    .map((m) => (
                      <option key={m.id} value={m.id}>
                        {m.id} ({m.dimension})
                      </option>
                    ))}
                </datalist>
                <NumberField
                  label={t('config.field_embedding_dim')}
                  value={get('memory.embedding.dimension', 1024)}
                  onChange={(v) => setNested('memory.embedding.dimension', v)}
                  min={64}
                  step={64}
                />
              </>
            ) : (
              <>
                <BrowseTextField
                  label={t('config.field_ollama_path')}
                  value={get('memory.embedding.ollama_path', 'G:\\ollama\\ollama.exe')}
                  onChange={(v) => setNested('memory.embedding.ollama_path', v)}
                  placeholder="G:\\ollama\\ollama.exe"
                  onBrowse={async () => {
                    try {
                      const selected = await open({
                        multiple: false,
                        filters: [{ name: 'ollama', extensions: ['exe'] }],
                      });
                      if (typeof selected === 'string' && selected) {
                        setNested('memory.embedding.ollama_path', selected);
                      }
                    } catch (e) {
                      console.warn('选择 ollama 路径失败:', e);
                    }
                  }}
                  browseLabel={t('config.btn_browse')}
                />
                <SelectField
                  label={t('config.field_ollama_model')}
                  value={get('memory.embedding.ollama_model', 'bge-m3')}
                  onChange={(v) => {
                    setNested('memory.embedding.ollama_model', v);
                    const dim = OLLAMA_MODEL_DIMS[v];
                    if (dim) setNested('memory.embedding.dimension', dim);
                  }}
                  options={(() => {
                    const cur = get('memory.embedding.ollama_model', 'bge-m3');
                    const set = new Set<string>([
                      'bge-m3',
                      'nomic-embed-text',
                      ...ollamaModels,
                    ]);
                    if (cur && !set.has(cur)) set.add(cur);
                    return Array.from(set).map((m) => ({ value: m, label: m }));
                  })()}
                />
                <NumberField
                  label={t('config.field_embedding_dim')}
                  value={get('memory.embedding.dimension', 1024)}
                  onChange={(v) => setNested('memory.embedding.dimension', v)}
                  min={64}
                  step={64}
                />

                {/* Ollama 服务控制：启动/停止 + 状态徽章 + PID */}
                <div style={{ ...fieldStyle, display: 'flex', alignItems: 'center', gap: 10 }}>
                  <button
                    type="button"
                    onClick={toggleOllamaService}
                    disabled={ollamaServiceBusy}
                    style={{
                      padding: '6px 14px',
                      border: 'none',
                      borderRadius: 6,
                      background:
                        ollamaService?.status === 'running' ? '#e74c3c' : '#27ae60',
                      color: '#fff',
                      fontSize: 12,
                      cursor: ollamaServiceBusy ? 'not-allowed' : 'pointer',
                      fontFamily: 'inherit',
                      opacity: ollamaServiceBusy ? 0.6 : 1,
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: 6,
                    }}
                  >
                    {ollamaService?.status === 'running'
                      ? t('config.btn_ollama_stop')
                      : t('config.btn_ollama_start')}
                  </button>
                  <span
                    style={{
                      fontSize: 11,
                      padding: '2px 8px',
                      borderRadius: 4,
                      background:
                        ollamaService?.status === 'running'
                          ? 'rgba(39,174,96,0.15)'
                          : ollamaService?.status === 'crashed'
                            ? 'rgba(231,76,60,0.15)'
                            : 'var(--panel-bg-surface-elevated)',
                      color:
                        ollamaService?.status === 'running'
                          ? '#27ae60'
                          : ollamaService?.status === 'crashed'
                            ? '#e74c3c'
                            : 'var(--panel-text-secondary)',
                    }}
                  >
                    {(() => {
                      const s = ollamaService?.status ?? 'stopped';
                      switch (s) {
                        case 'running':
                          return t('config.ollama_status_running');
                        case 'starting':
                          return t('config.ollama_status_starting');
                        case 'stopping':
                          return t('config.ollama_status_stopping');
                        case 'crashed':
                          return t('config.ollama_status_crashed');
                        default:
                          return t('config.ollama_status_stopped');
                      }
                    })()}
                  </span>
                  {ollamaService?.pid && (
                    <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)' }}>
                      PID: {ollamaService.pid}
                    </span>
                  )}
                </div>

                {/* 拉取模型 */}
                <div style={{ ...fieldStyle, display: 'flex', alignItems: 'center', gap: 10, flexWrap: 'wrap' }}>
                  <button
                    type="button"
                    onClick={pullOllamaModel}
                    disabled={ollamaPulling}
                    style={{
                      padding: '6px 14px',
                      border: '1.5px solid var(--panel-border)',
                      borderRadius: 6,
                      background: 'var(--panel-surface)',
                      color: 'var(--panel-text-secondary)',
                      fontSize: 12,
                      cursor: ollamaPulling ? 'not-allowed' : 'pointer',
                      fontFamily: 'inherit',
                      opacity: ollamaPulling ? 0.6 : 1,
                      whiteSpace: 'nowrap',
                    }}
                  >
                    {ollamaPulling
                      ? t('config.btn_ollama_pulling')
                      : t('config.btn_ollama_pull')}
                  </button>
                  {(() => {
                    const cur = get('memory.embedding.ollama_model', 'bge-m3');
                    const prefix = `${cur}:`;
                    const loaded = ollamaModels.some(
                      (m) => m === cur || m.startsWith(prefix),
                    );
                    const others = ollamaModels.filter(
                      (m) => m !== cur && !m.startsWith(prefix),
                    );
                    return (
                      <>
                        <span
                          style={{
                            fontSize: 11,
                            fontWeight: 600,
                            color: loaded ? '#27ae60' : '#e67e22',
                          }}
                        >
                          {loaded
                            ? t('config.ollama_current_model_loaded', { model: cur })
                            : t('config.ollama_current_model_missing', { model: cur })}
                        </span>
                        {others.length > 0 && (
                          <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)' }}>
                            {t('config.ollama_other_installed', {
                              models: others.join(', '),
                            })}
                          </span>
                        )}
                      </>
                    );
                  })()}
                </div>

                <ToggleField
                  label={t('config.field_ollama_auto_start')}
                  help={t('config.field_ollama_auto_start_help')}
                  value={get('memory.embedding.ollama_auto_start', false)}
                  onChange={(v) => setNested('memory.embedding.ollama_auto_start', v)}
                />
              </>
            )}

            {/* 向量索引后端配置（local=内置 sqlite-vec / external=外部向量库） */}
            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>
              {t('config.section_vector_store')}
            </div>
            <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', marginBottom: 14, lineHeight: 1.6 }}>
              {t('config.vector_store_description')}
            </div>
            <SelectField
              label={t('config.field_vector_store_source')}
              value={get<string>('memory.vector_store.source', 'local')}
              onChange={(v) => setNested('memory.vector_store.source', v)}
              options={[
                { value: 'local', label: t('config.opt_vector_store_local') },
                { value: 'external', label: t('config.opt_vector_store_external') },
              ]}
            />
            {get<string>('memory.vector_store.source', 'local') === 'external' && (
              <>
                <TextField
                  label={t('config.field_vector_store_url')}
                  value={get('memory.vector_store.external_url', '')}
                  onChange={(v) => setNested('memory.vector_store.external_url', v)}
                  placeholder="http://localhost:6333"
                />
                <TextField
                  label={t('config.field_vector_store_api_key')}
                  type="password"
                  value={get('memory.vector_store.api_key', '')}
                  onChange={(v) => setNested('memory.vector_store.api_key', v)}
                  placeholder={t('config.ph_vector_store_api_key')}
                />
                <TextField
                  label={t('config.field_vector_store_collection')}
                  value={get('memory.vector_store.collection', 'vivian_memories')}
                  onChange={(v) => setNested('memory.vector_store.collection', v)}
                  placeholder="vivian_memories"
                />
                <NumberField
                  label={t('config.field_vector_store_hnsw_m')}
                  value={get('memory.vector_store.hnsw_m', 16)}
                  onChange={(v) => setNested('memory.vector_store.hnsw_m', v)}
                  min={4}
                  max={128}
                  step={4}
                />
                <NumberField
                  label={t('config.field_vector_store_ef_construction')}
                  value={get('memory.vector_store.ef_construction', 200)}
                  onChange={(v) => setNested('memory.vector_store.ef_construction', v)}
                  min={64}
                  max={1000}
                  step={32}
                />
              </>
            )}

            {/* 独立精排（cross-encoder reranker）配置 */}
            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>
              {t('config.section_rerank')}
            </div>
            <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', marginBottom: 14, lineHeight: 1.6 }}>
              {t('config.rerank_description')}
            </div>
            <ToggleField
              label={t('config.field_rerank_enabled')}
              help={t('config.field_rerank_enabled_help')}
              value={get('memory.rerank.enabled', false)}
              onChange={(v) => setNested('memory.rerank.enabled', v)}
            />
            {get('memory.rerank.enabled', false) && (
              <>
                <TextField
                  label={t('config.field_rerank_endpoint')}
                  value={get('memory.rerank.endpoint', 'http://localhost:11434')}
                  onChange={(v) => setNested('memory.rerank.endpoint', v)}
                  placeholder="http://localhost:11434"
                />
                <SelectField
                  label={t('config.field_rerank_model')}
                  value={get('memory.rerank.model', 'bge-reranker-v2-m3')}
                  onChange={(v) => setNested('memory.rerank.model', v)}
                  options={(() => {
                    const cur = get('memory.rerank.model', 'bge-reranker-v2-m3');
                    const set = new Set<string>(['bge-reranker-v2-m3', 'bge-reranker-base', ...ollamaModels]);
                    if (cur && !set.has(cur)) set.add(cur);
                    return Array.from(set).map((m) => ({ value: m, label: m }));
                  })()}
                />
                <NumberField
                  label={t('config.field_rerank_top_k')}
                  value={get('memory.rerank.top_k', 20)}
                  onChange={(v) => setNested('memory.rerank.top_k', v)}
                  min={1}
                  max={100}
                  step={1}
                />
              </>
            )}
          </>
        );
      case 'voice':
        return (
          <>
            <div style={sectionTitleStyle}>{t('config.section_asr')}</div>
            <SelectField
              label={t('config.field_asr_engine')}
              value={get('speech_recognition.engine', 'winrt')}
              onChange={(v) => setNested('speech_recognition.engine', v)}
              options={[
                { value: 'winrt', label: t('config.opt_winrt') },
                { value: 'whisper', label: t('config.opt_whisper') },
                { value: 'azure', label: t('config.opt_azure') },
                { value: 'aliyun', label: t('config.opt_aliyun') },
                { value: 'openai_whisper', label: t('config.opt_openai_whisper') },
              ]}
              labelExtra={
                <button
                  type="button"
                  title={t('config.btn_asr_help')}
                  onClick={() => {
                    const engineToBackend: Record<string, AsrBackendKey> = {
                      winrt: 'winrt',
                      whisper: 'whisper',
                      azure: 'azure',
                      aliyun: 'aliyun',
                      openai_whisper: 'openai_whisper',
                    };
                    const cur = (get('speech_recognition.engine', 'winrt') as string) ?? 'winrt';
                    openAsrHelp(engineToBackend[cur] ?? 'winrt');
                  }}
                  style={{
                    width: 16,
                    height: 16,
                    borderRadius: '50%',
                    border: 'none',
                    background: 'var(--panel-toggle-off)',
                    color: 'var(--panel-text-secondary)',
                    fontSize: 11,
                    fontWeight: 600,
                    lineHeight: 1,
                    cursor: 'pointer',
                    padding: 0,
                    display: 'inline-flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    fontFamily: 'inherit',
                  }}
                >
                  ?
                </button>
              }
            />
            <SelectField
              label={t('config.field_asr_language')}
              value={get('speech_recognition.language', 'zh-CN')}
              onChange={(v) => setNested('speech_recognition.language', v)}
              options={[
                { value: 'zh-CN', label: t('config.opt_zh_cn') },
                { value: 'en-US', label: t('config.opt_en_us') },
                { value: 'ja-JP', label: t('config.opt_ja') },
              ]}
            />
            <NumberField
              label={t('config.field_silence_timeout')}
              value={get('speech_recognition.silence_timeout_ms', 1500)}
              onChange={(v) => setNested('speech_recognition.silence_timeout_ms', v)}
              min={200}
              step={100}
            />

            {(get('speech_recognition.engine', 'winrt') as string) === 'whisper' && (
              <>
                <div style={{ ...sectionTitleStyle, marginTop: 16 }}>
                  {t('config.section_whisper_service')}
                </div>
                {/* 一键启动/停止 faster-whisper-server 子进程 */}
                <div
                  style={{
                    padding: '8px 10px',
                    border: '1px solid var(--panel-border)',
                    borderRadius: 8,
                    background: 'var(--panel-bg-surface)',
                    display: 'flex',
                    alignItems: 'center',
                    gap: 10,
                    flexWrap: 'wrap',
                    marginBottom: 8,
                  }}
                >
                  <button
                    onClick={toggleWhisperService}
                    disabled={whisperServiceBusy || whisperService?.status === 'installing'}
                    style={{
                      padding: '6px 14px',
                      border: 'none',
                      borderRadius: 6,
                      background:
                        whisperService?.status === 'running'
                          ? '#e74c3c'
                          : whisperService?.status === 'installing'
                            ? '#e67e22'
                            : '#27ae60',
                      color: '#fff',
                      fontSize: 12,
                      cursor: whisperServiceBusy || whisperService?.status === 'installing' ? 'not-allowed' : 'pointer',
                      fontFamily: 'inherit',
                      opacity: whisperServiceBusy || whisperService?.status === 'installing' ? 0.6 : 1,
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: 6,
                    }}
                  >
                    {(whisperServiceBusy || whisperService?.status === 'installing') && (
                      <span
                        style={{
                          display: 'inline-block',
                          width: 12,
                          height: 12,
                          border: '2px solid rgba(0,0,0,0.3)',
                          borderTopColor: '#fff',
                          borderRadius: '50%',
                          animation: 'gptsovits-spin 0.8s linear infinite',
                          flexShrink: 0,
                        }}
                      />
                    )}
                    {whisperService?.status === 'running'
                      ? whisperServiceBusy
                        ? t('config.whisper_status_stopping')
                        : t('config.btn_whisper_stop')
                      : whisperService?.status === 'installing'
                        ? t('config.whisper_status_installing')
                        : whisperService?.status === 'starting'
                          ? t('config.whisper_status_starting')
                          : whisperService?.status === 'stopping'
                            ? t('config.whisper_status_stopping')
                            : whisperServiceBusy
                              ? t('config.whisper_status_starting')
                              : t('config.btn_whisper_start')}
                  </button>
                  <span
                    style={{
                      fontSize: 11,
                      padding: '2px 8px',
                      borderRadius: 4,
                      background:
                        whisperService?.status === 'running'
                          ? 'rgba(39,174,96,0.15)'
                          : whisperService?.status === 'crashed'
                            ? 'rgba(231,76,60,0.15)'
                            : whisperService?.status === 'installing'
                              ? 'rgba(230,126,34,0.15)'
                              : 'var(--panel-bg-surface-elevated)',
                      color:
                        whisperService?.status === 'running'
                          ? '#27ae60'
                          : whisperService?.status === 'crashed'
                            ? '#e74c3c'
                            : whisperService?.status === 'installing'
                              ? '#e67e22'
                              : 'var(--panel-text-secondary)',
                    }}
                  >
                    {(() => {
                      const s = whisperService?.status ?? 'stopped';
                      switch (s) {
                        case 'running':
                          return t('config.whisper_status_running');
                        case 'installing':
                          return t('config.whisper_status_installing');
                        case 'starting':
                          return t('config.whisper_status_starting');
                        case 'stopping':
                          return t('config.whisper_status_stopping');
                        case 'crashed':
                          return t('config.whisper_status_crashed');
                        default:
                          return t('config.whisper_status_stopped');
                      }
                    })()}
                  </span>
                  {whisperService?.pid && (
                    <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)' }}>
                      PID: {whisperService.pid}
                    </span>
                  )}
                  {whisperService?.endpoint && (
                    <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)' }}>
                      {whisperService.endpoint}
                    </span>
                  )}
                  {whisperService?.error && (
                    <div
                      style={{
                        width: '100%',
                        fontSize: 11,
                        color: '#e74c3c',
                        background: 'rgba(231,76,60,0.08)',
                        padding: '6px 8px',
                        borderRadius: 4,
                        marginTop: 4,
                        wordBreak: 'break-all',
                      }}
                    >
                      {whisperService.error}
                    </div>
                  )}
                </div>

                {/* ── 普通模式：常用选项 ── */}

                {/* 模型选择 */}
                <SelectField
                  label={t('config.field_whisper_service_model')}
                  value={get('speech_recognition.whisper.service_model', 'small')}
                  onChange={(v) =>
                    setNested('speech_recognition.whisper.service_model', v)
                  }
                  options={[
                    { value: 'tiny', label: 'tiny' },
                    { value: 'base', label: 'base' },
                    { value: 'small', label: 'small' },
                    { value: 'medium', label: 'medium' },
                    { value: 'large-v3', label: 'large-v3' },
                  ]}
                />

                {/* 设备 */}
                <SelectField
                  label={t('config.field_whisper_service_device')}
                  value={get('speech_recognition.whisper.service_device', 'auto')}
                  onChange={(v) =>
                    setNested('speech_recognition.whisper.service_device', v)
                  }
                  options={[
                    { value: 'auto', label: t('config.opt_whisper_device_auto') },
                    { value: 'cpu', label: 'CPU' },
                    { value: 'cuda', label: 'CUDA (GPU)' },
                  ]}
                />

                {/* 流式模式 */}
                <SelectField
                  label={t('config.field_whisper_streaming_mode')}
                  value={get('speech_recognition.whisper.streaming_mode', 'none')}
                  onChange={(v) =>
                    setNested('speech_recognition.whisper.streaming_mode', v)
                  }
                  options={[
                    { value: 'none', label: t('config.opt_whisper_streaming_none') },
                    { value: 'sse', label: t('config.opt_whisper_streaming_sse') },
                    { value: 'realtime_ws', label: t('config.opt_whisper_streaming_realtime_ws') },
                  ]}
                />
                <div
                  style={{
                    fontSize: 11,
                    color: 'var(--panel-text-tertiary)',
                    marginTop: -8,
                    marginBottom: 8,
                    lineHeight: 1.5,
                  }}
                >
                  {t('config.whisper_hint_streaming_mode')}
                </div>

                {/* ── 高级设置折叠区 ── */}
                <button
                  type="button"
                  onClick={() => setWhisperAdvancedOpen((v) => !v)}
                  style={{
                    width: '100%',
                    display: 'flex',
                    alignItems: 'center',
                    gap: 8,
                    padding: '8px 0',
                    background: 'transparent',
                    border: 'none',
                    borderTop: '1px solid var(--panel-border)',
                    cursor: 'pointer',
                    fontFamily: 'inherit',
                    color: 'var(--panel-text-secondary)',
                    fontSize: 12,
                    marginTop: 8,
                  }}
                >
                  <span
                    style={{
                      display: 'inline-block',
                      transition: 'transform 0.15s',
                      transform: whisperAdvancedOpen ? 'rotate(90deg)' : 'rotate(0deg)',
                      color: 'var(--panel-text-tertiary)',
                      fontSize: 10,
                    }}
                  >
                    ▸
                  </span>
                  <span>{t('config.section_whisper_advanced')}</span>
                </button>

                {whisperAdvancedOpen && (
                  <>
                    {/* 自动启动开关 */}
                    <ToggleField
                      label={t('config.field_whisper_service_auto_start')}
                      value={get('speech_recognition.whisper.service_auto_start', false)}
                      onChange={(v) =>
                        setNested('speech_recognition.whisper.service_auto_start', v)
                      }
                    />

                    {/* Python 路径（可选，用于推导 faster-whisper-server 脚本位置） */}
                    <BrowseTextField
                      label={t('config.field_whisper_service_python_path')}
                      value={get('speech_recognition.whisper.service_python_path', '') ?? ''}
                      onChange={(v) =>
                        setNested(
                          'speech_recognition.whisper.service_python_path',
                          v,
                        )
                      }
                      placeholder={t('config.placeholder_whisper_service_python_path')}
                      onBrowse={async () => {
                        try {
                          const selected = await open({
                            multiple: false,
                            filters: [{ name: 'Python', extensions: ['exe'] }],
                          });
                          if (typeof selected === 'string' && selected) {
                            setNested(
                              'speech_recognition.whisper.service_python_path',
                              selected,
                            );
                          }
                        } catch (e) {
                          console.warn('选择 Python 路径失败:', e);
                        }
                      }}
                      browseLabel={t('config.btn_browse')}
                    />

                    {/* 安装路径（可选，作为子进程 cwd） */}
                    <BrowseTextField
                      label={t('config.field_whisper_service_install_path')}
                      value={
                        get('speech_recognition.whisper.service_install_path', '') ?? ''
                      }
                      onChange={(v) =>
                        setNested(
                          'speech_recognition.whisper.service_install_path',
                          v,
                        )
                      }
                      placeholder={t('config.placeholder_whisper_service_install_path')}
                      onBrowse={async () => {
                        try {
                          const selected = await open({ directory: true, multiple: false });
                          if (typeof selected === 'string' && selected) {
                            setNested(
                              'speech_recognition.whisper.service_install_path',
                              selected,
                            );
                          }
                        } catch (e) {
                          console.warn('选择安装路径失败:', e);
                        }
                      }}
                      browseLabel={t('config.btn_browse')}
                    />

                    {/* 计算精度 */}
                    <SelectField
                      label={t('config.field_whisper_service_compute_type')}
                      value={get('speech_recognition.whisper.service_compute_type', 'auto')}
                      onChange={(v) =>
                        setNested('speech_recognition.whisper.service_compute_type', v)
                      }
                      options={[
                        { value: 'auto', label: t('config.opt_whisper_compute_auto') },
                        { value: 'int8', label: 'int8 (CPU 推荐)' },
                        { value: 'int8_float16', label: 'int8_float16' },
                        { value: 'float16', label: 'float16 (GPU 推荐)' },
                        { value: 'float32', label: 'float32' },
                      ]}
                    />

                    {/* 端口 */}
                    <NumberField
                      label={t('config.field_whisper_service_port')}
                      value={get('speech_recognition.whisper.service_port', 8000)}
                      onChange={(v) =>
                        setNested('speech_recognition.whisper.service_port', v)
                      }
                      min={1024}
                      max={65535}
                    />

                    <div
                      style={{
                        fontSize: 11,
                        color: 'var(--panel-text-tertiary)',
                        marginTop: -4,
                        marginBottom: 8,
                        lineHeight: 1.5,
                      }}
                    >
                      {t('config.whisper_hint_pip_required')}
                    </div>

                    {/* Realtime WebSocket 专属配置 */}
                    <div style={{ ...sectionTitleStyle, marginTop: 12 }}>
                      {t('config.section_whisper_realtime')}
                    </div>
                    <TextField
                      label={t('config.field_whisper_realtime_model')}
                      value={get('speech_recognition.whisper.realtime_model', '') ?? ''}
                      onChange={(v) =>
                        setNested('speech_recognition.whisper.realtime_model', v)
                      }
                      placeholder={t('config.placeholder_whisper_realtime_model')}
                    />
                    <TextField
                      label={t('config.field_whisper_realtime_language')}
                      value={get('speech_recognition.whisper.realtime_language', '') ?? ''}
                      onChange={(v) =>
                        setNested('speech_recognition.whisper.realtime_language', v)
                      }
                      placeholder="zh / en / ja"
                    />

                    {/* 手动配置外部 Whisper 服务（whisper.cpp / OpenAI API 等） */}
                    <div style={{ ...sectionTitleStyle, marginTop: 12 }}>
                      {t('config.section_whisper')}
                    </div>
                    <TextField
                      label={t('config.field_whisper_server_url')}
                      value={get('speech_recognition.whisper.server_url', '')}
                      onChange={(v) => setNested('speech_recognition.whisper.server_url', v)}
                      placeholder="http://localhost:8080"
                    />
                    <SelectField
                      label={t('config.field_whisper_api_format')}
                      value={get('speech_recognition.whisper.api_format', 'openai')}
                      onChange={(v) => setNested('speech_recognition.whisper.api_format', v)}
                      options={[
                        { value: 'openai', label: t('config.opt_whisper_api_format_openai') },
                        { value: 'whisper_cpp', label: t('config.opt_whisper_api_format_whisper_cpp') },
                      ]}
                    />
                    <TextField
                      label={t('config.field_whisper_api_key')}
                      value={get('speech_recognition.whisper.api_key', '')}
                      onChange={(v) => setNested('speech_recognition.whisper.api_key', v)}
                      placeholder={t('config.field_whisper_api_key_placeholder')}
                      type="password"
                    />
                    <NumberField
                      label={t('config.field_whisper_max_seconds')}
                      value={get('speech_recognition.whisper.max_audio_seconds', 30)}
                      onChange={(v) => setNested('speech_recognition.whisper.max_audio_seconds', v)}
                      min={5}
                      max={60}
                      step={5}
                    />
                  </>
                )}
              </>
            )}

            {(get('speech_recognition.engine', 'winrt') as string) === 'azure' && (
              <>
                <div style={{ ...sectionTitleStyle, marginTop: 16 }}>
                  {t('config.section_azure')}
                </div>
                <TextField
                  label={t('config.field_azure_speech_key')}
                  value={get('speech_recognition.azure.speech_key', '')}
                  onChange={(v) => setNested('speech_recognition.azure.speech_key', v)}
                  type="password"
                  placeholder="xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
                />
                <TextField
                  label={t('config.field_azure_region')}
                  value={get('speech_recognition.azure.speech_region', 'eastasia')}
                  onChange={(v) => setNested('speech_recognition.azure.speech_region', v)}
                  placeholder="eastasia"
                />
                <AdvancedToggle
                  open={azureAsrAdvancedOpen}
                  onToggle={() => setAzureAsrAdvancedOpen((v) => !v)}
                  label={t('config.section_advanced_settings')}
                />
                {azureAsrAdvancedOpen && (
                  <>
                    <ToggleField
                      label={t('config.field_azure_conversation_mode')}
                      value={get('speech_recognition.azure.conversation_mode', true)}
                      onChange={(v) => setNested('speech_recognition.azure.conversation_mode', v)}
                    />
                    <NumberField
                      label={t('config.field_azure_max_seconds')}
                      value={get('speech_recognition.azure.max_audio_seconds', 30)}
                      onChange={(v) => setNested('speech_recognition.azure.max_audio_seconds', v)}
                      min={5}
                      max={60}
                      step={5}
                    />
                  </>
                )}
              </>
            )}

            {(get('speech_recognition.engine', 'winrt') as string) === 'openai_whisper' && (
              <>
                <div style={{ ...sectionTitleStyle, marginTop: 16 }}>
                  {t('config.section_openai_whisper')}
                </div>
                <TextField
                  label={t('config.field_openai_whisper_api_key')}
                  value={get('speech_recognition.openai_whisper.api_key', '')}
                  onChange={(v) => setNested('speech_recognition.openai_whisper.api_key', v)}
                  placeholder="sk-..."
                  type="password"
                />
                <TextField
                  label={t('config.field_openai_whisper_base_url')}
                  value={get('speech_recognition.openai_whisper.base_url', 'https://api.openai.com')}
                  onChange={(v) => setNested('speech_recognition.openai_whisper.base_url', v)}
                  placeholder="https://api.openai.com"
                />
                <NumberField
                  label={t('config.field_openai_whisper_max_seconds')}
                  value={get('speech_recognition.openai_whisper.max_audio_seconds', 30)}
                  onChange={(v) => setNested('speech_recognition.openai_whisper.max_audio_seconds', v)}
                  min={5}
                  max={60}
                  step={5}
                />
              </>
            )}

            {(get('speech_recognition.engine', 'winrt') as string) === 'aliyun' && (
              <>
                <div style={{ ...sectionTitleStyle, marginTop: 16 }}>
                  {t('config.section_aliyun')}
                </div>
                <TextField
                  label={t('config.field_aliyun_app_key')}
                  value={get('speech_recognition.aliyun.app_key', '')}
                  onChange={(v) => setNested('speech_recognition.aliyun.app_key', v)}
                  placeholder="xxxxxxxxxxxxxxxx"
                />
                <TextField
                  label={t('config.field_aliyun_access_key_id')}
                  value={get('speech_recognition.aliyun.access_key_id', '')}
                  onChange={(v) => setNested('speech_recognition.aliyun.access_key_id', v)}
                  placeholder="LTAI..."
                />
                <TextField
                  label={t('config.field_aliyun_access_key_secret')}
                  value={get('speech_recognition.aliyun.access_key_secret', '')}
                  onChange={(v) => setNested('speech_recognition.aliyun.access_key_secret', v)}
                  type="password"
                  placeholder="xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"
                />
                <AdvancedToggle
                  open={aliyunAsrAdvancedOpen}
                  onToggle={() => setAliyunAsrAdvancedOpen((v) => !v)}
                  label={t('config.section_advanced_settings')}
                />
                {aliyunAsrAdvancedOpen && (
                  <NumberField
                    label={t('config.field_aliyun_max_seconds')}
                    value={get('speech_recognition.aliyun.max_audio_seconds', 60)}
                    onChange={(v) => setNested('speech_recognition.aliyun.max_audio_seconds', v)}
                    min={5}
                    max={120}
                    step={5}
                  />
                )}
              </>
            )}

            <div style={{ ...sectionTitleStyle, marginTop: 24 }}>
              {t('config.section_tts')}
            </div>
            {/* TTS 按角色独立配置：切换编辑目标角色 */}
            {characters.length > 1 && (
              <div style={{ marginBottom: 12, display: 'flex', gap: 8, flexWrap: 'wrap' }}>
                {characters.map((c) => {
                  const active = (ttsEditCharId ?? characters[0]?.id) === c.id;
                  return (
                    <button
                      key={c.id}
                      type="button"
                      onClick={() => switchTtsEditChar(c.id)}
                      style={{
                        padding: '6px 14px',
                        borderRadius: 16,
                        border: `1px solid ${active ? 'var(--panel-border-strong)' : 'var(--panel-border)'}`,
                        background: active ? 'var(--panel-selected-bg)' : 'transparent',
                        color: active ? 'var(--panel-selected-text)' : 'var(--panel-text-secondary)',
                        cursor: 'pointer',
                        fontSize: 13,
                        fontWeight: active ? 600 : 400,
                        transition: 'all 0.15s',
                      }}
                    >
                      {c.name}
                    </button>
                  );
                })}
              </div>
            )}
            {ttsConfig ? (
              <>
                <ToggleField
                  label={t('config.field_enable_tts')}
                  value={ttsConfig.enabled}
                  onChange={(v) => setTtsConfig({ ...ttsConfig, enabled: v })}
                />
                <SelectField
                  label={t('config.field_tts_engine')}
                  value={ttsConfig.engine}
                  onChange={(v) => {
                    const newEngine = v as TtsConfigState['engine'];
                    setTtsConfig({ ...ttsConfig, engine: newEngine });
                    if (newEngine === 'edgetts') {
                      void loadEdgeTtsVoices(ttsEditCharId ?? undefined);
                    } else {
                      setEdgeTtsVoices([]);
                    }
                  }}
                  options={[
                    { value: 'none', label: t('config.opt_tts_none') },
                    { value: 'edgetts', label: t('config.opt_tts_edgetts') },
                    { value: 'azure', label: t('config.opt_tts_azure') },
                    { value: 'gptsovits', label: t('config.opt_tts_gptsovits') },
                    { value: 'fishspeech', label: t('config.opt_tts_fishspeech') },
                    { value: 'minimax', label: t('config.opt_tts_minimax') },
                    { value: 'doubao', label: t('config.opt_tts_doubao') },
                    { value: 'mimo', label: t('config.opt_tts_mimo') },
                  ]}
                  labelExtra={
                    <button
                      type="button"
                      title={t('config.btn_tts_help')}
                      onClick={() => {
                        const engineToBackend: Record<string, TtsBackendKey> = {
                          edgetts: 'edgetts',
                          azure: 'azure',
                          gptsovits: 'gptsovits',
                          fishspeech: 'fishspeech',
                          bertvits2: 'fishspeech',
                          minimax: 'minimax',
                          doubao: 'doubao',
                        };
                        const target = ttsConfig
                          ? engineToBackend[ttsConfig.engine] ?? 'edgetts'
                          : 'edgetts';
                        openTtsHelp(target);
                      }}
                      style={{
                        width: 16,
                        height: 16,
                        borderRadius: '50%',
                        border: 'none',
                        background: 'var(--panel-toggle-off)',
                        color: 'var(--panel-text-secondary)',
                        fontSize: 11,
                        fontWeight: 600,
                        lineHeight: 1,
                        cursor: 'pointer',
                        padding: 0,
                        display: 'inline-flex',
                        alignItems: 'center',
                        justifyContent: 'center',
                        fontFamily: 'inherit',
                      }}
                    >
                      ?
                    </button>
                  }
                />
                <SelectField
                  label={t('config.field_tts_fallback_engine')}
                  value={ttsConfig.fallback_engine ?? 'none'}
                  onChange={(v) =>
                    setTtsConfig({
                      ...ttsConfig,
                      fallback_engine: v === 'none' ? null : (v as TtsConfigState['fallback_engine']),
                    })
                  }
                  options={[
                    { value: 'none', label: t('config.opt_tts_none') },
                    { value: 'edgetts', label: t('config.opt_tts_edgetts') },
                    { value: 'azure', label: t('config.opt_tts_azure') },
                    { value: 'gptsovits', label: t('config.opt_tts_gptsovits') },
                    { value: 'fishspeech', label: t('config.opt_tts_fishspeech') },
                    { value: 'minimax', label: t('config.opt_tts_minimax') },
                    { value: 'doubao', label: t('config.opt_tts_doubao') },
                    { value: 'mimo', label: t('config.opt_tts_mimo') },
                  ]}
                />
                {/* 双实例并行模式：仅在所有角色都选择 GPT-SoVITS 引擎且配置了安装路径时显示 */}
                {(() => {
                  const allCharsUseGptsovits = characters.length >= 2
                    && characters.every((c) => charTtsEngines[c.id] === 'gptsovits');
                  if (!allCharsUseGptsovits) return null;
                  return (
                    <div style={{ marginBottom: 8 }}>
                      <ToggleField
                        label={t('config.field_tts_gptsovits_dual_instance')}
                        value={ttsConfig.gpt_sovits_dual_instance}
                        onChange={(v) => setTtsConfig({
                          ...ttsConfig,
                          gpt_sovits_dual_instance: v,
                          gpt_sovits_second_port: v ? (ttsConfig.gpt_sovits_second_port ?? 9881) : null,
                        })}
                      />
                      {ttsConfig.gpt_sovits_dual_instance && (
                        <div style={{ marginTop: 6 }}>
                          <TextField
                            label={t('config.field_tts_gptsovits_second_port')}
                            value={ttsConfig.gpt_sovits_second_port?.toString() ?? ''}
                            onChange={(v) => {
                              const n = parseInt(v, 10);
                              setTtsConfig({ ...ttsConfig, gpt_sovits_second_port: isNaN(n) ? null : n });
                            }}
                            placeholder="9881"
                            type="number"
                          />
                          <div
                            style={{
                              width: '100%',
                              fontSize: 11,
                              color: 'var(--panel-text-tertiary)',
                              marginTop: 4,
                              padding: '6px 8px',
                              background: 'var(--panel-bg-surface)',
                              borderRadius: 4,
                            }}
                          >
                            {t('config.gptsovits_hint_dual_instance')}
                          </div>
                        </div>
                      )}
                    </div>
                  );
                })()}
                <NumberField
                  label={t('config.field_tts_retry_count')}
                  value={ttsConfig.retry_count}
                  onChange={(v) => setTtsConfig({ ...ttsConfig, retry_count: v })}
                  min={0}
                  max={5}
                  step={1}
                />
                <SliderField
                  label={t('config.field_tts_rate')}
                  value={ttsConfig.rate}
                  onChange={(v) => setTtsConfig({ ...ttsConfig, rate: v })}
                  min={0.5}
                  max={3.0}
                  step={0.1}
                  format={(v) => `${v.toFixed(1)}x`}
                />
                <SliderField
                  label={t('config.field_tts_volume')}
                  value={ttsConfig.volume}
                  onChange={(v) => setTtsConfig({ ...ttsConfig, volume: v })}
                  min={0}
                  max={1}
                  step={0.05}
                  format={(v) => `${Math.round(v * 100)}%`}
                />

                {/* EdgeTTS 配置 */}
                {ttsConfig.engine === 'edgetts' && (
                  <>
                    <div
                      style={{
                        ...sectionTitleStyle,
                        marginTop: 16,
                        fontSize: 12,
                      }}
                    >
                      <span>{t('config.section_tts_edgetts')}</span>
                    </div>
                    <SelectField
                      label={t('config.field_tts_edgetts_voice')}
                      value={ttsConfig.voice_id ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, voice_id: v || null })}
                      options={[
                        { value: '', label: t('config.opt_tts_edgetts_voice_default') },
                        ...edgeTtsVoices
                          .filter((v) => {
                            const lang = ttsConfig.tts_language ?? ttsConfig.display_language ?? '';
                            if (!lang) return true;
                            return v.language === lang || v.language.startsWith(lang + '-');
                          })
                          .map((v) => ({
                            value: v.id,
                            label: `${t(`config.tts_voice_names.${v.id}`, { defaultValue: v.name })} (${v.language})`,
                          })),
                      ]}
                    />
                  </>
                )}

                {/* Azure 配置 */}
                {ttsConfig.engine === 'azure' && (
                  <>
                    <div
                      style={{
                        ...sectionTitleStyle,
                        marginTop: 16,
                        fontSize: 12,
                        display: 'flex',
                        alignItems: 'center',
                        justifyContent: 'space-between',
                      }}
                    >
                      <span>{t('config.section_tts_azure')}</span>
                    </div>
                    <TextField
                      label={t('config.field_tts_azure_key')}
                      value={ttsConfig.azure_key ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, azure_key: v || null })}
                      placeholder="Azure Portal → 密钥和终结点 → KEY 1（32 位字符串）"
                      type="password"
                    />
                    <TextField
                      label={t('config.field_tts_azure_region')}
                      value={ttsConfig.azure_region ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, azure_region: v || null })}
                      placeholder="eastus（如 eastus / westus / southeastasia / japaneast）"
                    />
                    <AdvancedToggle
                      open={azureTtsAdvancedOpen}
                      onToggle={() => setAzureTtsAdvancedOpen((v) => !v)}
                      label={t('config.section_advanced_settings')}
                    />
                    {azureTtsAdvancedOpen && (
                      <>
                        <SelectField
                          label={t('config.field_tts_azure_style')}
                          value={ttsConfig.azure_style ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, azure_style: v || null })}
                          options={[
                            { value: '', label: t('config.opt_tts_azure_style_none') },
                            { value: 'cheerful', label: 'cheerful（愉快）' },
                            { value: 'sad', label: 'sad（悲伤）' },
                            { value: 'excited', label: 'excited（兴奋）' },
                            { value: 'friendly', label: 'friendly（友好）' },
                            { value: 'whispering', label: 'whispering（耳语）' },
                            { value: 'angry', label: 'angry（愤怒）' },
                            { value: 'calm', label: 'calm（平静）' },
                            { value: 'gentle', label: 'gentle（温柔）' },
                            { value: 'hopeful', label: 'hopeful（希望）' },
                            { value: 'shouting', label: 'shouting（呼喊）' },
                            { value: 'terrified', label: 'terrified（恐惧）' },
                            { value: 'chat', label: 'chat（闲聊）' },
                            { value: 'assistant', label: 'assistant（助手）' },
                            { value: 'customerservice', label: 'customerservice（客服）' },
                            { value: 'newscast', label: 'newscast（新闻播报）' },
                            { value: 'narration-professional', label: 'narration-professional（专业叙述）' },
                            { value: 'narration-relaxed', label: 'narration-relaxed（轻松叙述）' },
                            { value: 'empathetic', label: 'empathetic（共情）' },
                            { value: 'affectionate', label: 'affectionate（深情）' },
                          ]}
                        />
                        <TextField
                          label={t('config.field_tts_azure_style_degree')}
                          value={ttsConfig.azure_style_degree?.toString() ?? ''}
                          onChange={(v) => {
                            const n = parseFloat(v);
                            setTtsConfig({ ...ttsConfig, azure_style_degree: isNaN(n) ? null : n });
                          }}
                          placeholder="1.0（范围 0.5-2.0，默认 1.0）"
                          type="number"
                        />
                        <SelectField
                          label={t('config.field_tts_azure_role')}
                          value={ttsConfig.azure_role ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, azure_role: v || null })}
                          options={[
                            { value: '', label: t('config.opt_tts_azure_role_none') },
                            { value: 'YoungAdultFemale', label: 'YoungAdultFemale（年轻女性）' },
                            { value: 'YoungAdultMale', label: 'YoungAdultMale（年轻男性）' },
                            { value: 'OlderAdultFemale', label: 'OlderAdultFemale（中年女性）' },
                            { value: 'OlderAdultMale', label: 'OlderAdultMale（中年男性）' },
                            { value: 'SeniorFemale', label: 'SeniorFemale（老年女性）' },
                            { value: 'SeniorMale', label: 'SeniorMale（老年男性）' },
                            { value: 'Girl', label: 'Girl（女孩）' },
                            { value: 'Boy', label: 'Boy（男孩）' },
                          ]}
                        />
                        <TextField
                          label={t('config.field_tts_azure_pitch')}
                          value={ttsConfig.azure_pitch?.toString() ?? ''}
                          onChange={(v) => {
                            const n = parseFloat(v);
                            setTtsConfig({ ...ttsConfig, azure_pitch: isNaN(n) ? null : n });
                          }}
                          placeholder="0（范围 -50 到 +50 半音，默认 0）"
                          type="number"
                        />
                        <SelectField
                          label={t('config.field_tts_azure_output_format')}
                          value={ttsConfig.azure_output_format ?? 'audio-24khz-48kbitrate-mono-mp3'}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, azure_output_format: v || null })}
                          options={[
                            { value: 'audio-24khz-48kbitrate-mono-mp3', label: 'MP3 24kHz 48kbps（默认）' },
                            { value: 'audio-24khz-160kbitrate-mono-mp3', label: 'MP3 24kHz 160kbps（高码率）' },
                            { value: 'audio-48khz-192kbitrate-mono-mp3', label: 'MP3 48kHz 192kbps（HD）' },
                            { value: 'riff-24khz-16bit-mono-pcm', label: 'WAV 24kHz 16bit（推荐，无损）' },
                            { value: 'riff-16khz-16bit-mono-pcm', label: 'WAV 16kHz 16bit（低采样率）' },
                            { value: 'riff-48khz-16bit-mono-pcm', label: 'WAV 48kHz 16bit（HD）' },
                            { value: 'ogg-24khz-16bit-mono-opus', label: 'OGG 24kHz Opus' },
                            { value: 'webm-24khz-16bit-mono-opus', label: 'WebM 24kHz Opus' },
                            { value: 'raw-24khz-16bit-mono-pcm', label: 'PCM 24kHz 16bit（裸数据，无 WAV 头）' },
                          ]}
                        />
                      </>
                    )}
                  </>
                )}

                {/* GPT-SoVITS 配置 */}
                {ttsConfig.engine === 'gptsovits' && (
                  <>
                    <div
                      style={{
                        ...sectionTitleStyle,
                        marginTop: 16,
                      }}
                    >
                      {t('config.section_tts_gptsovits')}
                    </div>

                    {/* 服务控制面板 — 一键启动 / 停止 + 状态显示 */}
                    <div
                      style={{
                        padding: '12px 14px',
                        border: '1px solid var(--panel-border)',
                        borderRadius: 8,
                        background: 'var(--panel-bg-surface)',
                        display: 'flex',
                        alignItems: 'center',
                        gap: 10,
                        flexWrap: 'wrap',
                        marginBottom: 8,
                      }}
                    >
                      <button
                        onClick={toggleGptsovitsService}
                        disabled={gptsovitsServiceBusy || !ttsConfig.gpt_sovits_install_path}
                        style={{
                          padding: '6px 14px',
                          border: 'none',
                          borderRadius: 6,
                          background:
                            gptsovitsService?.status === 'running'
                              ? '#e74c3c'
                              : '#27ae60',
                          color: '#fff',
                          fontSize: 12,
                          cursor:
                            gptsovitsServiceBusy || !ttsConfig.gpt_sovits_install_path
                              ? 'not-allowed'
                              : 'pointer',
                          fontFamily: 'inherit',
                          opacity:
                            gptsovitsServiceBusy || !ttsConfig.gpt_sovits_install_path ? 0.6 : 1,
                          display: 'inline-flex',
                          alignItems: 'center',
                          gap: 6,
                        }}
                      >
                        {gptsovitsServiceBusy && (
                          <span
                            style={{
                              display: 'inline-block',
                              width: 12,
                              height: 12,
                              border: '2px solid rgba(0,0,0,0.3)',
                              borderTopColor: '#fff',
                              borderRadius: '50%',
                              animation: 'gptsovits-spin 0.8s linear infinite',
                              flexShrink: 0,
                            }}
                          />
                        )}
                        {gptsovitsService?.status === 'running'
                          ? gptsovitsServiceBusy
                            ? t('config.gptsovits_status_stopping')
                            : t('config.btn_gptsovits_stop')
                          : gptsovitsService?.status === 'starting'
                            ? t('config.gptsovits_status_starting')
                            : gptsovitsService?.status === 'stopping'
                              ? t('config.gptsovits_status_stopping')
                              : gptsovitsServiceBusy
                                ? t('config.gptsovits_status_starting')
                                : t('config.btn_gptsovits_start')}
                      </button>
                      <span
                        style={{
                          fontSize: 11,
                          padding: '2px 8px',
                          borderRadius: 4,
                          background:
                            gptsovitsService?.status === 'running'
                              ? 'rgba(39,174,96,0.15)'
                              : gptsovitsService?.status === 'crashed'
                                ? 'rgba(231,76,60,0.15)'
                                : 'var(--panel-bg-surface-elevated)',
                          color:
                            gptsovitsService?.status === 'running'
                              ? '#27ae60'
                              : gptsovitsService?.status === 'crashed'
                                ? '#e74c3c'
                                : 'var(--panel-text-secondary)',
                        }}
                      >
                        {(() => {
                          const s = gptsovitsService?.status ?? 'stopped';
                          switch (s) {
                            case 'running':
                              return t('config.gptsovits_status_running');
                            case 'starting':
                              return t('config.gptsovits_status_starting');
                            case 'stopping':
                              return t('config.gptsovits_status_stopping');
                            case 'crashed':
                              return t('config.gptsovits_status_crashed');
                            default:
                              return t('config.gptsovits_status_stopped');
                          }
                        })()}
                      </span>
                      {gptsovitsService?.pid && (
                        <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)' }}>
                          PID: {gptsovitsService.pid}
                        </span>
                      )}
                      {gptsovitsService?.endpoint && (
                        <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)' }}>
                          {gptsovitsService.endpoint}
                        </span>
                      )}
                      {gptsovitsService?.dual_instance && gptsovitsService.instances && gptsovitsService.instances.length > 0 && (
                        <div style={{ width: '100%', display: 'flex', gap: 8, flexWrap: 'wrap', marginTop: 2 }}>
                          {gptsovitsService.instances.map((inst) => (
                            <span
                              key={inst.port}
                              style={{
                                fontSize: 10,
                                padding: '2px 6px',
                                borderRadius: 3,
                                background: inst.status === 'running'
                                  ? 'rgba(39,174,96,0.12)'
                                  : inst.status === 'crashed'
                                    ? 'rgba(231,76,60,0.12)'
                                    : 'var(--panel-bg-hover)',
                                color: inst.status === 'running'
                                  ? '#27ae60'
                                  : inst.status === 'crashed'
                                    ? '#e74c3c'
                                    : 'var(--panel-text-tertiary)',
                              }}
                            >
                              :{inst.port} {inst.status === 'running' ? '✓' : inst.status === 'starting' ? '...' : inst.status === 'crashed' ? '✗' : '○'}
                              {inst.pid ? ` PID:${inst.pid}` : ''}
                            </span>
                          ))}
                        </div>
                      )}
                      {gptsovitsService?.error && (
                        <div
                          style={{
                            width: '100%',
                            fontSize: 11,
                            color: '#e74c3c',
                            background: 'rgba(231,76,60,0.08)',
                            padding: '6px 8px',
                            borderRadius: 4,
                            marginTop: 4,
                            wordBreak: 'break-all',
                          }}
                        >
                          {gptsovitsService.error}
                        </div>
                      )}
                      {!ttsConfig.gpt_sovits_install_path && (
                        <div
                          style={{
                            width: '100%',
                            fontSize: 11,
                            color: 'var(--panel-text-tertiary)',
                            marginTop: 4,
                          }}
                        >
                          {t('config.gptsovits_hint_install_required')}
                        </div>
                      )}
                    </div>
                    {/* 自动启动开关：仅在填写了安装路径后显示 */}
                    {ttsConfig.gpt_sovits_install_path && (
                      <ToggleField
                        label={t('config.field_tts_gptsovits_auto_start')}
                        value={ttsConfig.gpt_sovits_auto_start}
                        onChange={(v) => setTtsConfig({ ...ttsConfig, gpt_sovits_auto_start: v })}
                      />
                    )}

                    {/* 安装路径（始终显示，决定后续字段可见性） */}
                    <BrowseTextField
                      label={t('config.field_tts_gptsovits_install_path')}
                      value={ttsConfig.gpt_sovits_install_path ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, gpt_sovits_install_path: v || null })}
                      placeholder={t('config.placeholder_gptsovits_install_path')}
                      onBrowse={() => pickPath('gpt_sovits_install_path', true)}
                      browseLabel={t('config.btn_browse')}
                    />

                    {/* 部署参数 — 仅在填写了安装路径后显示 */}
                    {ttsConfig.gpt_sovits_install_path && (
                      <>
                        <div style={subsectionTitleStyle}>
                          {t('config.gptsovits_section_deploy')}
                        </div>
                        <SelectField
                          label={t('config.field_tts_gptsovits_gpt_model')}
                          value={ttsConfig.gpt_sovits_gpt_model ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, gpt_sovits_gpt_model: v || null })}
                          options={[
                            { value: '', label: t('config.opt_gptsovits_model_none') },
                            ...gptSovitsModels.gpt_models.map((m) => ({ value: m.path, label: m.name })),
                          ]}
                        />
                        <SelectField
                          label={t('config.field_tts_gptsovits_sovits_model')}
                          value={ttsConfig.gpt_sovits_sovits_model ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, gpt_sovits_sovits_model: v || null })}
                          options={[
                            { value: '', label: t('config.opt_gptsovits_model_none') },
                            ...gptSovitsModels.sovits_models.map((m) => ({ value: m.path, label: m.name })),
                          ]}
                        />
                        {gptSovitsModels.gpt_models.length === 0 &&
                          gptSovitsModels.sovits_models.length === 0 && (
                            <div
                              style={{
                                width: '100%',
                                fontSize: 11,
                                color: '#e67e22',
                                background: 'rgba(230,126,34,0.08)',
                                padding: '6px 8px',
                                borderRadius: 4,
                                marginBottom: 8,
                              }}
                            >
                              {t('config.gptsovits_hint_no_models')}
                            </div>
                          )}
                        <TextField
                          label={t('config.field_tts_gptsovits_gpu')}
                          value={ttsConfig.gpt_sovits_gpu?.toString() ?? '0'}
                          onChange={(v) => {
                            const n = parseInt(v, 10);
                            setTtsConfig({ ...ttsConfig, gpt_sovits_gpu: isNaN(n) ? null : n });
                          }}
                          placeholder="0（GPU 卡号；-1 为 CPU 推理）"
                          type="number"
                        />
                        <TextField
                          label={t('config.field_tts_gptsovits_python_path')}
                          value={ttsConfig.gpt_sovits_python_path ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, gpt_sovits_python_path: v || null })}
                          placeholder={t('config.placeholder_gptsovits_python_path')}
                        />
                        {!gptSovitsModels.has_runtime && !ttsConfig.gpt_sovits_python_path && (
                          <div
                            style={{
                              width: '100%',
                              fontSize: 11,
                              color: '#e67e22',
                              background: 'rgba(230,126,34,0.08)',
                              padding: '6px 8px',
                              borderRadius: 4,
                              marginBottom: 8,
                            }}
                          >
                            {t('config.gptsovits_hint_no_runtime')}
                          </div>
                        )}
                        <TextField
                          label={t('config.field_tts_gptsovits_port')}
                          value={ttsConfig.gpt_sovits_port?.toString() ?? ''}
                          onChange={(v) => {
                            const n = parseInt(v, 10);
                            setTtsConfig({ ...ttsConfig, gpt_sovits_port: isNaN(n) ? null : n });
                          }}
                          placeholder={t('config.placeholder_gptsovits_port')}
                          type="number"
                        />
                      </>
                    )}

                    {/* 参考音频 — 决定音色 */}
                    <div style={subsectionTitleStyle}>
                      {t('config.gptsovits_section_reference')}
                    </div>
                    <BrowseTextField
                      label={t('config.field_tts_gptsovits_ref_audio')}
                      value={ttsConfig.gpt_sovits_ref_audio ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, gpt_sovits_ref_audio: v || null })}
                      placeholder={t('config.placeholder_gptsovits_ref_audio')}
                      onBrowse={() => pickPath('gpt_sovits_ref_audio', false, ['wav', 'mp3', 'flac'])}
                      browseLabel={t('config.btn_browse')}
                    />
                    <SelectField
                      label={t('config.field_tts_gptsovits_prompt_lang')}
                      value={ttsConfig.gpt_sovits_prompt_lang ?? 'zh'}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, gpt_sovits_prompt_lang: v || null })}
                      options={[
                        { value: 'zh', label: '中文 (zh)' },
                        { value: 'en', label: 'English (en)' },
                        { value: 'ja', label: '日本語 (ja)' },
                        { value: 'ko', label: '한국어 (ko)' },
                        { value: 'yue', label: '粤语 (yue)' },
                      ]}
                    />
                    <TextField
                      label={t('config.field_tts_gptsovits_prompt_text')}
                      value={ttsConfig.gpt_sovits_prompt_text ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, gpt_sovits_prompt_text: v || null })}
                      placeholder={t('config.placeholder_gptsovits_prompt_text')}
                    />
                    {/* 辅助参考音频(不限数量) — 可折叠抽屉 */}
                    <AuxRefAudiosDrawer
                      value={ttsConfig.gpt_sovits_aux_ref_audios}
                      onChange={(arr) =>
                        setTtsConfig({
                          ...ttsConfig,
                          gpt_sovits_aux_ref_audios: arr.length ? arr : null,
                        })
                      }
                      label={t('config.field_tts_gptsovits_aux_ref_audios')}
                      addLabel={t('config.btn_gptsovits_add_aux')}
                    />

                    {/* 服务地址（启动后自动填入；远程服务手填） */}
                    <TextField
                      label={t('config.field_tts_gptsovits_url')}
                      value={ttsConfig.gpt_sovits_url ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, gpt_sovits_url: v || null })}
                      placeholder={t('config.placeholder_gptsovits_url')}
                    />

                    {/* 高级参数抽屉 */}
                    <details style={{ marginTop: 8 }}>
                      <summary
                        style={{
                          fontSize: 11,
                          color: 'var(--panel-text-tertiary)',
                          fontWeight: 600,
                          cursor: 'pointer',
                          userSelect: 'none',
                          padding: '6px 0',
                        }}
                      >
                        {t('config.gptsovits_section_advanced')}
                      </summary>
                      <div style={{ marginTop: 8, display: 'flex', flexDirection: 'column', gap: 8 }}>
                        <SelectField
                          label={t('config.field_tts_gptsovits_parallel_infer')}
                          value={ttsConfig.gpt_sovits_parallel_infer === false ? 'false' : 'true'}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, gpt_sovits_parallel_infer: v === 'true' })}
                          options={[
                            { value: 'true', label: t('config.opt_gptsovits_parallel_on') },
                            { value: 'false', label: t('config.opt_gptsovits_parallel_off') },
                          ]}
                        />
                        <SelectField
                          label={t('config.field_tts_gptsovits_text_split_method')}
                          value={ttsConfig.gpt_sovits_text_split_method ?? 'cut5'}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, gpt_sovits_text_split_method: v || null })}
                          options={[
                            { value: 'cut0', label: t('config.opt_gptsovits_cut0') },
                            { value: 'cut1', label: t('config.opt_gptsovits_cut1') },
                            { value: 'cut2', label: t('config.opt_gptsovits_cut2') },
                            { value: 'cut3', label: t('config.opt_gptsovits_cut3') },
                            { value: 'cut4', label: t('config.opt_gptsovits_cut4') },
                            { value: 'cut5', label: t('config.opt_gptsovits_cut5') },
                          ]}
                        />
                        <TextField
                          label={t('config.field_tts_gptsovits_top_k')}
                          value={ttsConfig.gpt_sovits_top_k?.toString() ?? ''}
                          onChange={(v) => {
                            const n = parseInt(v, 10);
                            setTtsConfig({ ...ttsConfig, gpt_sovits_top_k: isNaN(n) ? null : n });
                          }}
                          placeholder="15（默认）"
                          type="number"
                        />
                        <TextField
                          label={t('config.field_tts_gptsovits_top_p')}
                          value={ttsConfig.gpt_sovits_top_p?.toString() ?? ''}
                          onChange={(v) => {
                            const n = parseFloat(v);
                            setTtsConfig({ ...ttsConfig, gpt_sovits_top_p: isNaN(n) ? null : n });
                          }}
                          placeholder="1.0（默认）"
                          type="number"
                        />
                        <TextField
                          label={t('config.field_tts_gptsovits_temperature')}
                          value={ttsConfig.gpt_sovits_temperature?.toString() ?? ''}
                          onChange={(v) => {
                            const n = parseFloat(v);
                            setTtsConfig({ ...ttsConfig, gpt_sovits_temperature: isNaN(n) ? null : n });
                          }}
                          placeholder="1.0（默认）"
                          type="number"
                        />
                        <BrowseTextField
                          label={t('config.field_tts_gptsovits_config_path')}
                          value={ttsConfig.gpt_sovits_config_path ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, gpt_sovits_config_path: v || null })}
                          placeholder={t('config.placeholder_gptsovits_config_path')}
                          onBrowse={() => pickPath('gpt_sovits_config_path', false, ['yaml', 'yml'])}
                          browseLabel={t('config.btn_browse')}
                        />
                      </div>
                    </details>
                  </>
                )}

                {/* Fish Speech 配置 */}
                {ttsConfig.engine === 'fishspeech' && (
                  <>
                    <div
                      style={{
                        ...sectionTitleStyle,
                        marginTop: 16,
                        fontSize: 12,
                        display: 'flex',
                        alignItems: 'center',
                        justifyContent: 'space-between',
                      }}
                    >
                      <span>{t('config.section_tts_fishspeech')}</span>
                    </div>

                    {/* 服务控制面板 — 一键启动 / 停止 + 状态显示 */}
                    <div
                      style={{
                        padding: '12px 14px',
                        border: '1px solid var(--panel-border)',
                        borderRadius: 8,
                        background: 'var(--panel-bg-surface)',
                        display: 'flex',
                        alignItems: 'center',
                        gap: 10,
                        flexWrap: 'wrap',
                        marginBottom: 8,
                      }}
                    >
                      <button
                        onClick={toggleFishSpeechService}
                        disabled={fishSpeechServiceBusy || !ttsConfig.fish_speech_install_path}
                        style={{
                          padding: '6px 14px',
                          border: 'none',
                          borderRadius: 6,
                          background:
                            fishSpeechService?.status === 'running'
                              ? '#e74c3c'
                              : '#27ae60',
                          color: '#fff',
                          fontSize: 12,
                          cursor:
                            fishSpeechServiceBusy || !ttsConfig.fish_speech_install_path
                              ? 'not-allowed'
                              : 'pointer',
                          fontFamily: 'inherit',
                          opacity:
                            fishSpeechServiceBusy || !ttsConfig.fish_speech_install_path ? 0.6 : 1,
                          display: 'inline-flex',
                          alignItems: 'center',
                          gap: 6,
                        }}
                      >
                        {fishSpeechServiceBusy && (
                          <span
                            style={{
                              display: 'inline-block',
                              width: 12,
                              height: 12,
                              border: '2px solid rgba(0,0,0,0.3)',
                              borderTopColor: '#fff',
                              borderRadius: '50%',
                              animation: 'gptsovits-spin 0.8s linear infinite',
                              flexShrink: 0,
                            }}
                          />
                        )}
                        {fishSpeechService?.status === 'running'
                          ? fishSpeechServiceBusy
                            ? t('config.fishspeech_status_stopping')
                            : t('config.btn_fishspeech_stop')
                          : fishSpeechService?.status === 'starting'
                            ? t('config.fishspeech_status_starting')
                            : fishSpeechService?.status === 'stopping'
                              ? t('config.fishspeech_status_stopping')
                              : fishSpeechServiceBusy
                                ? t('config.fishspeech_status_starting')
                                : t('config.btn_fishspeech_start')}
                      </button>
                      <span
                        style={{
                          fontSize: 11,
                          padding: '2px 8px',
                          borderRadius: 4,
                          background:
                            fishSpeechService?.status === 'running'
                              ? 'rgba(39,174,96,0.15)'
                              : fishSpeechService?.status === 'crashed'
                                ? 'rgba(231,76,60,0.15)'
                                : 'var(--panel-bg-surface-elevated)',
                          color:
                            fishSpeechService?.status === 'running'
                              ? '#27ae60'
                              : fishSpeechService?.status === 'crashed'
                                ? '#e74c3c'
                                : 'var(--panel-text-secondary)',
                        }}
                      >
                        {(() => {
                          const s = fishSpeechService?.status ?? 'stopped';
                          switch (s) {
                            case 'running':
                              return t('config.fishspeech_status_running');
                            case 'starting':
                              return t('config.fishspeech_status_starting');
                            case 'stopping':
                              return t('config.fishspeech_status_stopping');
                            case 'crashed':
                              return t('config.fishspeech_status_crashed');
                            default:
                              return t('config.fishspeech_status_stopped');
                          }
                        })()}
                      </span>
                      {fishSpeechService?.pid && (
                        <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)' }}>
                          PID: {fishSpeechService.pid}
                        </span>
                      )}
                      {fishSpeechService?.endpoint && (
                        <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)' }}>
                          {fishSpeechService.endpoint}
                        </span>
                      )}
                      {fishSpeechService?.error && (
                        <div
                          style={{
                            width: '100%',
                            fontSize: 11,
                            color: '#e74c3c',
                            background: 'rgba(231,76,60,0.08)',
                            padding: '6px 8px',
                            borderRadius: 4,
                            marginTop: 4,
                            wordBreak: 'break-all',
                          }}
                        >
                          {fishSpeechService.error}
                        </div>
                      )}
                      {!ttsConfig.fish_speech_install_path && (
                        <div
                          style={{
                            width: '100%',
                            fontSize: 11,
                            color: 'var(--panel-text-tertiary)',
                            marginTop: 4,
                          }}
                        >
                          {t('config.fishspeech_hint_install_required')}
                        </div>
                      )}
                    </div>
                    {/* 自动启动开关：仅在填写了安装路径后显示 */}
                    {ttsConfig.fish_speech_install_path && (
                      <ToggleField
                        label={t('config.field_tts_fishspeech_auto_start')}
                        value={ttsConfig.fish_speech_auto_start}
                        onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_auto_start: v })}
                      />
                    )}

                    {/* 安装路径（决定后续字段可见性） */}
                    <BrowseTextField
                      label={t('config.field_tts_fishspeech_install_path')}
                      value={ttsConfig.fish_speech_install_path ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_install_path: v || null })}
                      placeholder={t('config.placeholder_fishspeech_install_path')}
                      onBrowse={() => pickPath('fish_speech_install_path', true)}
                      browseLabel={t('config.btn_browse')}
                    />

                    {/* 部署参数 — 仅在填写了安装路径后显示 */}
                    {ttsConfig.fish_speech_install_path && (
                      <>
                        <div style={subsectionTitleStyle}>
                          {t('config.fishspeech_section_deploy')}
                        </div>
                        <TextField
                          label={t('config.field_tts_fishspeech_python_path')}
                          value={ttsConfig.fish_speech_python_path ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_python_path: v || null })}
                          placeholder={t('config.placeholder_fishspeech_python_path')}
                        />
                        <TextField
                          label={t('config.field_tts_fishspeech_port')}
                          value={ttsConfig.fish_speech_port?.toString() ?? ''}
                          onChange={(v) => {
                            const n = parseInt(v, 10);
                            setTtsConfig({ ...ttsConfig, fish_speech_port: isNaN(n) ? null : n });
                          }}
                          placeholder={t('config.placeholder_fishspeech_port')}
                          type="number"
                        />
                        <BrowseTextField
                          label={t('config.field_tts_fishspeech_llama_checkpoint')}
                          value={ttsConfig.fish_speech_llama_checkpoint_path ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_llama_checkpoint_path: v || null })}
                          placeholder={t('config.placeholder_fishspeech_llama_checkpoint')}
                          onBrowse={() => pickPath('fish_speech_llama_checkpoint_path', false, ['ckpt', 'pt', 'pth', 'bin', 'safetensors'])}
                          browseLabel={t('config.btn_browse')}
                        />
                        <BrowseTextField
                          label={t('config.field_tts_fishspeech_decoder_checkpoint')}
                          value={ttsConfig.fish_speech_decoder_checkpoint_path ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_decoder_checkpoint_path: v || null })}
                          placeholder={t('config.placeholder_fishspeech_decoder_checkpoint')}
                          onBrowse={() => pickPath('fish_speech_decoder_checkpoint_path', false, ['ckpt', 'pt', 'pth', 'bin', 'safetensors'])}
                          browseLabel={t('config.btn_browse')}
                        />
                        <ToggleField
                          label={t('config.field_tts_fishspeech_half')}
                          value={ttsConfig.fish_speech_half}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_half: v })}
                        />
                        <ToggleField
                          label={t('config.field_tts_fishspeech_compile')}
                          value={ttsConfig.fish_speech_compile}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_compile: v })}
                        />
                      </>
                    )}

                    <TextField
                      label={t('config.field_tts_fishspeech_url')}
                      value={ttsConfig.fish_speech_url ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_url: v || null })}
                      placeholder="http://127.0.0.1:8080（本地部署），留空使用云端 https://api.fish.audio"
                    />
                    <TextField
                      label={t('config.field_tts_fishspeech_character')}
                      value={ttsConfig.fish_speech_character ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_character: v || null })}
                      placeholder="音色模型 ID（如 7f92f8afb8ec43bf81429cc1c9199cb1，留空则用下方参考音频）"
                    />
                    <AdvancedToggle
                      open={fishSpeechAdvancedOpen}
                      onToggle={() => setFishSpeechAdvancedOpen((v) => !v)}
                      label={t('config.section_advanced_settings')}
                    />
                    {fishSpeechAdvancedOpen && (
                      <>
                        <TextField
                          label={t('config.field_tts_fishspeech_key')}
                          value={ttsConfig.fish_speech_key ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_key: v || null })}
                          placeholder="fish.audio 的 API Key（云端必填，本地可留空）"
                          type="password"
                        />
                        <SelectField
                          label={t('config.field_tts_fishspeech_format')}
                          value={ttsConfig.fish_speech_format ?? 'wav'}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_format: v || null })}
                          options={[
                            { value: 'wav', label: 'WAV（推荐，无损）' },
                            { value: 'mp3', label: 'MP3' },
                            { value: 'pcm', label: 'PCM' },
                            { value: 'opus', label: 'Opus' },
                          ]}
                        />
                        <TextField
                          label={t('config.field_tts_fishspeech_ref_audio')}
                          value={ttsConfig.fish_speech_ref_audio ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_ref_audio: v || null })}
                          placeholder="D:/voices/sample.wav（5-10 秒，零样本克隆，与参考 ID 二选一）"
                        />
                        <TextField
                          label={t('config.field_tts_fishspeech_ref_text')}
                          value={ttsConfig.fish_speech_ref_text ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, fish_speech_ref_text: v || null })}
                          placeholder="参考音频对应的文字转写（配合上方参考音频使用）"
                        />
                      </>
                    )}
                  </>
                )}

                {/* MiniMax 配置 */}
                {ttsConfig.engine === 'minimax' && (
                  <>
                    <div
                      style={{
                        ...sectionTitleStyle,
                        marginTop: 16,
                        fontSize: 12,
                        display: 'flex',
                        alignItems: 'center',
                        justifyContent: 'space-between',
                      }}
                    >
                      <span>{t('config.section_tts_minimax')}</span>
                    </div>
                    <TextField
                      label={t('config.field_tts_minimax_key')}
                      value={ttsConfig.minimax_key ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, minimax_key: v || null })}
                      placeholder="MiniMax 平台 API Key"
                      type="password"
                    />
                    <TextField
                      label={t('config.field_tts_minimax_voice_id')}
                      value={ttsConfig.minimax_voice_id ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, minimax_voice_id: v || null })}
                      placeholder="音色 ID（在平台创建音色后获得，如 voice_id xxx）"
                    />
                    <AdvancedToggle
                      open={minimaxAdvancedOpen}
                      onToggle={() => setMinimaxAdvancedOpen((v) => !v)}
                      label={t('config.section_advanced_settings')}
                    />
                    {minimaxAdvancedOpen && (
                      <>
                        <SelectField
                          label={t('config.field_tts_minimax_model')}
                          value={ttsConfig.minimax_model ?? 'speech-01-turbo'}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, minimax_model: v || null })}
                          options={[
                            { value: 'speech-01-turbo', label: t('config.opt_tts_minimax_model_turbo') },
                            { value: 'speech-01-hd', label: t('config.opt_tts_minimax_model_hd') },
                          ]}
                        />
                        <SelectField
                          label={t('config.field_tts_minimax_format')}
                          value={ttsConfig.minimax_format ?? 'mp3'}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, minimax_format: v || null })}
                          options={[
                            { value: 'mp3', label: 'MP3（推荐）' },
                            { value: 'wav', label: 'WAV（无损）' },
                            { value: 'pcm', label: 'PCM' },
                          ]}
                        />
                        <SelectField
                          label={t('config.field_tts_minimax_sample_rate')}
                          value={String(ttsConfig.minimax_sample_rate ?? 32000)}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, minimax_sample_rate: Number(v) })}
                          options={[
                            { value: '32000', label: '32000 Hz（默认）' },
                            { value: '24000', label: '24000 Hz' },
                            { value: '16000', label: '16000 Hz' },
                          ]}
                        />
                      </>
                    )}
                  </>
                )}

                {/* 豆包(火山引擎)配置 */}
                {ttsConfig.engine === 'doubao' && (
                  <>
                    <div
                      style={{
                        ...sectionTitleStyle,
                        marginTop: 16,
                        display: 'flex',
                        alignItems: 'center',
                        justifyContent: 'space-between',
                      }}
                    >
                      <span>{t('config.section_tts_doubao')}</span>
                    </div>
                    <TextField
                      label={t('config.field_tts_doubao_appid')}
                      value={ttsConfig.doubao_appid ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, doubao_appid: v || null })}
                      placeholder="火山引擎应用 ID (App ID)"
                    />
                    <TextField
                      label={t('config.field_tts_doubao_access_token')}
                      value={ttsConfig.doubao_access_token ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, doubao_access_token: v || null })}
                      placeholder="访问令牌 (Access Token)"
                      type="password"
                    />
                    <TextField
                      label={t('config.field_tts_doubao_voice_type')}
                      value={ttsConfig.doubao_voice_type ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, doubao_voice_type: v || null })}
                      placeholder="BV700_V2_streaming（灿灿 2.0，推荐）/ BV001_V2_streaming（通用女声 2.0）"
                    />
                    <AdvancedToggle
                      open={doubaoAdvancedOpen}
                      onToggle={() => setDoubaoAdvancedOpen((v) => !v)}
                      label={t('config.section_advanced_settings')}
                    />
                    {doubaoAdvancedOpen && (
                      <>
                        <TextField
                          label={t('config.field_tts_doubao_cluster')}
                          value={ttsConfig.doubao_cluster ?? ''}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, doubao_cluster: v || null })}
                          placeholder="volcano_tts（默认，声音复刻可能需修改）"
                        />
                        <SelectField
                          label={t('config.field_tts_doubao_format')}
                          value={ttsConfig.doubao_format ?? 'mp3'}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, doubao_format: v || null })}
                          options={[
                            { value: 'mp3', label: 'MP3（推荐）' },
                            { value: 'wav', label: 'WAV（无损）' },
                            { value: 'pcm', label: 'PCM' },
                            { value: 'ogg_opus', label: 'OGG Opus' },
                          ]}
                        />
                        <SelectField
                          label={t('config.field_tts_doubao_sample_rate')}
                          value={String(ttsConfig.doubao_sample_rate ?? 24000)}
                          onChange={(v) => setTtsConfig({ ...ttsConfig, doubao_sample_rate: Number(v) })}
                          options={[
                            { value: '24000', label: '24000 Hz（默认）' },
                            { value: '16000', label: '16000 Hz' },
                            { value: '8000', label: '8000 Hz' },
                          ]}
                        />
                      </>
                    )}
                  </>
                )}

                {/* MiMo（小米，语音克隆）配置 */}
                {ttsConfig.engine === 'mimo' && (
                  <>
                    <div
                      style={{
                        ...sectionTitleStyle,
                        marginTop: 16,
                        fontSize: 12,
                        display: 'flex',
                        alignItems: 'center',
                        justifyContent: 'space-between',
                      }}
                    >
                      <span>{t('config.section_tts_mimo')}</span>
                    </div>
                    <TextField
                      label={t('config.field_tts_mimo_key')}
                      value={ttsConfig.mimo_key ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, mimo_key: v || null })}
                      placeholder="MiMo 平台 API Key"
                      type="password"
                    />
                    <TextField
                      label={t('config.field_tts_mimo_voice_audio')}
                      value={ttsConfig.mimo_voice_audio_path ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, mimo_voice_audio_path: v || null })}
                      placeholder={t('config.ph_tts_mimo_voice_audio')}
                      help={t('config.field_tts_mimo_voice_audio_help')}
                    />
                    <TextField
                      label={t('config.field_tts_mimo_style_prompt')}
                      value={ttsConfig.mimo_style_prompt ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, mimo_style_prompt: v || null })}
                      placeholder={t('config.ph_tts_mimo_style_prompt')}
                    />
                    <TextField
                      label={t('config.field_tts_mimo_endpoint')}
                      value={ttsConfig.mimo_endpoint ?? ''}
                      onChange={(v) => setTtsConfig({ ...ttsConfig, mimo_endpoint: v || null })}
                      placeholder="https://api.xiaomimimo.com/v1/chat/completions（默认）"
                    />
                  </>
                )}

                {/* ── 跨语言 TTS：显示语言与 TTS 语言不同时启用翻译 ── */}
                <div style={subsectionTitleStyle}>
                  {t('config.section_tts_cross_lang')}
                </div>
                <SelectField
                  label={t('config.field_display_language')}
                  value={ttsConfig.display_language ?? ''}
                  onChange={(v) => setTtsConfig({ ...ttsConfig, display_language: v || null })}
                  options={[
                    { value: '', label: t('config.opt_lang_same_as_system') },
                    { value: 'zh', label: '中文' },
                    { value: 'ja', label: '日本語' },
                    { value: 'en', label: 'English' },
                  ]}
                />
                <SelectField
                  label={t('config.field_tts_language')}
                  value={ttsConfig.tts_language ?? ''}
                  onChange={(v) => {
                    const next = { ...ttsConfig, tts_language: v || null };
                    // 选了 TTS 语言但显示语言为空时，自动填充中文（角色默认中文对话）
                    if (v && !next.display_language) {
                      next.display_language = 'zh';
                    }
                    setTtsConfig(next);
                  }}
                  options={[
                    { value: '', label: t('config.opt_lang_same_as_display') },
                    { value: 'zh', label: '中文' },
                    { value: 'ja', label: '日本語' },
                    { value: 'en', label: 'English' },
                  ]}
                />
                {/* 翻译服务配置：仅在 display_language 和 tts_language 都已设置且不同时显示 */}
                {ttsConfig.display_language &&
                  ttsConfig.tts_language &&
                  ttsConfig.display_language !== ttsConfig.tts_language && (
                    <>
                      <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', marginBottom: 6 }}>
                        {t('config.translation_provider_hint')}
                      </div>
                      <SelectField
                        label={t('config.field_translation_provider')}
                        value={ttsConfig.translation_provider ?? 'google'}
                        onChange={(v) => setTtsConfig({ ...ttsConfig, translation_provider: v || null })}
                        options={[
                          { value: 'google', label: 'Google Translate' },
                          { value: 'deepl', label: 'DeepL' },
                          { value: 'llm', label: 'LLM' },
                        ]}
                      />
                      {ttsConfig.translation_provider === 'llm' ? (
                        <>
                          <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', marginBottom: 6 }}>
                            {t('config.translation_llm_hint')}
                          </div>
                          <ProviderSelector
                            pathPrefix="routing_matrix.translation"
                            get={get}
                            setNested={setNested}
                            t={t}
                          />
                          <TextField
                            label={t('config.field_model_name')}
                            value={get('routing_matrix.translation.model', '')}
                            onChange={(v) => setNested('routing_matrix.translation.model', v)}
                          />
                          <TextField
                            label={t('config.field_api_key')}
                            type="password"
                            value={get('routing_matrix.translation.api_key', '')}
                            onChange={(v) => setNested('routing_matrix.translation.api_key', v)}
                          />
                          {needsSecretFor(get('routing_matrix.translation.provider_type', 'openai')) && (
                            <TextField
                              label={t('config.field_api_secret')}
                              type="password"
                              value={get('routing_matrix.translation.api_secret', '')}
                              onChange={(v) => setNested('routing_matrix.translation.api_secret', v)}
                            />
                          )}
                          {needsAppIdFor(get('routing_matrix.translation.provider_type', 'openai')) && (
                            <TextField
                              label={t('config.field_app_id')}
                              value={get('routing_matrix.translation.app_id', '')}
                              onChange={(v) => setNested('routing_matrix.translation.app_id', v)}
                            />
                          )}
                          <TextField
                            label={t('config.field_endpoint')}
                            value={get('routing_matrix.translation.endpoint', '')}
                            onChange={(v) => setNested('routing_matrix.translation.endpoint', v)}
                          />
                          <SliderField
                            label={t('config.field_temperature')}
                            value={get('routing_matrix.translation.temperature', get('ai.temperature', 0.70))}
                            onChange={(v) => setNested('routing_matrix.translation.temperature', v)}
                            min={0}
                            max={2}
                            step={0.05}
                            format={(v) => v.toFixed(2)}
                            help={t('config.field_route_temperature_help')}
                          />
                          <NumberField
                            label={t('config.field_max_tokens')}
                            value={get('routing_matrix.translation.max_tokens', get('ai.max_tokens', 2048))}
                            onChange={(v) => setNested('routing_matrix.translation.max_tokens', v)}
                            min={64}
                            step={64}
                            help={t('config.field_route_max_tokens_help')}
                          />
                        </>
                      ) : (
                        <>
                          <TextField
                            label={t('config.field_translation_api_key')}
                            value={ttsConfig.translation_api_key ?? ''}
                            onChange={(v) => setTtsConfig({ ...ttsConfig, translation_api_key: v || null })}
                            placeholder={t('config.placeholder_translation_api_key')}
                            type="password"
                          />
                          <TextField
                            label={t('config.field_translation_endpoint')}
                            value={ttsConfig.translation_endpoint ?? ''}
                            onChange={(v) => setTtsConfig({ ...ttsConfig, translation_endpoint: v || null })}
                            placeholder={t('config.placeholder_translation_endpoint')}
                          />
                        </>
                      )}
                      <div style={{ display: 'flex', gap: 10, marginBottom: 8 }}>
                        <button
                          type="button"
                          onClick={async () => {
                            try {
                              const result = await invoke<string>('test_translation', {
                                text: '你好，今天天气真好。',
                                characterId: ttsEditCharId ?? undefined,
                              });
                              emit('toast:show', {
                                type: 'success',
                                message: `${t('config.toast_translation_test_ok')}: ${result}`,
                                duration: 5000,
                              });
                            } catch (e) {
                              emit('toast:show', {
                                type: 'error',
                                message: `${t('config.toast_translation_test_failed')}: ${e}`,
                                duration: 6000,
                              });
                            }
                          }}
                          style={{
                            padding: '6px 14px',
                            border: '1px solid var(--panel-border)',
                            borderRadius: 6,
                            background: 'transparent',
                            color: 'var(--panel-text)',
                            fontSize: 12,
                            cursor: 'pointer',
                            fontFamily: 'inherit',
                          }}
                        >
                          {t('config.btn_test_translation')}
                        </button>
                      </div>
                    </>
                  )}

                <div style={{ display: 'flex', gap: 10, marginTop: 12 }}>
                  <button
                    onClick={handleTestTts}
                    style={{
                      flex: 1,
                      padding: '8px 12px',
                      border: '1px solid var(--panel-border)',
                      background: 'transparent',
                      color: 'var(--panel-text)',
                      borderRadius: 6,
                      fontSize: 13,
                      cursor: 'pointer',
                      fontFamily: 'inherit',
                    }}
                  >
                    {t('config.btn_test_tts')}
                  </button>
                </div>
              </>
            ) : (
              <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)' }}>{t('common.loading')}</div>
            )}

            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>{t('config.section_realtime')}</div>
            <ToggleField
              label={t('config.field_realtime_enable')}
              value={get('realtime_voice.enabled', false) as boolean}
              onChange={(v) => {
                setNested('realtime_voice.enabled', v);
                // 启用时强制固定输入模式为麦克风音频
                if (v) setNested('realtime_voice.input_mod', 'audio');
              }}
            />
            {get('realtime_voice.enabled', false) && (
              <>
                <SelectField
                  label={t('config.field_realtime_provider')}
                  value={get('realtime_voice.provider', 'doubao') as string}
                  onChange={(v) => setNested('realtime_voice.provider', v)}
                  options={[
                    { value: 'doubao', label: t('config.opt_realtime_provider_doubao') },
                    { value: 'gpt_live', label: t('config.opt_realtime_provider_gpt_live'), disabled: true },
                  ]}
                />
                {get('realtime_voice.provider', 'doubao') === 'doubao' && (
                  <>
                    <TextField
                      label={t('config.field_realtime_app_id')}
                      value={get('realtime_voice.app_id', '') as string}
                      onChange={(v) => setNested('realtime_voice.app_id', v)}
                      placeholder="1234567890"
                    />
                    <TextField
                      label={t('config.field_realtime_access_key')}
                      value={get('realtime_voice.access_key', '') as string}
                      onChange={(v) => setNested('realtime_voice.access_key', v)}
                      placeholder="your-access-key"
                      type="password"
                    />
                    <SelectField
                      label={t('config.field_realtime_model')}
                      value={get('realtime_voice.model', 'SC') as string}
                      onChange={(v) => setNested('realtime_voice.model', v)}
                      options={[
                        { value: 'SC', label: 'SC (声音复刻克隆音色)' },
                        { value: 'O', label: 'O (精品音色+联网)' },
                      ]}
                    />
                    <TextField
                      label={t('config.field_realtime_speaker')}
                      value={get('realtime_voice.speaker', '') as string}
                      onChange={(v) => setNested('realtime_voice.speaker', v)}
                      placeholder="ICL_zh_female_wenrouwenya_tob"
                    />
                    <TextField
                      label={t('config.field_realtime_end_smooth_window')}
                      value={String(get('realtime_voice.end_smooth_window_ms', 1500))}
                      onChange={(v) => setNested('realtime_voice.end_smooth_window_ms', Number(v) || 1500)}
                      placeholder="1500"
                    />
                  </>
                )}
              </>
            )}
          </>
        );

      case 'network':
        return (
          <>
            <div style={sectionTitleStyle}>{t('config.section_network')}</div>
            <SelectField
              label={t('config.field_proxy_mode')}
              value={get('network.proxy_mode', 'direct')}
              onChange={(v) => setNested('network.proxy_mode', v)}
              options={[
                { value: 'direct', label: t('config.opt_no_proxy') },
                { value: 'system', label: t('config.opt_system_proxy') },
                { value: 'manual', label: t('config.opt_manual') },
              ]}
            />
            <TextField
              label={t('config.field_proxy_url')}
              value={get('network.proxy_url', '')}
              onChange={(v) => setNested('network.proxy_url', v)}
              placeholder={t('config.ph_proxy')}
              disabled={get<string>('network.proxy_mode', 'direct') !== 'manual'}
            />
            <NumberField
              label={t('config.field_timeout')}
              value={get('network.timeout', 30)}
              onChange={(v) => setNested('network.timeout', v)}
              min={5}
              step={5}
            />
            <div style={{ ...fieldStyle, display: 'flex', alignItems: 'center', gap: 12 }}>
              <button
                onClick={handleTestConnection}
                disabled={networkTesting}
                style={{
                  padding: '8px 16px',
                  border: '1px solid var(--panel-border)',
                  borderRadius: 6,
                  background: networkTesting ? 'var(--panel-toggle-off)' : 'var(--panel-bg-active)',
                  color: 'var(--panel-text)',
                  fontSize: 13,
                  fontFamily: 'inherit',
                  cursor: networkTesting ? 'not-allowed' : 'pointer',
                  opacity: networkTesting ? 0.6 : 1,
                }}
              >
                {networkTesting ? t('config.test_connection_testing') : t('config.test_connection')}
              </button>
              {networkTesting ? (
                <span style={{ fontSize: 12, color: '#ffffff', fontWeight: 500 }}>
                  {t('config.test_connection_testing')}
                </span>
              ) : (
                networkTestResult && (
                  <span
                    style={{
                      fontSize: 12,
                      color: networkTestSuccess ? '#4caf50' : '#f44336',
                      fontWeight: 500,
                    }}
                  >
                    {networkTestResult}
                  </span>
                )
              )}
            </div>

            {/* ── 远程访问（Tailscale 场景）：启用开关与章节标题同行 ── */}
            <div
              style={{
                ...sectionTitleStyle,
                marginTop: 24,
                display: 'flex',
                justifyContent: 'space-between',
                alignItems: 'center',
                gap: 10,
              }}
            >
              <span style={{ flexShrink: 0 }}>{t('config.section_remote_access')}</span>
              <span style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                <span style={{ fontSize: 11, color: 'var(--panel-text-secondary)', fontWeight: 500 }}>
                  {t('config.field_remote_access_enabled')}
                </span>
                <button
                  onClick={() => setNested('network.remote_access.enabled', !get<boolean>('network.remote_access.enabled', false))}
                  style={{
                    width: 40,
                    height: 22,
                    borderRadius: 11,
                    border: 'none',
                    background: get<boolean>('network.remote_access.enabled', false) ? 'var(--panel-accent)' : 'var(--panel-toggle-off)',
                    position: 'relative',
                    cursor: 'pointer',
                    transition: 'background 0.2s ease',
                    flexShrink: 0,
                  }}
                  aria-label={t('config.field_remote_access_enabled')}
                >
                  <span
                    style={{
                      position: 'absolute',
                      top: 2,
                      left: get<boolean>('network.remote_access.enabled', false) ? 20 : 2,
                      width: 18,
                      height: 18,
                      borderRadius: '50%',
                      background: 'var(--panel-surface)',
                      transition: 'left 0.2s ease',
                      boxShadow: 'var(--panel-shadow-subtle)',
                    }}
                  />
                </button>
              </span>
            </div>
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -8, marginBottom: 14, lineHeight: 1.4 }}>
              {t('config.help_remote_access_enabled')}
            </div>
            <NumberField
              label={t('config.field_remote_access_port')}
              value={get<number>('network.remote_access.port', 8080)}
              onChange={(v) => setNested('network.remote_access.port', v)}
              min={1024}
              max={65535}
              step={1}
              help={t('config.help_remote_access_port')}
            />

            {/* ── 网络搜索（已并入网络页签，多引擎混用）── */}
            <div style={{ ...sectionTitleStyle, marginTop: 24 }}>
              {t('config.section_web_search')}
            </div>

            {/* 多引擎混选：用户可同时启用多个引擎（也是智能体 engines 参数的可选范围） */}
            <MultiCheckboxField
              label={t('config.field_web_search_providers')}
              help={t('config.help_web_search_providers')}
              values={get<string[]>('web_search.providers', ['duckduckgo'])}
              onChange={(next) => setNested('web_search.providers', next)}
              options={[
                { value: 'duckduckgo', label: t('config.opt_web_search_duckduckgo') },
                { value: 'searxng', label: t('config.opt_web_search_searxng') },
                { value: 'tavily', label: t('config.opt_web_search_tavily') },
                { value: 'bing', label: t('config.opt_web_search_bing') },
                { value: 'deepseek', label: t('config.opt_web_search_deepseek') },
              ]}
              minSelected={1}
            />
            <NumberField
              label={t('config.field_web_search_max_results')}
              help={t('config.help_web_search_max_results')}
              value={get('web_search.max_results', 0)}
              onChange={(v) => setNested('web_search.max_results', v)}
              min={0}
              max={20}
              step={1}
            />
            <NumberField
              label={t('config.field_web_search_timeout')}
              value={get('web_search.timeout_secs', 15)}
              onChange={(v) => setNested('web_search.timeout_secs', v)}
              min={5}
              step={5}
            />
            <ToggleField
              label={t('config.field_web_search_bg_fetch')}
              help={t('config.help_web_search_bg_fetch')}
              value={get('web_search.enable_background_knowledge_fetch', true)}
              onChange={(v) => setNested('web_search.enable_background_knowledge_fetch', v)}
            />
            <SelectField
              label={t('config.field_web_search_language')}
              value={get('web_search.language', '')}
              onChange={(v) => setNested('web_search.language', v)}
              options={[
                { value: '', label: t('config.opt_lang_auto') },
                { value: 'zh-CN', label: '简体中文' },
                { value: 'en', label: 'English' },
                { value: 'ja', label: '日本語' },
              ]}
            />

            {/* SearXNG 子配置（启用 SearXNG 时显示） */}
            {get<string[]>('web_search.providers', ['duckduckgo']).includes('searxng') && (
              <CollapsibleSection
                title={t('config.section_web_search_searxng')}
                defaultOpen
              >
                <TextField
                  label={t('config.field_searxng_base_url')}
                  value={get('web_search.searxng.base_url', '')}
                  onChange={(v) => setNested('web_search.searxng.base_url', v)}
                  placeholder="http://localhost:8080"
                />
                <TextField
                  label={t('config.field_searxng_token')}
                  value={get('web_search.searxng.auth_token', '')}
                  onChange={(v) => setNested('web_search.searxng.auth_token', v)}
                  placeholder={t('config.ph_optional')}
                  type="password"
                />
              </CollapsibleSection>
            )}

            {/* Tavily 子配置（启用 Tavily 时显示） */}
            {get<string[]>('web_search.providers', ['duckduckgo']).includes('tavily') && (
              <CollapsibleSection
                title={t('config.section_web_search_tavily')}
                defaultOpen
              >
                <TextField
                  label={t('config.field_tavily_api_key')}
                  value={get('web_search.tavily.api_key', '')}
                  onChange={(v) => setNested('web_search.tavily.api_key', v)}
                  placeholder="tvly-..."
                  type="password"
                />
                <ToggleField
                  label={t('config.field_tavily_include_raw')}
                  value={get('web_search.tavily.include_raw_content', true)}
                  onChange={(v) => setNested('web_search.tavily.include_raw_content', v)}
                />
                <SelectField
                  label={t('config.field_tavily_search_depth')}
                  value={get('web_search.tavily.search_depth', 'basic')}
                  onChange={(v) => setNested('web_search.tavily.search_depth', v)}
                  options={[
                    { value: 'basic', label: t('config.opt_tavily_basic') },
                    { value: 'advanced', label: t('config.opt_tavily_advanced') },
                  ]}
                />
              </CollapsibleSection>
            )}

            {/* DeepSeek 官方原生搜索子配置（启用 deepseek 时显示） */}
            {get<string[]>('web_search.providers', ['duckduckgo']).includes('deepseek') && (
              <CollapsibleSection
                title={t('config.section_web_search_deepseek')}
                defaultOpen
              >
                <TextField
                  label={t('config.field_ds_search_api_key')}
                  value={get('web_search.deepseek.api_key', '')}
                  onChange={(v) => setNested('web_search.deepseek.api_key', v)}
                  placeholder={t('config.ph_ds_search_api_key')}
                  type="password"
                />
                <TextField
                  label={t('config.field_ds_search_model')}
                  value={get('web_search.deepseek.model', '')}
                  onChange={(v) => setNested('web_search.deepseek.model', v)}
                  placeholder={t('config.ph_ds_search_model')}
                />
                <TextField
                  label={t('config.field_ds_search_base_url')}
                  value={get('web_search.deepseek.base_url', 'https://api.deepseek.com/anthropic/v1')}
                  onChange={(v) => setNested('web_search.deepseek.base_url', v)}
                  placeholder="https://api.deepseek.com/anthropic/v1"
                />
                <NumberField
                  label={t('config.field_ds_search_max_uses')}
                  value={get('web_search.deepseek.max_uses', 5)}
                  onChange={(v) => setNested('web_search.deepseek.max_uses', v)}
                  min={1}
                  step={1}
                />
                <NumberField
                  label={t('config.field_ds_search_timeout')}
                  value={get('web_search.deepseek.timeout_secs', 60)}
                  onChange={(v) => setNested('web_search.deepseek.timeout_secs', v)}
                  min={10}
                  step={10}
                />
              </CollapsibleSection>
            )}
          </>
        );
      case 'about':
        return (
          <>
            <div style={sectionTitleStyle}>{t('config.section_about')}</div>
            <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', padding: '32px 0 16px' }}>
              <img
                src="/favicon.ico"
                alt="Vivian"
                style={{ width: 72, height: 72, borderRadius: '50%', marginBottom: 14, display: 'block' }}
              />
              <div style={{ fontSize: 18, fontWeight: 600, marginBottom: 4 }}>Vivian</div>
              <div style={{ fontSize: 13, color: 'var(--panel-text-secondary)', marginBottom: 4 }}>
                {t('config.about_subtitle')}
              </div>
              {appVersion && (
                <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', marginBottom: 24 }}>
                  v{appVersion}
                </div>
              )}
            </div>
            <div style={{ padding: '0 8px' }}>
              {[
                { label: t('config.about_project'), value: (
                  <a href="https://github.com/SpacervalLam/Vivian-ai-desktop-pet" target="_blank" rel="noopener noreferrer" style={{ color: 'var(--panel-text)', textDecoration: 'none' }}>
                    github.com/SpacervalLam/Vivian-ai-desktop-pet
                  </a>
                )},
                { label: t('config.about_contact'), value: 'spacervallam@gmail.com' },
                ...(osInfo ? [{ label: t('config.about_os'), value: osInfo }] : []),
              ].map((row, i) => (
                <div key={row.label} style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', padding: '10px 0', borderBottom: '1px solid var(--panel-border)' }}>
                  <span style={{ fontSize: 13, color: 'var(--panel-text-secondary)' }}>{row.label}</span>
                  <span style={{ fontSize: 13, color: 'var(--panel-text)', textAlign: 'right' }}>{row.value}</span>
                </div>
              ))}
            </div>
          </>
        );
      case 'plugins':
        return <PluginsPanel />;
      case 'browser':
        return <BrowserPanel />;
    }
  }, [
    activeTab,
    config,
    ttsConfig,
    diaryConfig,
    diaryLoading,
    networkTesting,
    networkTestResult,
    networkTestSuccess,
    saving,
    saveError,
    t,
    handleShortcutChange,
    mcpServers,
    mcpEditing,
    mcpSaving,
    detectingLocation,
    appVersion,
    osInfo,
    gptsovitsService,
    gptsovitsServiceBusy,
    gptSovitsModels,
    llmTesting,
    llmTestResults,
    // 工具页搜索与清单：搜索词/清单变化需重算工具开关卡片区
    toolSearch,
    toolList,
    workModels,
  ]);

  return (
    <div
      className="scrapbook scrapbook-bg"
      style={{
        display: 'flex',
        flexDirection: 'column',
        height: '100vh',
        background: 'var(--panel-bg)',
        fontFamily:
          '"Noto Serif SC", "Source Han Serif SC", "Songti SC", "STSong", "SimSun", "PingFang SC", "Microsoft YaHei", serif',
        color: 'var(--panel-text)',
        overflow: 'hidden',
      }}
    >
      <style>{`
        @keyframes gptsovits-spin{to{transform:rotate(360deg)}}
        @keyframes cfg-fade{from{opacity:0;transform:translateY(6px)}to{opacity:1;transform:translateY(0)}}
        .scrapbook input:focus,
        .scrapbook select:focus,
        .scrapbook textarea:focus {
          outline: none;
          border-color: var(--panel-accent) !important;
          box-shadow: 0 0 0 2px var(--panel-accent-soft), var(--panel-shadow-card) !important;
        }
        .scrapbook button { cursor: pointer; }
        .scrapbook button:hover:not(:disabled) { filter: brightness(1.06); }
        .scrapbook button:active:not(:disabled) { transform: translateY(0.5px); }
        .scrapbook input, .scrapbook select, .scrapbook textarea {
          border-radius: 12px;
        }
        @media (prefers-reduced-motion: reduce) {
          .scrapbook * { animation: none !important; transition: none !important; }
        }
      `}</style>
      {/* 手账封面条（清新插画风：纸胶带 + 印章标题） */}
      <header
        data-tauri-drag-region
        className="cfg-cover"
        style={{
          position: 'relative',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          margin: '10px 14px 0',
          padding: '10px 18px 9px',
          border: '1px solid var(--panel-border-light)',
          borderRadius: 16,
          background: 'var(--panel-surface)',
          boxShadow: 'var(--panel-shadow-card)',
          flexShrink: 0,
          userSelect: 'none',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
          <span
            style={{
              width: 10,
              height: 10,
              borderRadius: 999,
              background: 'var(--panel-accent)',
              boxShadow: '0 0 0 4px var(--panel-accent-soft)',
            }}
          />
          <div style={{ fontSize: 15, fontWeight: 700, letterSpacing: 0.5 }}>{t('config.title')}</div>
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <button
            onClick={() => setSetupGuideOpen(true)}
            title={t('config.setup_guide.guide_btn')}
            style={{
              width: 26,
              height: 26,
              border: '1px solid var(--panel-border)',
              background: 'transparent',
              color: 'var(--panel-text-tertiary)',
              cursor: 'pointer',
              borderRadius: 999,
              fontSize: 12,
              fontWeight: 600,
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              transition: 'background 0.15s ease, color 0.15s ease, border-color 0.15s ease',
              padding: 0,
              lineHeight: 1,
            }}
            onMouseEnter={(e) => {
              e.currentTarget.style.background = 'var(--panel-selected-bg)';
              e.currentTarget.style.color = 'var(--panel-selected-text)';
              e.currentTarget.style.borderColor = 'var(--panel-selected-bg)';
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.background = 'transparent';
              e.currentTarget.style.color = 'var(--panel-text-tertiary)';
              e.currentTarget.style.borderColor = 'var(--panel-border)';
            }}
          >
            ?
          </button>
          <button
            onClick={closeWindow}
            title={t('common.close')}
            style={{
              width: 26,
              height: 26,
              border: 'none',
              background: 'transparent',
              color: 'var(--panel-text-secondary)',
              cursor: 'pointer',
              borderRadius: 999,
              fontSize: 13,
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              transition: 'background 0.15s ease, color 0.15s ease',
            }}
            onMouseEnter={(e) => {
              e.currentTarget.style.background = 'var(--panel-danger)';
              e.currentTarget.style.color = 'var(--panel-selected-text)';
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.background = 'transparent';
              e.currentTarget.style.color = 'var(--panel-text-secondary)';
            }}
          >
            ✕
          </button>
        </div>
      </header>

      <div style={{ display: 'flex', flex: 1, overflow: 'hidden', marginTop: 10 }}>
        {/* 左侧贴纸导航 */}
        <div
          className="no-scrollbar"
          style={{
            width: 150,
            background: 'transparent',
            borderRight: '2px dashed var(--panel-border-light)',
            padding: '14px 10px 20px 14px',
            overflowY: 'auto',
            flexShrink: 0,
          }}
        >
          {tabs.map((tab, idx) => {
            const isActive = activeTab === tab.key;
            const Icon = tab.icon;
            const stickerColors = [
              'var(--sticker-pink-soft)',
              'var(--sticker-lilac-soft)',
              'var(--sticker-sky-soft)',
              'var(--sticker-mint-soft)',
              'var(--sticker-butter-soft)',
            ];
            const stickerColor = stickerColors[idx % stickerColors.length];
            return (
              <button
                key={tab.key}
                onClick={() => handleTabChange(tab.key)}
                onMouseEnter={(e) => {
                  if (!isActive) {
                    const btn = e.currentTarget;
                    btn.style.background = 'var(--panel-bg-hover)';
                    btn.style.transform = 'translateY(-2px) rotate(-1deg)';
                    btn.style.boxShadow = 'var(--panel-shadow-card)';
                  }
                }}
                onMouseLeave={(e) => {
                  if (!isActive) {
                    const btn = e.currentTarget;
                    btn.style.background = 'transparent';
                    btn.style.transform = 'translateY(0) rotate(0)';
                    btn.style.boxShadow = 'none';
                  }
                }}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 9,
                  width: '100%',
                  textAlign: 'left',
                  padding: '10px 12px',
                  border: isActive ? '1.5px solid var(--panel-accent)' : '1.5px solid transparent',
                  background: isActive ? stickerColor : 'transparent',
                  color: 'var(--panel-text)',
                  fontSize: 13,
                  borderRadius: 14,
                  cursor: 'pointer',
                  marginBottom: 6,
                  fontFamily: 'inherit',
                  transition: 'transform 0.18s cubic-bezier(0.2,0.8,0.2,1), box-shadow 0.18s ease, border-color 0.18s ease, background 0.18s ease, color 0.15s ease',
                  fontWeight: isActive ? 700 : 400,
                  transform: isActive ? 'translateY(-2px) rotate(-1deg)' : 'translateY(0) rotate(0)',
                  boxShadow: isActive ? 'var(--panel-shadow-card)' : 'none',
                }}
              >
                <Icon size={16} strokeWidth={isActive ? 2.2 : 1.8} style={{ flexShrink: 0, color: 'var(--panel-accent)' }} />
                {t(tab.labelKey)}
              </button>
            );
          })}
        </div>

        <div
          style={{
            flex: 1,
            overflowY: 'auto',
            padding: '20px 26px',
            animation: 'cfg-fade 0.3s cubic-bezier(0.22,0.9,0.28,1)',
          }}
        >
          {tabContent}
        </div>
      </div>

      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'flex-end',
          gap: 10,
          padding: '12px 18px',
          margin: '0 14px 12px',
          border: '1px solid var(--panel-border-light)',
          borderRadius: 16,
          flexShrink: 0,
          background: 'var(--panel-surface)',
          boxShadow: 'var(--panel-shadow-card)',
        }}
      >
        {saveError && (
          <span
            style={{
              flex: 1,
              fontSize: 12,
              color: 'var(--panel-danger)',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
            title={saveError}
          >
            {saveError}
          </span>
        )}
        <button
          onClick={handleReset}
          style={{
            padding: '9px 18px',
            minWidth: 120,
            border: '1.5px solid var(--panel-border)',
            background: 'var(--panel-surface)',
            color: 'var(--panel-text)',
            borderRadius: 12,
            fontSize: 13,
            cursor: 'pointer',
            fontFamily: 'inherit',
            boxShadow: 'var(--panel-shadow-subtle)',
          }}
        >
          {t('common.reset')}
        </button>
        <button
          onClick={handleSave}
          disabled={saving}
          style={{
            padding: '9px 18px',
            minWidth: 120,
            border: 'none',
            background: savedFlash ? 'var(--panel-success)' : 'var(--panel-accent)',
            color: 'var(--panel-selected-text)',
            borderRadius: 12,
            fontSize: 13,
            fontWeight: 600,
            cursor: saving ? 'not-allowed' : 'pointer',
            opacity: saving ? 0.7 : 1,
            fontFamily: 'inherit',
            transition: 'background 0.2s ease',
            boxShadow: 'var(--panel-shadow-subtle)',
          }}
        >
          {saving ? t('common.saving') : savedFlash ? t('common.saved') : t('common.save')}
        </button>
      </div>

      {/* 保存中遮罩：阻止保存期间修改表单 */}
      {saving && (
        <div
          style={{
            position: 'fixed',
            inset: 0,
            zIndex: 9999,
            background: 'rgba(0, 0, 0, 0.18)',
            backdropFilter: 'blur(1.5px)',
            WebkitBackdropFilter: 'blur(1.5px)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            cursor: 'wait',
          }}
        >
          <div
            style={{
              display: 'inline-flex',
              alignItems: 'center',
              gap: 8,
              fontSize: 13,
              fontWeight: 600,
              color: 'var(--panel-text)',
              background: 'var(--panel-surface)',
              border: '1px solid var(--panel-border)',
              borderRadius: 12,
              padding: '10px 20px',
              boxShadow: 'var(--panel-shadow-card)',
            }}
          >
            <span
              style={{
                width: 12,
                height: 12,
                borderRadius: 999,
                border: '2px solid var(--panel-border)',
                borderTopColor: 'var(--panel-accent)',
                animation: 'gptsovits-spin 0.8s linear infinite',
              }}
            />
            {t('common.saving')}
          </div>
        </div>
      )}

      {/* TTS 后端使用说明书抽屉 */}
      <TtsHelpDrawer
        open={ttsHelpOpen}
        initialBackend={ttsHelpBackend}
        onClose={() => setTtsHelpOpen(false)}
      />

      {/* ASR 后端使用说明书抽屉 */}
      <AsrHelpDrawer
        open={asrHelpOpen}
        initialBackend={asrHelpBackend}
        onClose={() => setAsrHelpOpen(false)}
      />

      {/* 清空记忆确认弹窗 */}
      <ClearConfirmDialog
        open={clearMemoriesOpen}
        loading={clearingMemories}
        onConfirm={handleClearMemoriesConfirm}
        onCancel={() => setClearMemoriesOpen(false)}
      />

      {/* 恢复备份确认弹窗 */}
      <ClearConfirmDialog
        open={restoreConfirmOpen}
        loading={restoring}
        title={t('config.restore_confirm_title')}
        message={`${t('config.restore_confirm_message')}\n${restoreSource ?? ''}`}
        confirmLabel={t('config.restore_btn_confirm')}
        loadingLabel={t('config.restore_btn_loading')}
        onConfirm={handleRestoreConfirm}
        onCancel={() => setRestoreConfirmOpen(false)}
      />

      {/* 配置引导弹窗 */}
      <SetupGuideModal
        open={setupGuideOpen}
        onClose={() => setSetupGuideOpen(false)}
        onGoLlm={() => {
          setActiveTab('ai');
          setSetupGuideOpen(false);
        }}
        onGoMemory={() => {
          setActiveTab('memory');
          setSetupGuideOpen(false);
        }}
        onGoVoice={() => {
          setActiveTab('voice');
          setSetupGuideOpen(false);
        }}
        onGoSearch={() => {
          setActiveTab('network');
          setSetupGuideOpen(false);
        }}
        status={{
          llm: !!get('ai.api_key', ''),
          memory:
            get<boolean>('memory.embedding.enabled', false) === true &&
            (get<string>('memory.embedding.source', 'cloud') === 'local' ||
              !!get('memory.embedding.api_key', '')),
          search: (get<string[]>('web_search.providers', ['duckduckgo']) ?? []).length > 0,
          voice: ttsConfig !== null && ttsConfig.engine !== 'none',
          routing: get<boolean>('enable_routing_matrix', false) === true,
        }}
      />
    </div>
  );
};

export default ConfigWindow;
