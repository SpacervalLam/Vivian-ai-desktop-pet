import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from 'react';
import type { RefObject } from 'react';
import { Application, Ticker, ShaderSystem } from 'pixi.js';
import { install } from '@pixi/unsafe-eval';
import { Live2DModel } from 'pixi-live2d-display/cubism4';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useTranslation } from 'react-i18next';
import type { Live2DLipsync } from '../utils/Live2DLipsync';
import { getCharacterId } from '../characterContext';

/**
 * 获取当前窗口的角色 ID，优先从 characterContext 读取，
 * 降级从窗口 label 获取（确保多窗口场景下不会互相干扰）。
 */
function getWindowCharacterId(): string | undefined {
  const ctxId = getCharacterId();
  if (ctxId) return ctxId;
  // 降级：从窗口 label 获取（每个 Tauri 窗口的 label = character_id）
  try {
    const win = getCurrentWindow();
    const label = win.label;
    if (label && label !== 'main') return label;
  } catch {
    /* 非 Tauri 环境忽略 */
  }
  return undefined;
}
import {
  useMicroPresence,
} from '../hooks/useLive2DBehavior';
import { useEmotionFacs } from '../hooks/useEmotionFacs';
import { useInstantReact } from '../hooks/useInstantReact';
import { LayeredParameterMixer, setMixer } from '../utils/LayeredParameterMixer';

// 生产环境 CSP 禁止 unsafe-eval，PIXI shader 编译会失败。
// @pixi/unsafe-eval 用 Function 构造器之外的替代实现替换 ShaderSystem 的 apply-attribute/uniform 逻辑，
// 必须在创建任何 Application/Renderer 之前调用一次。
install({ ShaderSystem });

// 注册 Ticker，使模型自动驱动呼吸 / 眨眼 / 动作
Live2DModel.registerTicker(Ticker);

/** 缩放范围 */
const SCALE_MIN = 0.5;
const SCALE_MAX = 50.0;
/** 滚轮缩放敏感度：每 100px 滚轮位移约 3% 缩放变化 */
const SCROLL_SENSITIVITY = 0.0003;

/* ============ 交互检测参数 ============ */
/** 快速点击：800ms 内连续 3 次点击 */
const FAST_CLICK_COUNT = 3;
const FAST_CLICK_WINDOW = 800;
/** 双击间隔：350ms内连续2次点击 */
const DOUBLE_CLICK_WINDOW = 350;
/** 快速拖动：速度超过此阈值（px/ms） */
const FAST_DRAG_VELOCITY = 1.8;
/** 抚摸：未按下按键时，鼠标在面部区域缓慢悬移超过此距离（px）且速度低于此值 */
const PET_MIN_DISTANCE = 60;
const PET_MAX_VELOCITY = 0.5;
/** 面部区域：canvas 高度的上此比例范围内视为面部周围 */
const FACE_REGION_RATIO = 0.45;
/** 悬停移动连续性：超过此间隔（ms）视为中断，重置累计距离 */
const HOVER_RESET_GAP = 1000;
/** 长按：按下不动超过此时间（ms） */
const LONG_PRESS_THRESHOLD = 1500;
/** 交互冷却：同类型交互的最小间隔（ms），防止刷屏 */
const INTERACTION_COOLDOWN = 1500;
/** 拖动开始检测阈值：移动超过此距离（px）视为开始拖动 */
const DRAG_START_THRESHOLD = 8;

/**
 * 角色默认表情名（与 model_manifest.json 中 aliases.default 保持一致）。
 * 这些模型的"无表情"状态瞳孔/耳朵不可见，必须常驻一个默认表情来维持正常外观。
 */
const DEFAULT_EXPRESSION_BY_CHAR: Record<string, string> = {
  Vivian: 'love_eyes',
  Nana: 'star_eyes',
};

function getDefaultExpression(): string {
  const cid = getWindowCharacterId() ?? getCharacterId();
  if (cid && DEFAULT_EXPRESSION_BY_CHAR[cid]) return DEFAULT_EXPRESSION_BY_CHAR[cid];
  const label = (() => {
    try { return getCurrentWindow().label; } catch { return ''; }
  })();
  if (label && DEFAULT_EXPRESSION_BY_CHAR[label]) return DEFAULT_EXPRESSION_BY_CHAR[label];
  return 'star_eyes';
}

interface InteractionState {
  clickTimes: number[];
  lastClickTime: number;
  clickCount: number;
  singleClickTimer: number | null;
  dragStart: { x: number; y: number; t: number } | null;
  lastMove: { x: number; y: number; t: number } | null;
  totalDragDistance: number;
  isDragging: boolean;
  dragStarted: boolean;
  dragMaxVelocity: number;
  pointerDownTime: number;
  longPressTimer: number | null;
  lastInteractionTime: number;
  lastInteractionType: string;
  /** 悬停抚摸（未按下按键时鼠标在面部区域移动）的累计状态 */
  hoverLastMove: { x: number; y: number; t: number } | null;
  hoverTotalDistance: number;
  hoverMaxVelocity: number;
  /** 鼠标是否在窗口内 */
  mouseInWindow: boolean;
  /** 双击已在 pointerdown 时触发，pointerup 时跳过单击检测 */
  _doubleClickPending: boolean;
}

function createInteractionState(): InteractionState {
  return {
    clickTimes: [],
    lastClickTime: 0,
    clickCount: 0,
    singleClickTimer: null,
    dragStart: null,
    lastMove: null,
    totalDragDistance: 0,
    isDragging: false,
    dragStarted: false,
    dragMaxVelocity: 0,
    pointerDownTime: 0,
    longPressTimer: null,
    lastInteractionTime: 0,
    lastInteractionType: '',
    hoverLastMove: null,
    hoverTotalDistance: 0,
    hoverMaxVelocity: 0,
    mouseInWindow: false,
    _doubleClickPending: false,
  };
}

/** 检测并触发交互事件，返回触发的交互类型（或 null）
 *  注：单次点击使用延迟检测，pointerup后等待DOUBLE_CLICK_WINDOW，若没有第二次点击则触发single_click
 */
