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
import { planAmbientWalk } from '../chibi/walkPlan';
import { runSlide } from '../chibi/slideTrack';
import './ChibiPetCanvas.css';

export type ChibiInteraction = 'single_click' | 'double_click' | 'rough_click';

/** 词表里声明过的动作名；pose 状态即动作名，不再单独维护一份枚举。 */
type ChibiPose = string;

const WALK_SPEC = animation('walk');
const TURN_SPEC = animation('turn');
const BLINK_SPEC = animation('blink');
const CAST_SPEC = animation('cast');
const IDLE_SPEC = pose('idle');
const IDLE_SLOT = IDLE_SPEC.slot;

/**
 * 自主漫步的静息间隔（ms）：一趟走完至少歇这么久再起步。
 *
 * 上一版是固定 [7s, 12s)、平均每 9.5s 起步一次——间隔只由常数决定、与上下文无关，
 * 于是它既不是对「安静待着」的表达，也不是对「该有动静」的回应，只是节拍器。
 * 现在换成一段足够长的静息期；长距离漫步本身占用更久（时长由距离推），
 * 一趟远路自然把下一次推得更后。
 */
const WALK_REST_MIN_MS = 48_000;
const WALK_REST_RANGE_MS = 72_000;

/** 因被占用（说话/全屏/避让/被按住）而错过时点时的重试间隔（ms），比静息期短得多。 */
const WALK_BLOCKED_RETRY_MIN_MS = 8_000;
const WALK_BLOCKED_RETRY_RANGE_MS = 6_000;

/**
 * 上线后第一趟漫步的等待（ms）。
 *
 * 不直接套用静息期：静息期的语义是「刚走过一趟，歇一会儿」，上线时并不成立——
 * 真按 48–120s 算，用户打开桌宠后将近两分钟看不到它挪窝，第一印象偏"死"。
 */
const WALK_STARTUP_MIN_MS = 10_000;
const WALK_STARTUP_RANGE_MS = 15_000;

/**
 * 单次漫步的距离区间（px）。
 *
 * 上一版是 [58, 122) 的均匀分布：区间本来就窄，落在屏幕上不管抽到哪一头都像"同一小步"；
 * 而且时长是另一根独立的随机数（[2400, 3300) ms），于是"走 58px"和"走 120px"花一样
 * 的时间——位移与时长脱钩，速度纯随机。现在时长不再是独立随机数，由 walkPlan 按图集
 * 原生步速从距离反推（走多远就花多久），距离本身则在对数尺度上取样（见 walkOnce）：
 * 多数是几步的短挪动，偶尔来一趟横跨半屏的长溜达，不存在"档位"台阶。
 */
const WALK_DISTANCE_MIN_PX = 140;
const WALK_DISTANCE_MAX_PX = 900;

/** 桌宠与屏幕边缘保持的间隙（px）：贴着边站会显得被裁掉一半。 */
const EDGE_MARGIN_PX = 8;

const TURN_IN_FRAMES = Array.from({ length: TURN_SPEC.frames }, (_, index) => index);
const TURN_OUT_FRAMES = [...TURN_IN_FRAMES].reverse();
const BLINK_FRAMES = Array.from({ length: BLINK_SPEC.frames }, (_, index) => index);
const TURN_OUT_DURATIONS = [...TURN_SPEC.durations].reverse();

const BLINK_MIN_DELAY_MS = 3_200;
const BLINK_DELAY_RANGE_MS = 4_300;
/** 双击/单击等交互让动作停留的默认时长。 */
const POSE_HOLD_MS = 900;

/**
 * 单击（摸头）的反应池：`[动作名, 权重]`，空串代表「这一下不播表情」。
 *
 * 早先是写死的 `happy`——戳十次看十张一样的脸，反馈就退化成按钮了。现在变成一排
 * 「被戳一下」可能有的态度：被摸高兴了、得意、琢磨这是什么、或者懒得理你。权重不等
 * 是刻意的：`happy` 不再是必然，但仍是被摸头最自然的那一个；`smug` 稍多给一点，
 * 因为随机池里最需要的是「和上一次不一样」。空串要占够比例，否则池子退化成
 * 「每次都有表情」——那只是把单调从一张脸换成了四张脸。
 */
const TAP_REACTIONS: ReadonlyArray<readonly [string, number]> = [
  ['smug', 4],
  ['think', 3],
  ['happy', 3],
  ['', 3],
];

/**
 * 戳烦了的判定。
 *
 * 点击本身不携带力度，能测的只有次数和时间——所以「太频繁」和「太粗暴」不是两套
 * 规则，而是同一个账本上的两种计法：每戳一下记一笔，与上一戳贴得极近（猛戳）再记一笔。
 * 只统计最近 {@link TAP_ANNOY_WINDOW_MS} 内的账，于是慢慢戳永远攒不满，停手就自动清账。
 *
 * 攒够 {@link TAP_ANNOY_THRESHOLD} 就生气：清空账本、进入气头上，这期间怎么戳都是生气
 * （每戳一次把气头续期），停手满 {@link TAP_ANNOY_HOLD_MS} 才消气、从零重新攒。
 */
