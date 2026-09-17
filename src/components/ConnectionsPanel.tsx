/**
 * ConnectionsPanel — 设置页「外部连接」分区
 *
 * 这里并排两种「给 AI 接外部能力」的连接，协议完全不同，不要混为一谈：
 * - 浏览器桥：Chrome 扩展反向连入本应用，自建 WebSocket 线协议（回环 3080 端口）。
 *   后端把它的工具名收进 `mcp__browser__*` 命名空间，那只是为了让模型侧只看到一套
 *   外部工具命名——它不是 MCP，本面板也从不把这个前缀显示给用户。
 * - MCP Servers：本应用拉起子进程，走标准 MCP（stdio JSON-RPC）。
 *
 * 注意「凭据状态」（平台登录态）不是连接，也不提供任何工具：
 * 它是扩展用 Cookie 哨兵探测出的布尔值，唯一消费方是 discovery 被动采集器
 * （判断某平台已登录、可以采集）。因此它作为桥卡片的附属子区呈现，
 * 不与 MCP Servers 并列，避免让人误以为"登录即获得能力"。
 *
 * 视觉约定：分区标题 → 说明段落 → 卡片。卡片内部用「一条淡分隔线 + 上留白」
 * 切子区，子区标题一律走 subTitleStyle，避免多层边框叠加造成的表格感。
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { Globe, Plug, Unplug, Loader, FolderOpen, ExternalLink, Cable } from 'lucide-react';

interface PlatformStatusView {
  platform: string;
  logged_in: boolean;
}

interface BrowserBridgeStatus {
  connected: boolean;
  platforms: PlatformStatusView[];
  reported_at_ms: number;
  extension_dir: string;
}

interface McpServerView {
  id: string;
  name: string;
  enabled: boolean;
  tool_count: number;
  alive: boolean;
}

interface McpEditingState {
  id: string;
  name: string;
  command: string;
  args: string;
  enabled: boolean;
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
};

/**
 * 桥工具的展示别名：动作名 -> i18n key。
 *
 * 后端只提供技术名（`mcp__browser__eval_js`）和面向模型的长描述，没有面向人的
 * 短标签——"给人看什么"属于 UI 关注点，补在前端而不是塞进 Rust 工具定义。
 * 缺项（新增了工具但还没补别名）回退到技术名，不会露出裸 i18n key。
 *
 * `mcp__browser__` 只是工具名的命名空间约定，不是协议：桥跑的是自建 WebSocket
 * 线协议（扩展反向连入本机），与 MCP server 的 stdio JSON-RPC 无关。统一前缀的
 * 目的是让模型侧只看到一套外部工具命名，这个动机不适用于用户，所以 UI 不暴露它。
 */
const BRIDGE_TOOL_LABEL_KEYS: Record<string, string> = {
  back: 'connections.tool_back',
  click: 'connections.tool_click',
  eval_js: 'connections.tool_eval_js',
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

const MONO_FONT = 'ui-monospace, Consolas, "Courier New", monospace';

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
const dangerButtonStyle: React.CSSProperties = {
  display: 'inline-flex',
  alignItems: 'center',
  padding: '4px 11px',
  border: '1.5px solid var(--panel-border)',
  borderRadius: 7,
  background: 'transparent',
  color: 'var(--panel-danger)',
  fontSize: 12,
  fontWeight: 500,
  cursor: 'pointer',
  flexShrink: 0,
};

/** 新增类动作：虚线框，一眼区分"这是一个待填充的入口" */
const addButtonStyle: React.CSSProperties = {
  display: 'inline-flex',
  alignItems: 'center',
  gap: 7,
  padding: '8px 16px',
  border: '1.5px dashed var(--panel-border)',
  borderRadius: 8,
  background: 'transparent',
  color: 'var(--panel-text-secondary)',
  fontSize: 12.5,
  fontWeight: 600,
  cursor: 'pointer',
};

const fieldLabelStyle: React.CSSProperties = {
  display: 'block',
  fontSize: 12,
  fontWeight: 600,
  color: 'var(--panel-text-secondary)',
  marginBottom: 6,
  paddingLeft: 2,
};

const fieldInputStyle: React.CSSProperties = {
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
};

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

const subHeaderLeftStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'baseline',
  gap: 8,
  minWidth: 0,
};

/** 弱化后的长说明文字（凭据区等），降低视觉重量而非删信息 */
const hintTextStyle: React.CSSProperties = {
  fontSize: 11.5,
  color: 'var(--panel-text-tertiary)',
  lineHeight: 1.65,
  marginBottom: 10,
};

