/**
 * 记忆管理窗口（精简外壳）
 *
 * 无独立标题栏——窗口拖拽区与最小化/关闭按钮已迁移到内部 <MindInspector /> 的封面条。
 * Tab 切换与页面渲染全部由内部 <MindInspector /> 负责。
 */

import React, { useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { changeLanguage } from '../i18n';
import MindInspector from './mind-inspector/MindInspector';

const MemoryWindow: React.FC = () => {
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

  return (
    <div className="codex-root mind-memory-window">
      {/* 内容区 */}
      <div style={{ flex: 1, minHeight: 0 }}>
        <MindInspector />
      </div>
    </div>
  );
};

export default MemoryWindow;
