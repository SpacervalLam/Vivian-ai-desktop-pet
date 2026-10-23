import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useTranslation } from 'react-i18next';
import { ArrowUpRight, ChevronDown, Monitor, NotebookPen, Pencil, Plus, Rocket, Search, Trash2, X } from 'lucide-react';
import { getCharacterId } from '../characterContext';
import { isAssistantPanel } from './assistantPanels';
import { groupShortcuts, showsGroupHeaders, shortcutGroupNames } from '../utils/shortcutGroups';
import './DesktopAssistantPanel.css';

const showErrorToast = (error: unknown) => {
  void emit('toast:show', {
    message: error instanceof Error ? error.message : String(error),
    type: 'error', duration: 5000, key: Date.now(),
  }).catch(error => console.warn('[DesktopAssistantPanel] 显示错误提示失败:', error));
};

interface Shortcut { id: string; name: string; kind: 'application' | 'website'; target: string; group: string }
/** 只填 `bilibili.com` 时补上 https://：后端取图标和 open_url 工具都要求带协议头。 */
const normalizeShortcutTarget = (kind: Shortcut['kind'], target: string) => {
  const trimmed = target.trim();
  if (kind !== 'website' || /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed)) return trimmed;
  return 'https://' + trimmed;
};
/** `companion_desktop_context` 只返回 shortcuts：分组归并与搜索都在前端完成。 */
interface Context {
  shortcuts: Shortcut[];
}
interface DesktopAssistantProps {
  initialCharacter?: string;
  /**
   * 要渲染的子页（`notes` / `shortcuts`）。
   *
   * 这个面板只有一个宿主：聊天窗口（`ChatWindow`）把它挂进 `assistant` 视图。
   * 没有总览层、没有独立窗口，页签由调用方决定，组件内不保留切页状态。
   */
  panel?: string;
}
export default function DesktopAssistantPanel({ initialCharacter, panel }: DesktopAssistantProps = {}) {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh'), ja = i18n.language.startsWith('ja');
  const label = (cn: string, jp: string, en: string) => zh ? cn : ja ? jp : en;
  const [character] = useState(initialCharacter ?? getCharacterId() ?? 'vivian');
  const [context, setContext] = useState<Context | null>(null);
  const requestedPanel = panel ?? new URLSearchParams(window.location.search).get('assistant_panel') ?? new URLSearchParams(window.location.search).get('panel');
  const tab = isAssistantPanel(requestedPanel) ? requestedPanel! : 'notes';
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [note, setNote] = useState('');
  const [shortcutName, setShortcutName] = useState('');
  const [shortcutTarget, setShortcutTarget] = useState('');
  const [shortcutKind, setShortcutKind] = useState<Shortcut['kind']>('website');
  const [shortcutFormOpen, setShortcutFormOpen] = useState(false);
  /** 表单里的分组名；空串即未分组。 */
  const [shortcutGroup, setShortcutGroup] = useState('');
  /** 非空表示弹窗处于编辑态，提交时按 id 覆盖而不是追加。 */
  const [editingId, setEditingId] = useState<string | null>(null);
  /** 列表搜索词：命中名称 / 目标 / 分组。 */
  const [shortcutSearch, setShortcutSearch] = useState('');
  /** 折叠的分组 key；只在非搜索态生效（搜索时全部展开，否则命中藏在折叠里等于没搜到）。 */
  const [collapsedGroups, setCollapsedGroups] = useState<string[]>([]);
  const shortcutDialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const dialog = shortcutDialog.current;
    if (!dialog) return;
    if (shortcutFormOpen && tab === 'shortcuts') {
      dialog.showModal();
      dialog.querySelector<HTMLInputElement>('input')?.focus();
    }
    else dialog.close();
    return () => dialog.close();
  }, [shortcutFormOpen, tab]);
  const mounted = useRef(false);
  const refresh = useCallback(async () => {
    const next = await invoke<Context>('companion_desktop_context');
    if (mounted.current) setContext(next);
  }, []);
  useEffect(() => {
    mounted.current = true;
    let active = true;
    let inFlight = false;
    const cleanup: UnlistenFn[] = [];
    const update = async () => {
      if (!active || inFlight) return;
      inFlight = true;
      try { await refresh(); } catch (e) { if (active) showErrorToast(e); }
      finally { inFlight = false; }
    };
    void (async () => {
      for (const name of ['config:saved', 'desktop:shown']) {
        const unlisten = await listen(name, () => void update());
        if (!active) unlisten(); else cleanup.push(unlisten);
      }
      await update();
    })().catch(e => { if (active) showErrorToast(e); });
    const timer = window.setInterval(() => {
      void getCurrentWindow().isVisible().then(visible => { if (visible) void update(); }).catch(() => {});
    }, 30_000);
    return () => { mounted.current = false; active = false; clearInterval(timer); cleanup.forEach(unlisten => unlisten()); };
  }, [refresh]);
  const feedback = async (content: string, motion = 'remind') => {
    await emit('companion:interaction', { character_id: character, content, motion });
  };
  const run = async (action: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true);
    try { await action(); } catch (e) { if (mounted.current) showErrorToast(e); }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  };
  const saveShortcuts = async (shortcuts: Shortcut[]) => {
    await invoke('companion_save_shortcuts', { shortcuts }); await refresh();
  };
  // 面板始终挂在聊天窗口里：进出子页都要把整窗恢复成可点击（StrictMode 会走 setup/cleanup/setup）。
  useEffect(() => {
    void invoke('ensure_chat_interactive').catch(() => {});
    return () => {
      void invoke('ensure_chat_interactive').catch(() => {});
    };
  }, []);
  const saveNote = async () => {
    const value = note.trim();
    await invoke('user_quick_notes_save', { content: value });
    setNote('');
  };
  const shortcutCount = context?.shortcuts.length ?? 0;
  /** 打开弹窗：不带参数是新增，带条目是编辑（分组、名称、链接都能改）。 */
  const openShortcutForm = (item?: Shortcut) => {
    setShortcutName(item?.name ?? '');
    setShortcutTarget(item?.target ?? '');
    setShortcutKind(item?.kind ?? 'website');
    setShortcutGroup(item?.group ?? '');
    setEditingId(item?.id ?? null);
    setShortcutFormOpen(true);
  };
  const closeShortcutForm = () => {
    setShortcutFormOpen(false);
    setEditingId(null);
  };
  const toggleGroup = (key: string) => {
    setCollapsedGroups(previous => previous.includes(key)
      ? previous.filter(item => item !== key)
      : [...previous, key]);
  };
  const searching = shortcutSearch.trim().length > 0;
  const sections = groupShortcuts<Shortcut>(context?.shortcuts ?? [], shortcutSearch);
  const groupHeaders = showsGroupHeaders(sections);
  const visibleCount = sections.reduce((sum, section) => sum + section.items.length, 0);
  const groupCount = shortcutGroupNames(context?.shortcuts ?? []).length;
  return <main data-panel={tab} className="desktop-assistant">
    <div className="assistant-scroll">
      <div className="assistant-page">
        <div className="assistant-body">
          {tab === 'notes' && <>
            <div className="assistant-intro"><NotebookPen size={19} /><div><h2>{label('留住此刻的想法', '今のひらめきを残そう', 'A little space for your thoughts')}</h2><p>{label('灵感、琐事，或一句想记住的话。', 'ひらめきも、日々の小さなことも。', 'Ideas, little things, and words to keep.')}</p></div></div>
            <section className="assistant-card assistant-editor"><textarea aria-label={label('随手记', 'メモ', 'Note')} placeholder={label('想到什么就写下来，不用整理。', '思いついたことをそのまま書いて。', 'Write it down as it comes.')} value={note} maxLength={10_000} onChange={e => setNote(e.target.value)} /><div className="assistant-editor-footer"><span className="assistant-count">{[...note].length}<b> / 10,000</b></span><button className="assistant-primary" disabled={busy || !note.trim()} onClick={() => void run(saveNote)}>{label('保存', '保存', 'Save')}</button></div></section>
            <section className="assistant-card assistant-notes-archive"><p className="assistant-card-title">{label('已记录', '保存したメモ', 'Saved notes')}</p><UserQuickNotes /></section>
          </>}
          {tab === 'shortcuts' && <>
            <div className="assistant-shortcut-toolbar">
              <label className="assistant-shortcut-search">
                <Search size={14} aria-hidden />
                <input value={shortcutSearch} maxLength={120} aria-label={label('搜索快捷方式', 'ショートカットを検索', 'Search shortcuts')} placeholder={label('搜索名称、网址或分组', '名前・URL・グループを検索', 'Search name, link or group')} onChange={e => setShortcutSearch(e.target.value)} />
                {searching && <button type="button" className="assistant-shortcut-search-clear" aria-label={label('清空搜索', '検索をクリア', 'Clear search')} onClick={() => setShortcutSearch('')}><X size={11} /></button>}
              </label>
              <button className="assistant-add-shortcut" aria-label={label('添加快捷方式', 'ショートカットを追加', 'Add shortcut')} onClick={() => openShortcutForm()}><Plus size={20} /></button>
            </div>
            <p className="assistant-shortcut-meta">{searching
              ? label('找到 ' + visibleCount + ' 个', visibleCount + ' 件見つかりました', visibleCount + ' found')
              : label('共 ' + shortcutCount + ' 个快捷方式' + (groupCount ? ' · ' + groupCount + ' 个分组' : ''), '全 ' + shortcutCount + ' 件' + (groupCount ? ' · ' + groupCount + ' グループ' : ''), shortcutCount + ' shortcut' + (shortcutCount === 1 ? '' : 's') + (groupCount ? ' · ' + groupCount + ' group' + (groupCount === 1 ? '' : 's') : ''))}</p>
            <div className="assistant-shortcut-list">
              {sections.map(section => {
                const collapsed = !searching && collapsedGroups.includes(section.key);
                return <section className="assistant-shortcut-group" key={section.key} data-collapsed={collapsed ? '' : undefined}>
                  {groupHeaders && <button type="button" className="assistant-shortcut-group-head" aria-expanded={!collapsed} onClick={() => toggleGroup(section.key)}>
                    <ChevronDown size={14} className="assistant-shortcut-group-chevron" aria-hidden />
                    <span className="assistant-shortcut-group-name">{section.name || label('未分组', '未分類', 'Ungrouped')}</span>
                    <span className="assistant-shortcut-group-count">{section.items.length}</span>
                  </button>}
                  {!collapsed && <div className="assistant-shortcut-group-items">
                    {section.items.map(item => <div className="assistant-shortcut" key={item.id}>
                      <ShortcutIcon key={item.kind + item.target} shortcut={item} />
                      <div className="assistant-shortcut-text">
                        <p className="assistant-shortcut-name">{item.name}</p>
                        <p className="assistant-shortcut-target" title={item.target}>{item.target}</p>
                      </div>
                      <ArrowUpRight size={14} className="assistant-shortcut-go" />
                      <button className="assistant-shortcut-open" title={item.target} aria-label={label('打开 ' + item.name, item.name + 'を開く', 'Open ' + item.name)} disabled={busy} onClick={() => void run(async () => { await invoke('companion_launch_shortcut', { shortcutId: item.id, characterId: character }); try { await feedback(label('已打开 ' + item.name + '。', item.name + 'を開きました。', 'Opened ' + item.name + '.')); } finally { await invoke('close_chat_window'); } })} />
                      <button className="assistant-shortcut-edit" aria-label={label('编辑 ' + item.name, item.name + 'を編集', 'Edit ' + item.name)} disabled={busy} onClick={() => openShortcutForm(item)}><Pencil size={13} /></button>
                      <button className="assistant-shortcut-remove" aria-label={label('删除 ' + item.name, item.name + 'を削除', 'Remove ' + item.name)} disabled={busy} onClick={() => void run(() => saveShortcuts((context?.shortcuts ?? []).filter(s => s.id !== item.id)))}><Trash2 size={13} /></button>
                    </div>)}
                  </div>}
                </section>;
              })}
              {!shortcutCount && <div className="assistant-empty">
                <Rocket size={26} />
                <p>{label('还没有快捷方式', 'ショートカットがありません', 'No shortcuts yet')}</p>
                <small>{label('点击右上角的加号，添加网站或应用。', '右上の＋からサイトやアプリを追加。', 'Use the plus button to add a site or app.')}</small>
              </div>}
              {!!shortcutCount && !visibleCount && <div className="assistant-empty">
                <Search size={22} />
                <p>{label('没有匹配的快捷方式', '一致するショートカットがありません', 'No matching shortcuts')}</p>
                <small>{label('换个关键词，或换个分组名试试。', '別のキーワードを試してみて。', 'Try another keyword or group name.')}</small>
              </div>}
            </div>

          </>}
        </div>
      </div>
    </div>
    <dialog ref={shortcutDialog} className="assistant-shortcut-dialog" aria-labelledby="assistant-shortcut-dialog-title" onCancel={e => { e.preventDefault(); if (!busy) closeShortcutForm(); }} onClick={e => { if (e.target === e.currentTarget && !busy) { const rect = e.currentTarget.getBoundingClientRect(); if (e.clientX < rect.left || e.clientX > rect.right || e.clientY < rect.top || e.clientY > rect.bottom) closeShortcutForm(); } }}>
            <form className="assistant-card assistant-shortcut-form" onSubmit={e => {
              e.preventDefault();
              if (busy || !context || !shortcutName.trim() || !shortcutTarget.trim()) return;
              void run(async () => {
                const payload: Shortcut = {
                  id: editingId ?? crypto.randomUUID(),
                  name: shortcutName.trim(),
                  kind: shortcutKind,
                  target: normalizeShortcutTarget(shortcutKind, shortcutTarget),
                  group: shortcutGroup.trim(),
                };
                const current = context.shortcuts ?? [];
                await saveShortcuts(editingId ? current.map(item => item.id === editingId ? payload : item) : [...current, payload]);
                setShortcutName(''); setShortcutTarget(''); setShortcutGroup(''); setEditingId(null); setShortcutFormOpen(false);
              });
            }}>
              <p id="assistant-shortcut-dialog-title" className="assistant-card-title">{editingId ? label('编辑快捷方式', 'ショートカットを編集', 'Edit shortcut') : label('添加快捷方式', 'ショートカットを追加', 'Add shortcut')}</p>
              <div className="assistant-kind-switch">{(['website', 'application'] as const).map(kind => <button type="button" key={kind} disabled={busy} aria-pressed={shortcutKind === kind} onClick={() => setShortcutKind(kind)}>{kind === 'website' ? label('网站', 'サイト', 'Website') : label('应用', 'アプリ', 'App')}</button>)}</div>
              <input autoFocus required maxLength={80} aria-label={label('快捷方式名称', 'ショートカット名', 'Shortcut name')} placeholder={label('名称', '名前', 'Name')} value={shortcutName} onChange={e => setShortcutName(e.target.value)} />
              <input required aria-label={label('快捷方式目标', 'ショートカットのリンク先', 'Shortcut target')} placeholder={shortcutKind === 'website' ? 'https://…' : 'C:\\…\\app.exe'} value={shortcutTarget} onChange={e => setShortcutTarget(e.target.value)} />
              <input list="assistant-shortcut-group-names" maxLength={40} aria-label={label('分组', 'グループ', 'Group')} placeholder={label('分组（可留空）', 'グループ（空でも可）', 'Group (optional)')} value={shortcutGroup} onChange={e => setShortcutGroup(e.target.value)} />
              <datalist id="assistant-shortcut-group-names">{shortcutGroupNames(context?.shortcuts ?? []).map(name => <option key={name} value={name} />)}</datalist>
              <div className="assistant-modal-actions"><button type="button" className="assistant-ghost-button" disabled={busy} onClick={closeShortcutForm}>{label('取消', 'キャンセル', 'Cancel')}</button><button type="submit" className="assistant-primary" disabled={busy || !context || !shortcutName.trim() || !shortcutTarget.trim()}>{busy ? label('保存中…', '保存中…', 'Saving…') : label('保存', '保存', 'Save')}</button></div>
            </form>
    </dialog>
  </main>;
}