function detectInteraction(
  state: InteractionState,
  eventType: 'pointerdown' | 'pointermove' | 'pointerup' | 'mouseenter' | 'mouseleave',
  x: number,
  y: number,
  canvasHeight?: number,
): string | null {
  const now = Date.now();

  if (eventType === 'mouseenter') {
    if (!state.mouseInWindow) {
      state.mouseInWindow = true;
      return triggerInteraction(state, 'mouse_enter', now);
    }
    return null;
  }

  if (eventType === 'mouseleave') {
    state.mouseInWindow = false;
    state.hoverLastMove = null;
    state.hoverTotalDistance = 0;
    return triggerInteraction(state, 'mouse_leave', now);
  }

  if (eventType === 'pointerdown') {
    state.clickTimes.push(now);
    while (state.clickTimes.length > 0 && now - state.clickTimes[0] > FAST_CLICK_WINDOW) {
      state.clickTimes.shift();
    }
    state.dragStart = { x, y, t: now };
    state.lastMove = { x, y, t: now };
    state.totalDragDistance = 0;
    state.isDragging = true;
    state.dragStarted = false;
    state.dragMaxVelocity = 0;
    state.pointerDownTime = now;

    // 长按检测
    if (state.longPressTimer !== null) clearTimeout(state.longPressTimer);
    state.longPressTimer = window.setTimeout(() => {
      // 长按触发时，拖动距离需很小
      if (state.totalDragDistance < 15) {
        triggerInteraction(state, 'long_press', Date.now());
      }
    }, LONG_PRESS_THRESHOLD);

    // 快速点击检测（连续3次）
    if (state.clickTimes.length >= FAST_CLICK_COUNT) {
      state.clickTimes = [];
      if (state.singleClickTimer !== null) {
        clearTimeout(state.singleClickTimer);
        state.singleClickTimer = null;
      }
      return triggerInteraction(state, 'fast_click', now);
    }

    // 双击检测：在 pointerdown 时提前检测（而不是等 pointerup）
    // 这样体感更快，因为第二次按下的瞬间就响应
    const timeSinceLastClick = now - state.lastClickTime;
    if (timeSinceLastClick < DOUBLE_CLICK_WINDOW && state.clickCount >= 1) {
      if (state.singleClickTimer !== null) {
        clearTimeout(state.singleClickTimer);
        state.singleClickTimer = null;
      }
      state.clickCount = 0;
      state.lastClickTime = 0;
      // 标记本次按下是双击的一部分，pointerup 时不再触发单击
      state._doubleClickPending = true;
      return triggerInteraction(state, 'double_click', now);
    }

    // 按下立即触发 drag_start（伸手表情），不等到移动超过阈值
    // dragStarted 标记仍由 pointermove 检测移动距离设置，用于区分真拖拽和单击
    return triggerInteraction(state, 'drag_start', now);
  }

  // 按住拖动：检测drag_start和fast_drag
  if (eventType === 'pointermove' && state.isDragging && state.lastMove && state.dragStart) {
    const dt = now - state.lastMove.t;
    if (dt > 0) {
      const dx = x - state.lastMove.x;
      const dy = y - state.lastMove.y;
      const dist = Math.sqrt(dx * dx + dy * dy);
      const velocity = dist / dt;
      state.totalDragDistance += dist;
      if (velocity > state.dragMaxVelocity) state.dragMaxVelocity = velocity;

      // 拖动开始标记：移动超过阈值时标记为真拖拽（drag_start 交互已在 pointerdown 时触发）
      // 此标记用于 pointerup 时区分真拖拽（触发 drag_end）和单击（触发 single_click）
      if (!state.dragStarted && state.totalDragDistance > DRAG_START_THRESHOLD) {
        state.dragStarted = true;
        if (state.longPressTimer !== null) {
          clearTimeout(state.longPressTimer);
          state.longPressTimer = null;
        }
        state.lastMove = { x, y, t: now };
      }

      // 快速拖动检测
      if (velocity > FAST_DRAG_VELOCITY && state.totalDragDistance > 20) {
        const result = triggerInteraction(state, 'fast_drag', now);
        // 重置拖动起点，避免持续触发
        state.dragStart = { x, y, t: now };
        state.totalDragDistance = 0;
        state.dragMaxVelocity = 0;
        if (state.longPressTimer !== null) {
          clearTimeout(state.longPressTimer);
          state.longPressTimer = null;
        }
        if (result) return result;
      }
    }
    state.lastMove = { x, y, t: now };
    return null;
  }

  // 悬停抚摸检测：未按下按键时，鼠标在面部区域缓慢移动
  if (eventType === 'pointermove' && !state.isDragging && canvasHeight !== undefined) {
    const inFaceRegion = y < canvasHeight * FACE_REGION_RATIO;
    if (!inFaceRegion) {
      // 离开面部区域，重置悬停累计
      state.hoverLastMove = null;
      state.hoverTotalDistance = 0;
      state.hoverMaxVelocity = 0;
      return null;
    }
    if (state.hoverLastMove) {
      const dt = now - state.hoverLastMove.t;
      if (dt > 0 && dt < HOVER_RESET_GAP) {
        const dx = x - state.hoverLastMove.x;
        const dy = y - state.hoverLastMove.y;
        const dist = Math.sqrt(dx * dx + dy * dy);
        const velocity = dist / dt;
        state.hoverTotalDistance += dist;
        if (velocity > state.hoverMaxVelocity) state.hoverMaxVelocity = velocity;

        // 抚摸检测：缓慢悬移累计足够距离
        if (
          state.hoverTotalDistance > PET_MIN_DISTANCE &&
          state.hoverMaxVelocity < PET_MAX_VELOCITY
        ) {
          // 触发后重置累计，避免持续触发（冷却由 triggerInteraction 处理）
          state.hoverTotalDistance = 0;
          state.hoverMaxVelocity = 0;
          state.hoverLastMove = { x, y, t: now };
          return triggerInteraction(state, 'pet', now);
        }
      } else {
        // 间隔过久视为中断，重置累计
        state.hoverTotalDistance = 0;
        state.hoverMaxVelocity = 0;
      }
    }
    state.hoverLastMove = { x, y, t: now };
    return null;
  }

  if (eventType === 'pointerup' && state.isDragging) {
    state.isDragging = false;
    if (state.longPressTimer !== null) {
      clearTimeout(state.longPressTimer);
      state.longPressTimer = null;
    }

    const wasDragging = state.dragStarted;
    state.dragStarted = false;
    state.dragStart = null;
    state.lastMove = null;

    // 如果拖动过，触发drag_end
    if (wasDragging) {
      return triggerInteraction(state, 'drag_end', now);
    }

    // 未拖动则为点击：检测单次点击（双击已在 pointerdown 时提前触发）
    if (state.totalDragDistance < DRAG_START_THRESHOLD) {
      // 如果本次是双击的第二次松开，跳过单击检测
      if (state._doubleClickPending) {
        state._doubleClickPending = false;
        return null;
      }

      // 单次点击：记录点击时间，为下一次双击检测做准备
      state.clickCount = 1;
      state.lastClickTime = now;

      return triggerInteraction(state, 'single_click', now);
    }
  }

  return null;
}

