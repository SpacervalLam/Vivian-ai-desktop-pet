export const WORKBENCH_SNAPSHOT_KEY = 'vivian.workbench.snapshot.v1';
export interface WorkbenchSnapshot {
  activeId: string | null;
  input: string;
  rightTab: 'overview' | 'trajectory' | 'changes' | 'preview' | 'terminal';
  previewTabs: { path: string; key: string }[];
  activePreview: string | null;
  previewFullView: boolean;
  sessionView: 'workspace' | 'flat';
  sessionSort: 'manual' | 'recent';
  workspaceTreeOpen: boolean;
  scroll: Record<string, number>;
}
export function readWorkbenchSnapshot(): WorkbenchSnapshot {
  let value: Partial<WorkbenchSnapshot> = {};
  try { value = JSON.parse(localStorage.getItem(WORKBENCH_SNAPSHOT_KEY) ?? '{}') ?? {}; } catch { /* unavailable or invalid storage */ }
  const tabs = Array.isArray(value.previewTabs) ? value.previewTabs.filter(tab => tab && typeof tab.path === 'string' && typeof tab.key === 'string').slice(0, 100) : [];
  const rightTab = ['overview', 'trajectory', 'changes', 'preview', 'terminal'].includes(value.rightTab ?? '') ? value.rightTab! : 'overview';
  return {
    activeId: typeof value.activeId === 'string' ? value.activeId : null,
    input: typeof value.input === 'string' ? value.input : '',
    rightTab, previewTabs: tabs,
    activePreview: tabs.some(tab => tab.path === value.activePreview) ? value.activePreview! : tabs[0]?.path ?? null,
    previewFullView: rightTab === 'preview' && value.previewFullView === true,
    sessionView: value.sessionView === 'flat' ? 'flat' : 'workspace',
    sessionSort: value.sessionSort === 'manual' ? 'manual' : 'recent',
    workspaceTreeOpen: value.workspaceTreeOpen !== false,
    scroll: value.scroll && typeof value.scroll === 'object' ? Object.fromEntries(Object.entries(value.scroll).filter(([, n]) => typeof n === 'number' && Number.isFinite(n) && n >= 0)) : {},
  };
}
export const SNAPSHOT_SCROLL_SELECTORS = ['.codex-chat', '.codex-sidebar-scroll', '.md-live', '.codex-src-scroll', '.codex-preview-body'];
