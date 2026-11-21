import { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export function solarTheme(hour: number, sunrise?: number, sunset?: number): 'light' | 'dark' | null {
  if (sunrise == null || sunset == null || !Number.isFinite(sunrise) || !Number.isFinite(sunset) || sunrise < 0 || sunset > 24 || sunrise >= sunset) return null;
  return hour >= sunrise && hour < sunset ? 'light' : 'dark';
}

/** Each webview resolves the same saved preference without overwriting the manual choice. */
export function useGlobalTheme() {
  useEffect(() => {
    let disposed = false;
    let generation = 0;
    let preference = 'system';
    let automatic = false;
    let sun: { sunrise_hour?: number; sunset_hour?: number } | null = null;
    const media = matchMedia('(prefers-color-scheme: dark)');
    const unlisten: (() => void)[] = [];
    const apply = () => {
      if (disposed) return;
      const now = new Date();
      const solar = automatic ? solarTheme(now.getHours() + now.getMinutes() / 60, sun?.sunrise_hour, sun?.sunset_hour) : null;
      const theme = solar ?? (preference === 'light' || preference === 'dark' ? preference : media.matches ? 'dark' : 'light');
      document.documentElement.dataset.theme = theme;
      void invoke('report_effective_theme', { theme }).catch(() => {});
    };
    const reload = async () => {
      const current = ++generation;
      try {
        const [theme, auto] = await Promise.all([
          invoke<string | null>('get_config', { key: 'base.theme' }),
          invoke<boolean>('get_config', { key: 'base.theme_auto_solar' }),
        ]);
        if (disposed || current !== generation) return;
        preference = theme ?? 'system'; automatic = auto === true;
        if (automatic) {
          try {
            const context = await invoke<{ weather?: typeof sun }>('companion_desktop_context');
            if (disposed || current !== generation) return;
            sun = context.weather ?? null;
          } catch { /* Retain the last successful solar times; otherwise use the manual preference. */ }
        }
        apply();
      } catch { apply(); }
    };
    void (async () => {
      for (const event of ['config:theme-changed', 'config:saved']) {
        const stop = await listen(event, () => void reload());
        if (disposed) stop(); else unlisten.push(stop);
      }
      await reload();
    })().catch(() => {});
    const minute = setInterval(apply, 30_000);
    const refresh = setInterval(() => void reload(), 600_000);
    const wake = () => { if (!document.hidden) void reload(); };
    window.addEventListener('focus', wake);
    document.addEventListener('visibilitychange', wake);
    media.addEventListener('change', apply);
    return () => { disposed = true; generation++; unlisten.forEach(stop => stop()); clearInterval(minute); clearInterval(refresh); window.removeEventListener('focus', wake); document.removeEventListener('visibilitychange', wake); media.removeEventListener('change', apply); };
  }, []);
}
