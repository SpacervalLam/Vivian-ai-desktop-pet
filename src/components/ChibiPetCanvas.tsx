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
  reversePlayback,
  sheetUrl,
  totalDurationMs,
  type ChibiAnimationSpec,
  type ChibiDirection,
  type ChibiMotionSpec,
} from '../chibi/motionRegistry';
import { planAmbientWalk } from '../chibi/walkPlan';
import { runSlide } from '../chibi/slideTrack';
import { runFlee, FLEE_TAKEOVER_HOLD_MS, type FleeEnv } from '../chibi/fleeTrack';
import type { FleeGeometry } from '../chibi/fleePlan';
import { TapAngerLedger } from '../chibi/tapAnger';
import './ChibiPetCanvas.css';

export type ChibiInteraction = 'single_click' | 'double_click' | 'rough_click';

declare global {
  interface Window {
    /**
     * 浏览器验收路由上的逃离位移轨迹（只有 `previewMode` 会写）。
     *
     * 预览页没有窗口可移：滑动的时间轴照跑，但采样点不下发给 Tauri 而是记在这里，
     * 验收脚本据此断言「真的从起点滑到了 `planFlee` 抽出的那个落点」——否则「逃离」
     * 这件事在浏览器里完全没有可观测面，只能靠姿态反推。
     */
    __chibiFleeTrace__?: Array<{ x: number; y: number; t: number }>;
    /**
     * 这一趟逃离的**计划落点**（只有 `previewMode` 会写）。
     *
     * 仅靠轨迹「有没有动」不够：位移被半路掐死时 `easeInOutCubic` 前段跑得快，采样点照样
     * 盖住大半距离，弱断言（「至少跑了 320px」）也会绿。记录计划落点，验收才能断言
     * 「最后一个采样点就是那个落点」——掐死与跑完是两回事。
     */
    __chibiFleePlan__?: { fromX: number; fromY: number; toX: number; toY: number; durationMs: number };
  }
}

/** 词表里声明过的动作名；pose 状态即动作名，不再单独维护一份枚举。 */
type ChibiPose = string;

const WALK_SPEC = animation('walk');
const TURN_SPEC = animation('turn');
const BLINK_SPEC = animation('blink');
const CAST_SPEC = animation('cast');
const BUSY_IN_SPEC = animation('busy-in');
const BUSY_LOOP_SPEC = animation('busy-loop');
const BUSY_IN_FRAMES = Array.from({ length: BUSY_IN_SPEC.frames }, (_, i) => i);
/** 退场 = 进场倒放：图集里没有单独的「收起手机」素材，倒着播就是收起。 */
const BUSY_OUT = reversePlayback(BUSY_IN_SPEC);
const IDLE_SPEC = pose('idle');
const IDLE_SLOT = IDLE_SPEC.slot;

/**
 * 是否是忙碌阶段占着舞台的那两个动作（掏出手机 / 看手机）。
 *
 * 忙碌在画布上是一段帧序列，但它表达的是 presence 级的**常驻状态**，而不是一次性的表演。
 * 这个区别在好几处都要用到：按住它不该把表演掐掉、插播的一次性动作播完该回落到它、
 * 循环推进器也要认识它。集中在一处判断，免得三处各写一份名字比较。
 */
function isBusyStage(spec: ChibiMotionSpec | null): boolean {
  return spec?.kind === 'animation'
    && (spec.name === BUSY_IN_SPEC.name || spec.name === BUSY_LOOP_SPEC.name);
}

    /**
     * 自主漫步的静息间隔（ms）：一趟走完至少歇这么久再起步。
     *
     * 静息期足够长，且长距离漫步本身占用更久（时长由距离推），一趟远路自然把下一次推得更后，
     * 避免出现「刚走完又立刻走」的节拍器感。
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
     * 单次漫步的距离区间（px），在对数尺度上取样。
     *
     * 多数为几步的短挪动，偶尔来一趟横跨半屏的长溜达，不存在「档位」台阶。
     * 时长由 walkPlan 按图集原生步速从距离反推（走多远就花多久），位移与时长不再脱钩。
     */
const WALK_DISTANCE_MIN_PX = 140;
const WALK_DISTANCE_MAX_PX = 900;

/** 桌宠与屏幕边缘保持的间隙（px）：贴着边站会显得被裁掉一半。 */
const EDGE_MARGIN_PX = 8;

