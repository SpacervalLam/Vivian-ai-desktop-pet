/**
 * Toast 子窗口 - 在屏幕右下角的独立透明窗口中渲染 Toast 通知与工具确认卡片。
 *
 * 主窗口通过 Tauri 事件驱动本窗口：
 * - `toast:show` 显示一条 toast（payload: { message, type?, duration?, key }）
 * - `toast:confirm` 显示一个工具执行确认卡片（三按钮：拒绝/放行一次/始终允许）
 * - `toast:confirm_done` 某确认已被响应，移除同 request_id 的卡片（覆盖多窗口场景）
 *
 * 本窗口自身透明、无边框、跳过任务栏、始终置顶，默认点击穿透；
 * 存在确认卡片时关闭点击穿透以接收按钮点击，卡片全部清除后恢复穿透。
 * 窗口内有 toast 或确认卡片时显示，全部关闭后隐藏窗口以彻底让出屏幕。
 *
 * 窗口高度固定为屏幕的一半：内容在这个**确定容量**里自底向上堆叠。堆叠位置由
 * 「距底偏移」写成 transform 而非交给 flex 重排，因此任何一次增删都表现为整体
 * 平滑滑动而不是一帧跳变；容量不足时先把最老的条目平滑请出，腾干净了再放行新
 * 条目入场——顺序是「先出后进」，顶部条目不会越界被窗口边界裁掉。
 */

import React, { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { listen, emit } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow, currentMonitor, LogicalPosition, LogicalSize } from '@tauri-apps/api/window';
import { open as shellOpen } from '@tauri-apps/plugin-shell';
import { stackRank, toastFingerprint, ToastDedupGate, ToastKeyTracker } from '../utils/toastDedup';
import Toast, { type ToastAction, type ToastType } from './Toast';
import ConfirmToast, {
  type AllowAlwaysScope,
  type ConfirmRiskLevel,
} from './ConfirmToast';

/**
 * Toast 在堆叠里的生命周期阶段。
 *
 * - `queued`：已收到请求，但当前空间不够，排在队首等待放行（不渲染）
 * - `live`：已入场
 * - `exiting`：被容量管理判定淘汰，正在播退场动画
 */
type ToastPhase = 'queued' | 'live' | 'exiting';

interface ToastItem {
  id: number;
  key?: number;
  /** 启动进度条目标识：整段启动进度共用 STARTUP_TOAST_KEY 单一条目 */
  taskKey?: string;
  message: string;
  type: ToastType;
  duration: number;
  progress?: number;
  /** 附带的一键操作（如主题切换确认按钮） */
  action?: ToastAction;
  /**
   * 是否是「原地刷新」条目（同一 key 会被反复喂新文案的进度条）。
   *
   * 只有它为 true 的条目才豁免内容去重——判据来自 `ToastKeyTracker` 的观察结果，
   * 而不是"payload 里有没有 key"（那条判据会被 `key: Date.now()` 这种一次性 key 击穿）。
   */
  inplace: boolean;
  phase: ToastPhase;
}

interface ToastShowPayload {
  message: string;
  type?: ToastType;
  duration?: number;
  key: number;
  character_id?: string;
  progress?: number;
  action?: ToastAction;
}

interface ConfirmItem {
  requestId: number;
  tool: string;
  reason: string;
  riskLevel: ConfirmRiskLevel;
  allowAlwaysScope: AllowAlwaysScope;
  charId: string;
  args?: unknown;
}

interface ToastConfirmPayload {
  request_id: number;
  tool: string;
  arguments: unknown;
  reason: string;
  risk_level: ConfirmRiskLevel;
  char_id: string;
  allow_always_scope: AllowAlwaysScope;
}

let nextId = 1;

/** 嵌入初始化进度 toast 的固定 key */
const EMBEDDING_TOAST_KEY = 99001;
/**
 * 启动进度 toast 的固定 key。
 *
 * 整段启动进度只占用这一条固定条目：阶段 emit、周期重发、挂载时的状态快照
 * 都只是"更新同一条 toast"，阶段变化即文本变化。不再按 stage 文本派生任务
 * 标识去推断"上一阶段那条 toast 是谁"——那种推断要求游标与界面内容时刻一致，
 * 而游标有多个写入点、事件到达顺序又不保证，一旦被覆盖就会留下无人认领的
 * 持久条目（duration 0，窗口再也不会隐藏）。
 */
const STARTUP_TOAST_KEY = '__startup__';
/** 启动进度应用节流窗口（毫秒）：进度刷新高频到时合并应用，保持文本可读 */
const STARTUP_THROTTLE_MS = 400;
/** 进度事件停滞多久后查询后端终态（毫秒）：兜住"完成事件丢失"的场景 */
const STARTUP_STALL_TIMEOUT_MS = 3000;

/** 窗口内容四周留白；跨窗口堆叠偏移在此基础上追加 */
const WINDOW_PADDING = 20;

/** 堆叠条目之间的间隙（像素） */
const STACK_GAP = 10;
/** 退场动画时长（毫秒）：到点即把条目从列表移除，需覆盖 StackSlot 的过渡时长 */
const EXIT_DURATION = 300;
/** 位移/入场过渡时长（毫秒） */
const MOVE_DURATION = 320;

/** 堆叠中的一个可视条目（把 toast 与确认卡片统一到同一套排布逻辑里） */
interface StackEntry {
  /** 稳定标识：确认卡 `c-<requestId>`，toast `t-<id>` */
  id: string;
  /** 是否计入容量（退场中的条目已让出位置，不再占位） */
  live: boolean;
  /** 是否允许因容量不足被淘汰 */
  evictable: boolean;
  /** 确认卡撑满容器宽度，toast 保持自适应宽度 */
  wide: boolean;
  /**
   * 是否需要鼠标：确认卡、带操作按钮的 toast。
   *
   * 只有这些条目的矩形会上报给后端作为"可接收鼠标"的区域，窗口其余部分（含普通 toast
   * 之间的空隙）一律穿透——那片透明区域压着桌面、任务栏和其他窗口，不能吃掉点击。
   */
  interactive: boolean;
  node: ReactNode;
}