/** 命名空间等技术标识，等宽小字 */
const namespaceTagStyle: React.CSSProperties = {
  fontFamily: MONO_FONT,
  fontSize: 10.5,
  color: 'var(--panel-text-tertiary)',
  letterSpacing: 0.2,
  whiteSpace: 'nowrap',
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

/** 引导步骤序号 */
const stepBadgeStyle: React.CSSProperties = {
  width: 20,
  height: 20,
  borderRadius: '50%',
  flexShrink: 0,
  display: 'inline-flex',
  alignItems: 'center',
  justifyContent: 'center',
  fontSize: 11,
  fontWeight: 700,
  color: 'var(--panel-accent)',
  background: 'var(--panel-accent-soft)',
  border: '1px solid var(--panel-accent-muted)',
};

/** 扩展目录路径：等宽 + 可换行，装进淡框里避免和正文混在一起 */
const pathBoxStyle: React.CSSProperties = {
  marginTop: 12,
  padding: '7px 10px',
  borderRadius: 6,
  fontSize: 11,
  color: 'var(--panel-text-tertiary)',
  fontFamily: MONO_FONT,
  wordBreak: 'break-all',
  background: 'var(--panel-bg-surface)',
  border: '1px solid var(--panel-border-light)',
};

/** MCP server 行 */
const serverRowStyle: React.CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 10,
  padding: '10px 14px',
  borderRadius: 8,
  border: '1px solid var(--panel-border-light)',
  background: 'var(--panel-surface)',
};

const TextField: React.FC<{
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
}> = ({ label, value, onChange, placeholder }) => (
  <div style={{ marginBottom: 18 }}>
    <label style={fieldLabelStyle}>{label}</label>
    <input
      type="text"
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      style={fieldInputStyle}
    />
  </div>
);

const ToggleField: React.FC<{
  label: string;
  value: boolean;
  onChange: (v: boolean) => void;
}> = ({ label, value, onChange }) => (
  <div style={{ marginBottom: 18, display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
    <label style={{ ...fieldLabelStyle, marginBottom: 0 }}>{label}</label>
    <button
      type="button"
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
        }}
      />
    </button>
  </div>
);

