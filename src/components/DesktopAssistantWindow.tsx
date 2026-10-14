import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useTranslation } from 'react-i18next';
import { ArrowUpRight, Monitor, NotebookPen, Plus, Rocket, Trash2 } from 'lucide-react';
import { getCharacterId } from '../characterContext';
import { isAssistantPanel } from './assistantPanels';
import './DesktopAssistantWindow.css';

interface Shortcut { id: string; name: string; kind: 'application' | 'website'; target: string }
/** 只填 `bilibili.com` 时补上 https://：后端取图标和 open_url 工具都要求带协议头。 */
const normalizeShortcutTarget = (kind: Shortcut['kind'], target: string) => {
  const trimmed = target.trim();
  if (kind !== 'website' || /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed)) return trimmed;
  return 'https://' + trimmed;
};
interface Context {
  shortcuts: Shortcut[]; quiet_reason: string | null;
}
interface DesktopAssistantProps {
  embedded?: boolean;
  initialCharacter?: string;
  /**
   * 要渲染的子页（`notes` / `shortcuts`）。
   *
   * 这个组件没有总览层：入口是 chat 头部三点下拉与桌宠侧边栏，两边都直接指定一页，
   * 所以页签由调用方决定，组件内不再有切页状态。
   */
  panel?: string;
}
export default function DesktopAssistantWindow({ embedded = false, initialCharacter, panel }: DesktopAssistantProps = {}) {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh'), ja = i18n.language.startsWith('ja');
  const label = (cn: string, jp: string, en: string) => zh ? cn : ja ? jp : en;
  const [character] = useState(initialCharacter ?? getCharacterId() ?? 'vivian');
  const [context, setContext] = useState<Context | null>(null);
  const requestedPanel = panel ?? new URLSearchParams(window.location.search).get('assistant_panel') ?? new URLSearchParams(window.location.search).get('panel');
  const tab = isAssistantPanel(requestedPanel) ? requestedPanel! : 'notes';
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [note, setNote] = useState('');
  const [shortcutName, setShortcutName] = useState('');
  const [shortcutTarget, setShortcutTarget] = useState('');
  const [shortcutKind, setShortcutKind] = useState<Shortcut['kind']>('website');
  const [shortcutFormOpen, setShortcutFormOpen] = useState(false);
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
      try { await refresh(); } catch (e) { if (active) setError(String(e)); }
      finally { inFlight = false; }
    };
    void (async () => {
      for (const name of ['config:saved', 'desktop:shown']) {
        const unlisten = await listen(name, () => void update());
        if (!active) unlisten(); else cleanup.push(unlisten);
      }
      await update();
    })().catch(e => { if (active) setError(String(e)); });
    const timer = window.setInterval(() => {
      void getCurrentWindow().isVisible().then(visible => { if (visible) void update(); }).catch(() => {});
    }, 30_000);
    return () => { mounted.current = false; active = false; clearInterval(timer); cleanup.forEach(unlisten => unlisten()); };
  }, [refresh]);
  const feedback = async (content: string, motion = 'remind') => {
    if (mounted.current) setMessage(content);
    await emit('companion:interaction', { character_id: character, content, motion });
  };
  const run = async (action: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true); setError(''); setMessage('');
    try { await action(); } catch (e) { if (mounted.current) setError(String(e)); }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  };
  const saveShortcuts = async (shortcuts: Shortcut[]) => {
    await invoke('companion_save_shortcuts', { shortcuts }); await refresh();
  };
  useEffect(() => {
    if (!embedded) return;
    // Keep the full chat window interactive across page transitions and StrictMode.
    void invoke('ensure_chat_interactive').catch(() => {});
    return () => {
      void invoke('ensure_chat_interactive').catch(() => {});
    };
  }, [embedded]);
  const saveNote = async () => {
    const value = note.trim();
    await invoke('user_quick_notes_save', { content: value });
    setNote(''); setMessage(label('已保存。', '保存しました。', 'Saved.'));
  };
  const shortcutCount = context?.shortcuts.length ?? 0;
  // 提示条放在页面内部，不是滚动容器顶部：放外面会把 sticky 页头连同返回键一起推下去，
  // 一条转瞬即逝的"处理中"不该挪动整个页面骨架。
  const notices = <>
    {busy && <div className="assistant-notice" role="status"><span className="assistant-working" />{label('处理中', '処理中', 'Working')}</div>}
    {error && <p className="assistant-notice assistant-error" role="alert">{error}</p>}
    {message && <p className="assistant-notice" role="status">{message}</p>}
  </>;
  return <main data-panel={tab} className={'desktop-assistant' + (embedded ? ' desktop-assistant-embedded' : '')}>
    <div className="assistant-aurora" aria-hidden />
    <div className="assistant-scroll">
      <div className="assistant-page">
        <div className="assistant-body">
          {notices}
          {tab === 'notes' && <>
            <div className="assistant-intro"><NotebookPen size={19} /><div><h2>{label('留住此刻的想法', '今のひらめきを残そう', 'A little space for your thoughts')}</h2><p>{label('灵感、琐事，或一句想记住的话。', 'ひらめきも、日々の小さなことも。', 'Ideas, little things, and words to keep.')}</p></div></div>
            <section className="assistant-card assistant-editor"><textarea aria-label={label('随手记', 'メモ', 'Note')} placeholder={label('想到什么就写下来，不用整理。', '思いついたことをそのまま書いて。', 'Write it down as it comes.')} value={note} maxLength={10_000} onChange={e => setNote(e.target.value)} /><div className="assistant-editor-footer"><span className="assistant-count">{[...note].length}<b> / 10,000</b></span><button className="assistant-primary" disabled={busy || !note.trim()} onClick={() => void run(saveNote)}>{label('保存', '保存', 'Save')}</button></div></section>
            <section className="assistant-card assistant-notes-archive"><p className="assistant-card-title">{label('已记录', '保存したメモ', 'Saved notes')}</p><UserQuickNotes /></section>
          </>}
          {tab === 'shortcuts' && <>
            <div className="assistant-shortcut-toolbar"><span className="assistant-muted">{label('共 ' + shortcutCount + ' 个快捷方式', shortcutCount + ' 件のショートカット', shortcutCount + ' shortcuts')}</span><button className="assistant-add-shortcut" aria-label={label('添加快捷方式', 'ショートカットを追加', 'Add shortcut')} onClick={() => { setError(''); setShortcutFormOpen(true); }}><Plus size={20} /></button></div>
            <div className="assistant-shortcut-list">
              {context?.shortcuts.map(item => <div className="assistant-shortcut" key={item.id}>
                <ShortcutIcon key={item.kind + item.target} shortcut={item} />
                <div className="assistant-shortcut-text">
                  <p className="assistant-shortcut-name">{item.name}</p>
                  <p className="assistant-shortcut-target" title={item.target}>{item.target}</p>
                </div>
                <ArrowUpRight size={14} className="assistant-shortcut-go" />
                <button className="assistant-shortcut-open" title={item.target} aria-label={label('打开 ' + item.name, item.name + 'を開く', 'Open ' + item.name)} disabled={busy} onClick={() => void run(async () => { await invoke('companion_launch_shortcut', { shortcutId: item.id, characterId: character }); await feedback(label('已打开 ' + item.name + '。', item.name + 'を開きました。', 'Opened ' + item.name + '.')); })} />
                <button className="assistant-shortcut-remove" aria-label={label('删除 ' + item.name, item.name + 'を削除', 'Remove ' + item.name)} disabled={busy} onClick={() => void run(() => saveShortcuts(context.shortcuts.filter(s => s.id !== item.id)))}><Trash2 size={13} /></button>
              </div>)}
              {!shortcutCount && <div className="assistant-empty">
                <Rocket size={26} />
                <p>{label('还没有快捷方式', 'ショートカットがありません', 'No shortcuts yet')}</p>
                <small>{label('点击右上角的加号，添加网站或应用。', '右上の＋からサイトやアプリを追加。', 'Use the plus button to add a site or app.')}</small>
              </div>}
            </div>

          </>}
        </div>
      </div>
    </div>
    <dialog ref={shortcutDialog} className="assistant-shortcut-dialog" aria-labelledby="assistant-shortcut-dialog-title" onCancel={e => { e.preventDefault(); if (!busy) setShortcutFormOpen(false); }} onClick={e => { if (e.target === e.currentTarget && !busy) { const rect = e.currentTarget.getBoundingClientRect(); if (e.clientX < rect.left || e.clientX > rect.right || e.clientY < rect.top || e.clientY > rect.bottom) setShortcutFormOpen(false); } }}>
            <form className="assistant-card assistant-shortcut-form" onSubmit={e => { e.preventDefault(); if (busy || !context || !shortcutName.trim() || !shortcutTarget.trim()) return; void run(async () => { await saveShortcuts([...(context?.shortcuts ?? []), { id: crypto.randomUUID(), name: shortcutName.trim(), kind: shortcutKind, target: normalizeShortcutTarget(shortcutKind, shortcutTarget) }]); setShortcutName(''); setShortcutTarget(''); setShortcutFormOpen(false); }); }}>
              <p id="assistant-shortcut-dialog-title" className="assistant-card-title">{label('添加快捷方式', 'ショートカットを追加', 'Add shortcut')}</p>
              <div className="assistant-kind-switch">{(['website', 'application'] as const).map(kind => <button type="button" key={kind} disabled={busy} aria-pressed={shortcutKind === kind} onClick={() => setShortcutKind(kind)}>{kind === 'website' ? label('网站', 'サイト', 'Website') : label('应用', 'アプリ', 'App')}</button>)}</div>
              <input autoFocus required maxLength={80} aria-label={label('快捷方式名称', 'ショートカット名', 'Shortcut name')} placeholder={label('名称', '名前', 'Name')} value={shortcutName} onChange={e => setShortcutName(e.target.value)} />
              <input required aria-label={label('快捷方式目标', 'ショートカットのリンク先', 'Shortcut target')} placeholder={shortcutKind === 'website' ? 'https://…' : 'C:\\…\\app.exe'} value={shortcutTarget} onChange={e => setShortcutTarget(e.target.value)} />
              {error && <p className="assistant-review-error" role="alert">{error}</p>}
              <div className="assistant-modal-actions"><button type="button" className="assistant-ghost-button" disabled={busy} onClick={() => setShortcutFormOpen(false)}>{label('取消', 'キャンセル', 'Cancel')}</button><button type="submit" className="assistant-primary" disabled={busy || !context || !shortcutName.trim() || !shortcutTarget.trim()}>{busy ? label('添加中…', '追加中…', 'Adding…') : label('添加', '追加', 'Add')}</button></div>
            </form>
    </dialog>
  </main>;
}

