/**
 * ConnectionsPanel — 设置页「外部连接」分区
 *
 * 内嵌浏览器直接提供页面工具与普通配置的登录状态；MCP 为独立的外部连接。
 *
 * 注意「凭据状态」（平台登录态）不是连接，也不提供任何工具：
 * 它是 WebView2 普通配置中 Cookie 哨兵的布尔值，供 discovery 被动采集器
 * （判断某平台已登录、可以采集）。因此它作为桥卡片的附属子区呈现，
 * 不与 MCP Servers 并列，避免让人误以为"登录即获得能力"。
 *
 * 视觉约定：分区标题 → 说明段落 → 卡片。卡片内部用「一条淡分隔线 + 上留白」
 * 切子区，子区标题一律走 subTitleStyle，避免多层边框叠加造成的表格感。
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import McpConnections from './connections/McpConnections';
import { Globe, Plug, Unplug, Loader } from 'lucide-react';

interface PlatformStatusView {
  platform: string;
  logged_in: boolean;
}

interface BrowserBridgeStatus {
  connected: boolean;
  platforms: PlatformStatusView[];
  reported_at_ms: number;
  error: string | null;
}

interface ToolView {
  name: string;
  description: string;
  category: string;
}

/**
 * 后端给桥工具注册的名字前缀（与后端 BRIDGE_SERVER_ID 一致）。
 *
 * 只用于「从 list_tools 结果里筛出桥工具」和「提取动作名」两件事，**不展示给用户**：
 * 那是模型侧的命名约定，前端一律显示别名。
 */
const BRIDGE_TOOL_PREFIX = 'mcp__browser__';

/** 平台元数据：显示名 + 登录页（未登录时一键前往） */
const PLATFORM_META: Record<string, { nameKey: string; loginUrl: string }> = {
  bilibili: { nameKey: 'browser.platform_bilibili', loginUrl: 'https://passport.bilibili.com/login' },
  zhihu: { nameKey: 'browser.platform_zhihu', loginUrl: 'https://www.zhihu.com/signin' },
  xiaohongshu: { nameKey: 'browser.platform_xiaohongshu', loginUrl: 'https://www.xiaohongshu.com' },
  douyin: { nameKey: 'browser.platform_douyin', loginUrl: 'https://www.douyin.com/' },
  weibo: { nameKey: 'browser.platform_weibo', loginUrl: 'https://passport.weibo.com/signin/login' },
  v2ex: { nameKey: 'browser.platform_v2ex', loginUrl: 'https://www.v2ex.com/signin' },
  bangumi: { nameKey: 'browser.platform_bangumi', loginUrl: 'https://bgm.tv/login' },
  youtube: { nameKey: 'browser.platform_youtube', loginUrl: 'https://accounts.google.com/' },
  twitter: { nameKey: 'browser.platform_twitter', loginUrl: 'https://x.com/i/flow/login' },
  reddit: { nameKey: 'browser.platform_reddit', loginUrl: 'https://www.reddit.com/login/' },
};

/**
 * 桥工具的展示别名：动作名 -> i18n key。
 *
 * 后端只提供技术名（`mcp__browser__eval_js`）和面向模型的长描述，没有面向人的
 * 短标签——"给人看什么"属于 UI 关注点，补在前端而不是塞进 Rust 工具定义。
 * 缺项（新增了工具但还没补别名）回退到技术名，不会露出裸 i18n key。
 *
 * `mcp__browser__` 保留既有工具与权限的命名约定；实际直接调用 WebView2，与 MCP 的
 * stdio JSON-RPC 无关。统一前缀的
 * 目的是让模型侧只看到一套外部工具命名，这个动机不适用于用户，所以 UI 不暴露它。
 */
