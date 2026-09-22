import { useCallback, useEffect, useRef, useState } from 'react';
import { getCurrentWindow, currentMonitor, LogicalSize, LogicalPosition } from '@tauri-apps/api/window';
import type { Effect } from '@tauri-apps/api/window';
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { listen, emit, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { useAppStore, hasPersistedVoiceEnabled } from './stores/useAppStore';
import { useTranslation } from 'react-i18next';
import {
  useConfig,
  useEnvironment,
  useMood,
  useProactive,
  useTTS,
  useExtractFileText,
  type FileTextResult,
} from './hooks/useTauriCommands';
import type { ProactiveMessage, ProactiveTickContext, TtsConfig } from './types';
import { ModelCanvas, type ModelRendererHandle } from './components/ModelCanvas';
import type { ChibiInteraction } from './components/ChibiPetCanvas';
import VideoAnimationLayer from './components/VideoAnimationLayer';
import SystemTray, { syncTrayMenuCheck } from './components/SystemTray';
import type { ToastType, ToastAction } from './components/Toast';
import { PROVIDER_PRESETS } from './components/ConfigWindow';
import { ChatController } from './controllers/ChatController';
import { BubbleController, computeDuration } from './controllers/BubbleController';
import { TtsStreamQueue } from './controllers/TtsStreamQueue';
import { LifecycleController } from './controllers/LifecycleController';
import { useHiding } from './hooks/useHiding';
import type { Corner, HideReason } from './hooks/useHiding';
import { useSmartPositioning } from './hooks/useSmartPositioning';
import { positioningCoordinator } from './hooks/positioningCoordinator';
import { FLEE_TAKEOVER_HOLD_MS } from './chibi/fleeTrack';
import { changeLanguage } from './i18n';
import type { BubblePosition } from './components/MessageBubble';
import { getCharacterId } from './characterContext';
import { stripActions } from './utils/ActionText';
import { raiseWindow, isWindowOnScreen, RAISE_UNLISTEN } from './utils/windowRaiser';
import { openRoomWindow } from './utils/roomWindow';
import { buildPetRectQuery, buildPrewarmQuery, emitPetReveal, PET_PREWARM_READY_EVENT, type PetPrewarmReady, type PetRect } from './utils/petReveal';
import HoldProgressRing from './components/HoldProgressRing';

const ENVIRONMENT_UPDATE_INTERVAL_MS = 30_000;
/** 兜底轮询间隔（防 pet:action_pending 事件丢失；事件驱动为主，降频减少 IPC） */
const PET_ACTION_DRAIN_INTERVAL_MS = 2500;
const IDLE_AWAY_THRESHOLD_SECONDS = 300;

/**
 * 按厂商 endpoint 反查预设（忽略结尾斜杠与大小写，前缀匹配以兼容用户
 * 在 endpoint 后追加路径的写法，如 `https://api.deepseek.com/v1`）。
 * 找不到（custom endpoint）返回 undefined，错误 toast 就不挂控制台动作。
 */
function findProviderPresetByEndpoint(endpoint: string) {
  const norm = endpoint.replace(/\/+$/, '').toLowerCase();
  if (!norm) return undefined;
  return PROVIDER_PRESETS.find((p) => {
    const urls = [p.endpoint, ...(p.protocols ?? []).map((pr) => pr.endpoint)];
    return urls.some((u) => u && norm.startsWith(u.replace(/\/+$/, '').toLowerCase()));
  });
}
/** 等待 Brain 初始化的超时（毫秒），超时后用兜底问候
 *  预加载流程包含种子记忆注入与情绪/语义语料嵌入，远程嵌入可能需要较长时间，
 *  因此超时放宽到 120s；后端 `send_message` 在初始化完成前也会拒绝请求作为双保险。 */
const APP_READY_TIMEOUT_MS = 120_000;

/** 气泡子窗口尺寸（逻辑像素） */
const BUBBLE_WINDOW_WIDTH = 340;
const BUBBLE_WINDOW_HEIGHT = 140;
/** 气泡动态扩大的高度上下限（流式输出时按文本长度自适应） */
const BUBBLE_WINDOW_MIN_HEIGHT = 100;
const BUBBLE_WINDOW_MAX_HEIGHT = 420;

const SIDE_CHAT_WIDTH = 320;

/** 长按桌宠打开心智观察器：显示环形进度前的静默期（毫秒） */
const HOLD_RING_DELAY_MS = 200;
/** 长按桌宠打开心智观察器的总时长（毫秒） */
const HOLD_OPEN_TOTAL_MS = 1000;
/** 长按期间允许的窗口位移容差（物理像素），超过判定为拖拽并取消 */
const HOLD_MOVE_TOLERANCE_PX = 10;
/** 长按开始多久后提前去加载心智观察器窗口（毫秒）。
 *
 *  窗口的加载（WebView 冷启 + React 挂载 + 首屏数据）是这条链上最慢的一环，
 *  而长按判定本身要 1s。等到长按成立才去建窗口，用户松手后还得再等一截才看到
 *  界面；提前起步能把这截等待基本吃干净。0.1s 是个折中：比进度环（0.2s）更早，
 *  又不至于让「点一下 / 拖走」这类根本不会成立的手势白建窗口。 */
const HOLD_PREWARM_DELAY_MS = 100;
/** 预热就绪回执的等待上限（毫秒）：超时就直接按常规路径打开。
 *  回执本身只是一条事件，丢了（IPC 异常 / 监听未挂上）不该让长按毫无反应——
 *  宁可少一段入场动画，也不能不打开。 */
const PREWARM_READY_WAIT_MS = 1200;
/** 后端 drag:dizzy 事件未带时长时的兜底晕眩时长（毫秒） */
const DIZZY_FALLBACK_MS = 1500;

/** 根据文本长度估算气泡窗口所需高度（逻辑像素）
 *
 *  气泡内文本 maxWidth≈300、padding 10x14、fontSize 14、lineHeight 1.55。
 *  CJK 与英文混排时按 ~20 字/行估算，同时尊重显式换行。
 */
function estimateBubbleHeight(text: string): number {
  if (!text) return BUBBLE_WINDOW_HEIGHT;
  const lines = text.split('\n');
  let totalLines = 0;
  for (const line of lines) {
    const len = line.length;
    totalLines += Math.max(1, Math.ceil(len / 20));
  }
  // 每行 ~22px + 上下 padding 20 + 尾巴 8 + 容器边距 16
  const h = totalLines * 22 + 44;
  return Math.min(BUBBLE_WINDOW_MAX_HEIGHT, Math.max(BUBBLE_WINDOW_MIN_HEIGHT, h));
}
/**
 * Q 版图集的每个状态格都是同一尺寸，因此两个角色共用同一逻辑窗口。
 * 不再沿用旧 角色模型各自不同的画布比例，避免 Nana 被额外放大约 19%。
 * 默认尺寸为上一版 355.33×411.33 的 3/5。
 */
const CHARACTER_WINDOW_SIZE = { w: 213.2, h: 246.8 };
const getWindowSize = (_charId: string | null) => CHARACTER_WINDOW_SIZE;

/**
 * 用户对桌宠的动作 —— 与后端 `generate_pet_reaction` 的 `action` 参数一一对应。
 *
 * 前四个来自 ChibiPetCanvas 的点击手势与长按进度环（`rough_click` 是画布判定
 * 「戳得太频繁/太急」后的结果），后两个来自后端甩飞线程发出的 `drag:dizzy` 事件。
 */
type PetAction =
  | 'single_click'
  | 'double_click'
  | 'rough_click'
  | 'long_press'
  | 'fast_drag'
  | 'edge_bounce';

let petSfxContext: AudioContext | null = null;

/** 用户手势内合成一枚极短提示音，不读取文件也不阻塞本地回应。 */
function playPetTapSound(characterId: 'vivian' | 'nana'): void {
  try {
    const AudioContextCtor = window.AudioContext
      || (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!AudioContextCtor) return;
    petSfxContext ??= new AudioContextCtor();
    const ctx = petSfxContext;
    if (ctx.state === 'suspended') void ctx.resume().catch(() => {});
    const notes = characterId === 'nana' ? [740, 988] : [880, 1175];
    const startAt = ctx.currentTime;
    notes.forEach((frequency, index) => {
      const oscillator = ctx.createOscillator();
      const gain = ctx.createGain();
      const noteStart = startAt + index * 0.055;
      oscillator.type = 'sine';
      oscillator.frequency.setValueAtTime(frequency, noteStart);
      gain.gain.setValueAtTime(0.0001, noteStart);
      gain.gain.exponentialRampToValueAtTime(0.055, noteStart + 0.012);
      gain.gain.exponentialRampToValueAtTime(0.0001, noteStart + 0.09);
      oscillator.connect(gain);
      gain.connect(ctx.destination);
      oscillator.start(noteStart);
      oscillator.stop(noteStart + 0.1);
    });
  } catch {
    // 音频设备不可用时仍保留文字和动作反馈。
  }
}

/** Toast 子窗口尺寸（逻辑像素）—— 覆盖屏幕右下角区域以容纳堆叠 toast */
const TOAST_WINDOW_WIDTH = 400;
const TOAST_WINDOW_HEIGHT = 320;

/** 记忆向量重建进度 toast 的固定 key：进度更新原地刷新同一条 toast，避免堆叠 */
const REBUILD_TOAST_KEY = 864201;
/** 嵌入模型就绪 toast 的固定 key：防止 listener 泄漏导致重复弹窗时原地刷新 */
const OLLAMA_READY_TOAST_KEY = 864202;

/** 后端 tool:confirmation_request 事件载荷（原样转发给 toast 子窗口渲染三按钮确认卡片） */
interface ToolConfirmPayload {
  request_id: number;
  tool: string;
  arguments: unknown;
  reason: string;
  risk_level: 'low' | 'medium' | 'high';
  char_id: string;
  allow_always_scope: 'persistent' | 'session';
}

/**
 * 安全调用 Tauri unlisten 函数。
 * Tauri v2 的 listen() 在 Rust 端通过 eval() 异步注册 JS 侧监听器，但 invoke 在 eval
 * 完成前就返回 eventId。若 React StrictMode 在 listen 刚 resolve 时立即清理 effect，
 * unlisten() 会在 listeners[eventId] 尚未写入时调用 unregisterListener，抛出
 * "Cannot read properties of undefined (reading 'handlerId')"。unlisten 运行时是 async
 * 函数（类型标注为 () => void），未 catch 的 rejection 成为 uncaught promise error。
 */
function safeUnlisten(fn?: (() => void) | undefined): void {
  if (!fn) return;
  try {
    void Promise.resolve(fn()).catch(() => {});
  } catch {
    /* ignore */
  }
}

/**
 * 等待后端 Brain 初始化完成。
 * 先查 is_initialized（防止事件已发过后才监听），再监听 app:ready 事件，带超时兜底。
 */
async function waitForAppReady(): Promise<void> {
  const cid = getCharacterId();
  console.log(`[DIAG] waitForAppReady START, char=${cid}, time=${Date.now()}`);
  // 快速路径：Brain 可能已初始化完成
  try {
    const ready = await invoke<boolean>('is_initialized');
    console.log(`[DIAG] is_initialized=${ready}, char=${cid}`);
    if (ready) {
      console.log(`[DIAG] waitForAppReady FAST PATH done, char=${cid}`);
      return;
    }
  } catch {
    /* ignore */
  }
  // 监听 app:ready 事件
  await new Promise<void>((resolve) => {
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      resolve();
    };
    void listen('app:ready', finish).then((un) => {
      // 如果事件已错过，unlisten 后走超时兜底
      if (done) un();
      else setTimeout(() => { un(); finish(); }, APP_READY_TIMEOUT_MS);
    });
    // 超时兜底，确保 UI 不会永远卡住
    setTimeout(finish, APP_READY_TIMEOUT_MS);
  });
  console.log(`[DIAG] waitForAppReady DONE, char=${cid}, time=${Date.now()}`);
}

/** GPT-SoVITS 服务就绪等待上限（毫秒）。
 *  后端 wait_for_health 自身超时 60s，前端略短以便更早走 fallback。 */
const GPT_SOVITS_READY_TIMEOUT_MS = 30_000;
/** 轮询间隔（与后端 wait_for_health 一致） */
const GPT_SOVITS_POLL_INTERVAL_MS = 1_500;

/** 等待 GPT-SoVITS 服务进入 running 状态。
 *
 *  仅在配置 engine=gptsovits 且开启 auto_start 时调用：
 *  auto_start 触发的服务启动是异步的，问候朗读若抢先发起会连接失败。
 *  running 立即返回；crashed/stopped 或超时则放弃（交给后端 fallback）。 */
async function waitForGptSoVitsReady(): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < GPT_SOVITS_READY_TIMEOUT_MS) {
    try {
      const st = await invoke<{ status: string }>('get_gpt_sovits_service_status');
      if (st.status === 'running') {
        console.log(`[Lifecycle] GPT-SoVITS 就绪, 耗时 ${Date.now() - start}ms`);
        return;
      }
      if (st.status === 'crashed' || st.status === 'stopped') {
        console.warn(`[Lifecycle] GPT-SoVITS 状态=${st.status}, 放弃等待走 fallback`);
        return;
      }
    } catch {
      /* 查询失败继续轮询 */
    }
    await new Promise((r) => setTimeout(r, GPT_SOVITS_POLL_INTERVAL_MS));
  }
  console.warn(`[Lifecycle] GPT-SoVITS 等待超时(${GPT_SOVITS_READY_TIMEOUT_MS}ms), 走 fallback`);
}

/** 已打开的子窗口引用追踪。
 *  防止 JS 侧 WebviewWindow 引用被 GC 回收，同时用于陈旧引用检测：
 *  getByLabel 可能返回已关闭但未清理的窗口引用，isVisible() 可区分活性。 */
const CHILD_WINDOWS = new Map<string, WebviewWindow>();

/** 已注册过关闭清理的子窗口 label。
 *  openWindow 的"复用已有窗口"路径可能被反复调用（右键菜单/快捷键/聚焦恢复），
 *  onCloseRequested 是追加式订阅，不去重的话每次调用都多挂一个监听器
 *  （长驻进程内无界累积）。窗口真正关闭时在清理回调里摘除本标记。 */
const CLOSE_CLEANUP_REGISTERED = new Set<string>();

/** 共享子窗口集合：任一角色右键菜单打开的都是同一实例。
 *  这些窗口在顶部提供 Vivian / Nana / 综合 三视图切换，不绑定具体角色。
 *  其余子窗口（bubble/toast）按角色隔离，各自独立。 */
// 注：room（3D 公寓）不在其中——它有独立的固定全屏/无边框/透明形态，
// 走 utils/roomWindow 的专用入口，不走这里的通用 openWindow。
const SHARED_SUBWINDOWS = new Set(['chat', 'config', 'memory', 'input']);

/** 子窗口 label 生成。
 *  - 共享子窗口（chat/config/memory）：返回 base label，所有角色复用同一实例
 *  - 角色私有子窗口（bubble/toast）：按 `${charId}_${base}` 前缀隔离 */
function charScopedLabel(base: string): string {
  if (SHARED_SUBWINDOWS.has(base)) return base;
  return `${getCharacterId() ?? 'main'}_${base}`;
}

/** 自显形窗口的兜底超时：子窗口若因事件丢失或脚本异常始终没显形，到这个点强制显示，
 *  避免窗口永远不出现（长按看起来毫无反应）。1600ms 是给 webview 冷启动留的余量。 */
const SELF_REVEAL_FALLBACK_MS = 1600;

/** 给「显形归子窗口」的窗口挂一个兜底显形。
 *
 *  子窗口负责显形是入场动画的前提（先摆好首帧再让窗口出现），代价是显形这条链
 *  多了一环：事件没送到、或它自己脚本异常，窗口就会一直不出现。这里补一个超时兜底，
 *  到点还没在屏上就自己把它放出来——宁可直接显形、没有动画，也不能让长按没反应。
 *  最小化也算「没在屏上」，所以还原要在 show 之前。 */
function armSelfRevealFallback(win: WebviewWindow): void {
  window.setTimeout(() => {
    void (async () => {
      try {
        if (await isWindowOnScreen(win)) return;
        await win.unminimize();
        await win.show();
      } catch {
        /* 窗口已销毁 */
      }
    })();
  }, SELF_REVEAL_FALLBACK_MS);
}

/** 创建中的窗口：label → 创建落地（`tauri://created` / `tauri://error`）的 promise。
 *
 *  Tauri 的窗口创建是异步的：`new WebviewWindow()` 返回时 label 还没进 Rust 侧的
 *  窗口注册表，这段时间里 `getByLabel` 查不到它、`CHILD_WINDOWS` 里却已经有引用。
 *  若第二个调用者在这段窗口期走到「查注册表」那一步，就会用同一个 label 再建一次，
 *  必然报 label 冲突，对外表现是「点了毫无反应」。
 *
 *  预热正好把这段窗口期放大了：窗口在长按 0.1s 时开始建，长按 1s 成立时才去打开，
 *  两个动作分处不同的调用栈，靠「碰巧建完了」是不可靠的。所以这里显式等它落地。 */
const WINDOW_CREATION = new Map<string, Promise<void>>();

/** 等创建落地的最长时间（毫秒）：创建卡死时不能把后续调用一起拖住 */
const WINDOW_CREATION_TIMEOUT_MS = 3000;

/** 桌宠窗口当前的矩形（物理像素）；取不到返回 null（调用方退化为无入场动画） */
async function readPetRect(): Promise<PetRect | null> {
  try {
    const petWin = getCurrentWindow();
    const [pos, size] = await Promise.all([petWin.outerPosition(), petWin.outerSize()]);
    return { x: pos.x, y: pos.y, w: size.width, h: size.height };
  } catch {
    return null;
  }
}

/** 销毁一个「预热出来但没派上用场」的窗口。
 *
 *  长按中止（松手 / 拖动超容差）时调用：这个窗口是那次长按的私产，长按没成立就
 *  不该留下任何东西——既不能显示，也不该在后台常驻（心智观察器是整页应用，
 *  白留一份 webview 的内存不划算）。下一次长按会重新预热，成本已经被 0.9s 的
 *  提前量吸收掉了。
 *
 *  一道保险：窗口若已经被别的入口摆上屏（长按期间用户又从托盘点了一次），
 *  就不再销毁——那已经是用户要看的东西，不能替他关掉。
 *
 *  调用前提：句柄来自「预热时 getByLabel 查不到、于是新建」的那条路，所以关掉的
 *  一定是本次预热自己建的窗口。已存在（被最小化 / hide）的窗口走的是认领分支，
 *  压根不会拿到这里来销毁——那种窗口里有用户可能没保存的输入。 */
async function destroyPrewarmedWindow(win: WebviewWindow): Promise<void> {
  try {
    if (await isWindowOnScreen(win)) return;
    await win.close();
  } catch {
    /* 已销毁 / IPC 失败：无需处理 */
  }
}

/** 一次长按预热会话。
 *
 *  生命周期：长按 0.1s 时开一个会话（建窗口 / 认领已有窗口），长按 1s 成立时
 *  「放行」（走常规打开路径让它显形），中途松手则「作废」（销毁自己建的窗口）。
 *  会话号随 URL 带给子窗口、再随就绪回执带回来，用于把回执对上号
 *  （memory 是共享窗口，两个角色桌宠可能各有一场预热，回执只该被发起者认领）。 */
interface MemoryPrewarmSession {
  session: number;
  /** 预热链（建窗口 / 认领已有窗口）；放行前先等它落地，避免与创建抢跑 */
  chain: Promise<void>;
  /** 本次会话新建出来的窗口句柄，创建完成后回填。
   *  为 null 有两种含义：还没回填，或这次是「认领已有窗口」（本来就没有要销毁的东西）。 */
  win: WebviewWindow | null;
  /** 子窗口已挂好 `pet:reveal` 监听（或内容早已加载完）——此时发显形事件才不会丢 */
  childReady: boolean;
  /** 长按已成立，在等子窗口就绪 */
  committed: boolean;
  /** 会话已作废（长按中止） */
  aborted: boolean;
  /** 等就绪回执的兜底定时器（回执丢了也得能打开） */
  readyTimer: number | null;
}

/** 作废一场预热会话。
 *
 *  `session.win` 还没回填（窗口正在创建）时不在这里销毁——预热链 await 到句柄后
 *  会看到 `aborted` 并就地销毁，那里才不会和创建抢跑。 */
function abortPrewarmSession(session: MemoryPrewarmSession): void {
  session.aborted = true;
  if (session.readyTimer !== null) {
    window.clearTimeout(session.readyTimer);
    session.readyTimer = null;
  }
  if (session.win) void destroyPrewarmedWindow(session.win);
}

/** 放掉一场预热会话：腾出会话位、清等待定时器、执行长按动作（真正打开/收起窗口）。
 *
 *  抽成模块函数是为了让「长按成立」与「子窗口回执」两条路径共用同一套收尾——
 *  它们分处不同的回调与 effect，写成组件内闭包会互相看不见对方。
 *  会话位已经不是它了（被新长按顶掉 / 已作废）就什么都不做。 */
function finishPrewarm(
  slot: { current: MemoryPrewarmSession | null },
  session: MemoryPrewarmSession,
  action: () => void,
): void {
  if (slot.current !== session) return;
  slot.current = null;
  if (session.readyTimer !== null) {
    window.clearTimeout(session.readyTimer);
    session.readyTimer = null;
  }
  action();
}

