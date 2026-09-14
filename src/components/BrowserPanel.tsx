/**
 * BrowserPanel — 设置页「浏览器平台」分区
 *
 * 数据源：invoke('get_browser_platforms')（轮询 4s）
 * 操作：invoke('open_extension_folder') / invoke('open_chrome_extensions')；
 *       登录页经 plugin-shell open（http(s) 协议系统可正常处理）
 *
 * 展示：
 * - 桥连接状态卡（扩展未连接时显示三步引导）
 * - 平台登录态网格（扩展 Cookie 哨兵上报；已登录/未登录/检测中）
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { Globe, CircleCheck, CircleDashed, FolderOpen, ExternalLink } from 'lucide-react';

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

const sectionTitleStyle: React.CSSProperties = {
  fontSize: 13,
  fontWeight: 700,
  color: 'var(--panel-text)',
  marginBottom: 14,
  paddingBottom: 8,
  paddingLeft: 10,
  borderLeft: '3px solid var(--panel-accent)',
  borderBottom: '1.5px solid var(--panel-border)',
};

const cardStyle: React.CSSProperties = {
  padding: '16px 18px',
  borderRadius: 10,
  border: '1.5px solid var(--panel-border)',
  background: 'var(--panel-surface)',
  marginBottom: 16,
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
};

const BrowserPanel: React.FC = () => {
  const { t } = useTranslation();
  const [status, setStatus] = useState<BrowserBridgeStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);

  const load = useCallback(() => {
    invoke<BrowserBridgeStatus>('get_browser_platforms')
      .then((res) => {
        if (mountedRef.current) setStatus(res);
      })
      .catch((e) => {
        if (mountedRef.current) setError(String(e));
      });
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    load();
    const timer = window.setInterval(load, 4000);
    return () => {
      mountedRef.current = false;
      window.clearInterval(timer);
    };
  }, [load]);

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

  const connected = status?.connected ?? false;
  const reported = (status?.platforms?.length ?? 0) > 0;

  // 平台排序：已登录在前，未登录在后
  const platforms = [...(status?.platforms ?? [])].sort((a, b) => {
    if (a.logged_in !== b.logged_in) return a.logged_in ? -1 : 1;
    return a.platform.localeCompare(b.platform);
  });

  return (
    <div>
      {/* === 桥连接状态 === */}
      <div style={sectionTitleStyle}>{t('browser.section_connection')}</div>
      <div
        style={{
          ...cardStyle,
          display: 'flex',
          alignItems: 'center',
          gap: 14,
          borderColor: connected ? 'var(--panel-success, #4a9e5f)' : 'var(--panel-border)',
        }}
      >
        {connected ? (
          <CircleCheck size={26} style={{ color: 'var(--panel-success, #4a9e5f)', flexShrink: 0 }} />
        ) : (
          <CircleDashed size={26} style={{ color: 'var(--panel-text-tertiary)', flexShrink: 0 }} />
        )}
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 14, fontWeight: 600, color: 'var(--panel-text)' }}>
            {connected ? t('browser.connected_title') : t('browser.disconnected_title')}
          </div>
          <div style={{ fontSize: 12, color: 'var(--panel-text-secondary)', marginTop: 2 }}>
            {connected
              ? t('browser.connected_hint')
              : t('browser.disconnected_hint')}
          </div>
        </div>
        {!connected && (
          <button type="button" style={primaryButtonStyle} onClick={handleOpenExtensionFolder}>
            <FolderOpen size={15} />
            {t('browser.open_extension_dir')}
          </button>
        )}
      </div>

      {/* === 扩展安装引导（未连接时显示） === */}
      {!connected && (
        <>
          <div style={sectionTitleStyle}>{t('browser.section_setup')}</div>
          <div style={cardStyle}>
            {[1, 2, 3].map((step) => (
              <div key={step} style={{ display: 'flex', gap: 12, marginBottom: step < 3 ? 14 : 0 }}>
                <span
                  style={{
                    width: 22,
                    height: 22,
                    borderRadius: '50%',
                    background: 'var(--panel-accent-muted, var(--panel-accent))',
                    color: 'var(--panel-text)',
                    display: 'inline-flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    fontSize: 12,
                    fontWeight: 700,
                    flexShrink: 0,
                  }}
                >
                  {step}
                </span>
                <div style={{ fontSize: 13, color: 'var(--panel-text)', lineHeight: 1.6 }}>
                  {t(`browser.setup_step_${step}`)}
                </div>
              </div>
            ))}
            <div style={{ display: 'flex', gap: 10, marginTop: 16 }}>
              <button type="button" style={primaryButtonStyle} onClick={handleOpenExtensionFolder}>
                <FolderOpen size={15} />
                {t('browser.open_extension_dir')}
              </button>
              <button type="button" style={ghostButtonStyle} onClick={handleOpenChromeExtensions}>
                <ExternalLink size={14} />
                {t('browser.open_chrome_extensions')}
              </button>
            </div>
            {status?.extension_dir && (
              <div
                style={{
                  marginTop: 12,
                  fontSize: 11.5,
                  color: 'var(--panel-text-tertiary)',
                  fontFamily: 'Consolas, monospace',
                  wordBreak: 'break-all',
                }}
              >
                {status.extension_dir}
              </div>
            )}
          </div>
        </>
      )}

      {/* === 平台登录态 === */}
      <div style={sectionTitleStyle}>{t('browser.section_platforms')}</div>
      {!connected ? (
        <div style={{ ...cardStyle, color: 'var(--panel-text-tertiary)', fontSize: 13 }}>
          {t('browser.platforms_need_connection')}
        </div>
      ) : !reported ? (
        <div style={{ ...cardStyle, color: 'var(--panel-text-tertiary)', fontSize: 13 }}>
          {t('browser.platforms_detecting')}
        </div>
      ) : (
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fill, minmax(190px, 1fr))',
            gap: 12,
          }}
        >
          {platforms.map((p) => {
            const meta = PLATFORM_META[p.platform];
            return (
              <div
                key={p.platform}
                style={{
                  ...cardStyle,
                  marginBottom: 0,
                  display: 'flex',
                  alignItems: 'center',
                  gap: 10,
                  borderColor: p.logged_in
                    ? 'var(--panel-success, #4a9e5f)'
                    : 'var(--panel-border)',
                }}
              >
                <Globe
                  size={18}
                  style={{
                    color: p.logged_in ? 'var(--panel-success, #4a9e5f)' : 'var(--panel-text-tertiary)',
                    flexShrink: 0,
                  }}
                />
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 13.5, fontWeight: 600, color: 'var(--panel-text)' }}>
                    {meta ? t(meta.nameKey) : p.platform}
                  </div>
                  <div
                    style={{
                      fontSize: 11.5,
                      color: p.logged_in
                        ? 'var(--panel-success, #4a9e5f)'
                        : 'var(--panel-text-tertiary)',
                      marginTop: 1,
                    }}
                  >
                    {p.logged_in ? t('browser.status_logged_in') : t('browser.status_not_logged_in')}
                  </div>
                </div>
                {!p.logged_in && meta && (
                  <button
                    type="button"
                    style={{ ...ghostButtonStyle, padding: '4px 10px', fontSize: 11.5, flexShrink: 0 }}
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

      <div style={{ fontSize: 12, color: 'var(--panel-text-tertiary)', marginTop: 14, lineHeight: 1.6 }}>
        {t('browser.privacy_note')}
      </div>

      {error && (
        <div style={{ fontSize: 12, color: 'var(--panel-danger, #c0392b)', marginTop: 10 }}>{error}</div>
      )}
    </div>
  );
};

export default BrowserPanel;