const TURN_IN_FRAMES = Array.from({ length: TURN_SPEC.frames }, (_, index) => index);
/** 回身 = 转身倒放（帧与节奏一起倒序）。 */
const { frames: TURN_OUT_FRAMES, durations: TURN_OUT_DURATIONS } = reversePlayback(TURN_SPEC);
const BLINK_FRAMES = Array.from({ length: BLINK_SPEC.frames }, (_, index) => index);

const BLINK_MIN_DELAY_MS = 3_200;
const BLINK_DELAY_RANGE_MS = 4_300;
/** 双击/单击等交互让动作停留的默认时长。 */
const POSE_HOLD_MS = 900;

    /**
     * 单击（摸头）的反应池：`[动作名, 权重]`，空串代表「这一下不播表情」。
     *
     * 权重不等是刻意的：`happy` 仍是被摸头最自然的那一个，但不再是必然；`smug` 稍多给一点，
     * 因为随机池最需要「和上一次不一样」。空串要占够比例，否则池子退化成「每次都有表情」。
     */
const TAP_REACTIONS: ReadonlyArray<readonly [string, number]> = [
  ['smug', 4],
  ['think', 3],
  ['happy', 3],
  ['', 3],
];

/**
 * 逃离的落点区间（px）。
 *
 * 下限决定「多远才算跑开」：低于它只是原地蹭一下，用户读成「没反应」。
 * 上限决定「一趟跑多远」：逃离是一口气的冲刺，横跨整个桌面会显得像被瞬移。
 *
 * 「多久到位」不在这里——那是 `chibi/fleeTrack` 的 `FLEE_SPEED_PX_PER_MS`：这两条
 * 讲的是**跑多远**（场地问题），速度讲的是**跑多快**（手感问题），混在一起改起来会打架。
 */
const FLEE_MIN_DISTANCE_PX = 320;
const FLEE_MAX_DISTANCE_PX = 900;

/**
 * 生气脸先亮多久再起步（ms）。
 *
 * 这不是延迟——生气脸是**当场**切上去的，这一拍只是让它被看见：窗口一动起来，视线就
 * 跟到位移上去了。先瞪你一眼再窜出去，读起来是「有情绪」而不是「被弹开」。
 * 400ms ≈ angry 图集的前 4~5 帧，够看清「它生气了」，又不至于让逃离显得拖沓。
 */
const FLEE_ANGER_BEAT_MS = 400;

/**
 * 浏览器 QA 路由（`?view=rig_preview`）里的合成舞台几何。
 *
 * 预览页没有窗口也没有显示器，几何读不出来；给一组固定值，落点抽签与滑动时间轴就能在
 * 浏览器里照常跑完，验收脚本据此断言落点区间与时长。真机不会走到这里。
 */
