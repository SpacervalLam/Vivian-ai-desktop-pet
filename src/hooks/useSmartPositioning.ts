// 智能避让 hook:检测纯色区域移动桌宠避免遮挡

import { useEffect, useRef } from 'react';
import type { RefObject } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { ChibiPetCanvasHandle } from '../components/ChibiPetCanvas';
import { positioningCoordinator } from './positioningCoordinator';
import { getCharacterId } from '../characterContext';
import { planSmartMove } from '../chibi/walkPlan';
import { runSlide } from '../chibi/slideTrack';

const POLL_INTERVAL_BASE_MS = 2_500;
const POLL_INTERVAL_MAX_MS = 20_000;
const POLL_INTERVAL_STEP_MS = 2_500;
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

export function useSmartPositioning(
  petRef: RefObject<ChibiPetCanvasHandle | null>,
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
  /**
   * 用户是否正在操作桌宠（按住 / 拖动 / 长按）。
   *
   * 与 `focusedRef` 互补：按住桌宠拖动时窗口会获得焦点，但下面这些路径
   * 都可能让「正在操作」早于或晚于焦点状态成立，所以单独计数：
   * - 前端 `mousedown` 命中得比 `onFocusChanged` 早，收尾后到的检查会漏网
   * - 用户按住不动（长按）时没有窗口位移，只有这个标志能拦住避让
   * - 拖动结束后 `mouseup` 的展开早于失焦事件，避免松手瞬间被立刻拽走
   *
   * 首个问题（拖动/长按期间仍自动移动）与第二个问题（移动途中获得焦点
   * 应即刻停住）都由「检查移动意图的每一帧都重读这个标志」来解决。
   */
  const userInteractingRef = useRef(false);
  /** 正在执行的避让滑动会话代号；任何取消条件命中时自增即让循环当帧退出 */
  const moveTokenRef = useRef(0);

  /** 请求中止当前避让滑动（同步生效：滑动的下一帧即退出，不再下发位移） */
  const abortMove = () => {
    moveTokenRef.current++;
  };

  useEffect(() => {
    modelReadyRef.current = modelReady;
  }, [modelReady]);

  useEffect(() => {
    enabledRef.current = enabled;
  }, [enabled]);

  // 用户交互期挂起避让：按下即中止可能正在进行的滑动，
  // 松手后补跑一次检查（此时若已失焦，避让自然恢复）。
  useEffect(() => {
    const begin = () => {
      userInteractingRef.current = true;
      abortMove();
    };
    const end = () => {
      if (!userInteractingRef.current) return;
      userInteractingRef.current = false;
      // 松手后给系统一点时间派发失焦事件，再补跑，避免与焦点抖动打架
      if (focusCheckTimerRef.current !== null) {
        window.clearTimeout(focusCheckTimerRef.current);
      }
      focusCheckTimerRef.current = window.setTimeout(() => {
        focusCheckTimerRef.current = null;
        positioningCoordinator.triggerSmartCheck?.();
      }, 250);
    };
    window.addEventListener('mousedown', begin, true);
    window.addEventListener('mouseup', end, true);
    window.addEventListener('blur', end);
    return () => {
      window.removeEventListener('mousedown', begin, true);
      window.removeEventListener('mouseup', end, true);
      window.removeEventListener('blur', end);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

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
      token: number,
    ) => {
      const plan = planSmartMove(targetX - startX, targetY - startY);

      /**
       * 是否应立即放弃这次滑动。
       *
       * 除了组件卸载（`cancelled`），还要在**每一帧**重读用户的交互状态与
       * 焦点状态：避让的规划/截图/缓动分散在多个 await 之间，用户完全可能
       * 在滑动进行到一半时按下桌宠或把窗口切到前台——此时必须当帧停住，
       * 否则就出现「我正拖着它，它却还在自己滑」的抢夺。
       */
      const shouldAbort = () =>
        cancelled ||
        token !== moveTokenRef.current ||
        userInteractingRef.current ||
        focusedRef.current;

      /**
       * 打断时把角色送回基准姿态。
       *
       * 用户按下时由精灵自身的 mousedown 收尾（它会把动作切回基调），这里不抢——
       * 否则会盖掉随后下发的「被拎起」姿态。其余打断（获得焦点、卸载）如果不收尾，
       * 角色会定格在侧身或抬腿的格子上。
       */
      const abortToRest = () => {
        if (!userInteractingRef.current) petRef.current?.resetExpression();
      };

      // 有水平分量才转身起步。腿是侧向的，纵向挪动既不需要转身也不需要摆腿，
      // 由窗口滑动本身表达；这类位移此前会播一段与移动方向无关的碎步。
      // 转身是位移的入场过渡：转完停在侧身（playTurn 不回落基准姿态），紧接着起步。
      let walkDone: Promise<boolean> | undefined;
      if (plan.walking) {
        const turned = await petRef.current?.playTurn(plan.direction);
        if (shouldAbort()) return abortToRest();
        if (turned) {
          walkDone = petRef.current?.playWalk(plan.direction, plan.frames, plan.frameDelayMs);
        }
      }

      // 窗口按计划时长滑到位。时间轴是共享的：walkPlan 保证 durationMs === frames ×
      // frameDelayMs，且 durationMs 与 slideMs 相差不超过一个帧间隔，所以走动的播放
      // 窗口与这段滑动基本等长、快慢同涨同落。
      const completed = await runSlide({
        fromX: startX,
        fromY: startY,
        toX: targetX,
        toY: targetY,
        durationMs: plan.durationMs,
        apply: (x, y) => {
          void invoke('set_window_position', { x, y });
        },
        shouldAbort,
      });
      if (!completed) return abortToRest();

      // 走动与滑动共用同一段时间轴，等走动收尾再回身：否则角色会停在抬腿到一半的
      // 格子上，或者腿还在摆就被硬切回待机。回身是位移的退场过渡，转回正面后才回落基调。
      // 走动若被别的动作（说话/表情）抢走，那是新动作的舞台，不再插手去盖它。
      if (walkDone) {
        const walked = await walkDone;
        if (shouldAbort()) return abortToRest();
        if (walked) await petRef.current?.playTurnBack();
      }
    };

    const runCheck = async (force = false) => {
      if (inFlightRef.current) return;
      if (cancelled || !enabledRef.current || !modelReadyRef.current) return;
      if (focusedRef.current) return;
      // 用户正在按住/拖动桌宠：整体跳过。仅靠 focusedRef 不够——mousedown
      // 早于焦点事件，且长按期间窗口无位移、焦点也可能尚未落到桌宠窗口。
      if (userInteractingRef.current) return;
      if (
        positioningCoordinator.ambientMoveInFlight ||
        (!force &&
          (positioningCoordinator.fullscreenInFlight ||
            positioningCoordinator.fullscreenHidden))
      ) {
        return;
      }

      inFlightRef.current = true;
      positioningCoordinator.smartPositioningInFlight = true;
      // 本次滑动的会话代号：交互/失焦打断时自增，滑动循环据此当帧退出
      const token = ++moveTokenRef.current;
      try {
        const win = getCurrentWindow();
        const [pos, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
        if (cancelled) return;
        // 截图/规划期间用户可能已经按下桌宠，重新确认再继续
        if (userInteractingRef.current || focusedRef.current) return;

        const result = await invoke<FindSafePositionResult>('find_safe_position', {
          petX: pos.x,
          petY: pos.y,
          petW: size.width,
          petH: size.height,
          force,
        });
        if (cancelled) return;
        // 同上：截图是异步的，落点判断前再确认一次交互状态
        if (userInteractingRef.current || focusedRef.current) return;

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

        await animatePosition(targetX, targetY, pos.x, pos.y, token);
      } catch {
      } finally {
        inFlightRef.current = false;
        positioningCoordinator.smartPositioningInFlight = false;
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
          // 获得焦点 = 用户正在玩桌宠：立即中止可能正在进行的滑动，
          // 而不是等它把这段缓动播完（那正是「正在移动时获得焦点还在动」）
          abortMove();
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
      // 卸载即作废当前滑动会话
      moveTokenRef.current++;
      if (timerRef.current !== null) {
        window.clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      if (focusCheckTimerRef.current !== null) {
        window.clearTimeout(focusCheckTimerRef.current);
        focusCheckTimerRef.current = null;
      }
      unlistenFocus?.();
      positioningCoordinator.smartPositioningInFlight = false;
      positioningCoordinator.triggerSmartCheck = null;
    };
  }, [enabled]);
}
