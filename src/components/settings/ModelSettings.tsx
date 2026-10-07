import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open as openShell } from '@tauri-apps/plugin-shell';
import { ExternalLink } from 'lucide-react';
import { createAsyncCache } from './asyncCache';
import taskCatalog from '../../../src-tauri/prompts/routing/task_catalog.json';
import { fieldStyle, labelStyle, inputStyle, selectStyle } from './SettingsFields';
export type ConfigValue = string | number | boolean | ConfigObject | string[] | null;
export interface ConfigObject {
  [key: string]: ConfigValue;
}

/** Shared with Rust defaults, routing semantics, and task-specific prompt contracts. */
export const ROUTING_TASKS = taskCatalog.map(({ groupKey, labelKey, id, helpKey }) => ({
  groupKey, labelKey, taskType: id, helpKey,
}));

/** 厂商下的一个嵌入模型（插件 llm-providers 的 embedding-providers.json） */
interface EmbeddingProviderModelPreset {
  model: string;
  /** 默认输出维度，选中模型时自动填入 */
  dimension: number;
  /** 单请求 input 数组条数上限（适配器据此分块） */
  maxBatch?: number;
  /** 可切换的维度取值，仅作提示 */
  dimensions?: number[];
  note?: string;
}

/** 云端嵌入厂商预设（厂商级：选厂商 → 自动填充端点，模型仍由用户选定） */
export interface EmbeddingProviderPreset {
  id: string;
  provider: string;
  endpoint: string;
  models: EmbeddingProviderModelPreset[];
  region?: string;
  consoleUrl?: string;
  /** 请求体里下发维度的参数名；缺省表示不下发（服务端维度固定） */
  dimensionParam?: string;
  /** 本地服务填任意占位 Key 即可 */
  needsApiKey?: boolean;
  recommendedFor?: string;
  verifiedAt?: string;
  verifiedSource?: string;
}

/**
 * 端点归一化：仅用于「当前配置命中哪个厂商预设」的反查。
 * 用户可能把 `/embeddings` 后缀一起写进端点（适配器两种写法都接受），比较时要去掉，
 * 否则明明填的是智谱官方端点却显示成「自定义」。
 */
export const normalizeEmbeddingEndpoint = (v: string): string =>
  (v || '').trim().replace(/\/+$/, '').replace(/\/embeddings$/i, '');

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
 * - 文心一言: https://aip.baidubce.com（原生 OAuth + access_token）
 * - 腾讯混元: https://api.hunyuan.cloud.tencent.com/v1（OpenAI 兼容 Chat Completions）
 *
 * 注：讯飞星火因 WebSocket + HMAC 鉴权复杂且预设实用性低，未提供预设；
 *     用户仍可通过手动选择 provider=spark 进行配置。
 */
export interface ProviderPreset {
  /** 稳定标识，用作 provider_cache 的 key（不随 i18n 变化） */
  id: string;
  /** 服务商名 i18n key（内置预设用；插件预设可用 label 直接给名，二者至少其一） */
  labelKey?: string;
  /** 直接显示名（插件预设可用，无 i18n 键时使用；优先于 labelKey） */
  label?: string;
  providerType: string;
  endpoint: string;
  /** 选中预设时自动填充的默认模型（可缺省——如「自定义」卡片） */
  defaultModel?: string;
  /** 模型名输入建议列表（datalist 下拉建议，仍可自由输入） */
  mainModels?: string[];
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
  /** 协议显示名 i18n key（复用 config.proto_* 键） */
  labelKey?: string;
  /** 直接显示名（插件预设可用；优先于 labelKey） */
  label?: string;
  /** 该协议的接口端点 */
  endpoint: string;
}

/**
 * 预设是否包含指定的 (provider_type, endpoint) 组合 ——
 * 匹配默认协议或任一协议变体。endpoint 为空时仅按 provider_type 匹配。
 */
export const presetMatches = (p: ProviderPreset, type: string, endpoint: string): boolean =>
  (p.providerType === type && (endpoint === '' || p.endpoint === endpoint)) ||
  (p.protocols ?? []).some(
    (pr) => pr.providerType === type && (endpoint === '' || pr.endpoint === endpoint),
  );

/**
 * 厂商预设表。导出供 App.tsx 在 LLM 错误 toast 里反查：
 * 错误事件只带 endpoint，据此拿 consoleUrl 与显示名，给「余额不足」类
 * 错误挂上直达对应厂商控制台的动作。
 */
