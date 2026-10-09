import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useTranslation } from 'react-i18next';
import { ArrowUpRight, ChevronRight, Dice5, Settings, Mic, Monitor, NotebookPen, Plus, Rocket, Trash2 } from 'lucide-react';
import { getCharacterId } from '../characterContext';
import { playRps, rollDice, type Hand } from '../utils/companionGames';
import { isAssistantOverview, isAssistantPanel, assistantPanelTitle } from './assistantPanels';
import './DesktopAssistantWindow.css';

interface Shortcut { id: string; name: string; kind: 'application' | 'website'; target: string }
interface Context {
  shortcuts: Shortcut[]; quiet_reason: string | null;
}
interface DesktopAssistantProps {
  embedded?: boolean;
  initialCharacter?: string;
  initialPanel?: string;
  /**
   * 受控页签。chat window 头部要按它决定标题和返回层级（这个页面自己已经没有标题栏了），
   * 所以页签的真源在头部那侧；不传时（预览台 / 独立渲染）回落到组件内状态。
   */
  panel?: string;
  /** 组件内切页后向上汇报，头部据此更新标题。 */
  onPanelChange?: (panel: string) => void;
}
export default function DesktopAssistantWindow({ embedded = false, initialCharacter, initialPanel, panel, onPanelChange }: DesktopAssistantProps = {}) {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh'), ja = i18n.language.startsWith('ja');
  const label = (cn: string, jp: string, en: string) => zh ? cn : ja ? jp : en;
  const [character] = useState(initialCharacter ?? getCharacterId() ?? 'vivian');
  const [context, setContext] = useState<Context | null>(null);
  const requestedPanel = panel ?? initialPanel ?? new URLSearchParams(window.location.search).get('panel');
  const [localPanel, setLocalPanel] = useState(isAssistantPanel(requestedPanel) ? requestedPanel! : 'daily');
  const controlled = panel !== undefined;
  const tab = controlled ? panel! : localPanel;
  const [busy, setBusy] = useState(false);
  const busyRef = useRef(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [note, setNote] = useState('');
  const [preference, setPreference] = useState('');
  const [shortcutName, setShortcutName] = useState('');
  const [shortcutTarget, setShortcutTarget] = useState('');
  const [shortcutKind, setShortcutKind] = useState<Shortcut['kind']>('website');
  const [roll, setRoll] = useState<{ player: number; companion: number; outcome: string } | null>(null);
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
  const navigate = (next: string) => {
    if (!controlled) setLocalPanel(next);
    onPanelChange?.(next);
    setMessage('');
    setError('');
  };
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
  const look = async () => {
    const result = await invoke<{ cancelled?: boolean; handled?: boolean; saved_path?: string; copied?: boolean }>('companion_screen_analyze', { characterId: character });
    if (result.cancelled) return;
    const saved = label('已保存至系统截图目录', 'システムのスクリーンショットフォルダーに保存しました', 'Saved to your system Screenshots folder');
    if (result.handled) {
      if (result.saved_path) void emit('toast:show', { message: saved, type: 'success', duration: 2500, key: Date.now() });
      return;
    }
    if (result.copied) setMessage(label('已复制到剪贴板', 'クリップボードにコピーしました', 'Copied to clipboard'));
    if (result.saved_path) await feedback(saved, 'happy');
  };
  const gameResult = async (game: ReturnType<typeof rollDice> | ReturnType<typeof playRps>) => {
    const outcome = game.outcome === 'draw' ? label('平局', '引き分け', 'Draw') : game.outcome === 'win' ? label('你赢了', 'あなたの勝ち', 'You win') : label('角色赢了', 'キャラの勝ち', 'Companion wins');
    const handLabel = (value: number | string) => typeof value === 'number' ? value : ({ rock: '✊', paper: '✋', scissors: '✌️' })[value as Hand];
    const motion = typeof game.companion === 'string' ? `rps-${game.companion}` : game.outcome === 'draw' ? 'think' : 'happy';
    await feedback(`${handLabel(game.player)} vs ${handLabel(game.companion)} · ${outcome}`, motion);
  };
  useEffect(() => {
    if (!embedded) return;
    // Keep the full chat window interactive across page transitions and StrictMode.
    void invoke('ensure_chat_interactive').catch(() => {});
    return () => {
      void invoke('ensure_chat_interactive').catch(() => {});
    };
  }, [embedded]);
  const hands = (['rock', 'scissors', 'paper'] as const).map(hand => ({
    hand,
    glyph: ({ rock: '✊', paper: '✋', scissors: '✌️' })[hand],
    name: label(({ rock: '石头', scissors: '剪刀', paper: '布' })[hand], hand, hand),
  }));
  const overview = isAssistantOverview(tab);
  // 分区名走共享目录：chat 头部的标题用的是同一份，两处各写一遍必然漂移。
  const panelName = (id: string) => assistantPanelTitle(id, i18n.language) ?? id;
  const saveNote = async () => {
    const value = note.trim();
    await invoke('user_quick_notes_save', { content: value });
    setNote(''); setMessage(label('已保存。', '保存しました。', 'Saved.'));
  };
  const shortcutCount = context?.shortcuts.length ?? 0;
  const rollOutcome = roll ? (roll.outcome === 'draw' ? label('平局', '引き分け', 'Draw') : roll.outcome === 'win' ? label('你赢了', 'あなたの勝ち', 'You win') : label('角色赢了', 'キャラの勝ち', 'Companion wins')) : '';
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
      {overview ? <div className="assistant-page assistant-page-overview">
        {notices}
        <p className="assistant-label">{label('工具', 'ツール', 'Tools')}</p>
        <div className="assistant-grid">
          <button className="assistant-tile" data-action="notes" disabled={busy} onClick={() => navigate('notes')}><NotebookPen size={20} className="assistant-icon assistant-hue-amber" /><span className="assistant-tile-label">{panelName('notes')}</span><ChevronRight size={15} className="assistant-chevron" /></button>
          <button className="assistant-tile" data-action="screen" title={label('框选屏幕区域并分析', 'スクリーンショットを視覚モデルへ送信', 'Send a screenshot to your vision model')} disabled={busy} onClick={() => void run(look)}><Monitor size={20} className="assistant-icon assistant-hue-cyan" /><span className="assistant-tile-label">{label('截图分析', '画面分析', 'Analyze screen')}</span></button>
          <button className="assistant-tile" disabled={busy} onClick={() => navigate('shortcuts')}><Rocket size={20} className="assistant-icon assistant-hue-mint" /><span className="assistant-tile-label">{panelName('shortcuts')}</span><ChevronRight size={15} className="assistant-chevron" /></button>
          <button className="assistant-tile" disabled={busy} onClick={() => navigate('games')}><Dice5 size={20} className="assistant-icon assistant-hue-rose" /><span className="assistant-tile-label">{panelName('games')}</span><ChevronRight size={15} className="assistant-chevron" /></button>
        </div>
        <div className="assistant-stack">
          <button className="assistant-row" disabled={busy} onClick={() => void run(() => emit('tray:menu_action', { action: 'settings', character_id: character }))}><Settings size={20} className="assistant-icon assistant-hue-cyan" /><span className="assistant-row-label">{label('设置', '設定', 'Settings')}</span><ChevronRight size={16} className="assistant-chevron" /></button>
        </div>
      </div> : <div className="assistant-page">
        <div className="assistant-body">
          {notices}
          {tab === 'notes' && <>
            <div className="assistant-intro"><NotebookPen size={19} /><div><h2>{label('留住此刻的想法', '今のひらめきを残そう', 'A little space for your thoughts')}</h2><p>{label('灵感、琐事，或一句想记住的话。', 'ひらめきも、日々の小さなことも。', 'Ideas, little things, and words to keep.')}</p></div></div>
            <section className="assistant-card assistant-editor"><textarea aria-label="Note" placeholder={label('想到什么就写下来，不用整理。', '思いついたことをそのまま書いて。', 'Write it down as it comes.')} value={note} maxLength={10_000} onChange={e => setNote(e.target.value)} /><div className="assistant-editor-footer"><span className="assistant-count">{[...note].length}<b> / 10,000</b></span><button className="assistant-primary" disabled={busy || !note.trim()} onClick={() => void run(saveNote)}>{label('保存', '保存', 'Save')}</button></div></section>
            <section className="assistant-card assistant-notes-archive"><p className="assistant-card-title">{label('已记录', '保存したメモ', 'Saved notes')}</p><UserQuickNotes /></section>
          </>}
          {tab === 'games' && <>
            <div className="assistant-game-intro">
              <div><span className="assistant-game-tag">PLAY TIME</span><h2>{label('一起玩一会儿', 'ちょっと遊ぼう', 'Let’s take a play break')}</h2><p>{label('把快乐，贴进今天的手账。', '今日のノートに、小さな楽しさを。', 'A little happiness for today’s journal.')}</p></div>
              <img className="assistant-game-sticker" src={`/stickers/${character === 'nana' ? 'nana' : 'vivian'}_cheer_01-v2.webp`} alt="" aria-hidden="true" />
            </div>
            <p className="assistant-label">{label('猜拳', 'じゃんけん', 'Rock, paper, scissors')}</p>
            <div className="assistant-hands">{hands.map(({ hand, glyph, name }) => <div className="assistant-rps-cell" key={hand}>
              <button className="assistant-rps" disabled={busy} aria-label={name} onClick={() => void run(() => gameResult(playRps(hand)))}><span className="assistant-rps-glyph">{glyph}</span></button>
              <span className="assistant-rps-caption">{name}</span>
            </div>)}</div>
            <p className="assistant-label">{label('掷骰子', 'サイコロ', 'Roll dice')} · 1–100</p>
            <section className="assistant-card assistant-dice">
              <div className="assistant-dice-read">
                <p className={'assistant-dice-value' + (roll ? '' : ' assistant-dice-value-empty')}>{roll ? roll.player : '—'}</p>
                <p className="assistant-dice-caption">{roll ? label(`你 ${roll.player} · 我 ${roll.companion} · ${rollOutcome}`, `あなた ${roll.player} · 私 ${roll.companion} · ${rollOutcome}`, `You ${roll.player} · me ${roll.companion} · ${rollOutcome}`) : label('还没掷过骰子', 'まだ振っていない', 'Not rolled yet')}</p>
              </div>
              <button disabled={busy} aria-label={label('掷骰子', 'サイコロを振る', 'Roll dice')} onClick={() => void run(async () => { const next = rollDice(); setRoll(next); await gameResult(next); })}>🎲</button>
            </section>
            <p className="assistant-footnote">{label('再来一局？今天的好运还在继续。', 'もう一回？今日の幸運はまだ続くよ。', 'One more round? There’s more luck to come.')}</p>
          </>}
          {tab === 'preferences' && <section className="assistant-card assistant-editor"><textarea aria-label="Preference" placeholder={label('她应该记住的事，写在这里。', '覚えてほしいことをここに。', 'What she should remember.')} maxLength={1000} value={preference} onChange={e => setPreference(e.target.value)} /><div className="assistant-editor-footer"><span className="assistant-count">{[...preference].length}<b> / 1,000</b></span><button className="assistant-primary" disabled={busy || !preference.trim()} onClick={() => void run(async () => { await invoke('companion_remember_preference', { characterId: character, content: preference }); setPreference(''); await feedback(label('已记住。', '覚えました。', 'Remembered.')); })}>{label('记住', '保存', 'Remember')}</button></div></section>}
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
                <button className="assistant-shortcut-remove" aria-label={'Remove ' + item.name} disabled={busy} onClick={() => void run(() => saveShortcuts(context.shortcuts.filter(s => s.id !== item.id)))}><Trash2 size={13} /></button>
              </div>)}
              {!shortcutCount && <div className="assistant-empty">
                <Rocket size={26} />
                <p>{label('还没有快捷方式', 'ショートカットがありません', 'No shortcuts yet')}</p>
                <small>{label('点击右上角的加号，添加网站或应用。', '右上の＋からサイトやアプリを追加。', 'Use the plus button to add a site or app.')}</small>
              </div>}
            </div>

          </>}
          {tab === 'voice' && <VoiceDiagnostics />}
        </div>
      </div>}
    </div>
    <dialog ref={shortcutDialog} className="assistant-shortcut-dialog" aria-labelledby="assistant-shortcut-dialog-title" onCancel={e => { e.preventDefault(); if (!busy) setShortcutFormOpen(false); }} onClick={e => { if (e.target === e.currentTarget && !busy) { const rect = e.currentTarget.getBoundingClientRect(); if (e.clientX < rect.left || e.clientX > rect.right || e.clientY < rect.top || e.clientY > rect.bottom) setShortcutFormOpen(false); } }}>
            <form className="assistant-card assistant-shortcut-form" onSubmit={e => { e.preventDefault(); if (busy || !context || !shortcutName.trim() || !shortcutTarget.trim()) return; void run(async () => { await saveShortcuts([...(context?.shortcuts ?? []), { id: crypto.randomUUID(), name: shortcutName.trim(), kind: shortcutKind, target: shortcutTarget.trim() }]); setShortcutName(''); setShortcutTarget(''); setShortcutFormOpen(false); }); }}>
              <p id="assistant-shortcut-dialog-title" className="assistant-card-title">{label('添加快捷方式', 'ショートカットを追加', 'Add shortcut')}</p>
              <div className="assistant-kind-switch">{(['website', 'application'] as const).map(kind => <button type="button" key={kind} disabled={busy} aria-pressed={shortcutKind === kind} onClick={() => setShortcutKind(kind)}>{kind === 'website' ? label('网站', 'サイト', 'Website') : label('应用', 'アプリ', 'App')}</button>)}</div>
              <input autoFocus required maxLength={80} aria-label="Shortcut name" placeholder={label('名称', '名前', 'Name')} value={shortcutName} onChange={e => setShortcutName(e.target.value)} />
              <input required aria-label="Shortcut target" placeholder={shortcutKind === 'website' ? 'https://…' : 'C:\\…\\app.exe'} value={shortcutTarget} onChange={e => setShortcutTarget(e.target.value)} />
              {error && <p className="assistant-review-error" role="alert">{error}</p>}
              <div className="assistant-modal-actions"><button type="button" className="assistant-ghost-button" disabled={busy} onClick={() => setShortcutFormOpen(false)}>{label('取消', 'キャンセル', 'Cancel')}</button><button type="submit" className="assistant-primary" disabled={busy || !context || !shortcutName.trim() || !shortcutTarget.trim()}>{busy ? label('添加中…', '追加中…', 'Adding…') : label('添加', '追加', 'Add')}</button></div>
            </form>
    </dialog>
  </main>;
}

