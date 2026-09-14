/**
 * Vivian 浏览器桥 — background service worker。
 *
 * 职责：
 * 1. 发现本机 vivian 桥服务（轮询 127.0.0.1 候选端口上的 /ext/bridge-config），
 *    拿到 wsUrl 与 token，完成 token 认证的 WebSocket 连接。
 * 2. 监听服务端 `tool.call`，在受控标签页（最近聚焦窗口的活动标签）里执行动作，
 *    并通过 `tool.result` 回传纯文本结果。
 * 3. 连接断开自动带退避重连。
 *
 * 安全机制：
 * - `expiresAt` 过期检查：迟到的调用直接按 timeout 失败，不执行过期动作。
 * - `tool.cancel`：服务端撤回（超时/被取消）时中止对应 in-flight 工具。
 * - untrusted 包裹：读类结果用随机 nonce 双边界包裹，页面文本不能伪造闭合边界。
 * - 标签亲和：受控标签显式记录，用户手动切标签不静默改变工具目标（handoff）。
 */

'use strict';

/** 尝试发现桥服务的候选端口（顺序优先）。桥服务默认监听 3080。 */
const CANDIDATE_PORTS = [3080, 1921, 9090, 8080];

let ws = null;
let config = null;          // { wsUrl, token }
let reconnectTimer = null;
let reconnectDelay = 3000;
let lastTabId = null;

// ── 客户端心跳：主动收发双保险保活 ──────────────────────
// Chrome MV3 service worker 约 30s 不活动即被终止（Chrome 116+ 起 WS 消息
// 交换会重置该计时器）。服务端每 20s 发一次 ping，但为防调度抖动与
// "仅发送方重置计时器"的版本差异，扩展侧也每 20s 主动发送一帧 pong
// （服务端对主动 pong 静默忽略，无契约变更），确保连接不因 SW 空闲
// 终止而掉线。
const HEARTBEAT_INTERVAL_MS = 20000;
let heartbeatTimer = null;

// ── 标签亲和：受控标签追踪 ──────────────────────────────
// following: 是否处于"跟随"模式（默认 true：自动跟随最近聚焦的活动标签）
// controlledTabId: 显式锁定为某个标签时非 null（用户选择"跟随此标签"）
let following = true;
let controlledTabId = null;

// ── in-flight 工具：支持 tool.cancel 撤回 ────────────────
// id -> AbortController
const inflight = new Map();

// ── 平台登录态探测：Cookie 哨兵 ─────────────────────────
// 每个平台用一枚登录后才存在的 Cookie 判定；只在浏览器本地判定并回传布尔值，
// Cookie 值本身不离开浏览器——例外是 X (Twitter) cookie 重放（见下方
// reportXCookie，驱动 twitter-cli）与 Reddit cookie 同步（见下方
// reportRedditCookie，驱动 rdt-cli）：服务端内容发现需要真实 Cookie 值。
const PLATFORM_LOGIN_COOKIES = [
  { platform: 'bilibili',    url: 'https://www.bilibili.com',    name: 'SESSDATA' },
  { platform: 'zhihu',       url: 'https://www.zhihu.com',       name: 'z_c0' },
  { platform: 'xiaohongshu', url: 'https://www.xiaohongshu.com', name: 'web_session' },
  { platform: 'douyin',      url: 'https://www.douyin.com',      name: 'sessionid_ss' },
  { platform: 'weibo',       url: 'https://weibo.com',           name: 'SUB' },
  { platform: 'v2ex',        url: 'https://www.v2ex.com',        name: 'A2' },
  { platform: 'bangumi',     url: 'https://bgm.tv',              name: 'chii_auth' },
  { platform: 'youtube',     url: 'https://www.youtube.com',     name: 'SAPISID' },
  { platform: 'twitter',     url: 'https://x.com',               name: 'auth_token' },
  { platform: 'reddit',      url: 'https://www.reddit.com',      name: 'reddit_session' },
];

/** 探测各平台登录态并经 rpc 上报（连接未建立时静默跳过）。 */
async function reportPlatformStatus() {
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  try {
    const platforms = await Promise.all(PLATFORM_LOGIN_COOKIES.map(async (p) => {
      let loggedIn = false;
      try {
        const cookie = await chrome.cookies.get({ url: p.url, name: p.name });
        loggedIn = !!(cookie && cookie.value);
      } catch (_) { /* cookies API 不可用时按未登录处理 */ }
      return { platform: p.platform, loggedIn };
    }));
    sendFrame({
      t: 'rpc',
      id: `platform-status-${Date.now()}`,
      method: 'bridge.reportPlatformStatus',
      payload: { platforms },
    });
  } catch (_) { /* 探测失败静默，下次 cookies 变化再试 */ }
}

