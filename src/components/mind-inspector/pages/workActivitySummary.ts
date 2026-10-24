type Activity = { tool_name?: string | null; tool_arguments?: unknown };
type Translate = (key: string, options?: Record<string, unknown>) => string;

/** Summarize actual operations without an extra model request or exposing raw commands. */
export function workActivitySummary(messages: Activity[], t: Translate): string {
  const labels: string[] = [];
  for (const message of messages) {
    const name = message.tool_name;
    if (!name) continue;
    let args: Record<string, unknown> = {};
    try {
      const value = typeof message.tool_arguments === 'string' ? JSON.parse(message.tool_arguments) : message.tool_arguments;
      if (value && typeof value === 'object' && !Array.isArray(value)) args = value as Record<string, unknown>;
    } catch { /* Older records may not contain structured arguments. */ }
    const kind = /search|grep/.test(name) ? 'search'
      : /read_content|web_fetch|browse|browser/.test(name) ? 'browse'
      : /read_file|list_dir|list_files/.test(name) ? 'read'
      : /write_file|edit_file|patch/.test(name) ? 'edit'
      : /command|terminal|shell/.test(name) ? 'run'
      : /todo|plan/.test(name) ? 'plan'
      : /image/.test(name) ? 'image' : 'other';
    const description = typeof args.description === 'string' ? args.description.trim().replace(/\s+/g, ' ') : '';
    const label = description || t(`workbench.activity_${kind}`);
    if (!labels.includes(label)) labels.push(label);
  }
  if (!labels.length) return t('workbench.workThinking');
  const summary = labels.slice(0, 2).join(t('workbench.activity_join'));
  return summary.length > 44 ? summary.slice(0, 43) + '…' : summary;
}