export const PROVIDER_PRESETS: ProviderPreset[] = [
  { id: 'openai', labelKey: 'config.preset_openai', providerType: 'openai', endpoint: 'https://api.openai.com/v1', defaultModel: 'gpt-5.5', mainModels: ['gpt-5.5', 'gpt-5.6', 'gpt-5', 'o3', 'o4-mini'], contextWindow: 400_000, suggestedMaxTokens: 32768, consoleUrl: 'https://platform.openai.com/api-keys', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.openai.com/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.openai.com/v1' },
    { providerType: 'openai_agents', labelKey: 'config.proto_agents', endpoint: 'https://api.openai.com/v1' },
  ] },
  { id: 'anthropic', labelKey: 'config.preset_anthropic', providerType: 'anthropic', endpoint: 'https://api.anthropic.com', defaultModel: 'claude-sonnet-4-6', mainModels: ['claude-sonnet-4-6', 'claude-sonnet-5', 'claude-opus-4-8', 'claude-haiku-4-5'], contextWindow: 1_000_000, suggestedMaxTokens: 64000, consoleUrl: 'https://console.anthropic.com/settings/keys' },
  { id: 'gemini', labelKey: 'config.preset_gemini', providerType: 'gemini', endpoint: 'https://generativelanguage.googleapis.com', defaultModel: 'gemini-3.1-pro-preview', mainModels: ['gemini-3.1-pro-preview', 'gemini-3-flash-preview', 'gemini-3.1-flash-lite'], contextWindow: 1_000_000, suggestedMaxTokens: 65536, consoleUrl: 'https://aistudio.google.com/apikey' },
  { id: 'deepseek', labelKey: 'config.preset_deepseek', providerType: 'openai', endpoint: 'https://api.deepseek.com', defaultModel: 'deepseek-v4-flash', mainModels: ['deepseek-v4-pro', 'deepseek-v4-flash', 'deepseek-v4-flash-vision-exp'], contextWindow: 1_000_000, suggestedMaxTokens: 16384, consoleUrl: 'https://platform.deepseek.com/api_keys', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.deepseek.com' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.deepseek.com' },
    { providerType: 'anthropic', labelKey: 'config.proto_anthropic', endpoint: 'https://api.deepseek.com/anthropic' },
  ] },
  { id: 'qwen', labelKey: 'config.preset_qwen', providerType: 'openai', endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1', defaultModel: 'qwen3.8-max', mainModels: ['qwen3.8-max', 'qwen3.7-max', 'qwen3.7-plus', 'qwen3.7-flash', 'qwen3-max', 'qwen-plus', 'qwen-flash'], contextWindow: 256_000, suggestedMaxTokens: 32768, consoleUrl: 'https://bailian.console.aliyun.com/?apiKey=1', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://dashscope.aliyuncs.com/compatible-mode/v1' },
  ] },
  { id: 'glm', labelKey: 'config.preset_glm', providerType: 'zhipu', endpoint: 'https://open.bigmodel.cn/api/paas/v4', defaultModel: 'glm-5.3', mainModels: ['glm-5.3', 'glm-5.2', 'glm-5', 'glm-5.3-flash', 'glm-4.7'], contextWindow: 1_000_000, suggestedMaxTokens: 65536, consoleUrl: 'https://open.bigmodel.cn/apikey/platform', protocols: [
    { providerType: 'zhipu', labelKey: 'config.proto_zhipu', endpoint: 'https://open.bigmodel.cn/api/paas/v4' },
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://open.bigmodel.cn/api/paas/v4' },
    { providerType: 'anthropic', labelKey: 'config.proto_anthropic', endpoint: 'https://open.bigmodel.cn/api/anthropic' },
  ] },
  { id: 'moonshot', labelKey: 'config.preset_moonshot', providerType: 'openai', endpoint: 'https://api.moonshot.cn/v1', defaultModel: 'kimi-k2.6', mainModels: ['kimi-k3', 'kimi-k2.6', 'kimi-k2.5', 'kimi-k2-thinking'], contextWindow: 256_000, suggestedMaxTokens: 32768, consoleUrl: 'https://platform.moonshot.cn/console/api-keys', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.moonshot.cn/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.moonshot.cn/v1' },
  ] },
  { id: 'doubao', labelKey: 'config.preset_doubao', providerType: 'openai', endpoint: 'https://ark.cn-beijing.volces.com/api/v3', defaultModel: 'doubao-seed-2.1-pro', mainModels: ['doubao-seed-2.1-pro', 'doubao-seed-2.1-turbo', 'doubao-seed-evolving', 'doubao-seed-2.1-pro-260628'], contextWindow: 256_000, suggestedMaxTokens: 65536, consoleUrl: 'https://console.volcengine.com/ark', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://ark.cn-beijing.volces.com/api/v3' },
    { providerType: 'doubao', labelKey: 'config.proto_doubao_responses', endpoint: 'https://ark.cn-beijing.volces.com/api/v3' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://ark.cn-beijing.volces.com/api/v3' },
    { providerType: 'anthropic', labelKey: 'config.proto_anthropic', endpoint: 'https://ark.cn-beijing.volces.com/api/v3/anthropic' },
  ] },
  { id: 'minimax', labelKey: 'config.preset_minimax', providerType: 'openai', endpoint: 'https://api.minimaxi.com/v1', defaultModel: 'MiniMax-M3', mainModels: ['MiniMax-M3', 'MiniMax-M2.7', 'MiniMax-M2.5'], contextWindow: 1_000_000, suggestedMaxTokens: 16384, consoleUrl: 'https://platform.minimaxi.com/user-center/basic-information/interface-key', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.minimaxi.com/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.minimaxi.com/v1' },
    { providerType: 'anthropic', labelKey: 'config.proto_anthropic', endpoint: 'https://api.minimaxi.com/anthropic' },
  ] },
  { id: 'mimo', labelKey: 'config.preset_mimo', providerType: 'openai', endpoint: 'https://api.xiaomimimo.com/v1', defaultModel: 'mimo-v2.5-pro', mainModels: ['mimo-v2.5-pro', 'mimo-v2.5'], contextWindow: 1_000_000, suggestedMaxTokens: 16384, consoleUrl: 'https://www.xiaomimimo.com/', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.xiaomimimo.com/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.xiaomimimo.com/v1' },
    { providerType: 'anthropic', labelKey: 'config.proto_anthropic', endpoint: 'https://api.xiaomimimo.com/anthropic' },
  ] },
  { id: 'grok', labelKey: 'config.preset_grok', providerType: 'openai', endpoint: 'https://api.x.ai/v1', defaultModel: 'grok-4.5', mainModels: ['grok-4.5', 'grok-4.3', 'grok-4.1-fast'], contextWindow: 500_000, suggestedMaxTokens: 32768, consoleUrl: 'https://console.x.ai', protocols: [
    { providerType: 'openai', labelKey: 'config.proto_responses', endpoint: 'https://api.x.ai/v1' },
    { providerType: 'chat_completions', labelKey: 'config.proto_chat_completions', endpoint: 'https://api.x.ai/v1' },
  ] },
  { id: 'openrouter', labelKey: 'config.preset_openrouter', providerType: 'chat_completions', endpoint: 'https://openrouter.ai/api/v1', defaultModel: 'openai/gpt-4o', mainModels: ['openai/gpt-4o', 'anthropic/claude-sonnet-4', 'deepseek/deepseek-chat'], contextWindow: 131_072, suggestedMaxTokens: 8192, consoleUrl: 'https://openrouter.ai/settings/keys' },
  { id: 'groq', labelKey: 'config.preset_groq', providerType: 'chat_completions', endpoint: 'https://api.groq.com/openai/v1', defaultModel: 'llama-3.3-70b-versatile', mainModels: ['llama-3.3-70b-versatile', 'llama-3.1-8b-instant'], contextWindow: 131_072, suggestedMaxTokens: 8192, consoleUrl: 'https://console.groq.com/keys' },
  { id: 'ollama', labelKey: 'config.preset_ollama', providerType: 'chat_completions', endpoint: 'http://localhost:11434/v1', defaultModel: 'llama3.2', mainModels: ['llama3.2', 'qwen2.5', 'deepseek-r1'], contextWindow: 131_072, suggestedMaxTokens: 8192, consoleUrl: 'https://ollama.com' },
  { id: 'mistral', labelKey: 'config.preset_mistral', providerType: 'chat_completions', endpoint: 'https://api.mistral.ai/v1', defaultModel: 'mistral-large-latest', mainModels: ['mistral-large-latest', 'mistral-small-latest'], contextWindow: 256_000, suggestedMaxTokens: 16384, consoleUrl: 'https://console.mistral.ai/api-keys' },
  { id: 'together', labelKey: 'config.preset_together', providerType: 'chat_completions', endpoint: 'https://api.together.xyz/v1', defaultModel: 'meta-llama/Llama-3.3-70B-Instruct-Turbo', mainModels: ['meta-llama/Llama-3.3-70B-Instruct-Turbo', 'deepseek-ai/DeepSeek-V3'], contextWindow: 131_072, suggestedMaxTokens: 8192, consoleUrl: 'https://api.together.ai/settings/api-keys' },
  { id: 'wenxin', labelKey: 'config.preset_wenxin', providerType: 'wenxin', endpoint: 'https://aip.baidubce.com', defaultModel: 'ernie-4.5-8k-latest', mainModels: ['ernie-4.5-8k-latest', 'ernie-4.5-turbo-8k', 'ernie-4.0-8k-latest'], contextWindow: 8192, suggestedMaxTokens: 4096, needsSecret: true, consoleUrl: 'https://console.bce.baidu.com/iam/#/iam/apikey/list' },
  { id: 'hunyuan', labelKey: 'config.preset_hunyuan', providerType: 'chat_completions', endpoint: 'https://api.hunyuan.cloud.tencent.com/v1', defaultModel: 'hunyuan-turbos-latest', mainModels: ['hunyuan-turbos-latest', 'hunyuan-t1-latest', 'hunyuan-pro', 'hunyuan-standard', 'hunyuan-lite'], contextWindow: 32_000, suggestedMaxTokens: 16384, consoleUrl: 'https://console.cloud.tencent.com/tokenhub/apikey' },
  { id: 'jev', labelKey: 'config.preset_jev', providerType: 'jev', endpoint: 'https://api.typesafe.ai/v1/systemone', defaultModel: 'jev-latest', mainModels: ['jev-latest'], consoleUrl: 'https://console.typesafe.ai/' },
  { id: 'custom', labelKey: 'config.preset_custom', providerType: 'chat_completions', endpoint: '', defaultModel: '', mainModels: [] },
];