/**
 * 估算一条 toast 渲染后的高度（像素），用于在它真正入场前判断能否放下。
 *
 * 偏保守（每行按较少字符算）：估高了至多多淘汰一条；估低了则会先入场、再被容量
 * 修正补淘汰一次——后者是一次视觉抖动，所以宁高不低。真入场后由实测高度接管。
 */
function estimateToastHeight(message: string): number {
  const CHARS_PER_LINE = 22;
  const LINE_HEIGHT = 20.2;
  const lines = message.split('\n').reduce(
    (sum, seg) => sum + Math.max(1, Math.ceil(seg.length / CHARS_PER_LINE)),
    0,
  );
  return Math.round(24 + lines * LINE_HEIGHT);
}

/**
 * 挑出要为 `need` 腾地方的条目，自栈顶（最老）向下取。
 *
 * 至少保留最底部一条：若把它也让掉，极端情况下（可用空间本就小于一条 toast）
 * 会把屏幕清空，反而比顶部被裁更糟。没有可让的条目时返回空数组——调用方据此
 * 直接放行，而不是把新 toast 永久卡在队列里。
 */
function pickVictims(
  entries: StackEntry[],
  heights: Map<string, number>,
  available: number,
  need: number,
): string[] {
  const victims: string[] = [];
  let cur = need;
  for (let i = 0; i < entries.length - 1 && cur > available; i++) {
    if (!entries[i].evictable) continue;
    cur -= (heights.get(entries[i].id) ?? 0) + STACK_GAP;
    victims.push(entries[i].id);
  }
  return victims;
}

/**
 * 堆叠中的一个位置单元。
 *
 * 所有条目的 `bottom` 都一致抵底，垂直距离写进 transform——位移走 CSS 过渡，
 * 增删条目时下方的一整套内容才会平滑滑动，而不是重排造成的一帧跳变。
 * 退场时冻结自身偏移：它已退出容量计算，若不冻结就会跟着下面那批一起沉下去。
 */
const StackSlot: React.FC<{
  entryId: string;
  offset: number;
  exiting: boolean;
  wide: boolean;
  onHeight: (id: string, height: number) => void;
  onExited: (id: string) => void;
  onNode: (id: string, el: HTMLDivElement | null) => void;
  children: ReactNode;
}> = ({ entryId, offset, exiting, wide, onHeight, onExited, onNode, children }) => {
  const ref = useRef<HTMLDivElement | null>(null);
  const frozenOffset = useRef(offset);
  if (!exiting) frozenOffset.current = offset;

  // 元素本体同时交给父级：上报命中矩形时要读它的布局盒
  const setRef = useCallback(
    (el: HTMLDivElement | null) => {
      ref.current = el;
      onNode(entryId, el);
    },
    [entryId, onNode],
  );

  // 实测高度上报：容量决策依赖它，容不得估算误差长期积累
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const report = () => onHeight(entryId, el.offsetHeight);
    report();
    const ro = new ResizeObserver(report);
    ro.observe(el);
    return () => {
      ro.disconnect();
      onHeight(entryId, 0);
    };
  }, [entryId, onHeight]);

  useEffect(() => {
    if (!exiting) return;
    const timer = window.setTimeout(() => onExited(entryId), EXIT_DURATION);
    return () => window.clearTimeout(timer);
  }, [exiting, entryId, onExited]);

  const y = exiting ? frozenOffset.current : offset;
  return (
    <div
      ref={setRef}
      style={{
        position: 'absolute',
        bottom: 0,
        // 不指定 left 时条目按内容自适应宽度贴右（toast）；确认卡显式占满整列
        ...(wide ? { left: 0, right: 0 } : { right: 0 }),
        pointerEvents: 'none',
        transform: exiting
          ? `translate3d(32px, ${-y}px, 0) scale(0.94)`
          : `translate3d(0, ${-y}px, 0) scale(1)`,
        opacity: exiting ? 0 : 1,
        transition: `transform ${MOVE_DURATION}ms cubic-bezier(0.16, 1, 0.3, 1), opacity 220ms ease`,
      }}
    >
      {children}
    </div>
  );
};

