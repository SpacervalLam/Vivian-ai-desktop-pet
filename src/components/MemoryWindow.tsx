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
import {
  PET_REVEAL_EVENT,
  emitPrewarmReady,
  parsePetRect,
  parsePrewarmSession,
  playPetReveal,
  replayPetReveal,
  type PetRect,
} from '../utils/petReveal';
import { isWindowOnScreen } from '../utils/windowRaiser';

const MemoryWindow: React.FC = () => {
  const rootRef = useRef<HTMLDivElement>(null);
  /** 入场动画是否已启动（子窗口在 StrictMode 下 effect 会双执行，必须只播一次） */
  const revealStartedRef = useRef(false);
  /** 重播动画进行中标志：期间的重复通知直接丢弃，避免两段 transform 互相踩踏 */
  const replayBusyRef = useRef(false);
  /** 本次是不是「预热窗口」（长按 0.1s 就建出来、等长按成立才显形的那种） */
  const prewarmSessionRef = useRef<number | null>(null);

  // 入场动画：从桌宠矩形「长」到全屏。
  // 用 useLayoutEffect（首次绘制前同步执行）先把根节点藏起来——playPetReveal
  // 内部要 await 几次 Tauri 调用才能算出起手 transform，而 main.tsx 在渲染完成后
  // 两帧就会 show 窗口；这段空档若已绘制，窗口显形时会先闪一帧全屏大图。
  // 守卫由 playPetReveal 在起手时清掉。
  //
  // 预热窗口走例外分支：桌宠只是提前把窗口建出来加载，长按成不成立还不知道，
  // 所以这里**只藏不播**，把「何时显形」整个交给随后的 pet:reveal（见下一个 effect）。
  // 窗口本身也不会自己冒出来：预热 URL 带了 hidden=1，main.tsx 的兜底 show 被跳过。
  //
  // 这一支刻意排在桌宠矩形判定之前：预热窗口的「就绪」只取决于监听有没有挂上，
  // 与能不能读到桌宠矩形无关。桌宠取不到自己的位置时（rect 缺失）仍然该收到回执，
  // 否则它只能干等 1.2s 超时——那段等待正是预热要消灭的东西。矩形缺失只影响
  // 入场动画的起点，显形本身照旧（见 playPetReveal 的退化分支）。
  useLayoutEffect(() => {
    if (revealStartedRef.current) return;
    const root = rootRef.current;
    if (!root) return;
    const prewarmSession = parsePrewarmSession(window.location.search);
    if (prewarmSession !== null) {
      revealStartedRef.current = true;
      // 首帧先藏起来：预热模式下这一藏要持续到长按成立为止
      root.style.opacity = '0';
      prewarmSessionRef.current = prewarmSession;
      return;
    }
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
  //
  // 预热窗口的「显形」也落在同一条事件上：它就是本窗口第一次上屏的信号，
  // 所以预热模式不另开分支——此刻窗口必然不在屏上，自然走 play。
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
      // 预热回执：监听确实挂上之后再发，否则桌宠那边可能在「事件无人接收」的
      // 窗口期里发出显形事件、白等一轮兜底。发晚了也无害——桌宠按会话号比对，
      // 它要么还在等（收到即显形），要么已经放行过（直接丢弃）。
      const prewarmSession = prewarmSessionRef.current;
      if (prewarmSession !== null) {
        prewarmSessionRef.current = null;
        await emitPrewarmReady(prewarmSession);
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // 兜底显形：极端情况下（pet:reveal 事件丢失 / 子窗口脚本异常 / 入场动画在
  // 真实 WebView 里没走到 finally / 兜底 show 比 opacity 守卫清除先到）根节点可能
  // 一直停在 opacity:0，窗口被 show 出来却整片透明——对外表现就是「打开了一个空窗口」。
  //
  // 这里兜底：挂载后一小段时间若根节点仍不可见，强制清掉 opacity 守卫并把窗口本身
  // 显示出来。正常路径下 playPetReveal 在 ~340ms 内就会清除守卫，这条兜底不会干扰
  // 入场动画；只有「显形链断了」时才兜底显形，杜绝永久空白窗口。
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const t = window.setTimeout(() => {
      if (root.style.opacity === '0') {
        root.style.opacity = '';
        void getCurrentWindow().show().catch(() => {});
      }
    }, 1200);
    return () => window.clearTimeout(t);
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
