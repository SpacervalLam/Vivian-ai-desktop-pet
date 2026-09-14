/**
 * 交互终端面板（ConPTY ↔ xterm.js 桥接）
 *
 * - 挂载时 terminal_create 启动持久 PowerShell 会话（工作目录 = 编程会话目录）
 * - `terminal:data` 事件 → term.write()（UTF-8 流，含 VT 颜色/光标序列）
 * - term.onData → terminal_write（回显由 ConPTY 负责）；onResize → terminal_resize
 * - 卸载时 terminal_kill 释放 ConPTY 资源
 *
 * 本组件经 React.lazy 懒加载：仅在「编程」页签切到终端 tab 时才加载 xterm 代码。
 */

import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { Loader2 } from 'lucide-react';
import { COLORS, SPACING, RADIUS } from '../design-system';

interface TerminalPanelProps {
  workingDirectory: string;
}

/** 从 CSS 变量解析具体色值（xterm 主题需要实际颜色而非 var() 引用） */
function cssVar(name: string, fallback: string): string {
  if (typeof window === 'undefined') return fallback;
  const v = getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return v || fallback;
}

/** 当前是否为浅色主题（跟随系统 + data-theme 覆盖） */
function isLightTheme(): boolean {
  if (typeof window === 'undefined') return false;
  const forced = document.documentElement.getAttribute('data-theme');
  if (forced === 'light') return true;
  if (forced === 'dark') return false;
  return window.matchMedia('(prefers-color-scheme: light)').matches;
}

/** 终端 ANSI 调色板（深色 Tokyo Night / 浅色暖纸风格） */
function termPalette(light: boolean) {
  return light
    ? {
        black: '#4a433c',
        brightBlack: '#8f867b',
        red: '#b3402f',
        brightRed: '#c94f3c',
        green: '#4a6b4a',
        brightGreen: '#5a7d5a',
        yellow: '#a2721f',
        brightYellow: '#c08a2e',
        blue: '#3f6179',
        brightBlue: '#537d96',
        magenta: '#7a5c99',
        brightMagenta: '#9775b5',
        cyan: '#3a7a80',
        brightCyan: '#4a8f96',
        white: '#2a2622',
        brightWhite: '#2a2622',
      }
    : {
        black: '#414868',
        brightBlack: '#545c7e',
        red: '#f7768e',
        brightRed: '#f7768e',
        green: '#9ece6a',
        brightGreen: '#9ece6a',
        yellow: '#e0af68',
        brightYellow: '#e0af68',
        blue: '#7aa2f7',
        brightBlue: '#7aa2f7',
        magenta: '#bb9af7',
        brightMagenta: '#bb9af7',
        cyan: '#7dcfff',
        brightCyan: '#7dcfff',
        white: '#c0caf5',
        brightWhite: '#c0caf5',
      };
}

/** 构建当前主题下的 xterm theme（每次主题切换时重新读取 CSS 变量） */
function buildTheme(light: boolean): Record<string, string | undefined> {
  const palette = termPalette(light);
  return {
    background: cssVar('--panel-elevated', light ? '#ffffff' : '#1a1b26'),
    foreground: cssVar('--panel-text', light ? '#2a2622' : '#c0caf5'),
    cursor: cssVar('--panel-accent', light ? '#537d96' : '#7aa2f7'),
    cursorAccent: cssVar('--panel-elevated', light ? '#ffffff' : '#1a1b26'),
    selectionBackground: cssVar('--panel-accent-muted', 'rgba(122,162,247,0.35)'),
    selectionForeground: undefined,
    ...palette,
  };
}

