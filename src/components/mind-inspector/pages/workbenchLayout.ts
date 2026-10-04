/** Keep reading/composing space independent of auxiliary panels. All sizes are CSS pixels. */
export const WORKBENCH_READING_WIDTH = 540;
export const WORKBENCH_RAIL_WIDTH = 54;
export const WORKBENCH_HANDLE_WIDTH = 6;

export function workbenchLayout(width: number, leftWidth: number, rightWidth: number, leftCollapsed: boolean, rightCollapsed: boolean, focus: boolean) {
  const available = Math.max(0, width);
  const autoLeft = available < leftWidth + WORKBENCH_READING_WIDTH + WORKBENCH_HANDLE_WIDTH;
  const compactLeft = focus || leftCollapsed || autoLeft;
  const left = compactLeft ? WORKBENCH_RAIL_WIDTH : leftWidth;
  const rightOpen = !focus && !rightCollapsed;
  const drawer = rightOpen && available - left - rightWidth - WORKBENCH_HANDLE_WIDTH * 2 < WORKBENCH_READING_WIDTH;
  return {
    autoLeft,
    leftCollapsed: compactLeft,
    rightCollapsed: !rightOpen,
    rightDrawer: drawer,
    rightWidth: drawer ? Math.max(0, Math.min(rightWidth, available - 20)) : rightWidth,
    centralWidth: Math.max(0, available - left - WORKBENCH_HANDLE_WIDTH - (rightOpen && !drawer ? rightWidth + WORKBENCH_HANDLE_WIDTH : 0)),
  };
}

/** A persisted intent and its result share one row. Never merge anonymous calls by name. */
export function reconcileToolMessages<T extends { role: string; tool_name?: string | null; tool_call_id?: string | null; tool_arguments?: unknown }>(messages: T[]): T[] {
  const rows: T[] = [];
  const calls = new Map<string, number>();
  for (const message of messages) {
    if (message.role === 'tool_use' && !message.tool_name) continue;
    const id = message.tool_call_id;
    const index = id ? calls.get(id) : undefined;
    if (index === undefined) {
      if (id) calls.set(id, rows.length);
      rows.push(message);
    } else if (message.role === 'tool_result' || rows[index].role !== 'tool_result') {
      rows[index] = { ...rows[index], ...message, tool_name: message.tool_name ?? rows[index].tool_name, tool_arguments: message.tool_arguments ?? rows[index].tool_arguments };
    }
  }
  return rows;
}

/** 聊天流渲染项：普通消息 或 一组连续的工具调用消息 */
export type WorkChatRenderItem<T> =
  | { kind: 'msg'; msg: T; index: number }
  | { kind: 'group'; msgs: T[]; index: number; settled: boolean };

/**
 * 把消息列表切分为渲染项：连续的工具消息、过程说明和思考文本聚为一组，
 * 其余消息原样透传，最终回复保留在分组之外。
 *
 * `settled`（组已收尾）判定：组后出现了总结（assistant）/下一轮 user 消息，
 * 或会话已不在运行态——此时分组自动折叠成一行摘要。
 */
function isWorkProcessMessage(msg: { role: string }): boolean {
  return ['tool_use', 'tool_result', 'commentary', 'thinking'].includes(msg.role);
}

export function groupWorkMessages<T extends { role: string; tool_name?: string | null; tool_call_id?: string | null; tool_arguments?: unknown }>(messages: T[], running: boolean): WorkChatRenderItem<T>[] {
  const items: WorkChatRenderItem<T>[] = [];
  let i = 0;
  while (i < messages.length) {
    const m = messages[i];
    if (isWorkProcessMessage(m)) {
      let j = i;
      const group: T[] = [];
      while (j < messages.length && isWorkProcessMessage(messages[j])) {
        group.push(messages[j]);
        j += 1;
      }
      const rows = reconcileToolMessages(group);
      if (rows.length >= 1) {
        const followed = messages
          .slice(j)
          .some((x) => x.role === 'assistant' || x.role === 'user');
        items.push({ kind: 'group', msgs: rows, index: i, settled: followed || !running });
      } else {
        rows.forEach((msg, k) => items.push({ kind: 'msg', msg, index: i + k }));
      }
      i = j;
    } else {
      items.push({ kind: 'msg', msg: m, index: i });
      i += 1;
    }
  }
  return items;
}
