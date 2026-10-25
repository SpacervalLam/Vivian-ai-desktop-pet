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

type WorkMessage = {
  role: string;
  tool_name?: string | null;
  tool_call_id?: string | null;
  tool_arguments?: unknown;
  content?: string;
  interjected?: boolean | null;
  guided?: boolean | null;
};

/** Legacy models echo tool names as prose; the activity cards already show these. */
export function stripToolCallAnnotations(content: string): string {
  const lines = content.split('\n');
  const kept = lines.filter(line => !/^\s*(?:调用工具|tool calls?)\s*[:：]\s*`?[a-zA-Z_][\w.-]*`?(?:\s*[,，、]\s*`?[a-zA-Z_][\w.-]*`?)*\s*[。.]?\s*$/i.test(line));
  return kept.length === lines.length ? content : kept.join('\n').trim();
}

/** Preserve invocation order and original indexes, even when results arrive after prose. */
function reconcileRows<T extends WorkMessage>(messages: T[]) {
  const rows: { message: T; index: number }[] = [];
  const calls = new Map<string, number>();
  messages.forEach((message, sourceIndex) => {
    if (message.role === 'tool_use' && !message.tool_name) return;
    if ((message.role === 'assistant' || message.role === 'commentary') && message.content) {
      const content = stripToolCallAnnotations(message.content);
      if (!content) return;
      if (content !== message.content) message = { ...message, content };
    }
    if (message.role === 'user' && !message.interjected && !message.guided) calls.clear();
    const id = message.role === 'tool_use' || message.role === 'tool_result' ? message.tool_call_id : undefined;
    const index = id ? calls.get(id) : undefined;
    if (index === undefined) {
      if (id) calls.set(id, rows.length);
      rows.push({ message, index: sourceIndex });
    } else if (message.role === 'tool_result' || rows[index].message.role !== 'tool_result') {
      const previous = rows[index].message;
      rows[index].message = { ...previous, ...message, tool_name: message.tool_name ?? previous.tool_name, tool_arguments: message.tool_arguments ?? previous.tool_arguments };
    }
  });
  return rows;
}

export function reconcileToolMessages<T extends WorkMessage>(messages: T[]): T[] {
  return reconcileRows(messages).map(row => row.message);
}

export type WorkChatRenderItem<T> =
  | { kind: 'msg'; msg: T; index: number }
  | { kind: 'group'; msgs: T[]; index: number; settled: boolean };

function isProcessMessage(msg: WorkMessage): boolean {
  return msg.role === 'tool_use' || msg.role === 'tool_result' || msg.role === 'thinking';
}

/** Public progress stays visible; only tool details and reasoning collapse. */
export function groupWorkMessages<T extends WorkMessage>(messages: T[], running: boolean): WorkChatRenderItem<T>[] {
  const rows = reconcileRows(messages).filter(({ message }) => !(message.role === 'notice'
    && /^(?:历史消息已达|上下文占用已达窗口的).*已自动把.*早期历史压缩为摘要/.test(message.content ?? '')));
  const items: WorkChatRenderItem<T>[] = [];
  let i = 0;
  while (i < rows.length) {
    const row = rows[i];
    if (!isProcessMessage(row.message)) {
      items.push({ kind: 'msg', msg: row.message, index: row.index });
      i++;
      continue;
    }
    let j = i;
    const group: T[] = [];
    while (j < rows.length && isProcessMessage(rows[j].message)) group.push(rows[j++].message);
    const turnEnded = rows.slice(j).some(({ message }) => message.role === 'assistant'
      || (message.role === 'user' && !message.interjected && !message.guided));
    items.push({ kind: 'group', msgs: group, index: row.index,
      settled: !running || !group.some(message => message.role === 'tool_use') || turnEnded });
    i = j;
  }
  return items;
}