function triggerInteraction(
  state: InteractionState,
  type: string,
  now: number,
): string | null {
  // 冷却检查：同类型交互在冷却期内不重复触发
  if (
    type === state.lastInteractionType &&
    now - state.lastInteractionTime < INTERACTION_COOLDOWN
  ) {
    return null;
  }
  state.lastInteractionType = type;
  state.lastInteractionTime = now;
  return type;
}

export interface Live2DCanvasHandle {
  setExpression: (name: string, durationMs?: number) => void;
  playMotion: (group: string, index?: number) => void;
  focus: (x: number, y: number) => void;
  /** 获取底层 Live2DModel 实例（供 lipsync 等模块直接驱动参数） */
  getModel: () => Live2DModel | null;
  /** 设置模型缩放（对应 Python wheelEvent Ctrl+滚轮） */
  setScale: (scale: number) => void;
  /** 获取当前缩放 */
  getScale: () => number;
  /** 重置模型到 fitModel 计算的默认缩放与居中位置 */
  refitModel: () => void;
  /** 重置当前 SDK 表情到 defaultExpression（清空所有表情参数） */
  resetExpression: () => void;
}

interface Live2DCanvasProps {
  modelUrl?: string;
  /** Live2DLipsync 实例引用，供微存在感呼吸偏移使用 */
  lipsyncRef?: RefObject<Live2DLipsync | null>;
  onReady?: () => void;
  onExpressionEnd?: () => void;
  onModelClick?: () => void;
  /**
   * 鼠标跟随粗粒度开关：'window'=允许跟随（实际跟随由交互事件触发），'off'=禁止跟随。
   * 移除全屏持续跟随模式，实现"活人感"：仅鼠标进入窗口/点击/拖动时短暂跟随 5~8s。
   */
  mouseFollowMode?: 'window' | 'off';
  /** Ctrl+滚轮缩放回调，参数为新的缩放因子（1.0=默认大小） */
  onScaleChange?: (scale: number) => void;
}

// 模型 URL 由后端 get_model_url 命令统一解析：
// - 开发模式（debug 编译）：返回 Vite dev server 路径（如 /Vivian/Vivian.model3.json）
// - 生产模式（release 编译）：返回 asset 协议 URL（如 http://asset.localhost/Vivian/Vivian.model3.json）
// 后端通过 strip_prefix 计算资源根目录的相对路径，消除前端正则匹配 public/ 的脆弱性。
async function resolveModelUrl(): Promise<string> {
  return invoke<string>('get_model_url', { characterId: getCharacterId() ?? undefined });
}