function UserQuickNotes() {
  const { i18n } = useTranslation();
  const label = (zh: string, ja: string, en: string) => i18n.language.startsWith('zh') ? zh : i18n.language.startsWith('ja') ? ja : en;
  const [notes, setNotes] = useState<Array<{ id: string; title: string; content: string; created_at: number }>>([]);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let active = true, revision = 0;
    let cleanup: (() => void) | undefined;
    const update = () => {
      const request = ++revision;
      void invoke<typeof notes>('user_quick_notes_list')
        .then(next => { if (active && request === revision) setNotes(next); }).catch(e => { if (active && request === revision) showErrorToast(e); });
    };
    void listen('user_quick_notes:changed', update).then(unlisten => { if (active) { cleanup = unlisten; update(); } else unlisten(); }).catch(e => { if (active) showErrorToast(e); });
    return () => { active = false; cleanup?.(); };
  }, []);
  return <div className="assistant-notes-list">
    {!notes.length && <p className="assistant-muted">{label('暂无随手记', 'メモはまだありません', 'No notes yet')}</p>}
    {notes.map(note => <details className="assistant-details" key={note.id}>
      <summary>{note.title}</summary>
      <p style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{note.content}</p>
      <button className="assistant-icon-button" aria-label={label('删除随手记', 'メモを削除', 'Delete note')} disabled={busy} onClick={() => {
        setBusy(true);
        void invoke('user_quick_notes_delete', { id: note.id }).then(() => setNotes(previous => previous.filter(item => item.id !== note.id)))
          .catch(showErrorToast).finally(() => setBusy(false));
      }}><Trash2 size={16} /></button>
    </details>)}
  </div>;
}

