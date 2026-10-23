import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Brain, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import './WorkMemoryPanel.css';

interface Document { id: string; path: string; content: string | null }
interface Snapshot {
  policy: { use_memories: boolean; generate_memories: boolean };
  documents: Document[];
  jobs: { session_id: string; failures: number; paused: boolean; next_attempt_at: number; last_error: string | null }[];
  instructions: { path: string; scope: string; content: string }[];
}
const strings = {
  zh: { title: '记忆与项目规约', read: '本会话使用已有记忆', write: '本会话生成后续记忆',
    hint: '关闭读取后，已出现在对话里的内容仍会保留。项目规约和用户自定义指令独立生效。',
    global: '跨项目用户记忆', project: '项目经验 / 无工作区笔记', rules: '当前目录规约', global_rules: '全局工作规约',
    distill: '从当前会话提炼', retry: '重试待处理任务', jobs: '待处理记忆任务', save: '保存', saved: '已保存', close: '关闭', loading: '加载中…', refresh: '重新加载',
    instructions: '本会话加载的规约', scope: '作用域', explicit: '规约仅由明确编辑维护；自动提炼不会修改 AGENTS.md。较深目录的规约只适用于对应目录。',
    evidence: '经验正文中的 memory-entry 和 memory-source 标记可追溯来源。删除正文不会删除原始会话和来源材料。',
    tools: '更多操作：/memory 提炼、/memory 状态、/memory 重试。全局记忆仅保存你明确指定的跨项目信息。',
    dirty: '有尚未保存的修改；切换文档前请保存或重新加载。',
  },
  en: { title: 'Memory & project instructions', read: 'Use existing memories in this chat', write: 'Generate memories from this chat',
    hint: 'Disabling recall does not remove content already in the conversation. Explicit instructions apply independently.',
    global: 'User memory across projects', project: 'Project experience / workspace-free notes', rules: 'Current directory instructions', global_rules: 'Global work instructions',
    distill: 'Extract from this chat', retry: 'Retry pending jobs', jobs: 'Pending memory jobs', save: 'Save', saved: 'Saved', close: 'Close', loading: 'Loading…', refresh: 'Reload', instructions: 'Loaded instructions', scope: 'Scope',
    explicit: 'Instructions are maintained through explicit edits. Automatic extraction never modifies AGENTS.md. Nested rules apply only to their directory.',
    evidence: 'memory-entry and memory-source markers link to evidence. Removing text does not delete original conversations or evidence files.',
    tools: 'More actions: /memory 提炼, /memory 状态, /memory 重试. Global memory contains explicitly saved information only.',
    dirty: 'Unsaved changes. Save or reload before switching documents.',
  },
  ja: { title: 'メモリとプロジェクト規約', read: 'この会話で既存メモリを使う', write: 'この会話からメモリを生成する',
    hint: '無効にしても会話に含まれた情報は残ります。明示的な規約は独立して適用されます。',
    global: 'プロジェクト共通メモリ', project: 'プロジェクト経験 / 作業メモ', rules: '現在のディレクトリ規約', global_rules: '全体の作業規約',
    distill: 'この会話から抽出', retry: '保留タスクを再試行', jobs: '保留中のメモリタスク', save: '保存', saved: '保存しました', close: '閉じる', loading: '読み込み中…', refresh: '再読み込み', instructions: '読み込まれた規約', scope: '適用範囲',
    explicit: '規約は明示的な編集で管理します。自動抽出は AGENTS.md を変更しません。子規約はそのディレクトリだけに適用されます。',
    evidence: 'memory-entry と memory-source は出典への参照です。本文の削除で元の会話や出典は削除されません。',
    tools: '操作：/memory 提炼、/memory 状态、/memory 重试。全体メモリには明示的に保存した情報のみを含めます。',
    dirty: '未保存の変更があります。保存または再読み込みしてください。',
  },
};

