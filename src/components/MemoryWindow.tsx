/**
 * 记忆管理窗口（精简外壳）
 *
 * 无独立标题栏——窗口拖拽区与最小化/关闭按钮已迁移到内部 <MindInspector /> 的封面条。
 * Tab 切换与页面渲染全部由内部 <MindInspector /> 负责。
 */

import React, { useEffect, useLayoutEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { changeLanguage } from '../i18n';
import MindInspector from './mind-inspector/MindInspector';
import { PET_REVEAL_EVENT, parsePetRect, playPetReveal, replayPetReveal, type PetRect } from '../utils/petReveal';
import { isWindowOnScreen } from '../utils/windowRaiser';

const MemoryWindow: React.FC = () => {
  const rootRef = useRef<HTMLDivElement>(null);
  /** 入场动画是否已启动（子窗口在 StrictMode 下 effect 会双执行，必须只播一次） */
  const revealStartedRef = useRef(false);
  /** 重播动画进行中标志：期间的重复通知直接丢弃，避免两段 transform 互相踩踏 */
  const replayBusyRef = useRef(false);

  // 入场动画：从桌宠矩形「长」到全屏。
  // 用 useLayoutEffect（首次绘制前同步执行）先把根节点藏起来——playPetReveal
  // 内部要 await 几次 Tauri 调用才能算出起手 transform，而 main.tsx 在渲染完成后
  // 两帧就会 show 窗口；这段空档若已绘制，窗口显形时会先闪一帧全屏大图。
  // 守卫由 playPetReveal 在起手时清掉。
  useLayoutEffect(() => {
    if (revealStartedRef.current) return;
    const root = rootRef.current;
    if (!root) return;
    const pet = parsePetRect(window.location.search);
    if (!pet) return; // 不是从桌宠打开的（窗口复用/直达）→ 不播动画，按常态显示
    revealStartedRef.current = true;
    root.style.opacity = '0';
    // 首次动画期间同样占用互斥位：这条路径一旦在开始算起手 transform，
    // 期间收到的重播通知必须丢弃，否则两段 transform 会互相踩踏
    replayBusyRef.current = true;
    void playPetReveal(root, pet).finally(() => {
      replayBusyRef.current = false;
    });
  }, []);

  // 入场动画：窗口已经开着时再次长按桌宠，桌宠侧把它提上来后发事件过来。
  // 此时既不能走 URL 参数那条路（navigate 会整页 reload，丢掉当前页签与输入），
  // 也不重建窗口（label 冲突），只能让本窗口原地再播一遍入场。
  //
  // 播哪一条要看此刻在不在屏上，而这个判断只有本窗口做得了（桌宠那边看不到它的
  // 最小化状态）。两条路径对「显形」的假设是相反的，不能混：
  //   在屏上   → replay：只能「收拢 → 展开」，先 hide 再 show 会闪、会抖 Z 序；
  //   不在屏上 → play  ：跟首开完全同一条路（先摆好首帧再显形）。
  // 最小化时若照旧走 replay，就得先把窗口还原才看得见内容，而那次还原本身就是一次呼出，
  // 紧接着的收拢展开是第二次——用户看到的就是「呼出了两次」。
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<PetRect>(PET_REVEAL_EVENT, (e) => {
          const root = rootRef.current;
          const pet = e.payload;
          if (!root || !pet || typeof pet.x !== 'number') return;
          if (replayBusyRef.current) return; // 正在播 → 直接忽略重复触发
          replayBusyRef.current = true;
          void (async () => {
            if (await isWindowOnScreen(getCurrentWindow())) {
              await replayPetReveal(root, pet);
            } else {
              await playPetReveal(root, pet);
            }
          })().finally(() => {
            replayBusyRef.current = false;
          });
        });
        if (cancelled) {
          unlisten();
          unlisten = undefined;
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
    <div
      ref={rootRef}
      className="codex-theme mind-memory-window"
      onContextMenu={(event) => event.preventDefault()}
    >
      {/* 内容区 */}
      <div style={{ flex: 1, minHeight: 0 }}>
        <MindInspector />
      </div>
    </div>
  );
};

export default MemoryWindow;