function VoiceDiagnostics() {
  const [report, setReport] = useState<{ samples: Array<{ mode: string; elapsed_ms: number; stage: string }>; groups: Record<string, { count: number; p50_ms: number; p95_ms: number }> } | null>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    let alive = true;
    const update = () => void invoke<typeof report>('companion_voice_diagnostics').then(value => { if (alive) setReport(value); }).catch(e => { if (alive) setError(String(e)); });
    update(); const timer = setInterval(update, 3000);
    return () => { alive = false; clearInterval(timer); };
  }, []);
  return <section className="assistant-card">
    <h2 className="assistant-card-title assistant-card-title-row"><Mic size={15} /><span>ASR / LLM / first audio</span></h2>
    <p className="assistant-muted" style={{ marginTop: 4 }}>{report?.samples.length ?? 0} samples</p>
    <div className="assistant-voice-groups">{Object.entries(report?.groups ?? {}).map(([name, group]) => <div className="assistant-voice-group" key={name}><strong>{name} · n={group.count}</strong><span>p50 {Math.round(group.p50_ms)} / p95 {Math.round(group.p95_ms)} ms</span></div>)}</div>
    {!report?.samples.length && <p className="assistant-muted">暂无记录</p>}
    <details className="assistant-details"><summary>测量范围</summary><p>实时指标从最后一次服务端识别结果到音频输出回调计时；不包含讲话结束之前的 VAD 等待，也不代表声卡的物理输出延迟。</p></details>
    <button className="assistant-primary" style={{ marginTop: 14 }} disabled={!report?.samples.length} onClick={() => {
      const url = URL.createObjectURL(new Blob([JSON.stringify(report, null, 2)], { type: 'application/json' }));
      const anchor = document.createElement('a'); anchor.href = url; anchor.download = 'voice-latency.json'; anchor.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    }}>导出测量数据</button>
    {error && <p role="alert" className="assistant-review-error">{error}</p>}
    <div className="assistant-voice-samples">{report?.samples.slice(-12).reverse().map((sample, index) => <p key={index}>{sample.mode} · {sample.stage}: {Math.round(sample.elapsed_ms)} ms</p>)}</div>
  </section>;
}