/**
 * 插件贡献的供应商预设行（对齐后端 plugins::ProviderPresetData，camelCase）。
 * 由 `list_provider_presets` 命令返回（来源：`plugins/<name>/providers.json`，
 * 内置 llm-providers 插件播种于 <用户数据目录>/plugins/llm-providers/）；
 * 后端对 None 字段 skip 序列化——未出现的键不参与合并覆盖。
 */
export interface ProviderPresetData {
  id: string;
  labelKey?: string;
  label?: string;
  providerType: string;
  endpoint: string;
  defaultModel?: string;
  mainModels?: string[];
  contextWindow?: number;
  suggestedMaxTokens?: number;
  needsSecret?: boolean;
  needsAppId?: boolean;
  consoleUrl?: string;
  protocols?: ProviderProtocol[];
  /** 上次逐字段核对官方 API 文档的日期（YYYY-MM-DD，核对技能写入；透传字段） */
  verifiedAt?: string;
  /** 本次核对依据的官方文档入口 URL（下次核对直接回访） */
  verifiedSource?: string;
}

/**
 * 合并内置预设与插件贡献的预设（llm-providers 插件为厂商卡片主数据源）：
 * - 插件按 id 浅合并覆盖内置同名项（仅覆盖出现的字段，凭据缓存按 id 索引不受影响）
 * - 插件新增的 id 追加在「自定义」卡片之前
 * - 插件数据为空（未安装/解析失败）时原样返回内置兜底
 */