/** 创建或聚焦独立窗口。
 *  复用判定：`isVisible()` 抛异常才说明引用已失效（窗口已销毁），此时丢弃引用重新创建；
 *  它返回 false 只说明窗口被 hide 过，仍然复用——raiseWindow 负责把它 show 回来。
 *  窗口已存在却去重建是走不通的（label 冲突），所以「存在即复用」是唯一安全的规则。
 *
 *  返回值：最终落到这个 label 上的窗口（新建或复用），创建失败时为 null。
 *  绝大多数调用方只关心副作用，忽略即可；预热需要它来在会话作废时销毁自己建的那个。 */
async function openWindow(
  label: string,
  view: string,
  title: string,
  width: number,
  height: number,
  options: {
    resizable?: boolean;
    transparent?: boolean;
    decorations?: boolean;
    shadow?: boolean;
    /** 是否始终置顶（仅状态面板等需置顶的窗口传 true，其余默认 false） */
    alwaysOnTop?: boolean;
    /** 原生窗口效果（Mica/Acrylic 等），用于实现 OS 级毛玻璃模糊 */
    windowEffects?: { effects: Effect[]; color?: string };
    minWidth?: number;
    minHeight?: number;
    /** 是否全屏覆盖（无边框 + 透明 + 占满屏幕） */
    fullscreen?: boolean;
      /** 追加到子窗口 URL 的额外 query（如 guide=1） */
      extraQuery?: string;
    /**
     * 由子窗口自己决定显形时机——两条路径都不代它显形：创建时不自动 show，
     * 复用时也**不**还原/显示（只做 Z 序与焦点）。
     * 用于「先摆好首帧再出现」的入场动画：心智观察器从桌宠矩形长到全屏时，
     * 若沿用「created 即 show」会先闪一帧全屏大图；已被最小化时更明显——
     * 先还原一次再播一遍入场，就是呼出了两回。超时未显形时有兜底显示。
     */
    selfReveal?: boolean;
    /**
     * 预热模式：窗口建出来加载，但**连兜底显形都不挂**。
     *
     *  与 selfReveal 的区别就在那个兜底：兜底的前提是「这个窗口本来就该出现，
     *  只是显形链慢/断了」；而预热窗口该不该出现取决于长按成不成立，此刻还没有
     *  答案——挂上兜底就成了「长按没成立，窗口自己冒出来」。所以预热期间一律
     *  不上屏，显形改由调用方在长按成立后驱动（那时走的是复用路径，兜底照常挂）。
     */
    prewarm?: boolean;
    /**
     * 窗口已经打开且可见时的回调——取代「只聚焦就返回」的默认行为。
     *
     * 不传则维持原样（提升 Z 序 + 聚焦）。需要重播入场动画的窗口传它：
     * 复用已有窗口既不能 navigate（会整页 reload、丢掉当前页签与输入），
     * 也不能重建（label 冲突），所以只能由子窗口收到通知后自己再播一遍。
     */
    onExisting?: (win: WebviewWindow) => void;
  } = {},
  t?: (key: string) => string,
): Promise<WebviewWindow | null> {
  // 按角色区分 label，避免多角色窗口的子窗口冲突
  const fullLabel = charScopedLabel(label);

  // 有同 label 的窗口正在创建 → 先等它落地（否则下面的注册表查询查不到它，
  // 会走到重名重建那条死路，详见 WINDOW_CREATION 的说明）
  const inFlight = WINDOW_CREATION.get(fullLabel);
  if (inFlight) await inFlight;

  // 预热窗口的显形完全由调用方驱动，兜底显形一律不挂
  const selfReveal = options.selfReveal ?? false;
  const armRevealFallback = selfReveal && !options.prewarm;

  // 复用分支的判定与动作必须分开：
  // - 活性探测（`isVisible()`）只认「抛异常」＝窗口真的被销毁了。返回 false 不代表窗口没了，
  //   那只是它被 hide 过——恰恰是该被 show 回来的对象，按「已销毁」处理会害得下面重名重建，
  //   而重名重建必然失败。
  // - 提到前台的动作单独兜：它失败只意味着这次没提到前台，绝不能顺势被当成「窗口已销毁」。
  //   误判的后果是走到下面用同一个 label 再建一次，Tauri 报 label 冲突（只有 console 里
  //   一行 tauri://error），对外表现就是「点了毫无反应」。
  //
  // 这条路径上曾经还顺手补一次 `setResizable(false)`。窗口的 resizable 在创建时就定死了
  // （openMemory 传的就是 false，全仓也没有第二处改它），这次重设是冗余的；
  // 而它依赖的 `core:window:allow-set-resizable` 权限从未授予，调用必然 reject——
  // 于是每次「已打开后再长按」都在这里抛错、被当成窗口已死，整个复用路径全废。
  // 窗口创建时本身就带这个约束，重设删掉即可；真需要一个可复用的重设入口，
  // 该做的是补权限，而不是让它继续吞掉异常。

  // 1. 检查追踪缓存：窗口仍在存活 → 直接聚焦
  const tracked = CHILD_WINDOWS.get(fullLabel);
  if (tracked) {
    let alive = false;
    try {
      await tracked.isVisible();
      alive = true;
    } catch {
      // 窗口已销毁，isVisible 抛异常 → 清理缓存
      alive = false;
    }
    if (alive) {
      await raiseWindow(tracked, label, selfReveal).catch(() => {});
      options.onExisting?.(tracked);
      if (armRevealFallback) armSelfRevealFallback(tracked);
      return tracked;
    }
    CHILD_WINDOWS.delete(fullLabel);
  }

  // 2. 检查 Tauri 运行时注册表（捕获 getByLabel 返回的陈旧引用）
  try {
    const existing = await WebviewWindow.getByLabel(fullLabel);
    if (existing) {
      let alive = false;
      try {
        await existing.isVisible();
        alive = true;
      } catch {
        // 陈旧引用：窗口已关闭但标签未清理 → 继续创建新窗口
        alive = false;
      }
      if (alive) {
        // 窗口确实还活着 → 纳入追踪并聚焦
        CHILD_WINDOWS.set(fullLabel, existing);
        if (!CLOSE_CLEANUP_REGISTERED.has(fullLabel)) {
          CLOSE_CLEANUP_REGISTERED.add(fullLabel);
          void existing.onCloseRequested(() => {
            CHILD_WINDOWS.delete(fullLabel);
            CLOSE_CLEANUP_REGISTERED.delete(fullLabel);
            const u = RAISE_UNLISTEN.get(label);
            if (u) { u(); RAISE_UNLISTEN.delete(label); }
          });
        }
        await raiseWindow(existing, label, selfReveal).catch(() => {});
        options.onExisting?.(existing);
        if (armRevealFallback) armSelfRevealFallback(existing);
        return existing;
      }
    }
  } catch {
    // getByLabel 异常 → 继续创建新窗口
  }

  // 3. 创建新窗口
  const resizable = options.resizable ?? true;
  const transparent = options.transparent ?? false;
  const isFullscreen = options.fullscreen ?? false;
  try {
    const win = new WebviewWindow(fullLabel, {
      // 共享子窗口不绑定 character_id，由窗口内部三视图切换决定数据源
      url: SHARED_SUBWINDOWS.has(label)
        ? `/?view=${view}${options.extraQuery ? `&${options.extraQuery}` : ''}`
        : `/?view=${view}&character_id=${getCharacterId() ?? ''}${options.extraQuery ? `&${options.extraQuery}` : ''}`,
      title,
      width: isFullscreen ? screen.width : width,
      height: isFullscreen ? screen.height : height,
      resizable: isFullscreen ? false : resizable,
      decorations: isFullscreen ? false : (options.decorations ?? true),
      transparent: isFullscreen ? true : transparent,
      alwaysOnTop: options.alwaysOnTop ?? false,
      center: true,
      shadow: isFullscreen ? false : (options.shadow ?? true),
      minWidth: isFullscreen ? undefined : (options.minWidth ?? (resizable ? 320 : width)),
      minHeight: isFullscreen ? undefined : (options.minHeight ?? (resizable ? 300 : height)),
      maxWidth: isFullscreen ? undefined : (resizable ? undefined : width),
      maxHeight: isFullscreen ? undefined : (resizable ? undefined : height),
      windowEffects: options.windowEffects,
      visible: false,
      // 启用 Tauri 原生拖放，通过 onDragDropEvent 获取文件路径
      // （HTML5 File.path 在 Tauri v2 已移除，需用原生事件获取路径）
      dragDropEnabled: true,
    });

    // 4. 追踪引用 + 注册关闭清理
    CHILD_WINDOWS.set(fullLabel, win);
    CLOSE_CLEANUP_REGISTERED.add(fullLabel);
    void win.onCloseRequested(() => {
      CHILD_WINDOWS.delete(fullLabel);
      CLOSE_CLEANUP_REGISTERED.delete(fullLabel);
      const u = RAISE_UNLISTEN.get(label);
      if (u) { u(); RAISE_UNLISTEN.delete(label); }
    });
    // 5. 窗口创建后显示（visible:false 创建，webview 就绪后 show）
    if (selfReveal) {
      // 显形交给子窗口（入场动画需要先把首帧摆成桌宠大小再 show）
      if (armRevealFallback) armSelfRevealFallback(win);
    } else {
      win.once('tauri://created', () => {
        void win.show().catch(() => {});
      });
    }
    win.once('tauri://error', (e) => {
      console.error(`[openWindow] 窗口 "${fullLabel}" 创建失败:`, e);
    });

    // 6. 登记「创建中」：任一落地信号（含超时）都会解开等待者
    let settleCreation: () => void = () => {};
    const creation = new Promise<void>((resolve) => { settleCreation = resolve; });
    WINDOW_CREATION.set(fullLabel, creation);
    let creationTimer: number | undefined;
    const settleOnce = () => {
      if (creationTimer !== undefined) {
        window.clearTimeout(creationTimer);
        creationTimer = undefined;
      }
      if (WINDOW_CREATION.get(fullLabel) === creation) WINDOW_CREATION.delete(fullLabel);
      settleCreation();
    };
    win.once('tauri://created', settleOnce);
    win.once('tauri://error', settleOnce);
    creationTimer = window.setTimeout(settleOnce, WINDOW_CREATION_TIMEOUT_MS);

    return win;
  } catch (err) {
    console.error(`[openWindow] 创建窗口 "${fullLabel}" 失败:`, err);
    return null;
  }
}