export default function WorkMemoryPanel({ sessionId }: { sessionId: string }) {
  const { i18n } = useTranslation();
  const language = i18n.resolvedLanguage || i18n.language;
  const text = strings[language.startsWith('zh') ? 'zh' : language.startsWith('ja') ? 'ja' : 'en'];
  const dialog = useRef<HTMLDialogElement>(null);
  const [open, setOpen] = useState(false);
  const [snapshot, setSnapshot] = useState<Snapshot>();
  const [selected, setSelected] = useState('project');
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const doc = snapshot?.documents.find(d => d.id === selected);
  const dirty = Boolean(doc && draft !== (doc.content ?? ''));
  const version = useRef(0);

  async function load(document = selected) {
    const request = ++version.current;
    setBusy(true); setError(''); setNotice('');
    try {
      const next = await invoke<Snapshot>('coding_memory_snapshot', { sessionId });
      if (version.current !== request) return;
      setSnapshot(next); setDraft(next.documents.find(d => d.id === document)?.content ?? '');
    } catch (e) { if (version.current === request) setError(String(e)); }
    finally { if (version.current === request) setBusy(false); }
  }
  useEffect(() => {
    if (open) { dialog.current?.showModal(); void load(); }
    else dialog.current?.close();
    return () => { version.current++; };
    // The parent keys this component by session id; local edits never cross sessions.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, sessionId]);

  async function updatePolicy(key: 'use_memories' | 'generate_memories', value: boolean) {
    if (!snapshot) return;
    setBusy(true); setError('');
    const policy = { ...snapshot.policy, [key]: value };
    try {
      await invoke('coding_memory_settings', { sessionId, useMemories: policy.use_memories, generateMemories: policy.generate_memories });
      setSnapshot({ ...snapshot, policy });
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function save() {
    if (!doc) return;
    setBusy(true); setError(''); setNotice('');
    try {
      await invoke('coding_memory_save', { sessionId, document: doc.id, content: draft, expected: doc.content });
      await load(); setNotice(text.saved);
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function action(name: '提炼' | '重试') {
    setBusy(true); setError(''); setNotice('');
    try {
      const result = await invoke<string>('coding_memory_action', { sessionId, action: name });
      await load(); setNotice(result);
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  return <>
    <button type="button" className="codex-icon-btn" title={text.title} aria-label={text.title} onClick={() => setOpen(true)}><Brain size={15} /></button>
    {open && <dialog className="work-memory-dialog" ref={dialog} onCancel={e => { if (busy) e.preventDefault(); else setOpen(false); }} onClose={() => setOpen(false)}>
      <header><strong>{text.title}</strong><button type="button" aria-label={text.close} disabled={busy} onClick={() => setOpen(false)}><X size={18} /></button></header>
      <div className="work-memory-body">
        <fieldset disabled={busy || !snapshot}>
          <label><input type="checkbox" checked={snapshot?.policy.use_memories ?? false} onChange={e => void updatePolicy('use_memories', e.target.checked)} />{text.read}</label>
          <label><input type="checkbox" checked={snapshot?.policy.generate_memories ?? false} onChange={e => void updatePolicy('generate_memories', e.target.checked)} />{text.write}</label>
        </fieldset>
        <p>{text.hint}</p>
        <select aria-label={text.title} disabled={busy || !snapshot} value={selected} onChange={e => {
          if (dirty) { setError(text.dirty); return; }
          setSelected(e.target.value); setDraft(snapshot?.documents.find(d => d.id === e.target.value)?.content ?? ''); setError(''); setNotice('');
        }}>
          {snapshot?.documents.map(d => <option key={d.id} value={d.id}>{text[d.id as 'global' | 'project' | 'rules' | 'global_rules']}</option>)}
        </select>
        <code className="work-memory-path">{doc?.path}</code>
        <textarea aria-label={doc?.path || text.title} disabled={busy || !doc} value={draft} onChange={e => { setDraft(e.target.value); setNotice(''); }} spellCheck={false} />
        <p>{selected.includes('rules') ? text.explicit : text.evidence}</p>
        <details><summary>{text.instructions}</summary>{snapshot?.instructions.map(rule => <div key={rule.path}><code>{rule.path}</code><p>{text.scope}: {rule.scope}</p><pre>{rule.content}</pre></div>)}</details>
        <details><summary>{text.jobs} ({snapshot?.jobs.length ?? 0})</summary>
          {snapshot?.jobs.map((job, i) => <p key={`${job.session_id}-${i}`}>{job.session_id} · {job.failures} · {job.last_error || (job.next_attempt_at ? new Date(job.next_attempt_at).toLocaleString() : '—')}</p>)}
        </details>
        <p>{text.tools}</p>
        {error && <p role="alert" className="work-memory-error">{error}</p>}
        <p role="status">{busy ? text.loading : notice}</p>
      </div>
      <footer><button type="button" disabled={busy || dirty || !snapshot} onClick={() => void action('提炼')}>{text.distill}</button><button type="button" disabled={busy || dirty || !snapshot?.jobs.length} onClick={() => void action('重试')}>{text.retry}</button><button type="button" disabled={busy} onClick={() => void load()}>{text.refresh}</button><button type="button" disabled={busy || !dirty} onClick={() => void save()}>{text.save}</button></footer>
    </dialog>}
  </>;
}
