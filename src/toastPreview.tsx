/**
 * Toast 堆叠的免 Tauri 预览入口。
 *
 * ToastWindow 平时只能跑在它那个半屏高、透明、无边框的子窗口里，改完排布想看一眼得走
 * 完整 tauri dev。这里注入一个假的 Tauri bridge，把 **`toast:show` / `toast:confirm`
 * 这套事件总线在页内跑通**，于是普通浏览器就能渲染出真实的 ToastWindow
 * （`npm run dev` 后访问 /toast-preview.html）。
 *
 * 整页 body 代表那个窗口的客户区：ToastWindow 用 `window.innerHeight` 换算堆叠容量，
 * 所以把浏览器窗口拉成任意高度，就等于在预览该高度的 toast 窗口——「栈里有几条、
 * 第几条会被请出去」与实际表现完全一致，不需要额外猜测。
 *
 * URL 参数：
 * - `?character_id=` 模拟窗口归属，默认 vivian（无归属 toast 只认主角色窗口）
 * - `?theme=dark` 强制深色
 *
 * 仅供开发期看排布与容量行为，不参与生产构建（根目录的预览 html 不在 vite 构建入口里）。
 */

import { StrictMode, useState, type CSSProperties } from 'react';
import { createRoot } from 'react-dom/client';

type Handler = (payload: unknown) => void;

const params = new URLSearchParams(window.location.search);
const theme = params.get('theme');
const charId = params.get('character_id') ?? 'vivian';
if (theme) document.documentElement.setAttribute('data-theme', theme);
if (!params.has('character_id')) {
  params.set('character_id', charId);
  window.history.replaceState(null, '', `${location.pathname}?${params.toString()}`);
}

/**
 * 页内事件总线 + invoke mock。
 *
 * Tauri 的 listen/emit 在底层就是这两个 IPC：`plugin:event|listen` 登记（handler 由
 * transformCallback 换成数字 id），`plugin:event|emit` 广播。这里用两张表在页内复刻，
 * 于是跨角色的 `toast:shown` / `toast:stack` 广播也能照常参与去重与堆叠协调。
 */
const callbacks = new Map<number, Handler>();
const listeners = new Map<string, Map<number, Handler>>();
let nextCallbackId = 1;

const RESPONSES: Record<string, unknown> = {
  get_config: theme ?? 'light',
  list_characters: { active_id: 'vivian', characters: [{ id: 'vivian', online: true }] },
  get_startup_progress: { in_progress: false, current: null, total: null, stage: null },
  // 浏览器里没有显示器概念：返回 null 让 ToastWindow 跳过窗口几何设置
  'plugin:window|current_monitor': null,
};

const dispatch = (event: string, payload: unknown) => {
  const handlers = listeners.get(event);
  if (!handlers) return;
  for (const [id, handler] of [...handlers]) {
    handler({ event, id, payload });
  }
};

const internals = {
  metadata: {
    windows: [{ label: 'preview_toast' }],
    currentWindow: { label: 'preview_toast' },
    currentWebview: { label: 'preview_toast', windowLabel: 'preview_toast' },
  },
  transformCallback: (cb: Handler) => {
    const id = nextCallbackId++;
    callbacks.set(id, cb);
    return id;
  },
  convertFileSrc: (path: string) => path,
  invoke: async (cmd: string, args?: Record<string, unknown>) => {
    switch (cmd) {
      case 'plugin:event|listen': {
        const event = args?.event as string;
        const handlerId = args?.handler as number;
        const handler = callbacks.get(handlerId);
        if (!event || !handler) return handlerId;
        const bucket = listeners.get(event) ?? new Map<number, Handler>();
        bucket.set(handlerId, handler);
        listeners.set(event, bucket);
        return handlerId;
      }
      case 'plugin:event|unlisten': {
        listeners.get(args?.event as string)?.delete(args?.eventId as number);
        return null;
      }
      case 'plugin:event|emit':
        dispatch(args?.event as string, args?.payload);
        return null;
      default:
        return cmd in RESPONSES ? RESPONSES[cmd] : null;
    }
  },
};

