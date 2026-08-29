/**
 * 统一隐藏管理 - 桌宠退到屏幕角落侧边隐藏。
 *
 * 两种触发来源，任一触发都隐藏到角落，全部退出后才恢复：
 * 1. 全屏应用聚焦（轮询 is_foreground_fullscreen）—— 归入智能避让开关控制
 * 2. 在场状态切到 Rest/Offline（休息/离线）—— 独立于智能避让
 *
 * 退出隐藏的方式：
 * - 全屏退出（仅全屏触发时）
 * - 用户点击 PeekButton（强制召回，标记本次全屏期间不再自动隐藏）
 * - Ctrl+Shift+V 快捷键（强制退出隐藏 + 恢复在线）
 *
 * 判定由后端 `is_foreground_fullscreen` 命令完成（Win32 API 比对前台窗口
 * 矩形与显示器矩形）。前端负责轮询 + 动画驱动窗口位移。
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type { RefObject } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { currentMonitor, getCurrentWindow } from '@tauri-apps/api/window';
import type { Live2DCanvasHandle } from '../components/Live2DCanvas';
import { positioningCoordinator } from './positioningCoordinator';

/** 轮询间隔（毫秒） */
const POLL_INTERVAL_MS = 1500;
/** 隐藏时窗口在角落露出的像素数 */
const HIDDEN_PEEK_PIXELS = 48;
/** 过渡动画时长（毫秒） */
const TRANSITION_DURATION_MS = 650;
/** 窗口位移分步数（避免高频 IPC 拥塞） */
const POSITION_STEPS = 14;

/** 屏幕角落标识 —— 同时表示桌宠隐藏到的屏幕角落 */
export type Corner = 'tl' | 'tr' | 'bl' | 'br';

/** 隐藏原因 */
export type HideReason = 'fullscreen' | 'sleep';

/** Offline 下坠/上升动画时长（毫秒） */
const OFFLINE_TRANSITION_DURATION_MS = 800;
/** Offline 动画分步数 */
const OFFLINE_POSITION_STEPS = 18;

interface SavedWindowState {
  x: number;
  y: number;
}

function easeInOutCubic(t: number): number {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

function easeInQuad(t: number): number {
  return t * t;
}

function easeOutBack(t: number): number {
  const c1 = 1.70158;
  const c3 = c1 + 1;
  return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2);
}

/**
 * 计算窗口应退到的角落目标坐标（物理像素）与角落标识。
 * 根据窗口中心点相对屏幕中心的位置，选择最近的角落。
 */
function computeCornerTarget(
  winX: number,
  winY: number,
  winW: number,
  winH: number,
  screenW: number,
  screenH: number,
): { x: number; y: number; corner: Corner } {
  const centerX = winX + winW / 2;
  const centerY = winY + winH / 2;
  const toRight = centerX > screenW / 2;
  const toBottom = centerY > screenH / 2;

  if (toRight && toBottom) {
    return { x: screenW - HIDDEN_PEEK_PIXELS, y: screenH - HIDDEN_PEEK_PIXELS, corner: 'br' };
  }
  if (!toRight && toBottom) {
    return { x: -winW + HIDDEN_PEEK_PIXELS, y: screenH - HIDDEN_PEEK_PIXELS, corner: 'bl' };
  }
  if (toRight && !toBottom) {
    return { x: screenW - HIDDEN_PEEK_PIXELS, y: -winH + HIDDEN_PEEK_PIXELS, corner: 'tr' };
  }
  return { x: -winW + HIDDEN_PEEK_PIXELS, y: -winH + HIDDEN_PEEK_PIXELS, corner: 'tl' };
}

