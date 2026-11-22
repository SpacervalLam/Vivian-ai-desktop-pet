import { ReplyWaiting } from '../chibi/replyWaiting';
import { isBuiltinCharacterId } from '../characters';
import { CrossPresentation, type CrossPresentationEvent } from '../chibi/crossPresentation';
import { speechDisplayMotion } from '../chibi/speechPose';
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
import { currentMonitor, getCurrentWindow, getAllWindows } from '@tauri-apps/api/window';
import { getCharacterId } from '../characterContext';
import { useAppStore } from '../stores/useAppStore';
import { positioningCoordinator } from '../hooks/positioningCoordinator';
import { useDesktopPhysics } from '../hooks/useDesktopPhysics';
import {
  animation,
  CHIBI_CHARACTER_IDS,
  frameDurationMs,
  frameStyle,
  getMotion,
  IDLE_ACCENT,
  pose,
  resolveMotion,
  reversePlayback,
  sheetUrl,
  totalDurationMs,
  type ChibiAnimationSpec,
  type ChibiDirection,
  type ChibiMotionSpec,
} from '../chibi/motionRegistry';
import { defaultSizeStyle } from '../chibi/defaultSize';
import { bottomDockStyle } from '../chibi/dockSize';
import { planAmbientWalk, planSmartMove, walkCycleTravelPx } from '../chibi/walkPlan';
import { quickTurnPlayback, playWalkingFrames } from '../chibi/movementPlayback';
import { leanHoldMs, MIN_ROAM_TRAVEL_PX, planRoam, type ScreenInterestPoint } from '../chibi/roamPlan';
import { RoamRhythm } from '../chibi/roamRhythm';
import { runSustainedPose } from '../chibi/sustainedPose';
import { leanContactX, leanWallPositions, wallLeanOffsets } from '../chibi/wallLean';
import { ambientLane, type PetRect } from '../chibi/ambientLane';
import { peerApproachPosition, type PeerWorkArea } from '../chibi/peerApproach';
import { runSlide } from '../chibi/slideTrack';
import { dockClip, dockEntryPlayback, dockMotion, planScreenDock, type EdgeDockPlan, type DockSide } from '../chibi/edgeDock';
import { runFlee, FLEE_TAKEOVER_HOLD_MS, type FleeEnv } from '../chibi/fleeTrack';
import type { FleeGeometry } from '../chibi/fleePlan';
import { TapAngerLedger } from '../chibi/tapAnger';
import { SheetLoader } from '../chibi/sheetLoader';
import { optionalAssetSource } from '../utils/optionalResources';
import { IdleAccentRhythm } from '../chibi/idleAccentRhythm';
import './ChibiPetCanvas.css';

/**
 * 「等待回复」姿态的最长保持时间。
 *
 * 正常回复会在收到首字（chat:chunk）或终态事件（chat:done / chat:error / …）时退出思考。
 * 后端一旦卡死则不会有任何事件到达，超时后强制回落到静息姿态，避免桌宠永远循环思考动作。
 * 取值与 ChatController 的流式看门狗保持一致。
 */
const REPLY_THINKING_TIMEOUT_MS = 180_000;
const PHONE_ACTIVITY_MS = 8_000;
const PEER_CONTINUATION_GRACE_MS = 3_000;
const PEER_WALK_MAX_MS = 14_000;

interface PeerApproachEvent {
  stream_id: string;
  source_id: string;
  target_id: string;
  work_area: PeerWorkArea;
}

export interface PetInteractionMetrics {
  dragSessionId?: string;
  flung?: boolean;
  clickCount?: number;
  windowMs?: number;
  intervalMs?: number;
  pressDurationMs?: number;
  dragSpeedPxPerMs?: number;
  dragDistancePx?: number;
}

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

const TURN_SPEC = animation('turn');
const IDLE_SPEC = pose('idle');
const IDLE_SLOT = IDLE_SPEC.slot;
const TEND_SPEC = animation('tend');
const TEND_OUT_SPEC = animation('tend-out');
const SLEEP_WAKE_AFTER_MS = 5 * 60_000;

/**
 * 判断当前帧序列是否属于看手机动作（掏出手机 / 看手机）。
 *
 * 调用方决定何时结束动作，画布负责进场、循环和退场。
 */
function isPhoneAnimation(spec: ChibiMotionSpec | null): boolean {
  return spec?.kind === 'animation'
    && (spec.name === 'busy-in' || spec.name === 'busy-loop');
}

    /**
     * 一趟巡游至少要有这么多位移才值得迈步（px）。
     *
     * 距离采样本身在 `chibi/roamPlan`：非重力模式保持原来的对数取样，重力模式按
     * 巡逻/靠墙/闲逛三种意图给出目标。这里只保留「太短就不走」的下限。
     */
const WALK_DISTANCE_MIN_PX = MIN_ROAM_TRAVEL_PX;

/**
 * 屏幕兴趣点的刷新间隔（ms）。
 *
 * 本机全屏截图 + 亮度/边缘/彩度统计虽然便宜，也没必要每趟巡游都做一次：
 * 首次巡游抓一次，之后每 2–4 分钟更新；碰到别的捕获正在进行就 30s 后重试。
 */
const INTEREST_REFRESH_MIN_MS = 120_000;
const INTEREST_REFRESH_RANGE_MS = 120_000;
const INTEREST_BUSY_RETRY_MS = 30_000;

/** 桌宠与屏幕边缘保持的间隙（px）：贴着边站会显得被裁掉一半。 */
const EDGE_MARGIN_PX = 8;

const { frames: TURN_IN_FRAMES, durations: TURN_IN_DURATIONS } = quickTurnPlayback(TURN_SPEC.frames);
/** 回身 = 转身倒放（帧与节奏一起倒序）。 */
const TURN_OUT_FRAMES = [...TURN_IN_FRAMES].reverse();
const TURN_OUT_DURATIONS = [...TURN_IN_DURATIONS].reverse();

const BLINK_MIN_DELAY_MS = 3_200;
const BLINK_DELAY_RANGE_MS = 4_300;
/** 待机点缀被占用（说话/看手机/拖动/漫步/逃离）时的重试间隔（ms），远短于点缀本身间隔。 */
const IDLE_ACCENT_RETRY_MS = 1_000;
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
   * 播放 `busy` 动作：掏出手机并循环播放「看手机」，直到调用 {@link stopBusy}。
   * 进场或循环期间重复调用会延续当前帧序列。
   */
  startBusy: () => void;
  /**
   * 结束看手机动作：倒放「掏出手机」，再回到当前基准姿态。
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
  /** Load and start vertical hovering; returned cleanup only stops this movement. */
  startVerticalMove: () => Promise<(() => void) | null>;
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
  onResetScale?: () => void;
  /**
   * 一次点击手势的结果：单击、双击，或「戳烦了」（{@link TAP_ANNOY_THRESHOLD}）。
   * 前两者由点击节奏区分，`rough_click` 由画布内的戳烦了账本判定，三者互斥。
   */
  onInteraction?: (interaction: ChibiInteraction, metrics?: PetInteractionMetrics) => void;
  onOpenQuickChat?: () => void;
  /** Kept compatible with ModelCanvas while stage walking is coordinated above the renderer. */
  ambientMotionEnabled?: boolean;
  desktopPhysicsEnabled?: boolean;
  gameMode?: boolean;
  /** Browser-only visual QA route; avoids subscribing to unavailable Tauri events. */
  previewMode?: boolean;
}

