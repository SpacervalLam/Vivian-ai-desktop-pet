import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { FlaskConical, GitBranch, Play, RefreshCw, Trash2 } from 'lucide-react';
import './DialogueLabPage.css';

interface Turn { user: string; reply: string; raw: string; route: string; model: string; prompt_note: string; elapsed_ms: number }
interface Snapshot { user_input: string; captured_at: number; model: string; route: string; messages?: { role: string; content: string }[] }
interface Summary { id: string; title: string; captured_at: number; user_input: string; turn_count: number }
interface Branch { id: string; title: string; parent_id?: string; snapshot: Snapshot; turns: Turn[] }
interface Listing { snapshot?: Snapshot; branches: Summary[]; routes: { id: string; model: string }[] }
const COPY = {
  zh: {
    title: '对话试验', subtitle: '从同一个起点比较不同的回应，再继续聊几轮。', refresh: '刷新', create: '保存起点',
    empty: '先和角色进行一次普通文字对话，再来保存起点。', scope: '试聊单独保存，不改变角色记忆和关系；不执行工具，也不会自动朗读。生成会使用所选模型的 API。',
    snapshot: '最近捕获的起点', branches: '已保存的分支', choose: '选择分支', compare: '与另一分支对照', none: '暂不对照',
    start: '从起点创建对照', fork: '从这里分支', prompt: '本分支的补充提示', hint: '例如：更随意地接话，保留自己的小偏好。留空使用原提示词。',
    replay: '生成起点回复', send: '继续试聊', input: '你接下来想说什么？', busy: '生成中…', silence: '保持沉默', raw: '查看模型原始回复',
    context: '查看冻结的文字上下文', model: '模型路由', original: '起点输入', turns: '轮', delete: '删除分支', deleted: '分支已删除',
    recorded: '保存本轮的路由与提示；标注的模型是配置目标，发生回退时可能不同。', newBranch: '试聊', confirmDelete: '删除这个试聊分支？真实聊天不会受影响。',
  },
  en: {
    title: 'Dialogue lab', subtitle: 'Compare replies from the same starting point, then keep chatting.', refresh: 'Refresh', create: 'Save starting point',
    empty: 'Have a normal text conversation with this character first.', scope: 'Rehearsals are saved separately. They do not change memories or relationships, execute tools, or play speech automatically. Generation uses the selected model API.',
    snapshot: 'Latest captured starting point', branches: 'Saved branches', choose: 'Select branch', compare: 'Compare with another branch', none: 'No comparison',
    start: 'Compare from start', fork: 'Branch from here', prompt: 'Additional prompt for this branch', hint: 'For example: respond more casually while keeping your own preferences. Leave blank to use the original prompt.',
    replay: 'Generate starting reply', send: 'Continue rehearsal', input: 'What would you say next?', busy: 'Generating…', silence: 'Stays silent', raw: 'Raw model reply',
    context: 'Frozen text context', model: 'Model route', original: 'Starting input', turns: 'turns', delete: 'Delete branch', deleted: 'Branch deleted',
    recorded: 'Each turn records its route and prompt. Model labels show the configured target; fallback can use another model.', newBranch: 'Rehearsal', confirmDelete: 'Delete this rehearsal branch? Real conversations will be unaffected.',
  },
  ja: {
    title: '対話テスト', subtitle: '同じ起点から返事を比べ、そのまま数往復続けられます。', refresh: '更新', create: '起点を保存',
    empty: '先にこのキャラクターと普通のテキスト会話をしてください。', scope: 'テストは別に保存され、記憶や関係を変更せず、ツールを実行したり自動で音声を再生したりしません。生成には選択したモデルの API を使います。',
    snapshot: '直近に取得した起点', branches: '保存した分岐', choose: '分岐を選択', compare: '別の分岐と比較', none: '比較しない',
    start: '起点から比較', fork: 'ここから分岐', prompt: 'この分岐の追加プロンプト', hint: '例：自分の好みを保ちつつ、もっと気軽に返す。空欄では元のプロンプトを使用します。',
    replay: '起点の返事を生成', send: 'テストを続ける', input: '次に何と言いますか？', busy: '生成中…', silence: '発言なし', raw: 'モデルの元の返事',
    context: '保存したテキスト文脈', model: 'モデルルート', original: '起点の入力', turns: '往復', delete: '分岐を削除', deleted: '削除しました',
    recorded: '各往復のルートとプロンプトを保存します。モデル名は設定先であり、フォールバック時には変わる場合があります。', newBranch: 'テスト', confirmDelete: 'このテスト分岐を削除しますか？実際の会話には影響しません。',
  },
};

