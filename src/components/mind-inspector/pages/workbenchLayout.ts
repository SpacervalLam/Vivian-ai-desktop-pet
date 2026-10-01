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
