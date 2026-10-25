export interface SidebarSession { session_id: string; title: string; working_directory: string; updated_at: number; status: string }
export interface SessionGroup { id: string; name: string }
export interface SessionGroupState { groups: SessionGroup[]; membership: Record<string, string> }
export const SESSION_GROUP_KEY = 'vivian.workbench.session-groups.v1';
export const SPECIAL_GROUPS = ['projects', 'recent'] as const;
/** Keep loaded history, but never revive a locally deleted row from an older snapshot. */
export function mergeSidebarSessions<T extends SidebarSession & { history_loaded?: boolean; extra_workspaces?: unknown }>(previous: T[], list: T[], activeId: string | null, deleted: ReadonlySet<string>): T[] {
  const loaded = new Map(previous.filter(session => session.history_loaded !== false).map(session => [session.session_id, session]));
  const rows = [...new Map(list.filter(session => !deleted.has(session.session_id)).map(session => {
    const opened = loaded.get(session.session_id);
    return [session.session_id, opened ? { ...opened, title: session.title, status: session.status, updated_at: session.updated_at, working_directory: session.working_directory, extra_workspaces: session.extra_workspaces } : session] as const;
  })).values()];
  const active = previous.find(session => session.session_id === activeId && !deleted.has(session.session_id));
  if (active && !rows.some(session => session.session_id === active.session_id)) rows.push(active);
  return rows;
}
export function readSessionGroups(raw: string | null): SessionGroupState {
  try {
    const value = JSON.parse(raw || '{}');
    const ids = new Set<string>(SPECIAL_GROUPS);
    const groups: SessionGroup[] = [];
    for (const group of Array.isArray(value.groups) ? value.groups : []) {
      if (!group || typeof group.id !== 'string' || !group.id || ids.has(group.id) || typeof group.name !== 'string' || !group.name.trim()) continue;
      ids.add(group.id); groups.push({ id: group.id, name: group.name.trim().slice(0, 100) });
    }
    const custom = new Set(groups.map(group => group.id));
    const membership = Object.fromEntries(Object.entries(value.membership || {}).filter(([id, group]) => id && typeof group === 'string' && custom.has(group)));
    return { groups, membership: membership as Record<string, string> };
  } catch { return { groups: [], membership: {} }; }
}
export function recentSidebarSessions<T extends SidebarSession>(sessions: T[]): T[] {
  return [...new Map(sessions.map(session => [session.session_id, session])).values()]
    .sort((a, b) => b.updated_at - a.updated_at || a.session_id.localeCompare(b.session_id)).slice(0, 40);
}
export function sidebarProjects<T extends SidebarSession>(sessions: T[]): { path: string; sessions: T[] }[] {
  const projects = new Map<string, { path: string; sessions: T[] }>();
  for (const session of [...sessions].sort((a, b) => b.updated_at - a.updated_at)) {
    const path = session.working_directory.trim();
    if (!path) continue;
    const key = path.replace(/\\/g, '/').replace(/\/+$/, '').replace(/^([a-z]):/i, (_, drive: string) => `${drive.toLowerCase()}:`);
    const normalized = /^[a-z]:/i.test(key) || key.startsWith('//') ? key.toLowerCase() : key;
    const project = projects.get(normalized) ?? { path, sessions: [] };
    project.sessions.push(session); projects.set(normalized, project);
  }
  return [...projects.values()];
}
export function removeSessionGroup(state: SessionGroupState, id: string): SessionGroupState {
  if (!state.groups.some(group => group.id === id)) return state;
  return { groups: state.groups.filter(group => group.id !== id), membership: Object.fromEntries(Object.entries(state.membership).filter(([, group]) => group !== id)) };
}