// ── X (Twitter) cookie 重放：auth_token + ct0 回传服务端 ──
// 服务端 X 内容发现经 twitter-cli 做 cookie 重放，需要真实的 auth_token 与
// ct0 两枚 Cookie（缺一 401）。仅在两枚齐全时回传拼好的 Cookie 头。

async function reportXCookie() {
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  try {
    const [authToken, ct0] = await Promise.all([
      chrome.cookies.get({ url: 'https://x.com', name: 'auth_token' }),
      chrome.cookies.get({ url: 'https://x.com', name: 'ct0' }),
    ]);
    if (!authToken || !authToken.value || !ct0 || !ct0.value) return;
    sendFrame({
      t: 'rpc',
      id: `x-cookie-${Date.now()}`,
      method: 'bridge.reportXCookie',
      payload: { cookie: `auth_token=${authToken.value}; ct0=${ct0.value}` },
    });
  } catch (_) { /* cookies API 不可用时静默跳过 */ }
}

// ── Reddit cookie 同步：reddit.com 整罐 Cookie 回传服务端 ──
// 服务端 Reddit 登录态发现经 rdt-cli 驱动，凭据文件需要 reddit_session
// 等真实 Cookie 值。仅在 reddit_session 存在时回传整罐 Cookie 头。

async function reportRedditCookie() {
  if (!ws || ws.readyState !== WebSocket.OPEN) return;
  try {
    const cookies = await chrome.cookies.getAll({ url: 'https://www.reddit.com/' });
    const usable = (cookies || []).filter((c) => c.name && c.value);
    if (!usable.some((c) => c.name === 'reddit_session')) return;
    sendFrame({
      t: 'rpc',
      id: `reddit-cookie-${Date.now()}`,
      method: 'bridge.reportRedditCookie',
      payload: { cookie: usable.map((c) => `${c.name}=${c.value}`).join('; ') },
    });
  } catch (_) { /* cookies API 不可用时静默跳过 */ }
}

// 登录/登出即时反映：Cookie 变化触发上报（去抖，避免批量变化反复上报）
let cookieReportTimer = null;
try {
  chrome.cookies.onChanged.addListener(() => {
    if (cookieReportTimer !== null) return;
    cookieReportTimer = setTimeout(() => {
      cookieReportTimer = null;
      reportPlatformStatus();
      reportXCookie();
      reportRedditCookie();
    }, 2000);
  });
} catch (_) { /* 无 cookies 权限时无监听 */ }

// ── 发现与连接 ──────────────────────────────────────────