export interface UseHidingResult {
  /** 当前桌宠隐藏到的屏幕角落；null 表示未隐藏 */
  hiddenCorner: Corner | null;
  /** 当前隐藏原因；null 表示未隐藏 */
  hideReason: HideReason | null;
  /** 用户手动召回 —— 立即触发 restore，无需等待 1.5s 轮询 */
  requestRestore: () => void;
  /** 触发休息隐藏（Presence 切到 Rest/Offline） */
  hideForSleep: () => void;
  /** 退出睡眠隐藏 */
  restoreFromSleep: () => void;
  /** 真正隐藏窗口到托盘（Presence 切到 Offline） */
  hideForOffline: () => Promise<void>;
  /** 从 Offline 恢复（show_window） */
  restoreFromOffline: () => Promise<void>;
}

/**
 * 统一隐藏管理 hook。
 *
 * @param live2dRef Live2D 画布引用
 * @param modelReady 模型是否就绪
 * @param fullscreenHideEnabled 智能避让总开关；为 false 时全屏应用触发角落隐藏的功能不生效
 *   （sleep/offline 隐藏独立于此开关，仍按在场状态切换）
 */
export function useHiding(
  live2dRef: RefObject<Live2DCanvasHandle | null>,
  modelReady: boolean,
  fullscreenHideEnabled: boolean,
): UseHidingResult {
  /** 当前是否处于隐藏状态（任一原因） */
  const isHiddenRef = useRef(false);
  /** 当前隐藏原因（fullscreen / sleep / null） */
  const hideReasonRef = useRef<HideReason | null>(null);
  /** 睡眠隐藏标记（与全屏独立） */
  const sleepHiddenRef = useRef(false);
  /** 离线隐藏标记（真正 hide_window，与 Rest 退到角落区分） */
  const offlineHiddenRef = useRef(false);
  /** 全屏隐藏标记 */
  const fullscreenHiddenRef = useRef(false);
  const savedStateRef = useRef<SavedWindowState | null>(null);
  /** 锁定整个 hide/restore 周期，防止动画期间被新一轮 check 打断 */
  const inFlightRef = useRef(false);
  /** 跟踪 modelReady，避免在 effect 依赖中引入频繁重跑 */
  const modelReadyRef = useRef(modelReady);
  /** 当前隐藏到的屏幕角落（驱动 PeekButton 渲染） */
  const [hiddenCorner, setHiddenCorner] = useState<Corner | null>(null);
  /** 当前隐藏原因（驱动 UI 行为，如睡眠时显示 ZZZ） */
  const [hideReason, setHideReason] = useState<HideReason | null>(null);
  /** doHide / doRestore 函数引用（让外部 API 可以调用最新的闭包） */
  const hideFnRef = useRef<((reason: HideReason) => Promise<void>) | null>(null);
  const restoreFnRef = useRef<(() => Promise<void>) | null>(null);
  /** Offline 下坠/上升动画函数引用 */
  const offlineHideFnRef = useRef<(() => Promise<void>) | null>(null);
  const offlineRestoreFnRef = useRef<(() => Promise<void>) | null>(null);
  /** Offline 前保存的窗口位置（供恢复时使用） */
  const offlineSavedPosRef = useRef<{ x: number; y: number } | null>(null);
  /** Offline 动画进行中标记，防止重复触发 */
  const offlineInFlightRef = useRef(false);
  /**
   * 用户主动召回标记 —— 用户点击 PeekButton 召回后置 true，
   * 本次全屏期间不再自动 hide；检测到退出全屏时清除。
   * 区分"用户主动召回"与"全屏退出导致的自动 restore"。
   * 注意：睡眠隐藏不受此标记影响 —— 睡眠只能通过显式唤醒退出。
   */
  const userRecalledRef = useRef(false);
  /** 智能避让总开关镜像 —— 关闭时全屏应用触发角落隐藏的功能不生效 */
  const fullscreenHideEnabledRef = useRef(fullscreenHideEnabled);

  useEffect(() => {
    modelReadyRef.current = modelReady;
  }, [modelReady]);

  useEffect(() => {
    fullscreenHideEnabledRef.current = fullscreenHideEnabled;
  }, [fullscreenHideEnabled]);

  /** 分步动画窗口位置（限频 IPC）。返回 cancel 函数 */
  const animatePosition = (
    targetX: number,
    targetY: number,
    duration: number,
    startPos: { x: number; y: number },
    easing: (t: number) => number = easeInOutCubic,
    steps: number = POSITION_STEPS,
  ) => {
    let cancelled = false;
    const stepMs = duration / steps;
    const run = async () => {
      for (let i = 1; i <= steps; i++) {
        if (cancelled) return;
        const t = i / steps;
        const eased = easing(t);
        const x = Math.round(startPos.x + (targetX - startPos.x) * eased);
        const y = Math.round(startPos.y + (targetY - startPos.y) * eased);
        void invoke('set_window_position', { x, y });
        if (i < steps) {
          await new Promise((r) => setTimeout(r, stepMs));
        }
      }
    };
    void run();
    return () => {
      cancelled = true;
    };
  };

  useEffect(() => {
    let cancelled = false;
    let cancelPosAnim: (() => void) | null = null;
    /** 上次 is_foreground_fullscreen 返回值，仅状态变化时输出日志 */
    const lastFsRef = { current: false };
    /** 上次日志消息，避免重复刷屏 */
    const lastLogRef = { current: '' };

    /** 实际执行隐藏动画（全屏/睡眠共用） */
    const doHide = async (reason: HideReason) => {
      if (isHiddenRef.current || inFlightRef.current) return;
      if (!modelReadyRef.current || !live2dRef.current) return;
      inFlightRef.current = true;
      positioningCoordinator.fullscreenInFlight = true;
      // 取消上一个尚未完成的动画，避免位置竞争
      if (cancelPosAnim) { cancelPosAnim(); cancelPosAnim = null; }
      const win = getCurrentWindow();
      const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
      const monitor = await currentMonitor();
      if (!monitor || cancelled) {
        inFlightRef.current = false;
        positioningCoordinator.fullscreenInFlight = false;
        return;
      }
      const screenW = monitor.size.width;
      const screenH = monitor.size.height;
      const target = computeCornerTarget(
        pos.x,
        pos.y,
        size.width,
        size.height,
        screenW,
        screenH,
      );
      savedStateRef.current = { x: pos.x, y: pos.y };
      isHiddenRef.current = true;
      hideReasonRef.current = reason;
      setHideReason(reason);
      // 通知协调器：隐藏周期开始，smart positioning 应完全跳过
      positioningCoordinator.fullscreenHidden = true;

      cancelPosAnim = animatePosition(target.x, target.y, TRANSITION_DURATION_MS, {
        x: pos.x,
        y: pos.y,
      });
      await new Promise((r) => setTimeout(r, TRANSITION_DURATION_MS + 50));
      inFlightRef.current = false;
      positioningCoordinator.fullscreenInFlight = false;
      // 动画完成后才暴露角落给 PeekButton —— 避免按钮跟随窗口动画从屏幕中心漂移到角落
      setHiddenCorner(target.corner);
    };

    /** 实际执行恢复动画 */
    const doRestore = async () => {
      if (!isHiddenRef.current || inFlightRef.current) return;
      const saved = savedStateRef.current;
      if (!saved) {
        isHiddenRef.current = false;
        hideReasonRef.current = null;
        setHideReason(null);
        positioningCoordinator.fullscreenHidden = false;
        setHiddenCorner(null);
        return;
      }
      inFlightRef.current = true;
      isHiddenRef.current = false;
      hideReasonRef.current = null;
      setHideReason(null);
      // restore 开始即隐藏 PeekButton，避免按钮跟随窗口动画漂移
      setHiddenCorner(null);
      // 通知协调器：restore 周期开始，smart positioning 应完全跳过
      positioningCoordinator.fullscreenInFlight = true;
      // 取消上一个尚未完成的动画，避免位置竞争
      if (cancelPosAnim) { cancelPosAnim(); cancelPosAnim = null; }

      const startPos = await getCurrentWindow()
        .outerPosition()
        .catch(() => ({ x: saved.x, y: saved.y }));
      cancelPosAnim = animatePosition(saved.x, saved.y, TRANSITION_DURATION_MS, startPos);
      await new Promise((r) => setTimeout(r, TRANSITION_DURATION_MS + 50));
      savedStateRef.current = null;
      inFlightRef.current = false;
      positioningCoordinator.fullscreenInFlight = false;
      positioningCoordinator.fullscreenHidden = false;
      // restore 完成：立即触发一次强制屏幕捕获，把桌宠移动到当前屏幕最纯色位置
      positioningCoordinator.triggerSmartCheck?.();
    };

    /** 检查是否应该退出隐藏（仅全屏触发时自动退出；睡眠需显式唤醒） */
    const checkFullscreen = async () => {
      const label = getCurrentWindow().label;
      // 仅在状态变化时打日志，避免 1.5s 轮询刷屏
      const logOnce = (msg: string) => {
        const key = `${label}:${msg}`;
        if (lastLogRef.current === key) return;
        lastLogRef.current = key;
        void invoke('debug_log', { msg, label });
      };
      if (cancelled) { logOnce('checkFullscreen skip: cancelled'); return; }
      if (inFlightRef.current) { logOnce('checkFullscreen skip: inFlight'); return; }
      // Offline 上升/下坠动画进行中时跳过：避免与全屏检查的 doHide/doRestore 并发 set_window_position，
      // 导致窗口停留在屏幕外或错误位置
      if (offlineInFlightRef.current) { logOnce('checkFullscreen skip: offlineInFlight'); return; }
      // 窗口处于 Offline 隐藏态时跳过全屏检查：窗口已 hide，无需再触发角落隐藏
      if (offlineHiddenRef.current) { logOnce('checkFullscreen skip: offlineHidden'); return; }
      if (!modelReadyRef.current) { logOnce('checkFullscreen skip: !modelReady'); return; }
      // 睡眠隐藏不参与全屏轮询的自动恢复
      if (sleepHiddenRef.current) { logOnce('checkFullscreen skip: sleepHidden'); return; }
      // 智能避让关闭时，全屏应用触发角落隐藏的功能不生效；
      // 若此前已因全屏隐藏，立即恢复到原位
      if (!fullscreenHideEnabledRef.current) {
        if (fullscreenHiddenRef.current) {
          logOnce('trigger doRestore (smart positioning disabled)');
          fullscreenHiddenRef.current = false;
          await doRestore();
        } else {
          logOnce('checkFullscreen skip: smart positioning disabled');
        }
        return;
      }
      logOnce('checkFullscreen running');
      try {
        const fs = await invoke<boolean>('is_foreground_fullscreen');
        if (cancelled) return;
        // 仅在 fs 状态变化时输出，避免 1.5s 轮询刷屏
        if (fs !== lastFsRef.current) {
          lastFsRef.current = fs;
          void invoke('debug_log', { msg: `is_foreground_fullscreen=${fs}`, label });
        }
        if (fs && !fullscreenHiddenRef.current) {
          // 用户已主动召回，本次全屏期间不再自动隐藏
          if (userRecalledRef.current) { logOnce('skip hide: userRecalled'); return; }
          logOnce('trigger doHide(fullscreen)');
          fullscreenHiddenRef.current = true;
          await doHide('fullscreen');
        } else if (!fs && fullscreenHiddenRef.current) {
          logOnce('trigger doRestore');
          fullscreenHiddenRef.current = false;
          await doRestore();
        }
        // 退出全屏时清除用户召回标记，下次进入全屏可正常自动隐藏
        if (!fs && userRecalledRef.current) {
          userRecalledRef.current = false;
        }
      } catch (err) {
        void invoke('debug_log', { msg: `is_foreground_fullscreen error: ${String(err)}`, label });
      }
    };

    const id = window.setInterval(checkFullscreen, POLL_INTERVAL_MS);
    void checkFullscreen();

    // Offline 下坠离场动画：从当前位置加速下落到屏幕下方，然后隐藏窗口
    const doOfflineHide = async () => {
      if (offlineInFlightRef.current) return;
      if (!modelReadyRef.current || !live2dRef.current) return;
      offlineInFlightRef.current = true;
      positioningCoordinator.fullscreenInFlight = true;
      // 同步置 inFlightRef，防止全屏轮询触发 doHide/doRestore 与本动画并发 set_window_position
      inFlightRef.current = true;
      // 取消上一个尚未完成的动画，避免位置竞争
      if (cancelPosAnim) { cancelPosAnim(); cancelPosAnim = null; }

      const win = getCurrentWindow();
      const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
      const monitor = await currentMonitor();
      if (!monitor) {
        offlineInFlightRef.current = false;
        inFlightRef.current = false;
        positioningCoordinator.fullscreenInFlight = false;
        return;
      }

      const screenH = monitor.size.height;
      const targetY = screenH + 20;
      offlineSavedPosRef.current = { x: pos.x, y: pos.y };

      cancelPosAnim = animatePosition(
        pos.x,
        targetY,
        OFFLINE_TRANSITION_DURATION_MS,
        { x: pos.x, y: pos.y },
        easeInQuad,
        OFFLINE_POSITION_STEPS,
      );
      await new Promise((r) => setTimeout(r, OFFLINE_TRANSITION_DURATION_MS + 50));

      try {
        await invoke('hide_window');
      } catch (err) {
        console.warn('[useHiding] hide_window 失败:', err);
      }

      offlineInFlightRef.current = false;
      inFlightRef.current = false;
      positioningCoordinator.fullscreenInFlight = false;
      cancelPosAnim = null;
    };

    // Offline 上升入场动画：先把窗口移到屏幕下方并显示，然后上升到原位
    const doOfflineRestore = async () => {
      if (offlineInFlightRef.current) return;
      offlineInFlightRef.current = true;
      positioningCoordinator.fullscreenInFlight = true;
      // 同步置 inFlightRef，防止全屏轮询触发 doHide/doRestore 与本动画并发 set_window_position
      inFlightRef.current = true;
      // 取消上一个尚未完成的动画，避免位置竞争
      if (cancelPosAnim) { cancelPosAnim(); cancelPosAnim = null; }

      const win = getCurrentWindow();
      const savedPos = offlineSavedPosRef.current;

      let startX: number;
      let startY: number;
      let targetX: number;
      let targetY: number;

      try {
        const [size, monitor] = await Promise.all([win.outerSize(), currentMonitor()]);
        const screenH = monitor?.size.height ?? 800;
        const screenW = monitor?.size.width ?? 1200;

        if (savedPos) {
          targetX = savedPos.x;
          targetY = savedPos.y;
        } else {
          targetX = Math.round(screenW / 2 - size.width / 2);
          targetY = Math.round(screenH / 2 - size.height / 2);
        }
        startX = targetX;
        startY = screenH + 20;

        await invoke('set_window_position', { x: startX, y: startY });
        await invoke('show_window');
      } catch (err) {
        console.warn('[useHiding] show_window 前置准备失败:', err);
        offlineInFlightRef.current = false;
        inFlightRef.current = false;
        positioningCoordinator.fullscreenInFlight = false;
        return;
      }

      await new Promise((r) => setTimeout(r, 30));

      cancelPosAnim = animatePosition(
        targetX,
        targetY,
        OFFLINE_TRANSITION_DURATION_MS,
        { x: startX, y: startY },
        easeOutBack,
        OFFLINE_POSITION_STEPS,
      );
      await new Promise((r) => setTimeout(r, OFFLINE_TRANSITION_DURATION_MS + 50));

      offlineSavedPosRef.current = null;
      offlineInFlightRef.current = false;
      inFlightRef.current = false;
      positioningCoordinator.fullscreenInFlight = false;
      cancelPosAnim = null;
      positioningCoordinator.triggerSmartCheck?.();
    };

    // 把 doHide / doRestore / Offline 动画函数暴露给 hook 调用方
    hideFnRef.current = doHide;
    restoreFnRef.current = doRestore;
    offlineHideFnRef.current = doOfflineHide;
    offlineRestoreFnRef.current = doOfflineRestore;

    return () => {
      cancelled = true;
      window.clearInterval(id);
      if (cancelPosAnim) cancelPosAnim();
      hideFnRef.current = null;
      restoreFnRef.current = null;
      offlineHideFnRef.current = null;
      offlineRestoreFnRef.current = null;
      // 重置协调器状态，避免卸载后 smart positioning 永久被跳过
      positioningCoordinator.fullscreenHidden = false;
      positioningCoordinator.fullscreenInFlight = false;
    };
  }, [live2dRef]);

  /** 用户点击 PeekButton 时调用 —— 立即触发 restore，无需等待 1.5s 轮询 */
  const requestRestore = useCallback(() => {
    // 标记用户主动召回：本次全屏期间不再自动隐藏
    userRecalledRef.current = true;
    fullscreenHiddenRef.current = false;
    sleepHiddenRef.current = false;
    void restoreFnRef.current?.();
  }, []);

  /** 触发休息隐藏（Presence 切到 Rest/Offline） */
  const hideForSleep = useCallback(() => {
    if (sleepHiddenRef.current) return;
    sleepHiddenRef.current = true;
    // 如果已因全屏隐藏，无需重复动画
    if (isHiddenRef.current) {
      hideReasonRef.current = 'sleep';
      setHideReason('sleep');
      return;
    }
    void hideFnRef.current?.('sleep');
  }, []);

  /** 退出睡眠隐藏 */
  const restoreFromSleep = useCallback(() => {
    if (!sleepHiddenRef.current) return;
    sleepHiddenRef.current = false;
    // 如果全屏仍然触发，保持隐藏状态（原因切回全屏）
    if (fullscreenHiddenRef.current) {
      hideReasonRef.current = 'fullscreen';
      setHideReason('fullscreen');
      return;
    }
    void restoreFnRef.current?.();
  }, []);

  /** 真正隐藏窗口到托盘（Presence 切到 Offline）
   *
   * 与 Rest 不同：Offline 是"完全失联"，窗口应当完全不可见，
   * 只能通过托盘菜单或快捷键唤回。
   * 带下坠动画：窗口从当前位置加速下落到屏幕下方后隐藏。
   * 若窗口已因全屏/睡眠隐藏，仅标记，不重复动画。 */
  const hideForOffline = useCallback(async () => {
    if (offlineHiddenRef.current) return;
    offlineHiddenRef.current = true;
    if (isHiddenRef.current) return;
    if (offlineHideFnRef.current) {
      await offlineHideFnRef.current();
    } else {
      try {
        await invoke('hide_window');
      } catch (err) {
        console.warn('[useHiding] hide_window 失败:', err);
      }
    }
  }, []);

  /** 从 Offline 恢复（Presence 从 Offline 切到其他状态）
   *
   * 带上升动画：窗口从屏幕下方上升回弹到原位。
   * 动画结束后触发智能避让强制检查。 */
  const restoreFromOffline = useCallback(async () => {
    if (!offlineHiddenRef.current) return;
    offlineHiddenRef.current = false;
    if (offlineRestoreFnRef.current) {
      await offlineRestoreFnRef.current();
    } else {
      try {
        await invoke('show_window');
      } catch (err) {
        console.warn('[useHiding] show_window 失败:', err);
      }
      positioningCoordinator.triggerSmartCheck?.();
    }
  }, []);

  return {
    hiddenCorner,
    hideReason,
    requestRestore,
    hideForSleep,
    restoreFromSleep,
    hideForOffline,
    restoreFromOffline,
  };
}