export default function ToastWindow() {
  // 子窗口身份：startup 专用窗口只展示启动进度。它与角色 toast 锚定同一位置
  // （右下角、半屏高），纵向错开交给下面的跨窗口堆叠协议，不再靠"一个右上一个右下"回避重叠
  const myCharId = new URLSearchParams(window.location.search).get('character_id') ?? '';

  /**
   * 主角色 ID：无归属 toast 的呈现者。取活跃角色，但它必须在线（见 resolvePrimary）。
   *
   * 按归属过滤能消灭「有归属的事件被每个角色窗口各弹一次」，但有一类事件天生
   * 没有归属：共享子窗口（chat/config/memory/input）不绑定角色，getCharacterId()
   * 恒为 null；全局后台事件（embedding:progress、记忆向量重建）也不隶属任何角色。
   * 若放任它们广播，同一条文案仍会在每只桌宠的 toast 窗口各弹一次——正是归属
   * 过滤要消灭的现象。
   *
   * 故无归属统一收敛到主角色窗口：既不重复，也不像严格丢弃那样把全局错误吞掉。
   * 用 ref 而非 state：监听器在 [] 依赖下注册，需要在事件到达时读到最新值，
   * 且主角色切换不应触发本窗口重渲染。
   */
  const primaryCharIdRef = useRef('');
  /**
   * 主角色是否已解析完成。未完成时无归属 toast 一律放行——
   * 解析延迟或失败时宁可短暂重复，也不能把消息静默丢掉。
   */
  const primaryResolvedRef = useRef(false);
  /**
   * 无归属 toast 的呈现权判定：只有主角色的窗口呈现。
   *
   * 注意这里与「有归属」分支是互补的：有归属的事件永远按角色精确路由，
   * 不受主角色是谁影响；只有没人认领的事件才落到主角色窗口。
   */
  const canRenderUnowned = () =>
    !primaryResolvedRef.current || primaryCharIdRef.current === myCharId;

  /**
   * 内容去重闸门（与来源无关的兜底）。
   *
   * 归属路由解决的是「已知的广播事件」，但只要有人往广播监听里写一句不带归属的
   * showToast、或某天新增事件忘了加守卫，重复就会重新出现。闸门让
   * 「同一内容在屏幕上只存在一条」成为不依赖发送侧行为的硬约束：
   * 本窗口重复收到同一内容不再追加；另一窗口已认领且优先级更高时本窗口让位。
   *
   * 判定规则与优先级全序定义在 utils/toastDedup.ts，可脱离界面单独验证。
   */
  const dedupGateRef = useRef(new ToastDedupGate(myCharId));

  /**
   * key 重复出现的追踪器：判定某次到达是「原地刷新」还是「一次性提示」。
   *
   * 这个判据是去重网能否生效的关键。曾经用 `typeof key === 'number'` 当代理——结果是
   * 一次性提示普遍自带的 `key: Date.now()` 把整张去重网关掉了：同一条文案在每个角色的
   * toast 窗口各弹一条。详见 utils/toastDedup.ts 里 ToastKeyTracker 的注释。
   */
  const keyTrackerRef = useRef(new ToastKeyTracker());

  const [items, setItems] = useState<ToastItem[]>([]);
  const [confirms, setConfirms] = useState<ConfirmItem[]>([]);
  /**
   * 每个堆叠条目的实测高度。放 ref 而非 state：高度变化来自 ResizeObserver，
   * 只参与布局计算，不该让整棵子树重渲染；需要依据它重算时 bump layoutTick。
   */
  const heightsRef = useRef<Map<string, number>>(new Map());
  const [layoutTick, setLayoutTick] = useState(0);
  /**
   * 视口高度，即窗口高度（二者恒等）。
   *
   * 用它而不是再去问显示器：窗口几何一旦固定为半屏高，可供堆叠的空间就是一个
   * 确定的量，且天然跟随窗口的真实尺寸漂移。
   */
  const [viewportH, setViewportH] = useState(0);
  /** 上一次算出的每个条目距底偏移：退场条目据此冻结自己的位置 */
  const lastOffsetRef = useRef<Map<string, number>>(new Map());
  /** 堆叠容器：命中矩形要按它的布局盒定位（容器自身不带 transform，读数可信） */
  const stackContainerRef = useRef<HTMLDivElement>(null);
  /** 条目 id → 元素本体，用于取布局盒上报命中矩形 */
  const nodesRef = useRef<Map<string, HTMLDivElement>>(new Map());

  // 跨窗口堆叠协调：各窗口把自己的占用高度广播出来，据此把内容顶到下方的窗口之上。
  // 单靠窗口定位做不到——每个窗口里有多少内容只有它自己的前端知道。
  const [stackHeights, setStackHeights] = useState<Record<string, number>>({});
  const myRank = stackRank(myCharId);
  const bottomOffset = Object.entries(stackHeights).reduce(
    (sum, [id, h]) => (id !== myCharId && stackRank(id) < myRank ? sum + h : sum),
    0,
  );

  // 主题：读取 base.theme 配置设置根节点 data-theme，并监听实时变更
  useEffect(() => {
    const applyTheme = (theme: string | null | undefined) => {
      document.documentElement.setAttribute('data-theme', theme === 'light' || theme === 'dark' ? theme : 'system');
    };
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
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    let cancelled = false;
    /** 启动进度停滞看门狗计时器 */
    let stallTimer: number | null = null;
    const unlistens: Array<() => void> = [];

    /**
     * 解析「主角色」——无归属 toast 的呈现者。
     *
     * 取活跃角色，但它必须是在线角色：桌宠窗口只为在线角色创建（lib.rs 按 online
     * 过滤后建窗），窗口不存在就没人能呈现。活跃角色与在线状态可能不一致
     * （characters.active_id 可以指向一个 default_online=false 的角色），
     * 此时退到第一个在线角色兜底——否则无归属 toast 会彻底无人认领、静默消失，
     * 那正是本机制要避免的丢信息。
     */
    const resolvePrimary = async () => {
      const res = await invoke<{
        active_id?: string;
        characters?: Array<{ id: string; online: boolean }>;
      }>('list_characters');
      const online = (res?.characters ?? []).filter((c) => c.online).map((c) => c.id);
      const active = res?.active_id ?? '';
      primaryCharIdRef.current = online.includes(active) ? active : (online[0] ?? '');
      // 一个在线角色都没有时保持未解析态：此时也没有任何 toast 窗口存在，
      // 放行与否都不会产生重复。
      primaryResolvedRef.current = primaryCharIdRef.current !== '';
    };

    // 启动进度节流状态：进度刷新高频时合并应用。
    // - 去重：与最近已应用载荷一致的周期重发直接丢弃，不触发重渲染
    // - 节流：窗口期内只暂存最新载荷，到期一次性应用
    // - 完成态旁路：current ≥ total 立即生效，保证收尾提示及时出现
    const startupThrottle: {
      lastPayload: { current: number; total: number; stage: string } | null;
      lastAppliedAt: number;
      timer: number | null;
      pending: { current: number; total: number; stage: string } | null;
    } = { lastPayload: null, lastAppliedAt: 0, timer: null, pending: null };

    // 启动进度是单一条目：进度刷新、阶段切换、周期重发、挂载快照都只更新
    // taskKey === STARTUP_TOAST_KEY 的那一条。阶段变化即文本变化，因此不存在
    // "上一条 toast 该由谁收尾"的推断，也就不可能留下无人认领的持久条目。
    const applyStartupProgress = (p: { current: number; total: number; stage: string }) => {
      const pct = Math.round((p.current / Math.max(p.total, 1)) * 100);
      const done = p.current >= p.total;
      startupThrottle.lastPayload = p;
      startupThrottle.lastAppliedAt = Date.now();

      setItems((prev) => {
        const idx = prev.findIndex((it) => it.taskKey === STARTUP_TOAST_KEY);
        if (idx < 0) {
          // 从未展示过启动进度的窗口（挂载时启动已收尾）不补弹提示
          if (done) return prev;
          return [
            ...prev,
            {
              id: nextId++,
              taskKey: STARTUP_TOAST_KEY,
              message: `${p.stage} ${pct}%`,
              type: 'info' as ToastType,
              duration: 0,
              progress: pct,
              // 整段启动进度共用这一条，后续事件都是刷新它 → 不参与内容去重
              inplace: true,
              phase: 'queued' as ToastPhase,
            },
          ];
        }
        const next = [...prev];
        next[idx] = {
          ...next[idx],
          message: done ? (p.stage || '启动完成 ✓') : `${p.stage} ${pct}%`,
          type: (done ? 'success' : 'info') as ToastType,
          duration: done ? 4000 : 0,
          progress: done ? undefined : pct,
        };
        return next;
      });
    };

    // 终态自愈：完成态只由一次性的 startup:progress 事件传递，前端若在事件发出
    // 之后才完成挂载，就会永久停在最后一条持久进度上。正常启动每阶段都有事件
    // （周期重发在启动期间持续投递），事件流一旦静止即说明"最后一条事件没送到"，
    // 此时查询后端启动状态：已结束就清掉遗留条目，仍在进行则继续等下一轮。
    // 相比固定周期轮询，只在事件流静止时查询，启动正常结束时零额外开销。
    const armStallWatchdog = () => {
      if (stallTimer !== null) window.clearTimeout(stallTimer);
      stallTimer = window.setTimeout(() => {
        stallTimer = null;
        void (async () => {
          if (cancelled) return;
          try {
            const s = await invoke<{ in_progress: boolean }>('get_startup_progress');
            if (cancelled) return;
            if (s && !s.in_progress) {
              setItems((prev) => prev.filter((it) => it.taskKey !== STARTUP_TOAST_KEY));
            } else {
              armStallWatchdog();
            }
          } catch {
            armStallWatchdog();
          }
        })();
      }, STARTUP_STALL_TIMEOUT_MS);
    };

    void (async () => {
      // 主角色必须在监听器生效前解析完成：父窗口收到 toast:ready 会立刻补发挂载期
      // 缓存的无归属 toast，若此刻主角色未知，这些 toast 会在每个窗口都被判定为
      // "非主角色"而集体丢弃。失败则保持未解析态，由 canRenderUnowned 放行兜底。
      try {
        await resolvePrimary();
      } catch { /* 解析失败：无归属 toast 退化为广播，可见但不静默 */ }

      // 并行注册所有事件监听，避免顺序 await 累积延迟拖慢挂载
      const [
        unlistenShow,
        unlistenShown,
        unlistenConfirm,
        unlistenDone,
        unlistenEmbedProgress,
        unlistenStartupProgress,
        unlistenActiveChanged,
        unlistenOnlineChanged,
      ] = await Promise.all([
        listen<ToastShowPayload>('toast:show', (e) => {
          if (myCharId === 'startup') return;
          const p = e.payload;
          // 归属判定：有归属 → 只弹该角色窗口；无归属（共享窗口 / 全局事件）→ 只弹
          // 主角色窗口。后者是多角色下重复弹窗的根因——共享窗口不绑定角色，
          // 事件一旦广播，每只桌宠的 toast 窗口都会各自渲染一条同样的文案。
          const owned = p.character_id ? p.character_id === myCharId : canRenderUnowned();
          const toastType = p.type ?? 'info';
          const now = Date.now();
          // 原地刷新（同一个 key 第二次出现）才豁免内容去重：进度条会被反复喂新文案，
          // 按内容去重会把刷新误判成重复、把进度冻住。One-shot 提示（哪怕自带
          // `key: Date.now()`）一律参与去重——这是"同一内容在屏幕上只存在一条"的前提。
          const refresh = keyTrackerRef.current.isRepeat(p.key, now);
          const dedupable = !refresh;
          const fp = dedupable ? toastFingerprint(p.message, toastType) : '';
          if (dedupable && owned) {
            const gate = dedupGateRef.current;
            gate.prune(now);
            // 本窗口重复收到同一内容，或另一窗口已认领且优先级更高 → 不追加
            if (gate.shouldSkip(fp, now)) return;
          }
          setItems((prev) => {
            // 同 key 原地更新：用于持久进度 toast 的刷新与收尾（duration 0 → >0 触发自动关闭）。
            // 更新路径不受归属过滤约束——条目已经落在本窗口里，就必须允许它被收尾；
            // 否则主角色切换后，遗留的持久条目（duration 0）永远关不掉，窗口再也无法隐藏。
            if (p.key != null) {
              const idx = prev.findIndex((it) => it.key === p.key);
              if (idx >= 0) {
                const next = [...prev];
                next[idx] = {
                  ...next[idx],
                  message: p.message,
                  type: p.type ?? next[idx].type,
                  duration: p.duration ?? next[idx].duration,
                  progress: p.progress ?? next[idx].progress,
                  // 被刷新过就确认了它的身份：此后不再参与跨窗口让位
                  inplace: true,
                };
                return next;
              }
            }
            // 新建路径才受归属约束：无归属且本窗口不是主角色 → 不认领这条
            if (!owned) return prev;
            // 可见去重（兜底时间窗）：上一条同内容还在屏幕上时，不重复追加。
            // 2s 时间窗只防「快速连发」，拦不住「间隔 >2s 但前一条 3s 自动关闭尚未到期」的重发——
            // 那条场景里前一条还在屏幕上，再弹一条就变成上下叠两条。
            // 原地刷新条目是「同一条的更新」，不算重复。
            if (dedupable) {
              for (const it of prev) {
                if (it.inplace) continue;
                if (toastFingerprint(it.message, it.type) === fp) return prev;
              }
            }
            return [
              ...prev,
              {
                id: nextId++,
                key: p.key,
                message: p.message,
                type: toastType,
                duration: p.duration ?? 3000,
                progress: p.progress,
                action: p.action,
                // 命中的是同一条的逻辑身份（此前那条已被移除），仍然按原地刷新对待
                inplace: refresh,
                // 入场要过容量管理这一关：先排队，许可下达后才切到 live
                phase: 'queued' as ToastPhase,
              },
            ];
          });
          if (dedupable && owned) {
            // 同步登记 + 广播认领：登记是同步的，同一 tick 内到达的第二条相同内容
            // 会被上面拦住；广播让其他窗口据此让位。
            dedupGateRef.current.claim(fp, now);
            void emit('toast:shown', { char_id: myCharId, fp }).catch(() => {});
          }
          // 只有真正走到这里的 key 才算"落过屏"：被去重拦下的那条不该让后续同名到达
          // 获得豁免，否则一次误放的重复会自我加固成永久例外。
          keyTrackerRef.current.note(p.key, now);
        }),
        // 跨窗口内容去重：另一窗口宣告呈现了某条内容。若它优先级更高，本窗口让位，
        // 撤掉自己那条同样的一次性提示——保证同一内容在屏幕上只存在一条。
        // 原地刷新条目（进度条那类）不受影响：它本就该在自己的窗口里持续更新。
        listen<{ char_id: string; fp: string }>('toast:shown', (e) => {
          const id = e.payload?.char_id;
          const fp = e.payload?.fp;
          if (!id || !fp || id === myCharId) return;
          const yieldToPeer = dedupGateRef.current.observe(id, fp, Date.now());
          if (!yieldToPeer) return; // 对方优先级更低 → 保留本窗口那条
          setItems((prev) => {
            const next = prev.filter(
              (it) => it.inplace || toastFingerprint(it.message, it.type) !== fp,
            );
            return next.length === prev.length ? prev : next;
          });
        }),
        listen<ToastConfirmPayload>('toast:confirm', (e) => {
          const p = e.payload;
          if (!p) return;
          if (myCharId === 'startup') return;
          // char_id 为空表示不区分角色，所有窗口显示（先响应者生效）
          if (p.char_id && p.char_id !== myCharId) return;
          setConfirms((prev) => {
            if (prev.some((c) => c.requestId === p.request_id)) return prev;
            return [
              ...prev,
              {
                requestId: p.request_id,
                tool: p.tool,
                reason: p.reason,
                riskLevel: p.risk_level,
                allowAlwaysScope: p.allow_always_scope,
                charId: p.char_id ?? '',
                args: p.arguments,
              },
            ];
          });
        }),
        listen<{ request_id: number }>('toast:confirm_done', (e) => {
          const rid = e.payload?.request_id;
          if (rid == null) return;
          if (myCharId === 'startup') return;
          setConfirms((prev) => prev.filter((c) => c.requestId !== rid));
        }),
        // 嵌入初始化进度：后端每完成一批 (168条) 后 emit，前端管理持久 toast 的生命周期。
        // 该事件不带角色归属（模型初始化是全局操作），故新建条目时按无归属规则收敛到主角色窗口。
        listen<{ current: number; total: number }>('embedding:progress', (e) => {
          if (myCharId === 'startup') return;
          const { current, total } = e.payload ?? { current: 0, total: 1 };
          const pct = Math.round((current / Math.max(total, 1)) * 100);
          if (current >= total) {
            // 完成：切换为 success + 自动关闭。
            // 不做归属过滤：谁持有这条就由谁收尾，否则主角色切换会让旧窗口留下
            // duration 0 的遗留条目，窗口再也无法隐藏。
            setItems((prev) => {
              const idx = prev.findIndex((it) => it.key === EMBEDDING_TOAST_KEY);
              if (idx >= 0) {
                const next = [...prev];
                next[idx] = {
                  ...next[idx],
                  message: '情绪感知就绪 ✓',
                  type: 'success',
                  duration: 4000,
                  progress: undefined,
                };
                return next;
              }
              return prev;
            });
          } else {
            // 进度中：创建或更新持久 toast。已有条目则更新（同上，收尾权优先），
            // 需要新建时才判定呈现权，避免两只桌宠各建一条同样的进度 toast。
            // 判定在 setItems 之外完成，保持更新函数纯净（StrictMode 下会被重复调用）。
            const canCreate = canRenderUnowned();
            setItems((prev) => {
              const idx = prev.findIndex((it) => it.key === EMBEDDING_TOAST_KEY);
              if (idx >= 0) {
                const next = [...prev];
                next[idx] = {
                  ...next[idx],
                  message: `情绪感知初始化中… ${pct}%`,
                  progress: pct,
                };
                return next;
              }
              if (!canCreate) return prev;
              return [
                ...prev,
                {
                  id: nextId++,
                  key: EMBEDDING_TOAST_KEY,
                  message: `情绪感知初始化中… ${pct}%`,
                  type: 'info' as ToastType,
                  duration: 0,
                  progress: pct,
                  // 固定 key + 反复刷新 → 原地刷新条目，不参与内容去重
                  inplace: true,
                  phase: 'queued' as ToastPhase,
                },
              ];
            });
          }
        }),
        // 统一启动进度：所有启动加载阶段共用同一条持久 toast（去重/节流见 applyStartupProgress）
        listen<{ current: number; total: number; stage: string }>('startup:progress', (e) => {
          if (myCharId !== 'startup') return;
          // 收到任何进度（含被去重的周期重发）都重置停滞看门狗：说明启动仍在推进
          armStallWatchdog();
          const p = e.payload ?? { current: 0, total: 1, stage: '' };
          // 去重：与最近已应用载荷一致的周期重发直接丢弃
          const last = startupThrottle.lastPayload;
          if (
            last &&
            last.current === p.current &&
            last.total === p.total &&
            last.stage === p.stage
          ) {
            return;
          }
          const done = p.current >= p.total;
          const now = Date.now();
          if (done || now - startupThrottle.lastAppliedAt >= STARTUP_THROTTLE_MS) {
            if (startupThrottle.timer !== null) {
              window.clearTimeout(startupThrottle.timer);
              startupThrottle.timer = null;
              startupThrottle.pending = null;
            }
            applyStartupProgress(p);
            return;
          }
          // 节流窗口内：暂存最新载荷，到期一次性应用
          startupThrottle.pending = p;
          if (startupThrottle.timer === null) {
            startupThrottle.timer = window.setTimeout(() => {
              startupThrottle.timer = null;
              const pending = startupThrottle.pending;
              startupThrottle.pending = null;
              if (pending && !cancelled) applyStartupProgress(pending);
            }, STARTUP_THROTTLE_MS - (now - startupThrottle.lastAppliedAt));
          }
        }),
        // 活跃角色切换：主角色窗口随之转移，否则无归属 toast 会一直弹在旧角色窗口上。
        // 重新解析而非直接采用 payload：活跃角色可能不在线（无窗口），仍需兜底判定。
        listen<{ character_id: string }>('character:active_changed', () => {
          void resolvePrimary().catch(() => {});
        }),
        // 角色上下线会改变「在线角色集合」，主角色的兜底目标随之变化
        listen<{ character_id: string; online: boolean }>('character:online_changed', () => {
          void resolvePrimary().catch(() => {});
        }),
      ]);


      // 启动窗口：拉取一次进度快照补齐。后端预检不等前端就绪就直接开始
      // （进度事件由周期重发补齐），此处快照保证窗口一挂载就能看到当前进度
      if (myCharId === 'startup') {
        type StartupSnapshot = {
          in_progress: boolean;
          current: number | null;
          total: number | null;
          stage: string | null;
        };
        let snap: StartupSnapshot | null = null;
        try {
          snap = await invoke<StartupSnapshot | null>('get_startup_progress');
        } catch { /* ignore */ }
        // 快照与事件共用同一条更新路径：两条来源只更新同一条 toast，去重基准也由
        // 它初始化，紧随其后的周期重发（与快照同源）会被丢弃。
        // 启动已收尾时不建任何条目——那意味着这个窗口本就不该再显示内容。
        if (!cancelled && snap?.in_progress && snap.current != null && snap.stage != null) {
          applyStartupProgress({
            current: snap.current,
            total: snap.total ?? 100,
            stage: snap.stage,
          });
        }
        // 快照到达前可能一个事件都没收到，看门狗需自行起跳：
        // 若启动其实已收尾（快照 in_progress=false），它会清掉所有遗留条目
        if (!cancelled) armStallWatchdog();
      }

      if (cancelled) {
        await unlistenShow();
        await unlistenShown();
        await unlistenConfirm();
        await unlistenDone();
        await unlistenEmbedProgress();
        await unlistenStartupProgress();
        await unlistenActiveChanged();
        await unlistenOnlineChanged();
        return;
      }

      unlistens.push(unlistenShow, unlistenShown, unlistenConfirm, unlistenDone, unlistenEmbedProgress, unlistenStartupProgress, unlistenActiveChanged, unlistenOnlineChanged);
      void emit('toast:ready', { character_id: myCharId });
    })();

    return () => {
      cancelled = true;
      if (startupThrottle.timer !== null) {
        window.clearTimeout(startupThrottle.timer);
        startupThrottle.timer = null;
      }
      if (stallTimer !== null) {
        window.clearTimeout(stallTimer);
        stallTimer = null;
      }
      for (const un of unlistens) un();
    };
  }, []);

  // 窗口可见性：有 toast 或确认卡片时显示，全部清除后隐藏
  const hasContent = items.length > 0 || confirms.length > 0;
  useEffect(() => {
    const win = getCurrentWindow();
    if (hasContent) {
      void win.show().catch(() => {});
    } else {
      void win.hide().catch(() => {});
    }
  }, [hasContent]);

  // 监听其他窗口广播的占用高度（emit 是全局广播，自己也会收到，故过滤掉自身）
  useEffect(() => {
    let cancelled = false;
    let un: (() => void) | undefined;
    void (async () => {
      try {
        un = await listen<{ char_id: string; height: number }>('toast:stack', (e) => {
          const id = e.payload?.char_id;
          const h = e.payload?.height;
          if (!id || id === myCharId || typeof h !== 'number') return;
          setStackHeights((prev) => (prev[id] === h ? prev : { ...prev, [id]: h }));
        });
        if (cancelled) un();
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      un?.();
    };
  }, [myCharId]);

  const removeItem = (id: number) => {
    setItems((prev) => prev.filter((it) => it.id !== id));
  };

  /**
   * toast 动作处理。两种动作都遵循「先关掉自己，再去干活」：
   * 点击反馈是即时的，副作用在后台完成，失败只进日志。
   */
  const handleToastAction = (it: ToastItem) => {
    const action = it.action;
    if (!action) return;
    removeItem(it.id);
    if (action.kind === 'switch_theme') {
      const theme = action.theme;
      if (theme !== 'light' && theme !== 'dark') return;
      void (async () => {
        try {
          await invoke('set_config', { key: 'base.theme', value: theme });
          await invoke('save_config');
          await emit('config:theme-changed', { theme });
        } catch (e) {
          console.warn('[ToastWindow] 切换主题失败:', e);
        }
      })();
    } else if (action.kind === 'open_url' && action.url) {
      // shell.open 走系统默认浏览器（capability 已放行 shell:allow-open）；
      // URL 由下发方保证为 http(s)。厂商控制台页面无需保持登录态，无 Chrome 限定需求。
      void shellOpen(action.url).catch((e) => console.warn('[ToastWindow] 打开链接失败:', e));
    }
  };

  const removeConfirm = (requestId: number) => {
    setConfirms((prev) => prev.filter((c) => c.requestId !== requestId));
  };

  /**
   * 堆叠条目（自顶向下）：确认卡片排在最上，其后按创建顺序排已入场的 toast，
   * 最新的压在最底、紧贴屏幕右下角。排队中的条目不在这里——它还没拿到入场许可。
   */
  const entries: StackEntry[] = [];
  for (const c of confirms) {
    entries.push({
      id: `c-${c.requestId}`,
      live: true,
      // 确认卡等的是用户决策，不能因为空间紧张就被请走
      evictable: false,
      wide: true,
      interactive: true,
      node: (
        <ConfirmToast
          key={c.requestId}
          requestId={c.requestId}
          tool={c.tool}
          reason={c.reason}
          riskLevel={c.riskLevel}
          allowAlwaysScope={c.allowAlwaysScope}
          charId={c.charId}
          args={c.args}
          onDone={() => removeConfirm(c.requestId)}
        />
      ),
    });
  }
  for (const it of items) {
    if (it.phase === 'queued') continue;
    const exiting = it.phase === 'exiting';
    entries.push({
      id: `t-${it.id}`,
      live: !exiting,
      // 持久条目（duration<=0，如后台任务进度）由外部事件收尾：淘汰它只是让进度消失，
      // 下一次进度事件又会原地重建，来回抖动比占着地方更糟。
      evictable: !exiting && it.duration > 0,
      wide: false,
      // 只有带操作按钮的 toast 需要鼠标；普通 toast 整块让给下面的窗口。
      // 退场中的条目也一并排除——它马上就不在了，没必要为它开一块可点区域。
      interactive: !exiting && it.action != null,
      node: (
        <Toast
          key={it.id}
          message={it.message}
          type={it.type}
          duration={it.duration}
          progress={it.progress}
          action={it.action}
          onAction={it.action ? () => handleToastAction(it) : undefined}
          onClose={() => removeItem(it.id)}
        />
      ),
    });
  }

  /**
   * 距底偏移：自底向上累加已占高度，第 i 条的偏移就是它下方所有条目的高度与间隙之和。
   * 退场条目已让出位置、不参与累加，于是它下方的条目会顺势滑落下来补位。
   */
  const offsets = new Map<string, number>();
  let accHeight = 0;
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i];
    if (!e.live) continue;
    offsets.set(e.id, accHeight);
    lastOffsetRef.current.set(e.id, accHeight);
    accHeight += (heightsRef.current.get(e.id) ?? 0) + STACK_GAP;
  }
  const stackH = Math.max(0, accHeight - STACK_GAP);
  // 没有条目在退场时，缓存里已经没有任何还需要「冻结旧偏移」的读者了，顺手回收
  if (offsets.size === entries.length) {
    for (const id of [...lastOffsetRef.current.keys()]) {
      if (!offsets.has(id)) lastOffsetRef.current.delete(id);
    }
  }

  /** 视口高度即窗口高度：量到它就能换算出堆叠可用空间 */
  useEffect(() => {
    const sync = () => setViewportH(window.innerHeight);
    sync();
    window.addEventListener('resize', sync);
    return () => window.removeEventListener('resize', sync);
  }, []);

  /**
   * 堆叠可用高度（像素）。
   *
   * 窗口高度固定为屏幕的一半，「能放几条」由此变成一个确定的常量：超过它的部分
   * 注定会被窗口边界裁掉，容量管理正是靠这个不等式才站得住。
   * 反过来，若窗口高度跟着内容伸缩，增删条目与重新测高之间必然存在一帧错位，
   * 那一帧里顶部就是断的——这正是原来按 scrollHeight 自适应窗口时偶发被裁的根因。
   */
  const availableH = Math.max(0, viewportH - WINDOW_PADDING * 2 - bottomOffset);

  /** 上报实测高度：合并成一次重算，免得在 ResizeObserver 回调里连着 setState */
  const pendingBumpRef = useRef(0);
  const reportHeight = useCallback((id: string, height: number) => {
    const map = heightsRef.current;
    if ((map.get(id) ?? 0) === height) return;
    if (height > 0) map.set(id, height);
    else map.delete(id);
    if (pendingBumpRef.current) return;
    pendingBumpRef.current = window.requestAnimationFrame(() => {
      pendingBumpRef.current = 0;
      setLayoutTick((t) => t + 1);
    });
  }, []);

  /** 条目元素登记：命中矩形要按元素布局盒定位，故在父级留一份索引 */
  const handleNode = useCallback((id: string, el: HTMLDivElement | null) => {
    if (el) nodesRef.current.set(id, el);
    else nodesRef.current.delete(id);
  }, []);
  /** 上一次已上报的命中矩形（序列化），相同则不再打扰后端 */
  const pushedRectsRef = useRef('');

  /** 退场动画播完：彻底摘掉条目，归还占位，排队的下一条随即获得放行机会 */
  const handleExited = useCallback((entryId: string) => {
    if (!entryId.startsWith('t-')) return;
    const id = Number(entryId.slice(2));
    if (!Number.isFinite(id)) return;
    setItems((prev) => prev.filter((it) => it.id !== id));
  }, []);

  /** 把指定条目切到某个阶段（已处于该阶段的不动，避免多余渲染） */
  const shiftPhase = (ids: readonly string[], phase: ToastPhase) => {
    const wanted = new Set(ids);
    setItems((prev) => {
      let changed = false;
      const next = prev.map((it) => {
        if (it.phase === phase || !wanted.has(`t-${it.id}`)) return it;
        changed = true;
        return { ...it, phase };
      });
      return changed ? next : prev;
    });
  };

  /**
   * 容量管理：先出后进。
   *
   * 两个职责共用一套判定——
   * 1. 容量修正：已入场的内容超出可用高度（估算偏差、文本变更、别的窗口挤占），
   *    从最老的开始请走。
   * 2. 准入：队首 toast 只有在「放得下」时才放行；放不下就先请走够数的老条目，
   *    等它们退场完毕、占位归还，下一次运行再把它放进来。
   */
  useEffect(() => {
    // 容量未知（视口还没量到，多半是窗口尚未被系统实现）：一律放行。
    // 宁可挤着显示甚至被裁，也不能让 toast 卡在队列里永不出现——丢信息比难看严重得多。
    if (viewportH <= 0) {
      const stuck = items.filter((it) => it.phase === 'queued').map((it) => `t-${it.id}`);
      if (stuck.length > 0) shiftPhase(stuck, 'live');
      return;
    }
    const live = entries.filter((e) => e.live);
    // 还有条目正在退场：等它们摘干净。不然「一边出一边进」，中间那一瞬仍是溢出的
    if (live.length !== entries.length) return;

    if (stackH > availableH) {
      const victims = pickVictims(live, heightsRef.current, availableH, stackH);
      // 无位可让（剩下的全是确认卡或持久进度条目）时不在这里卡住，继续走准入分支：
      // 宁可顶部被裁，也不能把新 toast 永久压在队列里
      if (victims.length > 0) {
        shiftPhase(victims, 'exiting');
        return;
      }
    }

    const head = items.find((it) => it.phase === 'queued');
    if (!head) return;
    const projected =
      stackH + (live.length > 0 ? STACK_GAP : 0) + estimateToastHeight(head.message);
    if (projected <= availableH || live.length === 0) {
      shiftPhase([`t-${head.id}`], 'live');
      return;
    }
    const victims = pickVictims(live, heightsRef.current, availableH, projected);
    if (victims.length > 0) {
      // 先出：这些条目的退场动画结束后 items 会变，本 effect 随之下一次放行新条目
      shiftPhase(victims, 'exiting');
      return;
    }
    shiftPhase([`t-${head.id}`], 'live');
  }, [items, confirms, stackH, availableH, viewportH]);

  // 广播自己的占用高度：内容增删都会改变它，下方窗口据此重新让位
  useEffect(() => {
    // 广播的是「从屏幕底到我内容顶边的距离」= 底留白 + 内容高度。
    // 下方窗口直接把自己顶到这个距离之上即可，不必再猜对方有没有算间隙——
    // 若只广播内容高度，每个接收方都要额外补 padding 与 gap，边界很容易差一档。
    void emit('toast:stack', {
      char_id: myCharId,
      height: hasContent ? WINDOW_PADDING + stackH : 0,
    }).catch(() => {});
  }, [myCharId, hasContent, stackH]);

  // 卸载前归还占位，否则别的窗口会一直为它空出位置
  useEffect(
    () => () => {
      void emit('toast:stack', { char_id: myCharId, height: 0 }).catch(() => {});
    },
    [myCharId],
  );

  /**
   * 命中区域上报：只有真正需要鼠标的条目（确认卡 / 带按钮的 toast）才登记矩形，
   * 其余透明区域交给后端保持穿透——窗口是一整块真实窗口，透明不等于不挡鼠标。
   *
   * 矩形取「布局盒」而不是 `getBoundingClientRect()`：入场/退场动画用 transform 位移，
   * 后者会把动画中间态的量出来当最终位置，且动画结束时不会再有渲染可以纠正它。
   * 布局盒是稳定的落定位置——落定之后动画期间也不会变。
   */
  useEffect(() => {
    const container = stackContainerRef.current;
    if (!container) return;
    const box = container.getBoundingClientRect();
    const rects: number[][] = [];
    for (const e of entries) {
      // 退场中的条目不再占用可点区域：它马上就不在了，不该在淡出过程中继续吃鼠标
      if (!e.live || !e.interactive) continue;
      const el = nodesRef.current.get(e.id);
      if (!el) continue;
      const w = el.offsetWidth;
      const h = el.offsetHeight;
      if (w <= 0 || h <= 0) continue;
      // 条目在容器里统一 bottom:0 抵底，视觉位置 = 布局位置 + translateY(-offset)
      const offset = offsets.get(e.id) ?? 0;
      rects.push([box.left + el.offsetLeft, box.bottom - h - offset, w, h]);
    }
    const key = JSON.stringify(rects);
    if (key === pushedRectsRef.current) return;
    pushedRectsRef.current = key;
    void invoke('set_toast_hit_regions', { regions: rects }).catch(() => {});
  });

  // 窗口几何：高度固定为屏幕的一半，贴屏幕右下角——固定值是容量管理的前提（见 availableH）。
  // 宽度沿用创建侧给的值（setSize 要求宽高同时给），此处不重复维护一份宽度常量。
  useEffect(() => {
    const win = getCurrentWindow();
    void currentMonitor()
      .then((monitor) => {
        if (!monitor) return;
        const factor = monitor.scaleFactor;
        const screenW = monitor.size.width / factor;
        const screenH = monitor.size.height / factor;
        const winW = window.innerWidth || 400;
        const winH = Math.max(1, Math.round(screenH / 2));
        void win.setSize(new LogicalSize(winW, winH)).catch(() => {});
        void win.setPosition(new LogicalPosition(screenW - winW, screenH - winH)).catch(() => {});
      })
      .catch(() => {});
  }, []);

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        overflow: 'hidden',
        background: 'transparent',
        pointerEvents: 'none',
      }}
    >
      {/* 堆叠容器：所有 toast 窗口统一锚定右下角，与其他角色窗口的错开由 bottomOffset
          承担（详见 STACK_ORDER 与跨窗口堆叠协调）。条目在容器内绝对定位、共同抵底，
          各自的垂直距离通过 transform 给出，位移才会是动画而不是重排。 */}
      <div
        ref={stackContainerRef}
        style={{
          position: 'absolute',
          left: WINDOW_PADDING,
          right: WINDOW_PADDING,
          top: WINDOW_PADDING,
          bottom: WINDOW_PADDING + bottomOffset,
          pointerEvents: 'none',
        }}
      >
        {entries.map((e) => (
          <StackSlot
            key={e.id}
            entryId={e.id}
            offset={offsets.get(e.id) ?? lastOffsetRef.current.get(e.id) ?? 0}
            exiting={!e.live}
            wide={e.wide}
            onHeight={reportHeight}
            onExited={handleExited}
            onNode={handleNode}
          >
            {e.node}
          </StackSlot>
        ))}
      </div>
    </div>
  );
}
