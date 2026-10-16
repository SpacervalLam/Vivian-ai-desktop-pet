import type { WorkflowView } from './workflowModel';

export const WORKBENCH_SNAPSHOT_KEY = 'vivian.workbench.snapshot.v1';
export interface WorkbenchSnapshot {
  activeId: string | null;
  input: string;
  rightTab: 'overview' | 'workflow' | 'browser' | 'changes' | 'preview' | 'terminal';
  workflowView: WorkflowView;
  previewTabs: { path: string; key: string }[];
  activePreview: string | null;
  previewFullView: boolean;
  sessionView: 'workspace' | 'flat';
  sessionSort: 'manual' | 'recent';
  workspaceTreeOpen: boolean;
  scroll: Record<string, number>;
}
export function readWorkbenchSnapshot(): WorkbenchSnapshot {
  let value: Partial<Omit<WorkbenchSnapshot, 'rightTab'> & { rightTab: WorkbenchSnapshot['rightTab'] | 'trajectory' }> = {};
  try { value = JSON.parse(localStorage.getItem(WORKBENCH_SNAPSHOT_KEY) ?? '{}') ?? {}; } catch { /* unavailable or invalid storage */ }
  const tabs = Array.isArray(value.previewTabs) ? value.previewTabs.filter(tab => tab && typeof tab.path === 'string' && typeof tab.key === 'string').slice(0, 100) : [];
  const storedTab = value.rightTab === 'trajectory' ? 'workflow' : value.rightTab;
  const rightTab = ['overview', 'workflow', 'browser', 'changes', 'preview', 'terminal'].includes(storedTab ?? '') ? storedTab! : 'overview';
  return {
    activeId: typeof value.activeId === 'string' ? value.activeId : null,
    input: typeof value.input === 'string' ? value.input : '',
    rightTab, workflowView: value.rightTab === 'trajectory' || value.workflowView === 'events' ? 'events' : 'flow', previewTabs: tabs,
    activePreview: tabs.some(tab => tab.path === value.activePreview) ? value.activePreview! : tabs[0]?.path ?? null,
    previewFullView: (['preview', 'browser', 'terminal'].includes(rightTab)) && value.previewFullView === true,
    sessionView: value.sessionView === 'flat' ? 'flat' : 'workspace',
    sessionSort: value.sessionSort === 'manual' ? 'manual' : 'recent',
    workspaceTreeOpen: value.workspaceTreeOpen !== false,
    scroll: value.scroll && typeof value.scroll === 'object' ? Object.fromEntries(Object.entries(value.scroll).filter(([, n]) => typeof n === 'number' && Number.isFinite(n) && n >= 0)) : {},
  };
}
export const SNAPSHOT_SCROLL_SELECTORS = ['.codex-chat', '.codex-sidebar-scroll', '.md-live', '.codex-src-scroll', '.codex-preview-body'];
