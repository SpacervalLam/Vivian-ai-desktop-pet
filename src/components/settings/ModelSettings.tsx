import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open as openShell } from '@tauri-apps/plugin-shell';
import { ExternalLink } from 'lucide-react';
import { createAsyncCache } from './asyncCache';
import { PROVIDER_PRESETS, presetMatches, type ProviderPreset, type ProviderProtocol } from './providerPresets';
import taskCatalog from '../../../src-tauri/prompts/routing/task_catalog.json';
import { fieldStyle, labelStyle } from './SettingsFields';
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

export { PROVIDER_PRESETS, presetMatches } from './providerPresets';
export type { ProviderPreset } from './providerPresets';

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
