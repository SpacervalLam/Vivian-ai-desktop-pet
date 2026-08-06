import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import ReactDOM from 'react-dom';
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { platform, version as osVersion, arch } from '@tauri-apps/plugin-os';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { emit, listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';
import { changeLanguage } from '../i18n';
import { getCharacterId } from '../characterContext';
import TtsHelpDrawer, { TtsBackendKey } from './TtsHelpDrawer';
import AsrHelpDrawer, { AsrBackendKey } from './AsrHelpDrawer';
import ShortcutRecorder, { type ConflictResult, formatForDisplay } from './ShortcutRecorder';
import ClearConfirmDialog from './ClearConfirmDialog';
import SetupGuideModal from './SetupGuideModal';
import type { FishSpeechServiceState, GptSoVitsServiceState, GptSoVitsServiceStatus, OllamaServiceState, WhisperServiceState } from '../types';

type TabKey =
  | 'general'
  | 'ai'
  | 'tools'
  | 'memory'
  | 'voice'
  | 'world'
  | 'network'
  | 'about';

interface TtsConfigState {
  enabled: boolean;
  rate: number;
  volume: number;
  voice_id: string | null;
  engine: 'none' | 'edgetts' | 'azure' | 'gptsovits' | 'fishspeech' | 'bertvits2' | 'minimax' | 'doubao';
  fallback_engine: 'none' | 'edgetts' | 'azure' | 'gptsovits' | 'fishspeech' | 'bertvits2' | 'minimax' | 'doubao' | null;
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

const tabs: { key: TabKey; labelKey: string }[] = [
  { key: 'general', labelKey: 'config.tab_general' },
  { key: 'ai', labelKey: 'config.tab_ai' },
  { key: 'tools', labelKey: 'config.tab_tools' },
  { key: 'memory', labelKey: 'config.tab_memory' },
  { key: 'voice', labelKey: 'config.tab_voice' },
  { key: 'world', labelKey: 'config.tab_world' },
  { key: 'network', labelKey: 'config.tab_network' },
  { key: 'about', labelKey: 'config.tab_about' },
];

type ConfigValue = string | number | boolean | ConfigObject | string[];
interface ConfigObject {
  [key: string]: ConfigValue;
}

/**
 * 路由矩阵任务定义 - 12 个真实启用的任务，每个任务独立配置完整模型
 *
 * 任务职责说明：
 * - chat:                日常对话与问答（高频，可用便宜模型）
 * - reasoning:           长输入/工具调用深度推理（低频，需强模型）
 * - vision_describe:     图片理解（用户发图时使用，必须配置支持视觉的多模态模型）
 * - diary:               智能日记内容生成
 * - memory:              写入时记忆抽取（enrich：关键词/重要性/语义类型分类，高频，建议便宜模型）
 * - consolidation:       离线记忆巩固（三阶段流水线：短期→长期摘要、画像抽取、洞察生成，低频，需深度推理模型）
 * - inner_monologue:     离线内心独白（用户不交互时自主思考，30分钟一次，建议廉价快速模型）
 * - activity_extraction: 活动类型提取（每次对话后台调用，高频，建议便宜模型）
 * - emotion_analysis:    情绪分类分析（每轮对话后台调用，高频，建议便宜模型）
 * - knowledge_acquisition: 空闲时知识搜索学习（后台低频，建议便宜模型）
 * - interest_search:     内心独白中联网搜索兴趣话题（后台低频，建议便宜模型）
 */
const ROUTING_TASKS: { labelKey: string; taskType: string; helpKey: string }[] = [
  { labelKey: 'config.routing_chat', taskType: 'chat', helpKey: 'config.routing_chat_help' },
  { labelKey: 'config.routing_reasoning', taskType: 'reasoning', helpKey: 'config.routing_reasoning_help' },
  { labelKey: 'config.routing_vision_describe', taskType: 'vision_describe', helpKey: 'config.routing_vision_describe_help' },
  { labelKey: 'config.routing_diary', taskType: 'diary', helpKey: 'config.routing_diary_help' },
  { labelKey: 'config.routing_memory', taskType: 'memory', helpKey: 'config.routing_memory_help' },
  { labelKey: 'config.routing_consolidation', taskType: 'consolidation', helpKey: 'config.routing_consolidation_help' },
  { labelKey: 'config.routing_inner_monologue', taskType: 'inner_monologue', helpKey: 'config.routing_inner_monologue_help' },
  { labelKey: 'config.routing_activity_extraction', taskType: 'activity_extraction', helpKey: 'config.routing_activity_extraction_help' },
  { labelKey: 'config.routing_emotion_analysis', taskType: 'emotion_analysis', helpKey: 'config.routing_emotion_analysis_help' },
  { labelKey: 'config.routing_knowledge_acquisition', taskType: 'knowledge_acquisition', helpKey: 'config.routing_knowledge_acquisition_help' },
  { labelKey: 'config.routing_interest_search', taskType: 'interest_search', helpKey: 'config.routing_interest_search_help' },
  { labelKey: 'config.routing_translation', taskType: 'translation', helpKey: 'config.routing_translation_help' },
];

/**
 * 服务商预设 - 选中后自动填充 provider_type / endpoint / 默认 model
 *
 * 数据来源：2026-07 各服务商官方 API 文档实测
 * - OpenAI: https://api.openai.com/v1
 * - Anthropic: https://api.anthropic.com（原生 /v1/messages，非 OpenAI 兼容）
 * - Gemini: https://generativelanguage.googleapis.com（原生 REST）
 * - DeepSeek: https://api.deepseek.com/v1（OpenAI 兼容）
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
  /** 是否需要 api_secret（文心等 OAuth/HMAC 鉴权） */
  needsSecret?: boolean;
  /** 是否需要 app_id */
  needsAppId?: boolean;
}

const PROVIDER_PRESETS: ProviderPreset[] = [
  { id: 'openai', labelKey: 'config.preset_openai', providerType: 'openai', endpoint: 'https://api.openai.com/v1', defaultModel: 'gpt-5.5' },
  { id: 'anthropic', labelKey: 'config.preset_anthropic', providerType: 'anthropic', endpoint: 'https://api.anthropic.com', defaultModel: 'claude-sonnet-4' },
  { id: 'gemini', labelKey: 'config.preset_gemini', providerType: 'gemini', endpoint: 'https://generativelanguage.googleapis.com', defaultModel: 'gemini-3-pro' },
  { id: 'deepseek', labelKey: 'config.preset_deepseek', providerType: 'openai', endpoint: 'https://api.deepseek.com/v1', defaultModel: 'deepseek-chat' },
  { id: 'qwen', labelKey: 'config.preset_qwen', providerType: 'openai', endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1', defaultModel: 'qwen3-max' },
  { id: 'glm', labelKey: 'config.preset_glm', providerType: 'openai', endpoint: 'https://open.bigmodel.cn/api/paas/v4', defaultModel: 'glm-5' },
  { id: 'moonshot', labelKey: 'config.preset_moonshot', providerType: 'openai', endpoint: 'https://api.moonshot.cn/v1', defaultModel: 'kimi-k2.6' },
  { id: 'doubao', labelKey: 'config.preset_doubao', providerType: 'openai', endpoint: 'https://ark.cn-beijing.volces.com/api/v3', defaultModel: 'doubao-seed-1.6' },
  { id: 'siliconflow', labelKey: 'config.preset_siliconflow', providerType: 'openai', endpoint: 'https://api.siliconflow.cn/v1', defaultModel: 'deepseek-ai/DeepSeek-V3.1' },
  { id: 'grok', labelKey: 'config.preset_grok', providerType: 'openai', endpoint: 'https://api.x.ai/v1', defaultModel: 'grok-4.5' },
  { id: 'openrouter', labelKey: 'config.preset_openrouter', providerType: 'chat_completions', endpoint: 'https://openrouter.ai/api/v1', defaultModel: 'openai/gpt-4o' },
  { id: 'groq', labelKey: 'config.preset_groq', providerType: 'chat_completions', endpoint: 'https://api.groq.com/openai/v1', defaultModel: 'llama-3.3-70b-versatile' },
  { id: 'ollama', labelKey: 'config.preset_ollama', providerType: 'chat_completions', endpoint: 'http://localhost:11434/v1', defaultModel: 'llama3.2' },
  { id: 'mistral', labelKey: 'config.preset_mistral', providerType: 'chat_completions', endpoint: 'https://api.mistral.ai/v1', defaultModel: 'mistral-large-latest' },
  { id: 'together', labelKey: 'config.preset_together', providerType: 'chat_completions', endpoint: 'https://api.together.xyz/v1', defaultModel: 'meta-llama/Llama-3.3-70B-Instruct-Turbo' },
  { id: 'wenxin', labelKey: 'config.preset_wenxin', providerType: 'wenxin', endpoint: 'https://aip.baidubce.com', defaultModel: 'ernie-4.5-8k-latest', needsSecret: true },
  { id: 'custom', labelKey: 'config.preset_custom', providerType: 'chat_completions', endpoint: '', defaultModel: '' },
];

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

/**
 * 服务商选择器 —— 合并了"快速预设"和"服务商类型"两个下拉
 *
 * 选项 = 11 个服务商预设（仅显示服务商名）+ "自定义"。
 * 选中预设 → 自动填充 provider_type / endpoint / model；
 * 选"自定义" → 不填充，用户手动填写下方字段。
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

  const currentType = get(providerTypePath, 'openai') as string;
  const currentEndpoint = get(endpointPath, '') as string;

  // 匹配当前配置对应的预设（用于回显当前选中项）
  // 优先按 provider_type + endpoint 双重匹配；endpoint 为空时回退到 provider_type 匹配
  const matchingPreset = PROVIDER_PRESETS.find(
    (p) =>
      p.providerType === currentType &&
      (currentEndpoint === '' || p.endpoint === currentEndpoint),
  );
  const currentValue = matchingPreset?.labelKey ?? '__custom__';
  const currentPresetId = matchingPreset?.id ?? 'custom';

  return (
    <SelectField
      label={t('config.field_provider')}
      value={currentValue}
      onChange={(v) => {
        const preset = PROVIDER_PRESETS.find((p) => p.labelKey === v);
        if (!preset) return;

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

        // ② 切换预设：覆盖 provider_type / endpoint / model
        setNested(providerTypePath, preset.providerType);
        setNested(endpointPath, preset.endpoint);
        if (preset.defaultModel) {
          setNested(modelPath, preset.defaultModel);
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
      }}
      options={PROVIDER_PRESETS.map((p) => ({
        value: p.labelKey,
        label: t(p.labelKey),
      }))}
    />
  );
};

const fieldStyle: React.CSSProperties = {
  marginBottom: 18,
};
const labelStyle: React.CSSProperties = {
  display: 'block',
  fontSize: 12,
  color: 'var(--panel-text-secondary)',
  marginBottom: 6,
};
const inputStyle: React.CSSProperties = {
  width: '100%',
  padding: '8px 10px',
  border: '1.5px solid var(--panel-border)',
  borderRadius: 8,
  background: 'var(--panel-surface)',
  color: 'var(--panel-text)',
  fontSize: 13,
  fontFamily: 'inherit',
  outline: 'none',
  boxSizing: 'border-box',
  boxShadow: 'var(--panel-shadow-subtle)',
};
const selectStyle: React.CSSProperties = {
  ...inputStyle,
  appearance: 'none',
  cursor: 'pointer',
  paddingRight: 30,
};
const sectionTitleStyle: React.CSSProperties = {
  fontSize: 13,
  fontWeight: 700,
  color: 'var(--panel-text)',
  marginBottom: 14,
  paddingBottom: 8,
  borderBottom: '1.5px solid var(--panel-border)',
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
}> = ({ label, value, onChange, placeholder, type = 'text', disabled = false }) => (
  <div style={fieldStyle}>
    <label style={{ ...labelStyle, ...(disabled ? { opacity: 0.5 } : {}) }}>{label}</label>
    <input
      type={type}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      disabled={disabled}
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
          borderRadius: 8,
          background: 'var(--panel-surface)',
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
            <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>{subtitle}</span>
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
  const [clearMemoriesOpen, setClearMemoriesOpen] = useState(false);
  const [clearingMemories, setClearingMemories] = useState(false);

  // 路由矩阵：每个任务最近一次请求状态（'ok' 绿色 / 'error' 红色）
  // 由后端 chat:route_status 事件驱动，仅在路由矩阵开启时有意义
  const [routeStatus, setRouteStatus] = useState<Record<string, 'ok' | 'error'>>({});

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
      // 启动：先保存当前编辑中的 service_* 配置，确保后端读到最新值
      setWhisperServiceBusy(true);
      try {
        await handleSave();
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
    void loadConfig();
    // 首次打开设置窗口时自动弹出配置指引
    if (!localStorage.getItem('vivian-setup-guide-seen')) {
      localStorage.setItem('vivian-setup-guide-seen', '1');
      setSetupGuideOpen(true);
    }
    // 主 LLM 未配置时作为兜底：即使已看过指引仍弹出
    void invoke<boolean>('is_main_api_configured')
      .then((ok) => {
        if (!ok) setSetupGuideOpen(true);
      })
      .catch(() => {});
    // 加载角色列表，初始化语音页签的编辑目标为窗口所属角色
    void (async () => {
      try {
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
      }
    })();
    void loadDiaryConfig();
    void getVersion().then(setAppVersion).catch(() => { /* 忽略 */ });
    void Promise.all([platform(), osVersion(), arch()])
      .then(([p, v, a]) => setOsInfo(`${p} ${v} (${a})`))
      .catch(() => { /* 忽略 */ });
  }, []);

  // 切换到工具页签时加载 MCP server 列表
  useEffect(() => {
    if (activeTab === 'tools') {
      invoke<Array<{ id: string; name: string; enabled: boolean; tool_count: number; alive: boolean }>>('list_mcp_servers')
        .then(setMcpServers)
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
      // 4. 重启整个应用，重启后行为自然恢复
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
            <NumberField
              label={t('config.field_blink_interval')}
              value={get('live2d_render.blink_interval', 4000)}
              onChange={(v) => setNested('live2d_render.blink_interval', v)}
              min={100}
              step={100}
            />
            <ToggleField
              label={t('config.field_smart_positioning')}
              help={t('config.smart_positioning_help')}
              value={get('window.smart_positioning_enabled', true)}
              onChange={(v) => setNested('window.smart_positioning_enabled', v)}
            />
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

            {/* ── 主动对话（从独立页签合并）── */}
            <div style={{ ...sectionTitleStyle, marginTop: 24 }}>
              {t('config.section_proactive')}
            </div>
            <ToggleField
              label={t('config.field_enable_proactive')}
              value={get('proactive.enabled', true)}
              onChange={(v) => setNested('proactive.enabled', v)}
            />
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

            <CollapsibleSection
              title={t('config.section_danger')}
              tone="danger"
              defaultOpen={false}
            >
              <button
                onClick={() => setClearMemoriesOpen(true)}
                style={{
                  width: '100%',
                  padding: '12px 14px',
                  borderRadius: 12,
                  background: 'rgba(255, 69, 58, 0.12)',
                  border: '1px solid rgba(255, 69, 58, 0.3)',
                  color: '#E53935',
                  fontSize: 14,
                  fontWeight: 600,
                  cursor: 'pointer',
                  transition: 'background 0.2s',
                }}
                onMouseEnter={(e) => { e.currentTarget.style.background = 'rgba(255, 69, 58, 0.2)'; }}
                onMouseLeave={(e) => { e.currentTarget.style.background = 'rgba(255, 69, 58, 0.12)'; }}
              >
                {t('config.clear_memories_btn')}
              </button>
            </CollapsibleSection>
          </>
        );
      case 'ai':
        return (
          <>
            <div style={sectionTitleStyle}>{t('config.section_ai')}</div>
            <ProviderSelector pathPrefix="ai" get={get} setNested={setNested} t={t} />
            <TextField
              label={t('config.field_model_name')}
              value={get('ai.model', 'gpt-5.5')}
              onChange={(v) => setNested('ai.model', v)}
              placeholder={t('config.ph_model')}
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
              value={get('ai.temperature', 0.7)}
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
                  isComplete && modelVal ? (
                    <span
                      style={{
                        color: routeStatus[task.taskType] === 'error' ? '#F44336' : '#4CAF50',
                        fontSize: 12,
                        fontWeight: 500,
                        display: 'inline-block',
                        maxWidth: '100%',
                      }}
                      title={modelVal}
                    >
                      {modelVal}
                    </span>
                  ) : undefined
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
              </CollapsibleSection>
              );
            })}
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

            <div style={{ ...sectionTitleStyle, marginTop: 28 }}>{t('config.section_tools_execution')}</div>
            <NumberField
              label={t('config.field_tool_max_iterations')}
              value={get('tools.max_iterations', 10)}
              onChange={(v) => setNested('tools.max_iterations', v)}
              min={1}
              max={50}
              step={1}
            />
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
              {t('config.field_tool_max_iterations_help')}
            </div>
            <NumberField
              label={t('config.field_tool_max_rounds')}
              value={get('tools.max_rounds', 4)}
              onChange={(v) => setNested('tools.max_rounds', v)}
              min={1}
              max={10}
              step={1}
            />
            <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: -10, marginBottom: 14, lineHeight: 1.5 }}>
              {t('config.field_tool_max_rounds_help')}
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
              value={get('memory.retrieval_weights.recency', 0.3)}
              onChange={(v) => setNested('memory.retrieval_weights.recency', v)}
              min={0}
              max={1}
              step={0.05}
              format={(v) => v.toFixed(2)}
            />
            <SliderField
              label={t('config.field_weight_relevance')}
              help={t('config.field_weight_relevance_help')}
              value={get('memory.retrieval_weights.relevance', 0.5)}
              onChange={(v) => setNested('memory.retrieval_weights.relevance', v)}
              min={0}
              max={1}
              step={0.05}
              format={(v) => v.toFixed(2)}
            />
            <SliderField
              label={t('config.field_weight_importance')}
              help={t('config.field_weight_importance_help')}
              value={get('memory.retrieval_weights.importance', 0.2)}
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
                  onChange={(v) => setNested('memory.embedding.model', v)}
                  placeholder={t('config.ph_embedding_model')}
                />
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
                  value={get('memory.embedding.ollama_path', '')}
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
                  value={get('memory.embedding.ollama_auto_start', false)}
                  onChange={(v) => setNested('memory.embedding.ollama_auto_start', v)}
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
                    disabled={whisperServiceBusy}
                    style={{
                      padding: '6px 14px',
                      border: 'none',
                      borderRadius: 6,
                      background:
                        whisperService?.status === 'running'
                          ? '#e74c3c'
                          : '#27ae60',
                      color: '#fff',
                      fontSize: 12,
                      cursor: whisperServiceBusy ? 'not-allowed' : 'pointer',
                      fontFamily: 'inherit',
                      opacity: whisperServiceBusy ? 0.6 : 1,
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: 6,
                    }}
                  >
                    {whisperServiceBusy && (
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
                            : 'var(--panel-bg-surface-elevated)',
                      color:
                        whisperService?.status === 'running'
                          ? '#27ae60'
                          : whisperService?.status === 'crashed'
                            ? '#e74c3c'
                            : 'var(--panel-text-secondary)',
                    }}
                  >
                    {(() => {
                      const s = whisperService?.status ?? 'stopped';
                      switch (s) {
                        case 'running':
                          return t('config.whisper_status_running');
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
                  onChange={(v) => setTtsConfig({ ...ttsConfig, tts_language: v || null })}
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

            {/* ── 网络搜索（已并入网络页签，多引擎混用）── */}
            <div style={{ ...sectionTitleStyle, marginTop: 24 }}>
              {t('config.section_web_search')}
            </div>

            {/* 多引擎混选：用户可同时启用多个引擎 */}
            <MultiCheckboxField
              label={t('config.field_web_search_providers')}
              help={t('config.help_web_search_providers')}
              values={get<string[]>('web_search.providers', ['duckduckgo'])}
              onChange={(next) => setNested('web_search.providers', next)}
              options={[
                { value: 'duckduckgo', label: t('config.opt_web_search_duckduckgo') },
                { value: 'searxng', label: t('config.opt_web_search_searxng') },
                { value: 'tavily', label: t('config.opt_web_search_tavily') },
              ]}
              minSelected={1}
            />
            <NumberField
              label={t('config.field_web_search_max_results')}
              value={get('web_search.max_results', 5)}
              onChange={(v) => setNested('web_search.max_results', v)}
              min={1}
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
          </>
        );
      case 'world':
        return (
          <>
            <div style={sectionTitleStyle}>{t('config.section_world')}</div>
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
            <div style={sectionTitleStyle}>{t('config.section_world_weather')}</div>
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
            <div style={sectionTitleStyle}>{t('config.section_world_monologue')}</div>
            <ToggleField
              label={t('config.field_world_monologue')}
              value={get('world.enable_inner_monologue', true)}
              onChange={(v) => setNested('world.enable_inner_monologue', v)}
            />
            <div style={sectionTitleStyle}>{t('config.section_world_consolidation')}</div>
            <ToggleField
              label={t('config.field_world_consolidation')}
              help={t('config.world_consolidation_help')}
              value={get('world.enable_memory_consolidation', true)}
              onChange={(v) => setNested('world.enable_memory_consolidation', v)}
            />

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
          '-apple-system, BlinkMacSystemFont, "PingFang SC", "Microsoft YaHei", "Segoe UI", sans-serif',
        color: 'var(--panel-text)',
        overflow: 'hidden',
      }}
    >
      <style>{`@keyframes gptsovits-spin{to{transform:rotate(360deg)}}`}</style>
      <div
        data-tauri-drag-region
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          padding: '12px 16px',
          borderBottom: '1.5px solid var(--panel-border)',
          flexShrink: 0,
          userSelect: 'none',
          background: 'var(--panel-bg-surface-elevated)',
          backdropFilter: 'blur(12px)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <div style={{ fontSize: 15, fontWeight: 700 }}>{t('config.title')}</div>
          <button
            onClick={() => setSetupGuideOpen(true)}
            title={t('config.setup_guide.guide_btn')}
            style={{
              width: 22,
              height: 22,
              border: '1px solid var(--panel-border)',
              background: 'transparent',
              color: 'var(--panel-text-tertiary)',
              cursor: 'pointer',
              borderRadius: 11,
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
        </div>
        <button
          onClick={closeWindow}
          title={t('common.close')}
          style={{
            width: 28,
            height: 28,
            border: 'none',
            background: 'transparent',
            color: 'var(--panel-text-secondary)',
            cursor: 'pointer',
            borderRadius: 6,
            fontSize: 14,
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            transition: 'background 0.15s ease, color 0.15s ease',
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.background = 'var(--panel-selected-bg)';
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

      <div style={{ display: 'flex', flex: 1, overflow: 'hidden' }}>
        <div
          className="no-scrollbar"
          style={{
            width: 140,
            background: 'var(--panel-bg-surface)',
            borderRight: '1.5px solid var(--panel-border)',
            padding: '8px 6px',
            overflowY: 'auto',
            flexShrink: 0,
          }}
        >
          {tabs.map((tab) => {
            const isActive = activeTab === tab.key;
            return (
              <button
                key={tab.key}
                onClick={() => handleTabChange(tab.key)}
                onMouseEnter={(e) => {
                  if (!isActive) {
                    const btn = e.currentTarget;
                    btn.style.borderColor = 'var(--panel-border-strong)';
                    btn.style.transform = 'translateY(-2px) rotate(-0.5deg)';
                    btn.style.boxShadow = 'var(--panel-shadow-card)';
                  }
                }}
                onMouseLeave={(e) => {
                  if (!isActive) {
                    const btn = e.currentTarget;
                    btn.style.borderColor = 'transparent';
                    btn.style.transform = 'translateY(0) rotate(0)';
                    btn.style.boxShadow = 'none';
                  }
                }}
                style={{
                  display: 'block',
                  width: '100%',
                  textAlign: 'left',
                  padding: '9px 12px',
                  border: isActive ? '1.5px solid var(--panel-border-strong)' : '1.5px solid transparent',
                  background: 'transparent',
                  color: 'var(--panel-text)',
                  fontSize: 13,
                  borderRadius: 8,
                  cursor: 'pointer',
                  marginBottom: 4,
                  fontFamily: 'inherit',
                  transition: 'transform 0.18s cubic-bezier(0.2,0.8,0.2,1), box-shadow 0.18s ease, border-color 0.18s ease, color 0.15s ease',
                  fontWeight: isActive ? 700 : 400,
                  transform: isActive ? 'translateY(-2px) rotate(-0.5deg)' : 'translateY(0) rotate(0)',
                  boxShadow: isActive ? 'var(--panel-shadow-card)' : 'none',
                }}
              >
                {t(tab.labelKey)}
              </button>
            );
          })}
        </div>

        <div
          style={{
            flex: 1,
            overflowY: 'auto',
            padding: '20px 24px',
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
          padding: '12px 16px',
          borderTop: '1.5px solid var(--panel-border)',
          flexShrink: 0,
          background: 'var(--panel-bg-surface-elevated)',
          backdropFilter: 'blur(12px)',
        }}
      >
        {saveError && (
          <span
            style={{
              flex: 1,
              fontSize: 12,
              color: '#E53935',
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
            padding: '8px 18px',
            border: '1.5px solid var(--panel-border)',
            background: 'var(--panel-surface)',
            color: 'var(--panel-text)',
            borderRadius: 8,
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
            padding: '8px 22px',
            border: 'none',
            background: savedFlash ? '#4CAF50' : 'var(--panel-accent)',
            color: 'var(--panel-selected-text)',
            borderRadius: 8,
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
      />
    </div>
  );
};

export default ConfigWindow;
