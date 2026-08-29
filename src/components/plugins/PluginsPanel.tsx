import React, { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

/** 插件清单条目（对齐后端 plugins::PluginInventoryEntry） */
interface PluginEntry {
  name: string;
  version: string;
  description: string;
  skills: string[];
  mcp_servers: string[];
  status: string;
  reason?: string | null;
  dir: string;
}

/** 技能条目（对齐后端 commands::plugins::SkillEntryInfo） */
interface SkillEntry {
  name: string;
  description: string;
  scope: string | null;
  origin: string; // user / plugin
  body_len: number;
}

const sectionTitle: React.CSSProperties = {
  fontSize: 14,
  fontWeight: 700,
  color: 'var(--panel-text)',
  margin: '0 0 12px',
};

/** 设置窗口「插件/技能」页：盘点插件与技能清单（只读，不装载/卸载）。 */
const PluginsPanel: React.FC = () => {
  const { t } = useTranslation();
  const [plugins, setPlugins] = useState<PluginEntry[] | null>(null);
  const [skills, setSkills] = useState<SkillEntry[] | null>(null);
  const [paths, setPaths] = useState<{ plugins_dir: string; skills_dir: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [p, s, d] = await Promise.all([
          invoke<PluginEntry[]>('list_plugins'),
          invoke<SkillEntry[]>('list_skills'),
          invoke<{ plugins_dir: string; skills_dir: string }>('plugin_paths'),
        ]);
        if (cancelled) return;
        setPlugins(p);
        setSkills(s);
        setPaths(d);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const originLabel = (origin: string): string => {
    if (origin === 'plugin') return t('config.plugins.origin_plugin');
    return t('config.plugins.origin_user');
  };

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 22 }}>
      <div style={sectionTitle}>{t('config.plugins.section_plugins')}</div>
      {error ? (
        <div style={{ fontSize: 13, color: '#E53935' }}>{error}</div>
      ) : plugins === null ? (
        <div style={{ fontSize: 13, color: 'var(--panel-text-secondary)' }}>{t('mind_inspector.common.loading')}</div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          {plugins.length === 0 && (
            <div style={{ fontSize: 13, color: 'var(--panel-text-secondary)' }}>
              {t('config.plugins.no_plugins')}
              {paths && (
                <span>{t('config.plugins.plugins_dir_hint', { dir: paths.plugins_dir })}</span>
              )}
            </div>
          )}
          {plugins.map((p) => (
            <div
              key={p.name}
              title={p.dir}
              style={{
                border: '1px solid var(--panel-border)',
                borderRadius: 8,
                padding: '8px 12px',
                background: 'var(--panel-surface)',
                boxShadow: 'var(--panel-shadow-subtle)',
              }}
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 2 }}>
                <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--panel-text)' }}>{p.name}</span>
                <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)' }}>v{p.version}</span>
                <span
                  style={{
                    fontSize: 11,
                    padding: '1px 8px',
                    borderRadius: 999,
                    color: p.status === 'loaded' ? 'var(--panel-text)' : '#8B2C1F',
                    border: `1px solid ${p.status === 'loaded' ? 'var(--panel-border-strong)' : 'var(--panel-border)'}`,
                  }}
                >
                  {p.status === 'loaded' ? t('config.plugins.status_loaded') : t('config.plugins.status_skipped')}
                </span>
              </div>
              {p.description && (
                <div
                  style={{
                    fontSize: 12,
                    color: 'var(--panel-text-secondary)',
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {p.description}
                </div>
              )}
              <div style={{ fontSize: 12, color: 'var(--panel-text-secondary)' }}>
                {[
                  p.skills.length > 0 ? t('config.plugins.skills_count', { n: p.skills.length }) : '',
                  p.mcp_servers.length > 0 ? `MCP ${p.mcp_servers.length}` : '',
                  p.reason ? t('config.plugins.skip_reason', { reason: p.reason }) : '',
                ].filter(Boolean).join(' · ') || '—'}
              </div>
            </div>
          ))}
        </div>
      )}

      <div style={sectionTitle}>{t('config.plugins.section_skills')}</div>
      {error ? null : skills === null ? (
        <div style={{ fontSize: 13, color: 'var(--panel-text-secondary)' }}>{t('mind_inspector.common.loading')}</div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          {skills.length === 0 && (
            <div style={{ fontSize: 13, color: 'var(--panel-text-secondary)' }}>
              {t('config.plugins.no_skills')}
              {paths && (
                <span>{t('config.plugins.skills_dir_hint', { dir: paths.skills_dir })}</span>
              )}
            </div>
          )}
          {skills.map((s) => (
            <div
              key={s.name}
              title={s.description || s.name}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 10,
                padding: '6px 12px',
                borderBottom: '1px solid var(--panel-border)',
              }}
            >
              <span style={{ fontSize: 13, fontWeight: 500, color: 'var(--panel-text)', minWidth: 120 }}>
                {s.name}
              </span>
              <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', flexShrink: 0 }}>
                {originLabel(s.origin)}
              </span>
              <span
                style={{
                  flex: 1,
                  fontSize: 12,
                  color: 'var(--panel-text-secondary)',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap',
                }}
              >
                {s.description || '—'}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
};

export default PluginsPanel;