const BRIDGE_TOOL_LABEL_KEYS: Record<string, string> = {
  back: 'connections.tool_back',
  click: 'connections.tool_click',
  extract: 'connections.tool_extract',
  forward: 'connections.tool_forward',
  get_text: 'connections.tool_get_text',
  navigate: 'connections.tool_navigate',
  press: 'connections.tool_press',
  reload: 'connections.tool_reload',
  scroll: 'connections.tool_scroll',
  snapshot: 'connections.tool_snapshot',
  task_tab: 'connections.tool_task_tab',
  type: 'connections.tool_type',
  wait: 'connections.tool_wait',
};


const sectionTitleStyle: React.CSSProperties = {
  fontSize: 13,
  fontWeight: 700,
  color: 'var(--panel-text)',
  marginBottom: 12,
  paddingBottom: 8,
  paddingLeft: 10,
  borderLeft: '3px solid var(--panel-accent)',
  borderBottom: '1.5px solid var(--panel-border)',
};

/** 分区说明段落（与卡片内文案拉开层级：更淡、更小、无边框） */
const sectionDescStyle: React.CSSProperties = {
  fontSize: 12,
  color: 'var(--panel-text-secondary)',
  lineHeight: 1.7,
  marginBottom: 14,
};

const cardStyle: React.CSSProperties = {
  padding: '16px 18px',
  borderRadius: 10,
  border: '1.5px solid var(--panel-border)',
  background: 'var(--panel-surface)',
  marginBottom: 16,
};

/** 卡片内子区块：一条淡分隔线 + 上留白，避免和卡片边框形成"表格"错觉 */
const subSectionStyle: React.CSSProperties = {
  marginTop: 18,
  paddingTop: 16,
  borderTop: '1px solid var(--panel-border-light)',
};

const primaryButtonStyle: React.CSSProperties = {
  display: 'inline-flex',
  alignItems: 'center',
  gap: 6,
  padding: '7px 16px',
  border: 'none',
  borderRadius: 8,
  background: 'var(--panel-accent)',
  color: 'var(--panel-bg)',
  fontSize: 13,
  fontWeight: 600,
  cursor: 'pointer',
  flexShrink: 0,
};

const ghostButtonStyle: React.CSSProperties = {
  display: 'inline-flex',
  alignItems: 'center',
  gap: 6,
  padding: '7px 14px',
  border: '1.5px solid var(--panel-border)',
  borderRadius: 8,
  background: 'transparent',
  color: 'var(--panel-text-secondary)',
  fontSize: 12.5,
  fontWeight: 500,
  cursor: 'pointer',
  flexShrink: 0,
};

/** 危险动作（移除）：描边红字，比实心红底克制 */
/** 分区小标题（卡片内部的二级标题） */
const subTitleStyle: React.CSSProperties = {
  fontSize: 12,
  fontWeight: 600,
  color: 'var(--panel-text-secondary)',
  marginBottom: 8,
  letterSpacing: 0.2,
};

/** 子标题与右侧元信息同一行时的排布 */
const subHeaderStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'baseline',
  justifyContent: 'space-between',
  gap: 10,
  marginBottom: 9,
};

/** 弱化后的长说明文字（凭据区等），降低视觉重量而非删信息 */
const hintTextStyle: React.CSSProperties = {
  fontSize: 11.5,
  color: 'var(--panel-text-tertiary)',
  lineHeight: 1.65,
  marginBottom: 10,
};

/** 状态色点：background 取 currentColor，使用时只设 color 即可 */
const statusDotStyle: React.CSSProperties = {
  width: 6,
  height: 6,
  borderRadius: '50%',
  background: 'currentColor',
  flexShrink: 0,
};

/** 桥卡片左侧图标底座 */
const iconWellStyle = (on: boolean): React.CSSProperties => ({
  width: 34,
  height: 34,
  borderRadius: 10,
  flexShrink: 0,
  display: 'inline-flex',
  alignItems: 'center',
  justifyContent: 'center',
  color: on ? 'var(--panel-success)' : 'var(--panel-text-tertiary)',
  background: 'var(--panel-bg-surface)',
  border: `1px solid ${on ? 'var(--panel-success)' : 'var(--panel-border)'}`,
});

