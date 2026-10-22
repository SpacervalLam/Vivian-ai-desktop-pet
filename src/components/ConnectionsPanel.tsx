import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { ArrowUpRight, Globe, Loader } from 'lucide-react';
import McpConnections from './connections/McpConnections';
import './connections/ConnectionsPanel.css';

interface BrowserBridgeStatus {
  connected: boolean;
  platforms: { platform: string; logged_in: boolean }[];
  reported_at_ms: number;
  error: string | null;
  credential_error: string | null;
}

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

const ConnectionsPanel: React.FC = () => {
  const { t } = useTranslation();
  const [bridge, setBridge] = useState<BrowserBridgeStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  const loadingBridge = useRef(false);

  const loadBridge = useCallback(() => {
    if (loadingBridge.current) return;
    loadingBridge.current = true;
    invoke<BrowserBridgeStatus>('get_browser_platforms')
      .then(res => {
        if (mountedRef.current) { setBridge(res); setStatusError(null); }
      })
      .catch(e => { if (mountedRef.current) setStatusError(String(e)); })
      .finally(() => { loadingBridge.current = false; });
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    loadBridge();
    const timer = window.setInterval(loadBridge, 4000);
    return () => { mountedRef.current = false; window.clearInterval(timer); };
  }, [loadBridge]);

  const openBrowser = (url?: string) => {
    setError(null);
    void invoke('open_embedded_browser', url ? { url } : undefined)
      .catch(e => { if (mountedRef.current) setError(String(e)); });
  };
  const connected = bridge?.connected ?? false;
  const stale = !!bridge?.credential_error || !!statusError;
  const platforms = [...(bridge?.platforms ?? [])].sort((a, b) =>
    Number(b.logged_in) - Number(a.logged_in) || a.platform.localeCompare(b.platform));
  const loggedIn = platforms.filter(p => p.logged_in).length;

  return (
    <div className="external-connections">
      <div className="connection-section-heading">
        <h3>{t('connections.section_builtin')}</h3>
        <p>{t('connections.builtin_description')}</p>
      </div>
      <section className="connection-browser-card" aria-label={t('connections.bridge_name')}>
        <div className="connection-browser-header">
          <span className="connection-service-icon"><Globe size={21} /></span>
          <div className="connection-service-title">
            <div className="connection-title-row">
              <h4>{t('connections.bridge_name')}</h4>
              <span className={`connection-status${connected ? ' is-connected' : ''}`} role="status">
                <span className="connection-status-dot" />
                {t(connected ? 'connections.bridge_connected' : 'connections.bridge_disconnected')}
                {statusError && ` · ${t('browser.status_refresh_failed')}`}
              </span>
            </div>
            <p>{t(connected ? 'browser.connected_hint' : 'browser.disconnected_hint')}</p>
          </div>
          <button type="button" className="connection-button connection-button-primary" onClick={() => openBrowser()}>
            {t('browser.open_browser')}<ArrowUpRight size={15} />
          </button>
        </div>
        <div className="connection-platform-section">
          <div className="connection-platform-heading">
            <h5>{t('connections.credentials_title')}</h5>
            {platforms.length > 0 && <span className="connection-count">{t('connections.credentials_count', { logged: loggedIn, total: platforms.length })}</span>}
            {platforms.length > 0 && !stale && <span className="connection-checked">{t('browser.credentials_checked', { time: new Date(bridge!.reported_at_ms).toLocaleTimeString() })}</span>}
          </div>
          <p className="connection-hint">{t('connections.credentials_hint')}</p>
          {stale && platforms.length > 0 && <p className="connection-hint" role="status">{t('browser.credentials_stale', { time: new Date(bridge!.reported_at_ms).toLocaleTimeString() })}</p>}
          {bridge?.credential_error && <p className="connection-hint" role="status">{bridge.credential_error}</p>}
          {!connected || platforms.length === 0 ? (
            <div className="connection-empty-inline" role="status">
              {connected ? <Loader size={16} className="connection-spin" /> : <Globe size={16} />}
              {t(connected ? 'browser.platforms_detecting' : 'browser.platforms_need_connection')}
            </div>
          ) : (
            <div className="connection-platform-grid">
              {platforms.map(p => {
                const meta = PLATFORM_META[p.platform];
                return <div key={p.platform} className={`connection-platform${p.logged_in && !stale ? ' has-credentials' : ''}`}>
                  <span className="connection-status-dot" />
                  <div className="connection-platform-name">
                    <strong>{meta ? t(meta.nameKey) : p.platform}</strong>
                    <span>{t(p.logged_in ? stale ? 'browser.credentials_previously_detected' : 'browser.status_logged_in' : 'browser.status_not_logged_in')}</span>
                  </div>
                  {!p.logged_in && meta && <button type="button" className="connection-login" onClick={() => openBrowser(meta.loginUrl)}>
                    {t('browser.go_login')}<ArrowUpRight size={12} />
                  </button>}
                </div>;
              })}
            </div>
          )}
        </div>
        {(error || statusError || bridge?.error) && <div role="alert" className="connection-alert">{error || statusError || bridge?.error}</div>}
      </section>
      <McpConnections />
    </div>
  );
};

export default ConnectionsPanel;