const mergeProviderPresets = (builtin: ProviderPreset[], pluginRows: ProviderPresetData[]): ProviderPreset[] => {
  if (!pluginRows.length) return builtin;
  const byId = new Map<string, ProviderPreset>(builtin.map((p) => [p.id, p]));
  const appended: ProviderPreset[] = [];
  for (const row of pluginRows) {
    if (!row || !row.id) continue;
    const merged = { ...(byId.get(row.id) ?? {}), ...row } as ProviderPreset;
    if (byId.has(row.id)) {
      byId.set(row.id, merged);
    } else {
      appended.push(merged);
    }
  }
  const ordered = [...byId.values()];
  const customIdx = ordered.findIndex((p) => p.id === 'custom');
  if (customIdx >= 0) {
    const custom = ordered.splice(customIdx, 1)[0];
    return [...ordered, ...appended, custom];
  }
  return [...ordered, ...appended];
};

const presetsCache = createAsyncCache(async () => {
  try {
    const rows = await invoke<ProviderPresetData[]>('list_provider_presets');
    return mergeProviderPresets(PROVIDER_PRESETS, rows ?? []);
  } catch { return PROVIDER_PRESETS; }
});
export const invalidateProviderPresetsCache = () => presetsCache.invalidate();
export const useProviderPresets = (): ProviderPreset[] => {
  const [presets, setPresets] = useState<ProviderPreset[]>(PROVIDER_PRESETS);
  useEffect(() => {
    let alive = true;
    const refresh = () => { void presetsCache.get().then(value => { if (alive) setPresets(value); }); };
    const unsubscribe = presetsCache.subscribe(refresh);
    refresh();
    return () => { alive = false; unsubscribe(); };
  }, []);
  return presets;
};