/** 连接状态胶囊：色点 + 短标签，比长句副标题更快被扫到 */
const statusPillStyle = (on: boolean): React.CSSProperties => ({
  display: 'inline-flex',
  alignItems: 'center',
  gap: 5,
  padding: '2px 9px',
  borderRadius: 999,
  fontSize: 11,
  fontWeight: 600,
  lineHeight: '17px',
  whiteSpace: 'nowrap',
  color: on ? 'var(--panel-success)' : 'var(--panel-text-tertiary)',
  border: `1px solid ${on ? 'var(--panel-success)' : 'var(--panel-border)'}`,
});

const cardTitleStyle: React.CSSProperties = {
  fontSize: 14,
  fontWeight: 700,
  color: 'var(--panel-text)',
  letterSpacing: 0.2,
};

const cardDescStyle: React.CSSProperties = {
  fontSize: 12,
  color: 'var(--panel-text-secondary)',
  marginTop: 3,
  lineHeight: 1.6,
};

/**
 * 工具 chip：显示面向人的别名（如「执行 JS」）；技术名与后端说明走 title 悬停。
 * 别名长度比技术名均匀（2-4 字 vs `eval_js` / `task_tab` 的 7-8 字符），grid 排得更齐。
 */
const toolChipStyle: React.CSSProperties = {
  fontSize: 11.5,
  color: 'var(--panel-text-secondary)',
  background: 'var(--panel-bg-surface)',
  border: '1px solid var(--panel-border-light)',
  borderRadius: 6,
  padding: '4px 8px',
  textAlign: 'center',
  whiteSpace: 'nowrap',
  overflow: 'hidden',
  textOverflow: 'ellipsis',
};

/** 定宽列网格：换行后不会出现锯齿状的右边缘 */
const toolGridStyle: React.CSSProperties = {
  display: 'grid',
  gridTemplateColumns: 'repeat(auto-fill, minmax(96px, 1fr))',
  gap: 6,
};

const platformCardStyle = (on: boolean): React.CSSProperties => ({
  display: 'flex',
  alignItems: 'center',
  gap: 9,
  padding: '8px 10px',
  borderRadius: 8,
  border: `1px solid ${on ? 'var(--panel-success)' : 'var(--panel-border-light)'}`,
  background: on ? 'var(--panel-bg-surface-elevated)' : 'transparent',
});

/** 空状态占位：虚线框，明确这是"待填充"而不是排版漏了一行 */
const emptyStateStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  justifyContent: 'center',
  gap: 8,
  padding: '16px 14px',
  borderRadius: 8,
  border: '1px dashed var(--panel-border)',
  color: 'var(--panel-text-tertiary)',
  fontSize: 12.5,
};