export default function DialogueLabPage() {
  const { i18n } = useTranslation();
  const copy = i18n.language.startsWith('en') ? COPY.en : i18n.language.startsWith('ja') ? COPY.ja : COPY.zh;
  const [character, setCharacter] = useState('vivian');
  const [listing, setListing] = useState<Listing>({ branches: [], routes: [] });
  const [active, setActive] = useState<Branch | null>(null);
  const [comparison, setComparison] = useState<Branch | null>(null);
  const [route, setRoute] = useState('chat');
  const [note, setNote] = useState('');
  const [input, setInput] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const generation = useRef(0);
  const activeRequest = useRef(0);
  const compareRequest = useRef(0);

  async function refresh(epoch = generation.current) {
    const result = await invoke<Listing>('list_dialogue_lab', { characterId: character });
    if (epoch === generation.current) setListing(result);
  }
  useEffect(() => {
    const epoch = ++generation.current;
    activeRequest.current++; compareRequest.current++;
    setActive(null); setComparison(null); setNote(''); setInput(''); setError('');
    setListing({ branches: [], routes: [] });
    void refresh(epoch).catch(e => { if (epoch === generation.current) setError(String(e)); });
    return () => { generation.current++; };
  }, [character]);

  async function select(id: string, compare = false) {
    const request = compare ? ++compareRequest.current : ++activeRequest.current;
    const epoch = generation.current;
    if (!id) { if (compare) setComparison(null); else setActive(null); return; }
    try {
      const branch = await invoke<Branch>('get_dialogue_lab', { branchId: id });
      if (epoch !== generation.current || request !== (compare ? compareRequest.current : activeRequest.current)) return;
      if (compare) setComparison(branch);
      else {
        setActive(branch); setInput('');
        const last = branch.turns[branch.turns.length - 1];
        setNote(last?.prompt_note ?? '');
        setRoute(last?.route ?? branch.snapshot.route);
      }
    } catch (e) { if (epoch === generation.current) setError(String(e)); }
  }
  async function action(work: () => Promise<void>) {
    if (busy) return;
    activeRequest.current++; compareRequest.current++;
    setBusy(true); setError('');
    try { await work(); } catch (e) { setError(String(e)); } finally { setBusy(false); }
  }
  async function fork(fromStart: boolean) {
    if (!active) return;
    const previous = active;
    const branch = await invoke<Branch>('fork_dialogue_lab', { branchId: active.id, fromStart, name: `${copy.newBranch} ${listing.branches.length + 1}` });
    setComparison(previous); setActive(branch); setInput(''); await refresh();
  }
  async function run() {
    if (!active) return;
    const branch = await invoke<Branch>('run_dialogue_lab', { branchId: active.id, input, route, promptNote: note });
    setActive(branch); setInput(''); await refresh();
  }
  function card(branch: Branch, compare: boolean) {
    return <article className="dialogue-lab-card" key={branch.id}>
      <header><span className="dialogue-lab-badge">{compare ? 'B' : 'A'}</span><h3>{branch.title || copy.newBranch}</h3><span>{branch.turns.length} {copy.turns}</span></header>
      <p className="dialogue-lab-original"><small>{copy.original}</small>{branch.snapshot.user_input}</p>
      <div className="dialogue-lab-turns">
        {branch.turns.map((turn, index) => <section key={index} className="dialogue-lab-turn">
          {turn.user && <p className="dialogue-lab-user">{turn.user}</p>}
          <p className="dialogue-lab-reply">{turn.reply || <em>{copy.silence}</em>}</p>
          <small>{turn.route} · {turn.model} · {(turn.elapsed_ms / 1000).toFixed(1)}s</small>
          {turn.prompt_note && <p className="dialogue-lab-note">{turn.prompt_note}</p>}
          <details><summary>{copy.raw}</summary><pre>{turn.raw}</pre></details>
        </section>)}
      </div>
      <details className="dialogue-lab-context"><summary>{copy.context}</summary>
        {branch.snapshot.messages?.map((message, i) => <div key={i}><small>{message.role}</small><pre>{message.content}</pre></div>)}
      </details>
    </article>;
  }
  return <div className="dialogue-lab">
    <header className="dialogue-lab-heading"><div><h2><FlaskConical size={22} />{copy.title}</h2><p>{copy.subtitle}</p></div>
      <button disabled={busy} onClick={() => void action(() => refresh())}><RefreshCw size={15} />{copy.refresh}</button></header>
    <p className="dialogue-lab-scope">{copy.scope}</p>
    <div className="dialogue-lab-toolbar">
      <div className="dialogue-lab-characters">{['vivian', 'nana'].map(id => <button key={id} disabled={busy} aria-pressed={character === id} onClick={() => setCharacter(id)}>{id === 'vivian' ? 'Vivian' : 'Nana'}</button>)}</div>
      <button disabled={busy || !listing.snapshot} onClick={() => void action(async () => {
        const branch = await invoke<Branch>('create_dialogue_lab', { characterId: character, name: `${copy.newBranch} ${listing.branches.length + 1}` });
        setActive(branch); setComparison(null); setNote(''); setInput(''); setRoute(branch.snapshot.route); await refresh();
      })}><GitBranch size={15} />{copy.create}</button>
    </div>
    <div className="dialogue-lab-snapshot"><small>{copy.snapshot}</small>
      {listing.snapshot ? <><p>{listing.snapshot.user_input}</p><small>{new Date(listing.snapshot.captured_at * 1000).toLocaleString()} · {listing.snapshot.model}</small></> : <p>{copy.empty}</p>}
    </div>
    {error && <p className="dialogue-lab-error" role="alert">{error}</p>}
    <div className="dialogue-lab-selectors">
      <label>{copy.choose}<select disabled={busy} value={active?.id ?? ''} onChange={e => void select(e.target.value)}><option value="">—</option>{listing.branches.map(b => <option key={b.id} value={b.id}>{b.title} · {b.turn_count} {copy.turns}</option>)}</select></label>
      <label>{copy.compare}<select disabled={busy} value={comparison?.id ?? ''} onChange={e => void select(e.target.value, true)}><option value="">{copy.none}</option>{listing.branches.filter(b => b.id !== active?.id).map(b => <option key={b.id} value={b.id}>{b.title}</option>)}</select></label>
    </div>
    {active && <>
      <div className={`dialogue-lab-comparison ${comparison ? 'has-comparison' : ''}`}>{card(active, false)}{comparison && card(comparison, true)}</div>
      <div className="dialogue-lab-controls">
        <div className="dialogue-lab-actions"><button disabled={busy} onClick={() => void action(() => fork(true))}>{copy.start}</button><button disabled={busy} onClick={() => void action(() => fork(false))}>{copy.fork}</button>
          <button className="dialogue-lab-delete" disabled={busy} onClick={() => { if (window.confirm(copy.confirmDelete)) void action(async () => {
            await invoke('delete_dialogue_lab', { branchId: active.id }); setActive(null); await refresh();
          }); }}><Trash2 size={14} />{copy.delete}</button></div>
        <label>{copy.model}<select value={route} disabled={busy} onChange={e => setRoute(e.target.value)}>{listing.routes.map(r => <option key={r.id} value={r.id}>{r.id} · {r.model}</option>)}</select></label>
        <label>{copy.prompt}<textarea maxLength={2000} disabled={busy} value={note} onChange={e => setNote(e.target.value)} placeholder={copy.hint} rows={2} /></label>
        <form onSubmit={e => { e.preventDefault(); void action(run); }}>
          <textarea disabled={busy} maxLength={4000} rows={2} value={input} onChange={e => setInput(e.target.value)} placeholder={copy.input} />
          <button className="dialogue-lab-primary" disabled={busy || (active.turns.length > 0 && !input.trim())}><Play size={14} />{busy ? copy.busy : active.turns.length ? copy.send : copy.replay}</button>
        </form><small>{copy.recorded}</small>
      </div>
    </>}
  </div>;
}