export const Live2DCanvas = forwardRef<Live2DCanvasHandle, Live2DCanvasProps>(
  function Live2DCanvas(
    {
      modelUrl,
      lipsyncRef,
      onReady,
      onExpressionEnd,
      onModelClick,
      mouseFollowMode = 'off',
      onScaleChange,
    },
    ref,
  ) {
    const containerRef = useRef<HTMLDivElement | null>(null);
    const appRef = useRef<Application | null>(null);
    const modelRef = useRef<Live2DModel | null>(null);
    const mixerRef = useRef<LayeredParameterMixer | null>(null);
    const expressionTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
    const [failed, setFailed] = useState<string | null>(null);
    const { t } = useTranslation();
    const mouseFollowModeRef = useRef<'window' | 'off'>(mouseFollowMode);
    /** cursor:position 事件监听清理函数 */
    const cursorUnlistenRef = useRef<(() => void) | null>(null);
    /**
     * 交互触发的鼠标跟随截止时间戳（performance.now() 基准）。
     * 鼠标进入窗口/点击/拖动时刷新为 now + 5000~8000ms，超时后回归自主行为。
     * 实现"活人感"：不无时不刻跟随鼠标，仅在有交互意图时短暂注视。
     */
    const interactionFollowUntilRef = useRef(0);
    /** 刷新交互跟随窗口（5~8 秒随机） */
    const refreshInteractionFollow = useCallback(() => {
      interactionFollowUntilRef.current = performance.now() + 5000 + Math.random() * 3000;
    }, []);
    /** 是否处于交互触发的跟随窗口内 */
    const isInteractionFollowActive = useCallback(() => {
      return performance.now() < interactionFollowUntilRef.current;
    }, []);
    const onScaleChangeRef = useRef(onScaleChange);
    /** 用户缩放因子（1.0 = 默认大小，由 fitModel 确定基础比例） */
    const userScaleRef = useRef(1.0);
    /** 基础缩放比例（让模型刚好填满窗口的比例） */
    const baseScaleRef = useRef(1.0);
    /** 模型显示缩放系数（用于补偿画布留白，默认 1.0） */
    const displayScaleRef = useRef(1.0);
    /** 防止程序化调整窗口大小时的递归缩放 */
    const applyingScaleRef = useRef(false);

    // ---- 微存在感 共享状态 ----
    /** 安全重置表情并恢复默认表情的函数（在 useEffect 内赋值） */
    const safeResetExpressionRef = useRef<() => void>(() => {});
    const interactionStateRef = useRef<InteractionState>(createInteractionState());
    /** 按住拖动时维持表情：存储当前 drag 表情名和定时器 */
    const dragHoldExpressionRef = useRef<string | null>(null);
    const dragHoldTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
    /** dragHold 安全超时：防止 mouseup/pointercancel 丢失导致伸手表情永久卡住 */
    const dragHoldSafetyRef = useRef<ReturnType<typeof setTimeout> | null>(null);

    // ---- 微存在感（呼吸 + 身体微晃 + 视线游移）----
    // 返回 microTickRef：由 ticker 回调同帧调用，
    // 消除原 RAF 与 ticker.update 的时序错配
    const microTickRef = useMicroPresence(modelRef, {
      lipsyncRef: lipsyncRef ?? { current: null },
    });

    // ---- 情绪驱动 FACS 连续表情 ----
    // 监听后端 psychology:state 事件，每帧将 7 维 EmotionState 映射为
    // FACS 通道再写入 Cubism 参数到 LayeredParameterMixer 'emotion' 层
    const emotionFacsTickRef = useEmotionFacs(modelRef, {
      lipsyncRef: lipsyncRef ?? { current: null },
    });

    // ---- 即时反应 FACS（三层反应系统 Layer 1/2）----
    // 监听 chat:instant_react 事件，在用户消息到达瞬间 / AI 文本首段完成时
    // 立即应用 FACS 参数到 'instant' 层（优先级高于 'emotion' 层）
    // chat:meta / chat:done 到达时自动清除（由反思结果 Layer 3 接管）
    useInstantReact(modelRef);

    const fitModel = useCallback(() => {
      const app = appRef.current;
      const model = modelRef.current;
      if (!app || !model) return;

      const sw = app.screen.width;
      const sh = app.screen.height;
      if (sw <= 0 || sh <= 0) return;

      // 必须用 internalModel.width/height（常量，不受 scale 影响），
      // 不能用 model.width/height（PIXI Container getter 会返回 scale*internalModel.width），
      // 否则窗口尺寸变化后再次调用 fitModel 会用上次的 scale 反算新 scale，
      // 导致模型实际显示尺寸 = sw/oldScale，当 oldScale<1 时模型被放大裁切。
      const im = model.internalModel;
      const mw = im?.width ?? model.width;
      const mh = im?.height ?? model.height;
      if (mw <= 0 || mh <= 0) return;

      model.anchor.set(0.5, 0.5);
      // 窗口尺寸已与模型比例对齐，scale=1.0 让模型完整填充窗口（无留白）
      // display_scale 用于补偿模型画布留白（如 Nana 留白较多，设 1.3 放大角色视觉大小）
      // 注意：模型始终填满当前窗口，用户缩放通过调整窗口大小实现
      const scale = Math.min(sw / mw, sh / mh) * displayScaleRef.current;
      baseScaleRef.current = scale;
      model.scale.set(scale);
      model.x = sw / 2;
      model.y = sh / 2;
    }, []);

    // 应用配置中的眨眼间隔（SDK CubismEyeBlink.setBlinkingInterval 接受秒为单位）
    const applyBlinkInterval = useCallback(async () => {
      const model = modelRef.current;
      if (!model) return;
      try {
        const intervalMs = await invoke<number>('get_config', {
          key: 'live2d_render.blink_interval',
        });
        const intervalSec = Math.max(0.1, (intervalMs ?? 4000) / 1000);
        const eyeBlink = (model as unknown as {
          internalModel?: { eyeBlink?: { setBlinkingInterval?: (s: number) => void } };
        }).internalModel?.eyeBlink;
        eyeBlink?.setBlinkingInterval?.(intervalSec);
      } catch {
        /* 配置读取失败时使用 SDK 默认间隔 */
      }
    }, []);

    useEffect(() => {
      let destroyed = false;

      const init = async () => {
        const container = containerRef.current;
        if (!container) return;

        try {
          // 解析模型 URL：优先使用外部传入的 modelUrl，否则自动解析
          // dev 模式走 vite dev server，prod 模式走 asset 协议（后端返回绝对路径）
          const url = modelUrl ?? (await resolveModelUrl());
          const app = new Application({
            view: undefined,
            backgroundAlpha: 0,
            autoStart: true,
            antialias: true,
            resolution: window.devicePixelRatio || 1,
            autoDensity: true,
            width: container.clientWidth || 400,
            height: container.clientHeight || 400,
            sharedTicker: true,
          });
          container.appendChild(app.view as HTMLCanvasElement);
          app.view.style.position = 'absolute';
          app.view.style.inset = '0';
          app.view.style.width = '100%';
          app.view.style.height = '100%';
          appRef.current = app;

          const model = await Live2DModel.from(url, { autoInteract: false });
          if (destroyed) {
            model.destroy();
            return;
          }

          app.stage.addChild(model);
          modelRef.current = model;

          mixerRef.current = new LayeredParameterMixer(model);
          setMixer(model, mixerRef.current);

          // 启用交互：点击模型唤醒（不自动跟随全局鼠标）
          model.interactive = true;
          model.buttonMode = true;

          // Hook: 所有表情加载完成后 fade 时间置 0，实现瞬间切换
          try {
            const em = (model as unknown as {
              internalModel?: {
                motionManager?: {
                  expressionManager?: {
                    loadExpression?: (index: number) => Promise<unknown>;
                  };
                };
              };
            }).internalModel?.motionManager?.expressionManager;
            if (em && typeof em.loadExpression === 'function') {
              const origLoad = em.loadExpression.bind(em);
              em.loadExpression = async function (index: number) {
                const expr = await origLoad(index) as {
                  setFadeInTime?: (t: number) => void;
                  setFadeOutTime?: (t: number) => void;
                } | null;
                if (expr) {
                  expr.setFadeInTime?.(0);
                  expr.setFadeOutTime?.(0);
                }
                return expr;
              };
            }
          } catch {
            /* 非 Cubism4 模型跳过 */
          }

          // 交互检测：快速点击/拖动/抚摸/长按 → 后端情绪更新 + 直接播放表情动作
          // 使用 model.expression() 标准 SDK 路径驱动表情，兼容所有模型
          const clearDragHold = () => {
            if (dragHoldTimerRef.current) {
              clearInterval(dragHoldTimerRef.current);
              dragHoldTimerRef.current = null;
            }
            if (dragHoldSafetyRef.current) {
              clearTimeout(dragHoldSafetyRef.current);
              dragHoldSafetyRef.current = null;
            }
            dragHoldExpressionRef.current = null;
          };

          // 将指定表情实例的 fade 时间置 0，实现瞬间切换（兼容 Cubism2/4）
          const setExpressionFadeZero = (expressionName: string) => {
            const m = modelRef.current;
            if (!m) return;
            try {
              const em = (m as unknown as {
                internalModel?: {
                  motionManager?: {
                    expressionManager?: {
                      getExpressionIndex?: (name: string) => number;
                      expressions?: Array<{
                        setFadeInTime?: (t: number) => void;
                        setFadeOutTime?: (t: number) => void;
                        setFadeIn?: (t: number) => void;
                        setFadeOut?: (t: number) => void;
                      } | null | undefined>;
                    };
                  };
                };
              }).internalModel?.motionManager?.expressionManager;
              const idx = em?.getExpressionIndex?.(expressionName);
              if (idx === undefined || idx < 0) return;
              const expr = em?.expressions?.[idx];
              if (!expr) return;
              // Cubism4 使用 setFadeInTime，Cubism2 使用 setFadeIn
              if (typeof expr.setFadeInTime === 'function') {
                expr.setFadeInTime(0);
                expr.setFadeOutTime?.(0);
              } else if (typeof expr.setFadeIn === 'function') {
                expr.setFadeIn(0);
                expr.setFadeOut?.(0);
              }
            } catch {
              /* 忽略 */
            }
          };

          const startDragHold = (expressionName: string) => {
            clearDragHold();
            dragHoldExpressionRef.current = expressionName;
            // 首次应用：异步加载完成后确认仍在 dragHold 状态
            try {
              const m = modelRef.current;
              if (m) {
                const p = m.expression(expressionName);
                if (p && typeof p.then === 'function') {
                  p.then(() => {
                    if (dragHoldExpressionRef.current !== expressionName) {
                      // 用户已释放，SDK 内部可能已应用该表情，需重置回默认
                      safeResetExpressionRef.current();
                      return;
                    }
                  }).catch(() => {
                    /* 忽略 */
                  });
                }
              }
            } catch {
              /* 忽略 */
            }
            // 每 800ms 重新应用表情，防止被其他系统覆盖
            // SDK setExpression() 对同一表情会跳过 fade-in，不会闪烁
            dragHoldTimerRef.current = setInterval(() => {
              const m = modelRef.current;
              if (m && dragHoldExpressionRef.current) {
                try {
                  m.expression(dragHoldExpressionRef.current);
                } catch {
                  /* 忽略 */
                }
              }
            }, 800);
            // 安全超时：10 秒后强制清除 dragHold，防止 mouseup/pointercancel 事件丢失
            // 导致伸手表情永久卡住（Tauri 点击穿透 / 窗口失焦 / 系统级鼠标捕获丢失等场景）
            dragHoldSafetyRef.current = setTimeout(() => {
              if (dragHoldExpressionRef.current !== null) {
                clearDragHold();
                safeResetExpression();
              }
            }, 10000);
          };


          /**
           * 安全重置当前表情并恢复默认表情。
           *
           * 直接调用 model.expression(default) 做表情切换。
           * SDK 的 crossfade 同时管理两个表情的 Parameters 和 Part Opacity
           * 过渡，手臂等 ArtMesh 在切换过程中始终可见。
           *
           * 不能调用 resetExpression()——它会完全卸载当前表情，导致 Part Opacity
           * 瞬间归零（如"伸手"控制的手臂 ArtMesh 消失）。
           *
           * crossfade 完成后手臂参数由 SDK 表情系统自动管理。
           */
          const safeResetExpression = () => {
            const m = modelRef.current;
            if (!m) return;
            try {
              m.expression(getDefaultExpression());
            } catch {
              /* 忽略 */
            }
          };
          safeResetExpressionRef.current = safeResetExpression;

          const handleInteraction = (type: string) => {
            const model = modelRef.current;
            if (!model) return;

            // 交互触发时刷新鼠标跟随窗口（drag_end 除外，因为用户已松开）
            if (type !== 'drag_end') {
              refreshInteractionFollow();
            }

            // drag_end / pointerup 松开时：清除按住维持的表情
            if (type === 'drag_end') {
              clearDragHold();
              safeResetExpression();
            }

            // 按住拖动期间（dragHoldExpressionRef 不为空），伸手表情拥有最高优先级
            // 禁止任何其他表情/motion 覆盖
            const isDraggingHold = dragHoldExpressionRef.current !== null;

            // 按住期间跳过所有其他交互处理
            if (isDraggingHold && type !== 'drag_end') {
              // 仍然调用后端更新心理状态，但不应用表情/动作
              invoke('apply_user_interaction', {
                interaction: type,
                characterId: getCharacterId() ?? undefined,
              }).catch(() => {});
              return;
            }

            invoke<{ expression: string; motion: string; action?: string; duration_ms?: number; avoid_mouse?: boolean; avoid_probability?: number }>('apply_user_interaction', {
              interaction: type,
              characterId: getCharacterId() ?? undefined,
            })
              .then((fb) => {
                // drag_start 竞态保护：后端响应到达时指针已松开 → 跳过 drag_start 表情
                if (type === 'drag_start' && !interactionStateRef.current.isDragging) {
                  // 仍处理 motion/action，但跳过 expression
                } else if (fb.expression && fb.expression !== 'default' && fb.expression !== 'idle') {
                  // 直接应用新 expression，SDK crossfade 处理旧表情的 fade-out
                  // 和新表情的 fade-in，Part Opacity 平滑过渡
                  try {
                    model.expression(fb.expression);
                  } catch {
                    /* 未知表情忽略 */
                  }
                  if (expressionTimerRef.current) clearTimeout(expressionTimerRef.current);
                  // drag_start：指针仍按住时启动持续维持
                  if (type === 'drag_start') {
                    if (interactionStateRef.current.isDragging) {
                      startDragHold(fb.expression);
                    }
                  } else {
                    // 非 drag_start 交互表情：按 duration 自动恢复默认表情
                    const duration = (fb.duration_ms && fb.duration_ms > 0) ? fb.duration_ms : 3000;
                    expressionTimerRef.current = setTimeout(() => {
                      if (dragHoldExpressionRef.current === null) {
                        safeResetExpression();
                      }
                      expressionTimerRef.current = null;
                    }, duration);
                  }
                }
                // 动作用 model.motion() 播放
                if (fb.motion && fb.motion !== 'idle') {
                  try {
                    model.motion(fb.motion);
                  } catch {
                    /* 未知动作忽略 */
                  }
                }
                // 连续点击触发的避让鼠标（基于心理参数概率触发）
                if (fb.avoid_mouse) {
                  invoke('set_avoid_mouse', { enabled: true, characterId: getCharacterId() ?? undefined }).catch(() => {
                    /* 后端未就绪忽略 */
                  });
                }
              })
              .catch(() => {
                /* 后端未就绪忽略 */
              });
          };

          model.on('pointerdown', (e: any) => {
            onModelClick?.();
            const pt = e?.data?.global ?? { x: 0, y: 0 };
            const detected = detectInteraction(
              interactionStateRef.current,
              'pointerdown',
              pt.x,
              pt.y,
            );
            if (detected) {
              handleInteraction(detected);
              // drag_start 时不播 Tap 动作，避免覆盖伸手表情
              if (detected === 'drag_start') return;
            }
            try {
              model.motion('Tap');
            } catch {
              /* 忽略不存在的动作组 */
            }
          });

          // 鼠标跟随：只调整模型内部参数（眼睛/头部朝向），不移动模型位置
          const canvas = app.view as HTMLCanvasElement;

          // 缓存 canvas rect 避免每次 mousemove 都触发 getBoundingClientRect 强制布局重算
          // （mousemove 可达 60-120 次/秒，原实现每秒数百次布局刷新）
          // mouseenter / resize 时失效，下次 mousemove 重新读取
          let cachedRect: DOMRect | null = null;
          const invalidateRect = () => {
            cachedRect = null;
          };
          const getRect = (): DOMRect => {
            if (cachedRect) return cachedRect;
            cachedRect = canvas.getBoundingClientRect();
            return cachedRect;
          };

          // 窗口内鼠标移动：用于交互检测（按住拖动 / 悬停抚摸），不在此处调用 focus
          const handleMouseMove = (e: MouseEvent) => {
            const rect = getRect();
            const x = e.clientX - rect.left;
            const y = e.clientY - rect.top;

            // 交互检测：拖动中（fast_drag）或悬停面部区域（pet）
            const detected = detectInteraction(
              interactionStateRef.current,
              'pointermove',
              x,
              y,
              rect.height,
            );
            if (detected) {
              handleInteraction(detected);
            }
          };

          // pointerup：结束按住拖动状态
          const handleMouseUp = (e: MouseEvent) => {
            // 松开鼠标时，清除按住维持的表情（无论是否真正拖动过）
            if (dragHoldExpressionRef.current) {
              clearDragHold();
              safeResetExpression();
            }
            const rect = getRect();
            const x = e.clientX - rect.left;
            const y = e.clientY - rect.top;
            const detected = detectInteraction(
              interactionStateRef.current,
              'pointerup',
              x,
              y,
            );
            if (detected) {
              handleInteraction(detected);
            }
          };

          // 每个窗口使用独立 ticker，避免多窗口共享 Ticker 导致的竞争和异常
          const dpr = window.devicePixelRatio || 1;

          // 前端自主驱动 microTick（呼吸 + 身体微晃 + 视线游移）和 emotionFacsTick（情绪 FACS）
          const tickerCallback = () => {
            microTickRef.current?.();
            emotionFacsTickRef.current?.();
          };
          app.ticker.add(tickerCallback);
          (app as any)._vivianTickerCallback = tickerCallback;

          // 前端鼠标跟随：监听 canvas mousemove，无需 Rust eval 注入
          const handleCanvasMouseMove = (e: MouseEvent) => {
            const m = modelRef.current;
            if (!m) return;
            const mode = mouseFollowModeRef.current;
            if (mode === 'off') return;
            // 鼠标在窗口内移动时持续刷新跟随窗口；鼠标静止或离开后 5~8s 回归自主行为
            refreshInteractionFollow();

            const rect = getRect();
            const cx = rect.width > 0 ? (e.clientX - rect.left) / rect.width : 0.5;
            const cy = rect.height > 0 ? (e.clientY - rect.top) / rect.height : 0.5;
            const fx = Math.max(-1, Math.min(1, cx * 2 - 1));
            const fy = Math.max(-1, Math.min(1, -(cy * 2 - 1)));
            const fc = (m as unknown as {
              internalModel?: { focusController?: { focus?: (x: number, y: number) => void } };
            }).internalModel?.focusController;
            if (fc && typeof fc.focus === 'function') {
              fc.focus(fx, fy);
            } else {
              m.focus(cx * (m.width || 1), cy * (m.height || 1));
            }
          };
          canvas.addEventListener('mousemove', handleCanvasMouseMove);
          (app as any)._vivianMouseMove = handleCanvasMouseMove;

          // 鼠标进入/离开窗口事件检测
          const handleMouseEnter = () => {
            // 鼠标进入时失效 rect 缓存（窗口可能被移动或缩放，下次 mousemove 重新读取）
            invalidateRect();
            // 鼠标进入窗口时刷新跟随窗口（让桌宠看一眼鼠标）
            refreshInteractionFollow();
            const detected = detectInteraction(
              interactionStateRef.current,
              'mouseenter',
              0, 0,
            );
            if (detected) {
              handleInteraction(detected);
            }
          };
          const handleMouseLeave = () => {
            const detected = detectInteraction(
              interactionStateRef.current,
              'mouseleave',
              0, 0,
            );
            if (detected) {
              handleInteraction(detected);
            }
          };
          canvas.addEventListener('mouseenter', handleMouseEnter);
          canvas.addEventListener('mouseleave', handleMouseLeave);
          (app as any)._vivianMouseEnter = handleMouseEnter;
          (app as any)._vivianMouseLeave = handleMouseLeave;

          // 全局鼠标up/事件监听（用于拖出窗口时结束拖动）
          window.addEventListener('mouseup', handleMouseUp);
          window.addEventListener('mousemove', handleMouseMove);
          (app as any)._vivianWindowMouseUp = handleMouseUp;
          (app as any)._vivianWindowMouseMove = handleMouseMove;

          // 窗口失去焦点时自动清除 dragHold（防止鼠标在窗口外释放导致伸手表情卡住）
          const handleWindowBlur = () => {
            if (dragHoldExpressionRef.current) {
              clearDragHold();
              safeResetExpression();
            }
          };
          window.addEventListener('blur', handleWindowBlur);
          (app as any)._vivianWindowBlur = handleWindowBlur;

          // 窗口尺寸变化时失效 rect 缓存（如 Tauri 窗口被 resize）
          window.addEventListener('resize', invalidateRect);
          (app as any)._vivianWindowResize = invalidateRect;

          // pointercancel：系统取消指针交互时（窗口状态变化/点击穿透/系统级鼠标捕获丢失）
          // 清除 dragHold，防止伸手表情卡住。与 mouseup/blur 形成三重兜底。
          const handlePointerCancel = () => {
            if (dragHoldExpressionRef.current) {
              clearDragHold();
              safeResetExpression();
            }
          };
          canvas.addEventListener('pointercancel', handlePointerCancel);
          (app as any)._vivianPointerCancel = handlePointerCancel;

          // 后端 cursor_tracking 线程推送全局光标坐标，实现跨窗口鼠标跟随。
          // 解决点击穿透导致 canvas 收不到 mousemove 的问题：
          // - 鼠标在窗口矩形内时跟随（handleCanvasMouseMove 已刷新跟随窗口）
          // - 鼠标离开窗口后 5~8s 回归自主行为（interactionFollowUntil 倒计时）
          const myCharId = getWindowCharacterId();
          void (async () => {
            try {
              const unlisten = await listen<{
                character_id: string;
                cursor_x: number;
                cursor_y: number;
                window_x: number;
                window_y: number;
                window_w: number;
                window_h: number;
              }>('cursor:position', (event) => {
                // 过滤：只处理发给本角色的坐标
                if (myCharId && event.payload.character_id !== myCharId) return;
                const m = modelRef.current;
                if (!m) return;
                const mode = mouseFollowModeRef.current;
                if (mode === 'off') return;
                // 仅在交互触发的跟随窗口内才跟随（鼠标静止或离开后回归自主）
                if (!isInteractionFollowActive()) return;

                const { cursor_x, cursor_y, window_x, window_y, window_w, window_h } = event.payload;
                if (window_w <= 0 || window_h <= 0) return;

                // 鼠标不在窗口矩形内则不跟随（移除全屏跟随，避免无时不刻盯鼠标）
                const insideWindow =
                  cursor_x >= window_x &&
                  cursor_x <= window_x + window_w &&
                  cursor_y >= window_y &&
                  cursor_y <= window_y + window_h;
                if (!insideWindow) return;

                // 将全局坐标转换为相对窗口的归一化坐标 [-1, 1]
                const cx = (cursor_x - window_x) / window_w;
                const cy = (cursor_y - window_y) / window_h;
                const fx = Math.max(-1, Math.min(1, cx * 2 - 1));
                const fy = Math.max(-1, Math.min(1, -(cy * 2 - 1)));
                const fc = (m as unknown as {
                  internalModel?: { focusController?: { focus?: (x: number, y: number) => void } };
                }).internalModel?.focusController;
                if (fc && typeof fc.focus === 'function') {
                  fc.focus(fx, fy);
                } else {
                  m.focus(cx * (m.width || 1), cy * (m.height || 1));
                }
              });
              if (destroyed) {
                unlisten();
              } else {
                cursorUnlistenRef.current = unlisten;
              }
            } catch {
              /* 监听失败忽略 */
            }
          })();

          // Rust 光标追踪：拖动移动窗口 + 点击穿透切换 + 推送全局光标坐标
          void invoke('start_cursor_tracking', { characterId: myCharId });

          // 注：handleMouseMove / handleMouseUp 已在 window 级注册（见上文 _vivianWindowMouseUp/Move），
          // 鼠标在 canvas 上移动时事件会冒泡到 window，无需在 canvas 上重复注册。
          // 此前在此处重复注册导致每次 mousemove 触发两次 detectInteraction + invoke('apply_user_interaction')，
          // 同时这两个监听器未保存引用，cleanup 无法移除，造成内存泄漏。

          // Ctrl+滚轮缩放：deltaY 正值=向下滚(缩小)，负值=向上滚(放大)
          // 使用更温和的缩放系数，每 100px 滚轮位移约 5% 变化
          const handleWheel = (e: WheelEvent) => {
            if (!e.ctrlKey) return;
            e.preventDefault();
            e.stopPropagation();
            // 根据 deltaMode 调整系数：deltaMode=0 是像素级(触摸板)，deltaMode=1 是行级(鼠标滚轮)
            const modeFactor = e.deltaMode === 1 ? 16 : 1;
            const delta = e.deltaY * modeFactor;
            const factor = 1 - delta * SCROLL_SENSITIVITY;
            const newScale = Math.max(SCALE_MIN, Math.min(SCALE_MAX, userScaleRef.current * factor));
            if (Math.abs(newScale - userScaleRef.current) > 0.001) {
              userScaleRef.current = newScale;
              onScaleChangeRef.current?.(newScale);
            }
          };
          canvas.addEventListener('wheel', handleWheel, { passive: false });

          // 保存引用以便清理
          (app as any)._vivianCanvasMouseMove = handleCanvasMouseMove;
          (app as any)._vivianWheel = handleWheel;

          // 表情结束回调通过轮询检测（Cubism SDK 事件名不统一）
          try {
            const em = model.internalModel?.motionManager?.expressionManager;
            if (em && typeof (em as any).on === 'function') {
              (em as any).on('destroyExpression', () => onExpressionEnd?.());
            }
          } catch {
            /* 表情回调非关键，忽略 */
          }

          // 获取模型显示缩放系数（留白补偿），然后 fitModel
          // 使用 getWindowCharacterId() 确保获取当前窗口的角色 ID，避免多窗口干扰
          try {
            const charId = getWindowCharacterId();
            const { display_scale } = await invoke<{ display_scale: number }>('get_display_scale', {
              characterId: charId,
            });
            displayScaleRef.current = display_scale ?? 1.0;
          } catch {
            /* 后端未就绪时使用默认 1.0 */
          }

          fitModel();
          void applyBlinkInterval();
          // 模型加载完成后立即应用默认表情，确保瞳孔/耳朵可见
          try {
            model.expression(getDefaultExpression());
          } catch {
            /* 忽略 */
          }
          onReady?.();
        } catch (err) {
          console.error('[Live2D] 模型加载失败:', err);
          if (!destroyed) {
            setFailed(err instanceof Error ? err.message : String(err));
          }
        }
      };

      void init();

      const handleResize = () => {
        const app = appRef.current;
        const container = containerRef.current;
        if (!app || !container) return;
        app.renderer.resize(container.clientWidth, container.clientHeight);
        fitModel();
      };
      window.addEventListener('resize', handleResize);

      return () => {
        destroyed = true;
        window.removeEventListener('resize', handleResize);
        // 清理长按定时器
        const intState = interactionStateRef.current;
        if (intState.longPressTimer !== null) {
          clearTimeout(intState.longPressTimer);
          intState.longPressTimer = null;
        }
        // 清理按住拖动维持表情的定时器
        if (dragHoldTimerRef.current) {
          clearInterval(dragHoldTimerRef.current);
          dragHoldTimerRef.current = null;
        }
        dragHoldExpressionRef.current = null;
        // 移除 ticker 回调和事件监听
        const app = appRef.current;
        if (app) {
          const canvas = app.view as HTMLCanvasElement;
          const tickerCb = (app as any)._vivianTickerCallback as (() => void) | undefined;
          if (tickerCb) app.ticker.remove(tickerCb);
          // 清理canvas事件
          const mouseHandler = (app as any)._vivianMouseMove as ((e: MouseEvent) => void) | undefined;
          if (mouseHandler) canvas.removeEventListener('mousemove', mouseHandler);
          const mouseUpHandler = (app as any)._vivianWindowMouseUp as ((e: MouseEvent) => void) | undefined;
          if (mouseUpHandler) window.removeEventListener('mouseup', mouseUpHandler);
          const windowMouseMove = (app as any)._vivianWindowMouseMove as ((e: MouseEvent) => void) | undefined;
          if (windowMouseMove) window.removeEventListener('mousemove', windowMouseMove);
          const windowBlurHandler = (app as any)._vivianWindowBlur as (() => void) | undefined;
          if (windowBlurHandler) window.removeEventListener('blur', windowBlurHandler);
          const windowResizeHandler = (app as any)._vivianWindowResize as (() => void) | undefined;
          if (windowResizeHandler) window.removeEventListener('resize', windowResizeHandler);
          const pointerCancelHandler = (app as any)._vivianPointerCancel as (() => void) | undefined;
          if (pointerCancelHandler) canvas.removeEventListener('pointercancel', pointerCancelHandler);
          const mouseEnterHandler = (app as any)._vivianMouseEnter as ((e: MouseEvent) => void) | undefined;
          if (mouseEnterHandler) canvas.removeEventListener('mouseenter', mouseEnterHandler);
          const mouseLeaveHandler = (app as any)._vivianMouseLeave as ((e: MouseEvent) => void) | undefined;
          if (mouseLeaveHandler) canvas.removeEventListener('mouseleave', mouseLeaveHandler);
          const wheelHandler = (app as any)._vivianWheel as ((e: WheelEvent) => void) | undefined;
          if (wheelHandler) canvas.removeEventListener('wheel', wheelHandler);
        }
        // 清理 cursor:position 事件监听
        cursorUnlistenRef.current?.();
        cursorUnlistenRef.current = null;
        // 不在此处调用 stop_cursor_tracking：光标追踪线程是应用级单例，
        // 生命周期由 Rust 端 on_window_event (CloseRequested) 统一管理。
        // 此前在 cleanup 中无条件调用 stop 会误杀全局线程，导致其他角色
        // 窗口的光标追踪同时失效（组件 unmount / strict mode 双挂载均会触发）。
        if (expressionTimerRef.current) {
          clearTimeout(expressionTimerRef.current);
          expressionTimerRef.current = null;
        }
        if (mixerRef.current) {
          mixerRef.current.destroy();
          mixerRef.current = null;
        }
        try {
          modelRef.current?.destroy();
        } catch {
          /* ignore */
        }
        try {
          appRef.current?.destroy(true);
        } catch {
          /* ignore */
        }
        modelRef.current = null;
        appRef.current = null;
      };
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [modelUrl]);

    useEffect(() => {
      mouseFollowModeRef.current = mouseFollowMode;
    }, [mouseFollowMode]);

    useEffect(() => {
      onScaleChangeRef.current = onScaleChange;
    }, [onScaleChange]);

    // 配置保存后重新应用眨眼间隔（让设置窗口的修改实时生效）
    useEffect(() => {
      let cancelled = false;
      let unlistenFn: (() => void) | undefined;
      void (async () => {
        try {
          unlistenFn = await listen('config:saved', () => {
            void applyBlinkInterval();
          });
          if (cancelled) { unlistenFn(); return; }
        } catch {
          /* ignore */
        }
      })();
      return () => {
        cancelled = true;
        unlistenFn?.();
      };
    }, [applyBlinkInterval]);

    useImperativeHandle(
      ref,
      () => ({
        setExpression: (name, durationMs) => {
          // 按住拖动期间，伸手表情拥有最高优先级，禁止被覆盖
          if (dragHoldExpressionRef.current !== null) return;
          const model = modelRef.current;
          if (!model) return;
          // 空名/default/neutral 视为重置到默认表情
          if (!name || name === 'default' || name === 'idle' || name === 'neutral') {
            safeResetExpressionRef.current();
            return;
          }
          if (expressionTimerRef.current) clearTimeout(expressionTimerRef.current);
          // 直接应用新 expression，SDK crossfade 同时管理旧表情的 fade-out
          // 和新表情的 fade-in，Part Opacity（ArtMesh 不透明度）平滑过渡
          try {
            model.expression(name);
          } catch {
            /* 忽略未知表情 */
          }
          if (durationMs && durationMs > 0) {
            expressionTimerRef.current = setTimeout(() => {
              // duration 到期后恢复默认表情，避免 Part Opacity 残留导致瞳孔/耳朵消失
              if (dragHoldExpressionRef.current === null) {
                safeResetExpressionRef.current();
              }
              onExpressionEnd?.();
              expressionTimerRef.current = null;
            }, durationMs);
          }
        },
        playMotion: (group, index = 0) => {
          const model = modelRef.current;
          if (!model) return;
          try {
            model.motion(group, index);
          } catch {
            /* 忽略未知动作 */
          }
        },
        focus: (x, y) => {
          const model = modelRef.current;
          if (!model) return;
          try {
            model.focus(x, y);
          } catch {
            /* ignore */
          }
        },
        getModel: () => modelRef.current,
        // 设置用户缩放因子（1.0=默认大小）
        setScale: (scale) => {
          const clamped = Math.max(SCALE_MIN, Math.min(SCALE_MAX, scale));
          if (Math.abs(clamped - userScaleRef.current) > 0.001) {
            userScaleRef.current = clamped;
            onScaleChangeRef.current?.(clamped);
          }
        },
        getScale: () => userScaleRef.current,
        // 重置模型到 fitModel 计算的默认缩放与居中位置
        refitModel: () => {
          userScaleRef.current = 1.0;
          fitModel();
          onScaleChangeRef.current?.(1.0);
        },
        // 重置当前表情并恢复默认表情
        resetExpression: () => {
          // 按住拖动期间，伸手表情拥有最高优先级，禁止被重置
          if (dragHoldExpressionRef.current !== null) return;
          if (expressionTimerRef.current) {
            clearTimeout(expressionTimerRef.current);
            expressionTimerRef.current = null;
          }
          safeResetExpressionRef.current();
        },
      }),
      [onExpressionEnd],
    );

    if (failed) {
      return (
        <div
          style={{
            width: '100%',
            height: '100%',
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            padding: 12,
            color: 'var(--text-secondary)',
            fontSize: 14,
            textAlign: 'center',
          }}
        >
          {t('live2d.load_failed_title')}
          <div
            style={{
              fontSize: 11,
              marginTop: 8,
              wordBreak: 'break-all',
              opacity: 0.7,
              maxWidth: '100%',
            }}
          >
            {t('live2d.load_failed_hint')}
          </div>
        </div>
      );
    }

    return <div ref={containerRef} style={{ width: '100%', height: '100%' }} />;
  },
);

export default Live2DCanvas;
