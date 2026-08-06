/**
 * 记忆管理窗口（精简外壳）
 *
 * iOS 风格外壳：磨砂玻璃标题栏（窗口拖拽区 + 窗口控制按钮）
 * + 系统渐变背景 + 圆角窗口控制。
 * Tab 切换与页面渲染全部由内部 <MindInspector /> 负责。
 */

import React, { useCallback, useEffect } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import { changeLanguage } from '../i18n';
import MindInspector from './mind-inspector/MindInspector';

const MemoryWindow: React.FC = () => {
  const { t } = useTranslation();

  const closeWindow = useCallback(async () => {
    try {
      await getCurrentWindow().close();
    } catch {
      // ignore
    }
  }, []);

  const minimizeWindow = useCallback(async () => {
    try {
      await getCurrentWindow().minimize();
    } catch {
      // ignore
    }
  }, []);

  // 语言变更监听
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ language: string }>('config:language-changed', (e) => {
          if (e.payload?.language) void changeLanguage(e.payload.language);
        });
        if (cancelled) {
          unlisten();
          return;
        }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // 主题：读取 base.theme 配置设置根节点 data-theme，并监听实时变更
  useEffect(() => {
    const applyTheme = (theme: string | null | undefined) => {
      document.documentElement.setAttribute('data-theme', theme === 'light' || theme === 'dark' ? theme : 'system');
    };
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const theme = await invoke<string | null>('get_config', { key: 'base.theme' });
        if (!cancelled) applyTheme(theme);
        unlisten = await listen<{ theme: string }>('config:theme-changed', (e) => {
          applyTheme(e.payload?.theme);
        });
        if (cancelled) unlisten();
      } catch { /* ignore */ }
    })();
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  // 窗口控制按钮（iOS 风格：圆角胶囊，hover 时柔和过渡）
  const headerBtnBase: React.CSSProperties = {
    width: 28,
    height: 28,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    border: 'none',
    background: 'transparent',
    borderRadius: 8,
    cursor: 'pointer',
    transition: 'background 0.2s cubic-bezier(0.2, 0.8, 0.2, 1), transform 0.15s cubic-bezier(0.34, 1.56, 0.64, 1)',
  };

  return (
    <div
      className="scrapbook scrapbook-bg"
      style={{
        position: 'relative',
        display: 'flex',
        flexDirection: 'column',
        height: '100vh',
        color: 'var(--panel-text)',
        fontFamily:
          '"Caveat", "Ma Shan Zheng", "Dancing Script", "Hachi Maru Pop", -apple-system, BlinkMacSystemFont, "SF Pro Text", "SF Pro Display", "PingFang SC", "Microsoft YaHei", "Segoe UI", sans-serif',
        overflow: 'hidden',
        WebkitFontSmoothing: 'antialiased',
        textRendering: 'optimizeLegibility',
      }}
    >
      {/* 标题栏：纯窗口拖拽区 + 右侧窗口控制按钮（无标题文字） */}
      <div
        data-tauri-drag-region
        style={{
          position: 'relative',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'flex-end',
          padding: '8px 12px',
          borderBottom: '1.5px solid var(--panel-border)',
          flexShrink: 0,
          userSelect: 'none',
          background: 'var(--panel-bg-surface-elevated)',
          backdropFilter: 'blur(12px)',
          WebkitBackdropFilter: 'blur(12px)',
        }}
      >
        <div style={{ display: 'flex', gap: 4 }}>
          <button
            onClick={() => void minimizeWindow()}
            title={t('common.minimize')}
            style={headerBtnBase}
            onMouseEnter={(e) => {
              e.currentTarget.style.background = 'var(--panel-bg-hover)';
            }}
            onMouseLeave={(e) => (e.currentTarget.style.background = 'transparent')}
            onMouseDown={(e) => (e.currentTarget.style.transform = 'scale(0.92)')}
            onMouseUp={(e) => (e.currentTarget.style.transform = 'scale(1)')}
          >
            <svg width="11" height="11" viewBox="0 0 12 12" fill="none">
              <path
                d="M2.5 6H9.5"
                stroke="var(--panel-text)"
                strokeWidth="1.5"
                strokeLinecap="round"
              />
            </svg>
          </button>
          <button
            onClick={closeWindow}
            title={t('common.close')}
            style={headerBtnBase}
            onMouseEnter={(e) => {
              e.currentTarget.style.background = 'var(--panel-accent)';
              e.currentTarget.querySelector('path')?.setAttribute('stroke', 'var(--panel-bg)');
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.background = 'transparent';
              e.currentTarget.querySelector('path')?.setAttribute('stroke', 'var(--panel-text)');
            }}
            onMouseDown={(e) => (e.currentTarget.style.transform = 'scale(0.92)')}
            onMouseUp={(e) => (e.currentTarget.style.transform = 'scale(1)')}
          >
            <svg width="11" height="11" viewBox="0 0 12 12" fill="none">
              <path
                d="M3.5 3.5L8.5 8.5M8.5 3.5L3.5 8.5"
                stroke="var(--panel-text)"
                strokeWidth="1.5"
                strokeLinecap="round"
              />
            </svg>
          </button>
        </div>
      </div>

      {/* 内容区 */}
      <div style={{ flex: 1, minHeight: 0 }}>
        <MindInspector />
      </div>
    </div>
  );
};

export default MemoryWindow;
