/**
 * 心智观察器注意力上报。
 *
 * 工作智能体挂在「等用户拍板」上时，要不要让陪伴角色开口提醒，取决于用户此刻
 * 是否真的看得见那条提问。这个判断横跨两个世界：窗口层（存在/可见/最小化/前台）
 * Rust 直接可查，界面层（当前页签、激活会话）只有前端知道——这里就是那条上报通路。
 *
 * 两个组件分居不同层级：MindInspector 知道页签，CodeAgentPageNew 知道激活会话
 * （且只在工作页挂载）。所以两边各自只上报自己那半，缺省即保留 Rust 侧原值，
 * 谁也不会把对方覆盖成空。
 *
 * 上报前先比一次上一次的值：页签切换、会话切换都可能连续触发，
 * 没必要每次往返一次 IPC。Rust 侧一律保守缺省（没收到过就当作"用户看不见"），
 * 所以这里漏报不会导致"该提醒却不提醒"，最坏只是多提醒一次。
 */
import { invoke } from '@tauri-apps/api/core';

let lastNav: string | null = null;
let lastSessionId: string | null = null;

/** 上报当前页签（由 MindInspector 调用，覆盖全部页签）。 */
export function reportInspectorNav(nav: string): void {
  if (nav === lastNav) return;
  lastNav = nav;
  void invoke('report_inspector_attention', { nav }).catch(() => {});
}

/** 上报当前激活的工作会话（由 CodeAgentPageNew 调用，只在工作页存在）。 */
export function reportInspectorSession(sessionId: string | null | undefined): void {
  const next = sessionId ?? '';
  if (next === lastSessionId) return;
  lastSessionId = next;
  void invoke('report_inspector_attention', { sessionId: next }).catch(() => {});
}