/* ============ 主应用 ============ */
export default function App() {
  // 拆分 selector 订阅，避免全量订阅导致 App 频繁重渲染：
  // BubbleController 打字机 35ms 一次 setCurrentBubble，全量订阅会触发 ~28次/秒重渲染。
  // 渲染相关字段使用独立 selector；actions 与回调内读取的字段改用 useAppStore.getState() 即时获取。
  const currentBubble = useAppStore((s) => s.currentBubble);
  const settledBubbles = useAppStore((s) => s.settledBubbles);
  const ttsEnabled = useAppStore((s) => s.ttsEnabled);
  const voiceEnabled = useAppStore((s) => s.voiceEnabled);
  // 桌宠自身心情状态（energy=精力 0-100，focus=专注力 0-100，由后端 3s 心跳刷新）
  const currentMood = useAppStore((s) => s.currentMood);
  const { t } = useTranslation();
  const moodApi = useMood();
  const configApi = useConfig();
  const ttsApi = useTTS();
  const proactiveApi = useProactive();
  const environmentApi = useEnvironment();

  // ====== 诊断日志：第一层 - App mount ======
  const diagCharId = getCharacterId();
  useEffect(() => {
    console.log(`[DIAG] App mounted, char_id=${diagCharId}, label=${getCurrentWindow().label}, time=${Date.now()}`);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 主题：读取 base.theme 配置设置根节点 data-theme，并监听实时变更；
  // 同时向后端上报生效主题（"跟随系统"时按系统深浅偏好解析为 light/dark），
  // 供日出/日落提醒在建议切换主题前核对当前实际主题
  useEffect(() => {
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const reportEffective = (setting: string | null | undefined) => {
      const effective =
        setting === 'light' || setting === 'dark'
          ? setting
          : mq.matches
            ? 'dark'
            : 'light';
      void invoke('report_effective_theme', { theme: effective }).catch(() => {});
    };
    const applyTheme = (theme: string | null | undefined) => {
      document.documentElement.setAttribute('data-theme', theme === 'light' || theme === 'dark' ? theme : 'system');
      reportEffective(theme);
    };
    // 系统深浅偏好变化时（仅 data-theme 为 system 会影响生效值）重新上报
    const onPrefChange = () => {
      const cur = document.documentElement.getAttribute('data-theme') ?? 'system';
      reportEffective(cur);
    };
    mq.addEventListener('change', onPrefChange);
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const theme = await invoke<string | null>('get_config', { key: 'base.theme' });
        if (!cancelled) applyTheme(theme);
        unlisten = await listen<{ theme: string }>('config:theme-changed', (e) => {
          applyTheme(e.payload?.theme);
        });
        if (cancelled) unlisten();
      } catch { /* ignore */ }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
      mq.removeEventListener('change', onPrefChange);
    };
  }, []);

  const petRef = useRef<ModelRendererHandle | null>(null);
  const [modelReady, setModelReady] = useState(false);

  // 当前角色的在场状态（online/busy/rest/offline），驱动 桌宠行为（表情/闭眼/鼠标跟随/隐藏到角落）+ tick 降频
  const [presenceState, setPresenceState] = useState<string | null>(null);
  // presenceState 的 ref 镜像，供定时器回调读取最新值
  const presenceStateRef = useRef<string | null>(null);
  useEffect(() => { presenceStateRef.current = presenceState; }, [presenceState]);
  // 唤醒点击计数器：rest 状态需 3 次连续点击、busy 状态 1 次即唤醒
  const wakeClickRef = useRef<{ count: number; lastTime: number }>({ count: 0, lastTime: 0 });
  /** 基础窗口尺寸（模型加载后按比例计算，缩放基于此） */
  const baseWindowSizeRef = useRef<{ w: number; h: number }>({ w: 0, h: 0 });
  /** 当前用户缩放因子 */
  const windowScaleRef = useRef(1.0);
  /** 缩放目标值（滚轮事件同步写入，异步循环读取） */
  const targetScaleRef = useRef(1.0);
  /** 缩放循环是否运行中（存储 requestAnimationFrame ID） */
  const scaleRafRef = useRef<number | null>(null);
  /** 缓存的窗口中心点（物理像素），滚动会话期间不重新读取 */
  const scaleCenterRef = useRef<{ cx: number; cy: number; factor: number } | null>(null);
  /** 滚动停止后清除中心缓存 */
  const scaleIdleTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** 缩放 debounce 计时器：累积滚动事件，100ms 内无新事件才执行 resize，避免中间帧闪烁 */
  const scaleDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** 视频动画演出层是否在播放（播放期间隐藏 桌宠本体） */
  const [videoActive, setVideoActive] = useState(false);
  /** Ollama 就绪 toast 是否已弹出（每个应用生命周期只弹一次） */
  const ollamaToastedRef = useRef(false);
  /**
   * 最近一次 LLM 错误 toast 的时间戳。
   *
   * 启动问候失败与 LLM 调用失败常常是同一根因（问候本身要走主 LLM），
   * 两条路径各弹一次会让用户看到两条说同一件事的 toast，用它做短窗抑制。
   */
  const lastLlmErrorToastAtRef = useRef(0);
  // 统一隐藏管理：全屏应用聚焦隐藏到角落（受智能避让开关控制）/ Rest 退到角落 / Offline 真正 hide_window
  // 暴露 hiddenCorner（驱动角落感知按钮）、requestRestore（按钮点击召回 + 唤醒）、
  // hideForSleep / restoreFromSleep（Rest 时退到角落）
  // hideForOffline / restoreFromOffline（Offline 时真正 hide_window，从托盘/快捷键唤回）
  // 智能避让：检测纯色区域，移动桌宠避免遮挡内容 + 全屏应用时退到角落（受 window.smart_positioning_enabled 控制）
  const [smartPositioningEnabled, setSmartPositioningEnabled] = useState(true);
  // 心情注意力联动总开关（pet_render.always_follow_mouse）
  const [alwaysFollowMouse, setAlwaysFollowMouse] = useState(true);
  // 心情驱动的注视激活态：精力与专注力同时高于阈值时激活（实时看鼠标），回落则关闭
  // 使用滞回（HIGH=60 / LOW=35）避免 3s 心情心跳带来的频繁抖动切换
  const [moodFollowActive, setMoodFollowActive] = useState(false);
  const moodFollowActiveRef = useRef(false);
  const MOOD_FOLLOW_ENERGY_HIGH = 60;
  const MOOD_FOLLOW_ENERGY_LOW = 35;
  const MOOD_FOLLOW_FOCUS_HIGH = 60;
  const MOOD_FOLLOW_FOCUS_LOW = 35;
  const {
    hiddenCorner,
    hideReason,
    requestRestore,
    hideForSleep,
    restoreFromSleep,
    hideForOffline,
    restoreFromOffline,
  } = useHiding(petRef, modelReady, smartPositioningEnabled);
  useSmartPositioning(petRef, modelReady, smartPositioningEnabled);

  // 活动追踪 refs
  const lastActivityRef = useRef<number>(Date.now());
  const lastUserMessageRef = useRef<number>(Date.now());
  const activeWindowRef = useRef<string>('');
  const lastActiveWindowRef = useRef<string>('');
  const dragDistanceRef = useRef<number>(0);
  const lastBubbleFromProactiveRef = useRef<number>(0);
  // 主动对话流式文本累积
  const proactiveStreamTextRef = useRef<string>('');
  // 其他角色最近发言时间戳（毫秒），用于延迟本角色 tick 避免同时发言
  const otherSpokenAtRef = useRef<number>(0);
  // 跨角色冷却时长（毫秒），由后端 effective_cross_cooldown_ms 动态下发
  const crossCooldownMsRef = useRef<number>(15_000);
  const ttsConfigRef = useRef<TtsConfig | null>(null);
  const [proactiveStarted, setProactiveStarted] = useState<boolean>(false);
  // 主动对话 tick 间隔（毫秒），由 proactive.tick_interval 配置项驱动
  const proactiveTickIntervalRef = useRef<number>(10_000);
  // 拖拽表情联动：标记当前是否处于用户拖拽会话，以及拖拽表情是否已应用
  const dragSessionRef = useRef<boolean>(false);
  const dragExpressionAppliedRef = useRef<boolean>(false);
  // 逃离途中延后开启拖动的定时器句柄（见 handleBackgroundMouseDown）
  const dragDeferTimerRef = useRef<number | null>(null);
  // 左键此刻是否按着（前端视角）：延后开拖的定时器据此判断「人还按着吗」
  const dragPressAliveRef = useRef<boolean>(false);
  // 晕乎乎临时表情的回落定时器句柄（快速拖动 / 甩飞撞边共用）
  const dizzyTimerRef = useRef<number | null>(null);
  // 长按打开心智观察器：会话是否进行中 / 延迟计时器句柄 / 起始窗口位置（位移判拖拽用）
  const holdActiveRef = useRef(false);
  const holdTimerRef = useRef<number | undefined>(undefined);
  const holdStartWinPosRef = useRef<{ x: number; y: number } | null>(null);
  // 长按触发时刻：用于吞掉触发后紧跟的 click 余波（不触发摸头台词）
  const holdCompletedAtRef = useRef(0);
  // 长按进度环挂载位置（client 坐标）；null = 未显示
  const [holdRingPos, setHoldRingPos] = useState<{ x: number; y: number } | null>(null);
  // 长按触发的动作（打开心智观察器），由下方 openMemory 定义后回填
  const holdActionRef = useRef<() => void>(() => {});
  // 长按提前加载心智观察器（预热）：当前会话 / 起跑计时器 / 会话号序列。
  // 预热动作也走 ref 回填（同 holdActionRef）——startHold 定义在 prewarmMemory 之前，
  // 直接用会撞上暂时性死区，而这个回调又几乎不换身份，没必要进依赖表。
  const prewarmRef = useRef<MemoryPrewarmSession | null>(null);
  const prewarmTimerRef = useRef<number | undefined>(undefined);
  const prewarmActionRef = useRef<() => void>(() => {});
  const prewarmSeqRef = useRef(0);

  // 气泡子窗口管理：currentBubble 变化时创建/更新/隐藏气泡窗口
  // currentBubble 是单一数据源（涵盖普通气泡、流式气泡、追加气泡）
  const prevBubbleTextRef = useRef<string | null>(null);
  // 气泡窗口 webview 是否已就绪（监听器已注册）
  const bubbleReadyRef = useRef<boolean>(false);
  // 待发送的首次气泡文本（窗口未就绪时缓存）
  const pendingBubbleTextRef = useRef<string | null>(null);
  // 最近一次气泡定位锚点（流式动态扩大时据此重新计算 y 坐标）
  const lastBubbleAnchorRef = useRef<{
    position: BubblePosition;
    petWinY: number;
    petWinH: number;
    x: number;
  } | null>(null);
  // 上一次已结算气泡段列表的 ID 快照（用于检测增删并转发到气泡窗口）
  const prevSettledIdsRef = useRef<Set<number>>(new Set());

  // Toast 子窗口管理：屏幕右下角的透明、点击穿透窗口
  const toastReadyRef = useRef<boolean>(false);
  // 窗口未就绪时缓存的 toast 请求
  const pendingToastRef = useRef<Array<{ message: string; type: ToastType; duration: number; key?: number; action?: ToastAction; owner?: string }>>([]);
  // 窗口未就绪时缓存的工具确认请求（载荷原样转发给 toast 子窗口）
  const pendingConfirmRef = useRef<ToolConfirmPayload[]>([]);

  /** 创建 Toast 子窗口（首次惰性创建），定位到屏幕右下角并设置点击穿透 */
  const ensureToastWindow = useCallback(async (): Promise<void> => {
    const toastLabel = charScopedLabel('toast');
    const existing = await WebviewWindow.getByLabel(toastLabel);
    if (existing) return;
    // 先取屏幕尺寸，用于窗口高度（固定为屏幕的一半）+ 定位到右下角。
    // 半屏高是 ToastWindow 容量管理的前提：可用空间是确定的，新 toast 放不下时就先让
    // 最老的平滑退出再入场；若高度跟着内容伸缩，增删条目与重新测高之间必有一帧错位，
    // 那一帧里顶部的 toast 就会被窗口边界裁掉。
    let screenW = 0;
    let screenH = 0;
    try {
      const monitor = await currentMonitor();
      if (monitor) {
        const factor = monitor.scaleFactor;
        screenW = monitor.size.width / factor;
        screenH = monitor.size.height / factor;
      }
    } catch {
      /* ignore */
    }
    const toastHeight = screenH > 0 ? Math.round(screenH / 2) : TOAST_WINDOW_HEIGHT;
    // 直接使用构造函数返回的实例（getByLabel 在窗口未完全创建时可能返回 null）
    const win = new WebviewWindow(toastLabel, {
      url: `/?view=toast&character_id=${getCharacterId() ?? ''}`,
      title: 'Vivian Toast',
      width: TOAST_WINDOW_WIDTH,
      height: toastHeight,
      resizable: false,
      decorations: false,
      transparent: true,
      alwaysOnTop: true,
      skipTaskbar: true,
      shadow: false,
      focus: false,
      visible: false,
    });
    // 窗口创建完成后再定位：setPosition 在窗口未 ready 时会静默失败，
    // 导致 toast 窗口停在 Tauri 默认位置（屏幕左上角），用户会看到一个错误的左侧 toast
    win.once('tauri://created', async () => {
      try {
        if (screenW > 0 && screenH > 0) {
          await win.setPosition(
            new LogicalPosition(screenW - TOAST_WINDOW_WIDTH, screenH - toastHeight),
          );
        }
        await win.setIgnoreCursorEvents(true);
      } catch {
        /* ignore */
      }
    });
    win.once('tauri://error', (e) => {
      console.error('[ensureToastWindow] toast 窗口创建失败:', e);
    });
  }, []);

  /** 创建微信消息横幅窗口（首次惰性创建），常驻隐藏，由 wechat:message_banner 事件触发显示 */
  const ensureMessageBannerWindow = useCallback(async (): Promise<void> => {
    const label = 'message_banner';
    const existing = await WebviewWindow.getByLabel(label);
    if (existing) return;
    new WebviewWindow(label, {
      url: `/?view=message_banner`,
      title: 'Vivian Message Banner',
      width: 400,
      height: 160,
      resizable: false,
      decorations: false,
      transparent: true,
      alwaysOnTop: true,
      skipTaskbar: true,
      shadow: false,
      focus: false,
      visible: false,
    });
  }, []);

  /** 显示一条 toast（窗口就绪时直接 emit，未就绪时缓存并触发窗口创建）。
   *  传入固定 `key` 可原地更新同一条 toast；`duration <= 0` 表示持久显示（不自动关闭）。
   *
   *  **`key` 的语义是「这条有身份，后续会用同一个值再来更新它」**，只给需要原地刷新的
   *  条目（如重建进度的固定 key）。一次性提示不要传 key——曾经的默认值 `Date.now()`
   *  看似无害，实际做了两件坏事：同一毫秒发出的两条 toast 会撞上同一个 key 而互相顶掉；
   *  而接收侧的跨窗口去重又会把「带 key 的条目」整体豁免，导致同一条文案在两只桌宠上
   *  各弹一条。需要 key 时请传一个跨次调用稳定的常量。
   *
   *  `owner` 决定这条 toast 弹到哪个角色的窗口：
   *  - `undefined`（默认）：归属当前窗口角色。用于本窗口自己发起的事件
   *    （文件拖放、语音未开启等），以及 payload 已带 character_id 的定向事件。
   *  - `null`：显式声明「无归属」。用于全局事件——它们被广播给每个角色窗口，
   *    若各自都按自身角色 emit，同一条文案就会在每个 toast 窗口各弹一次。
   *    无归属的 toast 统一由主角色的 toast 窗口呈现（见 ToastWindow 的归属判定）。 */
  const showToast = useCallback(
    (message: string, type: ToastType = 'info', duration: number = 3000, key?: number, action?: ToastAction, owner?: string | null) => {
      // 归属解析：显式指定优先；`null` 与「当前窗口无角色」都归一为 undefined（无归属），
      // 由接收侧按主角色窗口收敛。
      const ownerId = owner === undefined ? (getCharacterId() ?? undefined) : (owner ?? undefined);
      // 点击穿透功能已移除：桌宠窗口始终整窗响应鼠标，
      // toast 显示期间不再需要 suspend/resume_click_through 配对。

      if (toastReadyRef.current) {
        void emit('toast:show', { message, type, duration, key, character_id: ownerId, action });
      } else {
        pendingToastRef.current.push({ message, type, duration, key, action, owner: ownerId });
        void ensureToastWindow();
      }
    },
    [ensureToastWindow],
  );

  // 注册 toast:ready 监听并创建 toast 子窗口（必须先 await listen 再创建窗口，避免竞态：
  // 若窗口先创建，ToastWindow emit('toast:ready') 时本窗口的监听器可能尚未注册，事件丢失）
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      unlisten = await listen<{ character_id?: string }>('toast:ready', (e) => {
        if (e.payload?.character_id && e.payload.character_id !== getCharacterId()) return;
        toastReadyRef.current = true;
        const pending = pendingToastRef.current;
        pendingToastRef.current = [];
        for (const p of pending) {
          void emit('toast:show', { message: p.message, type: p.type, duration: p.duration, key: p.key, action: p.action, character_id: p.owner });
        }
        const pendingConfirms = pendingConfirmRef.current;
        pendingConfirmRef.current = [];
        for (const c of pendingConfirms) {
          void emit('toast:confirm', c);
        }
      });
      if (cancelled) { safeUnlisten(unlisten); return; }
      console.log(`[DIAG] listen registered: toast:ready, char=${getCharacterId()}`);
      // 监听器注册完成后再创建窗口
      void ensureToastWindow().then(() => {
        // 超时保险：若 toast:ready 事件因异常原因丢失，1 秒后强制补发 pending
        setTimeout(() => {
          if (!toastReadyRef.current) {
            toastReadyRef.current = true;
            const pending = pendingToastRef.current;
            pendingToastRef.current = [];
            for (const p of pending) {
              void emit('toast:show', { message: p.message, type: p.type, duration: p.duration, key: p.key, action: p.action, character_id: p.owner });
            }
            const pendingConfirms = pendingConfirmRef.current;
            pendingConfirmRef.current = [];
            for (const c of pendingConfirms) {
              void emit('toast:confirm', c);
            }
          }
        }, 1000);
      });
    })();
    return () => { cancelled = true; safeUnlisten(unlisten); };
  }, [ensureToastWindow]);

  // 监听后台记忆向量重建进度：切换嵌入模型后设置窗口立即关闭，
  // 重建由后端 spawn 任务执行，进度经事件推送到这里，用常驻 toast 实时展示。
  useEffect(() => {
    let cancelled = false;
    const unlistens: Array<() => void> = [];
    void (async () => {
      const unProgress = await listen<{ current: number; total: number }>('memory:rebuild_progress', (e) => {
        const { current, total } = e.payload ?? { current: 0, total: 0 };
        // 向量重建是全局操作（不隶属任何角色），而事件会广播给每个角色窗口。
        // 显式声明无归属（owner=null）交由主角色窗口呈现；否则每个角色窗口都按
        // 自身身份 emit 一条，同一条进度会在每只桌宠的 toast 窗口各弹一次。
        showToast(t('config.rebuilding_embeddings_progress', { current, total }), 'info', 0, REBUILD_TOAST_KEY, undefined, null);
      });
      const unDone = await listen<{ rebuilt: number; total: number }>('memory:rebuild_done', (e) => {
        const rebuilt = e.payload?.rebuilt ?? 0;
        showToast(t('config.toast_rebuild_ok', { count: rebuilt }), 'success', 4000, REBUILD_TOAST_KEY, undefined, null);
      });
      if (cancelled) { safeUnlisten(unProgress); safeUnlisten(unDone); return; }
      unlistens.push(unProgress, unDone);
    })();
    return () => { cancelled = true; for (const un of unlistens) un(); };
  }, [showToast, t]);

  /** 计算气泡窗口位置（贴合主窗口上方或下方），返回逻辑坐标 + 朝向
   *
   *  `height` 用于动态扩大场景：流式输出时根据文本长度传入更大的高度，
   *  以此重新计算 y 坐标（上方模式 y 上移，下方模式 y 不变）。
   */
  const computeBubbleWindowPosition = useCallback(async (height: number = BUBBLE_WINDOW_HEIGHT): Promise<{
    x: number;
    y: number;
    position: BubblePosition;
  } | null> => {
    try {
      const win = getCurrentWindow();
      const [pos, size, factor] = await Promise.all([
        win.outerPosition(),
        win.outerSize(),
        win.scaleFactor(),
      ]);
      const monitor = await currentMonitor();
      if (!monitor) return null;

      const screenW = monitor.size.width / factor;
      const screenH = monitor.size.height / factor;
      const winX = pos.x / factor;
      const winY = pos.y / factor;
      const winW = size.width / factor;
      const winH = size.height / factor;

      // 纵向：优先放主窗口上方，空间不足则放下方
      const spaceAbove = winY;
      const above = spaceAbove >= height;
      // position='top' → 尾巴朝下（气泡在桌宠上方）；position='bottom' → 尾巴朝上（气泡在下方）
      const position: BubblePosition = above ? 'top' : 'bottom';
      const y = above ? winY - height : winY + winH;

      // 横向：气泡窗口与主窗口右对齐，但保证不超出屏幕
      let x = winX + winW - BUBBLE_WINDOW_WIDTH;
      if (x < 4) x = 4;
      if (x + BUBBLE_WINDOW_WIDTH > screenW - 4) {
        x = screenW - BUBBLE_WINDOW_WIDTH - 4;
      }
      // 缓存锚点，供流式动态扩大时同步重算 y
      lastBubbleAnchorRef.current = { position, petWinY: winY, petWinH: winH, x };
      return { x: Math.round(x), y: Math.round(y), position };
    } catch {
      return null;
    }
  }, []);

  /** 向气泡窗口发送 bubble:show 事件 */
  const emitBubbleShow = useCallback(async (text: string) => {
    // 首次显示即按文本长度估算高度，流式首块即可获得合适尺寸
    const dynHeight = estimateBubbleHeight(text);
    const posInfo = await computeBubbleWindowPosition(dynHeight);
    if (!posInfo) return;
    // 气泡窗口自身已设置 setIgnoreCursorEvents(true) 一直穿透，不影响 桌宠窗口的鼠标交互
    let bubbleWin = await WebviewWindow.getByLabel(charScopedLabel('bubble'));
    if (bubbleWin) {
      try {
        await bubbleWin.setSize(new LogicalSize(BUBBLE_WINDOW_WIDTH, dynHeight));
        await bubbleWin.setPosition(new LogicalPosition(posInfo.x, posInfo.y));
        await bubbleWin.show();
      } catch {
        /* ignore */
      }
    }
    void emit('bubble:show', {
      text,
      position: posInfo.position,
      duration: 0,
      character_id: getCharacterId() ?? undefined,
      cross_character: useAppStore.getState().bubbleCrossCharacter,
      listener_name: useAppStore.getState().bubbleListenerName ?? undefined,
    });
  }, [computeBubbleWindowPosition]);

  const ensureSideChatWindow = useCallback(async (opts?: { show?: boolean; lock?: boolean; showInput?: boolean; autoVoice?: boolean }): Promise<void> => {
    const label = 'side_chat';
    const shouldShow = opts?.show !== false;

    const win = getCurrentWindow();
    const [monitor, factor] = await Promise.all([
      currentMonitor(),
      win.scaleFactor(),
    ]);
    const screenH = (monitor?.size.height ?? 1080) / factor;
    const screenW = (monitor?.size.width ?? 1920) / factor;
    const windowHeight = Math.max(340, Math.round((screenH * 2) / 5));
    // 直接对话侧边栏停靠屏幕左缘：静止位为显示器左缘（位置由前端设定，显隐由 Rust 控制）
    const x = Math.round((monitor?.position.x ?? 0) / factor);
    const y = Math.round((screenH - windowHeight) / 2);

    const existing = await WebviewWindow.getByLabel(label);
    if (existing) {
      try {
        await existing.setSize(new LogicalSize(SIDE_CHAT_WIDTH, windowHeight));
        await existing.setPosition(new LogicalPosition(x, y));
      } catch {
        /* ignore */
      }
      if (shouldShow) {
        await invoke('show_side_chat_animated', { label: 'side_chat' }).catch(() => {});
      }
      if (opts?.lock) {
        await invoke('set_side_chat_locked', { locked: true }).catch(() => {});
      }
      // 窗口已存在：通过事件通知显示 InputDialog（携带角色 ID 用于发送路由）
      if (opts?.showInput) {
        void emit('sidechat:show_input', {
          character_id: getCharacterId(),
          auto_start_voice: opts?.autoVoice ?? false,
        });
      }
      return;
    }

    // 新建窗口：URL 参数传递 show_input，避免页面加载延迟导致事件丢失
    const params = new URLSearchParams();
    params.set('view', 'side_chat');
    if (getCharacterId()) params.set('active_character', getCharacterId()!);
    if (opts?.showInput) params.set('show_input', '1');
    if (opts?.autoVoice) params.set('auto_voice', '1');

    const sideWin = new WebviewWindow(label, {
      url: `/?${params.toString()}`,
      title: 'Side Chat',
      width: SIDE_CHAT_WIDTH,
      height: windowHeight,
      x,
      y,
      resizable: false,
      decorations: false,
      transparent: true,
      alwaysOnTop: true,
      skipTaskbar: true,
      shadow: false,
      focus: false,
      visible: false,
    });

    sideWin.once('tauri://created', () => {
      if (shouldShow) {
        void invoke('show_side_chat_animated', { label: 'side_chat' }).catch(() => {});
      }
      if (opts?.lock) {
        void invoke('set_side_chat_locked', { locked: true }).catch(() => {});
      }
    });
  }, []);

  /**
   * 请求一次桌宠轻量反应。
   *
   * 用户对桌宠做出动作时调用：后端用一次极简 prompt（精简人设 + 低权重历史对话窗口
   * + 本次动作描述）走 `intent_judge` 那档 flash 模型生成一句短反应，同时把这次操作
   * 作为有意义的用户行为记进统一事件账本。
   *
   * 完全静默：未配置模型 / 超时 / 失败时后端返回 null，这里不显示任何气泡，
   * 也不再回退到写死的固定台词。同类动作的节流在后端做，前端不重复计时。
   */
  const requestPetReaction = useCallback((action: PetAction, impact?: number) => {
    void invoke<string | null>('generate_pet_reaction', {
      action,
      impact: impact ?? null,
      characterId: getCharacterId() ?? undefined,
    })
      .then((text) => {
        const reply = text?.trim();
        if (!reply) return;
        // 生成期间用户可能已经开了新一轮对话：别用桌宠反应盖掉流式气泡
        if (ChatController.isStreaming) return;
        BubbleController.showBubble(reply, Math.max(2600, reply.length * 220));
      })
      .catch(() => {
        /* 静默：桌宠反应失败不该打扰用户 */
      });
  }, []);

  /**
   * 单击/双击桌宠：本地短音效 + 一次轻量 AI 反应（取代原先写死的固定台词）。
   *
   * `rough_click` 是画布判定「戳烦了」后的结果（短时间内戳得太多或太急），
   * 表情由画布自己切到生气，这里只负责让台词也跟着换个态度。
   * 长按（心智观察器）与甩飞晕眩（drag:dizzy）走各自的入口调同一个 requestPetReaction。
   * 从 rest/busy 唤醒仍由 onModelClick 的连续点击计数负责，不在这里重复处理。
   */
  const handleChibiInteraction = useCallback((interaction: ChibiInteraction) => {
    // 长按触发打开心智观察器后紧接着的 click 是松手余波，不触发反应
    if (Date.now() - holdCompletedAtRef.current < 500) return;
    lastActivityRef.current = Date.now();
    lastBubbleFromProactiveRef.current = 0;

    // 双击本身会展开侧边聊天窗、有自己的反馈，不叠提示音
    if (interaction !== 'double_click') {
      const characterId = (getCharacterId() ?? 'vivian').toLowerCase().includes('nana') ? 'nana' : 'vivian';
      playPetTapSound(characterId);
    }
    requestPetReaction(interaction);
  }, [requestPetReaction]);

  /** 确保/展开微信窗口（label='chat'，右缘三态侧边栏）。
   *  创建为屏幕右缘屏外隐藏，由 Rust 三态机制（edge watcher + mouse hook +
   *  expand/collapse）驱动往复滑动。已存在时直接展开（透视其当前屏外/peek 状态）。 */
  const ensureWechatWindow = useCallback(async (opts?: { show?: boolean }): Promise<void> => {
    const label = 'chat';
    const shouldShow = opts?.show !== false;

    // iPhone 17 真实比例（393 × 852 逻辑点，1:2.1679）。
    // 宽度固定 390，高度按 852/393 ≈ 2.1679 推导出 ≈ 845。
    // 避免随屏幕高度扩展导致内部元素/长宽比变形。
    const IPHONE17_ASPECT = 852 / 393;
    const windowWidth = 390;
    const windowHeight = Math.round(windowWidth * IPHONE17_ASPECT); // ≈ 845

    const [monitor, factor] = await Promise.all([
      currentMonitor(),
      getCurrentWindow().scaleFactor(),
    ]);
    const screenH = (monitor?.size.height ?? 1080) / factor;
    const screenW = (monitor?.size.width ?? 1920) / factor;
    // 垂直居中（屏幕过高时避免贴顶），过小的屏幕则压缩高度但保持整高比的下限 568 (iPhone SE)
    const maxHeight = Math.max(568, Math.min(windowHeight, Math.round(screenH - 24)));
    const fittedWidth = Math.round(maxHeight / IPHONE17_ASPECT);
    const finalW = maxHeight < windowHeight ? fittedWidth : windowWidth;
    const finalH = maxHeight;
    const x = Math.round(screenW); // 屏幕右缘之外（hidden / peek 起始）
    const y = Math.max(0, Math.round((screenH - finalH) / 2));

    const existing = await WebviewWindow.getByLabel(label);
    if (existing) {
      try {
        await existing.setSize(new LogicalSize(finalW, finalH));
        await existing.setPosition(new LogicalPosition(x, y));
      } catch {
        /* ignore */
      }
      if (shouldShow) {
        await invoke('show_side_chat_animated', { label }).catch(() => {});
      }
      return;
    }

    const win = new WebviewWindow(label, {
      url: `/?view=${label}`,
      title: '微信',
      width: finalW,
      height: finalH,
      x,
      y,
      resizable: false,
      decorations: false,
      transparent: true,
      alwaysOnTop: true,
      skipTaskbar: true,
      shadow: false,
      focus: false,
      visible: false,
    });

    win.once('tauri://created', () => {
      if (shouldShow) {
        void invoke('show_side_chat_animated', { label }).catch(() => {});
      }
    });
  }, []);
  const ensureBubbleWindow = useCallback(async (): Promise<void> => {
    const bubbleLabel = charScopedLabel('bubble');
    const existing = await WebviewWindow.getByLabel(bubbleLabel);
    if (existing) return;

    bubbleReadyRef.current = false;
    new WebviewWindow(bubbleLabel, {
      url: `/?view=bubble&character_id=${getCharacterId() ?? ''}`,
      title: 'Vivian Bubble',
      width: BUBBLE_WINDOW_WIDTH,
      height: BUBBLE_WINDOW_HEIGHT,
      resizable: false,
      decorations: false,
      transparent: true,
      alwaysOnTop: true,
      skipTaskbar: true,
      shadow: false,
      focus: false,
      visible: false,
    });
    // 等待 BubbleWindow 挂载并发出 bubble:ready（由 useEffect 监听）
  }, []);

  // 监听 bubble:ready：窗口就绪后发送缓存的待显示文本
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      unlisten = await listen<{ character_id?: string }>('bubble:ready', (e) => {
        if (e.payload?.character_id && e.payload.character_id !== getCharacterId()) return;
        bubbleReadyRef.current = true;
        const pending = pendingBubbleTextRef.current;
        if (pending !== null) {
          pendingBubbleTextRef.current = null;
          void emitBubbleShow(pending);
        }
      });
      if (cancelled) { safeUnlisten(unlisten); return; }
      console.log(`[DIAG] listen registered: bubble:ready, char=${getCharacterId()}`);
    })();
    return () => { cancelled = true; safeUnlisten(unlisten); };
  }, [emitBubbleShow]);

  // 气泡窗口自身一直穿透（setIgnoreCursorEvents(true)），不需要 suspend/resume
  // 桌宠窗口的穿透状态。

  // 监听 currentBubble 变化，驱动气泡窗口生命周期
  useEffect(() => {
    const text = currentBubble;
    const prev = prevBubbleTextRef.current;
    prevBubbleTextRef.current = text;

    if (text === null) {
      // 气泡隐藏
      void emit('bubble:hide', { character_id: getCharacterId() ?? undefined });
      void WebviewWindow.getByLabel(charScopedLabel('bubble')).then((w) => {
        if (w) void w.hide();
      });
      pendingBubbleTextRef.current = null;
      return;
    }

    if (prev === null) {
      // 首次显示（null → 非空）
      if (bubbleReadyRef.current) {
        // 窗口已存在且就绪 → 直接发送
        void emitBubbleShow(text);
      } else {
        // 窗口未创建或未就绪 → 缓存文本，创建窗口后由 bubble:ready 触发发送
        pendingBubbleTextRef.current = text;
        void ensureBubbleWindow();
      }
    } else {
      // 文本更新（流式/追加）— 不重建窗口，仅更新文本
      void emit('bubble:update', {
        text,
        character_id: getCharacterId() ?? undefined,
        cross_character: useAppStore.getState().bubbleCrossCharacter,
        listener_name: useAppStore.getState().bubbleListenerName ?? undefined,
      });
      // 流式动态扩大：根据文本长度重算窗口高度与 y 坐标，
      // 使气泡随内容增长而增大（上方模式向上扩展，下方模式向下扩展）
      const dynHeight = estimateBubbleHeight(text);
      const anchor = lastBubbleAnchorRef.current;
      if (anchor && dynHeight !== BUBBLE_WINDOW_HEIGHT) {
        void WebviewWindow.getByLabel(charScopedLabel('bubble')).then(async (w) => {
          if (!w) return;
          try {
            await w.setSize(new LogicalSize(BUBBLE_WINDOW_WIDTH, dynHeight));
            // 上方模式：窗口底边贴合桌宠顶部 → y = petWinY - dynHeight
            // 下方模式：窗口顶边贴合桌宠底部 → y = petWinY + petWinH
            const newY = anchor.position === 'top'
              ? anchor.petWinY - dynHeight
              : anchor.petWinY + anchor.petWinH;
            await w.setPosition(new LogicalPosition(anchor.x, Math.round(newY)));
          } catch {
            /* ignore */
          }
        });
      }
    }
  }, [currentBubble, emitBubbleShow, ensureBubbleWindow]);

  // 监听 settledBubbles 变化：转发 add/remove 事件到气泡窗口，并调整窗口高度
  useEffect(() => {
    const currentIds = new Set(settledBubbles.map((b) => b.id));
    const prevIds = prevSettledIdsRef.current;
    const charId = getCharacterId() ?? undefined;

    // 检测新增的已结算气泡 → 发送 settled_add 事件
    for (const b of settledBubbles) {
      if (!prevIds.has(b.id)) {
        void emit('bubble:settled_add', {
          id: b.id,
          text: b.text,
          duration: b.duration,
          character_id: charId,
        });
      }
    }

    // 检测移除的已结算气泡 → 发送 settled_remove 事件
    for (const id of prevIds) {
      if (!currentIds.has(id)) {
        void emit('bubble:settled_remove', {
          id,
          character_id: charId,
        });
      }
    }

    prevSettledIdsRef.current = currentIds;

    // 调整气泡窗口高度：活跃气泡 + 已结算气泡的总高度
    const activeText = currentBubble ?? '';
    const allTexts = [
      ...settledBubbles.map((b) => b.text),
      ...(activeText ? [activeText] : []),
    ];
    if (allTexts.length === 0) return;

    // 估算总高度：各气泡高度之和 + gap(8) * (n-1) + padding(16)
    const totalHeight = allTexts.reduce((sum, t) => sum + estimateBubbleHeight(t), 0)
      + 8 * Math.max(0, allTexts.length - 1) + 16;
    const dynHeight = Math.min(BUBBLE_WINDOW_MAX_HEIGHT * 2, Math.max(BUBBLE_WINDOW_MIN_HEIGHT, totalHeight));

    const anchor = lastBubbleAnchorRef.current;
    if (anchor) {
      void WebviewWindow.getByLabel(charScopedLabel('bubble')).then(async (w) => {
        if (!w) return;
        try {
          await w.setSize(new LogicalSize(BUBBLE_WINDOW_WIDTH, dynHeight));
          const newY = anchor.position === 'top'
            ? anchor.petWinY - dynHeight
            : anchor.petWinY + anchor.petWinH;
          await w.setPosition(new LogicalPosition(anchor.x, Math.round(newY)));
        } catch {
          /* ignore */
        }
      });
    }
  }, [settledBubbles, currentBubble]);

  // 主窗口移动时重新定位气泡窗口
  useEffect(() => {
    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      try {
        unlisten = await win.onMoved(() => {
          // 仅在气泡可见时（currentBubble 非空）重新定位
          if (prevBubbleTextRef.current === null) return;
          void (async () => {
            const bubbleWin = await WebviewWindow.getByLabel(charScopedLabel('bubble'));
            if (!bubbleWin) return;
            const posInfo = await computeBubbleWindowPosition();
            if (!posInfo) return;
            try {
              await bubbleWin.setPosition(new LogicalPosition(posInfo.x, posInfo.y));
            } catch {
              /* ignore */
            }
          })();
        });
        // 迟到 resolve 兜底：cleanup 先于监听器注册完成时，当场解绑防泄漏
        if (cancelled) { safeUnlisten(unlisten); unlisten = undefined; }
      } catch {
        /* ignore */
      }
    })();
    return () => { cancelled = true; safeUnlisten(unlisten); unlisten = undefined; };
  }, [computeBubbleWindowPosition]);

  // 记录用户活动（鼠标移动、键盘按下、点击）
  useEffect(() => {
    const markActive = () => {
      lastActivityRef.current = Date.now();
    };
    window.addEventListener('mousemove', markActive, { passive: true });
    window.addEventListener('keydown', markActive, { passive: true });
    window.addEventListener('mousedown', markActive, { passive: true });
    return () => {
      window.removeEventListener('mousemove', markActive);
      window.removeEventListener('keydown', markActive);
      window.removeEventListener('mousedown', markActive);
    };
  }, []);

  useEffect(() => {
    const timer = setTimeout(() => {
      void getCurrentWindow().show().catch(() => {});
    }, 5000);
    return () => clearTimeout(timer);
  }, []);

  // 初始化：加载语言、心情、启动问候、TTS 配置、启动主动对话
  useEffect(() => {
    void (async () => {
      console.log(`[DIAG] init useEffect START, char=${getCharacterId()}, time=${Date.now()}`);
      try {
        const lang = await configApi.get<string>('base.language').catch(() => '');
        if (lang) await changeLanguage(lang);
      } catch {
        /* ignore */
      }
      console.log(`[DIAG] init: lang done, char=${getCharacterId()}`);
      try {
        const mood = await moodApi.getCurrent();
        useAppStore.getState().setMood(mood);
      } catch {
        /* ignore */
      }
      console.log(`[DIAG] init: mood done, char=${getCharacterId()}`);
      // 加载用户自定义头像（data URL，null 表示使用默认头像）
      try {
        const dataUrl = await invoke<string | null>('get_user_avatar_data_url');
        useAppStore.getState().setUserAvatarUrl(dataUrl ?? null);
      } catch {
        /* ignore */
      }
      // 加载 TTS 配置
      try {
        ttsConfigRef.current = await ttsApi.getConfig();
        const ttsOn = !!ttsConfigRef.current?.enabled;
        // 同步后端 TTS 配置到 store
        useAppStore.getState().setTtsEnabled(ttsOn);
        // 后端启用且无持久化值（首次启动）时，前端 voiceEnabled 自动跟随启用；
        // 已有持久化值时尊重用户上次的选择，避免重启后静音状态丢失
        if (ttsOn && !hasPersistedVoiceEnabled()) {
          useAppStore.getState().setVoiceEnabled(true);
        }
        // 读取最新 state（set 后闭包中的 store 仍是旧快照），初始化 TtsStreamQueue
        const latestState = useAppStore.getState();
        TtsStreamQueue.setEnabled(ttsOn && latestState.voiceEnabled);
      } catch {
        /* ignore */
      }
      console.log(`[DIAG] init: tts done, char=${getCharacterId()}`);

      // 等待后端 Brain 初始化完成（监听 app:ready 事件，带超时兜底）
      await waitForAppReady();
      console.log(`[DIAG] init: waitForAppReady returned, char=${getCharacterId()}, time=${Date.now()}`);

      // 创建左侧对话面板窗口
      void ensureSideChatWindow();

      // 启动预检未通过（主 LLM / 嵌入服务未配置或本地服务未就绪）时，
      // 直接打开设置窗口并展示配置说明，不进入问候/主动对话。
      try {
        const initialized = await invoke<boolean>('is_initialized');
        if (!initialized) {
          console.log(`[DIAG] init: backend not initialized, opening config guide, char=${getCharacterId()}`);
          openConfig(true);
          useAppStore.getState().setInitialized(true);
          return;
        }
      } catch {
        /* ignore */
      }

      // 主 LLM 未配置时跳过问候与主动对话，打开设置页触发配置引导弹窗
      try {
        const mainApiConfigured = await invoke<boolean>('is_main_api_configured');
        if (!mainApiConfigured) {
          console.log(`[DIAG] init: main LLM not configured, opening config window, char=${getCharacterId()}`);
          openConfig();
          useAppStore.getState().setInitialized(true);
          return;
        }
      } catch {
        /* ignore */
      }

      // 启动问候 - 通过 LifecycleController 统一编排：首次见面判定 + 问候生成 + 持久化
      // 同步模式：TTS 启用时等语音就绪再显示气泡，音画同步
      try {
        const syncGreeting = !!ttsConfigRef.current?.enabled;
        const result = await LifecycleController.initGreeting({ syncWithAudio: syncGreeting });
        if (result.greeting) {
          lastBubbleFromProactiveRef.current = Date.now();
          if (ttsConfigRef.current?.enabled) {
            if (
              ttsConfigRef.current.engine === 'gptsovits' &&
              ttsConfigRef.current.gpt_sovits_auto_start
            ) {
              await waitForGptSoVitsReady();
            }
            await TtsStreamQueue.speakSync(result.greeting);
            LifecycleController.showGreetingBubble(result.greeting);
          }
        } else if (result.error) {
          // 问候失败几乎总是主 LLM 调用失败的同一次故障（问候要走主 LLM），
          // 而 llm:error 已经就同一个错误弹过提示了。短窗内不再重复，
          // 否则用户会看到两条说同一件事的 toast。原始错误请查日志。
          if (Date.now() - lastLlmErrorToastAtRef.current > 3000) {
            showToast(t('toast.greeting_failed'), 'warning', 6000);
          }
        }
      } catch {
        /* ignore */
      }
      // 启动主动对话（受 proactive.enabled 配置项控制）
      try {
        const proactiveEnabled = await configApi
          .get<boolean>('proactive.enabled')
          .catch(() => true);
        const tickIntervalSec = await configApi
          .get<number>('proactive.tick_interval')
          .catch(() => 10);
        proactiveTickIntervalRef.current = Math.max(
          1,
          Math.floor((tickIntervalSec || 10) * 1000),
        );
        if (proactiveEnabled) {
          await proactiveApi.start();
          setProactiveStarted(true);
        }
      } catch {
        /* ignore */
      }
      useAppStore.getState().setInitialized(true);
      console.log(`[DIAG] init useEffect COMPLETE, char=${getCharacterId()}, time=${Date.now()}`);
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 系统托盘事件（后端触发显示）
  useEffect(() => {
    let cancelled = false;
    let unlistenFn: (() => void) | undefined;
    void (async () => {
      try {
        unlistenFn = await listen('tray:show', () => {
          void getCurrentWindow().show();
          positioningCoordinator.triggerSmartCheck?.();
        });
        if (cancelled) { safeUnlisten(unlistenFn); return; }
        console.log(`[DIAG] listen registered: tray:show, char=${getCharacterId()}`);
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlistenFn);
    };
  }, []);

  // 设置窗口保存后同步语言切换（ConfigWindow 是独立 WebviewWindow，无法直接更新主窗口的 i18n）
  useEffect(() => {
    let cancelled = false;
    let unlistenFn: (() => void) | undefined;
    void (async () => {
      try {
        unlistenFn = await listen<{ language: string }>(
          'config:language-changed',
          (event) => {
            const lang = event.payload?.language;
            if (lang) void changeLanguage(lang);
          },
        );
        if (cancelled) { safeUnlisten(unlistenFn); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlistenFn);
    };
  }, []);

  // 智能避让配置：初始加载 + 监听 ConfigWindow 保存后的 config:saved 事件
  useEffect(() => {
    let cancelled = false;
    let unlistenFn: (() => void) | undefined;
    void (async () => {
      try {
        const enabled = await configApi
          .get<boolean>('window.smart_positioning_enabled')
          .catch(() => true);
        setSmartPositioningEnabled(enabled);
      } catch {
        /* ignore */
      }
      try {
        unlistenFn = await listen('config:saved', async () => {
          try {
            const enabled = await configApi
              .get<boolean>('window.smart_positioning_enabled')
              .catch(() => true);
            setSmartPositioningEnabled(enabled);
          } catch {
            /* ignore */
          }
        });
        if (cancelled) { safeUnlisten(unlistenFn); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlistenFn);
    };
  }, [configApi]);

  // 实时看鼠标配置：初始加载 + 监听 config:saved 事件
  useEffect(() => {
    let cancelled = false;
    let unlistenFn: (() => void) | undefined;
    const load = async () => {
      const enabled = await configApi
        .get<boolean>('pet_render.always_follow_mouse')
        .catch(() => false);
      setAlwaysFollowMouse(!!enabled);
    };
    void (async () => {
      try {
        await load();
      } catch {
        /* ignore */
      }
      try {
        unlistenFn = await listen('config:saved', () => {
          void load().catch(() => {
            /* ignore */
          });
        });
        if (cancelled) { safeUnlisten(unlistenFn); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlistenFn);
    };
  }, [configApi]);

  // 心情 → 注视行为联动（滞回状态机）：
  // 桌宠精力充沛且专注（energy/focus ≥ 60）→ 激活实时看鼠标；
  // 精力或专注任一回落 < 35 → 退回随机张望。中间地带维持原状态，避免抖动。
  useEffect(() => {
    const mood = currentMood;
    if (!mood) return;
    const energy = mood.energy ?? 0;
    const focus = mood.focus ?? 0;
    const prev = moodFollowActiveRef.current;
    let next = prev;
    if (!prev && energy >= MOOD_FOLLOW_ENERGY_HIGH && focus >= MOOD_FOLLOW_FOCUS_HIGH) {
      next = true;
    } else if (prev && (energy < MOOD_FOLLOW_ENERGY_LOW || focus < MOOD_FOLLOW_FOCUS_LOW)) {
      next = false;
    }
    if (next !== prev) {
      moodFollowActiveRef.current = next;
      setMoodFollowActive(next);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentMood]);

  // Ollama 服务就绪时弹出 toast（每个应用生命周期只弹一次）
  // 用 ref 持有最新依赖，useEffect 依赖为空数组确保 listener 只注册一次
  const showToastRef = useRef(showToast);
  const tRef = useRef(t);
  showToastRef.current = showToast;
  tRef.current = t;
  useEffect(() => {
    let unlistenFn: (() => void) | undefined;
    void (async () => {
      try {
        unlistenFn = await listen<{
          model_installed?: boolean;
          model?: string;
          permission_denied?: boolean;
        }>(
          'ollama:ready',
          async (event) => {
            // 提前去重：在 await 之前设置标志，防止并发 listener 同时通过检查
            if (ollamaToastedRef.current) return;
            ollamaToastedRef.current = true;
            try {
              const source = await configApi.get<string>('memory.embedding.source').catch(() => '');
              if (source !== 'local') return;
              const payload = event.payload ?? {};
              const model =
                payload.model ??
                (await configApi.get<string>('memory.embedding.ollama_model').catch(() => 'bge-m3'));
              if (payload.model_installed) {
                showToastRef.current(
                  tRef.current('config.toast_ollama_ready', { model }),
                  'success',
                  4000,
                  OLLAMA_READY_TOAST_KEY,
                );
              } else if (payload.permission_denied) {
                showToastRef.current(
                  tRef.current('config.toast_ollama_permission_denied', { model }),
                  'error',
                  8000,
                  OLLAMA_READY_TOAST_KEY,
                );
              } else {
                showToastRef.current(
                  tRef.current('config.toast_ollama_model_missing', { model }),
                  'warning',
                  6000,
                  OLLAMA_READY_TOAST_KEY,
                );
              }
            } catch {
              /* ignore */
            }
          },
        );
      } catch {
        /* ignore */
      }
    })();
    return () => { safeUnlisten(unlistenFn); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 监听主动对话配置变更（设置窗口保存后触发）：
  // - 更新 tick_interval（递归 setTimeout 下一次调度自动生效）
  // - 按 enabled 决定是否 start/stop proactive
  useEffect(() => {
    let cancelled = false;
    let unlistenFn: (() => void) | undefined;
    void (async () => {
      try {
        unlistenFn = await listen('proactive:config-changed', async () => {
          try {
            const tickIntervalSec = await configApi
              .get<number>('proactive.tick_interval')
              .catch(() => 10);
            proactiveTickIntervalRef.current = Math.max(
              1,
              Math.floor((tickIntervalSec || 10) * 1000),
            );
            const enabled = await configApi
              .get<boolean>('proactive.enabled')
              .catch(() => true);
            if (enabled && !proactiveStarted) {
              await proactiveApi.start();
              setProactiveStarted(true);
            } else if (!enabled && proactiveStarted) {
              await proactiveApi.stop();
              setProactiveStarted(false);
            }
          } catch {
            /* ignore */
          }
        });
        if (cancelled) { safeUnlisten(unlistenFn); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlistenFn);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [proactiveStarted]);

  // 监听 TTS 配置变更（设置窗口保存后触发），同步 ttsEnabled 与 voiceEnabled
  useEffect(() => {
    let cancelled = false;
    let unlistenFn: (() => void) | undefined;
    void (async () => {
      try {
        unlistenFn = await listen<{ enabled: boolean }>(
          'tts:config-changed',
          async (event) => {
            const ttsOn = !!event.payload?.enabled;
            ttsConfigRef.current = await ttsApi.getConfig().catch(() => ttsConfigRef.current);
            let shouldEnableQueue = ttsOn;
            if (ttsOn) {
              // 后端启用朗读：读取当前 voiceEnabled 状态
              const curState = useAppStore.getState();
              if (!curState.voiceEnabled) {
                // voiceEnabled 为 false 时自动开启（首次启用/后端刚打开）
                curState.setVoiceEnabled(true);
              }
              // 读取更新后的最新 state
              const latestState = useAppStore.getState();
              shouldEnableQueue = latestState.voiceEnabled;
              curState.setTtsEnabled(true);
            } else {
              // 后端禁用朗读：强制关闭前端语音开关并停止播放
              const curState = useAppStore.getState();
              curState.setTtsEnabled(false);
              curState.setVoiceEnabled(false);
              shouldEnableQueue = false;
              void TtsStreamQueue.stop();
            }
            TtsStreamQueue.setEnabled(shouldEnableQueue);
          },
        );
        if (cancelled) { safeUnlisten(unlistenFn); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlistenFn);
    };
  }, []);

  // 监听日记写入完成事件，显示 toast
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ character_id?: string; character_name?: string }>('diary:written', (event) => {
          // 多角色过滤：仅当前角色窗口显示对应角色的日记 toast
          if (event.payload?.character_id && event.payload.character_id !== getCharacterId()) return;
          showToast(t('toast.diary_written', { name: event.payload?.character_name ?? '' }), 'success', 4000, undefined, undefined, event.payload?.character_id || null);
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [t, showToast]);

  // 监听主 LLM API 未配置事件（聊天/唤醒问候/日记生成等场景后端校验失败）
  useEffect(() => {
    let cancelled = false;
    const unlistens: Array<() => void> = [];
    void (async () => {
      try {
        const un1 = await listen<{ character_id?: string }>('chat:config_error', (event) => {
          if (event.payload?.character_id && event.payload.character_id !== getCharacterId()) return;
          // 归属原样透传：空归属交给主角色窗口，避免每个角色窗口各弹一条
          showToast(t('toast.api_not_configured'), 'warning', 6000, undefined, undefined, event.payload?.character_id || null);
        });
        if (cancelled) { un1(); return; }
        unlistens.push(un1);
        const un2 = await listen<{ character_id?: string }>('llm:not_configured', (event) => {
          if (event.payload?.character_id && event.payload.character_id !== getCharacterId()) return;
          showToast(t('toast.api_not_configured'), 'warning', 6000, undefined, undefined, event.payload?.character_id || null);
        });
        if (cancelled) { un2(); return; }
        unlistens.push(un2);
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      for (const un of unlistens) un();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [t, showToast]);

  // 监听路由回退事件（路由矩阵中某任务 API 失败，已回退到主 LLM API）
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ task_type: string; message_key?: string; error?: string; character_id?: string }>('chat:route_fallback', (event) => {
          // 归属守卫：有归属时只响应属于本角色的回退；无归属转交主角色窗口（同 llm:error）
          const owner = event.payload?.character_id;
          if (owner && owner !== getCharacterId()) return;
          const taskType = event.payload?.task_type ?? '';
          const messageKey = event.payload?.message_key;
          const rawError = event.payload?.error ?? '';
          const taskLabelMap: Record<string, string> = {
            chat: t('config.routing_chat'),
            reasoning: t('config.routing_reasoning'),
            diary: t('config.routing_diary'),
            memory: t('config.routing_memory'),
            consolidation: t('config.routing_consolidation'),
            reflection: t('config.routing_reflection'),
            emotion_analysis: t('config.routing_emotion_analysis'),
            inner_monologue: t('config.routing_inner_monologue'),
          };
          const taskLabel = taskLabelMap[taskType] ?? taskType;
          let reasonText = '';
          if (messageKey) {
            const translated = t(messageKey as any, { error: rawError });
            if (translated && !translated.includes('llm_error_') && translated !== messageKey) {
              reasonText = translated;
            }
          }
          if (reasonText) {
            const shortReason = reasonText.length > 40 ? reasonText.slice(0, 40) + '…' : reasonText;
            showToast(t('toast.route_fallback_reason', { task: taskLabel, reason: shortReason }), 'warning', 5000, undefined, undefined, owner || null);
          } else {
            showToast(t('toast.route_fallback', { task: taskLabel }), 'warning', 5000, undefined, undefined, owner || null);
          }
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [t, showToast]);

  // 监听 LLM 错误事件（所有 LLM provider 均失败时触发，含后台任务）
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ task_type: string; message_key: string; error: string; error_kind: string; endpoint: string; character_id?: string }>('llm:error', (event) => {
          // 归属守卫：**有归属**时只弹给触发本次调用的角色。
          // 无归属（后台任务未声明角色 / 旧版后端未下发）不丢弃，转交主角色窗口呈现一次——
          // 严格丢弃会让「余额不足」这类必须让用户知道的错误彻底消失，而广播又会让
          // 每只桌宠各弹一条。两者之间只有「主角色窗口认领」这一个既不重复也不丢信息的解。
          const owner = event.payload?.character_id;
          if (owner && owner !== getCharacterId()) return;
          const { message_key: messageKey } = event.payload ?? {};
          if (!messageKey) return;
          // messageKey 后端下发时**已带 `toast.` 前缀**（如 toast.llm_error_insufficient_balance），
          // 必须原样查表。此前这里先 replace 掉前缀再查，i18n 永远命中不了，
          // 导致所有 LLM 错误都退化成通用文案、而厂商分类结果被白白丢掉。
          const translated = t(messageKey as any);
          // 未命中 i18n 时回落到通用文案，且不回显原始错误串：厂商响应体里带
          // request_id、endpoint、计费信息，不适合直接呈现给用户。排查请查日志。
          const message = translated && !translated.includes('llm_error_')
            ? translated
            : t('toast.llm_error_unknown');
          const kind = event.payload?.error_kind ?? '';
          // 熔断器打开是本地自我保护，会自行恢复，不打扰用户
          if (kind === 'circuit_breaker_open') return;
          lastLlmErrorToastAtRef.current = Date.now();
          const isPermanent = ['invalid_api_key', 'insufficient_balance', 'quota_exceeded', 'model_not_found', 'region_not_supported', 'permission_denied'].includes(kind);
          // 欠费 / 配额类错误挂「前往 xx 控制台」动作：endpoint 由 Rust 下发，
          // 是这次调用实际使用的 provider（含路由回退后的真实厂商），
          // 据此反查厂商预设拿 consoleUrl 与显示名；custom endpoint 查不到就不挂。
          let action: ToastAction | undefined;
          if (kind === 'insufficient_balance' || kind === 'quota_exceeded') {
            const preset = findProviderPresetByEndpoint(event.payload?.endpoint ?? '');
            if (preset?.consoleUrl) {
              action = {
                kind: 'open_url',
                url: preset.consoleUrl,
                label: t('toast.goto_console', { vendor: t(preset.labelKey as any) }),
              };
            }
          }
          showToast(message, isPermanent ? 'error' : 'warning', isPermanent ? 10000 : 6000, undefined, action, owner || null);
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [t, showToast]);

  // 监听待办变更事件（添加/更新/完成/删除时显示 Toast）
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ action: string; item: { title?: string; id: string }; character_id?: string }>(
          'todo:changed',
          (event) => {
            // 归属守卫：只弹给发起变更的角色（UI 手动操作无归属，不弹）
            if (event.payload?.character_id !== getCharacterId()) return;
            const { action, item } = event.payload;
            let message = '';
            switch (action) {
              case 'added':
                message = t('toast.todo_added', { title: item.title || '' });
                break;
              case 'updated':
                message = t('toast.todo_updated', { title: item.title || '' });
                break;
              case 'completed':
                message = t('toast.todo_completed', { title: item.title || '' });
                break;
              case 'deleted':
                message = t('toast.todo_deleted');
                break;
            }
            if (message) showToast(message, 'success', 4000);
          },
        );
        if (cancelled) { safeUnlisten(unlisten); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => { cancelled = true; safeUnlisten(unlisten); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [t, showToast]);

  // 监听定时任务变更事件（添加/触发/取消时显示 Toast）
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ action: string; task: { message?: string }; character_id?: string }>(
          'scheduler:changed',
          (event) => {
            // 归属守卫：只弹给创建该任务的角色（手动创建/旧任务无归属，不弹）
            if (event.payload?.character_id !== getCharacterId()) return;
            const { action, task } = event.payload;
            let message = '';
            switch (action) {
              case 'added':
                message = t('toast.scheduler_added', { message: task.message || '' });
                break;
              case 'triggered':
                message = t('toast.scheduler_triggered', { message: task.message || '' });
                break;
              case 'cancelled':
                message = t('toast.scheduler_cancelled');
                break;
            }
            if (message) showToast(message, 'info', 4000);
          },
        );
        if (cancelled) { safeUnlisten(unlisten); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => { cancelled = true; safeUnlisten(unlisten); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [t, showToast]);

  // 监听工具执行确认请求（文件操作、屏幕截取等隐私敏感工具）
  // 后端 emit tool:confirmation_request → 转发给 toast 子窗口渲染三按钮确认卡片
  // （拒绝 / 放行一次 / 始终允许），由 toast 窗口 invoke confirm_tool_execution 回传结果
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<ToolConfirmPayload>('tool:confirmation_request', (event) => {
          const payload = event.payload;
          // 仅处理本角色的确认请求（Tauri emit 广播到所有窗口，需按 char_id 过滤）
          if (payload.char_id && payload.char_id !== getCharacterId()) return;
          if (toastReadyRef.current) {
            void emit('toast:confirm', payload);
          } else {
            pendingConfirmRef.current.push(payload);
            void ensureToastWindow();
          }
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ensureToastWindow]);

  // 主动对话流式 chunk 监听：后端生成期间推送 proactive:chunk 事件
  // 流式期间只缓存文本，不喂 TTS 也不显示气泡——等 proactive_tick 返回后
  // 根据 delivery_channel 分发：bubble 渠道喂 TTS + showBubble，chat_window 渠道跳过 TTS
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ text: string; character_id?: string }>('proactive:chunk', (event) => {
          if (event.payload?.character_id && event.payload.character_id !== getCharacterId()) return;
          proactiveStreamTextRef.current += event.payload.text;
        });
        if (!unlisten) return;
      } catch {
        /* ignore */
      }
    })();
    return () => {
      safeUnlisten(unlisten);
    };
  }, []);

  // 主动旁观插话监听：用户与角色 A 对话时，旁观者 B 经 LLM 判断后主动插话
  // 后端 emit proactive:bubble 事件，前端负责 showBubble + TTS + 写入 chat:assistant_message
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{
          character_id: string;
          content: string;
          expression?: string;
        }>('proactive:bubble', (event) => {
          if (event.payload?.character_id && event.payload.character_id !== getCharacterId()) return;
          const rawText = event.payload?.content ?? '';
          if (!rawText) return;
          const text = stripActions(rawText);
          if (!text) return;
          void (async () => {
            if (ttsConfigRef.current?.enabled) {
              TtsStreamQueue.feedSync(text, {});
              await TtsStreamQueue.flushSync();
            }
            BubbleController.showBubble(text);
            void emit('chat:assistant_message', {
              content: text,
              timestamp: new Date().toISOString(),
              character_id: getCharacterId() ?? undefined,
              channel: 'proactive',
            });
          })();
        });
        if (!unlisten) return;
      } catch {
        /* ignore */
      }
    })();
    return () => {
      safeUnlisten(unlisten);
    };
  }, []);

  // 跨角色对话流式监听（同步模式：语音开始时才显示文字气泡，音画同步）
  useEffect(() => {
    let cancelled = false;
    const unlisteners: (() => void)[] = [];
    const crossStreamTextRef = { current: '' };
    let crossSyncStarted = false;
    const crossListenerNameRef = { current: '' };
    void (async () => {
      try {
        // cross:start：源角色发起说话（speaker_id=源角色）
        const unStart = await listen<{
          stream_id: string; speaker_id: string; listener_id: string;
          speaker_name: string; listener_name: string; message: string;
        }>('cross:start', (event) => {
          if (event.payload.speaker_id !== getCharacterId()) return;
          crossStreamTextRef.current = '';
          crossSyncStarted = false;
          crossListenerNameRef.current = event.payload.listener_name;
          TtsStreamQueue.resetBuffer();
          // listener_name为User时，这是对用户说话，不应标记为跨角色
          const isCrossChar = event.payload.listener_name !== 'User' && event.payload.listener_name !== 'user';
          const crossOpts = { crossCharacter: isCrossChar, listenerName: event.payload.listener_name };
          const rawMsg = event.payload.message;
          const msg = rawMsg ? stripActions(rawMsg) : '';
          if (msg && ttsConfigRef.current?.enabled) {
            void (async () => {
              await TtsStreamQueue.speakSync(msg);
              if (cancelled) return;
              BubbleController.showBubble(msg, undefined, crossOpts);
              void emit('chat:assistant_message', {
                content: msg,
                timestamp: new Date().toISOString(),
                character_id: getCharacterId() ?? undefined,
                channel: isCrossChar ? 'cross_character' : 'proactive',
              });
            })();
          } else if (msg) {
            BubbleController.showBubble(msg, undefined, crossOpts);
          }
        });
        unlisteners.push(unStart);

        // cross:chunk：目标角色回复流式文本（speaker_id=目标角色=正在回复的角色）
        const unChunk = await listen<{
          text: string; stream_id: string; speaker_id: string; listener_id: string; listener_name?: string;
        }>('cross:chunk', (event) => {
          if (event.payload.speaker_id !== getCharacterId()) return;
          crossStreamTextRef.current += event.payload.text;
          if (!crossListenerNameRef.current) {
            crossListenerNameRef.current = event.payload.listener_name || event.payload.listener_id;
          }
          const listenerName = crossListenerNameRef.current;
          const isCrossChar = listenerName !== 'User' && listenerName !== 'user';
          const crossOpts = { crossCharacter: isCrossChar, listenerName };
          if (ttsConfigRef.current?.enabled) {
            TtsStreamQueue.feedSync(event.payload.text, {
              onFirstAudioStart: () => {
                if (cancelled) return;
                crossSyncStarted = true;
                const cleanText = stripActions(crossStreamTextRef.current);
                BubbleController.showStreamingBubble(cleanText, crossOpts);
              },
            });
          } else {
            const cleanText = stripActions(crossStreamTextRef.current);
            BubbleController.showStreamingBubble(cleanText, crossOpts);
            TtsStreamQueue.feed(event.payload.text);
          }
        });
        unlisteners.push(unChunk);

        // cross:done：目标角色回复完成
        const unDone = await listen<{
          text: string; stream_id: string; speaker_id: string; listener_id: string; listener_name?: string;
          expression: string; motion: string; response_mode: string;
        }>('cross:done', (event) => {
          if (event.payload.speaker_id !== getCharacterId()) return;
          const rawText = event.payload.text || crossStreamTextRef.current;
          const finalText = stripActions(rawText);
          const listenerName = crossListenerNameRef.current || event.payload.listener_name || event.payload.listener_id;
          const isCrossChar = listenerName !== 'User' && listenerName !== 'user';
          const crossOpts = { crossCharacter: isCrossChar, listenerName };
          void (async () => {
            if (ttsConfigRef.current?.enabled) {
              await TtsStreamQueue.flushSync();
            } else {
              TtsStreamQueue.flush();
            }
            if (cancelled) return;
            if (finalText) {
              BubbleController.showBubble(finalText, undefined, crossOpts);
              void emit('chat:assistant_message', {
                content: finalText,
                timestamp: new Date().toISOString(),
                character_id: getCharacterId() ?? undefined,
                channel: isCrossChar ? 'cross_character' : 'proactive',
              });
            }
            crossStreamTextRef.current = '';
            crossSyncStarted = false;
            crossListenerNameRef.current = '';
          })();
        });
        unlisteners.push(unDone);

        if (cancelled) {
          unlisteners.forEach(u => u());
        }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      unlisteners.forEach(u => { try { u(); } catch { /* ignore */ } });
    };
  }, []);

  // 跨角色发言通知监听：其他角色发言后广播 proactive:spoken 事件，
  // 本角色记录时间戳，在下次 tick 时延迟执行，避免同时或连续发言
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ character_id: string; timestamp: number }>('proactive:spoken', (event) => {
          if (event.payload?.character_id === getCharacterId()) return;
          otherSpokenAtRef.current = Date.now();
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
  }, []);

  // 主动对话 tick 轮询：间隔由 proactive.tick_interval 配置项驱动（动态递归 setTimeout）
  useEffect(() => {
    if (!proactiveStarted) return;
    let cancelled = false;
    let timerId: number | null = null;
    const scheduleNext = (delayMs: number) => {
      timerId = window.setTimeout(runTick, delayMs);
    };
    const runTick = async () => {
      if (cancelled) return;
      // 用户对话进行中时跳过主动 tick，避免主动消息与对话 TTS/气泡竞争
      if (ChatController.isStreaming) {
        scheduleNext(proactiveTickIntervalRef.current);
        return;
      }
      // 其他角色最近发言后延迟本角色 tick，避免同时或连续发言
      if (otherSpokenAtRef.current > 0) {
        const sinceOther = Date.now() - otherSpokenAtRef.current;
        const cooldownMs = crossCooldownMsRef.current;
        if (sinceOther < cooldownMs) {
          scheduleNext(cooldownMs - sinceOther);
          return;
        }
      }
      // 视频动画演出自动触发（后端按 配置开关 + 冷却 + 活动匹配 决定；缺素材时优雅跳过）
      try {
        await invoke('video_animation_auto_tick', {
          characterId: getCharacterId() ?? undefined,
        });
      } catch {
        /* 后端未就绪忽略 */
      }
      const now = Date.now();
      const idleSeconds = (now - lastActivityRef.current) / 1000;
      const awaySeconds = (now - lastUserMessageRef.current) / 1000;
      const userPresent = idleSeconds < IDLE_AWAY_THRESHOLD_SECONDS;
      const windowChanged =
        activeWindowRef.current !== lastActiveWindowRef.current;
      const ctx: ProactiveTickContext = {
        idle_seconds: idleSeconds,
        away_seconds: awaySeconds,
        user_present: userPresent,
        interaction_count_today: 0,
        active_window: activeWindowRef.current,
        window_changed: windowChanged,
        last_topic_relevant: false,
        has_relevant_memory: false,
        drag_distance: dragDistanceRef.current,
        // 注意：这里必须用 LLM 在 chat:done 中判定的真实用户情绪，
        // 不能用 store.currentMood.primary_emotion——那是 Vivian 自身的 mood，
        // 会把 Vivian 的内部情绪误传为用户情绪（曾导致 proactive LLM 凭空"觉得用户难过"）。
        user_emotion: useAppStore.getState().lastUserEmotion ?? '',
      };
      try {
        proactiveStreamTextRef.current = '';
        const resp = await proactiveApi.tick(ctx);
        if (resp.messages && resp.messages.length > 0) {
          lastActiveWindowRef.current = activeWindowRef.current;
          for (const msg of resp.messages as ProactiveMessage[]) {
            // 按 delivery_channel 分流：
            // - chat_window（微信渠道）：后端已写入 dialogue(channel=wechat) + emit chat:assistant_message
            //   + 在 chat 窗口不可见时 emit wechat:message_banner。微信消息不需要 TTS，
            //   不 showBubble（不弹桌宠气泡）也不重复 emit chat:assistant_message。
            // - bubble（桌宠气泡）：前端负责 TTS + showBubble + emit chat:assistant_message(channel=proactive)，
            //   后端不发任何事件。
            if (msg.delivery_channel === 'chat_window') {
              // 微信渠道：跳过 TTS，关闭流式期间可能残留的桌宠气泡
              BubbleController.closeAll();
              continue;
            }
            // bubble 渠道：喂 TTS 并等待播放完成，再显示气泡
            if (ttsConfigRef.current?.enabled) {
              TtsStreamQueue.feedSync(msg.content, {});
              await TtsStreamQueue.flushSync();
            }
            BubbleController.showBubble(msg.content);
            void emit('chat:assistant_message', {
              content: msg.content,
              timestamp: new Date(msg.timestamp * 1000 || Date.now()).toISOString(),
              character_id: getCharacterId() ?? undefined,
              channel: 'proactive',
            });
          }
          lastBubbleFromProactiveRef.current = now;
        } else if (proactiveStreamTextRef.current) {
          BubbleController.closeAll();
        }
        // 自适应 tick 间隔：后端根据用户空闲时间推荐下次 tick 延迟
        if (typeof resp.recommended_next_interval_ms === 'number' && resp.recommended_next_interval_ms > 0) {
          proactiveTickIntervalRef.current = resp.recommended_next_interval_ms;
        }
        // 跨角色冷却时长：后端按角色 reluctance 差异化下发
        if (typeof resp.effective_cross_cooldown_ms === 'number' && resp.effective_cross_cooldown_ms > 0) {
          crossCooldownMsRef.current = resp.effective_cross_cooldown_ms;
        }
      } catch {
        /* ignore */
      }
      if (!cancelled) {
        scheduleNext(proactiveTickIntervalRef.current);
      }
    };
    // 首次 tick 错峰：根据 character_id 添加偏移，避免两个角色窗口同时触发 tick。
    // vivian 延迟 0ms，nana 延迟半个周期，使两者 tick 永远错开。
    const initialDelay = (() => {
      const cid = getCharacterId();
      if (cid === 'nana') return Math.floor(proactiveTickIntervalRef.current / 2);
      return proactiveTickIntervalRef.current;
    })();
    scheduleNext(initialDelay);
    return () => {
      cancelled = true;
      if (timerId !== null) window.clearTimeout(timerId);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [proactiveStarted]);

  // 心理微调 tick：让情绪持续波动（Homeostasis + 微噪声），并刷新 mood 到全局 store
  // 刷新 store.currentMood 后，微表情 / 呼吸频率 / 自主行为等订阅 mood 的逻辑才能随真实注意力动态切换
  // 休息时降频：正常 3s，休息/离线时 30s，避免空闲时高频写盘/IPC
  useEffect(() => {
    let cancelled = false;
    let timerId: number | null = null;
    const NORMAL_INTERVAL = 3000;
    const REST_INTERVAL = 30000;
    const scheduleNext = () => {
      if (cancelled) return;
      const ps = presenceStateRef.current;
      const isResting = ps === 'rest' || ps === 'offline';
      const interval = isResting ? REST_INTERVAL : NORMAL_INTERVAL;
      timerId = window.setTimeout(runTick, interval);
    };
    const runTick = async () => {
      await invoke('psychology_micro_tick', { characterId: getCharacterId() ?? undefined }).catch(() => {
        /* 后端未就绪忽略 */
      });
      const m = await moodApi.getCurrent().catch(() => null);
      if (m) useAppStore.getState().setMood(m);
      scheduleNext();
    };
    scheduleNext();
    return () => {
      cancelled = true;
      if (timerId !== null) window.clearTimeout(timerId);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 心情表情触发 tick：空闲时根据当前心情随机触发表情，让桌宠更生动
  // 随机 25-35s 间隔（避免机械感）；休息/对话流式/页面隐藏时跳过
  // 后端负责概率门控 + 20s 冷却 + push_action，前端只需周期调用
  useEffect(() => {
    let cancelled = false;
    let timerId: number | null = null;
    const scheduleNext = () => {
      if (cancelled) return;
      const interval = 25000 + Math.random() * 10000;
      timerId = window.setTimeout(runTick, interval);
    };
    const runTick = async () => {
      if (cancelled) return;
      const ps = presenceStateRef.current;
      const isResting = ps === 'rest' || ps === 'offline';
      if (!isResting && !ChatController.isStreaming && !document.hidden) {
        await invoke('mood_expression_tick', { characterId: getCharacterId() ?? undefined }).catch(() => {
          /* 后端未就绪忽略 */
        });
      }
      scheduleNext();
    };
    // 首次延迟 15s 启动（等待心理状态稳定）
    timerId = window.setTimeout(runTick, 15000);
    return () => {
      cancelled = true;
      if (timerId !== null) window.clearTimeout(timerId);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 自动表情触发 tick（每4秒）：检查空闲状态、心情持续表情、程序事件等
  // 无需LLM参与，纯规则驱动，让桌宠在用户不交互时也能有丰富表情
  useEffect(() => {
    let cancelled = false;
    let timerId: number | null = null;
    const AUTO_TICK_INTERVAL = 4000;
    let lastTimeOfDay = '';

    const tick = async () => {
      if (cancelled) return;
      try {
        const ps = presenceStateRef.current;
        const isResting = ps === 'rest' || ps === 'offline';
        if (!isResting && !ChatController.isStreaming && !document.hidden) {
          await invoke('auto_expression_tick', { characterId: getCharacterId() ?? undefined });
        }

        // 时间段变化检测（早/中/晚/夜）
        const hour = new Date().getHours();
        let timeOfDay = '';
        if (hour >= 6 && hour < 12) timeOfDay = 'morning';
        else if (hour >= 12 && hour < 18) timeOfDay = 'afternoon';
        else if (hour >= 18 && hour < 23) timeOfDay = 'evening';
        else timeOfDay = 'night';
        if (timeOfDay !== lastTimeOfDay && lastTimeOfDay !== '') {
          await invoke('trigger_system_event', { event: timeOfDay, characterId: getCharacterId() ?? undefined });
        }
        lastTimeOfDay = timeOfDay;
      } catch {
        /* ignore */
      }
      if (!cancelled) {
        timerId = window.setTimeout(tick, AUTO_TICK_INTERVAL);
      }
    };

    timerId = window.setTimeout(tick, 8000); // 延迟8秒启动，等其他系统就绪
    return () => {
      cancelled = true;
      if (timerId !== null) window.clearTimeout(timerId);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 窗口聚焦/失焦事件触发
  useEffect(() => {
    const handleFocus = () => {
      void invoke('trigger_system_event', { event: 'window_focus', characterId: getCharacterId() ?? undefined }).catch(() => {});
    };
    const handleBlur = () => {
      void invoke('trigger_system_event', { event: 'window_blur', characterId: getCharacterId() ?? undefined }).catch(() => {});
    };
    window.addEventListener('focus', handleFocus);
    window.addEventListener('blur', handleBlur);
    return () => {
      window.removeEventListener('focus', handleFocus);
      window.removeEventListener('blur', handleBlur);
    };
  }, []);

  // 桌宠动作队列消费：工具层通过 push_action 投递的动作请求在此取出并驱动 桌宠
  // 事件驱动：后端 push_action 时 emit `pet:action_pending`，前端 listen 触发一次性 drain
  // 兜底轮询：保留 PET_ACTION_DRAIN_INTERVAL_MS 间隔防止事件丢失
  useEffect(() => {
    type PendingAction = {
      kind: string;
      target: string;
      params: Record<string, unknown>;
      timestamp: number;
    };

    const drainAndApply = async () => {
      let actions: PendingAction[] = [];
      try {
        const resp = await invoke<{ actions: PendingAction[] }>('drain_pet_actions', { characterId: getCharacterId() ?? undefined });
        actions = resp.actions ?? [];
      } catch {
        /* 后端未就绪忽略 */
      }
      if (actions.length === 0) return;

      const handle = petRef.current;
      for (const act of actions) {
        const { kind, target, params } = act;
        switch (kind) {
          case 'expression':
            // 后端规则表驱动的限时表情。图集格位（如 dizzy）靠这个 duration_ms 限时，
            // 到点由画布自己回落到 idle——没有它那张脸会一直挂着。
            handle?.setExpression(target, (params.duration_ms as number) || 0);
            break;
          case 'motion':
          case 'animation':
            handle?.playMotion(target);
            break;
          case 'idle':
            // 触发引擎随机待机动作（与 commands::engine::trigger_idle_action 同源）
            void invoke('trigger_idle_action', { characterId: getCharacterId() ?? undefined }).catch(() => {});
            break;
          case 'bubble': {
            const text = (params.text as string) || '';
            if (text) {
              BubbleController.showBubble(text);
            }
            break;
          }
          case 'mood': {
            // mood 联动表情（pet_behavior_tools 中已计算 expression 字段）
            const expression = (params.expression as string) || '';
            if (expression) {
              handle?.setExpression(expression, 3000);
            }
            break;
          }
          case 'state':
            // 状态切换由 Presence 系统统一管理（rest/offline），此处无 桌宠对应
            break;
          case 'window': {
            // 窗口位置/尺寸由 Tauri window API 直接设置
            try {
              const win = getCurrentWindow();
              if (target === 'position') {
                const x = (params.x as number) ?? 0;
                const y = (params.y as number) ?? 0;
                await win.setPosition(new LogicalPosition(x, y));
              } else if (target === 'size') {
                const w = (params.width as number) ?? 400;
                const h = (params.height as number) ?? 500;
                await win.setSize(new LogicalSize(w, h));
              }
            } catch {
              /* ignore */
            }
            break;
          }
          case 'query':
          case 'watch_mode':
          case 'behavior_mode':
          case 'follow_cursor':
            // 这些是引擎状态/查询类，无直接 桌宠副作用，暂不处理
            break;
          default:
            break;
        }
      }
    };

    // 兜底轮询（防事件丢失）
    const id = window.setInterval(drainAndApply, PET_ACTION_DRAIN_INTERVAL_MS);

    // 事件驱动：收到后端 emit 后立即 drain 一次
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ character_id?: string }>('pet:action_pending', (event) => {
          if (event.payload?.character_id && event.payload.character_id !== getCharacterId()) return;
          void drainAndApply();
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
      } catch {
        /* ignore */
      }
    })();

    return () => {
      cancelled = true;
      window.clearInterval(id);
      safeUnlisten(unlisten);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 环境信息更新：每 30 秒同步一次鼠标位置和活动窗口
  useEffect(() => {
    const id = window.setInterval(async () => {
      try {
        // 通过后端获取当前活动窗口信息
        const info = await environmentApi.getInfo();
        if (info.active_window) {
          activeWindowRef.current = info.active_window;
        }
      } catch {
        /* ignore */
      }
    }, ENVIRONMENT_UPDATE_INTERVAL_MS);
    return () => window.clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 主动消息"被冷落"检测已迁移到后端（proactive.tick 内的权威判定，见 proactive/mod.rs）：
  // 主动消息投递后若用户在场且超过 IGNORE_TIMEOUT_SECS 仍未回应，则 on_ignored 一次。
  // 前端不再用 ref 驱动的定时器判定（ref 变化不触发重渲染，原实现不可靠且 ignored_count 恒为 0）。

  // requestRestore / hideForSleep / restoreFromSleep / hideForOffline / restoreFromOffline 引用 ——
  // 让快捷键回调和事件监听器能调用最新的闭包，无需重新注册
  const requestRestoreRef = useRef<() => void>(() => {});
  const hideForSleepRef = useRef<() => void>(() => {});
  const restoreFromSleepRef = useRef<() => void>(() => {});
  const hideForOfflineRef = useRef<() => Promise<void>>(() => Promise.resolve());
  const restoreFromOfflineRef = useRef<() => Promise<void>>(() => Promise.resolve());
  useEffect(() => {
    requestRestoreRef.current = requestRestore;
  }, [requestRestore]);
  useEffect(() => {
    hideForSleepRef.current = hideForSleep;
  }, [hideForSleep]);
  useEffect(() => {
    restoreFromSleepRef.current = restoreFromSleep;
  }, [restoreFromSleep]);
  useEffect(() => {
    hideForOfflineRef.current = hideForOffline;
  }, [hideForOffline]);
  useEffect(() => {
    restoreFromOfflineRef.current = restoreFromOffline;
  }, [restoreFromOffline]);

  // hideReason 引用 —— 快捷键回调需要读取最新的隐藏原因以判断是否从睡眠唤醒
  const hideReasonRef = useRef<HideReason | null>(null);
  useEffect(() => {
    hideReasonRef.current = hideReason;
  }, [hideReason]);

  /** 触发睡眠唤醒问候：调用后端 try_wake_greeting 命令，
   *  概率命中时 LLM 生成问候语，展示气泡 + TTS 朗读 */
  const triggerWakeGreeting = useCallback(async () => {
    try {
      const result = await invoke<{
        greeting: string | null;
        probability: number;
        triggered: boolean;
      }>('try_wake_greeting', { characterId: getCharacterId() ?? undefined });
      if (result.triggered && result.greeting) {
        lastBubbleFromProactiveRef.current = Date.now();
        if (ttsConfigRef.current?.enabled) {
          await TtsStreamQueue.speakSync(result.greeting);
        }
        BubbleController.showBubble(result.greeting);
      }
    } catch {
      /* 后端未就绪忽略 */
    }
    // 函数体内未使用 store/ttsApi，原先的依赖是误写——会导致每次 re-render 引用变化
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  const triggerWakeGreetingRef = useRef<() => void>(() => {});
  useEffect(() => {
    triggerWakeGreetingRef.current = triggerWakeGreeting;
  }, [triggerWakeGreeting]);

  // 文字快捷键由后端统一注册（tauri_plugin_global_shortcut），前端仅监听事件。
  // 三个快捷键：vivian 私聊、nana 私聊、broadcast 群发总框。
  // 配置变更由 ConfigWindow 直接调用 update_text_shortcuts 命令重新注册。

  // 监听 Vivian 私聊快捷键事件：确保 SideChat 窗口存在并显示 InputDialog
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen('input:shortcut:vivian', () => {
          if (getCharacterId() !== 'vivian') return;
          const wasSleep = hideReasonRef.current === 'sleep';
          requestRestoreRef.current?.();
          if (wasSleep) {
            void invoke('set_presence_state', { target: 'online', characterId: getCharacterId() ?? undefined }).catch(() => {});
            void triggerWakeGreetingRef.current?.();
          }
          // 确保 SideChat 窗口存在，并通过 URL 参数或事件通知显示 InputDialog
          void ensureSideChatWindow({ showInput: true, show: true, lock: true });
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
        console.log(`[DIAG] listen registered: input:shortcut:vivian, char=${getCharacterId()}`);
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 监听 Nana 私聊快捷键事件：确保 SideChat 窗口存在并显示 InputDialog
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen('input:shortcut:nana', () => {
          if (getCharacterId() !== 'nana') return;
          const wasSleep = hideReasonRef.current === 'sleep';
          requestRestoreRef.current?.();
          if (wasSleep) {
            void invoke('set_presence_state', { target: 'online', characterId: getCharacterId() ?? undefined }).catch(() => {});
            void triggerWakeGreetingRef.current?.();
          }
          void ensureSideChatWindow({ showInput: true, show: true, lock: true });
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
        console.log(`[DIAG] listen registered: input:shortcut:nana, char=${getCharacterId()}`);
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 启动微信窗口（chat）右缘三态边缘检测线程并预创建隐藏窗口：
  // Rust 线程幂等（双角色窗口重复调用无害），chat 窗口预创建后保持屏外隐藏，
  // 由右缘悬停 peek 或托盘「微信」展开，避免首次呼出冷启动 WebView2 的延迟。
  useEffect(() => {
    void invoke('start_side_chat_edge_watcher').catch(() => {});
    void invoke('start_side_chat_mouse_hook').catch(() => {});
    void invoke('start_side_chat_left_watcher').catch(() => {});
    void ensureWechatWindow({ show: false });
    void ensureSideChatWindow({ show: false });
    // 预创建微信消息横幅窗口（常驻隐藏，由后端事件触发显示）
    void ensureMessageBannerWindow();
  }, [ensureWechatWindow, ensureSideChatWindow, ensureMessageBannerWindow]);

  // 监听群发快捷键事件：打开 SideChat 广播模式
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen('input:shortcut:broadcast', async () => {
          await ensureSideChatWindow({ showInput: true, show: true, lock: true });
          void emit('sidechat:show_input', {
            broadcast: true,
            auto_start_voice: false,
          });
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 监听后端语音输入快捷键事件：确保 SideChat 窗口存在，
  // InputDialog 由 SideChatPanel 监听同一事件呼出并自动启动语音
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ character_id?: string }>('input:voice_shortcut', (event) => {
          const cid = event.payload?.character_id;
          // broadcast：群发语音，所有角色窗口都响应
          if (cid === 'broadcast') {
            void ensureSideChatWindow({ showInput: true, autoVoice: true, show: true, lock: true });
            void emit('sidechat:show_input', {
              broadcast: true,
              auto_start_voice: true,
            });
            return;
          }
          // 多角色过滤：仅活跃角色窗口响应全局语音快捷键
          if (cid && cid !== getCharacterId()) return;
          // 先退出隐藏到角落模式（全屏隐藏 / 睡眠隐藏均生效）
          const wasSleep = hideReasonRef.current === 'sleep';
          requestRestoreRef.current?.();
          if (wasSleep) {
            void invoke('set_presence_state', { target: 'online', characterId: getCharacterId() ?? undefined }).catch(() => {});
            void triggerWakeGreetingRef.current?.();
          }
          void ensureSideChatWindow({ showInput: true, autoVoice: true, show: true, lock: true });
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
        console.log(`[DIAG] listen registered: input:voice_shortcut, char=${getCharacterId()}`);
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 在场状态监听：驱动 桌宠行为（表情/闭眼/鼠标跟随/隐藏）
  // Rest 状态 = 休息：回落到 idle + 闭眼 + 隐藏到角落（露出 48px）
  // Busy 状态 = 后台任务：掏出手机 → 常驻「看手机」循环（留在原位，见下方姿态效应）
  // Offline 状态 = 离线：真正 hide_window，只能通过托盘/快捷键唤回
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        const info = await invoke<{ state: string }>('get_presence_state', { characterId: getCharacterId() });
        if (!cancelled) {
          setPresenceState(info?.state ?? null);
          useAppStore.getState().setPresenceState(info?.state ?? null);
          // 启动时按状态分发隐藏策略
          // Busy 不在其列：忙碌是一段**看得见**的表演（掏出手机、看手机），退到角落只剩
          // 48px 时就什么都看不到了，等于白做。忙碌时留在原位，「不主动打扰」由它自己
          // 那套姿态负责，而不是靠把窗口挪走。
          if (info?.state === 'rest') {
            hideForSleepRef.current?.();
          } else if (info?.state === 'offline') {
            void hideForOfflineRef.current?.();
          }
        }
      } catch (err) {
        console.warn('[presence] get_presence_state 失败:', err);
      }
      try {
        unlisten = await listen<{ character_id: string; from: string; to: string; farewell_text?: string | null }>('presence:changed', (event) => {
          if (event.payload?.character_id && event.payload.character_id !== getCharacterId()) return;
          const from = event.payload?.from ?? '';
          const to = event.payload?.to ?? null;
          const farewell = event.payload?.farewell_text ?? null;
          setPresenceState(to);
          useAppStore.getState().setPresenceState(to);

          const applyPresenceChange = () => {
            // Rest：退到角落；Offline：真正 hide_window；Busy：留在原位播忙碌表演
            if (to === 'rest') {
              hideForSleepRef.current?.();
            } else if (to === 'offline') {
              void hideForOfflineRef.current?.();
            } else if (from === 'rest') {
              restoreFromSleepRef.current?.();
            } else if (from === 'offline') {
              void restoreFromOfflineRef.current?.();
            }
          };

          // 有告别语时：先显示气泡 + TTS，延迟隐藏让用户看到告别语
          if (farewell && (to === 'rest' || to === 'offline')) {
            BubbleController.showBubble(farewell);
            if (ttsConfigRef.current?.enabled) {
              TtsStreamQueue.feed(farewell);
            }
            void emit('chat:assistant_message', {
              content: farewell,
              timestamp: new Date().toISOString(),
              character_id: getCharacterId() ?? undefined,
              channel: 'proactive',
            });
            const delay = Math.max(3000, farewell.length * 150);
            window.setTimeout(() => applyPresenceChange(), delay);
          } else {
            applyPresenceChange();
          }
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // presence 状态驱动的桌宠姿态：
  // - busy：掏出手机 → 常驻「看手机」循环，退出时倒放收起手机（见 ChibiPetCanvas 的
  //   startBusy / stopBusy）。它是一段**帧序列**而非单个格位，进场与退出各有前摇收尾，
  //   所以不能像格位那样一句 setExpression 了事。
  // - rest：休息 = 平静待机 → 回落到 idle，**不再挂 dizzy**
  //   rest 期间窗口本就隐藏，强制 dizzy 只在唤醒瞬间闪一下，看着像出 bug；
  //   且「休息」语义是安睡/发呆，不是持续旋转的晕眩脸（dizzy 应留给真正的头晕/疲惫）。
  // - online/其他：恢复默认姿态
  //
  // 退出忙碌必须单独走 stopBusy：它要先把「收起手机」倒放完，而不是一刀切回 idle。
  // 因此这里记一份「上一轮是不是忙碌」，否则 enter 与 exit 会被同一个 else 分支吞掉。
  const busyStageRef = useRef(false);
  useEffect(() => {
    if (!modelReady) return;
    const handle = petRef.current;
    if (!handle) return;
    if (presenceState === 'busy') {
      busyStageRef.current = true;
      handle.startBusy();
      return;
    }
    if (busyStageRef.current) {
      busyStageRef.current = false;
      handle.stopBusy();
      return;
    }
    // rest / online / 其他：统一回落到 idle，不再把 rest 当成 dizzy
    handle.resetExpression();
  }, [presenceState, modelReady]);

  // 监听 direct 渠道被拦截：后端 emit chat:presence_blocked → toast 提示用户改用微信
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{
          stream_id: string;
          character_id: string;
          presence: string;
          hint?: string;
        }>('chat:presence_blocked', (event) => {
          if (event.payload?.character_id && event.payload.character_id !== getCharacterId()) return;
          const hint = event.payload.hint || t('chat.presence_blocked_default');
          // 同 presence:wake_deferred：原样转发归属，空归属交由主角色窗口收敛
          void emit('toast:show', { message: hint, type: 'warning', duration: 5000, key: `presence_blocked_${Date.now()}`, character_id: event.payload?.character_id || undefined });
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 监听唤醒被延迟：用户尝试唤醒但任务进行中（Busy 知识采集 / Rest 记忆沉淀）
  // 后端 emit presence:wake_deferred → toast 提示「等我做完」
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{
          character_id: string;
          from_state: string;
          task: string;
          hint?: string;
        }>('presence:wake_deferred', (event) => {
          if (event.payload?.character_id && event.payload.character_id !== getCharacterId()) return;
          const hint = event.payload.hint || t('toast.wake_deferred_default');
          // 原样转发 payload 的归属，而不是改写成本窗口角色：本监听是宽松守卫
          // （空归属时所有角色窗口都会通过），若各自标成自身角色，就会各弹一条。
          // 空归属保持为空 → 接收侧按「无归属只弹主角色窗口」收敛。
          void emit('toast:show', {
            message: hint,
            type: 'info',
            duration: 4000,
            key: `wake_deferred_${Date.now()}`,
            character_id: event.payload?.character_id || undefined,
          });
        });
        if (cancelled) { safeUnlisten(unlisten); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      safeUnlisten(unlisten);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ── 文件拖放：通过 Tauri 原生 onDragDropEvent 获取文件路径 ──
  const extractFileText = useExtractFileText();
  const [isDragOver, setIsDragOver] = useState(false);

  // Drop 逻辑用 ref 保存最新闭包，避免 onDragDropEvent 监听器持有过时状态
  const handleFileDropRef = useRef<(paths: string[]) => void>(() => {});
  handleFileDropRef.current = (paths: string[]) => {
    const charId = getCharacterId() ?? undefined;
    if (!charId || paths.length === 0) return;

    void (async () => {
      for (const filePath of paths) {
        try {
          const result: FileTextResult = await extractFileText(filePath);

          if (result.file_type === 'image') {
            await invoke('send_image_message', {
              sourcePath: filePath,
              characterId: charId,
            });
            showToast(
              t('toast.image_dropped', {
                filename: result.filename,
                defaultValue: '已发送图片：{{filename}}',
              }),
              'success',
              3000,
            );
          } else if (result.file_type === 'unsupported') {
            showToast(
              t('toast.file_unsupported', {
                filename: result.filename,
                defaultValue: '不支持的文件类型：{{filename}}',
              }),
              'warning',
              4000,
            );
          } else {
            const truncatedHint = result.truncated
              ? t('toast.file_truncated', {
                  count: result.original_char_count,
                  defaultValue: `（文件过长，已截断，原始 ${result.original_char_count} 字符）`,
                })
              : '';
            const message = `[文件：${result.filename}]\n${result.text}${truncatedHint}`;
            const fileMetadata = {
              kind: 'file',
              file_name: result.filename,
              file_type: result.file_type,
              truncated: result.truncated,
              original_char_count: result.original_char_count,
            };
            void ChatController.sendMessage(message, charId, 'wechat', undefined, fileMetadata);
          }
        } catch (err) {
          showToast(
            t('toast.file_extract_failed', {
              error: String(err),
              defaultValue: '文件处理失败：{{error}}',
            }),
            'error',
            5000,
          );
        }
      }
    })();
  };

  // 注册原生拖放事件监听（仅一次）
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    void (async () => {
      // 点击穿透已移除：窗口始终响应鼠标，文件拖放不再需要 suspend/resume 配对
      unlisten = await getCurrentWindow().onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === 'enter') {
          setIsDragOver(true);
        } else if (payload.type === 'leave') {
          setIsDragOver(false);
        } else if (payload.type === 'drop') {
          setIsDragOver(false);
          handleFileDropRef.current(payload.paths);
        }
      });
    })();
    return () => { unlisten?.(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ── 长按桌宠打开心智观察器 ──
  // 左键按住（不拖动）满 HOLD_OPEN_TOTAL_MS（1s）后打开心智观察器；
  // 超过 HOLD_RING_DELAY_MS（0.2s）后在按住位置显示顺时针填充的环形进度槽，
  // 同时起播施法逐帧动画。
  /**
   * 放行一次已经成立的长按：让（预热好的）心智观察器显形。
   *
   *  预热把「建窗口」提前到了长按成立之前，但「建好」不等于「子窗口接得住显形事件」
   *  ——两者之间隔着一整个页面冷启。所以这里分三种情况：
   *  - 没有预热会话（长按快得没跑到 0.1s；或预热链自己放弃了）：直接走常规打开；
   *  - 子窗口已回执就绪：立刻放行，这一下就是「松手即见」；
   *  - 还没回执：挂起等回执（见 pet:prewarm_ready 监听），另配超时兜底——回执只是
   *    一条事件，丢了不该让长按毫无反应。
   *
   *  这里**不**提前腾会话位：挂起期间回执还要靠它认领这次长按。
   */
  const releasePrewarm = useCallback(() => {
    const session = prewarmRef.current;
    if (!session) {
      holdActionRef.current();
      return;
    }
    session.committed = true;
    void (async () => {
      // 等预热链落地：窗口得先真的建出来，常规打开路径的复用判定才认得它
      await session.chain;
      if (prewarmRef.current !== session) return;
      if (session.childReady) {
        finishPrewarm(prewarmRef, session, () => holdActionRef.current());
        return;
      }
      session.readyTimer = window.setTimeout(() => {
        session.readyTimer = null;
        finishPrewarm(prewarmRef, session, () => holdActionRef.current());
      }, PREWARM_READY_WAIT_MS);
    })();
  }, []);

  const cancelHold = useCallback(() => {
    if (!holdActiveRef.current) return;
    holdActiveRef.current = false;
    if (holdTimerRef.current !== undefined) {
      window.clearTimeout(holdTimerRef.current);
      holdTimerRef.current = undefined;
    }
    if (prewarmTimerRef.current !== undefined) {
      window.clearTimeout(prewarmTimerRef.current);
      prewarmTimerRef.current = undefined;
    }
    // 中止预热：这次长按没成立，提前建出来的窗口不该留下（详见 abortPrewarmSession）。
    // 加载没跑完的也在这一下被打断——窗口正在建的话由预热链自己收尾销毁。
    const session = prewarmRef.current;
    if (session) {
      prewarmRef.current = null;
      abortPrewarmSession(session);
    }
    setHoldRingPos(null);
    // 施法动画从当前帧倒放回初始帧（进度环未出现/已播完时无会话，no-op）
    petRef.current?.cancelCast();
  }, []);

  const completeHold = useCallback(() => {
    if (!holdActiveRef.current) return;
    holdActiveRef.current = false;
    holdCompletedAtRef.current = Date.now();
    setHoldRingPos(null);
    // 此时用户仍按着左键：主动结束拖拽会话，清除后端 DRAG_OFFSET，
    // 避免后续松手被光标轨迹采样解析为拖拽/甩飞
    void invoke('stop_window_drag').catch(() => {});
    // 施法动画与进度环同步播放，此时应已自然播完；兜底立即归位（不倒放）
    petRef.current?.stopCast();
    // 预热会话若已就绪，这一下就让它显形；还没就绪则挂起，等子窗口回执或超时
    // （详见 releasePrewarm）。没有预热会话（没跑到 0.1s / 预热链放弃了）时，
    // releasePrewarm 会直接走常规打开路径。
    releasePrewarm();
    // 长按本身也是一次用户动作：记进事件账本，并让桌宠随口反应一句
    requestPetReaction('long_press');
  }, [requestPetReaction, releasePrewarm]);

  const startHold = useCallback((clientX: number, clientY: number) => {
    // 新长按会话：上一段施法（含取消倒放）直接归位，避免与本次叠加
    petRef.current?.stopCast();
    cancelHold();
    holdActiveRef.current = true;
    holdCompletedAtRef.current = 0;
    // 记录窗口初始位置：拖拽期间窗口跟随光标移动（client 坐标不变，
    // mousemove 检测不到拖动），只能通过窗口位移判定
    void getCurrentWindow()
      .outerPosition()
      .then((pos) => {
        if (holdActiveRef.current) holdStartWinPosRef.current = { x: pos.x, y: pos.y };
      })
      .catch(() => {});
    // 提前加载：0.1s 就开始建心智观察器窗口（预热期间一律不上屏）。
    // 比进度环（0.2s）更早，把「窗口加载」这段最慢的活儿挪到长按判定期间干。
    prewarmTimerRef.current = window.setTimeout(() => {
      prewarmTimerRef.current = undefined;
      if (!holdActiveRef.current) return;
      prewarmActionRef.current();
    }, HOLD_PREWARM_DELAY_MS);
    holdTimerRef.current = window.setTimeout(() => {
      holdTimerRef.current = undefined;
      if (!holdActiveRef.current) return;
      setHoldRingPos({ x: clientX, y: clientY });
      // 进度环出现的同时开始施法动画：播放时长与进度环填充一致，同步填满
      petRef.current?.startCast(HOLD_OPEN_TOTAL_MS - HOLD_RING_DELAY_MS);
    }, HOLD_RING_DELAY_MS);
  }, [cancelHold]);

  // 长按取消监听：松手 / 拖拽位移超容差 / 后端 watchdog 取消拖拽，三条路径统一取消
  useEffect(() => {
    const win = getCurrentWindow();
    const onMouseUp = () => cancelHold();
    window.addEventListener('mouseup', onMouseUp);
    let unlistenDragCancelled: (() => void) | undefined;
    let unlistenMoved: (() => void) | undefined;
    void (async () => {
      try {
        unlistenDragCancelled = await win.listen('drag:cancelled', () => cancelHold());
      } catch {
        /* listen 不可用时跳过 */
      }
      try {
        unlistenMoved = await win.onMoved((e) => {
          if (!holdActiveRef.current || !holdStartWinPosRef.current) return;
          const dx = e.payload.x - holdStartWinPosRef.current.x;
          const dy = e.payload.y - holdStartWinPosRef.current.y;
          if (Math.hypot(dx, dy) > HOLD_MOVE_TOLERANCE_PX) cancelHold();
        });
      } catch {
        /* onMoved 不可用时跳过 */
      }
    })();
    return () => {
      window.removeEventListener('mouseup', onMouseUp);
      safeUnlisten(unlistenDragCancelled);
      safeUnlisten(unlistenMoved);
      cancelHold();
      // 卸载时预热会话也一并作废：窗口不该留在这个已经没人管的界面上
      const session = prewarmRef.current;
      if (session) {
        prewarmRef.current = null;
        abortPrewarmSession(session);
      }
    };
  }, [cancelHold]);

  // 预热就绪回执：子窗口挂好 `pet:reveal` 监听后广播一次。
  //
  // 这是「提前加载」这条链的最后一环。预热把建窗口提前了，但建好 ≠ 加载完，
  // 两者之间隔着一整个页面冷启；长按成立时若子窗口还没接住监听，显形事件就丢了，
  // 窗口只能靠兜底定时器迟到显形——那正好抵消掉预热的意义。
  //
  // 两种到达时机都要处理：
  // - 长按还没成立：只记下「就绪」，等长按成立时立刻放行；
  // - 长按已经成立（放行时在等它）：当场放行，这就是「松手即见」那一刻。
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      try {
        unlisten = await listen<PetPrewarmReady>(PET_PREWARM_READY_EVENT, (e) => {
          const session = prewarmRef.current;
          // 不是本会话的回执（多角色桌宠各有一场预热）→ 丢弃
          if (!session || e.payload?.session !== session.session) return;
          session.childReady = true;
          if (session.committed) {
            finishPrewarm(prewarmRef, session, () => holdActionRef.current());
          }
        });
        if (cancelled) { safeUnlisten(unlisten); unlisten = undefined; }
      } catch {
        /* listen 不可用：放行时的超时兜底会接管，长按照常打开 */
      }
    })();
    return () => { cancelled = true; safeUnlisten(unlisten); unlisten = undefined; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /**
   * 开始自定义窗口拖动。
   *
   * 绕过 Windows 工作区限制（`startDragging` 会把超出屏幕顶部的窗口弹回）：取光标的
   * 屏幕坐标交给后端，cursor tracking 线程随后用 `SetWindowPos` 追着光标移动窗口。
   *
   * 后端记下的是**调用那一刻**的光标相对窗口左上角的偏移，之后每 60ms 用它钉一次窗口
   * ——所以「什么时候调用」很关键：窗口被别的东西挪走之后再调用，这个偏移就是陈的，
   * 追踪线程会把窗口一路拽回调用时的位置（见 handleBackgroundMouseDown 里的逃离分支）。
   */
  const beginWindowDrag = useCallback(async () => {
    // 窗口归用户了：逃离（若正在跑）据此让位，别和人抢窗口
    positioningCoordinator.dragInFlight = true;
    try {
      const cursor = await invoke<{ x: number; y: number }>('get_cursor_position');
      await invoke('start_window_drag', { cursorX: cursor.x, cursorY: cursor.y });
    } catch {
      // 后端命令失败时回退到原生 startDragging
      void getCurrentWindow().startDragging();
    }
  }, []);

  // 背景层窗口拖拽
  const handleBackgroundMouseDown = useCallback(async (e: React.MouseEvent) => {
    console.log(`[DIAG] mousedown (drag), char=${getCharacterId()}, button=${e.button}, x=${e.clientX}, y=${e.clientY}`);
    if (e.button !== 0) return;
    // 左键按住期间启动长按检测：不松手、不拖动满 1s 打开心智观察器
    startHold(e.clientX, e.clientY);
    dragPressAliveRef.current = true;

    // 逃离途中先压住拖动，等按住够久（人真的想抓住它）再补上这一拖。
    //
    // 不能一按下就开拖：拖动是「窗口跟着光标走」，而追踪线程的偏移是**按下那一刻**采的
    // ——那时位移还没开始，于是每按一下窗口就被钉回按下时的位置，刚滑出去的距离整段作废。
    // 连点的时候每一按都拽一次，位移看起来就是「原地不动」。按下时长是这里唯一的判据：
    // 点一下是逗它，按住才是抓它（与画布那边 `isHoldingPet` 共用同一个阈值）。
    if (positioningCoordinator.fleeInFlight) {
      if (dragDeferTimerRef.current !== null) window.clearTimeout(dragDeferTimerRef.current);
      dragDeferTimerRef.current = window.setTimeout(() => {
        dragDeferTimerRef.current = null;
        // 到点时人已经松手 → 这一按就是个普通点击，不该在松手后凭空开一次拖动
        if (!dragPressAliveRef.current) return;
        dragSessionRef.current = true;
        void beginWindowDrag();
      }, FLEE_TAKEOVER_HOLD_MS);
      return;
    }

    // 标记进入用户拖拽会话：后续 onMoved 事件将触发收伞表情
    dragSessionRef.current = true;
    void beginWindowDrag();
  }, [beginWindowDrag, startHold]);

  // 拖拽表情联动：窗口实际移动时切到 drag 格位（被拎起），松手时重置。
  // 另外接管后端 `drag:dizzy` 事件——拖动过快或甩飞撞到屏幕边缘时，
  // 按后端给出的时长临时切到 dizzy 格位（晕乎乎），到点再回落。
  useEffect(() => {
    const win = getCurrentWindow();
    let unlistenMoved: (() => void) | undefined;

    const applyDragExpression = () => {
      if (dragExpressionAppliedRef.current) return;
      dragExpressionAppliedRef.current = true;
      petRef.current?.setExpression('drag');
    };

    /** 临时晕乎乎：按住时长播放 dizzy，到点后仍在拖拽就回到「被拎起」。 */
    const applyDizzy = (durationMs: number) => {
      const hold = durationMs > 0 ? durationMs : DIZZY_FALLBACK_MS;
      petRef.current?.setExpression('dizzy', hold);
      if (dizzyTimerRef.current !== null) window.clearTimeout(dizzyTimerRef.current);
      dizzyTimerRef.current = window.setTimeout(() => {
        dizzyTimerRef.current = null;
        // 晕眩结束时人还拎着它（本次会话确实拖动过）→ 回到「被拎起」；
        // 否则交给画布自行回落 idle，不要硬切姿态。
        if (dragSessionRef.current && dragExpressionAppliedRef.current) {
          petRef.current?.setExpression('drag');
        }
      }, hold);
    };

    const resetDragExpression = () => {
      // 松手了：延后开拖的那一按作废（否则松手后还会凭空补一次拖动）
      dragPressAliveRef.current = false;
      positioningCoordinator.dragInFlight = false;
      if (dragDeferTimerRef.current !== null) {
        window.clearTimeout(dragDeferTimerRef.current);
        dragDeferTimerRef.current = null;
      }
      if (dizzyTimerRef.current !== null) {
        window.clearTimeout(dizzyTimerRef.current);
        dizzyTimerRef.current = null;
      }
      if (dragExpressionAppliedRef.current) {
        dragExpressionAppliedRef.current = false;
        petRef.current?.resetExpression();
      }
      dragSessionRef.current = false;
      // 停止自定义拖动：清除后端 DRAG_OFFSET 状态，恢复点击穿透逻辑
      void invoke('stop_window_drag').catch(() => {});
    };

    void (async () => {
      try {
        unlistenMoved = await win.onMoved(() => {
          // 仅在用户拖拽会话期间响应，过滤 useSmartPositioning 等程序性移动
          if (!dragSessionRef.current) return;
          applyDragExpression();
        });
      } catch {
        /* onMoved 不可用时跳过 */
      }
    })();

    // 后端拖动 watchdog 兜底：松手时 mouseup 因窗口追逐延迟到不了 WebView，
    // 后端轮询到左键已抬起后发此事件，前端据此重置拖动会话状态
    let unlistenDragCancelled: (() => void) | undefined;
    void (async () => {
      try {
        unlistenDragCancelled = await win.listen('drag:cancelled', () => {
          resetDragExpression();
        });
      } catch {
        /* listen 不可用时跳过 */
      }
    })();

    // 晕乎乎触发：拖动过快（拖动中）或甩飞撞到屏幕边缘（松手后）
    let unlistenDizzy: (() => void) | undefined;
    void (async () => {
      try {
        unlistenDizzy = await win.listen<{
          duration_ms?: number;
          reason?: string;
          impact?: number;
        }>('drag:dizzy', (event) => {
          applyDizzy(event.payload?.duration_ms ?? 0);
          // 「被甩懵了」是一次实打实的用户动作：除了切表情，也让桌宠反应一句
          // （后端据此记进统一事件账本，并带节流合并连续撞击）
          const reason = event.payload?.reason;
          if (reason === 'fast_drag') {
            requestPetReaction('fast_drag');
          } else if (reason === 'edge_bounce') {
            requestPetReaction('edge_bounce', event.payload?.impact);
          }
        });
      } catch {
        /* listen 不可用时跳过 */
      }
    })();

    window.addEventListener('mouseup', resetDragExpression);

    return () => {
      safeUnlisten(unlistenMoved);
      safeUnlisten(unlistenDragCancelled);
      safeUnlisten(unlistenDizzy);
      window.removeEventListener('mouseup', resetDragExpression);
      if (dragDeferTimerRef.current !== null) {
        window.clearTimeout(dragDeferTimerRef.current);
        dragDeferTimerRef.current = null;
      }
      if (dizzyTimerRef.current !== null) {
        window.clearTimeout(dizzyTimerRef.current);
        dizzyTimerRef.current = null;
      }
    };
  }, [requestPetReaction]);

  // 右键：桌宠窗口不弹菜单（统一从系统托盘菜单访问），仅拦截默认行为
  const handleContextMenu = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
  }, []);

  // 初始化 ChatController + 设置 onMeta 回调（在 text 流式之前提前播放 桌宠动画）
  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        await ChatController.init();
        if (cancelled) ChatController.cleanup();
      } catch (err) {
        console.warn('[ChatController] 初始化失败:', err);
      }
    })();
    // 设置 onMeta 回调：chat:meta 事件在 chat:chunk 之前到达
    ChatController.setHandlers({
      onMeta: (meta) => {
        // 表情持续时间：优先用 LLM 在 ExpressionMotionRunnable 中决定的 expression_duration_ms；
        // 缺省/0 时回退到 3000ms（保持原有兜底行为，避免表情卡死）
        const expressionDuration = meta.expressionDurationMs && meta.expressionDurationMs > 0
          ? meta.expressionDurationMs
          : 3000;
        if (meta.expression) petRef.current?.setExpression(meta.expression, expressionDuration);
        if (meta.motion) petRef.current?.playMotion(meta.motion);
      },
    });
    return () => {
      cancelled = true;
      ChatController.cleanup();
    };
  }, []);

  // SideChat 窗口发送消息：SideChatPanel 是独立 WebviewWindow，持有自己的
  // ChatController 单例（未 init），无法直接处理流式回复。改为 emit 事件，
  // 由主窗口统一调用 ChatController.sendMessage，走 direct 渠道。
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      unlisten = await listen<{ text: string; character_id?: string; whisper?: boolean }>(
        'sidechat:send_message',
        (e) => {
          if (e.payload?.character_id && e.payload.character_id !== getCharacterId()) return;
          const text = e.payload?.text;
          if (!text) return;
          void ChatController.sendMessage(
            text,
            e.payload?.character_id ?? getCharacterId() ?? undefined,
            'direct',
            e.payload?.whisper,
          );
        },
      );
      // 迟到 resolve 兜底：cleanup 先于监听器注册完成时，当场解绑防泄漏
      if (cancelled) { safeUnlisten(unlisten); unlisten = undefined; }
    })();
    return () => { cancelled = true; safeUnlisten(unlisten); unlisten = undefined; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 用户从 SideChat 窗口发送消息时同步活跃时间戳，保持 idle/away 检测准确
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      unlisten = await listen<{ content: string; character_id?: string }>('chat:user_message', (e) => {
        if (e.payload?.character_id && e.payload.character_id !== getCharacterId()) return;
        lastUserMessageRef.current = Date.now();
        lastActivityRef.current = Date.now();
        lastBubbleFromProactiveRef.current = 0;
      });
      if (cancelled) { safeUnlisten(unlisten); unlisten = undefined; }
    })();
    return () => { cancelled = true; safeUnlisten(unlisten); unlisten = undefined; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleQuit = useCallback(async () => {
    // 退出整个应用：先注销托盘，再调用 exit_app 让 Rust 端 app.exit(0) 关闭所有窗口并结束进程
    await invoke('destroy_tray').catch((err) => {
      console.warn('[handleQuit] 注销托盘失败:', err);
    });
    CHILD_WINDOWS.clear();
    await invoke('exit_app').catch((err) => {
      console.warn('[handleQuit] exit_app 失败:', err);
    });
  }, []);

  // 鼠标跟随模式：
  // - 'always'（实时看鼠标）：总开关开启 + 心情激活（精力充沛且专注）时
  //   ——持续注视全局光标，由桌宠心情动态进入/退出
  // - 'window'（随机张望 + 交互短跟随）：心情未激活或总开关关闭时的默认态
  // - 'off'：busy/rest/offline 时不跟随
  const presenceBlockFollow = presenceState === 'busy' || presenceState === 'rest' || presenceState === 'offline';
  const mouseFollowMode: 'always' | 'window' | 'off' = presenceBlockFollow
    ? 'off'
    : alwaysFollowMouse && moodFollowActive
      ? 'always'
      : 'window';

  const openChat = useCallback(() => {
    void ensureWechatWindow({ show: true });
  }, [ensureWechatWindow]);

  const openConfig = useCallback((showGuide = false) => {
    void openWindow('config', 'config', t('config.title'), 768, 624, {
      decorations: false,
      transparent: false,
      shadow: true,
      minWidth: 768,
      minHeight: 624,
        extraQuery: showGuide ? 'guide=1' : undefined,
    });
  }, [t]);

  /**
   * 打开（或预热）心智观察器窗口——常规打开与长按预热共用同一条入口。
   *
   *  入场动画：窗口从桌宠当前矩形「长」到全屏。两条路径共用同一个矩形：
   *  - 新建：把桌宠的物理矩形随 URL 带过去，由子窗口自己折算成 CSS px 并播放；
   *  - 已存在：不能再 navigate（会整页 reload），改由事件通知子窗口自己再播一遍。
   *  取不到位置就退化为普通显示（没有动画），不阻塞打开。
   *
   *  两条路径的「显形」都由子窗口负责（`selfReveal`）：它得先把首帧摆成桌宠大小
   *  再让窗口出现。尤其是被最小化的时候——若这里先把窗口还原出来，用户会先看到
   *  整屏复位一次、再看到它缩回桌宠长出来，等于呼出了两回。
   *
   *  `prewarmSession` 有值即预热：窗口建出来加载，但不上屏、也不挂兜底显形
   *  （见 `openWindow` 的 `prewarm`），显形等长按成立后由 `pet:reveal` 驱动。
   *  返回窗口句柄，预热会话作废时要靠它把自己建的那个销毁掉。
   */
  const openMemoryWindow = useCallback(
    (prewarmSession?: number): Promise<WebviewWindow | null> => {
      // 心智观察器默认全屏大小（CSS 逻辑像素，Tauri 窗口尺寸同单位）
      const fullW = window.screen.width;
      const fullH = window.screen.height;
      return (async () => {
        const rect = await readPetRect();
        return openWindow('memory', 'memory', t('memory.title'), fullW, fullH, {
          decorations: false,
          resizable: false,
          // 入场动画缩放期间，卡片之外必须透出桌面，故用透明窗口 + 去阴影；
          // 静止态由 .codex-theme 的纸面背景铺满整窗，与不透明窗口视觉一致。
          transparent: true,
          shadow: false,
          selfReveal: true,
          minWidth: 1260,
          minHeight: 896,
          prewarm: prewarmSession !== undefined,
          extraQuery:
            prewarmSession !== undefined
              ? buildPrewarmQuery(rect, prewarmSession)
              : rect
                ? buildPetRectQuery(rect)
                : undefined,
          // 已打开时：提到前台后再从桌宠位置重播一次入场，观感与首开一致。
          // 预热不传：此刻显形与否还没定，通知子窗口播入场等于让它上屏。
          onExisting:
            prewarmSession === undefined && rect
              ? () => { void emitPetReveal('memory', rect); }
              : undefined,
        });
      })();
    },
    [t],
  );

  const openMemory = useCallback(() => {
    void openMemoryWindow();
  }, [openMemoryWindow]);

  /** 长按桌宠触发的动作：心智观察器的**开关**。
   *
   *  它此刻已经开在屏幕上（可见且未最小化）→ 长按是「收起来」，直接最小化；
   *  其余情况（没开过 / 已被最小化 / 被 hide）都走 `openMemory`：打开、提到前台、播入场动画。
   *
   *  判据为什么是「在不在屏上」而不是窗口焦点：按下桌宠那一刻焦点就被桌宠窗口抢走了，
   *  子窗口的失焦回调还会顺手把它降回非置顶；等 1 秒长按成立时它早已不是前台窗口，
   *  按焦点判断这个动作根本触发不了。
   *
   *  托盘菜单与全局快捷键不走这里——它们要的是明确的「打开」，不该变成开关。 */
  const toggleMemory = useCallback(() => {
    void (async () => {
      let win: WebviewWindow | null = null;
      try {
        win = await WebviewWindow.getByLabel(charScopedLabel('memory'));
      } catch {
        win = null;
      }
      if (win && (await isWindowOnScreen(win))) {
        await win.minimize().catch(() => {});
        return;
      }
      openMemory();
    })();
  }, [openMemory]);

  // 长按桌宠触发的动作：开关心智观察器（与托盘菜单、快捷键共用 openMemory）
  holdActionRef.current = toggleMemory;

  /**
   * 长按预热：长按 0.1s 时开一场会话，把心智观察器窗口提前建出来加载。
   *
   *  窗口加载（WebView 冷启 + React 挂载 + 首屏数据）是这条链上最慢的一环，而长按
   *  判定要整整 1s。提前起步，绝大多数情况下窗口在长按成立时已经就绪，松手即见。
   *
   *  三条铁律，缺一都会让「预热」变成「误开」：
   *  1. 预热期间窗口一律不上屏：URL 带 `hidden=1` 关掉 main.tsx 的兜底 show，
   *     子窗口收到预热参数后也只藏不播，兜底显形更是不挂（见 openWindow 的 prewarm）。
   *  2. 长按成立才放行：显形由 `pet:reveal` 驱动，且要等子窗口回执就绪再发
   *     （见 releasePrewarm），否则事件打在空处、窗口迟到。
   *  3. 长按中止就销毁：见 cancelHold → abortPrewarmSession。加载没跑完的也一并打断。
   *
   *  两种情况不预热：窗口已经开在屏上（长按是「收起」，没有加载可提前）、
   *  窗口已存在但被最小化/hide（内容早就加载完了，预热无事可做）。
   */
  const prewarmMemory = useCallback(() => {
    // 会话号加随机基数：memory 是共享窗口，两个角色桌宠各有一场预热时，
    // 纯自增会让两边的首个会话号撞车，回执就分不清是谁的。
    if (prewarmSeqRef.current === 0) prewarmSeqRef.current = Math.floor(Math.random() * 1e6);
    const session: MemoryPrewarmSession = {
      session: ++prewarmSeqRef.current,
      chain: Promise.resolve(),
      win: null,
      childReady: false,
      committed: false,
      aborted: false,
      readyTimer: null,
    };
    prewarmRef.current = session;

    session.chain = (async () => {
      // 已有实例？在屏上 → 长按是「收起」；不在屏上 → 内容早已加载完。
      // 两种都没有「加载」可提前，但后者要记下就绪，好让放行时的显形事件必被接住。
      let existing: WebviewWindow | null = null;
      try {
        existing = await WebviewWindow.getByLabel(charScopedLabel('memory'));
      } catch {
        existing = null;
      }
      if (session.aborted) return;
      if (existing) {
        if (await isWindowOnScreen(existing)) {
          // 长按会把它收起来，没有可预热的东西 → 撤掉会话，让放行走常规开关路径
          if (prewarmRef.current === session) prewarmRef.current = null;
          return;
        }
        session.childReady = true;
        return;
      }
      // 还没有这个窗口 → 建一个隐藏的。句柄回填到会话上，作废时要靠它销毁。
      const win = await openMemoryWindow(session.session);
      if (session.aborted) {
        // 长按在创建期间就中止了：这一下不销毁的话，窗口会永远留在后台
        if (win) await destroyPrewarmedWindow(win);
        return;
      }
      session.win = win;
    })();
  }, [openMemoryWindow]);

  // 长按 0.1s 触发的动作：提前加载心智观察器（见 startHold 的预热定时器）
  prewarmActionRef.current = prewarmMemory;

  // 3D 公寓窗口：固定屏幕尺寸 + 无边框 + 透明背景。
  // 与心智观察器共用 utils/roomWindow 的入口，保证两处打开的是同一个实例。
  // 触发来源：Rust 侧全局快捷键（base.shortcut_room，默认 Ctrl+Shift+R）emit
  // "window:shortcut" 的 room 分支，以及心智观察器的「进入公寓」按钮。
  const openRoom = useCallback(() => {
    void openRoomWindow(t('room.title', { defaultValue: '公寓' }));
  }, [t]);

  // 窗口快捷键：后端 emit "window:shortcut" 事件，前端根据 action 打开对应窗口
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      unlisten = await listen<{ action: string }>('window:shortcut', (e) => {
        switch (e.payload.action) {
          case 'chat':
            openChat();
            break;
          case 'settings':
            openConfig();
            break;
          case 'memory':
            openMemory();
            break;
          case 'room':
            openRoom();
            break;
        }
      });
      // 迟到 resolve 兜底：cleanup 先于监听器注册完成时，当场解绑防泄漏
      if (cancelled) { safeUnlisten(unlisten); unlisten = undefined; }
    })();
    return () => { cancelled = true; safeUnlisten(unlisten); unlisten = undefined; };
  }, [openChat, openConfig, openMemory, openRoom]);

  // 后端启动预检未通过时，打开设置窗口并展示配置说明
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      unlisten = await listen('setup-guide:show', () => {
        openConfig(true);
      });
      if (cancelled) { safeUnlisten(unlisten); unlisten = undefined; }
    })();
    return () => { cancelled = true; safeUnlisten(unlisten); unlisten = undefined; };
  }, [openConfig]);

  // Ctrl+滚轮缩放：100ms debounce + rAF 帧同步
  //
  // 闪烁根因：SetWindowPos 触发 DWM 立即更新窗口几何，但 WebView2 内部
  // canvas 纹理还是旧尺寸，透明背景下表现为短暂闪烁。连续滚动时每一帧都
  // 暴露中间态，累积成明显闪烁。
  //
  // 解决方案：
  // 1. 100ms debounce 累积滚动事件，仅在滚动停止后执行一次 resize，
  //    避免连续滚动期间每帧都产生中间态
  // 2. 后端 set_window_rect 用 SWP_NOREDRAW 延迟重绘，resize 完成后
  //    立即 RedrawWindow 强制同步重绘，让几何更新与纹理更新落在同一帧
  // 3. resize 完成后前端同步触发 app.renderer.resize + fitModel，
  //    不等待 Tauri 的 resize 事件（避免跨帧延迟）
  const handleScaleChange = useCallback((scale: number) => {
    const base = baseWindowSizeRef.current;
    if (base.w <= 0 || base.h <= 0) return;
    // 同步更新目标值
    targetScaleRef.current = scale;
    windowScaleRef.current = scale;

    // 滚动停止 150ms 后清除中心缓存，下次滚动重新读取
    if (scaleIdleTimerRef.current) clearTimeout(scaleIdleTimerRef.current);
    scaleIdleTimerRef.current = setTimeout(() => {
      scaleCenterRef.current = null;
      scaleIdleTimerRef.current = null;
    }, 150);

    // debounce：100ms 内有新滚动事件则重置计时器，仅最后一次滚动后执行 resize
    if (scaleDebounceRef.current) clearTimeout(scaleDebounceRef.current);
    scaleDebounceRef.current = setTimeout(() => {
      scaleDebounceRef.current = null;

      const b = baseWindowSizeRef.current;
      if (b.w <= 0 || b.h <= 0) return;
      const s = targetScaleRef.current;

      const applyResize = () => {
        const cur = scaleCenterRef.current;
        if (!cur) return;
        const { cx, cy, factor } = cur;
        const newW = Math.max(150, Math.round(b.w * s * factor));
        const newH = Math.max(150, Math.round(b.h * s * factor));
        void invoke('set_window_rect', {
          x: Math.round(cx - newW / 2),
          y: Math.round(cy - newH / 2),
          width: newW,
          height: newH,
        }).catch(() => { /* IPC 失败时静默 */ });
      };

      // 首次迭代缓存窗口中心，后续复用避免位置漂移
      if (!scaleCenterRef.current) {
        const win = getCurrentWindow();
        void Promise.all([
          win.outerPosition(),
          win.outerSize(),
          win.scaleFactor(),
        ]).then(([pos, size, factor]) => {
          scaleCenterRef.current = {
            cx: pos.x + size.width / 2,
            cy: pos.y + size.height / 2,
            factor,
          };
          applyResize();
        }).catch(() => { /* 窗口已销毁 */ });
      } else {
        applyResize();
      }
    }, 100);
  }, []);

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        overflow: 'hidden',
        background: 'transparent',
      }}
      // 拖拽必须在捕获阶段开始：对话期间画布会切换动作层，个别动作格/覆盖层会
      // 消费冒泡阶段的鼠标事件。若仍只在 onMouseDown（冒泡）绑定，桌宠说过话后就会
      // 偶发无法再拖动。捕获阶段保证窗口拖拽不依赖子组件是否 stopPropagation。
      onMouseDownCapture={handleBackgroundMouseDown}
      onContextMenu={handleContextMenu}
    >
      {/* 文件拖放文字提示（简化：仅文字，无蓝色遮罩） */}
      {isDragOver && (
        <div
          style={{
            position: 'absolute',
            inset: 0,
            zIndex: 9999,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            pointerEvents: 'none',
            color: 'rgba(255, 255, 255, 0.9)',
            fontSize: '14px',
            textShadow: '0 1px 4px rgba(0,0,0,0.6)',
            fontWeight: 600,
          }}
        >
          {t('ui.drop_file_hint', { defaultValue: '松开以发送文件' })}
        </div>
      )}
      {/* 长按桌宠的环形进度槽：按住超 0.2s 后显示，顺时针填满（0.8s）即打开心智观察器 */}
      {holdRingPos && (
        <HoldProgressRing
          x={holdRingPos.x}
          y={holdRingPos.y}
          durationMs={HOLD_OPEN_TOTAL_MS - HOLD_RING_DELAY_MS}
          onComplete={completeHold}
        />
      )}

      {/* 系统托盘右键菜单事件路由（不渲染 UI，监听 tray:menu_action 事件）
          即使两个角色都 Offline、桌宠窗口被 hide_window 隐藏，
          托盘菜单仍可访问所有子窗口入口（记忆/设置/微信），
          也可通过「微信」入口发消息唤醒离线智能体。
          与系统托盘菜单共用同一组 openXxx / toggleXxx 回调。 */}
      <SystemTray
        onOpenMemory={openMemory}
        onOpenSettings={openConfig}
        onOpenChat={openChat}
        onToggleVoice={() => {
          // 后端 TTS 未启用：弹 toast 提示前往设置开启
          if (!ttsEnabled) {
            showToast(t('toast.voice_disabled_hint'), 'warning', 5000);
            return;
          }
          const next = !voiceEnabled;
          useAppStore.getState().setVoiceEnabled(next);
          // 同步 TtsStreamQueue 启用状态，并停止正在播放的 TTS
          TtsStreamQueue.setEnabled(!!ttsConfigRef.current?.enabled && next);
          if (!next) void TtsStreamQueue.stop();
        }}
        onToggleSmartPositioning={() => {
          const next = !smartPositioningEnabled;
          setSmartPositioningEnabled(next);
          void configApi
            .set('window.smart_positioning_enabled', next)
            .then(() => configApi.save())
            .then(() => emit('config:saved', {}))
            .catch(() => {
              /* ignore */
            });
        }}
        onQuit={() => void handleQuit()}
      />

      {/* 托盘菜单勾选状态同步：voiceEnabled / ttsEnabled / smartPositioningEnabled 变化时
          通知后端更新原生 CheckMenuItem 的勾选标记。
          多角色窗口都会同步，最后一次写入覆盖前面，无害（store 全局共享同一值）。 */}
      <TrayCheckSync
        voiceChecked={ttsEnabled && voiceEnabled}
        smartPositioningChecked={smartPositioningEnabled}
      />

      {/* Q 版桌宠主内容（透明窗口；智能避让与窗口定位仍沿用原后端） */}
      <div
        style={{
          position: 'absolute',
          inset: 0,
          // 视频动画演出层播放期间隐藏 Q 版本体，避免两个角色同时出现
          opacity: videoActive ? 0 : 1,
          transition: 'opacity .25s ease',
        }}
      >
        <ModelCanvas
          ref={petRef}
          mouseFollowMode={mouseFollowMode}
          onScaleChange={handleScaleChange}
          ambientMotionEnabled={presenceState === 'online'}
          onInteraction={handleChibiInteraction}
          onOpenQuickChat={() => {
            lastActivityRef.current = Date.now();
            lastBubbleFromProactiveRef.current = 0;
            void ensureSideChatWindow({ show: true, showInput: true });
          }}
          onReady={() => {
            setModelReady(true);
            // 按角色模型画布比例设置窗口尺寸
            const showMainWindow = () => { void getCurrentWindow().show().catch(() => {}); };
            const winSize = getWindowSize(getCharacterId());
            baseWindowSizeRef.current = winSize;
            windowScaleRef.current = 1.0;
            void getCurrentWindow()
              .setSize(new LogicalSize(winSize.w, winSize.h))
              .then(() => {
                petRef.current?.refitModel();
                showMainWindow();
                // 点击穿透由 start_cursor_tracking 线程负责：
                // 鼠标在中心 1/3 宽 × 4/9 高矩形外时自动 set_ignore_cursor_events(true)
                // ChibiPetCanvas 初始化时已 invoke('start_cursor_tracking')，无需额外调用
              })
              .catch(() => { showMainWindow(); });
          }}
          onModelClick={() => {
            lastActivityRef.current = Date.now();
            lastBubbleFromProactiveRef.current = 0;

            // 从休息/忙碌状态唤醒：rest 需 3 次连续点击，busy 1 次即唤醒
            const presence = presenceState;
            if (presence === 'rest' || presence === 'busy') {
              const now = Date.now();
              const tracker = wakeClickRef.current;
              // 800ms 时间窗口内累加，否则重置
              if (now - tracker.lastTime > 800) {
                tracker.count = 0;
              }
              tracker.count += 1;
              tracker.lastTime = now;

              const threshold = presence === 'rest' ? 3 : 1;
              if (tracker.count >= threshold) {
                tracker.count = 0;
                tracker.lastTime = 0;
                void ChatController.triggerWakeInteraction(getCharacterId() ?? undefined);
              }
            }
          }}
        />
      </div>

      {/* 视频动画演出层：监听 video:animation 事件播放透明动画素材（缺素材优雅跳过） */}
      <VideoAnimationLayer
        characterId={getCharacterId() ?? undefined}
        onActiveChange={setVideoActive}
      />

      {/* 角落感知按钮 —— 隐藏到角落时显示，悬停可见，点击召回桌宠 + 唤醒睡眠 */}
      {hiddenCorner && (
        <PeekButton
          corner={hiddenCorner}
          onClick={() => {
            // 退出隐藏到角落模式（全屏隐藏 / 睡眠隐藏均生效）
            // 捕获唤醒前的隐藏原因，requestRestore 会清空 hideReason
            const wasSleep = hideReason === 'sleep';
            requestRestore();
            // 若因休息隐藏，切换 Presence 回 Online + 尝试生成唤醒问候
            if (wasSleep) {
              void invoke('set_presence_state', { target: 'online', characterId: getCharacterId() ?? undefined }).catch(() => {
                /* 后端未就绪忽略 */
              });
              void triggerWakeGreeting();
            }
          }}
        />
      )}
    </div>
  );
}

/* ============ 托盘菜单勾选状态同步组件 ============ */

/** 把前端的 voice / smart_positioning 勾选状态同步到后端原生 CheckMenuItem */
function TrayCheckSync({
  voiceChecked,
  smartPositioningChecked,
}: {
  voiceChecked: boolean;
  smartPositioningChecked: boolean;
}) {
  useEffect(() => {
    void syncTrayMenuCheck('voice', voiceChecked);
  }, [voiceChecked]);
  useEffect(() => {
    void syncTrayMenuCheck('smart_positioning', smartPositioningChecked);
  }, [smartPositioningChecked]);
  return null;
}

/* ============ 角落感知按钮（全屏隐藏时显示，悬停可见，点击召回）============ */

/**
 * 屏幕角落 → 按钮在窗口内的定位。
 *
 * 桌宠隐藏到屏幕角落时，窗口只有对角的 48×48 区域可见：
 * - 屏幕右下角 → 窗口左上角可见
 * - 屏幕左下角 → 窗口右上角可见
 * - 屏幕右上角 → 窗口左下角可见
 * - 屏幕左上角 → 窗口右下角可见
 */
const PEEK_BUTTON_POSITION: Record<Corner, React.CSSProperties> = {
  br: { top: 0, left: 0 },
  bl: { top: 0, right: 0 },
  tr: { bottom: 0, left: 0 },
  tl: { bottom: 0, right: 0 },
};

/**
 * 屏幕角落 → 箭头旋转角度（基础箭头指向上方 ↑）。
 * 箭头指向屏幕中心，提示用户"点击此处可将桌宠召回"。
 */
const PEEK_ARROW_ROTATION: Record<Corner, number> = {
  br: 315, // ↖ 屏幕右下角 → 指向左上
  bl: 45,  // ↗ 屏幕左下角 → 指向右上
  tr: 225, // ↙ 屏幕右上角 → 指向左下
  tl: 135, // ↘ 屏幕左上角 → 指向右下
};

const PeekButton: React.FC<{ corner: Corner; onClick: () => void }> = ({ corner, onClick }) => {
  const [hovered, setHovered] = useState(false);
  return (
    <button
      type="button"
      aria-label="召回桌宠"
      onClick={(e) => {
        e.stopPropagation();
        onClick();
      }}
      // 阻止 mousedown 冒泡到背景层，避免触发 startDragging 拖拽窗口
      onMouseDown={(e) => e.stopPropagation()}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        position: 'absolute',
        width: 48,
        height: 48,
        padding: 0,
        border: 'none',
        background: 'transparent',
        cursor: 'pointer',
        zIndex: 1000,
        opacity: hovered ? 1 : 0,
        transition: 'opacity 0.18s ease',
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        ...PEEK_BUTTON_POSITION[corner],
      }}
    >
      <div
        style={{
          width: 36,
          height: 36,
          borderRadius: 10,
          background: 'rgba(40, 40, 50, 0.78)',
          border: '1px solid rgba(255, 255, 255, 0.14)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          color: 'rgba(255, 255, 255, 0.92)',
          boxShadow: '0 4px 14px rgba(0, 0, 0, 0.4)',
        }}
      >
        <svg
          width="18"
          height="18"
          viewBox="0 0 24 24"
          fill="none"
          style={{
            transform: `rotate(${PEEK_ARROW_ROTATION[corner]}deg)`,
            transition: 'transform 0.2s ease',
          }}
        >
          {/* 基础箭头指向上方，通过 rotate 旋转到对应方向 */}
          <path
            d="M12 5L12 19M12 5L6 11M12 5L18 11"
            stroke="currentColor"
            strokeWidth="2.2"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </div>
    </button>
  );
};