Object.assign(window, {
  __TAURI_INTERNALS__: internals,
  __TAURI_EVENT_PLUGIN_INTERNALS__: {
    unregisterListener: (event: string, id: number) => {
      listeners.get(event)?.delete(id);
    },
  },
  /** 控制面板用它往总线里塞事件，等价于别处的 Tauri emit */
  __previewEmit: (event: string, payload: unknown) =>
    internals.invoke('plugin:event|emit', { event, payload }),
});

const emitToast = (message: string, type = 'info', duration = 6000) =>
  void internals.invoke('plugin:event|emit', {
    event: 'toast:show',
    payload: { message, type, duration, key: `preview_${Math.random()}`, character_id: charId },
  });

const LONG = '情绪感知模型初始化失败：嵌入服务返回 503，可能是本地 Ollama 尚未拉起模型，'
  + '或者端口被别的进程占用了。可以先去设定面板确认服务状态，再重试一次。';

/** 控制面板：只做触发，不参与 ToastWindow 的渲染 */
function ControlPanel() {
  const [n, setN] = useState(0);
  const next = () => {
    const v = n + 1;
    setN(v);
    return v;
  };
  const btn: CSSProperties = {
    font: '500 12px system-ui, sans-serif',
    padding: '5px 10px',
    borderRadius: 7,
    border: '1px solid rgba(0,0,0,0.12)',
    background: '#fff',
    color: '#333',
    cursor: 'pointer',
  };
  return (
    <div
      style={{
        position: 'fixed',
        left: 16,
        top: 16,
        zIndex: 9999,
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'flex-start',
        gap: 6,
        padding: 12,
        borderRadius: 10,
        background: 'rgba(255,255,255,0.86)',
        boxShadow: '0 4px 18px rgba(0,0,0,0.12)',
      }}
    >
      <span style={{ font: '600 11px ui-monospace, Consolas, monospace', opacity: 0.6 }}>
        preview · 视口 {window.innerWidth}×{window.innerHeight} · 可用容量{' '}
        {Math.max(0, window.innerHeight - 40)}px
      </span>
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, maxWidth: 380 }}>
        <button style={btn} onClick={() => emitToast(`短提示 #${next()}`, 'success')}>
          短提示
        </button>
        <button style={btn} onClick={() => emitToast(LONG, 'error', 15000)}>
          长文本
        </button>
        <button
          style={btn}
          onClick={() => {
            for (let i = 0; i < 5; i++) emitToast(`连发第 ${i + 1} 条 · T${next()}`);
          }}
        >
          连发 5 条
        </button>
        <button
          style={btn}
          onClick={() => {
            emitToast('埋点用持久 toast（不自动关）', 'info', 0);
          }}
        >
          持久条目
        </button>
        <button
          style={btn}
          onClick={() =>
            void internals.invoke('plugin:event|emit', {
              event: 'toast:confirm',
              payload: {
                request_id: Math.floor(Math.random() * 1e6),
                tool: 'run_shell',
                arguments: { command: 'docker compose logs -f api', cwd: 'G:\\vivian-rs' },
                reason: '准备执行一条 shell 命令，确认是否放行',
                risk_level: 'medium',
                char_id: charId,
                allow_always_scope: 'session',
              },
            })
          }
        >
          确认卡
        </button>
      </div>
      <span style={{ font: '11px system-ui, sans-serif', opacity: 0.55, maxWidth: 380 }}>
        调浏览器窗口高度即可改变容量：新 toast 放不下时，最老的会先滑出，随后新的才进入。
      </span>
    </div>
  );
}

void (async () => {
  await import('./i18n');
  await import('./styles/global.css');

  const { default: ToastWindow } = await import('./components/ToastWindow');

  const root = document.getElementById('root');
  if (!root) throw new Error('#root 缺失');

  createRoot(root).render(
    <StrictMode>
      <ControlPanel />
      <ToastWindow />
    </StrictMode>,
  );
})();
