import { useEffect, useRef } from 'react';
import type { RefObject } from 'react';
import type { Live2DModel } from 'pixi-live2d-display/cubism4';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getMixer, directSetParam } from '../utils/LayeredParameterMixer';
import { facsToCubismParams, type FacsChannels } from '../utils/EmotionFacs';
import { getCharacterId } from '../characterContext';

interface InstantReactPayload {
  emotion: string;
  intensity: number;
  facs: FacsChannels;
  layer: 'user' | 'ai';
  character_id?: string;
}

export function useInstantReact(
  modelRef: RefObject<Live2DModel | null>,
): void {
  const INSTANT_CLEAR_TIMEOUT_MS = 2500;
  const clearTimerRef = useRef<number | null>(null);
  const smootherRef = useRef<{
    current: Record<string, number>;
    speed: number;
  }>({ current: {}, speed: 18 });

  useEffect(() => {
    let unlisteners: UnlistenFn[] = [];
    let mounted = true;
    let lastFrame = 0;
    let targetParams: Record<string, number> = {};
    let active = false;

    const applyInstant = (payload: InstantReactPayload) => {
      if (!mounted) return;
      const charId = getCharacterId();
      if (payload.character_id && charId && payload.character_id !== charId) return;

      const facs: FacsChannels = {
        browInnerUp: payload.facs.browInnerUp ?? 0,
        browDown: payload.facs.browDown ?? 0,
        eyeSmile: payload.facs.eyeSmile ?? 0,
        eyeSquint: payload.facs.eyeSquint ?? 0,
        eyeOpen: payload.facs.eyeOpen ?? 1,
        mouthSmile: payload.facs.mouthSmile ?? 0.04,
        mouthFrown: payload.facs.mouthFrown ?? 0,
        mouthOpen: payload.facs.mouthOpen ?? 0,
        cheekPuff: payload.facs.cheekPuff ?? 0,
        blush: payload.facs.blush ?? 0,
        headZ: payload.facs.headZ ?? 0,
        headY: payload.facs.headY ?? 0,
      };
      targetParams = facsToCubismParams(facs);
      active = true;
      lastFrame = performance.now();
      startLoop();

      if (clearTimerRef.current !== null) {
        clearTimeout(clearTimerRef.current);
      }
      clearTimerRef.current = window.setTimeout(() => {
        clearInstantLayer();
      }, INSTANT_CLEAR_TIMEOUT_MS);
    };

    const clearInstantLayer = () => {
      active = false;
      targetParams = {};
      const model = modelRef.current;
      if (!model) return;
      const mixer = getMixer(model);
      mixer?.clearLayer('instant');
      smootherRef.current.current = {};
    };

    // tick 返回 true 表示需要继续下一帧，false 表示已收敛可停止 rAF 循环
    const tick = (): boolean => {
      if (!mounted) return false;
      if (!active && Object.keys(smootherRef.current.current).length === 0) return false;
      const model = modelRef.current;
      if (!model) return false;
      const mixer = getMixer(model);
      if (!mixer) {
        for (const [id, value] of Object.entries(targetParams)) {
          directSetParam(model, id, value);
        }
        return active;
      }

      const now = performance.now();
      if (lastFrame === 0) lastFrame = now;
      const delta = Math.min(0.1, (now - lastFrame) / 1000);
      lastFrame = now;

      const smoother = smootherRef.current;
      const factor = 1 - Math.exp(-smoother.speed * Math.max(0, delta));
      const keys = new Set([...Object.keys(targetParams), ...Object.keys(smoother.current)]);
      let anyActive = false;
      for (const key of keys) {
        const t = targetParams[key] ?? 0;
        const c = smoother.current[key] ?? 0;
        const next = c + (t - c) * factor;
        smoother.current[key] = next;
        if (Math.abs(next) > 0.001 || active) {
          anyActive = true;
        }
        mixer.setParameter('instant', key, next);
      }
      if (!active && !anyActive) {
        smoother.current = {};
        mixer.clearLayer('instant');
        return false;
      }
      return true;
    };

    let rafId: number | null = null;
    const tickLoop = () => {
      const cont = tick();
      if (cont && mounted) {
        rafId = requestAnimationFrame(tickLoop);
      } else {
        rafId = null;
      }
    };
    // 按需启动：仅当有活跃 instant 参数时才启动 rAF 循环，避免空闲时持续占用 CPU
    const startLoop = () => {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(tickLoop);
    };

    const setup = async () => {
      unlisteners.push(
        await listen<InstantReactPayload>('chat:instant_react', (event) => {
          applyInstant(event.payload);
        }),
      );
      unlisteners.push(
        await listen('chat:meta', () => {
          clearInstantLayer();
        }),
      );
      unlisteners.push(
        await listen('chat:done', () => {
          clearInstantLayer();
        }),
      );
      unlisteners.push(
        await listen('chat:cancelled', () => {
          clearInstantLayer();
        }),
      );
      unlisteners.push(
        await listen('chat:error', () => {
          clearInstantLayer();
        }),
      );
    };
    setup();

    return () => {
      mounted = false;
      if (rafId !== null) {
        cancelAnimationFrame(rafId);
      }
      if (clearTimerRef.current !== null) {
        clearTimeout(clearTimerRef.current);
      }
      for (const un of unlisteners) {
        try {
          un();
        } catch {
          /* ignore */
        }
      }
      unlisteners = [];
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
}
