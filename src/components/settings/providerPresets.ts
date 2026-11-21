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
export interface ProviderProtocol {
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

/** 按端点查找错误提示使用的内置厂商；路径扩展必须位于同一来源。 */
export function findProviderPresetByEndpoint(endpoint: string): ProviderPreset | undefined {
  let target: URL;
  try { target = new URL(endpoint); } catch { return undefined; }
  const path = target.pathname.replace(/\/+$/, '').toLowerCase();
  return PROVIDER_PRESETS.find(preset =>
    [preset.endpoint, ...(preset.protocols ?? []).map(protocol => protocol.endpoint)]
      .some(endpoint => {
        if (!endpoint) return false;
        const base = new URL(endpoint);
        const basePath = base.pathname.replace(/\/+$/, '').toLowerCase();
        return target.origin === base.origin && (path === basePath || path.startsWith(basePath + '/'));
      }),
  );
}
