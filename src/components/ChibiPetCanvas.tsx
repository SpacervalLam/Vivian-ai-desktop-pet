import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
  type WheelEvent as ReactWheelEvent,
} from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { currentMonitor, getCurrentWindow } from '@tauri-apps/api/window';
import { getCharacterId } from '../characterContext';
import { positioningCoordinator } from '../hooks/positioningCoordinator';
import {
  animation,
  frameDurationMs,
  frameStyle,
  getMotion,
  pose,
  prefetchUrls,
  resolveMotion,
  sheetUrl,
  totalDurationMs,
  type ChibiAnimationSpec,
  type ChibiDirection,
} from '../chibi/motionRegistry';
import './ChibiPetCanvas.css';

export type ChibiInteraction = 'single_click' | 'double_click';

/** 词表里声明过的动作名；pose 状态即动作名，不再单独维护一份枚举。 */
type ChibiPose = string;

const WALK_SPEC = animation('walk');
const TURN_SPEC = animation('turn');
const BLINK_SPEC = animation('blink');
const CAST_SPEC = animation('cast');
const IDLE_SPEC = pose('idle');
const IDLE_SLOT = IDLE_SPEC.slot;

const WALK_MIN_DELAY_MS = 7_000;
const WALK_DELAY_RANGE_MS = 5_000;
const TURN_IN_FRAMES = Array.from({ length: TURN_SPEC.frames }, (_, index) => index);
const TURN_OUT_FRAMES = [...TURN_IN_FRAMES].reverse();
const BLINK_FRAMES = Array.from({ length: BLINK_SPEC.frames }, (_, index) => index);
const TURN_OUT_DURATIONS = [...TURN_SPEC.durations].reverse();

const BLINK_MIN_DELAY_MS = 3_200;
const BLINK_DELAY_RANGE_MS = 4_300;
/** 双击/单击等交互让动作停留的默认时长。 */
const POSE_HOLD_MS = 900;

const wait = (durationMs: number) => new Promise<void>((resolve) => {
  window.setTimeout(resolve, durationMs);
});

export interface ChibiPetCanvasHandle {
  setExpression: (name: string, durationMs?: number) => void;
  /** 设置心情基调格位：一次性动作播完回落到它，而非回到 idle。只有图集格位能当基调。 */
  setMoodTone: (name: string) => void;
  playMotion: (group: string, index?: number) => void;
  focus: (x: number, y: number) => void;
  setScale: (scale: number) => void;
  getScale: () => number;
  refitModel: () => void;
  resetExpression: () => void;
  previewWalk: (direction: 'left' | 'right') => void;
  previewTurn: (direction: 'left' | 'right') => void;
  previewBlink: () => void;
  /** 智能避让开场：快速转身面向 direction，转身结束 resolve（false 表示被打断）。 */
  playTurn: (direction: 'left' | 'right') => Promise<boolean>;
  /**
   * 智能避让用：按给定的帧数与帧间隔播放走动，走到收尾时 resolve。
   * frames 由调用方按距离分档、恒为图集周期整数倍；false 表示被新动作或用户按压打断。
   */
  playWalk: (direction: 'left' | 'right', frames: number, frameDelayMs: number) => Promise<boolean>;
  /** 长按进度环出现时开始"施法召唤窗口"动画：durationMs 内正向播完（与进度环填充同步）；不被 pressed 状态打断。 */
  startCast: (durationMs: number) => void;
  /** 长按取消：从当前帧倒放回初始帧，随后回 idle。 */
  cancelCast: () => void;
  /** 长按完成：施法立即结束归位 idle（正常应已自然播完，此处兜底）。 */
  stopCast: () => void;
}

export interface ChibiPetCanvasProps {
  modelUrl?: string;
  onReady?: () => void;
  onExpressionEnd?: () => void;
  onModelClick?: () => void;
  mouseFollowMode?: 'always' | 'window' | 'off';
  onScaleChange?: (scale: number) => void;
  onInteraction?: (interaction: ChibiInteraction) => void;
  onOpenQuickChat?: () => void;
  /** Kept compatible with ModelCanvas while stage walking is coordinated above the renderer. */
  ambientMotionEnabled?: boolean;
  /** Browser-only visual QA route; avoids subscribing to unavailable Tauri events. */
  previewMode?: boolean;
}

