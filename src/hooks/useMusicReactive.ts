/**
 * 音乐驱动动画 hook — 监听系统音频（WASAPI 回环），驱动 Live2D 随音乐律动
 *
 * - 通过 `set_music_reactive` 幂等启停后端回环捕获（全局单实例）
 * - 在 pixi ticker 中调用 `analyzer.tick()`，把 bass/mid/treble/beat
 *   映射到 Live2D 参数的 'music' 层（优先级高于 emotion/blink，低于 instant）
 * - 说话时不驱动嘴型（speech 层优先级更高，由 mixer max 逻辑兜底）
 * - 休息/离线 / 无音乐时清空 'music' 层
 */

import { useEffect, useRef } from 'react';
import type { MutableRefObject, RefObject } from 'react';
import type { Live2DModel } from 'pixi-live2d-display/cubism4';
import { invoke } from '@tauri-apps/api/core';
import type { Live2DLipsync } from '../utils/Live2DLipsync';
import { AudioAnalyzer } from '../utils/AudioAnalyzer';
import { getMixer } from '../utils/LayeredParameterMixer';
import { useAppStore } from '../stores/useAppStore';

interface MusicReactiveOptions {
  lipsyncRef: RefObject<Live2DLipsync | null>;
}

export function useMusicReactive(
  modelRef: RefObject<Live2DModel | null>,
  options: MusicReactiveOptions,
): MutableRefObject<() => void> {
  const analyzerRef = useRef<AudioAnalyzer | null>(null);

  const presenceState = useAppStore((s) => s.presenceState);
  const presenceRef = useRef(presenceState);
  presenceRef.current = presenceState;

  const tickRef = useRef<() => void>(() => {});

  // 启停后端捕获 + 同步配置开关
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    const sync = async () => {
      let enabled = false;
      try {
        enabled = await invoke<boolean>('get_music_reactive');
        // 幂等：已启用则保持，未启用则启动；同时把持久化值同步到运行态
        await invoke('set_music_reactive', { enabled });
      } catch {
        enabled = false;
      }
      if (cancelled) return;
      if (enabled) {
        if (!analyzerRef.current) {
          const a = new AudioAnalyzer();
          analyzerRef.current = a;
          void a.start().catch(() => {
            analyzerRef.current = null;
          });
        }
      } else {
        analyzerRef.current?.stop();
        analyzerRef.current = null;
        const m = modelRef.current ? getMixer(modelRef.current) : null;
        m?.clearLayer('music');
      }
    };

    void sync();

    // 配置保存（设置界面开关）后重新同步
    void import('@tauri-apps/api/event')
      .then(({ listen }) =>
        listen('config:saved', () => {
          void sync();
        }).then((u) => {
          if (cancelled) u();
          else unlisten = u;
        }),
      )
      .catch(() => {
        /* ignore */
      });

    return () => {
      cancelled = true;
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const musicTick = () => {
      const analyzer = analyzerRef.current;
      const model = modelRef.current;
      if (!analyzer || !model) return;
      analyzer.tick();
      const mixer = getMixer(model);
      if (!mixer) return;

      // 休息/离线不律动
      if (presenceRef.current === 'rest' || presenceRef.current === 'offline') {
        mixer.clearLayer('music');
        return;
      }

      const isSpeaking = options.lipsyncRef.current?.getState() === 'speaking';
      const hasMusic = analyzer.bpm > 0 || analyzer.beat > 0.05 || analyzer.bass > 0.002;
      if (!hasMusic) {
        mixer.clearLayer('music');
        return;
      }

      const t = performance.now() / 1000;
      const beatEnv = analyzer.beat;

      // 低频 → 身体左右摇摆（跟随节奏，正弦 + 节拍脉冲）
      const sway = Math.sin(t * 2.2) * analyzer.bass * 5 + beatEnv * 2.5;
      mixer.setParameter('music', 'ParamBodyAngleZ', sway);
      // 中频 → 身体前倾微晃
      mixer.setParameter('music', 'ParamBodyAngleX', Math.sin(t * 3.1) * analyzer.mid * 2.5);
      // 节拍 → 头部点头
      mixer.setParameter('music', 'ParamAngleY', beatEnv * 4);

      // 嘴型：未说话时随音乐张合；说话时让位给 speech 层
      if (isSpeaking) {
        mixer.clearLayerParam('music', 'ParamMouthOpenY');
        mixer.clearLayerParam('music', 'JawOpen');
        mixer.clearLayerParam('music', 'Jawopen');
      } else {
        const mouth = Math.min(0.5, analyzer.bass * 0.35 + analyzer.treble * 0.25 + beatEnv * 0.15);
        mixer.setParameter('music', 'ParamMouthOpenY', mouth);
        mixer.setParameter('music', 'JawOpen', mouth);
        mixer.setParameter('music', 'Jawopen', mouth);
      }
    };

    tickRef.current = musicTick;

    return () => {
      const m = modelRef.current ? getMixer(modelRef.current) : null;
      m?.clearLayer('music');
      tickRef.current = () => {};
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return tickRef;
}