/** 预设显示名：插件直给的 label 优先，其次 i18n key，兜底 id */
const presetLabel = (
  p: { label?: string; labelKey?: string; id: string },
  t: (key: string) => string,
): string => p.label || (p.labelKey ? t(p.labelKey) : p.id);

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
  grok: 'icons/providers/grok.svg',
  openrouter: 'icons/providers/openrouter.svg',
  groq: 'icons/providers/groq.svg',
  ollama: 'icons/providers/ollama.svg',
  mistral: 'icons/providers/mistral.svg',
  together: 'icons/providers/together.svg',
  wenxin: 'icons/providers/wenxin.svg',
  hunyuan: 'icons/providers/hunyuan.svg',
  custom: 'icons/providers/custom-endpoint.svg',
};

/** 当前 provider_type 是否需要 api_secret 字段 */
export const needsSecretFor = (providerType: string): boolean => {
  const t = providerType.toLowerCase();
  return t === 'wenxin' || t === 'spark';
};

/** 当前 provider_type 是否需要 app_id 字段 */
export const needsAppIdFor = (providerType: string): boolean => {
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
 * 隐藏原生横向滚动条，左右导航按钮独立占位：
 * 点击箭头或使用滚轮按卡片滚动，卡片宽度随可用空间调整。
 * 滚动吸附到卡片边界，避免遮挡与静止时的卡片裁切。
 */
const ProviderPresetRow: React.FC<{
  presets: ProviderPreset[];
  activeId: string;
  onSelect: (presetId: string) => void;
  t: (key: string) => string;
}> = ({ presets, activeId, onSelect, t }) => {
  const scrollRowRef = useRef<HTMLDivElement>(null);
  const [cardWidth, setCardWidth] = useState(84);
  const [scrollEdges, setScrollEdges] = useState({ left: false, right: false });
  const cardWidthRef = useRef(84);
  const activeIndex = presets.findIndex(p => p.id === activeId);

  useEffect(() => {
    const el = scrollRowRef.current;
    if (!el) return;
    const updateEdges = () => {
      const left = el.scrollLeft > 1, right = el.scrollWidth - el.clientWidth - el.scrollLeft > 1;
      setScrollEdges(previous => previous.left === left && previous.right === right ? previous : { left, right });
    };
    const measure = () => {
      const count = Math.max(1, Math.floor((el.clientWidth + 8) / 92));
      const width = Math.max(1, (el.clientWidth - (count - 1) * 8) / count);
      cardWidthRef.current = width;
      setCardWidth(width);
      updateEdges();
    };
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    el.addEventListener('scroll', updateEdges, { passive: true });
    let lastWheel = 0;
    const onWheel = (e: WheelEvent) => {
      const delta = e.deltaX || e.deltaY;
      if (!delta || (delta < 0 && el.scrollLeft <= 1) || (delta > 0 && el.scrollWidth - el.clientWidth - el.scrollLeft <= 1)) return;
      e.preventDefault();
      if (performance.now() - lastWheel < 120) return;
      lastWheel = performance.now();
      el.scrollBy({ left: Math.sign(delta) * (cardWidthRef.current + 8), behavior: 'smooth' });
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    measure();
    return () => { observer.disconnect(); el.removeEventListener('scroll', updateEdges); el.removeEventListener('wheel', onWheel); };
  }, [presets.length]);

  useEffect(() => {
    const el = scrollRowRef.current;
    const index = activeIndex;
    if (!el || index < 0) return;
    const left = index * (cardWidth + 8);
    if (left < el.scrollLeft || left + cardWidth > el.scrollLeft + el.clientWidth + 1) {
      el.scrollTo({ left, behavior: 'smooth' });
    }
    setScrollEdges({ left: el.scrollLeft > 1, right: el.scrollWidth - el.clientWidth - el.scrollLeft > 1 });
  }, [activeId, cardWidth, activeIndex]);

  const scroll = (direction: number) => scrollRowRef.current?.scrollBy({ left: direction * (cardWidth + 8), behavior: 'smooth' });
  return (
    <>
      {/* Navigation occupies separate columns so cards remain unobstructed. */}
      <div className="provider-preset-row">
        <button type="button" className="provider-zone provider-zone-left" aria-label={t('config.provider_previous')} disabled={!scrollEdges.left} onClick={() => scroll(-1)} />
        <div
          ref={scrollRowRef}
          className="provider-hscroll"
          style={{
            display: 'flex',
            gap: 8,
            overflowX: 'auto',
            overflowY: 'hidden',
            padding: '4px 0 8px',
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
                aria-pressed={active}
                onClick={() => onSelect(p.id)}
                title={p.endpoint || t('config.preset_custom')}
                style={{
                  flex: '0 0 auto',
                  display: 'flex',
                  flexDirection: 'column',
                  alignItems: 'center',
                  justifyContent: 'center',
                  gap: 5,
                  width: cardWidth,
                  boxSizing: 'border-box',
                  scrollSnapAlign: 'start',
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
                    {presetLabel(p, t).slice(0, 1)}
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
                  {presetLabel(p, t)}
                </span>
              </button>
            );
          })}
        </div>
        <button type="button" className="provider-zone provider-zone-right" aria-label={t('config.provider_next')} disabled={!scrollEdges.right} onClick={() => scroll(1)} />
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
export const ProviderSelector: React.FC<{
  pathPrefix: string;
  get: <T extends ConfigValue>(path: string, fallback: T) => T;
  setNested: (path: string, value: ConfigValue) => void;
  t: (key: string) => string;
}> = ({ pathPrefix, get, setNested, t }) => {
  const isMain = pathPrefix === 'ai';
  // 供应商预设：内置兜底 + llm-providers 插件贡献（providers.json）合并
  const allPresets = useProviderPresets();
  const presets = allPresets.filter((p) => pathPrefix === 'routing_matrix.simple_judge' || p.providerType !== 'jev');
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
  const matchingPreset = presets.find((p) => presetMatches(p, currentType, currentEndpoint));
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
    const preset = presets.find((p) => p.id === presetId);
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
    <div className="settings-field" style={fieldStyle}>
      <label style={labelStyle}>{t('config.field_provider')}</label>
      <ProviderPresetRow presets={presets} activeId={currentPresetId} onSelect={applyPreset} t={t} />

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
                      {pr.label || (pr.labelKey ? t(pr.labelKey) : pr.providerType)}
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
                try { const u = new URL(url); if (u.protocol !== 'https:' && u.protocol !== 'http:') return; } catch { return; }
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
      {currentType === 'openai_agents' && (
        <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginBottom: 12 }}>
          {t('config.proto_agents_help')}
        </div>
      )}
    </div>
  );
};

/**
 * 工作模型服务商选择器 —— 复用主配置的供应商预设（内置兜底 + llm-providers 插件贡献合并）
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
export const WorkModelProviderSelector: React.FC<{
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
  // 供应商预设：与主配置共用（内置兜底 + llm-providers 插件贡献合并）
  const presets = useProviderPresets().filter((p) => p.providerType !== 'jev');
  const currentType = model.provider_type || 'openai';
  const currentEndpoint = model.endpoint || '';
  const matchingPreset = presets.find((p) => presetMatches(p, currentType, currentEndpoint));
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

    const preset = presets.find((p) => p.id === presetId);
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
      <div className="settings-field" style={fieldStyle}>
        <label style={labelStyle}>{t('config.field_provider')}</label>
        <ProviderPresetRow presets={presets} activeId={currentPresetId} onSelect={applyPresetById} t={t} />
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
                      {pr.label || (pr.labelKey ? t(pr.labelKey) : pr.providerType)}
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
                try { const u = new URL(url); if (u.protocol !== 'https:' && u.protocol !== 'http:') return; } catch { return; }
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
      {currentType === 'openai_agents' && (
        <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginBottom: 12 }}>
          {t('config.proto_agents_help')}
        </div>
      )}
    </>
  );
};