const ConnectionsPanel: React.FC = () => {
  const { t } = useTranslation();

  // ── 内置连接器：浏览器桥状态 ──
  const [bridge, setBridge] = useState<BrowserBridgeStatus | null>(null);
  // 存整个 ToolView 而不只是名字：description 由后端按界面语言返回，用作悬停说明
  const [bridgeTools, setBridgeTools] = useState<ToolView[]>([]);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);

  // ── MCP Servers ──
  const [mcpServers, setMcpServers] = useState<McpServerView[]>([]);
  const [mcpEditing, setMcpEditing] = useState<McpEditingState | null>(null);
  const [mcpSaving, setMcpSaving] = useState(false);

  const toast = useCallback(
    (message: string, type: 'success' | 'error') => {
      void emit('toast:show', { message, type, duration: 4000, key: Date.now() });
    },
    [],
  );

  const loadBridge = useCallback(() => {
    invoke<BrowserBridgeStatus>('get_browser_platforms')
      .then((res) => {
        if (mountedRef.current) setBridge(res);
      })
      .catch((e) => {
        if (mountedRef.current) setError(String(e));
      });
  }, []);

  const loadMcp = useCallback(async () => {
    try {
      const servers = await invoke<McpServerView[]>('list_mcp_servers');
      if (mountedRef.current) setMcpServers(servers);
    } catch {
      /* 静默：列表拉取失败不打断页面 */
    }
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
    void loadMcp();
    void loadBridgeTools();
    // 桥的连接状态由扩展侧上报，无推送通道，保持轮询
    const timer = window.setInterval(loadBridge, 4000);
    return () => {
      mountedRef.current = false;
      window.clearInterval(timer);
    };
  }, [loadBridge, loadMcp, loadBridgeTools]);

  const handleOpenExtensionFolder = () => {
    void invoke('open_extension_folder').catch((e) => setError(String(e)));
  };

  const handleOpenChromeExtensions = () => {
    // chrome:// 是 Chrome 内部 scheme，系统协议打开无效；由后端定位
    // chrome.exe 带参启动，失败（未装 Chrome 等）时显示错误提示
    void invoke('open_chrome_extensions').catch((e) => setError(String(e)));
  };

  const handleGoLogin = (url: string) => {
    // 登录页强制用 Chrome 打开：平台登录态由桥扩展在 Chrome 内探测，
    // 若走系统默认浏览器（可能是 Edge 等），登录不会同步到 Chrome
    void invoke('open_url_in_chrome', { url }).catch((e) => setError(String(e)));
  };

  const handleRemoveMcp = async (serverId: string) => {
    try {
      await invoke('remove_mcp_server', { serverId });
      await loadMcp();
      toast(t('config.mcp_removed'), 'success');
    } catch (e) {
      toast(String(e), 'error');
    }
  };

  const handleAddMcp = async () => {
    if (!mcpEditing) return;
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
      await loadMcp();
      setMcpEditing(null);
      toast(t('config.mcp_added'), 'success');
    } catch (e) {
      toast(String(e), 'error');
    } finally {
      setMcpSaving(false);
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
          {!connected && (
            <button type="button" style={ghostButtonStyle} onClick={handleOpenExtensionFolder}>
              <FolderOpen size={14} />
              {t('browser.open_extension_dir')}
            </button>
          )}
        </div>

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

      {/* === 扩展安装引导（未连接时显示） === */}
      {!connected && (
        <>
          <div style={{ ...sectionTitleStyle, marginTop: 28 }}>{t('browser.section_setup')}</div>
          <div style={cardStyle}>
            {[1, 2, 3].map((step) => (
              <div key={step} style={{ display: 'flex', gap: 11, marginBottom: step < 3 ? 12 : 0 }}>
                <span style={stepBadgeStyle}>{step}</span>
                <div
                  style={{
                    fontSize: 12.5,
                    color: 'var(--panel-text)',
                    lineHeight: 1.65,
                    paddingTop: 1,
                  }}
                >
                  {t(`browser.setup_step_${step}`)}
                </div>
              </div>
            ))}
            <div style={{ display: 'flex', gap: 10, marginTop: 16, flexWrap: 'wrap' }}>
              <button type="button" style={primaryButtonStyle} onClick={handleOpenExtensionFolder}>
                <FolderOpen size={15} />
                {t('browser.open_extension_dir')}
              </button>
              <button type="button" style={ghostButtonStyle} onClick={handleOpenChromeExtensions}>
                <ExternalLink size={14} />
                {t('browser.open_chrome_extensions')}
              </button>
            </div>
            {bridge?.extension_dir && <div style={pathBoxStyle}>{bridge.extension_dir}</div>}
          </div>
        </>
      )}

      {/* ==================== MCP Servers ==================== */}
      <div style={{ ...sectionTitleStyle, marginTop: 28 }}>{t('config.section_mcp')}</div>
      <div style={sectionDescStyle}>{t('config.mcp_description')}</div>

      {/* 已连接 server 列表 */}
      {mcpServers.length > 0 && (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, marginBottom: 14 }}>
          {mcpServers.map((s) => (
            <div key={s.id} style={serverRowStyle}>
              <span
                style={{
                  ...statusDotStyle,
                  width: 7,
                  height: 7,
                  color: s.alive ? 'var(--panel-success)' : 'var(--panel-danger)',
                }}
              />
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ display: 'flex', alignItems: 'baseline', gap: 7, flexWrap: 'wrap' }}>
                  <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--panel-text)' }}>
                    {s.name}
                  </span>
                  <code style={namespaceTagStyle}>{s.id}</code>
                </div>
                <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: 2 }}>
                  {s.alive ? t('config.mcp_alive') : t('config.mcp_dead')} · {s.tool_count}{' '}
                  {t('config.mcp_tools')}
                </div>
              </div>
              <button
                type="button"
                onClick={() => void handleRemoveMcp(s.id)}
                style={dangerButtonStyle}
              >
                {t('config.mcp_remove')}
              </button>
            </div>
          ))}
        </div>
      )}

      {/* 添加新 server */}
      {mcpEditing ? (
        <div style={{ ...cardStyle, marginBottom: 14 }}>
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
          <div
            style={{
              fontSize: 11,
              color: 'var(--panel-text-tertiary)',
              marginTop: -10,
              marginBottom: 14,
              lineHeight: 1.5,
            }}
          >
            {t('config.mcp_field_args_help')}
          </div>
          <ToggleField
            label={t('config.mcp_field_enabled')}
            value={mcpEditing.enabled}
            onChange={(v) => setMcpEditing({ ...mcpEditing, enabled: v })}
          />
          <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
            <button
              type="button"
              onClick={() => void handleAddMcp()}
              disabled={mcpSaving || !mcpEditing.id || !mcpEditing.command}
              style={{
                ...primaryButtonStyle,
                opacity: mcpSaving || !mcpEditing.id || !mcpEditing.command ? 0.5 : 1,
                cursor: mcpSaving ? 'wait' : 'pointer',
              }}
            >
              {mcpSaving ? t('config.mcp_connecting') : t('config.mcp_add')}
            </button>
            <button type="button" onClick={() => setMcpEditing(null)} style={ghostButtonStyle}>
              {t('config.mcp_cancel')}
            </button>
          </div>
        </div>
      ) : (
        <button
          type="button"
          onClick={() => setMcpEditing({ id: '', name: '', command: '', args: '', enabled: true })}
          style={addButtonStyle}
        >
          <Cable size={14} />
          {t('config.mcp_add_server')}
        </button>
      )}

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

      {error && (
        <div style={{ fontSize: 12, color: 'var(--panel-danger)', marginTop: 10 }}>{error}</div>
      )}
    </div>
  );
};

export default ConnectionsPanel;
