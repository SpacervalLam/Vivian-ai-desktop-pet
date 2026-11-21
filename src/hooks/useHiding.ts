/**
 * 管理在场状态的隐藏动画，以及智能避让开启时的全屏贴边。
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import type { RefObject } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { currentMonitor, getCurrentWindow } from '@tauri-apps/api/window';
import type { ChibiPetCanvasHandle } from '../components/ChibiPetCanvas';
import { useAppStore } from '../stores/useAppStore';
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
export type HideReason = 'sleep';

type FullscreenDock = [epoch: number, x: number, y: number];

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
 * @param petRef 桌宠画布引用
 * @param modelReady 模型是否就绪
 * @param fullscreenDockEnabled 是否允许全屏贴边，独立于休息和离线隐藏
 * @param onPetHideRef 全屏贴边、退角落或离线时收起桌宠附属窗口的回调。
 *   依附于桌宠的窗口（气泡等）要在这里一起收起：它们各自的关闭计时跑在桌宠窗口自己的
 *   webview 里，桌宠一隐藏/被遮挡就会被 Chromium 降频甚至挂起，计时到点也没人执行。
 *   用 ref 传而不是直接传函数——本 hook 的 hide 闭包在 mount 时固定（依赖表只有 petRef），
 *   直接传会永远拿到首次渲染的旧闭包。
 */