const TAP_ANNOY_WINDOW_MS = 5_000;
/** 两戳间隔短于此值即视为「猛戳」，额外记一笔。 */
const TAP_ROUGH_INTERVAL_MS = 350;
const TAP_ANNOY_THRESHOLD = 7;
const TAP_ANNOY_HOLD_MS = 2_500;

/**
 * 从反应池里按权重抽一个动作名，空串表示这次不播表情。
 *
 * 不播是**主动的**结果而非兜底：这一下只是被戳了，角色该干嘛干嘛去，
 * 所以调用方拿到空串时什么都不做，让待机与自然眨眼照常继续。
 */
function pickTapReaction(): string {
  const total = TAP_REACTIONS.reduce((sum, [, weight]) => sum + weight, 0);
  let roll = Math.random() * total;
  for (const [motion, weight] of TAP_REACTIONS) {
    roll -= weight;
    if (roll < 0) return motion;
  }
  return '';
}

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
  /**
   * 智能避让开场：快速转身面向 direction，转身结束 resolve（false 表示被打断）。
   *
   * 结束后**停在转身末帧**（侧身面向移动方向），不回落基准姿态——回落会让「转身完成」
   * 与「起步走动」之间闪一帧正面待机。由调用方接着播走动（`playWalk`）或回正
   * （`playTurnBack`）。
   */
  playTurn: (direction: 'left' | 'right') => Promise<boolean>;
  /**
   * 智能避让收尾：从侧身反向转回正面，随后回落到心情基调。false 表示被打断。
   *
   * 与 `playTurn` 配对，构成位移前后的转身过渡；走动未播（纵向位移）时不必调用。
   */
  playTurnBack: () => Promise<boolean>;
  /**
   * 智能避让用：按给定的帧数与帧间隔播放走动，走到收尾时 resolve。
   * frames / frameDelayMs 由 `planSmartMove` 推导，二者相乘即窗口位移时长；
   * false 表示被新动作或用户按压打断。
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
  /**
   * 一次点击手势的结果：单击、双击，或「戳烦了」（{@link TAP_ANNOY_THRESHOLD}）。
   * 前两者由点击节奏区分，`rough_click` 由画布内的戳烦了账本判定，三者互斥。
   */
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
    /** 戳烦了账本：最近这几次点击的时刻（只保留 TAP_ANNOY_WINDOW_MS 内的）。 */
    const tapLogRef = useRef<number[]>([]);
    /** 气头上到什么时候；早于此值前的点击都算「还没消气」。 */
    const annoyedUntilRef = useRef(0);
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
      // 不 returnToTone：停在转身末帧（侧身），由调用方接走动或回正。
      return playFrames(
        TURN_SPEC,
        TURN_IN_FRAMES,
        TURN_SPEC.durations,
        token,
        TURN_SPEC.name,
        direction,
      );
    }, [clearPoseTimer, playFrames]);

    /** 智能避让收尾回身：末帧起步倒放回正面，播完回落到心情基调。 */
    const playTurnBack = useCallback(async (): Promise<boolean> => {
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      // 不传 direction：沿用走动时的朝向，图集方向与转身前保持一致
      const completed = await playFrames(
        TURN_SPEC,
        TURN_OUT_FRAMES,
        TURN_OUT_DURATIONS,
        token,
        TURN_SPEC.name,
      );
      if (completed) returnToTone(token);
      return completed;
    }, [clearPoseTimer, playFrames, returnToTone]);

    /**
     * 起步走动：写好推进参数并切到走动姿态，返回本次走动的会话代号与「播完」的承诺。
     *
     * 帧数与帧间隔都由调用方按 walkPlan 算好（frames × frameDelayMs 即窗口滑动时长），
     * 这里只负责推进——于是腿和窗口收尾同时发生。
     *
     * 会话代号要露出来，是因为自主漫步得和窗口滑动**并行**跑：它必须自己判断这次走动
     * 有没有被别的动作（说话/表情/用户按压）接过——token 变了就该当帧停下窗口。
     */
    const beginWalk = useCallback((
      direction: ChibiDirection,
      frames: number,
      frameDelayMs: number,
    ): { token: number; done: Promise<boolean> } => {
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      walkTargetFramesRef.current = frames > 0 ? frames : null;
      walkFrameDelayMsRef.current = frameDelayMs > 0 ? frameDelayMs : null;
      setWalkDirection(direction);
      setActivePose(WALK_SPEC.name);
      // 帧数不再限定为图集周期的整数倍（见 walkPlan）：可能停在任意一格，紧接着的
      // playTurnBack 会立刻换掉这一格，所以不会留下「抬腿到一半」的定格。
      const done = wait(Math.max(0, frames) * Math.max(0, frameDelayMs))
        .then(() => sequenceTokenRef.current === token && !pressedRef.current);
      return { token, done };
    }, [clearPoseTimer, setActivePose]);

    const playWalk = useCallback(async (
      direction: ChibiDirection,
      frames: number,
      frameDelayMs: number,
    ): Promise<boolean> => beginWalk(direction, frames, frameDelayMs).done, [beginWalk]);

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
      playTurnBack,
      playWalk,
      startCast,
      cancelCast,
      stopCast,
    }), [applyMotion, cancelCast, clearPoseTimer, onScaleChange, previewBlink, previewTurn, previewWalk, returnToTone, startCast, stopCast, playTurn, playTurnBack, playWalk]);

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
        // 指定了帧数时播满即停。帧数由位移时长与帧间隔推导（不保证是周期的整数倍），
        // 停在哪一格都有可能；紧随其后的回身动画会立即接管这一格。
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

      /** 被占用（说话/全屏/避让/被按住）时短暂退避后重试，不消耗静息期。 */
      const scheduleRetry = () => {
        if (cancelled) return;
        if (timer !== null) window.clearTimeout(timer);
        timer = window.setTimeout(
          () => void walkOnce(),
          WALK_BLOCKED_RETRY_MIN_MS + Math.random() * WALK_BLOCKED_RETRY_RANGE_MS,
        );
      };

      /**
       * 走完一趟后的静息。被占用期间不排静息期，所以这里的等待只发生在「刚走过」
       * 或「主动决定不走」之后，语义干净：起点固定，间隔可控。
       */
      const scheduleRest = () => {
        if (cancelled) return;
        if (timer !== null) window.clearTimeout(timer);
        timer = window.setTimeout(
          () => void walkOnce(),
          WALK_REST_MIN_MS + Math.random() * WALK_REST_RANGE_MS,
        );
      };

      const walkOnce = async () => {
        if (
          cancelled || pressedRef.current || poseNameRef.current !== 'idle' ||
          positioningCoordinator.fullscreenHidden || positioningCoordinator.fullscreenInFlight ||
          positioningCoordinator.smartPositioningInFlight || positioningCoordinator.ambientMoveInFlight
        ) {
          scheduleRetry();
          return;
        }
        positioningCoordinator.ambientMoveInFlight = true;
        // 只有「被占用」才值得重试；能走到判断这一步说明是主动决定不走，按静息期处理。
        let retry = false;
        try {          const windowHandle = getCurrentWindow();
          const [position, size, monitor] = await Promise.all([
            windowHandle.outerPosition(),
            windowHandle.outerSize(),
            currentMonitor(),
          ]);
          if (cancelled) return;
          if (!monitor) {
            retry = true;
            return;
          }

          const minX = monitor.position.x + EDGE_MARGIN_PX;
          const maxX = monitor.position.x + monitor.size.width - size.width - EDGE_MARGIN_PX;
          /** 朝这个方向还剩多少可走空间（可能为负：桌宠已在边界外）。 */
          const roomFor = (dir: number) => (dir > 0 ? maxX - position.x : position.x - minX);

          let direction = Math.random() < 0.5 ? -1 : 1;
          // 贴着边就朝里走；两边都放不下一个完整步长（屏幕比桌宠还窄、多屏错位），
          // 这一趟直接放弃——硬塞出来的位移会比转身动画还短，只是徒劳地抖一下。
          if (roomFor(direction) < WALK_DISTANCE_MIN_PX) direction = -direction;
          const room = roomFor(direction);
          if (room < WALK_DISTANCE_MIN_PX) return;

          // 距离在对数尺度上取样：多数落在短端（几步就到），长距离偶尔出现，
          // 且连续可辨——不是"58px 档 / 120px 档"这种台阶。再按可用空间截断。
          const distance = Math.min(
            room,
            WALK_DISTANCE_MIN_PX *
              Math.pow(WALK_DISTANCE_MAX_PX / WALK_DISTANCE_MIN_PX, Math.random()),
          );
          const targetX = Math.max(
            minX,
            Math.min(maxX, Math.round(position.x + direction * distance)),
          );
          // 朝向必须由**实际**位移定，而不是抽签的方向：贴边截断后两者可能反号。
          const dx = targetX - position.x;
          if (Math.abs(dx) < WALK_DISTANCE_MIN_PX) return;

          // 时长与步数都由距离推（walkPlan）：走多远就花多久，腿摆一格地面挪一格。
          const plan = planAmbientWalk(dx);
          if (!plan.walking) return;
          const nextDirection: ChibiDirection = dx < 0 ? 'left' : 'right';

          const turned = await playTurn(nextDirection);
          if (!turned || cancelled) return;

          // 走动与窗口滑动并行、共用同一条时长轴（frames × frameDelayMs === durationMs）。
          const walk = beginWalk(nextDirection, plan.frames, plan.frameDelayMs);
          const slid = await runSlide({
            fromX: position.x,
            fromY: position.y,
            toX: targetX,
            toY: position.y,
            durationMs: plan.durationMs,
            apply: (x, y) => {
              void invoke('set_window_position', { x, y }).catch(() => {});
            },
            shouldAbort: () =>
              cancelled || pressedRef.current || sequenceTokenRef.current !== walk.token,
          });
          // 被打断：窗口停在半途，姿态交给接管者，不再插手。
          if (!slid) return;
          if (!(await walk.done)) return;
          await playTurnBack();
        } catch {
          // 智能避让仍是权威；自主漫步只是可选的舞台调度，出错就安静跳过这一趟。
          retry = true;
        } finally {
          positioningCoordinator.ambientMoveInFlight = false;
          if (retry) scheduleRetry();
          else scheduleRest();
        }
      };

      // 首趟：上线后不久走一次（静息期语义此时不适用），之后按静息期节奏走。
      timer = window.setTimeout(
        () => void walkOnce(),
        WALK_STARTUP_MIN_MS + Math.random() * WALK_STARTUP_RANGE_MS,
      );
      return () => {
        cancelled = true;
        if (timer !== null) window.clearTimeout(timer);
        positioningCoordinator.ambientMoveInFlight = false;
      };
    }, [ambientMotionEnabled, beginWalk, playTurn, playTurnBack, previewMode]);

    useEffect(() => () => {
      clearPoseTimer();
      if (clickTimerRef.current) clearTimeout(clickTimerRef.current);
    }, [clearPoseTimer]);

    /**
     * 记一笔点击，返回「这一下是否算戳烦了」。
     *
     * 记账口径见 {@link TAP_ANNOY_WINDOW_MS} 一带的注释。双击的两次点击同样入账——
     * 双击只是同时还另有用途（开快捷聊天），不代表这两下不算戳。
     */
    const noteTap = useCallback((now: number): boolean => {
      // 气头上：照旧算生气，并把气头往后续（只要还在戳就不消气）
      if (now < annoyedUntilRef.current) {
        annoyedUntilRef.current = now + TAP_ANNOY_HOLD_MS;
        return true;
      }

      const recent = tapLogRef.current.filter((at) => now - at < TAP_ANNOY_WINDOW_MS);
      recent.push(now);
      // 基准分=窗口中戳了几下；猛戳额外记一笔——「又戳一次」和「连着猛戳」不是一回事
      let score = recent.length;
      for (let index = 1; index < recent.length; index += 1) {
        if (recent[index] - recent[index - 1] < TAP_ROUGH_INTERVAL_MS) score += 1;
      }
      if (score < TAP_ANNOY_THRESHOLD) {
        tapLogRef.current = recent;
        return false;
      }
      // 攒够了：清账再生气，退出气头后从零重新攒，不会因为一笔旧账一直气下去
      tapLogRef.current = [];
      annoyedUntilRef.current = now + TAP_ANNOY_HOLD_MS;
      return true;
    }, []);

    const handleClick = () => {
      // 窗口发生过实际拖动时，mouseup 后浏览器仍可能补发 click；该 click 不应触发台词。
      if (Date.now() < suppressClickUntilRef.current) return;
      const annoyed = noteTap(Date.now());
      if (clickTimerRef.current) {
        clearTimeout(clickTimerRef.current);
        clickTimerRef.current = null;
        // 被戳毛了的时候双击照样开聊天，但脸上的态度不再是配合
        applyMotion(annoyed ? 'angry' : 'talk', annoyed ? undefined : 700);
        onInteraction?.(annoyed ? 'rough_click' : 'double_click');
        onOpenQuickChat?.();
        return;
      }
      clickTimerRef.current = setTimeout(() => {
        clickTimerRef.current = null;
        const reaction = annoyed ? 'angry' : pickTapReaction();
        // 空串 = 这一下不播表情：什么都不做，待机与自然眨眼照常继续。
        // 这张生气脸正播着就不重播——同一口气上被反复打断在第 0 帧会看起来像卡住。
        // （鼠标点击其实轮不到这里：mousedown 早已把动画切回基调了，拦住的是
        //   键盘回车/空格这条不经过 mousedown 的路径。）
        const alreadyAngry = annoyed && poseNameRef.current === 'angry';
        if (reaction && !alreadyAngry) applyMotion(reaction);
        onInteraction?.(annoyed ? 'rough_click' : 'single_click');
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