function normalizeCharacterId(): string {
  const id = (getCharacterId() ?? 'vivian').toLowerCase();
  return CHIBI_CHARACTER_IDS.includes(id) ? id : 'vivian';
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
      onResetScale,
      onInteraction,
      onOpenQuickChat,
      ambientMotionEnabled = true,
      desktopPhysicsEnabled = false,
      gameMode = false,
      previewMode = false,
    },
    ref,
  ) {
    const characterId = normalizeCharacterId();
    const WALK_SPEC = animation('walk', characterId);
    const BLINK_SPEC = animation('blink', characterId);
    const CAST_SPEC = animation('cast', characterId);
    const BUSY_IN_SPEC = animation('busy-in', characterId);
    const BUSY_LOOP_SPEC = animation('busy-loop', characterId);
    const BUSY_IN_FRAMES = Array.from({ length: BUSY_IN_SPEC.frames }, (_, i) => i);
    const BUSY_OUT = reversePlayback(BUSY_IN_SPEC);
    const BLINK_FRAMES = Array.from({ length: BLINK_SPEC.frames }, (_, i) => i);
    const SLEEP_SPEC = animation('sleep', characterId);
    const WAKE_SPEC = animation('wake', characterId);
    const stageRef = useRef<HTMLDivElement | null>(null);
    const leanOffsetRef = useRef<{ token: number; offsets: number[] } | null>(null);
    const physicsLayerRef = useRef<HTMLDivElement | null>(null);
    const physics = useDesktopPhysics(desktopPhysicsEnabled && !previewMode, physicsLayerRef);
    const [poseName, setPoseName] = useState<ChibiPose>('idle');
    const poseNameRef = useRef<ChibiPose>('idle');
    const waitingForReplyRef = useRef(false);
    const thinkingExitRef = useRef<{ motion: string; durationMs?: number } | null>(null);
    const crossPresentationRef = useRef(new CrossPresentation(characterId));
    const peerFacingRef = useRef<{ peer: string; direction: ChibiDirection; token: number; shown: number } | null>(null);
    const [, setCrossVersion] = useState(0);
    const [audioPlaying, setAudioPlaying] = useState(false);
    const [fling, setFling] = useState({ generation: 0, active: false });
    const flingActiveRef = useRef(false);
    flingActiveRef.current = fling.active;
    const presenceMode = useAppStore(state => state.presenceState);
    const presenceModeRef = useRef(presenceMode);
    presenceModeRef.current = presenceMode;
    // 贴边由后端几何裁决，与 presence 的对话语义正交。
    const dockState = useAppStore(state => state.dockState);
    const restingOnDesktop = dockState === 'Off' && presenceMode === 'rest';
    const offline = presenceMode === 'offline';
    /** 用函数读 store 而非局部 ref：TS 会把同一函数体里已检查过的值窄化成常量。 */
    const dockOff = () => useAppStore.getState().dockState === 'Off';
    const edgeSettledRef = useRef(false);
    const dockPlanRef = useRef<EdgeDockPlan | null>(null);
    const requestedDockRef = useRef<DockSide | undefined>();
    /** 主动入场（Docking）指定的边；不设 requestedDockRef 以复用 enterEdge 的走路路径。 */
    const dockSideRef = useRef<DockSide | undefined>();
    /**
     * 后端 dock 层的当前 epoch。所有位置 / region 写入都必须带上它，
     * 这样 undock 之后仍在飞行的动画采样会被 Rust 侧静默丢弃。
     */
    const dockEpochRef = useRef(0);
    const [dockRevision, setDockRevision] = useState(0);
    const [edgeClip, setEdgeClip] = useState<string | undefined>();
    const visibleSpeech = useAppStore(state => !!state.currentBubble?.trim()
      || state.settledBubbles.some(bubble => !!bubble.text.trim()));
    const speakingRef = useRef(false);
    speakingRef.current = !previewMode && (audioPlaying || visibleSpeech);
    const [walkDirection, setWalkDirection] = useState<ChibiDirection>('left');
    const walkDirectionRef = useRef<ChibiDirection>('left');
    walkDirectionRef.current = walkDirection;
    const [sheetLoader] = useState(() => new SheetLoader());
    useEffect(() => { sheetLoader.setCapacity(gameMode ? 1 : 2); }, [gameMode, sheetLoader]);
    const [sheetVersion, setSheetVersion] = useState(0);
    useEffect(() => {
      if (gameMode && poseName === 'idle') sheetLoader.clear(true);
    }, [gameMode, poseName, sheetLoader]);
    const idleSheet = characterId === 'nana' ? null : optionalAssetSource(`/chibi/motion/${characterId}/${characterId}-idle-sheet.webp`);
    useEffect(() => {
      if (!idleSheet) return;
      let cancelled = false;
      void sheetLoader.pin(idleSheet).then(() => {
        if (!cancelled) setSheetVersion(version => version + 1);
      });
      return () => { cancelled = true; };
    }, [idleSheet, sheetLoader]);
    const [frame, setFrame] = useState(0);
    /** 智能避让指定的走动帧间隔（ms）；为 null 时按图集固有节奏播放。 */
    const walkFrameDelayMsRef = useRef<number | null>(null);
    /** 智能避让播放的走动帧数；为 null 时按固有节奏持续循环。 */
    const walkTargetFramesRef = useRef<number | null>(null);
    const [pressed, setPressed] = useState(false);
    const pressedRef = useRef(false);
    const tapFeedbackRef = useRef<HTMLDivElement>(null);
    const tapFeedbackAnimationRef = useRef<Animation | null>(null);
    useEffect(() => () => { tapFeedbackAnimationRef.current?.cancel(); }, []);

    // Animate a separate layer so tap feedback composes with expression/idle transforms.
    const playTapFeedback = () => {
      tapFeedbackAnimationRef.current?.cancel();
      if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return;
      const layer = tapFeedbackRef.current;
      if (!layer) return;
      tapFeedbackAnimationRef.current = layer.animate([
        { transform: 'translateY(0) scale(1, 1)', offset: 0 },
        { transform: 'translateY(1%) scale(1.08, .88)', offset: .16 },
        { transform: 'translateY(-1.5%) scale(.96, 1.07)', offset: .40 },
        { transform: 'translateY(.4%) scale(1.03, .97)', offset: .64 },
        { transform: 'translateY(-.2%) scale(.99, 1.02)', offset: .82 },
        { transform: 'translateY(0) scale(1, 1)', offset: 1 },
      ], { duration: 420, easing: 'cubic-bezier(.22,.7,.35,1)' });
    };
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
    const recentTapTimesRef = useRef<number[]>([]);
    const lastPressDurationRef = useRef<number | undefined>(undefined);
    /** 一趟逃离是否还在跑：期间点击一律不改动作，免得把正在跑的位移打断在半路。 */
    const fleeingRef = useRef(false);
    const scaleRef = useRef(1);
    const scaleHoldRef = useRef<ReturnType<typeof setTimeout> | null>(null);
    const scalePressRef = useRef(false);
    const scalePressPositionRef = useRef({ x: 0, y: 0 });
    const cancelScaleHold = useCallback(() => {
      if (scaleHoldRef.current !== null) clearTimeout(scaleHoldRef.current);
      scaleHoldRef.current = null;
    }, []);
    useEffect(() => {
      const release = () => {
        cancelScaleHold();
        if (scalePressRef.current) suppressClickUntilRef.current = Date.now() + 300;
        scalePressRef.current = false;
      };
      const keyUp = (event: KeyboardEvent) => { if (event.key === 'Control') cancelScaleHold(); };
      window.addEventListener('mouseup', release);
      window.addEventListener('blur', release);
      window.addEventListener('keyup', keyUp);
      return () => {
        cancelScaleHold();
        window.removeEventListener('mouseup', release);
        window.removeEventListener('blur', release);
        window.removeEventListener('keyup', keyUp);
      };
    }, [cancelScaleHold]);
    const suppressClickUntilRef = useRef(0);
    const phoneAnimationPhaseRef = useRef<'idle' | 'in' | 'loop' | 'out'>('idle');
    // Keep channel activity independent of the current animation and its pose timer.
    const phoneTimerRef = useRef<number | null>(null);
    /** 进场动画播到第几帧。半路退出时据此决定倒放起点。 */
    const phoneEntryFrameRef = useRef(0);
    const sleepPhaseRef = useRef<'idle' | 'in' | 'loop' | 'out'>('idle');
    const wakeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
    const afterWakeRef = useRef<(() => void) | undefined>();
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
      // Commit the new sheet and its initial index together. Waiting for the
      // loop effect to reset it briefly paints the previous action's frame.
      if (poseNameRef.current !== next) setFrame(0);
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
    const isAtRest = useCallback(() => poseNameRef.current === 'idle' && !speakingRef.current &&
      !crossPresentationRef.current.motion && !positioningCoordinator.peerConversationInProgress, []);

    /**
     * 回到基准姿态：等待首字时回「思考中」，看手机期间回循环，否则回 `idle`。
     *
     * 看手机期间插播的一次性动作（说话、表情、触摸反应）播完后回到看手机循环。
     */
    const returnToTone = useCallback((token?: number) => {
      if (token !== undefined && sequenceTokenRef.current !== token) return;
      poseTimerRef.current = null;
      walkFrameDelayMsRef.current = null;
      walkTargetFramesRef.current = null;
      if (presenceModeRef.current === 'rest') {
        setActivePose(SLEEP_SPEC.name); setFrame(SLEEP_SPEC.frames - 1); return;
      }
      if (!dockOff() && edgeSettledRef.current) {
        const spec = animation(dockMotion(dockPlanRef.current?.side ?? 'right'), characterId);
        setActivePose(spec.name); setFrame(spec.frames - 1); return;
      }
      if (waitingForReplyRef.current) {
        const alreadyThinking = poseNameRef.current === 'thinking';
        setActivePose('thinking');
        if (!alreadyThinking) setFrame(0);
        onExpressionEnd?.();
        return;
      }
      if (phoneAnimationPhaseRef.current === 'in' || phoneAnimationPhaseRef.current === 'loop') {
        // 进场被打断（按住、或被插播动作顶掉）也落进循环：停在进场半路的一格上
        // 没有任何含义，看起来就是卡住了。
        phoneAnimationPhaseRef.current = 'loop';
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
    }, [characterId, onExpressionEnd, setActivePose]);

    /**
     * 逐帧播放一段序列；返回 false 表示被新的动作或用户按压打断。
     *
     * `onFrame` 让调用方跟住播放进度（掏出手机时用它记下「演到第几格」，半路退出就能
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
      shouldAbort?: () => boolean,
    ): Promise<boolean> => {
      const facing = direction ?? walkDirectionRef.current;
      const stopped = () => sequenceTokenRef.current !== token || pressedRef.current || shouldAbort?.();
      // Keep the next phase ready before the current phase ends. Otherwise a
      // cold walk/work sheet briefly renders the front-facing idle fallback.
      const nextSpec = spec.name === TURN_SPEC.name ? WALK_SPEC
        : spec.name === BUSY_IN_SPEC.name ? BUSY_LOOP_SPEC : null;
      const ready = await Promise.all([
        sheetLoader.load(sheetUrl(spec, characterId, facing)),
        nextSpec ? sheetLoader.load(sheetUrl(nextSpec, characterId, facing)) : Promise.resolve(true),
      ]);
      if (ready.some(value => !value) || stopped()) return false;
      setActivePose(poseLabel);
      if (direction) setWalkDirection(direction);
      let deadline = performance.now();
      for (let index = 0; index < frames.length; index += 1) {
        if (stopped()) return false;
        onFrame?.(index);
        setFrame(frames[index]);
        deadline += durations[index] ?? durations[durations.length - 1] ?? frameDurationMs(spec, index);
        await wait(Math.max(0, deadline - performance.now()));
      }
      return !stopped();
    }, [characterId, setActivePose, sheetLoader]);

    // 新事件在起身完成后接管；连续消息只保留最新动作，不重播起身。
    const wakeUp = useCallback((after?: () => void): boolean => {
      if (presenceModeRef.current === 'rest') return false;
      if (sleepPhaseRef.current === 'idle') return false;
      if (after) afterWakeRef.current = after;
      if (sleepPhaseRef.current === 'out') return true;
      sleepPhaseRef.current = 'out';
      if (wakeTimerRef.current) clearTimeout(wakeTimerRef.current);
      wakeTimerRef.current = null;
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      void (async () => {
        await playFrames(WAKE_SPEC, Array.from({ length: WAKE_SPEC.frames }, (_, i) => i),
          WAKE_SPEC.durations, token, WAKE_SPEC.name);
        if (sequenceTokenRef.current !== token) return;
        sleepPhaseRef.current = 'idle';
        const next = afterWakeRef.current;
        afterWakeRef.current = undefined;
        returnToTone(token);
        next?.();
      })();
      return true;
    }, [characterId, clearPoseTimer, playFrames, returnToTone]);

    const startIdleRoutine = useCallback((routine: 'sleep' | 'tend' = characterId === 'nana' ? 'tend' : 'sleep', persistent = false) => {
      if (sleepPhaseRef.current !== 'idle' || phoneAnimationPhaseRef.current !== 'idle'
        || pressedRef.current || positioningCoordinator.fleeInFlight) return;
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      const spec = routine === 'sleep' ? SLEEP_SPEC : TEND_SPEC;
      if (routine === 'sleep') sleepPhaseRef.current = 'in';
      void (async () => {
        const completed = await playFrames(spec, Array.from({ length: spec.frames }, (_, i) => i),
          spec.durations, token, spec.name);
        if (!completed) return;
        if (routine === 'tend') {
          const ended = await playFrames(TEND_OUT_SPEC,
            Array.from({ length: TEND_OUT_SPEC.frames }, (_, i) => i),
            TEND_OUT_SPEC.durations, token, TEND_OUT_SPEC.name);
          if (ended) returnToTone(token);
          return;
        }
        sleepPhaseRef.current = 'loop';
        if (!persistent) wakeTimerRef.current = setTimeout(() => wakeUp(), SLEEP_WAKE_AFTER_MS);
        if (SLEEP_SPEC.hold) {
          setFrame(SLEEP_SPEC.frames - 1);
          return;
        }
        while (sequenceTokenRef.current === token && sleepPhaseRef.current === 'loop') {
          if (!await playFrames(SLEEP_SPEC, [SLEEP_SPEC.frames - 2, SLEEP_SPEC.frames - 1],
            [900, 900], token, SLEEP_SPEC.name)) break;
        }
      })();
    }, [characterId, clearPoseTimer, playFrames, returnToTone, wakeUp]);

    /**
     * 播放 `busy` 动作：掏出手机，接上「看手机」循环。
     *
     * 循环自身的帧推进不在这里——`busy-loop` 是循环型帧序列，推进由循环推进器接管，
     * 这里只负责把姿态切过去。
     */
    const startBusy = useCallback((onLoop?: () => void, shouldStart?: () => boolean) => {
      if (shouldStart && !shouldStart()) return;
      if (wakeUp(() => startBusy(onLoop, shouldStart))) return;
      const phase = phoneAnimationPhaseRef.current;
      // 进场或循环期间继续当前动作。
      if (phase === 'in' || phase === 'loop') {
        if (phase === 'loop') onLoop?.();
        return;
      }
      clearPoseTimer();
      // 正在收手机时，新一次看手机动作会打断退场。
      const token = ++sequenceTokenRef.current;
      walkFrameDelayMsRef.current = null;
      walkTargetFramesRef.current = null;
      phoneAnimationPhaseRef.current = 'in';
      phoneEntryFrameRef.current = 0;
      void (async () => {
        await playFrames(
          BUSY_IN_SPEC,
          BUSY_IN_FRAMES,
          BUSY_IN_SPEC.durations,
          token,
          BUSY_IN_SPEC.name,
          undefined,
          (index) => { phoneEntryFrameRef.current = index; },
          () => phoneAnimationPhaseRef.current !== 'in',
        );
        // 被打断：姿态交给打断方；它若回落（returnToTone），自会落进循环。
        if (sequenceTokenRef.current !== token || phoneAnimationPhaseRef.current !== 'in') return;
        phoneAnimationPhaseRef.current = 'loop';
        setActivePose(BUSY_LOOP_SPEC.name);
        onLoop?.();
      })();
    }, [characterId, clearPoseTimer, playFrames, setActivePose, wakeUp]);

    /** 结束看手机动作，可选择立即切换姿态或先播放收手机的动作。 */
    const stopBusy = useCallback((immediate = false) => {
      const phase = phoneAnimationPhaseRef.current;
      if (phase === 'idle' || (phase === 'out' && !immediate)) return;
      const showingPhone = isPhoneAnimation(resolveMotion(poseNameRef.current, characterId));
      if (immediate || !showingPhone) {
        phoneAnimationPhaseRef.current = 'idle';
        if (showingPhone) {
          clearPoseTimer();
          sequenceTokenRef.current += 1;
          returnToTone();
        }
        return;
      }
      phoneAnimationPhaseRef.current = 'out';
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      walkFrameDelayMsRef.current = null;
      walkTargetFramesRef.current = null;
      // 进场还没演完就退场：从当前那一格往回倒，而不是先跳到最后一格再整段收一遍。
      const from = phase === 'in' ? phoneEntryFrameRef.current - 1 : BUSY_IN_SPEC.frames - 1;
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
        phoneAnimationPhaseRef.current = 'idle';
        returnToTone(token);
      })();
    }, [characterId, clearPoseTimer, playFrames, returnToTone]);

    const startPhoneIfActive = useCallback(() => {
      startBusy(undefined, () => phoneTimerRef.current !== null && dockOff() && !dockPlanRef.current
        && presenceModeRef.current !== 'rest' && presenceModeRef.current !== 'offline');
    }, [startBusy]);

    /** 倚靠帧按手掌标记对齐墙面；退出时倒放起身。 */
    const beginLean = useCallback((holdMs = leanHoldMs(), wall?: ChibiDirection,
      onHold?: () => void, onLeave?: () => void) => {
      const spec = resolveMotion('lean', characterId);
      if (spec.kind !== 'animation') return null;
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      const out = reversePlayback(spec);
      let leftWall = false;
      const interrupted = () => sequenceTokenRef.current !== token || pressAliveRef.current || pressedRef.current ||
        positioningCoordinator.dragInFlight || speakingRef.current || !!crossPresentationRef.current.motion ||
        positioningCoordinator.peerConversationInProgress ||
        positioningCoordinator.fullscreenHidden || positioningCoordinator.fullscreenInFlight ||
        positioningCoordinator.smartPositioningInFlight || positioningCoordinator.physicsInFlight ||
        (!previewMode && !dockOff()) || leftWall;
      const done = (async () => {
        let direction = wall;
        let unlisten: UnlistenFn | undefined;
        try {
          if (!previewMode) {
            const win = getCurrentWindow();
            const [position, size, monitor] = await Promise.all([win.outerPosition(), win.outerSize(), currentMonitor()]);
            if (!monitor || interrupted()) return false;
            const walls = leanWallPositions(monitor, size.width);
            direction ??= position.x === walls.left ? 'left' : 'right';
            if (position.x !== walls[direction]) return false;
            unlisten = await win.onMoved(({ payload }) => { leftWall ||= payload.x !== position.x; });

            const source = sheetUrl(spec, characterId, direction);
            if (!await sheetLoader.load(source) || (await win.outerPosition()).x !== position.x || interrupted()) return false;
            const stageWidth = stageRef.current?.getBoundingClientRect().width;
            if (!stageWidth) return false;
            const windowWidth = size.width / monitor.scaleFactor;
            const stageLeft = (windowWidth - stageWidth) / 2;
            const edge = direction === 'left' ? 0 : windowWidth;
            const target = (edge - stageLeft) * 100 / stageWidth;
            const offsets = wallLeanOffsets(spec, characterId, direction, target);
            if (!offsets) return false;
            leanOffsetRef.current = { token, offsets };
          }
          return await runSustainedPose(holdMs, {
            enter: () => playFrames(spec, Array.from({ length: spec.frames }, (_, i) => i), spec.durations,
              token, spec.name, direction, undefined, interrupted),
            leave: () => playFrames(spec, out.frames, out.durations, token, spec.name, direction, undefined, interrupted),
            interrupted, now: () => performance.now(), wait, onHold, onLeave,
          });
        } catch (error) {
          console.warn('[chibi] Wall lean failed', error);
          return false;
        } finally {
          unlisten?.();
          if (leanOffsetRef.current?.token === token) leanOffsetRef.current = null;
        }
      })();
      return { token, done };
    }, [characterId, clearPoseTimer, playFrames, previewMode, sheetLoader]);

    /**
     * 应用一个动作名。
     *
     * 图集格位是可持续的姿态：给了时长就按时长回落。帧序列分两类——循环型（走动）
     * 的帧推进由循环推进器负责，这里只管入场与回落时长；一次性动作自带节奏，
     * 播完即回落，不受外部时长影响（调用方传 3000ms 只是想表达"持续一会儿"）。
     */
    const applyMotion = useCallback((raw: string, durationMs?: number) => {
      if (presenceModeRef.current === 'rest') return;
      if (!dockOff() && dockPlanRef.current) return;
      const name = raw.trim().toLowerCase();
      if (peerFacingRef.current?.token === sequenceTokenRef.current
        && !['drag', 'dizzy', 'thinking', 'sleep', 'wake'].includes(name)) return;
      if (phoneTimerRef.current !== null && phoneAnimationPhaseRef.current !== 'idle'
        && !['drag', 'dizzy', 'wake'].includes(name)) return;
      if (poseNameRef.current === 'thinking'
        && !['drag', 'dizzy', 'sleep', 'wake'].includes(name)) {
        if (name === 'thinking') {
          // A new request takes over without restarting the current thought frame.
          thinkingExitRef.current = null;
        } else if (!waitingForReplyRef.current) {
          // Keep the latest reply gesture until the thought animation has left naturally.
          thinkingExitRef.current = { motion: raw, durationMs };
        }
        return;
      }
      thinkingExitRef.current = null;
      if (name === 'sleep' || (name === 'tend' && characterId === 'nana')) {
        startIdleRoutine(name as 'sleep' | 'tend');
        return;
      }
      if (name === 'wake') { wakeUp(); return; }
      if (wakeUp(() => applyMotion(raw, durationMs))) return;
      // 角色专属动作不请求另一角色不存在的图集。
      if (characterId === 'vivian' && (name === 'tend' || name === 'tend-out')) return;
      if (name === 'lean') {
        const lean = beginLean(durationMs && durationMs > 0 ? durationMs : undefined);
        if (!lean) return;
        void lean.done.then(() => {
          if (!positioningCoordinator.dragInFlight) returnToTone(lean.token);
        });
        return;
      }
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      if (raw.trim().toLowerCase() === 'busy') {
        startBusy();
        // 调用方可为看手机动作指定时长，默认循环至 stopBusy。
        if (durationMs && durationMs > 0) {
          poseTimerRef.current = setTimeout(() => stopBusy(), durationMs);
        }
        return;
      }
      const spec = resolveMotion(raw, characterId);
      if (spec.name === 'busy-loop') {
        startBusy(() => {
          poseTimerRef.current = setTimeout(() => stopBusy(), durationMs && durationMs > 0 ? durationMs : totalDurationMs(BUSY_LOOP_SPEC));
        });
        return;
      }
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
        // Reserve reply-waiting immediately; its frame timer waits for loading
        // below, while incoming text must still see the thinking state.
        if (spec.name === 'thinking') { setActivePose(spec.name); return; }
        const enterLoop = () => {
          const holdMs = durationMs && durationMs > 0 ? durationMs : totalDurationMs(spec);
          if (spec.name === 'walk') {
            const frameDelayMs = totalDurationMs(spec) / spec.frames;
            const frames = Math.max(1, Math.round(holdMs / totalDurationMs(spec))) * spec.frames;
            walkTargetFramesRef.current = frames;
            walkFrameDelayMsRef.current = frameDelayMs;
            setActivePose(spec.name);
            void playWalkingFrames({ frames, frameDelayMs, cycleFrames: spec.frames,
              show: setFrame, interrupted: () => sequenceTokenRef.current !== token || pressedRef.current,
            }).then(async completed => {
              if (!completed) return;
              const returned = await playFrames(TURN_SPEC, TURN_OUT_FRAMES, TURN_OUT_DURATIONS, token, TURN_SPEC.name);
              if (returned) returnToTone(token);
            });
            return;
          }
          setActivePose(spec.name);
          poseTimerRef.current = setTimeout(() => {
            returnToTone(token);
          }, holdMs);
        };
        const source = sheetUrl(spec, characterId, walkDirectionRef.current);
        if (spec.name !== 'walk' && sheetLoader.isReady(source)) { enterLoop(); return; }
        void (async () => {
          await sheetLoader.load(source);
          if (sequenceTokenRef.current !== token || pressedRef.current) return;
          if (spec.name === 'walk' && poseNameRef.current !== 'walk') {
            if (!await playFrames(TURN_SPEC, TURN_IN_FRAMES, TURN_IN_DURATIONS, token, TURN_SPEC.name)) return;
          }
          enterLoop();
        })();
        return;
      }
      const frames = Array.from({ length: spec.frames }, (_, index) => index);
      // Single-frame illustrations should remain visible for the requested interaction duration.
      const durations = spec.frames === 1 && durationMs && durationMs > 0 ? [durationMs] : spec.durations;
      void (async () => {
        const completed = await playFrames(spec, frames, durations, token, spec.name);
        // hold 动作（如睡觉）播完停在末帧，不回基调。
        if (completed && !spec.hold) returnToTone(token);
      })();
    }, [beginLean, characterId, clearPoseTimer, playFrames, returnToTone, setActivePose, sheetLoader, startBusy, stopBusy, startIdleRoutine, wakeUp]);

    /** 原地演示转身（不移动窗口）。 */
    const previewTurn = useCallback((direction: ChibiDirection) => {
      if (wakeUp(() => previewTurn(direction))) return;
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      void playFrames(TURN_SPEC, TURN_IN_FRAMES, TURN_IN_DURATIONS, token, TURN_SPEC.name, direction)
        .then(async completed => {
          if (!completed) return;
          const returned = await playFrames(TURN_SPEC, TURN_OUT_FRAMES, TURN_OUT_DURATIONS, token, TURN_SPEC.name);
          if (returned) returnToTone(token);
        });
    }, [clearPoseTimer, playFrames, returnToTone, wakeUp]);

    /** 原地演示走一圈：转身入场 → 走动 → 转身离场。 */
    const previewWalk = useCallback((direction: ChibiDirection) => {
      if (wakeUp(() => previewWalk(direction))) return;
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      walkFrameDelayMsRef.current = null;
      walkTargetFramesRef.current = null;
      void (async () => {
        const turned = await playFrames(
          TURN_SPEC,
          TURN_IN_FRAMES,
          TURN_IN_DURATIONS,
          token,
          TURN_SPEC.name,
          direction,
        );
        if (!turned) return;
        walkTargetFramesRef.current = WALK_SPEC.frames;
        setActivePose(WALK_SPEC.name);
        const walked = await playWalkingFrames({
          frames: WALK_SPEC.frames, frameDelayMs: totalDurationMs(WALK_SPEC) / WALK_SPEC.frames,
          cycleFrames: WALK_SPEC.frames, show: setFrame,
          interrupted: () => sequenceTokenRef.current !== token || pressedRef.current,
        });
        if (!walked) return;
        const returned = await playFrames(
          TURN_SPEC,
          TURN_OUT_FRAMES,
          TURN_OUT_DURATIONS,
          token,
          TURN_SPEC.name,
        );
        if (returned) returnToTone(token);
      })();
    }, [clearPoseTimer, playFrames, returnToTone, setActivePose, wakeUp]);

    const previewBlink = useCallback(() => {
      if (wakeUp(() => previewBlink())) return;
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      void playFrames(BLINK_SPEC, BLINK_FRAMES, BLINK_SPEC.durations, token, BLINK_SPEC.name)
        .then((completed) => {
          if (completed) returnToTone(token);
        });
    }, [clearPoseTimer, playFrames, returnToTone, wakeUp]);

    const playTurn = useCallback(async (direction: ChibiDirection): Promise<boolean> => {
      if (wakeUp()) return false;
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      // 不 returnToTone：停在转身末帧（侧身），由调用方接走动或回正。
      return playFrames(
        TURN_SPEC,
        TURN_IN_FRAMES,
        TURN_IN_DURATIONS,
        token,
        TURN_SPEC.name,
        direction,
      );
    }, [clearPoseTimer, playFrames, wakeUp]);

    /** 智能避让收尾回身：末帧起步倒放回正面，播完回落到 idle。 */
    const playTurnBack = useCallback(async (): Promise<boolean> => {
      if (wakeUp()) return false;
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
    }, [clearPoseTimer, playFrames, returnToTone, wakeUp]);

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
      if (wakeUp()) return { token: sequenceTokenRef.current, done: Promise.resolve(false) };
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      walkTargetFramesRef.current = frames > 0 ? frames : null;
      walkFrameDelayMsRef.current = frameDelayMs > 0 ? frameDelayMs : null;
      setWalkDirection(direction);
      setActivePose(WALK_SPEC.name);
      setFrame(0);
      const done = playWalkingFrames({
        frames: Math.max(1, frames), frameDelayMs: Math.max(1, frameDelayMs), cycleFrames: WALK_SPEC.frames,
        show: setFrame, interrupted: () => sequenceTokenRef.current !== token || pressedRef.current,
      });
      return { token, done };
    }, [characterId, clearPoseTimer, setActivePose, wakeUp]);

    const playWalk = useCallback(async (
      direction: ChibiDirection,
      frames: number,
      frameDelayMs: number,
    ): Promise<boolean> => beginWalk(direction, frames, frameDelayMs).done, [beginWalk]);

    const startVerticalMove = useCallback(async (): Promise<(() => void) | null> => {
      if (wakeUp()) return null;
      clearPoseTimer();
      const token = ++sequenceTokenRef.current;
      const spec = animation('float', characterId);
      const ready = await sheetLoader.load(sheetUrl(spec, characterId, walkDirectionRef.current));
      if (!ready || sequenceTokenRef.current !== token || pressedRef.current) return null;
      walkFrameDelayMsRef.current = null;
      walkTargetFramesRef.current = null;
      thinkingExitRef.current = null;
      setFrame(0);
      setActivePose(spec.name);
      return () => {
        if (sequenceTokenRef.current !== token) return;
        sequenceTokenRef.current += 1;
        returnToTone();
      };
    }, [characterId, clearPoseTimer, returnToTone, setActivePose, sheetLoader, wakeUp]);

    const peerMotionBlocked = useCallback(() => {
      const coordinator = positioningCoordinator;
      return pressAliveRef.current || coordinator.dragInFlight || coordinator.fleeInFlight ||
        flingActiveRef.current || coordinator.gameMode || coordinator.fullscreenHidden ||
        coordinator.fullscreenInFlight || !dockOff() || presenceModeRef.current === 'rest' ||
        presenceModeRef.current === 'offline';
    }, []);

    const facePeer = useCallback(async (peer: string, direction: ChibiDirection, cancelled: () => boolean, sideOn = false, react = false) => {
      const current = peerFacingRef.current;
      if (current?.peer === peer && current.direction === direction && current.token === sequenceTokenRef.current) return;
      const interrupted = () => cancelled() || peerMotionBlocked();
      if (interrupted() || ['drag', 'dizzy', 'cast'].includes(poseNameRef.current) || wakeUp()) return;
      const spec = animation('turn', characterId);
      const { frames, durations } = quickTurnPlayback(spec.frames);
      clearPoseTimer();
      const facing = { peer, direction, token: ++sequenceTokenRef.current, shown: -1 };
      peerFacingRef.current = facing;
      const start = sideOn ? frames.length - 1 : 0;
      if (react && Math.random() < 0.5) {
        const puzzled = animation('think', characterId);
        await playFrames(puzzled, Array.from({ length: puzzled.frames }, (_, index) => index),
          puzzled.durations, facing.token, puzzled.name, undefined, interrupted);
      }
      if (!interrupted() && await playFrames(spec, frames.slice(start), durations.slice(start), facing.token, spec.name,
        direction, index => { facing.shown = start + index; }, interrupted)) return;
      if (peerFacingRef.current === facing) {
        peerFacingRef.current = null;
        if (!pressAliveRef.current && !positioningCoordinator.dragInFlight) returnToTone(facing.token);
      }
    }, [characterId, clearPoseTimer, peerMotionBlocked, playFrames, returnToTone, wakeUp]);

    const stopFacingPeer = useCallback((animate: boolean) => {
      const facing = peerFacingRef.current;
      peerFacingRef.current = null;
      if (!facing || facing.token !== sequenceTokenRef.current) return;
      const token = ++sequenceTokenRef.current;
      if (pressAliveRef.current || positioningCoordinator.dragInFlight) return;
      if (!animate || facing.shown < 0 || peerMotionBlocked()) {
        returnToTone(token);
        return;
      }
      const spec = animation('turn', characterId);
      const { frames, durations } = quickTurnPlayback(spec.frames);
      void playFrames(spec, frames.slice(0, facing.shown + 1).reverse(), durations.slice(0, facing.shown + 1).reverse(),
        token, spec.name, facing.direction, undefined, peerMotionBlocked).then(() => {
        if (!pressAliveRef.current && !positioningCoordinator.dragInFlight) returnToTone(token);
      });
    }, [characterId, peerMotionBlocked, playFrames, returnToTone]);

    const walkToPeer = useCallback(async (event: PeerApproachEvent, cancelled: () => boolean) => {
      const coordinator = positioningCoordinator;
      const interrupted = () => cancelled() || peerMotionBlocked();
      if (interrupted()) return;

      // Give both windows a chance to stop their current autonomous movement before sampling.
      await wait(100);
      for (let i = 0; i < 12 && !interrupted() && (coordinator.ambientMoveInFlight || coordinator.smartPositioningInFlight); i++) {
        await wait(50);
      }
      if (interrupted() || coordinator.ambientMoveInFlight || coordinator.smartPositioningInFlight) return;
      const windows = (await getAllWindows()).filter(win => CHIBI_CHARACTER_IDS.includes(win.label));
      const rectangles = await Promise.all(windows.map(async (win): Promise<PetRect | null> => {
        try {
          if (!await win.isVisible()) return null;
          const [position, size] = await Promise.all([win.outerPosition(), win.outerSize()]);
          return { id: win.label, x: position.x, y: position.y, width: size.width, height: size.height };
        } catch { return null; }
      }));
      const peers = rectangles.filter((rect): rect is PetRect => rect !== null);
      const from = peers.find(pet => pet.id === characterId);
      const target = peers.find(pet => pet.id === event.target_id);
      if (!from || !target || interrupted()) return;
      const to = peerApproachPosition(from, target, peers, event.work_area);
      if (!to || Math.hypot(to.x - from.x, to.y - from.y) <= 3 * event.work_area.scale) return;

      const dx = to.x - from.x;
      const plan = planSmartMove(dx, to.y - from.y, walkCycleTravelPx(characterId, dx > 0 ? 'right' : 'left'));
      let poseToken = sequenceTokenRef.current;
      let stopVertical: (() => void) | null = null;
      let walk: ReturnType<typeof beginWalk> | undefined;
      let durationMs = plan.durationMs;
      try {
        if (plan.walking) {
          const turning = playTurn(plan.direction);
          poseToken = sequenceTokenRef.current;
          if (!await turning || interrupted()) return;
          const cycleMs = WALK_SPEC.frames * plan.frameDelayMs;
          const frames = Math.min(plan.frames, Math.floor(PEER_WALK_MAX_MS / cycleMs) * WALK_SPEC.frames);
          walk = beginWalk(plan.direction, frames, plan.frameDelayMs);
          poseToken = walk.token;
          durationMs = frames * plan.frameDelayMs;
        } else if (to.y !== from.y) {
          const starting = startVerticalMove();
          poseToken = sequenceTokenRef.current;
          stopVertical = await starting;
        }
        const stopped = () => interrupted() || sequenceTokenRef.current !== poseToken;
        if (stopped()) return;
        const arrived = await runSlide({
          fromX: from.x, fromY: from.y, toX: to.x, toY: to.y, durationMs,
          linear: plan.walking, apply: (x, y) => invoke('set_window_position', { x, y }),
          shouldAbort: stopped,
        });
        if (arrived && walk && await walk.done && !stopped()) {
          const direction = target.x + target.width / 2 < to.x + from.width / 2 ? 'left' : 'right';
          await facePeer(event.target_id, direction, interrupted, direction === plan.direction);
        }
      } finally {
        stopVertical?.();
        if (sequenceTokenRef.current === poseToken && !pressAliveRef.current && !coordinator.dragInFlight) {
          sequenceTokenRef.current += 1;
          returnToTone();
        }
      }
    }, [WALK_SPEC.frames, beginWalk, characterId, facePeer, peerMotionBlocked, playTurn, returnToTone, startVerticalMove]);

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
      if (wakeUp(() => startCast(durationMs))) return;
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
    }, [characterId, clearPoseTimer, returnToTone, setActivePose, stopCast, wakeUp]);

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
        const spec = resolveMotion(name, characterId);
        if (['remind', 'tired', 'umbrella', 'music', 'clipboard'].includes(spec.name) && (pressedRef.current
          || phoneAnimationPhaseRef.current !== 'idle' || positioningCoordinator.fleeInFlight
          || positioningCoordinator.ambientMoveInFlight || positioningCoordinator.dragInFlight || positioningCoordinator.physicsInFlight)) return;
        if (waitingForReplyRef.current && !['drag', 'dizzy'].includes(spec.name)) return;
        if (spec.kind === 'animation' && spec.directions) {
          setWalkDirection(/right|east|右/i.test(name) ? 'right' : 'left');
        }
        if (spec.name === 'drag') suppressClickUntilRef.current = Date.now() + 500;
        applyMotion(name, durationMs);
      },
      playMotion: (group) => {
        if (waitingForReplyRef.current) return;
        const spec = resolveMotion(group, characterId);
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
        if (wakeUp(() => {})) return;
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
      startVerticalMove,
      startCast,
      cancelCast,
      stopCast,
    }), [applyMotion, cancelCast, clearPoseTimer, onScaleChange, previewBlink, previewTurn, previewWalk, returnToTone, startBusy, startCast, stopBusy, stopCast, playTurn, playTurnBack, playWalk, startVerticalMove, wakeUp]);

    // Docking moves the window outside the selected monitor. The clip also
    // hides the body when another monitor occupies the neighbouring space.
    useEffect(() => {
      if (previewMode) return;
      let cancelled = false;
      phoneAnimationPhaseRef.current = 'idle';
      const moveDock = async (plan: EdgeDockPlan, x: number, y: number) => {
        setEdgeClip(dockClip(x, plan, y));
        await invoke('set_pet_dock_position', {x, y, monitorLeft: plan.left, monitorRight: plan.right,
          monitorTop: plan.top, monitorBottom: plan.bottom, epoch: dockEpochRef.current});
      };
      /** 贴边请求不成立（窗口已不在边缘）时退回完整模式，不播放任何贴边流程。 */
      const revertToFullMode = () => {
        dockPlanRef.current = null;
        requestedDockRef.current = undefined;
        dockSideRef.current = undefined;
        edgeSettledRef.current = false;
        setEdgeClip(undefined);
        void invoke('clear_pet_dock_clip').catch(() => {});
        useAppStore.getState().setDockState('Off');
      };
      /** 桌宠的窗口是否真的压在指定边缘之外（贴边流程的硬前置条件）。 */
      const requestSideAtEdge = (side: DockSide, x: number, y: number, width: number, height: number,
        monitor: {position: {x: number; y: number}; size: {width: number; height: number}}) => {
        const left = monitor.position.x;
        const right = monitor.position.x + monitor.size.width;
        const bottom = monitor.position.y + monitor.size.height;
        return side === 'left' ? x <= left
          : side === 'right' ? x + width >= right
          : y + height >= bottom;
      };
      let entering = false;
      const enterEdge = async () => {
        if (edgeSettledRef.current || entering) return;
        // 只认 dock:changed 带来的 epoch；没有 epoch 说明后端还没裁决，
        // 不能从屏幕中间自行开始贴边流程。
        if (dockEpochRef.current === 0) return;
        entering = true;
        try {
          const win = getCurrentWindow();
          const [position, size, monitor] = await Promise.all([win.outerPosition(), win.outerSize(), currentMonitor()]);
          if (!monitor || cancelled || dockOff()) return;
          const scale = monitor.scaleFactor;
          const stageElement = stageRef.current;
          const stage = stageElement?.getBoundingClientRect();
          const edge = animation('edge', characterId);
          const dock = dockPlanRef.current ?? planScreenDock(position, size,
            {left:monitor.position.x,right:monitor.position.x+monitor.size.width,
              top:monitor.position.y,bottom:monitor.position.y+monitor.size.height,scale},
            {left:stage?.left ?? 0,top:stageElement?.offsetTop ?? 0,width:stage?.width ?? size.width/scale,height:stage?.height ?? size.height/scale},
            requestedDockRef.current ?? dockSideRef.current, side => leanContactX(edge, characterId, side));
          // 贴边流程的前提是桌宠此刻确实压在边缘上。窗口在请求与开工之间被挪回屏幕内
          // （松手后的物理回弹、别人抢先移动等）时，宁可退回完整模式，也不从屏幕中间
          // 开始播贴边动作。
          if (requestedDockRef.current && !requestSideAtEdge(dock.side, position.x, position.y,
            size.width, size.height, monitor)) {
            revertToFullMode();
            return;
          }
          const horizontal = dock.side === 'left' || dock.side === 'right';
          const facing: ChibiDirection = dock.side === 'left' ? 'left' : 'right';
          const spec = horizontal ? edge : animation(dockMotion(dock.side), characterId);
          const distance = horizontal ? dock.walkX-position.x : dock.walkY-position.y;
          const plan = planAmbientWalk(distance, undefined,
            horizontal ? walkCycleTravelPx(characterId, distance > 0 ? 'right' : 'left') : undefined);
          positioningCoordinator.ambientMoveInFlight = true;
          if (plan.walking && !requestedDockRef.current) {
            let stopVertical: (() => void) | null = null;
            try {
              let walked: Promise<boolean> | undefined;
              if (horizontal) {
                if (!await playTurn(facing) || cancelled) return;
                walked = beginWalk(facing, plan.frames, plan.frameDelayMs).done;
              } else { stopVertical = await startVerticalMove(); }
              const slid = await runSlide({fromX:position.x,fromY:position.y,toX:dock.walkX,toY:dock.walkY,
                durationMs:plan.durationMs,linear:plan.walking,apply:(x,y)=>invoke('set_dock_walk_position',{x,y,epoch:dockEpochRef.current}),
                shouldAbort:()=>cancelled || pressedRef.current || positioningCoordinator.dragInFlight || dockOff()});
              if (!slid || (walked && !await walked) || cancelled) return;
            } finally { stopVertical?.(); }
          }
          dockPlanRef.current = dock;
          await sheetLoader.load(sheetUrl(spec, characterId, facing));
          if (cancelled || dockOff()) return;
          const token = ++sequenceTokenRef.current;
          const {approach, settled, slideDuration} = dockEntryPlayback(spec, dock.side);
          const interrupted = () => cancelled || dockOff() || sequenceTokenRef.current !== token
            || pressedRef.current || positioningCoordinator.dragInFlight;
          const [animated, slid] = await Promise.all([
            playFrames(spec, approach.frames, approach.durations, token, spec.name, facing, undefined, interrupted),
            runSlide({fromX: requestedDockRef.current ? position.x : dock.walkX, fromY: requestedDockRef.current ? position.y : dock.walkY, toX: dock.dockX, toY: dock.dockY,
              durationMs: slideDuration, apply: (x, y) => moveDock(dock, x, y),
              shouldAbort: interrupted}),
          ]);
          // Slides can finish late when native position updates are busy; keep the last approach frame until arrival.
          if (!animated || !slid || interrupted()) return;
          if (settled.frames.length && !await playFrames(
            spec, settled.frames, settled.durations, token, spec.name, facing, undefined, interrupted)) return;
          edgeSettledRef.current = true;
          dockSideRef.current = undefined;
          setFrame(spec.frames - 1);
          // 落位结算：入场期间窗口越界量可能先变小（推得比贴边位姿更深时动画要把
          // 窗口收回来），后端据此才把「离开边缘」的阈值定在最终位姿上。
          void invoke('settle_pet_dock', { epoch: dockEpochRef.current }).catch(() => {});
        } catch (error) { console.warn('[presence] edge movement failed', error); }
        finally { entering = false; positioningCoordinator.ambientMoveInFlight = false; }
      };
      // 恢复完整窗口是后端的一次几何裁决：SetWindowPos 时已清掉 Win32 region，
      // 窗口停在用户手里的位置。前端只撤 CSS clip，绝不写窗口位置——否则被
      // 说话/待机打断的滑入动画会和用户抢窗口，又把刚拉起来的桌宠送回边缘。
      const settleVisibleState = async () => {
        if (cancelled || !dockOff()) return;
        await invoke('clear_pet_dock_clip').catch(() => {});
        setEdgeClip(undefined);
      };
      const enterVisibleState = async () => {
        const dock = dockPlanRef.current;
        edgeSettledRef.current = false;
        // 用户正把贴边桌宠拖离边缘：窗口已经跟手移动，退场动画与「拖回贴边位姿」
        // 都会和拖动抢窗口。只撤掉贴边姿态与裁剪，停在用户手里。
        if (positioningCoordinator.dragInFlight) {
          dockPlanRef.current = null;
          requestedDockRef.current = undefined;
          await invoke('clear_pet_dock_clip').catch(() => {});
          setEdgeClip(undefined);
          return;
        }
        try {
          if (dock && !cancelled) {
            const token = ++sequenceTokenRef.current;
            const spec = animation(dockMotion(dock.side), characterId);
            const {approach} = dockEntryPlayback(spec, dock.side);
            // 只倒放贴边精灵：窗口位置由后端几何裁决决定，前端不再写。
            await playFrames(spec, approach.frames.reverse(), approach.durations.reverse(),
              token, spec.name, dock.side === 'left' ? 'left' : 'right');
          }
        } finally {
          // 仍要贴边（例如取消退场后又收到贴边请求）时保留裁剪，交由下一次 enterEdge 接管。
          if (!cancelled && dockOff()) {
            dockPlanRef.current = null;
            requestedDockRef.current = undefined;
            await settleVisibleState();
          }
        }
        if (cancelled) return;
        if (restingOnDesktop) {
          clearPoseTimer(); sequenceTokenRef.current += 1; sleepPhaseRef.current = 'idle';
          startIdleRoutine('sleep', true);
        } else if (phoneTimerRef.current !== null && presenceModeRef.current !== 'offline') {
          startPhoneIfActive();
        } else if (!wakeUp()) { sequenceTokenRef.current += 1; returnToTone(); }
      };
      if (dockState !== 'Off') {
        if (!wakeUp(() => { void enterEdge(); })) void enterEdge();
      } else { void enterVisibleState().catch(console.warn); }
      return () => {
        cancelled = true;
        sequenceTokenRef.current += 1;
        // `dock:changed` raises this flag before React re-runs the effect.
        // If the dock sequence is cancelled, bails out, or its wake-up animation
        // is invalidated by the token bump above, the flag would otherwise stay
        // raised forever and the physics loop would remain hard-paused (gravity
        // silently stops). Ownership of the flag always returns to the physics loop.
        positioningCoordinator.ambientMoveInFlight = false;
      };
    }, [dockState, dockRevision, restingOnDesktop, previewMode, beginWalk, playTurn, playFrames, wakeUp, startIdleRoutine, clearPoseTimer, returnToTone, startVerticalMove, startPhoneIfActive, sheetLoader, characterId]);

    // A hidden window can show a pending phone animation once it returns.
    useEffect(() => {
      if (!previewMode && !offline) startPhoneIfActive();
    }, [offline, previewMode, startPhoneIfActive]);

    // 仅加载当前动作；等待或缺图时维持主图集，避免空白精灵。
    useEffect(() => {
      const spec = resolveMotion(poseName, characterId);
      if (spec.kind !== 'animation') return;
      const source = sheetUrl(spec, characterId, walkDirection);
      let cancelled = false;
      void sheetLoader.load(source).then((ready) => {
        if (cancelled) return;
        setSheetVersion((version) => version + 1);
        if (!ready) console.warn(`[ChibiPetCanvas] 动作图集加载失败，已回落主图集: ${source}`);
      });
      return () => { cancelled = true; };
    }, [characterId, poseName, walkDirection, sheetLoader]);

    useEffect(() => {
      return () => {
        sequenceTokenRef.current += 1;
        if (wakeTimerRef.current) clearTimeout(wakeTimerRef.current);
        sleepPhaseRef.current = 'idle';
        afterWakeRef.current = undefined;
        sheetLoader.clear();
      };
    }, [characterId, sheetLoader]);

    // 循环型帧序列（走动、看手机、思考中）自推进；一次性动作由 applyMotion 自行播完。
    useEffect(() => {
      const spec = getMotion(poseName, characterId);
      if (!spec || spec.kind !== 'animation' || !spec.loop) {
        return undefined;
      }
      // Loading is not playback time: the first visible frame must be frame 0.
      if (!sheetLoader.isReady(sheetUrl(spec, characterId, walkDirection))) return undefined;
      if (spec.name === 'walk' && walkTargetFramesRef.current != null) return undefined;
      setFrame(0);
      let cancelled = false;
      let timer: number | null = null;
      const frameDelay = walkFrameDelayMsRef.current;
      const targetFrames = walkTargetFramesRef.current;
      const thinking = spec.name === 'thinking';
      const phoneLoop = spec.name === 'busy-loop' && characterId !== 'claude';
      const thinkingLoopStart = characterId === 'nana' ? 2 : 3;
      const advance = (current: number, shown: number, direction = 1) => {
        // 指定了帧数时播满即停。帧数由位移时长与帧间隔推导（不保证是周期的整数倍），
        // 停在哪一格都有可能；紧随其后的回身动画会立即接管这一格。
        if (targetFrames != null && shown >= targetFrames) return;
        timer = window.setTimeout(() => {
          if (cancelled) return;
          if (spec.name === 'walk' && walkTargetFramesRef.current != null) return;
          if (thinking && thinkingExitRef.current && current === spec.frames - 1) {
            const nextMotion = thinkingExitRef.current;
            thinkingExitRef.current = null;
            setActivePose('idle');
            applyMotion(nextMotion.motion, nextMotion.durationMs);
            return;
          }
          // One-based loop bounds: Vivian 4..9, Nana 3..9.
          // Play the intro, then ping-pong without repeating endpoints.
          if (thinking && !thinkingExitRef.current) {
            if (current >= Math.min(8, spec.frames - 2)) direction = -1;
            else if (current <= thinkingLoopStart) direction = 1;
          }
          // 看手机只有八张近似关键帧；往返播放避免末帧直接跳回首帧。
          // 端点的停顿由词表时长控制，不重复端点，也不影响走路等顺序循环。
          if (phoneLoop) {
            if (current >= spec.frames - 1) direction = -1;
            else if (current <= 0) direction = 1;
          }
          const next = thinking
            ? current + (thinkingExitRef.current ? 1 : direction)
            : phoneLoop ? current + direction : (current + 1) % spec.frames;
          setFrame(next);
          advance(next, shown + 1, direction);
        }, frameDelay ?? frameDurationMs(spec, current));
      };
      advance(0, 0);
      return () => {
        cancelled = true;
        if (timer !== null) window.clearTimeout(timer);
      };
    }, [poseName, characterId, walkDirection, sheetVersion, sheetLoader, applyMotion, setActivePose]);

    // 自然眨眼：只在待机且未被按压时插入，偶尔连眨两下
    useEffect(() => {
      if (gameMode) return;
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
    }, [characterId, gameMode, isAtRest, playFrames, returnToTone]);

    // 待机点缀：静息时按间隔插播一次循环小动作（如四处张望）。
    //
    // 张望独立于自主漫步开关：游戏模式、非在线状态也允许空闲时播放。
    // 只避开正在说话、已有姿态或按压拖动，不打断睡眠和用户交互。
    useEffect(() => {
      if (!IDLE_ACCENT || previewMode) return undefined;
      const { motion, intervalSecs, probability, durationMs } = IDLE_ACCENT;
      const rhythm = new IdleAccentRhythm(intervalSecs * 1000, performance.now());
      let cancelled = false;
      let timer: number | null = null;
      const arm = (delayMs: number) => {
        if (cancelled) return;
        if (timer !== null) window.clearTimeout(timer);
        timer = window.setTimeout(() => {
          timer = null;
          if (cancelled) return;
          const free = presenceModeRef.current === 'online' && isAtRest() && !pressAliveRef.current && !positioningCoordinator.dragInFlight;
          const now = performance.now();
          if (rhythm.ready(now, free)) {
            if (Math.random() < probability) {
              const choices = [motion, 'look', 'happy', 'smug', 'blink', 'lean'];
              const selected = choices[Math.floor(Math.random() * choices.length)];
              applyMotion(selected, selected === 'lean' ? undefined : durationMs);
            }
            rhythm.played(now, durationMs);
          }
          arm(IDLE_ACCENT_RETRY_MS);
        }, delayMs);
      };
      arm(IDLE_ACCENT_RETRY_MS);
      return () => {
        cancelled = true;
        if (timer !== null) window.clearTimeout(timer);
      };
    }, [applyMotion, isAtRest, previewMode]);

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

      // 贴边链路的唯一前端入口。origin 决定入场方式：
      // drag / tool+Docked 从当前位置直接滑出，tool+Docking 先走到边缘，
      // entry 只是几何达标收尾（不动 store，避免重跑动画），Off 是恢复完整窗口。
      void add<{character_id: string; mode: 'Off' | 'Docking' | 'Docked'; side?: DockSide;
        epoch: number; origin?: 'drag' | 'tool' | 'entry'}>('dock:changed', p => {
        if (!mine(p.character_id) || !Number.isFinite(p.epoch)) return;
        const store = useAppStore.getState();
        if (p.mode === 'Off') {
          // 后端已推进 epoch；此后到达的旧动画采样会被 Rust 侧 accept() 丢弃。
          dockEpochRef.current = 0;
          dockPlanRef.current = null;
          requestedDockRef.current = undefined;
          dockSideRef.current = undefined;
          edgeSettledRef.current = false;
          setEdgeClip(undefined);
          void invoke('clear_pet_dock_clip').catch(() => {});
          store.setDockState('Off');
          return;
        }
        dockEpochRef.current = p.epoch;
        if (p.mode === 'Docked' && p.origin === 'entry') {
          dockSideRef.current = undefined;
          return;
        }
        const side = ['left','right','bottom'].includes(p.side ?? '') ? p.side : undefined;
        if (p.mode === 'Docking') {
          // 不设 requestedDockRef：enterEdge 才会走「先走到 walkX」的分支。
          dockSideRef.current = side;
          requestedDockRef.current = undefined;
        } else {
          requestedDockRef.current = side;
          dockSideRef.current = undefined;
        }
        dockPlanRef.current = null;
        edgeSettledRef.current = false;
        positioningCoordinator.dragInFlight = false;
        positioningCoordinator.ambientMoveInFlight = true;
        pressedRef.current = false; pressAliveRef.current = false; setPressed(false);
        setEdgeClip(undefined);
        // 旧贴边区域可能在拖动/换边期间残留，先清掉；新 enterEdge 会按新位置重新裁剪。
        void invoke('clear_pet_dock_clip').catch(() => {});
        store.setDockState(p.mode);
        setDockRevision(revision => revision + 1);
      });
      const mayShowSpeechFeedback = () => !waitingForReplyRef.current && !pressAliveRef.current &&
        !positioningCoordinator.dragInFlight && speechDisplayMotion(poseNameRef.current, true, false) === 'talk';
      void add<{ character_id: string; generation: number; active: boolean }>('drag:fling', p => {
        if (!mine(p.character_id) || !Number.isSafeInteger(p.generation) || p.generation <= 0) return;
        setFling(previous => {
          if (p.generation < previous.generation
            || (p.generation === previous.generation && !previous.active)) return previous;
          return { generation: p.generation, active: p.active };
        });
      });
      void add<{ character_id?: string }>('tts:started', (p) => {
        if (mine(p.character_id)) setAudioPlaying(true);
        // Audio is a display overlay; it must not cancel dragging, sleep or walking.
      });
      void add<{ character_id?: string }>('tts:finished', (p) => {
        if (mine(p.character_id)) setAudioPlaying(false);
        if (mine(p.character_id) && mayShowSpeechFeedback()) applyMotion('happy', 700);
      });
      void add<{ character_id?: string }>('tts:error', (p) => {
        if (mine(p.character_id)) setAudioPlaying(false);
        if (mine(p.character_id) && mayShowSpeechFeedback()) applyMotion('idle');
      });
      void add<{ speaker_id?: string }>('presentation:stop', p => {
        if (mine(p.speaker_id)) setAudioPlaying(false);
      });
      const waiting = new ReplyWaiting();
      const cross = crossPresentationRef.current;
      const meetings = new Map<string, {
        peer: string;
        initiator: boolean;
        preparing: boolean;
        approaching: boolean;
        ended: boolean;
        continuing: boolean;
        pendingSpeech: Set<string>;
        timer: number;
      }>();
      const meetingWith = (peer: string) => [...meetings.values()].some(meeting => meeting.peer === peer);
      const removeMeeting = (id: string, animate = true) => {
        const meeting = meetings.get(id);
        if (!meeting) return;
        window.clearTimeout(meeting.timer);
        meetings.delete(id);
        positioningCoordinator.peerConversationInProgress = meetings.size > 0;
        if (peerFacingRef.current?.peer === meeting.peer && !meetingWith(meeting.peer)) {
          stopFacingPeer(animate);
        }
      };
      const clearMeetings = (animate = true) => { for (const id of meetings.keys()) removeMeeting(id, animate); };
      const settleMeeting = (id: string) => {
        const meeting = meetings.get(id);
        if (!meeting?.ended || meeting.pendingSpeech.size > 0) return;
        if (!meeting.continuing) {
          removeMeeting(id);
          return;
        }
        window.clearTimeout(meeting.timer);
        meeting.timer = window.setTimeout(() => removeMeeting(id), PEER_CONTINUATION_GRACE_MS);
      };
      void add<PeerApproachEvent>('cross:approach', p => {
        if (p.source_id !== characterId && p.target_id !== characterId) return;
        if (meetings.has(p.stream_id)) return;
        const initiator = p.source_id === characterId;
        const meeting = {
          peer: initiator ? p.target_id : p.source_id,
          initiator,
          preparing: false,
          approaching: initiator && meetings.size === 0,
          ended: false,
          continuing: false,
          pendingSpeech: new Set([p.source_id, p.target_id]),
          timer: window.setTimeout(() => removeMeeting(p.stream_id), REPLY_THINKING_TIMEOUT_MS),
        };
        meetings.set(p.stream_id, meeting);
        positioningCoordinator.peerConversationInProgress = true;
        positioningCoordinator.abortSmartMove?.();
        if (!initiator) return;
        void (async () => {
          try {
            if (meeting.approaching) await walkToPeer(p, () =>
              !meeting.approaching || meetings.get(p.stream_id) !== meeting);
          } catch (error) {
            console.warn('[chibi] Peer approach failed', error);
          } finally {
            meeting.approaching = false;
            await getCurrentWindow().emit('cross:approach-complete', { stream_id: p.stream_id, character_id: characterId }).catch(() => {});
          }
        })();
      });
      void add<{ stream_id: string; character_id: string }>('cross:speech-finished', p => {
        if (meetings.get(p.stream_id)?.pendingSpeech.delete(p.character_id)) settleMeeting(p.stream_id);
      });
      let crossWatchdog: number | null = null;
      const clearCrossWatchdog = () => {
        if (crossWatchdog !== null) window.clearTimeout(crossWatchdog);
        crossWatchdog = null;
      };
      const refreshCross = () => {
        setCrossVersion(version => version + 1);
        clearCrossWatchdog();
        if (cross.motion) crossWatchdog = window.setTimeout(() => {
          crossWatchdog = null;
          cross.clear();
          setCrossVersion(version => version + 1);
        }, REPLY_THINKING_TIMEOUT_MS);
      };
      void add<{ stream_id: string }>('cross:approach-cancelled', p => {
        removeMeeting(p.stream_id);
        if (cross.finish(p.stream_id)) refreshCross();
      });
      type ReplyEvent = { character_id?: string; stream_id?: string; text?: string; channel?: string };
      // 思考看门狗：进入「等待回复」后若长时间没有任何终态事件，强制退出思考循环。
      // 后端卡死（抢不到 think_lock、或 think 内部死锁）时不会有任何 chat:* 事件到达，
      // 没有这个兜底，桌宠会永远循环思考动作。
      let thinkingWatchdog: number | null = null;
      const clearThinkingWatchdog = () => {
        if (thinkingWatchdog !== null) {
          window.clearTimeout(thinkingWatchdog);
          thinkingWatchdog = null;
        }
      };
      const updateWaiting = () => {
        const next = waiting.thinking;
        if (next === waitingForReplyRef.current) return;
        waitingForReplyRef.current = next;
        if (next) {
          applyMotion('thinking');
          clearThinkingWatchdog();
          thinkingWatchdog = window.setTimeout(() => {
            thinkingWatchdog = null;
            waiting.clear();
            updateWaiting();
          }, REPLY_THINKING_TIMEOUT_MS);
        } else {
          clearThinkingWatchdog();
          if (poseNameRef.current === 'thinking') applyMotion('idle');
        }
      };
      const clearPhoneTimer = () => {
        if (phoneTimerRef.current === null) return;
        window.clearTimeout(phoneTimerRef.current);
        phoneTimerRef.current = null;
      };
      void add<{ character_id: string; channel: string }>('chat:channel_activity', p => {
        if (p.character_id?.toLowerCase() !== characterId) return;
        if (p.channel === 'direct' || p.channel === 'broadcast') {
          clearPhoneTimer();
          stopBusy(true);
          return;
        }
        if (p.channel !== 'wechat') return;
        clearMeetings();
        cross.clear();
        refreshCross();
        waiting.clear();
        updateWaiting();
        clearPhoneTimer();
        phoneTimerRef.current = window.setTimeout(() => {
          phoneTimerRef.current = null;
          stopBusy();
        }, PHONE_ACTIVITY_MS);
        startPhoneIfActive();
      });
      const startReply = (p: ReplyEvent) => {
        if (!mine(p.character_id) || p.channel === 'wechat') return;
        // Direct user conversation takes over; late peer chunks cannot reclaim it.
        clearMeetings();
        cross.clear();
        refreshCross();
        waiting.start(p.stream_id ?? '');
        updateWaiting();
      };
      void add<ReplyEvent>('chat:waiting', startReply);
      void add<ReplyEvent>('chat:start', startReply);
      void add<ReplyEvent>('chat:chunk', (p) => {
        if (!mine(p.character_id) || p.channel === 'wechat' || !waiting.text(p.stream_id ?? '', p.text ?? '')) return;
        updateWaiting();
        applyMotion('talk');
      });
      const finishReply = (p: ReplyEvent, success = false) => {
        if (!mine(p.character_id) || p.channel === 'wechat') return;
        waiting.finish(p.stream_id ?? '');
        updateWaiting();
        if (success && !waiting.thinking && mayShowSpeechFeedback()) applyMotion('happy', 850);
      };
      void add<ReplyEvent>('chat:done', p => finishReply(p, true));
      for (const event of ['chat:error', 'chat:cancelled', 'chat:config_error', 'chat:presence_blocked', 'chat:yielded', 'chat:waiting-ended']) {
        void add<ReplyEvent>(event, p => finishReply(p));
      }
      void add<CrossPresentationEvent>('cross:start', p => {
        if (!cross.update(p)) return;
        refreshCross();
      });
      void add<{ stream_id: string }>('cross:prepare-listener', ({ stream_id: stream }) => {
        const meeting = meetings.get(stream);
        if (!meeting || meeting.preparing) return;
        meeting.preparing = true;
        meeting.approaching = false;
        void (async () => {
          const self = getCurrentWindow();
          const peer = (await getAllWindows()).find(win => win.label === meeting.peer);
          if (!peer || !await peer.isVisible()) return;
          const [ownPosition, ownSize, peerPosition, peerSize] = await Promise.all([
            self.outerPosition(), self.outerSize(), peer.outerPosition(), peer.outerSize(),
          ]);
          const offset = peerPosition.x + peerSize.width / 2 - (ownPosition.x + ownSize.width / 2);
          if (Math.abs(offset) < 1) return;
          await facePeer(meeting.peer, offset < 0 ? 'left' : 'right', () => !meetingWith(meeting.peer), false, !meeting.initiator);
        })().catch(error => console.warn('[chibi] Could not face peer', error)).finally(() => {
          if (!meeting.initiator) {
            void getCurrentWindow().emit('cross:approach-complete', { stream_id: stream, character_id: characterId }).catch(() => {});
          }
        });
      });
      void add<CrossPresentationEvent>('cross:chunk', p => {
        if (cross.update(p, true)) refreshCross();
      });
      for (const event of ['cross:done', 'cross:error']) {
        void add<CrossPresentationEvent & { conv_state?: string; should_continue?: boolean }>(event, p => {
          const finished = cross.finish(p.stream_id);
          if (finished) refreshCross();
          if (!p.stream_id) return;
          if (event === 'cross:error' || p.conv_state === 'preempted') {
            removeMeeting(p.stream_id);
            return;
          }
          const meeting = meetings.get(p.stream_id);
          if (meeting) {
            meeting.ended = true;
            meeting.continuing = p.should_continue === true;
            settleMeeting(p.stream_id);
          }
        });
      }
      void add<{ character_id: string; online: boolean }>('character:online_changed', p => {
        if (!p.online) {
          if (mine(p.character_id)) {
            cross.clear();
            clearMeetings();
          } else {
            cross.removePeer(p.character_id);
            for (const [id, meeting] of meetings) if (meeting.peer === p.character_id) removeMeeting(id);
          }
          refreshCross();
        }
      });

      return () => {
        cancelled = true;
        unlisteners.forEach((unlisten) => unlisten());
        clearThinkingWatchdog();
        clearCrossWatchdog();
        clearPhoneTimer();
        clearMeetings(false);
        cross.clear();
        waitingForReplyRef.current = false;
      };
    }, [characterId, previewMode, applyMotion, facePeer, startPhoneIfActive, stopBusy, stopFacingPeer, walkToPeer]);

    useEffect(() => {
      if (!ambientMotionEnabled || previewMode) return undefined;
      let cancelled = false;
      let timer: number | null = null;
      const rhythm = new RoamRhythm(desktopPhysicsEnabled, performance.now());
      let leanToken: number | undefined;
      /** 最近一次本机全屏图像分析标注出的兴趣点，以及下一次该刷新的时刻。 */
      let interest: ScreenInterestPoint[] = [];
      let interestNextAt = 0;

      // 在静息期间轻量读取全局键鼠空闲，长等待也能及时切到活跃巡游节奏。
      const arm = () => {
        if (cancelled) return;
        if (timer !== null) window.clearTimeout(timer);
        timer = window.setTimeout(() => void poll(), rhythm.nextCheckMs(performance.now()));
      };
      const poll = async () => {
        let idleSeconds: number | null = null;
        try { idleSeconds = await invoke<number | null>('get_system_input_idle_seconds'); } catch { /* 使用正常节奏 */ }
        if (cancelled) return;
        rhythm.observeInput(idleSeconds, performance.now());
        if (rhythm.ready(performance.now())) await walkOnce();
        else arm();
      };
      const scheduleNext = (retry = false) => {
        rhythm.schedule(performance.now(), retry);
        arm();
      };
      const holdAtEdge = async (holdMs: number, side: ChibiDirection) => {
        if (cancelled) return;
        const lean = beginLean(holdMs, side,
          // 持续倚靠不占窗口位移锁，支撑面变化时物理仍能正常接管。
          () => { positioningCoordinator.ambientMoveInFlight = false; },
          () => { positioningCoordinator.ambientMoveInFlight = true; });
        if (!lean) return;
        leanToken = lean.token;
        const completed = await lean.done;
        leanToken = undefined;
        if (cancelled) return;
        if (completed) await playTurnBack();
        else if (!positioningCoordinator.dragInFlight) returnToTone(lean.token);
      };

      const walkOnce = async () => {
        // fleeInFlight 也在列：生气脸一播完桌宠就回到 idle，而逃离的位移可能还在跑
        // （逃离不播舞台动作，所以「姿态不是 idle」不再能替我们挡住这一趟）。
        if (
          cancelled || pressedRef.current || !isAtRest() ||
          positioningCoordinator.fullscreenHidden || positioningCoordinator.fullscreenInFlight ||
          positioningCoordinator.smartPositioningInFlight || positioningCoordinator.ambientMoveInFlight ||
          (positioningCoordinator.physicsEnabled && (!positioningCoordinator.physicsLane || positioningCoordinator.physicsInFlight)) ||
          positioningCoordinator.fleeInFlight
        ) {
          scheduleNext(true);
          return;
        }
        positioningCoordinator.ambientMoveInFlight = true;
        // 只有「被占用」才值得重试；能走到判断这一步说明是主动决定不走，按静息期处理。
        let retry = false;
        let walkToken: number | undefined;
        try {
          const windowHandle = getCurrentWindow();
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

          // Read visible peers only when planning a walk, without another polling timer.
          const peerWindows = (await getAllWindows()).filter(peer =>
            peer.label !== windowHandle.label && CHIBI_CHARACTER_IDS.includes(peer.label));
          const peers = (await Promise.all(peerWindows.map(async (peer): Promise<PetRect | null> => {
            try {
              if (!await peer.isVisible()) return null;
              const [pos, bounds] = await Promise.all([peer.outerPosition(), peer.outerSize()]);
              return { id: peer.label, x: pos.x, y: pos.y, width: bounds.width, height: bounds.height };
            } catch { return null; } // A peer may close while its geometry is being read.
          }))).filter((peer): peer is PetRect => peer !== null);
          if (cancelled || positioningCoordinator.dragInFlight || crossPresentationRef.current.motion || !isAtRest()) {
            retry = true;
            return;
          }
          const margin = EDGE_MARGIN_PX * monitor.scaleFactor;
          const inset = desktopPhysicsEnabled ? 0 : margin;
          const area = monitor.workArea;
          const physicsLane = positioningCoordinator.physicsLane;
          const wallPositions = leanWallPositions(monitor, size.width);
          const lane = ambientLane(
            { id: characterId, x: position.x, y: position.y, width: size.width, height: size.height },
            peers,
            Math.max(area.position.x + inset, physicsLane?.minX ?? -Infinity),
            Math.min(area.position.x + area.size.width - size.width - inset, physicsLane?.maxX ?? Infinity),
            margin,
          );
          // 偶尔用本机全屏图像分析标注一次「值得看一眼」的位置（不联网、不调模型）。
          // 只在重力模式跑：非重力模式下智能避让已经在做同一种截图，别重复捕获。
          const clock = performance.now();
          if (desktopPhysicsEnabled && clock >= interestNextAt) {
            try {
              const result = await invoke<{ points?: ScreenInterestPoint[]; busy?: boolean }>(
                'find_screen_interest_points',
              );
              if (cancelled) return;
              if (result.busy) {
                interestNextAt = performance.now() + INTEREST_BUSY_RETRY_MS;
              } else {
                interestNextAt = performance.now() + INTEREST_REFRESH_MIN_MS
                  + Math.random() * INTEREST_REFRESH_RANGE_MS;
                // 巡游是沿当前支撑面横向走的，只保留当前显示器上的兴趣点。
                const left = area.position.x;
                const right = left + area.size.width;
                interest = (result.points ?? []).filter(point =>
                  point.x + point.width / 2 >= left && point.x + point.width / 2 <= right);
              }
            } catch {
              interest = [];
              interestNextAt = performance.now() + INTEREST_BUSY_RETRY_MS;
            }
            if (cancelled || !isAtRest()) return;
          }
          // 一趟行动的意图由纯函数决定（巡逻 / 靠墙 / 闲逛 / 张望 / 围观兴趣点），这里只负责执行。
          const lean = resolveMotion('lean', characterId);
          const roam = planRoam({
            x: position.x, ...lane, gravity: desktopPhysicsEnabled, interest,
            // 站台、任务栏和同伴占位都会限制可走区间，只有显示器边缘可以倚靠。
            walls: {
              left: lean.kind === 'animation' && lane.walls.left && Math.round(lane.minX) === wallPositions.left && leanContactX(lean, characterId, 'left') !== null,
              right: lean.kind === 'animation' && lane.walls.right && Math.round(lane.maxX) === wallPositions.right && leanContactX(lean, characterId, 'right') !== null,
            },
          });
          const targetX = roam.targetX;
          const dx = targetX - position.x;

          // 原地巡礼：不挪窗口，只播一段「四处张望」。
          if (roam.intent === 'lookout') {
            if (roam.gesture) applyMotion(roam.gesture.motion, roam.gesture.durationMs);
            return;
          }

          if (!roam.edge && Math.abs(dx) < WALK_DISTANCE_MIN_PX) return;

          // 时长与步数都由距离推（walkPlan）：走多远就花多久，腿摆一格地面挪一格。
          const plan = planAmbientWalk(dx, undefined, walkCycleTravelPx(characterId, dx > 0 ? 'right' : 'left'));
          const nextDirection: ChibiDirection = dx === 0 ? roam.direction : plan.direction;

          const turned = await playTurn(nextDirection);
          if (!turned || cancelled) return;
          if (crossPresentationRef.current.motion || positioningCoordinator.peerConversationInProgress || positioningCoordinator.dragInFlight) {
            if (!positioningCoordinator.dragInFlight) returnToTone();
            return;
          }

          // 走动与窗口滑动并行、共用同一条时长轴（frames × frameDelayMs === durationMs）。
          const walk = plan.walking ? beginWalk(nextDirection, plan.frames, plan.frameDelayMs) : null;
          walkToken = walk?.token ?? sequenceTokenRef.current;
          const slid = dx === 0 || await runSlide({
            fromX: position.x,
            fromY: position.y,
            toX: targetX,
            toY: position.y,
            durationMs: plan.durationMs,
            linear: plan.walking,
            apply: (x, y) => invoke('set_window_position', { x, y }),
            shouldAbort: () =>
              cancelled || pressedRef.current || positioningCoordinator.dragInFlight ||
              speakingRef.current || positioningCoordinator.fullscreenHidden || positioningCoordinator.fullscreenInFlight ||
              positioningCoordinator.physicsInFlight ||
              positioningCoordinator.peerConversationInProgress || !!crossPresentationRef.current.motion || sequenceTokenRef.current !== walkToken,
          });
          // 被打断：窗口停在半途，姿态交给接管者，不再插手。
          if (!slid) {
            if (!cancelled && !positioningCoordinator.dragInFlight) returnToTone(walkToken);
            return;
          }
          if (walk && !await walk.done) return;
          if (cancelled || positioningCoordinator.peerConversationInProgress) {
            if (!positioningCoordinator.dragInFlight) returnToTone(walkToken);
            return;
          }
          if (roam.edge && roam.gesture) {
            // 靠墙：保持面向边界倚靠，停够时长后再回身回正。
            await holdAtEdge(roam.gesture.durationMs, roam.direction);
          } else {
            await playTurnBack();
            if (roam.gesture && !positioningCoordinator.peerConversationInProgress) applyMotion(roam.gesture.motion, roam.gesture.durationMs);
          }
        } catch {
          // 智能避让仍是权威；自主漫步只是可选的舞台调度，出错就安静跳过这一趟。
          retry = true;
          if (walkToken !== undefined && !cancelled && !positioningCoordinator.dragInFlight) returnToTone(walkToken);
        } finally {
          positioningCoordinator.ambientMoveInFlight = false;
          scheduleNext(retry);
        }
      };

      timer = window.setTimeout(() => void poll(), 0);
      return () => {
        cancelled = true;
        if (timer !== null) window.clearTimeout(timer);
        if (leanToken !== undefined && sequenceTokenRef.current === leanToken) {
          sequenceTokenRef.current += 1;
          if (!positioningCoordinator.dragInFlight) returnToTone();
        }
        positioningCoordinator.ambientMoveInFlight = false;
      };
    }, [ambientMotionEnabled, applyMotion, beginLean, beginWalk, characterId, desktopPhysicsEnabled, isAtRest, playTurn, playTurnBack, previewMode, returnToTone]);

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
      if (presenceModeRef.current === 'rest' || !dockOff()) return;
      if (scalePressRef.current || Date.now() < suppressClickUntilRef.current) return;
      wakeUp();
      // 窗口发生过实际拖动时，mouseup 后浏览器仍可能补发 click；该 click 不应触发台词。
      if (Date.now() < suppressClickUntilRef.current) return;
      playTapFeedback();
      const now = Date.now();
      const previous = recentTapTimesRef.current[recentTapTimesRef.current.length - 1];
      const recent = recentTapTimesRef.current.filter(at => now - at < 5000);
      recent.push(now);
      recentTapTimesRef.current = recent;
      const metrics: PetInteractionMetrics = {
        clickCount: recent.length, windowMs: 5000,
        intervalMs: previous === undefined ? undefined : now - previous,
        pressDurationMs: lastPressDurationRef.current,
      };
      lastPressDurationRef.current = undefined;
      const { annoyed, onset } = tapLedgerRef.current.note(now);
      const doubleClick = takePendingClick();

      // 逃跑途中不改动作：改动作会打断正在跑的位移，把桌宠扔在半路。语气照旧。
      if (fleeingRef.current) {
        onInteraction?.('rough_click', metrics);
        if (doubleClick) onOpenQuickChat?.();
        return;
      }

      // 气头刚点着：立刻给脸、立刻跑。双击语义照旧保留（这一下可能正是某一对里的第二下）
      if (onset) {
        applyMotion('angry');
        onInteraction?.('rough_click', metrics);
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
        onInteraction?.(annoyed ? 'rough_click' : 'double_click', metrics);
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
        onInteraction?.(annoyed ? 'rough_click' : 'single_click', metrics);
      }, 230);
    };

    const handleWheel = (event: ReactWheelEvent<HTMLDivElement>) => {
      if (!event.ctrlKey) return;
      event.preventDefault();
      cancelScaleHold();
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
    // Both frontend gravity and backend inertial flings use atlas slot 3 (drag).
    // A collision's dizzy pose stays above this overlay, including during speech.
    const physicalMotion = (physics.airborne || fling.active) && poseName !== 'dizzy' && !positioningCoordinator.dragInFlight ? 'drag' : poseName;
    // Incoming text/audio must not hide the remaining thought exit frames.
    const displayMotion = dockState !== 'Off' && edgeSettledRef.current ? dockMotion(dockPlanRef.current?.side ?? 'right')
      : physicalMotion === 'thinking' ? 'thinking'
      : speechDisplayMotion(physicalMotion, speakingRef.current, pressed, crossPresentationRef.current.motion);
    // 底边贴边沿用默认比例，只平移素材以对齐屏幕下沿。
    const bottomDocked = dockState !== 'Off' && dockPlanRef.current?.side === 'bottom';
    const activeSpec = resolveMotion(displayMotion, characterId);
    const peerSpeaking = !pressed && activeSpec.kind === 'animation' && activeSpec.name === 'turn' && frame === activeSpec.frames - 1 &&
      peerFacingRef.current?.token === sequenceTokenRef.current && (audioPlaying || crossPresentationRef.current.motion === 'talk');
    const wallLean = activeSpec.kind === 'animation' && activeSpec.name === 'lean'
      && leanOffsetRef.current?.token === sequenceTokenRef.current
      ? leanOffsetRef.current.offsets[frame]
      : undefined;
    const activeSheet =
      activeSpec.kind === 'animation'
        ? sheetUrl(activeSpec, characterId, walkDirection)
        : null;
    const idleFrameStyle = idleSheet && sheetLoader.isReady(idleSheet)
      ? { backgroundImage: `url('${idleSheet}')`, backgroundSize: '100% 100%', backgroundPosition: '0% 0%' }
      : frameStyle(IDLE_SPEC, IDLE_SLOT, characterId, walkDirection);
    const spriteFrameStyle = activeSpec.name === 'idle' ? idleFrameStyle :
      activeSheet !== null && !sheetLoader.isReady(activeSheet)
        ? idleFrameStyle
        : frameStyle(activeSpec, activeSpec.kind === 'pose' ? activeSpec.slot : frame, characterId, walkDirection);

    return (
      <div
        className={`chibi-pet-canvas chibi-pet-${characterId}${isBuiltinCharacterId(characterId) ? '' : ' chibi-pet-imported'}`}
        style={{ clipPath: edgeClip }}
        onWheel={handleWheel}
        aria-label={`${characterId} desktop pet`}
      >
        {physics.error && <span role="status" className="chibi-physics-error" title="Desktop physics unavailable">⚠</span>}
        <div
          ref={stageRef}
          className={`chibi-pet-stage pose-${displayMotion}${activeSpec.kind === 'animation' ? ' has-frame-animation' : ''}${peerSpeaking ? ' is-peer-speaking' : ''}${wallLean !== undefined ? ' is-wall-lean' : ''}${bottomDocked ? ' is-bottom-docked' : ''}${pressed ? ' is-pressed' : ''}`}
          style={wallLean === undefined ? undefined : { transform: `translateX(${wallLean}%)` }}
        >
          <div className="chibi-pet-shadow" />
          <div ref={physicsLayerRef} className="chibi-pet-physics-feedback">
          <div ref={tapFeedbackRef} className="chibi-pet-tap-feedback">
            {/* 默认尺寸校准与逐帧变换分层，底边贴边只调整裁切线的垂直对齐。 */}
            <div
              className="chibi-pet-default-size"
              style={bottomDocked ? bottomDockStyle(characterId) : defaultSizeStyle(characterId)}
            >
            <div
              className="chibi-pet-sprite"
              style={spriteFrameStyle}
              role="button"
              tabIndex={0}
              onMouseDown={(event) => {
                if (event.button !== 0) return;
                if (!dockOff() && edgeSettledRef.current) { onModelClick?.(); return; }
                if (event.ctrlKey) {
                  event.preventDefault();
                  scalePressRef.current = true;
                  scalePressPositionRef.current = { x: event.clientX, y: event.clientY };
                  takePendingClick();
                  cancelScaleHold();
                  scaleHoldRef.current = setTimeout(() => {
                    scaleHoldRef.current = null;
                    scaleRef.current = 1;
                    onResetScale?.();
                  }, 500);
                  return;
                }
                // 按下这件事先记下来，再谈它要不要打断当前姿态：逃离问的是「按了多久」，
                // 与这一按落在格位上还是帧序列上无关（见 pressAliveRef 的说明）。
                pressAliveRef.current = true;
                pressStartedAtRef.current = Date.now();
                if (presenceModeRef.current === 'rest') { onModelClick?.(); return; }
                if (wakeUp()) {
                  onModelClick?.();
                  return;
                }
                const spec = getMotion(poseNameRef.current, characterId);
                // 点击期间保留看手机动作，并继续向调用方报告点击。
                const phoneAnimation = isPhoneAnimation(spec);
                if (spec?.kind === 'animation' && !phoneAnimation) {
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
                // 保留进场的 token，掏出手机的动作才能继续接上循环。
                if (!phoneAnimation) sequenceTokenRef.current += 1;
                pressedRef.current = true;
                setPressed(true);
                onModelClick?.();
              }}
              onMouseUp={(event) => {
                if (event.button !== 0) return;
                lastPressDurationRef.current = pressAliveRef.current ? Date.now() - pressStartedAtRef.current : undefined;
                pressAliveRef.current = false;
                pressedRef.current = false;
                setPressed(false);
              }}
              onMouseMove={(event) => {
                if (!scalePressRef.current) return;
                const start = scalePressPositionRef.current;
                if (Math.hypot(event.clientX - start.x, event.clientY - start.y) > 6) cancelScaleHold();
              }}
              onMouseLeave={() => {
                cancelScaleHold();
                lastPressDurationRef.current = undefined;
                pressAliveRef.current = false;
                pressedRef.current = false;
                setPressed(false);
              }}
              onClick={handleClick}
              onDoubleClick={(event) => event.preventDefault()}
              onKeyDown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault();
                  if (!event.repeat) handleClick();
                }
              }}
            />
            </div>
          </div>
          </div>
        </div>
      </div>
    );
  },
);

ChibiPetCanvas.displayName = 'ChibiPetCanvas';
