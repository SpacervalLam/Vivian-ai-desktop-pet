import { useEffect, useRef, useState, type CSSProperties, type MouseEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { ChevronRight, Ellipsis, Folder, FolderPlus, HelpCircle, Loader2, Plus, Search } from 'lucide-react';
import { readSessionGroups, recentSidebarSessions, removeSessionGroup, SESSION_GROUP_KEY, sidebarProjects, type SidebarSession } from './sessionSidebarModel';
import './SessionSidebar.css';
import './sessionSidebarStrings';

interface Props<T extends SidebarSession> {
  sessions: T[]; activeId: string | null; unread: Set<string>; waitingId?: string | null;
  onSelect: (session: T) => void;
  onContextMenu: (session: T, event: MouseEvent) => void;
  onSearch: () => void;
  workspaceName: (path: string) => string;
  onNewProjectSession: (path: string) => void;
  onRenameProject: (path: string) => void;
  onDeleteProject?: (path: string) => void;
  onHoverStart?: () => void;
  onHoverEnd?: () => void;
}
export default function SessionSidebar<T extends SidebarSession>(props: Props<T>) {
  const { t } = useTranslation();
  const text = (key: string) => t(`sessionSidebar.${key}`);
  const [state, setState] = useState(() => { try { return readSessionGroups(localStorage.getItem(SESSION_GROUP_KEY)); } catch { return readSessionGroups(null); } });
  const [editor, setEditor] = useState<{ id: string | null; name: string } | null>(null);
  const [error, setError] = useState('');
  const [menu, setMenu] = useState<{ kind: 'group' | 'session' | 'project'; id: string; x: number; y: number; theme: CSSProperties } | null>(null);
  const [hover, setHover] = useState<{ title: string; groups: string[]; path: string; count?: string; x: number; y: number; theme: CSSProperties } | null>(null);
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(new Set());
  const hoverTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const card = useRef<HTMLDivElement>(null);
  const cardHovered = useRef(false);
  const onHoverEnd = useRef(props.onHoverEnd);
  onHoverEnd.current = props.onHoverEnd;
  const menuRef = useRef<HTMLDivElement>(null);
  const recent = recentSidebarSessions(props.sessions);
  const projects = sidebarProjects(props.sessions);
  const recentIds = new Set(recent.map(session => session.session_id));
  useEffect(() => { try { localStorage.setItem(SESSION_GROUP_KEY, JSON.stringify(state)); } catch { setError(text('storageError')); } }, [state]);
  useEffect(() => {
    const sync = (event: StorageEvent) => { if (event.key === SESSION_GROUP_KEY) setState(readSessionGroups(event.newValue)); };
    window.addEventListener('storage', sync); return () => window.removeEventListener('storage', sync);
  }, []);
  const cancelHover = () => { if (hoverTimer.current) clearTimeout(hoverTimer.current); hoverTimer.current = null; };
  const hideHover = () => { cancelHover(); setHover(null); };
  useEffect(() => () => cancelHover(), []);
  useEffect(() => {
    const close = (event: Event) => {
      if (event.type === 'scroll' && event.target instanceof Node && card.current?.contains(event.target)) return;
      if (cardHovered.current) { cardHovered.current = false; onHoverEnd.current?.(); }
      hideHover();
      if (!(event.target instanceof Node) || !menuRef.current?.contains(event.target)) setMenu(null);
    };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') { hideHover(); setMenu(null); setEditor(null); } };
    document.addEventListener('pointerdown', close); document.addEventListener('scroll', close, true);
    window.addEventListener('resize', close); window.addEventListener('blur', close); window.addEventListener('keydown', escape);
    return () => { document.removeEventListener('pointerdown', close); document.removeEventListener('scroll', close, true); window.removeEventListener('resize', close); window.removeEventListener('blur', close); window.removeEventListener('keydown', escape); };
  }, []);
  const theme = (element: HTMLElement): CSSProperties => {
    const computed = getComputedStyle(element);
    return { ...Object.fromEntries(['--codex-ink', '--codex-ink-faint', '--codex-line-light', '--codex-paper-card', '--codex-accent', '--codex-accent-soft'].map(key => [key, computed.getPropertyValue(key)])), fontFamily: computed.fontFamily } as CSSProperties;
  };
  const showDetails = (element: HTMLElement, details: Omit<NonNullable<typeof hover>, 'x' | 'y' | 'theme'>) => {
    cancelHover();
    if (menu) return;
    const rect = element.getBoundingClientRect();
    const width = Math.min(360, innerWidth - 16);
    const x = rect.right + 10 + width <= innerWidth ? rect.right + 10 : Math.max(8, rect.left - width - 10);
    const y = Math.max(8, Math.min(rect.top, innerHeight - 230));
    const colors = theme(element);
    hoverTimer.current = setTimeout(() => { setHover({ ...details, x, y, theme: colors }); hoverTimer.current = null; }, 250);
  };
  const leaveDetails = () => { cancelHover(); hoverTimer.current = setTimeout(() => setHover(null), 150); };
  const sessionGroups = (session: T) => [state.groups.find(group => group.id === state.membership[session.session_id])?.name, session.working_directory.trim() ? text('projects') : null, recentIds.has(session.session_id) ? text('recent') : null].filter(Boolean) as string[];
  const openMenu = (event: MouseEvent<HTMLButtonElement>, kind: NonNullable<typeof menu>['kind'], id: string) => {
    event.stopPropagation(); hideHover(); const rect = event.currentTarget.getBoundingClientRect();
    setMenu({ kind, id, x: Math.max(8, Math.min(rect.right, innerWidth - 238)), y: Math.max(8, Math.min(rect.bottom + 6, innerHeight - 308)), theme: theme(event.currentTarget) });
  };
  const assign = (sessionId: string, groupId: string) => { setState(current => { const membership = { ...current.membership }; if (groupId) membership[sessionId] = groupId; else delete membership[sessionId]; return { ...current, membership }; }); setMenu(null); };
  const row = (session: T) => <div key={session.session_id} className={`codex-session-item ${props.activeId === session.session_id ? 'active' : ''}`} role="button" tabIndex={0}
    aria-current={props.activeId === session.session_id ? 'true' : undefined}
    onClick={() => { hideHover(); props.onSelect(session); }}
    onKeyDown={event => { if (event.target === event.currentTarget && ['Enter', ' '].includes(event.key)) { event.preventDefault(); hideHover(); props.onSelect(session); } }}
    onMouseEnter={event => showDetails(event.currentTarget, { title: session.title || text('untitled'), groups: sessionGroups(session), path: session.working_directory })}
    onMouseLeave={leaveDetails} onFocus={event => { if (event.target === event.currentTarget) showDetails(event.currentTarget, { title: session.title || text('untitled'), groups: sessionGroups(session), path: session.working_directory }); }} onBlur={leaveDetails}
    onContextMenu={event => { hideHover(); event.preventDefault(); props.onContextMenu(session, event); }} draggable
    onDragStart={event => { hideHover(); event.dataTransfer.setData('application/x-vivian-work-session', session.session_id); event.dataTransfer.effectAllowed = 'move'; }}>
    <span className="codex-session-title">{session.title || text('untitled')}</span>
    {session.status === 'running' && <Loader2 size={13} className="codex-spin" aria-label={text('running')} />}
    {props.waitingId === session.session_id && <HelpCircle size={14} aria-label={t('mind_inspector.code_waiting_confirm')} />}
    {props.unread.has(session.session_id) && <span className="codex-session-unread-dot" aria-label={text('unread')} />}
    <button type="button" className="session-sidebar-row-action" title={text('assign')} aria-label={`${text('assign')} · ${session.title || text('untitled')}`} onClick={event => openMenu(event, 'session', session.session_id)}><FolderPlus size={13} /></button>
  </div>;
  const section = (id: string, name: string, content: ReactNode, count: number, custom = false) => <details key={id} data-group-id={id} className="session-sidebar-section" open
    onDragOver={event => { if (custom && event.dataTransfer.types.includes('application/x-vivian-work-session')) event.preventDefault(); }}
    onDrop={event => { if (!custom) return; event.preventDefault(); const sessionId = event.dataTransfer.getData('application/x-vivian-work-session'); if (props.sessions.some(session => session.session_id === sessionId)) assign(sessionId, id); }}>
    <summary><ChevronRight size={13} /><span>{name}</span><small>{count}</small>{custom && <button type="button" aria-label={`${text('manageGroup')} · ${name}`} onClick={event => { event.preventDefault(); openMenu(event, 'group', id); }}><Ellipsis size={14} /></button>}</summary>
    <div className="session-sidebar-section-body">{content}</div>
  </details>;
  return <div className="session-sidebar">
    <header className="codex-sidebar-label"><span>{text('sessions')}</span><button type="button" title={text('search')} aria-label={text('search')} onClick={props.onSearch}><Search size={14} /></button><button type="button" title={text('newGroup')} aria-label={text('newGroup')} onClick={() => { setError(''); setEditor({ id: null, name: '' }); }}><Plus size={15} /></button></header>
    {editor && <form className="session-sidebar-editor" onSubmit={event => {
      event.preventDefault(); const name = editor.name.trim();
      if (!name || ['项目', '最近', 'Projects', 'Recent', 'プロジェクト', ...state.groups.filter(group => group.id !== editor.id).map(group => group.name)].some(existing => existing.toLocaleLowerCase() === name.toLocaleLowerCase())) { setError(text('invalidName')); return; }
      setState(current => ({ ...current, groups: editor.id ? current.groups.map(group => group.id === editor.id ? { ...group, name } : group) : [...current.groups, { id: crypto.randomUUID(), name }] })); setEditor(null); setError('');
    }}><input autoFocus required maxLength={100} aria-label={text('groupName')} placeholder={text('groupName')} value={editor.name} onChange={event => setEditor({ ...editor, name: event.target.value })} /><div><button type="submit">{text('save')}</button><button type="button" onClick={() => { setEditor(null); setError(''); }}>{text('cancel')}</button></div></form>}
    {error && <p role="alert" className="session-sidebar-error">{error}</p>}
    {state.groups.map(group => { const sessions = props.sessions.filter(session => state.membership[session.session_id] === group.id).sort((a, b) => b.updated_at - a.updated_at); return section(group.id, group.name, sessions.length ? sessions.map(row) : <p className="session-sidebar-empty">{text('emptyGroup')}</p>, sessions.length, true); })}
    {section('projects', text('projects'), projects.length ? projects.map(project => <details className="session-sidebar-project" key={project.path} open>
      <summary onMouseEnter={event => showDetails(event.currentTarget, { title: props.workspaceName(project.path), groups: [text('projects')], path: project.path, count: t('sessionSidebar.chatCount', { count: project.sessions.length, running: project.sessions.filter(session => session.status === 'running').length }) })} onMouseLeave={leaveDetails}>
        <ChevronRight size={12} /><Folder size={14} /><span>{props.workspaceName(project.path)}</span><button type="button" aria-label={`${text('projectActions')} · ${props.workspaceName(project.path)}`} onClick={event => { event.preventDefault(); openMenu(event, 'project', project.path); }}><Ellipsis size={13} /></button><button type="button" aria-label={`${text('newSession')} · ${props.workspaceName(project.path)}`} onClick={event => { event.preventDefault(); props.onNewProjectSession(project.path); }}><Plus size={13} /></button>
      </summary>{(expandedProjects.has(project.path) ? project.sessions : project.sessions.slice(0, 5)).map(row)}{project.sessions.length > 5 && <button type="button" className="session-sidebar-more" onClick={() => setExpandedProjects(current => { const next = new Set(current); next.has(project.path) ? next.delete(project.path) : next.add(project.path); return next; })}>{text(expandedProjects.has(project.path) ? 'less' : 'more')}</button>}
    </details>) : <p className="session-sidebar-empty">{text('noProjects')}</p>, projects.length)}
    {section('recent', text('recent'), recent.length ? recent.map(row) : <p className="session-sidebar-empty">{text('noSessions')}</p>, recent.length)}
    {hover && createPortal(<div ref={card} className="session-sidebar-details" role="tooltip" data-browser-overlay="true" style={{ ...hover.theme, left: hover.x, top: hover.y }} onMouseEnter={() => { cardHovered.current = true; cancelHover(); props.onHoverStart?.(); }} onMouseLeave={() => { cardHovered.current = false; leaveDetails(); props.onHoverEnd?.(); }}><strong>{hover.title}</strong>{hover.count && <p>{hover.count}</p>}<div><Folder size={15} /><span>{hover.groups.join(' · ') || text('recent')}</span></div><div><FolderPlus size={15} /><span>{hover.path || text('noWorkspace')}</span></div></div>, document.body)}
    {menu && createPortal(<div ref={menuRef} className="session-sidebar-menu" role="menu" data-browser-overlay="true" onMouseEnter={() => { cardHovered.current = true; props.onHoverStart?.(); }} onMouseLeave={() => { cardHovered.current = false; props.onHoverEnd?.(); }} style={{ ...menu.theme, left: menu.x, top: menu.y }}>
      {menu.kind === 'session' && <><strong>{text('assign')}</strong><button type="button" role="menuitem" onClick={() => assign(menu.id, '')}>{text('noCustomGroup')}</button>{state.groups.map(group => <button type="button" role="menuitem" key={group.id} onClick={() => assign(menu.id, group.id)}>{state.membership[menu.id] === group.id ? '✓ ' : ''}{group.name}</button>)}{!state.groups.length && <button type="button" onClick={() => { setMenu(null); setEditor({ id: null, name: '' }); }}>{text('newGroup')}</button>}</>}
      {menu.kind === 'group' && <><button type="button" role="menuitem" onClick={() => { setEditor({ id: menu.id, name: state.groups.find(group => group.id === menu.id)?.name || '' }); setMenu(null); }}>{text('rename')}</button><button type="button" role="menuitem" onClick={() => { setState(current => removeSessionGroup(current, menu.id)); setMenu(null); }}>{text('deleteGroup')}</button><small>{text('keepSessions')}</small></>}
      {menu.kind === 'project' && <><button type="button" role="menuitem" onClick={() => { props.onRenameProject(menu.id); setMenu(null); }}>{text('renameProject')}</button>{props.onDeleteProject && <button type="button" role="menuitem" onClick={() => { props.onDeleteProject?.(menu.id); setMenu(null); }}>{t('mind_inspector.code_delete_workspace')}</button>}</>}
    </div>, document.body)}
  </div>;
}