function UserQuickNotes() {
  const { i18n } = useTranslation();
  const label = (zh: string, ja: string, en: string) => i18n.language.startsWith('zh') ? zh : i18n.language.startsWith('ja') ? ja : en;
  const [notes, setNotes] = useState<Array<{ id: string; title: string; content: string; created_at: number }>>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    let active = true, revision = 0;
    let cleanup: (() => void) | undefined;
    const update = () => {
      const request = ++revision;
      void invoke<typeof notes>('user_quick_notes_list')
        .then(next => { if (active && request === revision) setNotes(next); }).catch(e => { if (active && request === revision) setError(String(e)); });
    };
    setError('');
    void listen('user_quick_notes:changed', update).then(unlisten => { if (active) { cleanup = unlisten; update(); } else unlisten(); }).catch(e => { if (active) setError(String(e)); });
    return () => { active = false; cleanup?.(); };
  }, []);
  return <div className="assistant-notes-list">
    {!notes.length && <p className="assistant-muted">{label('暂无随手记', 'メモはまだありません', 'No notes yet')}</p>}
    {notes.map(note => <details className="assistant-details" key={note.id}>
      <summary>{note.title}</summary>
      <p style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{note.content}</p>
      <button className="assistant-icon-button" aria-label={label('删除随手记', 'メモを削除', 'Delete note')} disabled={busy} onClick={() => {
        setBusy(true); setError('');
        void invoke('user_quick_notes_delete', { id: note.id }).then(() => setNotes(previous => previous.filter(item => item.id !== note.id)))
          .catch(e => setError(String(e))).finally(() => setBusy(false));
      }}><Trash2 size={16} /></button>
    </details>)}
    {error && <p role="alert" className="assistant-review-error">{error}</p>}
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