const PREVIEW_FLEE_GEOMETRY: FleeGeometry = {
  fromX: 120,
  fromY: 60,
  windowWidth: 213,
  monitorX: 0,
  monitorWidth: 1_280,
  marginPx: EDGE_MARGIN_PX,
};

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
  /**
   * 播一个表情/动作多久。
   *
   * 图集格位（如 `dizzy`）**必须**给时长：格位自己不会计时，`durationMs` 为 0 或省略
   * 时它会一直挂着。帧序列自带节奏，时长被忽略。
   */
  setExpression: (name: string, durationMs?: number) => void;
  playMotion: (group: string, index?: number) => void;
  focus: (x: number, y: number) => void;
  setScale: (scale: number) => void;
  getScale: () => number;
  refitModel: () => void;
  resetExpression: () => void;
  /**
   * 进入忙碌（presence = busy）：掏出手机，接上「看手机」循环并一直停在循环里。
   *
   * 忙碌没有时长参数——它是一段状态，持续多久由状态源决定，退出走 {@link stopBusy}。
   * 已在忙碌中重复调用无副作用。
   */
  startBusy: () => void;
  /**
   * 退出忙碌：倒放「掏出手机」把手收回去，播完回落到 idle。非忙碌时无操作。
   *
   * 若进场还没演完就退出，从当前那一格往回倒，而不是跳到最后再整段收一遍。
   */
  stopBusy: () => void;
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
   * 智能避让收尾：从侧身反向转回正面，随后回落到 idle。false 表示被打断。
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
    /**
     * 左键此刻是否按着（不分姿态）。
     *
     * 与 `pressedRef` 是两件事：`pressedRef` 表达「按在一张**可持续的姿态**上，所以打断它」，
     * 播帧序列时按下并不会置真（否则每一按都会把正在播的动画掐掉）。而「有没有人按着」
     * 是逃离要问的问题，与当前播的是格位还是帧序列无关，所以单独记一份。
     */
    const pressAliveRef = useRef(false);
    /** 这一次按下的时刻。逃离要知道的是「按了多久」，不只是「按没按」。 */
    const pressStartedAtRef = useRef(0);
    const sequenceTokenRef = useRef(0);
    const poseTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
    const clickTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
    /** 戳烦了账本（判定规则见 chibi/tapAnger）。 */
    const tapLedgerRef = useRef(new TapAngerLedger());
    /** 一趟逃离是否还在跑：期间点击一律不改动作，免得把正在跑的位移打断在半路。 */
    const fleeingRef = useRef(false);
    const scaleRef = useRef(1);
    const suppressClickUntilRef = useRef(0);
    /**
     * 忙碌走到哪一步了。
     *
     * 忙碌是**常驻状态**而非定时动作：角色在忙自己的事，持续多久由 presence 决定
     * （几秒也可能几分钟），所以这里只记「正在进 / 正在循环 / 正在退」，不设超时。
     * 到点自动收场会让还在忙的角色突然收起手机，与状态本身矛盾。
     */
    const busyPhaseRef = useRef<'idle' | 'in' | 'loop' | 'out'>('idle');
    /** 进场动画播到第几帧。半路退出时据此决定倒放起点。 */
    const busyInFrameRef = useRef(0);
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
     * 是否停在基准姿态上（`idle`）。
     *
     * 基准姿态就是 `idle`，没有别的：曾经还有一条"心情基调"通道能把 `dizzy` 之类的
     * 格位长期挂成底色，那条通道连同后端的心情基调一起撤掉了（挂着不动的脸会一直
     * 骗人，还会因为姿态不是 `idle` 把自主漫步冻住）。现在所有表情都是限时的，
     * 播完必然回到这里。
     */
    const isAtRest = useCallback(() => poseNameRef.current === 'idle', []);

    /**
     * 回到基准姿态：忙碌中回到「看手机」循环，否则回 `idle`。
     *
     * 忙碌是 presence 级的常驻状态，优先级高于基准姿态：插播的一次性动作（说话、表情、
     * 触摸反应）播完必须回落到循环里。否则角色只要在忙碌期间说一句话，那个循环就被
     * 「回到基准」永久抹掉了——状态还在忙，人却已经站直了。
     */
    const returnToTone = useCallback((token?: number) => {
      if (token !== undefined && sequenceTokenRef.current !== token) return;
      poseTimerRef.current = null;
      walkFrameDelayMsRef.current = null;
      walkTargetFramesRef.current = null;
      if (busyPhaseRef.current === 'in' || busyPhaseRef.current === 'loop') {
        // 进场被打断（按住、或被插播动作顶掉）也落进循环：停在进场半路的一格上
        // 没有任何含义，看起来就是卡住了。
        busyPhaseRef.current = 'loop';
        const alreadyLooping = poseNameRef.current === BUSY_LOOP_SPEC.name;
        setActivePose(BUSY_LOOP_SPEC.name);
        // 已经在循环里就别再置 0：循环推进器按自己的节拍走，重复置帧会和它打架。
        if (!alreadyLooping) setFrame(0);
        onExpressionEnd?.();
        return;
      }
      setActivePose('idle');
      setFrame(IDLE_SLOT);
      onExpressionEnd?.();
    }, [onExpressionEnd, setActivePose]);

    /**
     * 逐帧播放一段序列；返回 false 表示被新的动作或用户按压打断。
     *
     * `onFrame` 让调用方跟住播放进度（忙碌进场用它记下「演到第几格」，半路退出就能
     * 从那一格往回倒）。它只在真正换帧时回调，不在被打断时补一次。
     */
    const playFrames = useCallback(async (
      spec: ChibiAnimationSpec,
      frames: number[],
      durations: number[],
      token: number,
      poseLabel: string,
      direction?: ChibiDirection,
      onFrame?: (index: number) => void,
    ): Promise<boolean> => {
      setActivePose(poseLabel);
      if (direction) setWalkDirection(direction);
      for (let index = 0; index < frames.length; index += 1) {
        if (sequenceTokenRef.current !== token || pressedRef.current) return false;
        onFrame?.(index);
        setFrame(frames[index]);
        await wait(durations[index] ?? durations[durations.length - 1] ?? frameDurationMs(spec, index));
      }
      return sequenceTokenRef.current === token && !pressedRef.current;
    }, [setActivePose]);

    /**
     * 进入忙碌：掏出手机，接上「看手机」循环，此后一直停在循环里。
     *
     * 循环自身的帧推进不在这里——`busy-loop` 是循环型帧序列，推进由循环推进器接管，
     * 这里只负责把姿态切过去。
     */
    const startBusy = useCallback(() => {
      const phase = busyPhaseRef.current;
      // 已经在忙碌里（进场中或循环中）就不重播：presence 可能把同一个状态重复下发。
      if (phase === 'in' || phase === 'loop') return;
      clearPoseTimer();
      // 若正走到退场半路，这一下把退场掐掉——状态又回到忙碌，收手机的动作不该继续演。
      const token = ++sequenceTokenRef.current;
      walkFrameDelayMsRef.current = null;
      walkTargetFramesRef.current = null;
      busyPhaseRef.current = 'in';
      busyInFrameRef.current = 0;
      void (async () => {
        await playFrames(
          BUSY_IN_SPEC,
          BUSY_IN_FRAMES,
          BUSY_IN_SPEC.durations,
          token,
          BUSY_IN_SPEC.name,
          undefined,
          (index) => { busyInFrameRef.current = index; },
        );
        // 被打断：姿态交给打断方；它若回落（returnToTone），自会落进循环。
        if (sequenceTokenRef.current !== token) return;
        busyPhaseRef.current = 'loop';
        setActivePose(BUSY_LOOP_SPEC.name);
      })();
    }, [clearPoseTimer, playFrames, setActivePose]);

    /** 退出忙碌：倒放「掏出手机」把手收回去，播完回落到基准姿态。 */
    const stopBusy = useCallback(() => {
      const phase = busyPhaseRef.current;
      if (phase === 'idle' || phase === 'out') return;
      busyPhaseRef.current = 'out';
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      walkFrameDelayMsRef.current = null;
      walkTargetFramesRef.current = null;
      // 进场还没演完就退场：从当前那一格往回倒，而不是先跳到最后一格再整段收一遍。
      const from = phase === 'in' ? busyInFrameRef.current - 1 : BUSY_IN_SPEC.frames - 1;
      const offset = Math.max(0, BUSY_IN_SPEC.frames - 1 - from);
      void (async () => {
        await playFrames(
          BUSY_IN_SPEC,
          BUSY_OUT.frames.slice(offset),
          BUSY_OUT.durations.slice(offset),
          token,
          BUSY_IN_SPEC.name,
        );
        if (sequenceTokenRef.current !== token) return;
        busyPhaseRef.current = 'idle';
        returnToTone(token);
      })();
    }, [clearPoseTimer, playFrames, returnToTone]);

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
      if (raw.trim().toLowerCase() === 'busy') {
        startBusy();
        // 忙碌的权威是 presence：这里不传时长就一直忙到 stopBusy。时长只是留给
        // 「主动指定一段忙碌」的调用方——和格位一样，给多久就持续多久。
        if (durationMs && durationMs > 0) {
          poseTimerRef.current = setTimeout(() => stopBusy(), durationMs);
        }
        return;
      }
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
        // hold 动作（如睡觉）播完停在末帧，不回基调。
        if (completed && !spec.hold) returnToTone(token);
      })();
    }, [clearPoseTimer, playFrames, returnToTone, setActivePose, startBusy, stopBusy]);

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

    /** 智能避让收尾回身：末帧起步倒放回正面，播完回落到 idle。 */
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
     * 这里只负责推进，腿和窗口收尾同时发生。
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
      // 停掉仍在进行的正向/倒放循环并回落到 idle（自然播完时姿态已是 idle，此处兜底）
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
        // 打断当前动作并回落到 idle。基准姿态就是 idle（曾经还有一层"心情基调"
        // 能把它换成别的格位，那条通道已随持续基调一起撤掉），所以直接回落即可。
        clearPoseTimer();
        sequenceTokenRef.current += 1;
        returnToTone();
      },
      startBusy,
      stopBusy,
      previewWalk,
      previewTurn,
      previewBlink,
      playTurn,
      playTurnBack,
      playWalk,
      startCast,
      cancelCast,
      stopCast,
    }), [applyMotion, cancelCast, clearPoseTimer, onScaleChange, previewBlink, previewTurn, previewWalk, returnToTone, startBusy, startCast, stopBusy, stopCast, playTurn, playTurnBack, playWalk]);

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
        // fleeInFlight 也在列：生气脸一播完桌宠就回到 idle，而逃离的位移可能还在跑
        // （逃离不播舞台动作，所以「姿态不是 idle」不再能替我们挡住这一趟）。
        if (
          cancelled || pressedRef.current || poseNameRef.current !== 'idle' ||
          positioningCoordinator.fullscreenHidden || positioningCoordinator.fullscreenInFlight ||
          positioningCoordinator.smartPositioningInFlight || positioningCoordinator.ambientMoveInFlight ||
          positioningCoordinator.fleeInFlight
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
      // 卸载即作废「逃离占着舞台」这件事，否则智能避让会被一条永远不会清的标志永久拦住
      positioningCoordinator.fleeInFlight = false;
    }, [clearPoseTimer]);

    /**
     * 取走并清掉待判定的单击定时器。
     *
     * 返回值就是「这一下本来在等第二下」——即这是一次双击。点击处理里三处都要问
     * 同一件事（要不要开快捷聊天、要不要按双击上报），所以只留一个取用口。
     */
    const takePendingClick = useCallback((): boolean => {
      if (!clickTimerRef.current) return false;
      clearTimeout(clickTimerRef.current);
      clickTimerRef.current = null;
      return true;
    }, []);

    /**
     * 用户是不是**抓住了**桌宠（而不是在连点）。
     *
     * 逃离的中止条件不能只看「手在不在上面」：连点的时候每一下都把手按上去，而滑动
     * 每 32ms 问一次，真手按一下 60~120ms——于是几乎每一帧都问到「有人按着」，整段
     * 位移在第一帧就被掐死（真机症状：生气脸照播、窗口一动不动）。点一下是逗它，
     * 按住才是抓它：只有持续按过 `FLEE_TAKEOVER_HOLD_MS` 才算把窗口抢回去。
     */
    const isHoldingPet = useCallback(
      () => pressAliveRef.current && Date.now() - pressStartedAtRef.current >= FLEE_TAKEOVER_HOLD_MS,
      [],
    );

    /**
     * 窗口是不是已经归用户了 —— 逃离据此让位。
     *
     * 两个信号缺一不可：
     * - `isHoldingPet`：手指按在**桌宠身上**够久。这是画布看得见的那一半。
     * - `dragInFlight`：App 已经把窗口拖起来了。窗口一旦滑开，光标就落到桌宠旁边的
     *   透明区上，再按下去 mousedown 走的是**背景层**、画布根本收不到——所以按住
     *   背景把人挤走这一半只能由 App 记在协调器上。少了它，位移会一边被拖动钉回
     *   原地、一边继续往前写，桌宠来回抖。
     */
    const userTookWindow = useCallback(
      () => isHoldingPet() || positioningCoordinator.dragInFlight,
      [isHoldingPet],
    );

    /**
     * 戳烦了的「逃离」：把窗口挪到远处一个随机落点。
     *
     * **只动窗口，不播任何舞台动作**（不转身、不迈步、不回身）——舞台上那套会盖掉
     * 生气脸，用户戳了四下想看的是「它生气了」，不该只看到一个背影。详见 `chibi/fleeTrack`。
     *
     * 编排本身在 `chibi/fleeTrack`（可单测），这里只负责三件事：占地（宣告窗口归我）、
     * 把真实环境接上、收尾时把地还回去。
     */
    const fleeFromTaps = useCallback(async () => {
      if (fleeingRef.current) return; // 一趟还没跑完，不叠第二趟
      // 全屏隐藏/正在隐藏时桌宠已经退到角落，再挪它会和隐藏动画抢窗口
      if (positioningCoordinator.fullscreenHidden || positioningCoordinator.fullscreenInFlight) {
        return;
      }
      fleeingRef.current = true;
      positioningCoordinator.fleeInFlight = true;
      // 避让可能正把窗口拖向别处：逃离要独占窗口，先把它按停，免得两个写手互相盖
      positioningCoordinator.abortSmartMove?.();

      const env: FleeEnv = {
        readGeometry: async () => {
          if (previewMode) return PREVIEW_FLEE_GEOMETRY;
          const handle = getCurrentWindow();
          const [position, size, monitor] = await Promise.all([
            handle.outerPosition(),
            handle.outerSize(),
            currentMonitor(),
          ]);
          if (!monitor) return null;
          return {
            fromX: position.x,
            fromY: position.y,
            windowWidth: size.width,
            monitorX: monitor.position.x,
            monitorWidth: monitor.size.width,
            marginPx: EDGE_MARGIN_PX,
          };
        },
        // 预览页没有窗口可移：时间轴照跑，但采样点记进 `window.__chibiFleeTrace__`、
        // 计划落点记进 `window.__chibiFleePlan__` 而不是下发给 Tauri —— 验收脚本据此
        // 断言「最后一个采样点就是 planFlee 抽出的那个落点」（掐死与跑完在这里是两回事）。
        //
        // **中止条件里刻意没有会话代号**（曾经有过，是错的）。真手点击会带几像素抖动：
        // mousedown 一按下后端拖动会话就启动，窗口随之动一点 → App 的 onMoved 给桌宠切
        // 上 `drag` 格位 → `applyMotion` 推进会话代号。拿代号当中止条件，等于**每次真实
        // 点击都把位移掐死在第一帧**（真机实测：带 3px 抖动的连点，窗口只跟手挪了 10px、
        // 一步没逃；去掉抖动则正常滑出 343px）。代号表达的是「姿态换了一张脸」，而不是
        // 「有人来抢窗口」——聊天反应切 `talk` 同理，都不该打断逃离。
        //
        // 中止条件只有 `userTookWindow`（按住够久 / 拖动会话已经开起来）与组件卸载。真正的
        // 「两个写手」由 `positioningCoordinator.fleeInFlight` 拦住：智能避让与自主漫步
        // 都得让路，App 那边也据此延后开启窗口拖动（见 handleBackgroundMouseDown）。
        slide: (from, to, durationMs, shouldAbort) => {
          if (previewMode) {
            window.__chibiFleePlan__ = {
              fromX: from.x, fromY: from.y, toX: to.x, toY: to.y, durationMs,
            };
          }
          return runSlide({
            fromX: from.x,
            fromY: from.y,
            toX: to.x,
            toY: to.y,
            durationMs,
            apply: previewMode
              ? (x, y) => {
                  (window.__chibiFleeTrace__ ??= []).push({ x, y, t: Math.round(performance.now()) });
                }
              : (x, y) => {
                  void invoke('set_window_position', { x, y }).catch(() => {});
                },
            shouldAbort,
          });
        },
        wait,
      };

      try {
        await runFlee(env, {
          minDistancePx: FLEE_MIN_DISTANCE_PX,
          maxDistancePx: FLEE_MAX_DISTANCE_PX,
          angerBeatMs: FLEE_ANGER_BEAT_MS,
          shouldAbort: userTookWindow,
        });
      } catch {
        // 逃离只是舞台调度：出错就安静收场，姿态交给下一个动作（同自主漫步）
      } finally {
        positioningCoordinator.fleeInFlight = false;
        fleeingRef.current = false;
      }
    }, [previewMode, userTookWindow]);

    /**
     * 一次点击。
     *
     * 四种结果按优先级排：气头刚点着（onset）→ 逃跑途中 → 双击 → 单击。
     *
     * onset 必须**在点击当场**出手（生气脸 + 逃离），不能等 230ms 的双击判定窗口：
     * 连点的时候每一下都落在 230ms 内，于是每一下都走进「等第二下」的分支、每一下都
     * 从第 0 帧重播生气脸——脸于是永远停在第 0 帧，看起来就是「等用户停手才开始播」。
     */
    const handleClick = () => {
      // 窗口发生过实际拖动时，mouseup 后浏览器仍可能补发 click；该 click 不应触发台词。
      if (Date.now() < suppressClickUntilRef.current) return;
      const { annoyed, onset } = tapLedgerRef.current.note(Date.now());
      const doubleClick = takePendingClick();

      // 逃跑途中不改动作：改动作会打断正在跑的位移，把桌宠扔在半路。语气照旧。
      if (fleeingRef.current) {
        onInteraction?.('rough_click');
        if (doubleClick) onOpenQuickChat?.();
        return;
      }

      // 气头刚点着：立刻给脸、立刻跑。双击语义照旧保留（这一下可能正是某一对里的第二下）
      if (onset) {
        applyMotion('angry');
        onInteraction?.('rough_click');
        if (doubleClick) onOpenQuickChat?.();
        void fleeFromTaps();
        return;
      }

      if (doubleClick) {
        // 生气脸正播着就不重播：同一口气上被反复打断在第 0 帧会看起来像卡住。
        // 被戳毛了的时候双击照样开聊天，但脸上的态度不再是配合。
        if (!(annoyed && poseNameRef.current === 'angry')) {
          applyMotion(annoyed ? 'angry' : 'talk', annoyed ? undefined : 700);
        }
        onInteraction?.(annoyed ? 'rough_click' : 'double_click');
        onOpenQuickChat?.();
        return;
      }

      clickTimerRef.current = setTimeout(() => {
        clickTimerRef.current = null;
        const reaction = annoyed ? 'angry' : pickTapReaction();
        // 空串 = 这一下不播表情：什么都不做，待机与自然眨眼照常继续。
        // 生气脸正播着同样不重播（理由同上，这条同时兜住键盘回车/空格那条路径）。
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
     * 帧序列图集可用时按帧序列取格；不可用时退回主图集的 `idle` 格位——帧序列的
     * `frame` 序号在主图集里没有意义，拿它去定位只会取到无关的一格。
     */
    const activeSpec = resolveMotion(poseName);
    const activeSheet =
      activeSpec.kind === 'animation'
        ? sheetUrl(activeSpec, characterId, walkDirection)
        : null;
    const spriteFrameStyle =
      activeSheet !== null && failedSheets.has(activeSheet)
        ? frameStyle(IDLE_SPEC, IDLE_SLOT, characterId, walkDirection)
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
              // 按下这件事先记下来，再谈它要不要打断当前姿态：逃离问的是「按了多久」，
              // 与这一按落在格位上还是帧序列上无关（见 pressAliveRef 的说明）。
              pressAliveRef.current = true;
              pressStartedAtRef.current = Date.now();
              const spec = getMotion(poseNameRef.current);
              // 忙碌阶段（掏出手机 / 看手机）虽然也是帧序列，但它表达的是常驻**状态**而不是
              // 一次性的表演：按住它不该把表演掐掉，可这一按仍要算一次正常点击——忙碌中的
              // 单击唤醒正是由 onModelClick 发起的。若把它并进下面「按住就打断帧序列」那条
              // 路，onModelClick 永远不会被调用，忙起来的桌宠就再也叫不醒了。
              const busyStage = isBusyStage(spec);
              if (spec?.kind === 'animation' && !busyStage) {
                // 气头上的生气脸不许抹掉：逃离途中桌上唯一会播的就是这张脸，抹掉它
                // 这次「戳毛了」就只剩一次没有表情的位移了。
                //
                // mousedown 早于 click，连点的时候每一下都先把帧序列清回待机，再由
                // click 从头重播——「等用户停手才开始播」的另一半原因就在这里。
                // 气头上一共也就 2.5s，这段时间里「按住就打断」的交互让位给「它正在气头上」。
                if (spec.name === 'angry' && tapLedgerRef.current.isAnnoyed(Date.now())) {
                  return;
                }
                sequenceTokenRef.current += 1;
                returnToTone();
                return;
              }
              // 忙碌阶段的帧序列不由点击作废：它归 presence 管。作废了 token 却没有任何一方
              // 接手，进场就会停在半路那一格上（循环推进器不认识 busy-in，不会来接）。
              if (!busyStage) sequenceTokenRef.current += 1;
              pressedRef.current = true;
              setPressed(true);
              onModelClick?.();
            }}
            onMouseUp={() => {
              pressAliveRef.current = false;
              pressedRef.current = false;
              setPressed(false);
            }}
            onMouseLeave={() => {
              pressAliveRef.current = false;
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