function UserQuickNotes() {
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
    {!notes.length && <p className="assistant-muted">暂无随手记</p>}
    {notes.map(note => <details className="assistant-details" key={note.id}>
      <summary>{note.title}</summary>
      <p style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{note.content}</p>
      <button className="assistant-icon-button" aria-label="删除随手记" disabled={busy} onClick={() => {
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
  useEffect(() => {
    let active = true;
    const key = shortcut.id + ':' + shortcut.kind + ':' + shortcut.target;
    let request = shortcutIconCache.get(key);
    if (!request) {
      request = invoke<string>('companion_shortcut_icon', { shortcutId: shortcut.id }).catch(() => { shortcutIconCache.delete(key); return null; });
      if (shortcutIconCache.size >= 60) shortcutIconCache.delete(shortcutIconCache.keys().next().value!);
      shortcutIconCache.set(key, request);
    }
    void request.then(value => { if (active) setIcon(value); });
    return () => { active = false; };
  }, [shortcut.id, shortcut.kind, shortcut.target]);
  return <span className="assistant-shortcut-icon" aria-hidden="true">{icon ? <img src={icon} alt="" onError={() => setIcon(null)} /> : shortcut.kind === 'application' ? <Monitor size={20} /> : <span>{shortcut.name.trim().slice(0, 1).toUpperCase()}</span>}</span>;
}
