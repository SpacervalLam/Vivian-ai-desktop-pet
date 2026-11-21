import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Brain, ChevronDown, ChevronRight, RefreshCw, RotateCcw, Save, Sparkles, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import './WorkMemoryPanel.css';

interface Document { id: string; path: string; content: string | null }
interface WorkFact {
  id: string;
  text: string;
  source_session: string;
  updated_by: string;
  revision: number;
  updated_at: number;
}
interface FactsState { revision: number; facts: WorkFact[] }
interface WorkLoopView {
  id: string;
  title: string;
  prompt: string;
  cadence_label: string;
  next_run_at: number;
  max_runs: number;
  runs: number;
  enabled: boolean;
  last_status?: string | null;
}
interface LoopsState { loops: WorkLoopView[] }
interface RouteSuggestion {
  model: string;
  tokens_per_request: number;
  acceptance: number;
  replaces: string;
  saves_tokens_per_request: number;
}
interface RouteRow {
  task: string;
  current_model: string;
  donor_task: string | null;
  can_apply: boolean;
  suggestion: RouteSuggestion | null;
}
interface RouteReport { tasks: RouteRow[] }
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
    docs: '记忆文档', jobsShort: '待处理任务', unsaved: '未保存', paused: '已暂停',
    distill: '从当前会话提炼', retry: '重试待处理任务', jobs: '待处理记忆任务', save: '保存', saved: '已保存', close: '关闭', loading: '加载中…', refresh: '重新加载',
    instructions: '本会话加载的规约', scope: '作用域', explicit: '规约仅由明确编辑维护；自动提炼不会修改 AGENTS.md。较深目录的规约只适用于对应目录。',
    evidence: '经验正文中的 memory-entry 和 memory-source 标记可追溯来源。删除正文不会删除原始会话和来源材料。',
    tools: '更多操作：/memory 提炼、/memory 状态、/memory 重试。全局记忆仅保存你明确指定的跨项目信息。',
    emptyInstructions: '本会话还没有加载任何项目规约。', emptyJobs: '暂时没有待处理的记忆任务。',
    dirty: '有尚未保存的修改；切换文档前请保存或重新加载。',
    facts: '跨会话工作事实', factsHint: '这些一句话事实会进入每个工作会话的上下文，用来省掉重复交代。只属于某个会话的临时细节不要写进来。',
    factAdd: '添加事实', factPlaceholder: '一句话说清：稳定的前提、约定或决策', factEmpty: '还没有共享事实。',
    factClear: '清空', factUser: '用户确认', factModel: '会话确认', factSave: '保存', factDelete: '删除',
    loops: '值守', loopsHint: '到点由宿主唤醒本会话一次，其余时间不调用模型；只在应用运行时生效。',
    loopAdd: '安排值守', loopPrompt: '这一轮要做的事（写成能独立执行的一句话）', loopMinutes: '每几分钟',
    loopEmpty: '还没有值守条目。', loopPause: '停用', loopResume: '启用', loopNext: '下次',
    loopRuns: '已运行', loopUnlimited: '不限次数',
    routes: '路由建议', routesHint: '按人工验收与用量成本给出更省的模型建议；只有你自己点「采用」才会写入路由。',
    routeApply: '采用', routeManual: '需手动配置', routeCurrent: '当前', routeSaves: '每次省', routeAcceptance: '验收率',
  },
  en: { title: 'Memory & project instructions', read: 'Use existing memories in this chat', write: 'Generate memories from this chat',
    hint: 'Disabling recall does not remove content already in the conversation. Explicit instructions apply independently.',
    global: 'User memory across projects', project: 'Project experience / workspace-free notes', rules: 'Current directory instructions', global_rules: 'Global work instructions',
    docs: 'Memory documents', jobsShort: 'Pending', unsaved: 'Unsaved', paused: 'Paused',
    distill: 'Extract from this chat', retry: 'Retry pending jobs', jobs: 'Pending memory jobs', save: 'Save', saved: 'Saved', close: 'Close', loading: 'Loading…', refresh: 'Reload', instructions: 'Loaded instructions', scope: 'Scope',
    explicit: 'Instructions are maintained through explicit edits. Automatic extraction never modifies AGENTS.md. Nested rules apply only to their directory.',
    evidence: 'memory-entry and memory-source markers link to evidence. Removing text does not delete original conversations or evidence files.',
    tools: 'More actions: /memory 提炼, /memory 状态, /memory 重试. Global memory contains explicitly saved information only.',
    emptyInstructions: 'No project instructions loaded in this chat yet.', emptyJobs: 'No pending memory jobs.',
    dirty: 'Unsaved changes. Save or reload before switching documents.',
    facts: 'Facts shared across sessions', factsHint: 'These one-line facts enter every work session so you do not repeat the same context. Session-local details do not belong here.',
    factAdd: 'Add fact', factPlaceholder: 'One sentence: a stable premise, convention, or decision', factEmpty: 'No shared facts yet.',
    factClear: 'Clear', factUser: 'User confirmed', factModel: 'Session confirmed', factSave: 'Save', factDelete: 'Delete',
    loops: 'Watch loops', loopsHint: 'The host wakes this session once when due; no model calls otherwise. App must be running.',
    loopAdd: 'Schedule', loopPrompt: 'What this round should do (one self-contained instruction)', loopMinutes: 'Every N minutes',
    loopEmpty: 'No watch loops yet.', loopPause: 'Pause', loopResume: 'Resume', loopNext: 'Next',
    loopRuns: 'Runs', loopUnlimited: 'unlimited',
    routes: 'Routing suggestions', routesHint: 'Cheaper models based on human review and token cost. Nothing changes unless you press Adopt.',
    routeApply: 'Adopt', routeManual: 'Configure manually', routeCurrent: 'Current', routeSaves: 'Saves', routeAcceptance: 'Acceptance',
  },
  ja: { title: 'メモリとプロジェクト規約', read: 'この会話で既存メモリを使う', write: 'この会話からメモリを生成する',
    hint: '無効にしても会話に含まれた情報は残ります。明示的な規約は独立して適用されます。',
    global: 'プロジェクト共通メモリ', project: 'プロジェクト経験 / 作業メモ', rules: '現在のディレクトリ規約', global_rules: '全体の作業規約',
    docs: 'メモリ文書', jobsShort: '保留', unsaved: '未保存', paused: '一時停止中',
    distill: 'この会話から抽出', retry: '保留タスクを再試行', jobs: '保留中のメモリタスク', save: '保存', saved: '保存しました', close: '閉じる', loading: '読み込み中…', refresh: '再読み込み', instructions: '読み込まれた規約', scope: '適用範囲',
    explicit: '規約は明示的な編集で管理します。自動抽出は AGENTS.md を変更しません。子規約はそのディレクトリだけに適用されます。',
    evidence: 'memory-entry と memory-source は出典への参照です。本文の削除で元の会話や出典は削除されません。',
    tools: '操作：/memory 提炼、/memory 状态、/memory 重试。全体メモリには明示的に保存した情報のみを含めます。',
    emptyInstructions: 'この会話ではまだ規約を読み込んでいません。', emptyJobs: '保留中のメモリタスクはありません。',
    dirty: '未保存の変更があります。保存または再読み込みしてください。',
    facts: 'セッション間で共有する事実', factsHint: 'これらの一行事実は各作業セッションのコンテキストに入り、同じ説明の繰り返しを省きます。そのセッション限りの一時的な詳細は書かないでください。',
    factAdd: '事実を追加', factPlaceholder: '一文で：安定した前提・約束・決定', factEmpty: '共有された事実はまだありません。',
    factClear: 'すべて削除', factUser: 'ユーザー確認', factModel: 'セッション確認', factSave: '保存', factDelete: '削除',
    loops: '定期見守り', loopsHint: '時間になるとホストがこのセッションを一度だけ起こします。それ以外はモデルを呼びません。アプリ実行中のみ有効です。',
    loopAdd: '見守りを追加', loopPrompt: 'この回でやること（単独で実行できる一文）', loopMinutes: '何分ごと',
    loopEmpty: '見守りはまだありません。', loopPause: '停止', loopResume: '再開', loopNext: '次回',
    loopRuns: '実行回数', loopUnlimited: '無制限',
    routes: 'ルーティング提案', routesHint: '人手の確認結果とトークン費用から安いモデルを提案します。採用を押すまで設定は変わりません。',
    routeApply: '採用', routeManual: '手動設定が必要', routeCurrent: '現在', routeSaves: '1回あたり', routeAcceptance: '承認率',
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
  const [facts, setFacts] = useState<FactsState>();
  const [factDraft, setFactDraft] = useState('');
  const [factEdits, setFactEdits] = useState<Record<string, string>>({});
  const [loops, setLoops] = useState<LoopsState>();
  const [routes, setRoutes] = useState<RouteReport>();
  const [loopPrompt, setLoopPrompt] = useState('');
  const [loopMinutes, setLoopMinutes] = useState('30');
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
    if (open) { dialog.current?.showModal(); void load(); void loadFacts(); void loadLoops(); void loadRoutes(); }
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
  async function loadFacts() {
    try {
      setFacts(await invoke<FactsState>('work_facts_list'));
    } catch {
      // 命令不可用时保持上一份投影，不放假数据。
    }
  }
  async function writeFacts(operation: () => Promise<FactsState>) {
    setBusy(true); setError(''); setNotice('');
    try { setFacts(await operation()); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  function addFact() {
    const text = factDraft.trim();
    if (!text || busy) return;
    void writeFacts(async () => {
      const next = await invoke<FactsState>('work_facts_save', { text });
      setFactDraft('');
      return next;
    });
  }
  function editFact(id: string) {
    const text = (factEdits[id] ?? '').trim();
    if (!text || busy) return;
    void writeFacts(async () => {
      const next = await invoke<FactsState>('work_facts_save', { id, text });
      setFactEdits(previous => { const copy = { ...previous }; delete copy[id]; return copy; });
      return next;
    });
  }
  async function loadLoops() {
    try {
      setLoops(await invoke<LoopsState>('work_loop_list'));
    } catch {
      // 命令不可用时保持上一份投影。
    }
  }
  async function loadRoutes() {
    try {
      setRoutes(await invoke<RouteReport>('work_route_report', { days: 30 }));
    } catch {
      // 报表不可用时不显示建议，不放假数据。
    }
  }
  async function applyRoute(task: string, model: string) {
    setBusy(true); setError(''); setNotice('');
    try {
      await invoke('work_route_apply', { task, model });
      await loadRoutes();
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function writeLoops(operation: () => Promise<LoopsState>) {
    setBusy(true); setError(''); setNotice('');
    try { setLoops(await operation()); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  function addLoop() {
    const prompt = loopPrompt.trim();
    const minutes = Number(loopMinutes);
    if (!prompt || !Number.isFinite(minutes) || minutes < 1 || busy) return;
    void writeLoops(async () => {
      const next = await invoke<LoopsState>('work_loop_upsert', {
        sessionId, title: '', prompt, cadenceMinutes: Math.round(minutes),
      });
      setLoopPrompt('');
      return next;
    });
  }
  return <>
    <button type="button" className="codex-icon-btn" title={text.title} aria-label={text.title} onClick={() => setOpen(true)}><Brain size={15} /></button>
    {open && <dialog className="work-memory-dialog" ref={dialog} onCancel={e => { if (busy) e.preventDefault(); else setOpen(false); }} onClose={() => setOpen(false)}>
      <header className="wm-head">
        <span className="wm-head-icon" aria-hidden="true"><Brain size={14} /></span>
        <strong className="wm-head-title">{text.title}</strong>
        <button type="button" className="wm-icon-btn" aria-label={text.close} title={text.close} disabled={busy} onClick={() => setOpen(false)}><X size={16} /></button>
      </header>

      <div className="wm-body">
        <fieldset className="wm-policy" disabled={busy || !snapshot}>
          <label className="wm-toggle">
            <input type="checkbox" checked={snapshot?.policy.use_memories ?? false} onChange={e => void updatePolicy('use_memories', e.target.checked)} />
            <span>{text.read}</span>
          </label>
          <label className="wm-toggle">
            <input type="checkbox" checked={snapshot?.policy.generate_memories ?? false} onChange={e => void updatePolicy('generate_memories', e.target.checked)} />
            <span>{text.write}</span>
          </label>
        </fieldset>
        <p className="wm-note">{text.hint}</p>

        {/* 跨会话事实放在最前：它决定「新会话还要不要重新交代前提」，
            比具体某个记忆文档更常被用到。 */}
        <div className="wm-section-head">
          <span>{text.facts}</span>
          <span className="wm-rule-line" aria-hidden="true" />
          <span className="wm-count">{facts?.facts.length ?? 0}</span>
        </div>
        <div className="wm-facts">
          <p className="wm-note">{text.factsHint}</p>
          {facts?.facts.length
            ? facts.facts.map(fact => {
              const editing = factEdits[fact.id] !== undefined;
              return <div className="wm-fact" key={fact.id}>
                {editing
                  ? <input
                    className="wm-fact-input"
                    aria-label={text.factSave}
                    autoFocus
                    value={factEdits[fact.id]}
                    disabled={busy}
                    onChange={event => setFactEdits({ ...factEdits, [fact.id]: event.target.value })}
                    onKeyDown={event => {
                      if (event.key === 'Enter') editFact(fact.id);
                      if (event.key === 'Escape') setFactEdits(previous => { const copy = { ...previous }; delete copy[fact.id]; return copy; });
                    }}
                  />
                  : <button
                    type="button"
                    className="wm-fact-text"
                    title={text.factSave}
                    disabled={busy}
                    onClick={() => setFactEdits({ ...factEdits, [fact.id]: fact.text })}
                  >{fact.text}</button>}
                <span className={`wm-tag ${fact.updated_by === 'user' ? 'is-user' : ''}`}>
                  {fact.updated_by === 'user' ? text.factUser : text.factModel}
                </span>
                {editing && <button type="button" className="wm-btn" disabled={busy} onClick={() => editFact(fact.id)}>{text.factSave}</button>}
                <button
                  type="button"
                  className="wm-icon-btn"
                  aria-label={text.factDelete}
                  title={text.factDelete}
                  disabled={busy}
                  onClick={() => void writeFacts(() => invoke<FactsState>('work_facts_remove', { id: fact.id }))}
                ><X size={13} /></button>
              </div>;
            })
            : <p className="wm-empty">{text.factEmpty}</p>}
          <div className="wm-fact-add">
            <input
              className="wm-fact-input"
              placeholder={text.factPlaceholder}
              value={factDraft}
              disabled={busy}
              onChange={event => setFactDraft(event.target.value)}
              onKeyDown={event => { if (event.key === 'Enter') addFact(); }}
            />
            <button type="button" className="wm-btn" disabled={busy || !factDraft.trim()} onClick={addFact}>{text.factAdd}</button>
            {Boolean(facts?.facts.length) && (
              <button type="button" className="wm-btn" disabled={busy} onClick={() => void writeFacts(() => invoke<FactsState>('work_facts_clear'))}>{text.factClear}</button>
            )}
          </div>
        </div>

        <div className="wm-section-head">
          <span>{text.loops}</span>
          <span className="wm-rule-line" aria-hidden="true" />
          <span className="wm-count">{loops?.loops.length ?? 0}</span>
        </div>
        <div className="wm-loops">
          <p className="wm-note">{text.loopsHint}</p>
          {loops?.loops.length
            ? loops.loops.map(item => (
              <div className="wm-loop" key={item.id} data-enabled={item.enabled || undefined}>
                <span className="wm-loop-title" title={item.prompt}>{item.title}</span>
                <span className="wm-loop-meta">
                  {item.cadence_label} · {text.loopRuns} {item.runs}/{item.max_runs || text.loopUnlimited}
                  {item.enabled && ` · ${text.loopNext} ${new Date(item.next_run_at * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}`}
                </span>
                <button
                  type="button"
                  className="wm-btn"
                  disabled={busy}
                  onClick={() => void writeLoops(() => invoke<LoopsState>('work_loop_set_enabled', { id: item.id, enabled: !item.enabled }))}
                >{item.enabled ? text.loopPause : text.loopResume}</button>
                <button
                  type="button"
                  className="wm-icon-btn"
                  aria-label={text.factDelete}
                  title={text.factDelete}
                  disabled={busy}
                  onClick={() => void writeLoops(() => invoke<LoopsState>('work_loop_remove', { id: item.id }))}
                ><X size={13} /></button>
              </div>
            ))
            : <p className="wm-empty">{text.loopEmpty}</p>}
          <div className="wm-fact-add">
            <input
              className="wm-fact-input"
              placeholder={text.loopPrompt}
              value={loopPrompt}
              disabled={busy}
              onChange={event => setLoopPrompt(event.target.value)}
              onKeyDown={event => { if (event.key === 'Enter') addLoop(); }}
            />
            <input
              className="wm-fact-input wm-loop-minutes"
              type="number"
              min={1}
              aria-label={text.loopMinutes}
              title={text.loopMinutes}
              value={loopMinutes}
              disabled={busy}
              onChange={event => setLoopMinutes(event.target.value)}
            />
            <button type="button" className="wm-btn" disabled={busy || !loopPrompt.trim()} onClick={addLoop}>{text.loopAdd}</button>
          </div>
        </div>

        {routes && routes.tasks.some(row => row.suggestion) && (
          <>
            <div className="wm-section-head">
              <span>{text.routes}</span>
              <span className="wm-rule-line" aria-hidden="true" />
              <span className="wm-count">{routes.tasks.filter(row => row.suggestion).length}</span>
            </div>
            <div className="wm-loops">
              <p className="wm-note">{text.routesHint}</p>
              {routes.tasks.filter(row => row.suggestion).map(row => {
                const suggestion = row.suggestion!;
                const acceptance = Math.round((suggestion.acceptance ?? 0) * 100);
                return (
                  <div className="wm-loop" key={row.task} data-enabled>
                    <span className="wm-loop-title" title={`${suggestion.replaces} → ${suggestion.model}`}>
                      {row.task} → {suggestion.model}
                    </span>
                    <span className="wm-loop-meta">
                      {text.routeCurrent} {row.current_model} · {text.routeSaves} {suggestion.saves_tokens_per_request} · {text.routeAcceptance} {acceptance}%
                    </span>
                    {row.can_apply
                      ? <button type="button" className="wm-btn" disabled={busy} onClick={() => void applyRoute(row.task, suggestion.model)}>{text.routeApply}</button>
                      : <span className="wm-tag">{text.routeManual}</span>}
                  </div>
                );
              })}
            </div>
          </>
        )}

        <div className="wm-section-head"><span>{text.docs}</span><span className="wm-rule-line" aria-hidden="true" /></div>
        <div className="wm-doc-group">
          <div className="wm-doc">
            <span className="wm-select-wrap">
              <select className="wm-select" aria-label={text.docs} disabled={busy || !snapshot} value={selected} onChange={e => {
                if (dirty) { setError(text.dirty); return; }
                setSelected(e.target.value); setDraft(snapshot?.documents.find(d => d.id === e.target.value)?.content ?? ''); setError(''); setNotice('');
              }}>
                {!snapshot && <option value={selected}>{text.loading}</option>}
                {snapshot?.documents.map(d => <option key={d.id} value={d.id}>{text[d.id as 'global' | 'project' | 'rules' | 'global_rules']}</option>)}
              </select>
              <ChevronDown size={14} className="wm-caret" aria-hidden="true" />
            </span>
            {dirty && <span className="wm-dirty"><span className="wm-dot" aria-hidden="true" />{text.unsaved}</span>}
          </div>
          <code className="wm-path" title={doc?.path}>{doc?.path}</code>
          <textarea className="wm-editor" aria-label={doc?.path || text.title} disabled={busy || !doc} value={draft} onChange={e => { setDraft(e.target.value); setNotice(''); }} spellCheck={false} />
          <p className="wm-note">{selected.includes('rules') ? text.explicit : text.evidence}</p>
        </div>

        <details className="wm-fold">
          <summary>
            <ChevronRight size={13} className="wm-fold-caret" aria-hidden="true" />
            <span>{text.instructions}</span>
            <span className="wm-count">{snapshot?.instructions.length ?? 0}</span>
          </summary>
          <div className="wm-fold-body">
            {snapshot?.instructions.length
              ? snapshot.instructions.map(rule => <div className="wm-rule" key={rule.path}>
                <code className="wm-rule-path">{rule.path}</code>
                <p className="wm-rule-scope">{text.scope}: {rule.scope}</p>
                <pre className="wm-rule-body">{rule.content}</pre>
              </div>)
              : <p className="wm-empty">{text.emptyInstructions}</p>}
          </div>
        </details>

        <details className="wm-fold">
          <summary>
            <ChevronRight size={13} className="wm-fold-caret" aria-hidden="true" />
            <span>{text.jobs}</span>
            <span className="wm-count">{snapshot?.jobs.length ?? 0}</span>
          </summary>
          <div className="wm-fold-body is-jobs">
            {snapshot?.jobs.length
              ? snapshot.jobs.map((job, i) => <div className="wm-job" key={`${job.session_id}-${i}`}>
                <code className="wm-job-id" title={job.session_id}>{job.session_id}</code>
                {job.paused && <span className="wm-tag">{text.paused}</span>}
                <span className="wm-job-meta">{job.failures} · {job.last_error || (job.next_attempt_at ? new Date(job.next_attempt_at).toLocaleString() : '—')}</span>
              </div>)
              : <p className="wm-empty">{text.emptyJobs}</p>}
          </div>
        </details>

        <p className="wm-note wm-note-foot">{text.tools}</p>
      </div>

      <footer className="wm-foot">
        {/* 提示与错误放在**动作条里**，不放正文末尾：
            正文是可滚动容器，错误条落在折叠区下方时用户根本看不见，
            点了「提炼」失败也只会觉得「什么都没发生」。 */}
        <div className="wm-foot-note">
          {error
            ? <p className="wm-alert" role="alert">{error}</p>
            : <p className="wm-status" role="status">
              {busy && <span className="wm-spin" aria-hidden="true" />}
              {busy ? text.loading : notice}
            </p>}
        </div>
        <div className="wm-foot-row">
          <div className="wm-foot-group">
            <button type="button" className="wm-btn" disabled={busy || dirty || !snapshot} onClick={() => void action('提炼')}><Sparkles size={13} aria-hidden="true" />{text.distill}</button>
            <button type="button" className="wm-btn" disabled={busy || dirty || !snapshot?.jobs.length} onClick={() => void action('重试')}><RotateCcw size={13} aria-hidden="true" />{text.retry}</button>
          </div>
          <div className="wm-foot-group">
            <button type="button" className="wm-btn" disabled={busy} onClick={() => { void load(); void loadFacts(); void loadLoops(); void loadRoutes(); }}><RefreshCw size={13} aria-hidden="true" />{text.refresh}</button>
            <button type="button" className="wm-btn wm-btn-primary" disabled={busy || !dirty} onClick={() => void save()}><Save size={13} aria-hidden="true" />{text.save}</button>
          </div>
        </div>
      </footer>
    </dialog>}
  </>;
}