async function discoverConfig() {
  for (const port of CANDIDATE_PORTS) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/ext/bridge-config`, { cache: 'no-store' });
      if (!res.ok) continue;
      const body = await res.json();
      if (body && typeof body.wsUrl === 'string' && typeof body.token === 'string') {
        config = { wsUrl: body.wsUrl, token: body.token };
        return true;
      }
    } catch (_) { /* 端口不通，试下一个 */ }
  }
  return false;
}

function connect() {
  if (!ws || ws.readyState === WebSocket.CLOSED || ws.readyState === WebSocket.CLOSING) {
    if (ws) { try { ws.close(); } catch (_) {} ws = null; }
    const next = () => { scheduleReconnect(); };
    discoverConfig().then((ok) => {
      if (!ok) { scheduleReconnect(); return; }
      const socket = new WebSocket(config.wsUrl);
      ws = socket;

      socket.addEventListener('open', () => {
        socket.send(JSON.stringify({
          t: 'hello',
          token: config.token,
          caps: { textOnly: true, snapshotMaxChars: 32000, maxInteractiveItems: 60 },
        }));
      });

      socket.addEventListener('message', (ev) => {
        let frame;
        try { frame = JSON.parse(typeof ev.data === 'string' ? ev.data : String(ev.data)); }
        catch (_) { return; }
        if (!frame || typeof frame !== 'object') return;
        switch (frame.t) {
          case 'hello.ok':
            setBadge(true);
            // 连接建立即上报一次平台登录态与 X/Reddit cookie；后续由 cookies.onChanged 增量刷新
            reportPlatformStatus();
            reportXCookie();
            reportRedditCookie();
            break;
          case 'tool.call':
            dispatchToolCall(frame).then(() => {});
            break;
          case 'tool.cancel':
            cancelToolCall(frame.id);
            break;
          case 'ping':
            sendFrame({ t: 'pong' });
            break;
          case 'error':
            // 认证/致命错误：断连重连
            try { socket.close(); } catch (_) {}
            break;
          default:
            break;
        }
      });

      socket.addEventListener('close', () => {
        // 被顶替的旧 socket 迟到的 close：badge / in-flight / 重连均以
        // 新连接为准，不清理新连接的状态（否则会误杀新连接上的工具调用）。
        if (ws !== socket) return;
        setBadge(false);
        ws = null;
        // 连接断开：所有 in-flight 工具失败
        failAllInflight('桥连接断开');
        scheduleReconnect();
      });
      socket.addEventListener('error', () => {
        try { socket.close(); } catch (_) {}
      });
    }).catch(next);
  }
}

function scheduleReconnect() {
  if (reconnectTimer !== null) return;
  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    connect();
  }, reconnectDelay);
}

function sendFrame(obj) {
  if (!ws || ws.readyState !== WebSocket.OPEN) return false;
  try { ws.send(JSON.stringify(obj)); return true; } catch (_) { return false; }
}

/** 启动客户端心跳（幂等）：连接打开时周期发送主动 pong 保活帧。 */
function startHeartbeat() {
  if (heartbeatTimer !== null) return;
  heartbeatTimer = setInterval(() => {
    if (ws && ws.readyState === WebSocket.OPEN) {
      sendFrame({ t: 'pong' });
    }
  }, HEARTBEAT_INTERVAL_MS);
}

// ── untrusted 包裹：页面文本的模型信任边界 ────────────────
// 用随机 nonce 双边界包裹，页面即使包含相同标记也无法伪造闭合边界。

function wrapUntrusted(text, maxChars = 32000) {
  const nonce = (crypto.randomUUID ? crypto.randomUUID() : String(Math.random()).slice(2));
  const budget = Math.max(0, maxChars - 200); // 边界本身占预算
  let body = String(text);
  if (body.length > budget) body = `${body.slice(0, budget)}…`;
  return `<UNTRUSTED_PAGE_CONTENT nonce="${nonce}">\n${body}\n</UNTRUSTED_PAGE_CONTENT>`;
}

// ── 受控标签 ─────────────────────────────────────────────

/** 解析工具目标标签：显式受控标签优先，否则跟随模式取最近聚焦标签。 */
async function getControlledTab() {
  if (controlledTabId !== null) {
    try {
      const tab = await chrome.tabs.get(controlledTabId);
      return tab;
    } catch (_) { /* 受控标签已关闭 → 清除并回退 */ controlledTabId = null; }
  }
  if (!following) return null; // 非跟随且无受控标签：无可操作目标
  try {
    const tabs = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
    if (tabs && tabs.length > 0) { lastTabId = tabs[0].id; return tabs[0]; }
  } catch (_) {}
  if (lastTabId !== null) {
    try {
      const tab = await chrome.tabs.get(lastTabId);
      return tab;
    } catch (_) {}
  }
  try {
    const tabs = await chrome.tabs.query({});
    if (tabs && tabs.length > 0) {
      const normal = tabs.filter((t) => t.id !== undefined && !t.discarded && t.url && !t.url.startsWith('chrome://'));
      const pick = normal[0] || tabs[0];
      if (pick && pick.id !== undefined) { lastTabId = pick.id; return pick; }
    }
  } catch (_) {}
  return null;
}

/** 注入跟随页快照：受控标签切到新页时，主动把快照推送服务端（对齐 bridge.injectBrowserSnapshot）。 */
async function injectFollowedSnapshot() {
  const tab = await getControlledTab();
  if (!tab || tab.id === undefined || !sendFrame) return;
  try {
    const res = await chrome.tabs.sendMessage(tab.id, { type: 'vivian_browser_action', action: 'browser_snapshot', args: {} });
    if (res && typeof res.text === 'string') {
      sendFrame({
        t: 'rpc',
        id: `inject-${Date.now()}`,
        method: 'bridge.injectBrowserSnapshot',
        payload: { text: res.text },
      });
    }
  } catch (_) { /* 页面未就绪，跳过 */ }
}

// ── 工具派发 ────────────────────────────────────────────

async function dispatchToolCall(frame) {
  const { id, name, args = {}, expiresAt } = frame;

  // expiresAt 过期检查：过期调用不执行，按 timeout 失败（防迟到动作）
  if (typeof expiresAt === 'number' && Number.isFinite(expiresAt) && Date.now() > expiresAt) {
    sendFrame({ t: 'tool.result', id, ok: false, error: { code: 'timeout', message: '工具调用已过期，未执行。' } });
    return;
  }

  // 隔离任务 tab：background 层直接执行，不经过受控标签页
  if (name === 'browser_task_tab') {
    const controller = new AbortController();
    inflight.set(id, controller);
    try {
      const res = await taskTabAction(args);
      const text = res && typeof res.text === 'string' ? res.text : JSON.stringify(res);
      sendFrame({ t: 'tool.result', id, ok: true, result: { text: wrapUntrusted(text) } });
    } catch (e) {
      sendFrame({
        t: 'tool.result', id, ok: false,
        error: { code: (e && e.code) || 'action-failed', message: e && e.message ? e.message : String(e) },
      });
    } finally {
      inflight.delete(id);
    }
    return;
  }

  const tab = await getControlledTab();
  if (!tab || tab.id === undefined) {
    sendFrame({ t: 'tool.result', id, ok: false, error: { code: 'no-active-tab', message: '没有可操作的标签页。请先在 Chrome 里打开一个网页。' } });
    return;
  }

  // 注册 in-flight（支持 tool.cancel）
  const controller = new AbortController();
  inflight.set(id, controller);

  try {
    const res = await chrome.tabs.sendMessage(tab.id, { type: 'vivian_browser_action', action: name, args }, { frameId: 0 });
    if (controller.signal.aborted) {
      sendFrame({ t: 'tool.result', id, ok: false, error: { code: 'cancelled', message: '工具调用已取消。' } });
      return;
    }
    if (!res) {
      sendFrame({
        t: 'tool.result', id, ok: false,
        error: { code: 'content-unavailable', message: `页面脚本未就绪（${tab.url || tab.id}）。请刷新页面后重试 browser_snapshot。` },
      });
      return;
    }
    if (res.error) {
      sendFrame({ t: 'tool.result', id, ok: false, error: { code: res.error.code || 'action-failed', message: res.error.message } });
      return;
    }
    // 读类结果 → untrusted 包裹（页面文本视为不可信数据）
    const text = typeof res.text === 'string' ? res.text : JSON.stringify(res);
    const isRead = /^browser_(snapshot|get_text)$/.test(name);
    const payloadText = isRead ? wrapUntrusted(text) : text;
    sendFrame({ t: 'tool.result', id, ok: true, result: { text: payloadText } });
  } catch (e) {
    // 可能是页面在导航中卸载、或 content script 尚未注入。
    sendFrame({
      t: 'tool.result', id, ok: false,
      error: { code: 'action-failed', message: `动作执行失败：${e && e.message ? e.message : e}` },
    });
  } finally {
    inflight.delete(id);
  }
}

/** tool.cancel：中止 in-flight 工具（AbortController + 回执 cancelled）。 */
function cancelToolCall(id) {
  const controller = inflight.get(id);
  if (controller) {
    controller.abort();
    sendFrame({ t: 'tool.result', id, ok: false, error: { code: 'cancelled', message: '工具调用已取消。' } });
    inflight.delete(id);
  }
}

// ── 隔离任务 tab：后台静默执行平台任务，不触碰用户正在看的标签页 ──
//
// 语义：创建一个 inactive 标签（不抢占焦点）→ 静音 → 等待加载 →
// 在该 tab 的 content script 中执行 eval JS → 关闭 tab → 返回结果。
// 标签属于同一浏览器 profile，天然携带该平台的登录 Cookie，实现
// 小红书/抖音/知乎等需登录态平台的后台发现（create 后立即 muted，避免
// autoplay 打扰用户）。
//
// 参数：
// - url: 必填，要打开的平台页面 URL
// - code: 可选，页面加载完成后在该 tab 内执行的 JS 表达式（同 browser_eval_js 语义）
// - waitMs: 可选，加载稳定后的额外等待毫秒数（默认 800，供 SPA 渲染）

class ActionError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

const TASK_TAB_SETTLE_POLL_MS = 250;
const TASK_TAB_MAX_WAIT_MS = 15000;

async function taskTabAction(args) {
  const url = typeof args.url === 'string' ? args.url.trim() : '';
  if (!/^https?:\/\//i.test(url)) {
    throw new ActionError('bad-args', 'url must be a http(s) URL');
  }
  const code = typeof args.code === 'string' ? args.code : '';
  const waitMs = typeof args.waitMs === 'number' && args.waitMs > 0 ? args.waitMs : 800;

  // 创建 inactive 后台标签（不激活 = 不劫持用户当前标签）
  const tab = await chrome.tabs.create({ url, active: false });
  const tabId = tab.id;
  try {
    // 静音（Chrome 不支持 create 时传 muted，须 update；失败不阻塞）
    try { await chrome.tabs.update(tabId, { muted: true }); } catch (_) {}

    // 等待加载完成（status complete）+ 额外 settle
    const deadline = Date.now() + TASK_TAB_MAX_WAIT_MS;
    while (Date.now() < deadline) {
      const t = await chrome.tabs.get(tabId);
      if (t.status === 'complete') break;
      await new Promise((r) => setTimeout(r, TASK_TAB_SETTLE_POLL_MS));
    }
    // SPA 渲染缓冲
    await new Promise((r) => setTimeout(r, waitMs));

    if (!code) {
      return { text: `任务 tab 已加载并保持打开：${url}` };
    }
    // 在任务 tab 的 content script 中执行 eval（与 browser_eval_js 同管道）
    const res = await chrome.tabs.sendMessage(
      tabId,
      { type: 'vivian_browser_action', action: 'browser_eval_js', args: { code } },
      { frameId: 0 },
    );
    if (res && res.error) {
      throw new ActionError(res.error.code || 'action-failed', res.error.message);
    }
    return res || { text: 'null' };
  } finally {
    // 无论成败都关闭任务 tab，不留残留
    try { await chrome.tabs.remove(tabId); } catch (_) {}
  }
}

/** 连接断开时失败全部 in-flight。 */
function failAllInflight(message) {
  for (const id of Array.from(inflight.keys())) {
    sendFrame({ t: 'tool.result', id, ok: false, error: { code: 'bridge-closed', message } });
  }
  inflight.clear();
}

function setBadge(connected) {
  if (!chrome.action) return;
  chrome.action.setBadgeText({ text: connected ? 'ON' : '' }).catch(() => {});
  chrome.action.setBadgeBackgroundColor({ color: '#2e7d32' }).catch(() => {});
  chrome.action.setTitle({ title: connected ? 'Vivian 浏览器桥：已连接' : 'Vivian 浏览器桥：未连接' }).catch(() => {});
}

// ── 生命周期 ────────────────────────────────────────────

chrome.runtime.onStartup.addListener(() => { connect(); });

// 唤醒保底：MV3 service worker 被终止后，纯 setTimeout 链不会唤醒它；
// 若断线期间 SW 被杀且无标签页/cookie 事件，扩展将一直失联。用周期
// alarm 唤醒 SW，顶层 connect() 在每次唤醒时自动检查并重建连接。
try {
  chrome.alarms.create('vivian-bridge-keepalive', { periodInMinutes: 1 });
  chrome.alarms.onAlarm.addListener((alarm) => {
    if (alarm.name === 'vivian-bridge-keepalive') connect();
  });
} catch (_) { /* 无 alarms 权限时退化为事件驱动重连 */ }

// 标签亲和：用户手动切换活动标签时，若处于跟随模式则更新目标，但不静默改变已显式受控标签
chrome.tabs.onActivated.addListener((info) => {
  if (following && controlledTabId === null) {
    lastTabId = info.tabId;
    void injectFollowedSnapshot();
  }
});
chrome.tabs.onRemoved.addListener((tabId) => {
  if (lastTabId === tabId) lastTabId = null;
  if (controlledTabId === tabId) controlledTabId = null;
});
// 受控标签导航完成 → 注入新页面快照
chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
  if (changeInfo.status === 'complete' && (following || controlledTabId === tabId)) {
    void injectFollowedSnapshot();
  }
});

// 顶层启动即连接（service worker 重新唤醒时也会运行）。
setBadge(false);
startHeartbeat();
connect();
