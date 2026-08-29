// 智能避让 hook:检测纯色区域移动桌宠避免遮挡

import { useEffect, useRef } from 'react';
import type { RefObject } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { Live2DCanvasHandle } from '../components/Live2DCanvas';
import { positioningCoordinator } from './positioningCoordinator';
import { getCharacterId } from '../characterContext';

const POLL_INTERVAL_BASE_MS = 2_500;
const POLL_INTERVAL_MAX_MS = 20_000;
const POLL_INTERVAL_STEP_MS = 2_500;
const MOVE_DURATION_MS = 700;
const POSITION_STEPS = 10;
const FOREGROUND_DEBOUNCE_MS = 700;
const STARTUP_JITTER_MAX_MS = 1_200;
const CHARACTER_OFFSET_MS: Record<string, number> = {
  vivian: 0,
  nana: 800,
};
const MIN_MOVE_DISTANCE = 24;

interface SafeRegion {
  x: number;
  y: number;
  width: number;
  height: number;
}

interface FindSafePositionResult {
  unchanged: boolean;
  region: SafeRegion | null;
}

function easeInOutCubic(t: number): number {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

export function useSmartPositioning(
  _live2dRef: RefObject<Live2DCanvasHandle | null>,
  modelReady: boolean,
  enabled: boolean,
): void {
  const inFlightRef = useRef(false);
  const modelReadyRef = useRef(modelReady);
  const enabledRef = useRef(enabled);
  const currentIntervalRef = useRef(POLL_INTERVAL_BASE_MS);
  const timerRef = useRef<number | null>(null);
  const focusCheckTimerRef = useRef<number | null>(null);
  const lastForegroundSwitchRef = useRef(0);
  const focusedRef = useRef(false);

  useEffect(() => {
    modelReadyRef.current = modelReady;
  }, [modelReady]);

  useEffect(() => {
    enabledRef.current = enabled;
  }, [enabled]);

  useEffect(() => {
    if (!enabled) {
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      if (focusCheckTimerRef.current !== null) {
        window.clearTimeout(focusCheckTimerRef.current);
        focusCheckTimerRef.current = null;
      }
      return;
    }
    let cancelled = false;

    const charId = getCharacterId();
    const charOffset = charId ? (CHARACTER_OFFSET_MS[charId] ?? 400) : 400;

    const animatePosition = async (
      targetX: number,
      targetY: number,
      startX: number,
      startY: number,
    ) => {
      const stepMs = MOVE_DURATION_MS / POSITION_STEPS;
      for (let i = 1; i <= POSITION_STEPS; i++) {
        if (cancelled) return;
        const t = i / POSITION_STEPS;
        const eased = easeInOutCubic(t);
        const x = Math.round(startX + (targetX - startX) * eased);
        const y = Math.round(startY + (targetY - startY) * eased);
        void invoke('set_window_position', { x, y });
        if (i < POSITION_STEPS) {
          await new Promise((r) => setTimeout(r, stepMs));
        }
      }
    };

    const runCheck = async (force = false) => {
      if (inFlightRef.current) return;
      if (cancelled || !enabledRef.current || !modelReadyRef.current) return;
      if (focusedRef.current) return;
      if (
        !force &&
        (positioningCoordinator.fullscreenInFlight ||
          positioningCoordinator.fullscreenHidden)
      ) {
        return;
      }

      inFlightRef.current = true;
      try {
        const win = getCurrentWindow();
        const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
        if (cancelled) return;

        const result = await invoke<FindSafePositionResult>('find_safe_position', {
          petX: pos.x,
          petY: pos.y,
          petW: size.width,
          petH: size.height,
          force,
        });
        if (cancelled) return;

        if (result.unchanged) {
          currentIntervalRef.current = Math.min(
            currentIntervalRef.current + POLL_INTERVAL_STEP_MS,
            POLL_INTERVAL_MAX_MS,
          );
        } else {
          currentIntervalRef.current = POLL_INTERVAL_BASE_MS;
        }

        const region = result.region;
        if (!region) return;

        const targetX = Math.round(region.x + (region.width - size.width) / 2);
        const targetY = Math.round(region.y + (region.height - size.height) / 2);

        const dx = Math.abs(targetX - pos.x);
        const dy = Math.abs(targetY - pos.y);
        if (dx < MIN_MOVE_DISTANCE && dy < MIN_MOVE_DISTANCE) return;

        await animatePosition(targetX, targetY, pos.x, pos.y);
      } catch {
      } finally {
        inFlightRef.current = false;
      }
    };

    const scheduleNext = () => {
      if (cancelled) return;
      timerRef.current = window.setTimeout(() => {
        void runCheck(false);
        scheduleNext();
      }, currentIntervalRef.current);
    };

    const handleBlur = () => {
      if (focusCheckTimerRef.current !== null) {
        window.clearTimeout(focusCheckTimerRef.current);
      }
      const now = Date.now();
      if (now - lastForegroundSwitchRef.current < FOREGROUND_DEBOUNCE_MS) return;
      lastForegroundSwitchRef.current = now;
      focusCheckTimerRef.current = window.setTimeout(() => {
        focusCheckTimerRef.current = null;
        void runCheck(true);
      }, 500);
    };

    let unlistenFocus: (() => void) | undefined;
    void (async () => {
      const win = getCurrentWindow();
      const unlisten = await win.onFocusChanged(({ payload: focused }) => {
        if (cancelled) return; // 已卸载则不再处理
        focusedRef.current = focused;
        if (!focused) {
          handleBlur();
        } else {
          if (focusCheckTimerRef.current !== null) {
            window.clearTimeout(focusCheckTimerRef.current);
            focusCheckTimerRef.current = null;
          }
          currentIntervalRef.current = POLL_INTERVAL_BASE_MS;
        }
      });
      if (cancelled) {
        // 组件在 listen resolve 前已卸载，立即清理刚注册的监听器，避免泄漏
        try {
          void Promise.resolve(unlisten()).catch(() => {});
        } catch {
          /* ignore */
        }
      } else {
        unlistenFocus = unlisten;
      }
    })();

    positioningCoordinator.triggerSmartCheck = () => {
      void runCheck(true);
    };

    const startupDelay = charOffset + Math.floor(Math.random() * STARTUP_JITTER_MAX_MS);
    timerRef.current = window.setTimeout(() => {
      void runCheck(true);
      scheduleNext();
    }, startupDelay);

    return () => {
      cancelled = true;
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      if (focusCheckTimerRef.current !== null) {
        window.clearTimeout(focusCheckTimerRef.current);
        focusCheckTimerRef.current = null;
      }
      unlistenFocus?.();
      positioningCoordinator.triggerSmartCheck = null;
    };
  }, [enabled]);
}