const TerminalPanelInner: React.FC<TerminalPanelProps> = ({ workingDirectory }) => {
  const { t } = useTranslation();
  const containerRef = useRef<HTMLDivElement>(null);
  const sessionIdRef = useRef<string | null>(null);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const termRef = useRef<any>(null);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const fitRef = useRef<any>(null);
  const [error, setError] = useState<string | null>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    let disposed = false;
    const unlistens: UnlistenFn[] = [];
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let term: any = null;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    let fit: any = null;

    void (async () => {
      try {
        // 动态 import xterm（懒加载，主包不含此依赖）
        const [{ Terminal }, { FitAddon }] = await Promise.all([
          import('@xterm/xterm'),
          import('@xterm/addon-fit'),
        ]);
        await import('@xterm/xterm/css/xterm.css');
        if (disposed || !containerRef.current) return;

        // 创建 ConPTY 会话
        const sid = await invoke<string>('terminal_create', {
          workingDirectory,
          cols: 100,
          rows: 26,
        });
        if (disposed) {
          void invoke('terminal_kill', { sessionId: sid }).catch(() => {});
          return;
        }
        sessionIdRef.current = sid;

        // xterm 实例（主题取自设计系统 CSS 变量，深浅色各自适配）
        term = new Terminal({
          fontSize: 11,
          fontFamily: 'Consolas, "Courier New", monospace',
          cursorBlink: true,
          cursorStyle: 'bar',
          cursorWidth: 2,
          scrollback: 5000,
          convertEol: false,
          allowProposedApi: true,
          theme: buildTheme(isLightTheme()),
        });
        termRef.current = term;
        fit = new FitAddon();
        fitRef.current = fit;
        term.loadAddon(fit);
        term.open(containerRef.current);
        try { fit.fit(); } catch { /* 容器尺寸为 0 时忽略 */ }

        // 数据桥：后端事件 → term；term 输入 → 后端
        term.onData((data: string) => {
          const s = sessionIdRef.current;
          if (s) void invoke('terminal_write', { sessionId: s, data }).catch(() => {});
        });
        term.onResize(({ cols, rows }: { cols: number; rows: number }) => {
          const s = sessionIdRef.current;
          if (s) void invoke('terminal_resize', { sessionId: s, cols, rows }).catch(() => {});
        });

        unlistens.push(
          await listen<{ session_id: string; data: string }>('terminal:data', (e) => {
            if (e.payload?.session_id === sessionIdRef.current) term?.write(e.payload.data);
          }),
        );
        unlistens.push(
          await listen<{ session_id: string }>('terminal:exit', (e) => {
            if (e.payload?.session_id === sessionIdRef.current) {
              term?.write(`\r\n\x1b[90m[${t('mind_inspector.code_terminal_exit')}]\x1b[0m\r\n`);
            }
          }),
        );

        term.focus();
        setReady(true);
      } catch (e) {
        if (!disposed) setError(String(e));
      }
    })();

    return () => {
      disposed = true;
      unlistens.forEach((fn) => fn());
      const s = sessionIdRef.current;
      if (s) void invoke('terminal_kill', { sessionId: s }).catch(() => {});
      sessionIdRef.current = null;
      termRef.current = null;
      try { term?.dispose(); } catch { /* ignore */ }
    };
    // 工作目录变化 → 重建终端会话
  }, [workingDirectory]);

  // 深浅色切换 → 重新应用 xterm 主题（监听 data-theme 属性 + 系统偏好）
  useEffect(() => {
    if (!ready) return;
    const apply = () => {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const t = termRef.current as any;
      if (!t) return;
      try {
        t.options.theme = buildTheme(isLightTheme());
      } catch { /* ignore */ }
    };
    let lastTheme = document.documentElement.getAttribute('data-theme');
    const observer = new MutationObserver(() => {
      const cur = document.documentElement.getAttribute('data-theme');
      if (cur !== lastTheme) {
        lastTheme = cur;
        apply();
      }
    });
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    const mq = window.matchMedia('(prefers-color-scheme: light)');
    const onChange = () => apply();
    mq.addEventListener('change', onChange);
    return () => {
      observer.disconnect();
      mq.removeEventListener('change', onChange);
    };
  }, [ready]);

  // 容器尺寸变化 → fit（100ms 节流）
  useEffect(() => {
    const el = containerRef.current;
    if (!el || !ready) return;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const ro = new ResizeObserver(() => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        try { fitRef.current?.fit(); } catch { /* ignore */ }
      }, 100);
    });
    ro.observe(el);
    return () => {
      ro.disconnect();
      if (timer) clearTimeout(timer);
    };
  }, [ready]);

  return (
    <div
      style={{
        position: 'relative',
        width: '100%',
        height: '100%',
        background: 'var(--panel-elevated)',
        borderRadius: RADIUS.md,
        border: `1px solid ${COLORS.borderLight}`,
        overflow: 'hidden',
      }}
    >
      {error ? (
        <div style={{ padding: SPACING.md, color: COLORS.danger, fontFamily: '-apple-system, "Segoe UI", "Microsoft YaHei", sans-serif', fontSize: 12 }}>
          {t('mind_inspector.code_terminal_fail', { error })}
        </div>
      ) : !ready ? (
        <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.sm, padding: SPACING.md, color: COLORS.textTertiary, fontFamily: '-apple-system, "Segoe UI", "Microsoft YaHei", sans-serif', fontSize: 12 }}>
          <Loader2 size={14} style={{ animation: 'code-spin 1s linear infinite' }} />
          {t('mind_inspector.code_terminal_start')}
        </div>
      ) : null}
      <div ref={containerRef} style={{ width: '100%', height: '100%', padding: '8px 6px', opacity: ready ? 1 : 0 }} />
    </div>
  );
};

export default TerminalPanelInner;