export function useHiding(
  petRef: RefObject<ChibiPetCanvasHandle | null>,
  modelReady: boolean,
  fullscreenDockEnabled: boolean,
  onPetHideRef?: RefObject<(() => void) | null>,
  quietRef?: RefObject<boolean>,
): UseHidingResult {
  /** 当前是否处于隐藏状态（任一原因） */
  const isHiddenRef = useRef(false);
  /** 睡眠隐藏标记（与全屏独立） */
  const sleepHiddenRef = useRef(false);
  /** 离线隐藏标记（真正 hide_window，与 Rest 退到角落区分） */
  const offlineHiddenRef = useRef(false);
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
  const fullscreenDockEnabledRef = useRef(fullscreenDockEnabled);

  useEffect(() => {
    modelReadyRef.current = modelReady;
  }, [modelReady]);

  useEffect(() => {
    fullscreenDockEnabledRef.current = fullscreenDockEnabled;
  }, [fullscreenDockEnabled]);

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
    let fullscreenDock: FullscreenDock | null = null;
    let fullscreenCheck: Promise<void> | null = null;

    const restoreFullscreenDock = async () => {
      if (!fullscreenDock) return;
      const [epoch, x, y] = fullscreenDock;
      await invoke('restore_fullscreen_dock', { epoch, x, y });
      fullscreenDock = null;
    };

    /** 休息时退到角落。 */
    const doHide = async (reason: HideReason) => {
      if (quietRef?.current) return;
      if (isHiddenRef.current || inFlightRef.current) return;
      if (!modelReadyRef.current || !petRef.current) return;
      if (fullscreenCheck || fullscreenDock) {
        await fullscreenCheck;
        if (!sleepHiddenRef.current || offlineHiddenRef.current || cancelled) return;
        await restoreFullscreenDock();
        if (!sleepHiddenRef.current || offlineHiddenRef.current || cancelled) return;
      }
      inFlightRef.current = true;
      positioningCoordinator.fullscreenInFlight = true;
      // 桌宠要离场了：依附它的气泡一并收起（动画开始前就收，避免气泡被
      // onMoved 的重定位逻辑一路拖到屏幕角落）
      onPetHideRef?.current?.();
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
      if (quietRef?.current) return;
      if (!isHiddenRef.current || inFlightRef.current) return;
      const saved = savedStateRef.current;
      if (!saved) {
        isHiddenRef.current = false;
        setHideReason(null);
        positioningCoordinator.fullscreenHidden = false;
        setHiddenCorner(null);
        return;
      }
      inFlightRef.current = true;
      isHiddenRef.current = false;
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

    const updateFullscreenDock = async () => {
      if (quietRef?.current || inFlightRef.current || offlineHiddenRef.current ||
        sleepHiddenRef.current || !modelReadyRef.current || positioningCoordinator.dragInFlight) return;
      const fullscreen = fullscreenDockEnabledRef.current && await invoke<boolean>('is_foreground_fullscreen');
      if (cancelled || quietRef?.current || inFlightRef.current || offlineHiddenRef.current ||
        sleepHiddenRef.current || positioningCoordinator.dragInFlight) return;
      if (!fullscreen || !fullscreenDockEnabledRef.current) {
        await restoreFullscreenDock();
        return;
      }
      if (fullscreenDock || useAppStore.getState().dockState !== 'Off' ||
        positioningCoordinator.fleeInFlight ||
        positioningCoordinator.ambientMoveInFlight) return;

      if (positioningCoordinator.smartPositioningInFlight) {
        positioningCoordinator.abortSmartMove?.();
        return;
      }
      fullscreenDock = await invoke<FullscreenDock | null>('dock_for_fullscreen');
      if (fullscreenDock) onPetHideRef?.current?.();
    };

    const checkFullscreen = () => {
      if (fullscreenCheck) return;
      fullscreenCheck = updateFullscreenDock()
        .catch(error => console.warn('[useHiding] 全屏贴边失败:', error))
        .finally(() => { fullscreenCheck = null; });
    };
    const id = window.setInterval(checkFullscreen, POLL_INTERVAL_MS);
    checkFullscreen();

    // Offline 下坠离场动画：从当前位置加速下落到屏幕下方，然后隐藏窗口
    const doOfflineHide = async () => {
      if (offlineInFlightRef.current) return;
      if (!modelReadyRef.current || !petRef.current) return;
      // 先结束全屏贴边，再记录离线前的位置。
      if (fullscreenCheck || fullscreenDock) {
        await fullscreenCheck;
        if (!offlineHiddenRef.current || cancelled) return;
        await restoreFullscreenDock().catch(error => console.warn('[useHiding] 退出全屏贴边失败:', error));
        if (!offlineHiddenRef.current || cancelled) return;
      }
      offlineInFlightRef.current = true;
      positioningCoordinator.fullscreenInFlight = true;
      // 同步置 inFlightRef，避免全屏轮询在离线动画期间移动窗口。
      inFlightRef.current = true;
      // 桌宠要离场了：依附它的气泡一并收起（下坠动画开始前就收）
      onPetHideRef?.current?.();
      // 取消上一个尚未完成的动画，避免位置竞争
      if (cancelPosAnim) { cancelPosAnim(); cancelPosAnim = null; }

      const win = getCurrentWindow();
      const pos = await win.outerPosition();
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
      if (quietRef?.current) return;
      if (offlineInFlightRef.current) return;
      offlineInFlightRef.current = true;
      positioningCoordinator.fullscreenInFlight = true;
      // 同步置 inFlightRef，避免全屏轮询在恢复动画期间移动窗口。
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
  }, [petRef]);

  /** 用户点击 PeekButton 时调用 —— 立即触发 restore，无需等待 1.5s 轮询 */
  const requestRestore = useCallback(() => {
    sleepHiddenRef.current = false;
    void restoreFnRef.current?.();
  }, []);

  /** 触发休息隐藏（Presence 切到 Rest/Offline） */
  const hideForSleep = useCallback(() => {
    if (sleepHiddenRef.current) return;
    sleepHiddenRef.current = true;
    void hideFnRef.current?.('sleep');
  }, []);

  /** 退出睡眠隐藏 */
  const restoreFromSleep = useCallback(() => {
    if (!sleepHiddenRef.current) return;
    sleepHiddenRef.current = false;
    void restoreFnRef.current?.();
  }, []);

  /** 真正隐藏窗口到托盘（Presence 切到 Offline）
   *
   * 与 Rest 不同：Offline 是"完全失联"，窗口应当完全不可见，
   * 只能通过托盘菜单或快捷键唤回。
   * 带下坠动画：窗口从当前位置加速下落到屏幕下方后隐藏。
   * 若窗口已因睡眠隐藏，仅标记，不重复动画。 */
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
