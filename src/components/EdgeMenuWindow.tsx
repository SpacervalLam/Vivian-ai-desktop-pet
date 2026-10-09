import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emit, listen } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import ChatEdgeMenu, { type EdgeMenuAction } from './ChatEdgeMenu';

export default function EdgeMenuWindow() {
  const [mode, setMode] = useState<'menu' | 'folding'>('menu');
  const [status, setStatus] = useState({ quiet: false });
  const quiet = useRef(false);
  const target = useRef<HTMLButtonElement>(null);
  const { i18n } = useTranslation();
  useEffect(() => {
    let disposed = false, revision = 0, themeRevision = 0;
    let firstPaint = 0, secondPaint = 0;
    const cleanups: Array<() => void> = [];
    const register = async (name: string, callback: (payload: any) => void) => {
      const cleanup = await listen(name, event => { if (!disposed) callback(event.payload); });
      if (disposed) cleanup(); else cleanups.push(cleanup);
    };
    const refresh = async () => {
      const before = revision;
      const next = await invoke<{ quiet: boolean }>('edge_menu_status');
      if (!disposed && revision === before) { quiet.current = next.quiet; setStatus(next); }
    };
    const applyTheme = (theme: unknown) => {
      document.documentElement.dataset.theme = theme === 'light' || theme === 'dark' ? theme : 'system';
    };
    const refreshTheme = async () => {
      const before = ++themeRevision;
      const theme = await invoke<string | null>('get_config', { key: 'base.theme' });
      if (!disposed && before === themeRevision) applyTheme(theme);
    };
    void (async () => {
      await register('companion:quiet-changed', ({ active }: { active: boolean }) => {
        revision++; quiet.current = active; setStatus(previous => ({ ...previous, quiet: active }));
      });
      await register('edge_menu:fold', () => setMode('folding'));
      await register('config:theme-changed', ({ theme }: { theme: string }) => { themeRevision++; applyTheme(theme); });
      await register('config:saved', () => { void refreshTheme().catch(console.warn); });
      await register('edge_menu:shown', ({ theme }: { theme?: string }) => {
        setMode('menu');
        if (theme !== undefined) { themeRevision++; applyTheme(theme); }
        else void refreshTheme().catch(console.warn);
        void refresh().catch(console.warn);
      });
      await refreshTheme();
      await refresh();
      if (disposed) return;
      // The first entrance must wait for event subscriptions and the chosen palette.
      firstPaint = requestAnimationFrame(() => {
        secondPaint = requestAnimationFrame(() => {
          if (!disposed) void invoke('edge_menu_ready').catch(console.warn);
        });
      });
    })().catch(console.warn);
    return () => {
      disposed = true;
      cancelAnimationFrame(firstPaint); cancelAnimationFrame(secondPaint);
      cleanups.forEach(cleanup => cleanup());
    };
  }, []);

  const onFoldEnd = useCallback(() => { void invoke('hide_edge_menu'); setMode('menu'); }, []);
  const run = async (action: EdgeMenuAction) => {
    const result = await invoke<{ saved_path?: string }>('edge_menu_action', { action, enabled: action === 'dnd' ? !quiet.current : null });
    if (result.saved_path) void emit('toast:show', {
      message: i18n.language.startsWith('zh') ? '已保存至系统截图目录' : 'Screenshot saved',
      type: 'success', duration: 2500, key: Date.now(),
    });
  };
  return <div style={{ position: 'relative', height: '100vh', fontFamily: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "Microsoft YaHei", sans-serif' }}>
    <ChatEdgeMenu edge={{ mode, width: window.innerWidth }} target={target} quiet={status.quiet} onAction={run} onFoldEnd={onFoldEnd} />
  </div>;
}