function normalizeCharacterId(): 'vivian' | 'nana' {
  return (getCharacterId() ?? 'vivian').toLowerCase().includes('nana') ? 'nana' : 'vivian';
}

/**
 * 轻量 Q 版桌宠渲染器。
 *
 * 动作全部来自 `chibi/animations.json` 声明的词汇表：图集格位负责可持续的定格姿态，
 * 帧序列负责自带节奏的一次性动作。舞台层只使用 CSS 位移/缩放/回弹，不引入图形
 * 运行时与大体积模型资源。它保留旧 handle 的结构，让智能避让、presence、聊天 meta
 * 与拖动状态无需改写即可继续调度角色。
 */
export const ChibiPetCanvas = forwardRef<ChibiPetCanvasHandle, ChibiPetCanvasProps>(
  function ChibiPetCanvas(
    {
      onReady,
      onExpressionEnd,
      onModelClick,
      mouseFollowMode = 'window',
      onScaleChange,
      onInteraction,
      onOpenQuickChat,
      ambientMotionEnabled = true,
      previewMode = false,
    },
    ref,
  ) {
    const characterId = normalizeCharacterId();
    const stageRef = useRef<HTMLDivElement | null>(null);
    const [poseName, setPoseName] = useState<ChibiPose>('idle');
    const poseNameRef = useRef<ChibiPose>('idle');
    const [walkDirection, setWalkDirection] = useState<ChibiDirection>('left');
    const [frame, setFrame] = useState(0);
    /** 智能避让指定的走动帧间隔（ms）；为 null 时按图集固有节奏播放。 */
    const walkFrameDelayMsRef = useRef<number | null>(null);
    /** 智能避让播放的走动帧数；为 null 时按固有节奏持续循环。 */
    const walkTargetFramesRef = useRef<number | null>(null);
    const [pressed, setPressed] = useState(false);
    const pressedRef = useRef(false);
    const sequenceTokenRef = useRef(0);
    const poseTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
    const clickTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
    const scaleRef = useRef(1);
    const suppressClickUntilRef = useRef(0);
    /** 心情基调格位：一次性动作播完回落到它，而不是硬编码 idle。由后端 mood_tone 下发。 */
    const moodToneRef = useRef<ChibiPose>('idle');
    const moodToneSlotRef = useRef<number>(IDLE_SLOT);
    const onReadyRef = useRef(onReady);

    useEffect(() => {
      onReadyRef.current = onReady;
    }, [onReady]);

    const clearPoseTimer = useCallback(() => {
      if (poseTimerRef.current) {
        clearTimeout(poseTimerRef.current);
        poseTimerRef.current = null;
      }
    }, []);

    const setActivePose = useCallback((next: ChibiPose) => {
      poseNameRef.current = next;
      setPoseName(next);
    }, []);

    /**
     * 是否停在基准姿态上（idle 或当前心情基调）。
     *
     * 基调非 idle 时（如累到挂着 dizzy）也算静止，否则环境眨眼会把"静止的疲惫脸"
     * 误判成"正在播动作"而整个停掉，桌宠看起来像卡死。
     */
    const isAtRest = useCallback(
      () => poseNameRef.current === 'idle' || poseNameRef.current === moodToneRef.current,
      [],
    );

    /** 回到基准姿态（当前心情基调，默认 idle），并清理走动相关的调度参数。 */
    const returnToTone = useCallback((token?: number) => {
      if (token !== undefined && sequenceTokenRef.current !== token) return;
      poseTimerRef.current = null;
      walkFrameDelayMsRef.current = null;
      walkTargetFramesRef.current = null;
      setActivePose(moodToneRef.current);
      setFrame(moodToneSlotRef.current);
      onExpressionEnd?.();
    }, [onExpressionEnd, setActivePose]);

    /** 逐帧播放一段序列；返回 false 表示被新的动作或用户按压打断。 */
    const playFrames = useCallback(async (
      spec: ChibiAnimationSpec,
      frames: number[],
      durations: number[],
      token: number,
      poseLabel: string,
      direction?: ChibiDirection,
    ): Promise<boolean> => {
      setActivePose(poseLabel);
      if (direction) setWalkDirection(direction);
      for (let index = 0; index < frames.length; index += 1) {
        if (sequenceTokenRef.current !== token || pressedRef.current) return false;
        setFrame(frames[index]);
        await wait(durations[index] ?? durations[durations.length - 1] ?? frameDurationMs(spec, index));
      }
      return sequenceTokenRef.current === token && !pressedRef.current;
    }, [setActivePose]);

    /**
     * 应用一个动作名。
     *
     * 图集格位是可持续的姿态：给了时长就按时长回落。帧序列分两类——循环型（走动）
     * 的帧推进由循环推进器负责，这里只管入场与回落时长；一次性动作自带节奏，
     * 播完即回落，不受外部时长影响（调用方传 3000ms 只是想表达"持续一会儿"）。
     */
    const applyMotion = useCallback((raw: string, durationMs?: number) => {
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      const spec = resolveMotion(raw);
      if (spec.kind === 'pose') {
        setActivePose(spec.name);
        setFrame(spec.slot);
        if (durationMs && durationMs > 0) {
          poseTimerRef.current = setTimeout(() => returnToTone(token), durationMs);
        }
        return;
      }
      if (spec.loop) {
        walkFrameDelayMsRef.current = null;
        walkTargetFramesRef.current = null;
        setActivePose(spec.name);
        const holdMs = durationMs && durationMs > 0 ? durationMs : totalDurationMs(spec);
        poseTimerRef.current = setTimeout(() => returnToTone(token), holdMs);
        return;
      }
      const frames = Array.from({ length: spec.frames }, (_, index) => index);
      void (async () => {
        const completed = await playFrames(spec, frames, spec.durations, token, spec.name);
        if (completed) returnToTone(token);
      })();
    }, [clearPoseTimer, playFrames, returnToTone, setActivePose]);

    /** 原地演示转身（不移动窗口）。 */
    const previewTurn = useCallback((direction: ChibiDirection) => {
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      void playFrames(TURN_SPEC, TURN_IN_FRAMES, TURN_SPEC.durations, token, TURN_SPEC.name, direction)
        .then((completed) => {
          if (completed) returnToTone(token);
        });
    }, [clearPoseTimer, playFrames, returnToTone]);

    /** 原地演示走一圈：转身入场 → 走动 → 转身离场。 */
    const previewWalk = useCallback((direction: ChibiDirection) => {
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      walkFrameDelayMsRef.current = null;
      walkTargetFramesRef.current = null;
      void (async () => {
        const turned = await playFrames(
          TURN_SPEC,
          TURN_IN_FRAMES,
          TURN_SPEC.durations,
          token,
          TURN_SPEC.name,
          direction,
        );
        if (!turned) return;
        setActivePose(WALK_SPEC.name);
        await wait(totalDurationMs(WALK_SPEC));
        if (sequenceTokenRef.current !== token || pressedRef.current) return;
        const returned = await playFrames(
          TURN_SPEC,
          TURN_OUT_FRAMES,
          TURN_OUT_DURATIONS,
          token,
          TURN_SPEC.name,
        );
        if (returned) returnToTone(token);
      })();
    }, [clearPoseTimer, playFrames, returnToTone, setActivePose]);

    const previewBlink = useCallback(() => {
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      void playFrames(BLINK_SPEC, BLINK_FRAMES, BLINK_SPEC.durations, token, BLINK_SPEC.name)
        .then((completed) => {
          if (completed) returnToTone(token);
        });
    }, [clearPoseTimer, playFrames, returnToTone]);

    const playTurn = useCallback(async (direction: ChibiDirection): Promise<boolean> => {
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      const completed = await playFrames(
        TURN_SPEC,
        TURN_IN_FRAMES,
        TURN_SPEC.durations,
        token,
        TURN_SPEC.name,
        direction,
      );
      if (completed) returnToTone(token);
      return completed;
    }, [clearPoseTimer, playFrames, returnToTone]);

    const playWalk = useCallback(async (
      direction: ChibiDirection,
      frames: number,
      frameDelayMs: number,
    ): Promise<boolean> => {
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      // 帧数与帧间隔都由调用方按「距离分档 + 总时长反推」算好并限幅，这里只负责推进。
      walkTargetFramesRef.current = frames > 0 ? frames : null;
      walkFrameDelayMsRef.current = frameDelayMs > 0 ? frameDelayMs : null;
      setWalkDirection(direction);
      setActivePose(WALK_SPEC.name);
      // 帧数恒为图集周期的整数倍，播满即停在第一格，不会留一个抬腿到一半的姿势。
      await wait(Math.max(0, frames) * Math.max(0, frameDelayMs));
      return sequenceTokenRef.current === token && !pressedRef.current;
    }, [clearPoseTimer, setActivePose]);

    // 长按"施法召唤窗口"：与进度环同步的正向播放 + 取消倒放。
    // 会话（token/当前帧/缩放后帧时长）记在 ref，取消/完成据此从当前帧接续；
    // 自行控制帧推进，不被 pressedRef 打断（用户此时仍按着左键）。
    const castSessionRef = useRef<{ token: number; frame: number; durations: number[] } | null>(null);

    const stopCast = useCallback(() => {
      if (!castSessionRef.current) return;
      castSessionRef.current = null;
      // 停掉仍在进行的正向/倒放循环并回落到心情基调（自然播完时姿态已是基调，此处兜底）
      sequenceTokenRef.current += 1;
      returnToTone();
    }, [returnToTone]);

    const startCast = useCallback((durationMs: number) => {
      stopCast();
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      // 每帧时长按给定总时长等比缩放：保持原作节奏，同时与进度环填充时长对齐
      const total = totalDurationMs(CAST_SPEC);
      const scale = durationMs > 0 ? durationMs / total : 1;
      const durations = CAST_SPEC.durations.map((duration) => Math.max(16, Math.round(duration * scale)));
      castSessionRef.current = { token, frame: 0, durations };
      setActivePose(CAST_SPEC.name);
      void (async () => {
        for (let index = 0; index < durations.length; index += 1) {
          if (sequenceTokenRef.current !== token) return;
          castSessionRef.current = { token, frame: index, durations };
          setFrame(index);
          await wait(durations[index]);
        }
        if (sequenceTokenRef.current !== token) return;
        castSessionRef.current = null;
        returnToTone();
      })();
    }, [clearPoseTimer, returnToTone, setActivePose, stopCast]);

    const cancelCast = useCallback(() => {
      const session = castSessionRef.current;
      if (!session) return;
      castSessionRef.current = null;
      const token = ++sequenceTokenRef.current; // 停掉正向循环
      // 从当前帧倒放回初始帧，节奏与正向一致
      void (async () => {
        for (let index = session.frame - 1; index >= 0; index -= 1) {
          if (sequenceTokenRef.current !== token) return;
          setFrame(index);
          await wait(session.durations[index]);
        }
        if (sequenceTokenRef.current === token) {
          returnToTone();
        }
      })();
    }, [returnToTone]);

    useImperativeHandle(ref, () => ({
      setExpression: (name, durationMs) => {
        const spec = resolveMotion(name);
        if (spec.kind === 'animation' && spec.directions) {
          setWalkDirection(/right|east|右/i.test(name) ? 'right' : 'left');
        }
        if (spec.name === 'drag') suppressClickUntilRef.current = Date.now() + 500;
        applyMotion(name, durationMs);
      },
      setMoodTone: (name) => {
        const spec = resolveMotion(name);
        // 只有可持续的图集格位能当基调；帧序列（如 happy）会让位，不能拿来做基调。
        if (spec.kind !== 'pose') return;
        const previous = moodToneRef.current;
        moodToneRef.current = spec.name;
        moodToneSlotRef.current = spec.slot;
        // 正停在旧基调上才立刻换脸；正在播一次性动作时不打断，播完自然落到新基调。
        if (poseNameRef.current === previous || poseNameRef.current === 'idle') {
          setActivePose(spec.name);
          setFrame(spec.slot);
        }
      },
      playMotion: (group) => {
        const spec = resolveMotion(group);
        if (spec.kind === 'animation' && spec.directions) {
          setWalkDirection(/right|east|右/i.test(group) ? 'right' : 'left');
        }
        applyMotion(group, spec.name === 'walk' ? 1_500 : POSE_HOLD_MS);
      },
      focus: (x, y) => {
        const stage = stageRef.current;
        if (!stage) return;
        stage.style.setProperty('--gaze-x', `${Math.max(-5, Math.min(5, x * 5))}px`);
        stage.style.setProperty('--gaze-y', `${Math.max(-3, Math.min(3, y * 3))}px`);
      },
      setScale: (scale) => {
        scaleRef.current = Math.max(0.5, Math.min(2, scale));
        onScaleChange?.(scaleRef.current);
      },
      getScale: () => scaleRef.current,
      refitModel: () => {
        scaleRef.current = 1;
      },
      resetExpression: () => {
        // 打断当前动作并回落到心情基调（默认 idle）。基调是「回落目标」，
        // 硬切 idle 会把它抹掉——智能避让或拖拽结束后，挂着疲惫基调的角色
        // 不该突然换成一张无表情的默认脸。
        clearPoseTimer();
        sequenceTokenRef.current += 1;
        returnToTone();
      },
      previewWalk,
      previewTurn,
      previewBlink,
      playTurn,
      playWalk,
      startCast,
      cancelCast,
      stopCast,
    }), [applyMotion, cancelCast, clearPoseTimer, onScaleChange, previewBlink, previewTurn, previewWalk, returnToTone, startCast, stopCast, playTurn, playWalk]);

    // 动作图集在首次真正播放前预取：切换到走动/表情时不必等图集下载。
    //
    // 预取同时兼作「可用性探测」：帧序列的图集是一张张独立文件，任何一张缺失或损坏
    // 都会让精灵的背景图解析失败——背景图没了精灵就是透明的，整只桌宠在动作播放期间
    // 直接消失（只有影子还在），而且浏览器对背景图失败不报错。这里把结果记下来，
    // 渲染时据此回落主图集，让缺图退化成「停在基准姿态」而不是「桌宠不见了」。
    const [failedSheets, setFailedSheets] = useState<ReadonlySet<string>>(
      () => new Set<string>(),
    );
    /** 持有预取中的 Image：局部变量出作用域被回收后，浏览器可能取消这次请求。 */
    const sheetImagesRef = useRef<Map<string, HTMLImageElement>>(new Map());

    useEffect(() => {
      const cache = new Map<string, HTMLImageElement>();
      sheetImagesRef.current = cache;
      let cancelled = false;
      for (const source of prefetchUrls(characterId)) {
        const image = new Image();
        image.onerror = () => {
          if (cancelled) return;
          console.warn(`[ChibiPetCanvas] 动作图集加载失败，已回落主图集: ${source}`);
          setFailedSheets((previous) => {
            if (previous.has(source)) return previous;
            const next = new Set(previous);
            next.add(source);
            return next;
          });
        };
        image.src = source;
        cache.set(source, image);
      }
      return () => {
        cancelled = true;
        sheetImagesRef.current = new Map();
      };
    }, [characterId]);

    // 循环型帧序列（当前只有走动）自推进；一次性动作由 applyMotion 自行播完。
    useEffect(() => {
      const spec = getMotion(poseName);
      if (!spec || spec.kind !== 'animation' || !spec.loop) {
        return undefined;
      }
      setFrame(0);
      let cancelled = false;
      let timer: number | null = null;
      const frameDelay = walkFrameDelayMsRef.current;
      const targetFrames = walkTargetFramesRef.current;
      const advance = (current: number, shown: number) => {
        // 指定了帧数时播满即停。调用方给的帧数是图集周期的整数倍，所以停在第一格；
        // 若这个约束被破坏，角色会停在抬腿到一半的格子上，切回基准姿态时像腿突然落地。
        if (targetFrames != null && shown >= targetFrames) return;
        timer = window.setTimeout(() => {
          if (cancelled) return;
          const next = (current + 1) % spec.frames;
          setFrame(next);
          advance(next, shown + 1);
        }, frameDelay ?? frameDurationMs(spec, current));
      };
      advance(0, 0);
      return () => {
        cancelled = true;
        if (timer !== null) window.clearTimeout(timer);
      };
    }, [poseName]);

    // 自然眨眼：只在待机且未被按压时插入，偶尔连眨两下
    useEffect(() => {
      let cancelled = false;
      let timer: number | null = null;
      const schedule = () => {
        if (cancelled) return;
        timer = window.setTimeout(async () => {
          if (cancelled) return;
          if (!isAtRest() || pressedRef.current) {
            schedule();
            return;
          }
          const token = ++sequenceTokenRef.current;
          let completed = await playFrames(
            BLINK_SPEC,
            BLINK_FRAMES,
            BLINK_SPEC.durations,
            token,
            BLINK_SPEC.name,
          );
          if (completed && Math.random() < 0.12) {
            await wait(135 + Math.random() * 55);
            if (!cancelled && sequenceTokenRef.current === token) {
              completed = await playFrames(
                BLINK_SPEC,
                BLINK_FRAMES,
                BLINK_SPEC.durations,
                token,
                BLINK_SPEC.name,
              );
            }
          }
          if (completed && !cancelled) returnToTone(token);
          schedule();
        }, BLINK_MIN_DELAY_MS + Math.random() * BLINK_DELAY_RANGE_MS);
      };
      schedule();
      return () => {
        cancelled = true;
        if (timer !== null) window.clearTimeout(timer);
      };
    }, [characterId, isAtRest, playFrames, returnToTone]);

    useEffect(() => {
      if (previewMode) {
        const readyFrame = requestAnimationFrame(() => onReadyRef.current?.());
        return () => cancelAnimationFrame(readyFrame);
      }
      void invoke('start_cursor_tracking', { characterId: getCharacterId() ?? undefined }).catch(() => {});
      const readyFrame = requestAnimationFrame(() => onReadyRef.current?.());
      return () => cancelAnimationFrame(readyFrame);
    }, [previewMode]);

    useEffect(() => {
      if (mouseFollowMode === 'off') {
        stageRef.current?.style.setProperty('--gaze-x', '0px');
        stageRef.current?.style.setProperty('--gaze-y', '0px');
        return;
      }
      const handleMove = (event: PointerEvent) => {
        const stage = stageRef.current;
        if (!stage) return;
        const nx = event.clientX / Math.max(1, window.innerWidth) - 0.5;
        const ny = event.clientY / Math.max(1, window.innerHeight) - 0.5;
        stage.style.setProperty('--gaze-x', `${(nx * 7).toFixed(2)}px`);
        stage.style.setProperty('--gaze-y', `${(ny * 4).toFixed(2)}px`);
      };
      window.addEventListener('pointermove', handleMove, { passive: true });
      return () => window.removeEventListener('pointermove', handleMove);
    }, [mouseFollowMode]);

    useEffect(() => {
      if (previewMode) return;
      let cancelled = false;
      const unlisteners: UnlistenFn[] = [];
      const mine = (id?: string) => !id || id.toLowerCase() === characterId;
      const add = async <T,>(event: string, handler: (payload: T) => void) => {
        const unlisten = await listen<T>(event, (message) => handler(message.payload));
        if (cancelled) unlisten();
        else unlisteners.push(unlisten);
      };

      void add<{ character_id?: string }>('tts:started', (p) => {
        if (mine(p.character_id)) applyMotion('talk');
      });
      void add<{ character_id?: string }>('tts:finished', (p) => {
        if (mine(p.character_id)) applyMotion('happy', 700);
      });
      void add<{ character_id?: string }>('tts:error', (p) => {
        if (mine(p.character_id)) applyMotion('idle');
      });
      void add<{ character_id?: string }>('chat:chunk', (p) => {
        if (mine(p.character_id)) applyMotion('talk');
      });
      void add<{ character_id?: string }>('chat:done', (p) => {
        if (mine(p.character_id)) applyMotion('happy', 850);
      });
      void add<{ speaker_id?: string; listener_id?: string }>('cross:start', (p) => {
        if (p.speaker_id?.toLowerCase() === characterId) applyMotion('talk');
        else if (p.listener_id?.toLowerCase() === characterId) applyMotion('listen');
      });
      void add<{ speaker_id?: string; listener_id?: string }>('cross:chunk', (p) => {
        if (p.speaker_id?.toLowerCase() === characterId) applyMotion('talk');
        else if (p.listener_id?.toLowerCase() === characterId) applyMotion('listen');
      });
      void add<{ speaker_id?: string; listener_id?: string }>('cross:done', (p) => {
        if (p.speaker_id?.toLowerCase() === characterId || p.listener_id?.toLowerCase() === characterId) {
          applyMotion('happy', 700);
        }
      });

      return () => {
        cancelled = true;
        unlisteners.forEach((unlisten) => unlisten());
      };
    }, [characterId, previewMode, applyMotion]);

    useEffect(() => {
      if (!ambientMotionEnabled || previewMode) return undefined;
      let cancelled = false;
      let timer: number | null = null;
      const schedule = () => {
        if (!cancelled) timer = window.setTimeout(() => void walkOnce(), WALK_MIN_DELAY_MS + Math.random() * WALK_DELAY_RANGE_MS);
      };
      const walkOnce = async () => {
        if (
          cancelled || pressedRef.current || poseNameRef.current !== 'idle' ||
          positioningCoordinator.fullscreenHidden || positioningCoordinator.fullscreenInFlight ||
          positioningCoordinator.smartPositioningInFlight || positioningCoordinator.ambientMoveInFlight
        ) {
          schedule();
          return;
        }
        positioningCoordinator.ambientMoveInFlight = true;
        try {
          const windowHandle = getCurrentWindow();
          const [position, size, monitor] = await Promise.all([
            windowHandle.outerPosition(),
            windowHandle.outerSize(),
            currentMonitor(),
          ]);
          if (!monitor || cancelled) return;
          const minX = monitor.position.x + 8;
          const maxX = monitor.position.x + monitor.size.width - size.width - 8;
          let direction = Math.random() < 0.5 ? -1 : 1;
          if (position.x < minX + 90) direction = 1;
          if (position.x > maxX - 90) direction = -1;
          const targetX = Math.max(minX, Math.min(maxX, position.x + direction * (58 + Math.random() * 62)));
          if (Math.abs(targetX - position.x) < 18) return;
          const nextDirection: ChibiDirection = direction < 0 ? 'left' : 'right';
          const token = ++sequenceTokenRef.current;
          const turned = await playFrames(
            TURN_SPEC,
            TURN_IN_FRAMES,
            TURN_SPEC.durations,
            token,
            TURN_SPEC.name,
            nextDirection,
          );
          if (!turned || cancelled) return;
          setActivePose(WALK_SPEC.name);
          const duration = 2_400 + Math.random() * 900;
          const steps = 48;
          for (let step = 1; step <= steps; step += 1) {
            if (cancelled || pressedRef.current || sequenceTokenRef.current !== token) break;
            const progress = step / steps;
            const eased = progress < 0.5
              ? 2 * progress * progress
              : 1 - Math.pow(-2 * progress + 2, 2) / 2;
            await invoke('set_window_position', {
              x: Math.round(position.x + (targetX - position.x) * eased),
              y: position.y,
            }).catch(() => {});
            if (step < steps) await new Promise((resolve) => setTimeout(resolve, duration / steps));
          }
          if (!cancelled && !pressedRef.current && sequenceTokenRef.current === token) {
            await playFrames(
              TURN_SPEC,
              TURN_OUT_FRAMES,
              TURN_OUT_DURATIONS,
              token,
              TURN_SPEC.name,
            );
          }
          if (!cancelled) returnToTone(token);
        } catch {
          // Smart avoidance remains the authority; ambient walking is optional staging.
        } finally {
          positioningCoordinator.ambientMoveInFlight = false;
          schedule();
        }
      };
      schedule();
      return () => {
        cancelled = true;
        if (timer !== null) window.clearTimeout(timer);
        positioningCoordinator.ambientMoveInFlight = false;
      };
    }, [ambientMotionEnabled, playFrames, previewMode, returnToTone, setActivePose]);

    useEffect(() => () => {
      clearPoseTimer();
      if (clickTimerRef.current) clearTimeout(clickTimerRef.current);
    }, [clearPoseTimer]);

    const handleClick = () => {
      // 窗口发生过实际拖动时，mouseup 后浏览器仍可能补发 click；该 click 不应触发台词。
      if (Date.now() < suppressClickUntilRef.current) return;
      if (clickTimerRef.current) {
        clearTimeout(clickTimerRef.current);
        clickTimerRef.current = null;
        applyMotion('talk', 700);
        onInteraction?.('double_click');
        onOpenQuickChat?.();
        return;
      }
      clickTimerRef.current = setTimeout(() => {
        clickTimerRef.current = null;
        applyMotion('happy', 1200);
        onInteraction?.('single_click');
      }, 230);
    };

    const handleWheel = (event: ReactWheelEvent<HTMLDivElement>) => {
      if (!event.ctrlKey) return;
      event.preventDefault();
      const delta = event.deltaY < 0 ? 0.08 : -0.08;
      const next = Math.max(0.6, Math.min(1.8, scaleRef.current + delta));
      scaleRef.current = next;
      onScaleChange?.(next);
    };

    /**
     * 精灵的最终样式。
     *
     * 帧序列图集可用时按帧序列取格；不可用时退回主图集的当前基调格位——帧序列的
     * `frame` 序号在主图集里没有意义，拿它去定位只会取到无关的一格。
     */
    const activeSpec = resolveMotion(poseName);
    const activeSheet =
      activeSpec.kind === 'animation'
        ? sheetUrl(activeSpec, characterId, walkDirection)
        : null;
    const spriteFrameStyle =
      activeSheet !== null && failedSheets.has(activeSheet)
        ? frameStyle(
            getMotion(moodToneRef.current) ?? IDLE_SPEC,
            moodToneSlotRef.current,
            characterId,
            walkDirection,
          )
        : frameStyle(activeSpec, frame, characterId, walkDirection);

    return (
      <div
        className={`chibi-pet-canvas chibi-pet-${characterId}`}
        onWheel={handleWheel}
        aria-label={`${characterId} desktop pet`}
      >
        <div
          ref={stageRef}
          className={`chibi-pet-stage pose-${poseName}${pressed ? ' is-pressed' : ''}`}
        >
          <div className="chibi-pet-shadow" />
          <div
            className="chibi-pet-sprite"
            style={spriteFrameStyle}
            role="button"
            tabIndex={0}
            onMouseDown={() => {
              sequenceTokenRef.current += 1;
              const spec = getMotion(poseNameRef.current);
              if (spec?.kind === 'animation') {
                returnToTone();
              }
              pressedRef.current = true;
              setPressed(true);
              onModelClick?.();
            }}
            onMouseUp={() => {
              pressedRef.current = false;
              setPressed(false);
            }}
            onMouseLeave={() => {
              pressedRef.current = false;
              setPressed(false);
            }}
            onClick={handleClick}
            onDoubleClick={(event) => event.preventDefault()}
            onKeyDown={(event) => {
              if (event.key === 'Enter' || event.key === ' ') handleClick();
            }}
          />
        </div>
      </div>
    );
  },
);

ChibiPetCanvas.displayName = 'ChibiPetCanvas';