const shortcutIconCache = new Map<string, Promise<string | null>>();
function ShortcutIcon({ shortcut }: { shortcut: Shortcut }) {
  const [icon, setIcon] = useState<string | null>(null);
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh'), ja = i18n.language.startsWith('ja');
  const fallbackHint = zh ? '没取到这个站点的图标，暂时用首字母代替' : ja ? 'サイトのアイコンを取得できませんでした' : 'Could not load this site icon';
  useEffect(() => {
    let active = true;
    const key = shortcut.id + ':' + shortcut.kind + ':' + shortcut.target;
    let request = shortcutIconCache.get(key);
    if (!request) {
      // 后端失败原因（代理不通 / 站点没有 favicon）只留在日志里，控制台这行是给人排查用的
      request = invoke<string>('companion_shortcut_icon', { shortcutId: shortcut.id })
        .catch((error) => { shortcutIconCache.delete(key); console.debug('[shortcut-icon]', shortcut.name, error); return null; });
      if (shortcutIconCache.size >= 60) shortcutIconCache.delete(shortcutIconCache.keys().next().value!);
      shortcutIconCache.set(key, request);
    }
    void request.then(value => { if (active) setIcon(value); });
    return () => { active = false; };
  }, [shortcut.id, shortcut.kind, shortcut.target]);
  return <span className="assistant-shortcut-icon" aria-hidden="true">{icon ? <img src={icon} alt="" onError={() => setIcon(null)} /> : shortcut.kind === 'application' ? <Monitor size={20} /> : <span title={fallbackHint}>{shortcut.name.trim().slice(0, 1).toUpperCase()}</span>}</span>;
}