const ConnectionsPanel: React.FC = () => {
  const { t } = useTranslation();

  // ── 内置连接器：浏览器桥状态 ──
  const [bridge, setBridge] = useState<BrowserBridgeStatus | null>(null);
  // 存整个 ToolView 而不只是名字：description 由后端按界面语言返回，用作悬停说明
  const [bridgeTools, setBridgeTools] = useState<ToolView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [probing, setProbing] = useState(false);
  const [probeResult, setProbeResult] = useState<string | null>(null);
  useEffect(() => {
    if (!bridge?.connected) setProbeResult(null);
  }, [bridge?.connected]);
  const mountedRef = useRef(true);

  // ── MCP Servers ──

  const loadBridge = useCallback(() => {
    invoke<BrowserBridgeStatus>('get_browser_platforms')
      .then((res) => {
        if (mountedRef.current) setBridge(res);
      })
      .catch((e) => {
        if (mountedRef.current) setError(String(e));
      });
  }, []);

  /** 桥工具清单从后端取（而非前端硬编码），保证与注册的工具名同步 */
  const loadBridgeTools = useCallback(async () => {
    try {
      const res = await invoke<{ tools: ToolView[] }>('list_tools');
      const tools = (res?.tools ?? []).filter((tool) =>
        tool.name.startsWith(BRIDGE_TOOL_PREFIX),
      );
      if (mountedRef.current) setBridgeTools(tools);
    } catch {
      /* 静默 */
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    loadBridge();
    void loadBridgeTools();
    // 读取内嵌浏览器引擎状态和普通配置的登录检测缓存
    const timer = window.setInterval(loadBridge, 4000);
    return () => {
      mountedRef.current = false;
      window.clearInterval(timer);
    };
  }, [loadBridge, loadBridgeTools]);

  const handleOpenBrowser = () => { void invoke('open_embedded_browser').catch((e) => setError(String(e))); };

  const handleGoLogin = (url: string) => {
    void invoke('open_embedded_browser', { url }).catch((e) => setError(String(e)));
  };

  const handleProbe = async () => {
    setError(null);
    setProbing(true);
    setProbeResult(null);
    try {
      const result = await invoke<string>('verify_browser_page');
      if (mountedRef.current) setProbeResult(result);
    } catch (e) {
      if (mountedRef.current) setError(String(e));
    } finally {
      if (mountedRef.current) setProbing(false);
    }
  };

  const connected = bridge?.connected ?? false;
  const reported = (bridge?.platforms?.length ?? 0) > 0;

  // 平台排序：已登录在前，未登录在后
  const platforms = [...(bridge?.platforms ?? [])].sort((a, b) => {
    if (a.logged_in !== b.logged_in) return a.logged_in ? -1 : 1;
    return a.platform.localeCompare(b.platform);
  });

  const loggedInCount = platforms.filter((p) => p.logged_in).length;

  return (
    <div>
      {/* ==================== 内置连接器 ==================== */}
      <div style={sectionTitleStyle}>{t('connections.section_builtin')}</div>
      <div style={sectionDescStyle}>{t('connections.builtin_description')}</div>

      {/* === 浏览器桥卡片 === */}
      <div
        style={{
          ...cardStyle,
          borderColor: connected ? 'var(--panel-success)' : 'var(--panel-border)',
        }}
      >
        {/* 卡片头：图标 + 名称 + 状态胶囊 + 就近动作 */}
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <span style={iconWellStyle(connected)}>
            {connected ? <Plug size={16} /> : <Unplug size={16} />}
          </span>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
              <span style={cardTitleStyle}>{t('connections.bridge_name')}</span>
              <span style={statusPillStyle(connected)}>
                <span style={statusDotStyle} />
                {connected
                  ? t('connections.bridge_connected')
                  : t('connections.bridge_disconnected')}
              </span>
            </div>
            <div style={cardDescStyle}>
              {connected ? t('browser.connected_hint') : t('browser.disconnected_hint')}
            </div>
          </div>
          {/* 就近入口用次要样式：主按钮留给下方安装引导，避免两个主按钮抢焦点 */}
          <button type="button" style={primaryButtonStyle} onClick={handleOpenBrowser}><Globe size={14} />{t('browser.open_browser')}</button>
        </div>

        {connected && (
          <div style={subSectionStyle}>
            <button type="button" style={ghostButtonStyle} disabled={probing} onClick={() => void handleProbe()}>
              {probing ? t('browser.probing') : t('browser.verify_page')}
            </button>
            <div style={hintTextStyle}>{t('browser.verify_hint')}</div>
            {probeResult && (
              <pre style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', maxHeight: 180, overflow: 'auto', fontSize: 11 }}>
                {probeResult}
              </pre>
            )}
          </div>
        )}

        {/* 工具清单：默认显示面向人的别名，技术名与后端说明收进悬停 */}
        {bridgeTools.length > 0 && (
          <div style={subSectionStyle}>
            <div style={{ ...subTitleStyle, marginBottom: 9 }}>
              {t('connections.bridge_tools', { count: bridgeTools.length })}
            </div>
            <div style={toolGridStyle}>
              {bridgeTools.map((tool) => {
                const action = tool.name.slice(BRIDGE_TOOL_PREFIX.length);
                const labelKey = BRIDGE_TOOL_LABEL_KEYS[action];
                return (
                  <span
                    key={tool.name}
                    /* 悬停只给后端说明；技术名（含 mcp__ 前缀）不出现在 UI 上 */
                    title={tool.description || action}
                    style={toolChipStyle}
                  >
                    {labelKey ? t(labelKey) : action}
                  </span>
                );
              })}
            </div>
          </div>
        )}

        {/* 凭据状态：平台登录态（附属子区，不是独立连接） */}
        <div style={subSectionStyle}>
          <div style={{ ...subHeaderStyle, justifyContent: 'flex-start' }}>
            <span style={{ ...subTitleStyle, marginBottom: 0 }}>
              {t('connections.credentials_title')}
            </span>
            {reported && (
              <span style={{ fontSize: 11, fontWeight: 500, color: 'var(--panel-text-tertiary)' }}>
                {t('connections.credentials_count', {
                  logged: loggedInCount,
                  total: platforms.length,
                })}
              </span>
            )}
          </div>
          <div style={hintTextStyle}>{t('connections.credentials_hint')}</div>

          {!connected ? (
            <div style={emptyStateStyle}>
              <Globe size={15} />
              {t('browser.platforms_need_connection')}
            </div>
          ) : !reported ? (
            <div style={emptyStateStyle}>
              <Loader size={15} />
              {t('browser.platforms_detecting')}
            </div>
          ) : (
            <div
              style={{
                display: 'grid',
                gridTemplateColumns: 'repeat(auto-fill, minmax(174px, 1fr))',
                gap: 8,
              }}
            >
              {platforms.map((p) => {
                const meta = PLATFORM_META[p.platform];
                return (
                  <div key={p.platform} style={platformCardStyle(p.logged_in)}>
                    <span
                      style={{
                        ...statusDotStyle,
                        color: p.logged_in
                          ? 'var(--panel-success)'
                          : 'var(--panel-text-quaternary)',
                      }}
                    />
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div
                        style={{
                          fontSize: 12.5,
                          fontWeight: 600,
                          color: 'var(--panel-text)',
                          whiteSpace: 'nowrap',
                          overflow: 'hidden',
                          textOverflow: 'ellipsis',
                        }}
                      >
                        {meta ? t(meta.nameKey) : p.platform}
                      </div>
                      <div
                        style={{
                          fontSize: 11,
                          color: p.logged_in
                            ? 'var(--panel-success)'
                            : 'var(--panel-text-tertiary)',
                        }}
                      >
                        {p.logged_in
                          ? t('browser.status_logged_in')
                          : t('browser.status_not_logged_in')}
                      </div>
                    </div>
                    {!p.logged_in && meta && (
                      <button
                        type="button"
                        style={{ ...ghostButtonStyle, padding: '3px 10px', fontSize: 11 }}
                        onClick={() => handleGoLogin(meta.loginUrl)}
                      >
                        {t('browser.go_login')}
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </div>

      {/* ==================== MCP Servers ==================== */}
      <McpConnections />

      <div
        style={{
          fontSize: 12,
          color: 'var(--panel-text-tertiary)',
          marginTop: 14,
          lineHeight: 1.6,
        }}
      >
        {t('browser.privacy_note')}
      </div>

      {(error || bridge?.error) && (
        <div role="alert" style={{ fontSize: 12, color: 'var(--panel-danger)', marginTop: 10 }}>{error || bridge?.error}</div>
      )}
    </div>
  );
};

export default ConnectionsPanel;
