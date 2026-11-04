import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import { Bookmark, Check, Pause, Play, Plus, Square, ArrowUpRight } from 'lucide-react';
import { useNavigation } from '../NavigationContext';
import { ongoingLabel } from './memoryPresentation';
import { agreementWords, matchesAgreement, type AgreementView, type LinkedTodo } from './agreementPresentation';
import { useAgreements } from './useAgreements';
import './RecordTheme.css';
import './AgreementsPage.css';

export default function AgreementsPage() {
  const { i18n } = useTranslation();
  const language = i18n.resolvedLanguage ?? i18n.language;
  const say = (zh: string, en: string, ja?: string) => agreementWords(language, zh, en, ja);
  const nav = useNavigation();
  const { records, error: loadError, loading, reload } = useAgreements();
  const [todos, setTodos] = useState<LinkedTodo[]>([]);
  const [filter, setFilter] = useState('active');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState('');
  const [creating, setCreating] = useState(false);
  const [draft, setDraft] = useState({ owner: 'vivian', title: '', goal: '', trigger: '' });
  const [todoFor, setTodoFor] = useState('');
  const [todoTitle, setTodoTitle] = useState('');
  const [todoDescription, setTodoDescription] = useState('');
  const selected = String(nav?.pageParams.agreementId ?? '');
  const selectedOwner = String(nav?.pageParams.agreementOwner ?? '');
  useEffect(() => { if (selected) setFilter('all'); }, [selected]);
  useEffect(() => {
    let active = true; let stop: (() => void) | undefined;
    const load = () => invoke<{ items: LinkedTodo[] }>('list_todos', { includeCompleted: true })
      .then(value => { if (active) setTodos(value.items ?? []); }).catch(e => { if (active) setError(String(e)); });
    void load(); void listen('todo:changed', () => void load()).then(fn => { if (active) stop = fn; else fn(); });
    return () => { active = false; stop?.(); };
  }, []);
  const changeState = async (view: AgreementView, status: string) => {
    setBusy(`${view.owner}:${view.id}`); setError('');
    try { await invoke('set_agreement_state', { characterId: view.owner, id: view.id, revision: view.task.revision, status }); await reload(); }
    catch (e) { setError(String(e)); await reload(); }
    finally { setBusy(''); }
  };
  const save = async (event: React.FormEvent) => {
    event.preventDefault(); setBusy('create'); setError('');
    try {
      await invoke('create_agreement', { characterId: draft.owner, title: draft.title.trim(), goal: draft.goal.trim(), trigger: draft.trigger.trim() });
      setCreating(false); setDraft({ owner: draft.owner, title: '', goal: '', trigger: '' }); setFilter('active'); await reload();
    } catch (e) { setError(String(e)); } finally { setBusy(''); }
  };
  const addTodo = async (event: React.FormEvent, view: AgreementView) => {
    event.preventDefault(); setBusy(`${view.owner}:${view.id}`); setError('');
    try {
      const value = await invoke<{ item: LinkedTodo }>('add_todo_item', { title: todoTitle.trim(), description: todoDescription.trim(),
        agreement: { character_id: view.owner, id: view.id, title: view.task.title } });
      setTodos(old => [...old.filter(item => item.id !== value.item.id), value.item]); setTodoFor(''); setTodoTitle(''); setTodoDescription('');
    } catch (e) { setError(String(e)); } finally { setBusy(''); }
  };
  const complete = async (id: string) => {
    setBusy(id); setError('');
    try { const value = await invoke<{ item: LinkedTodo }>('complete_todo_item', { id }); setTodos(old => old.map(item => item.id === id ? value.item : item)); }
    catch (e) { setError(String(e)); } finally { setBusy(''); }
  };
  const visible = records.filter(view => (filter === 'all' || view.task.status === filter)
    && (!selected || view.id === selected && (!selectedOwner || view.owner === selectedOwner)));
  return <div className="record-plan agreement-page">
    <p className="agreement-intro">{say('这里保存持续关注和维护约定。具体行动列入关联待办，完成一次行动后，约定仍然有效。',
      'Keep continuing interests and maintenance agreements here. Concrete actions have linked todos; finishing an action leaves its agreement in place.',
      '継続する関心と約束を管理します。具体的な作業は関連ToDoにし、完了しても約束は続きます。')}</p>
    <div className="record-plan-bar">
      <div className="rec-seg">{['active', 'candidate', 'paused', 'all'].map(value => <button type="button" key={value}
        className={`rec-seg-item${filter === value ? ' is-active' : ''}`} onClick={() => setFilter(value)}>
        {value === 'all' ? say('全部', 'All', 'すべて') : ongoingLabel(value, language)}</button>)}</div>
      <button type="button" className="rec-btn is-primary" onClick={() => setCreating(!creating)}><Plus size={14} />{say('新增约定', 'New agreement', '約束を追加')}</button>
    </div>
    {selected && <button className="rec-btn" type="button" onClick={() => nav?.navigateTo('planner', { sub: 'agreements' })}>{say('查看全部关注与约定', 'Show all agreements', 'すべての約束')}</button>}
    {(error || loadError) && <p className="agreement-error" role="alert">{error || loadError}</p>}
    {creating && <form className="agreement-form" onSubmit={save}>
      <label>{say('交给谁', 'Companion', '担当')}<select className="rec-input" value={draft.owner} onChange={e => setDraft({ ...draft, owner: e.target.value })}><option value="vivian">Vivian</option><option value="nana">Nana</option></select></label>
      <label>{say('标题', 'Title', 'タイトル')}<input className="rec-input" required maxLength={100} value={draft.title} onChange={e => setDraft({ ...draft, title: e.target.value })} /></label>
      <label>{say('持续维护什么', 'What to keep maintaining', '続けて管理すること')}<textarea className="rec-textarea" required maxLength={500} value={draft.goal} onChange={e => setDraft({ ...draft, goal: e.target.value })} /></label>
      <label>{say('遇到什么信息时更新', 'When to update it', '更新するきっかけ')}<textarea className="rec-textarea" required maxLength={300} value={draft.trigger} onChange={e => setDraft({ ...draft, trigger: e.target.value })} /></label>
      <div className="agreement-actions"><button className="rec-btn is-primary" disabled={!!busy}>{say('保存并启用', 'Save and enable', '保存して有効にする')}</button><button type="button" className="rec-btn" onClick={() => setCreating(false)}>{say('取消', 'Cancel', 'キャンセル')}</button></div>
    </form>}
    {loading ? <p className="rec-blank">{say('正在读取…', 'Loading…', '読み込み中…')}</p> : visible.length === 0 ? <p className="rec-blank">{say('这里还没有对应的关注与约定。', 'No matching agreements yet.', '該当する約束はまだありません。')}</p> : <div className="agreement-list">{visible.map(view => {
      const { task } = view; const key = `${view.owner}:${view.id}`;
      const linked = todos.filter(todo => matchesAgreement(todo, view));
      return <article className="agreement-card" data-agreement-id={view.id} key={key}>
        <div className="agreement-card-top"><Bookmark size={16} /><h3>{task.title}</h3><span>{view.owner === 'nana' ? 'Nana' : 'Vivian'}</span><span>{ongoingLabel(task.status, language)}</span></div>
        <p>{task.goal}</p><p className="agreement-trigger">{say('更新线索：', 'Update when: ', '更新のきっかけ：')}{task.trigger}</p>
        <div className="agreement-actions">
          {task.status === 'active' ? <button type="button" disabled={!!busy} className="rec-btn" onClick={() => void changeState(view, 'paused')}><Pause size={13} />{say('暂停维护', 'Pause', '一時停止')}</button>
            : <button type="button" disabled={!!busy} className="rec-btn" onClick={() => void changeState(view, 'active')}><Play size={13} />{task.status === 'candidate' ? say('启用这个约定', 'Enable agreement', '約束を有効にする') : say('恢复维护', 'Resume', '再開')}</button>}
          {!['cancelled', 'completed'].includes(task.status) && <button type="button" className="rec-btn" disabled={!!busy} onClick={() => void changeState(view, 'cancelled')}><Square size={12} />{say('停止维护', 'Stop', '終了')}</button>}
          {task.status === 'active' && <button type="button" className="rec-btn" onClick={() => { setTodoFor(todoFor === key ? '' : key); setTodoTitle(''); setTodoDescription(''); }}><Plus size={13} />{say('新增关联待办', 'Add linked todo', '関連ToDoを追加')}</button>}
        </div>
        {todoFor === key && <form className="agreement-form" onSubmit={event => void addTodo(event, view)}>
          <label>{say('这次要完成的具体行动', 'Concrete action to finish', '今回完了する具体的な作業')}<input className="rec-input" required maxLength={200} value={todoTitle} onChange={e => setTodoTitle(e.target.value)} /></label>
          <label>{say('完成条件或补充说明', 'Completion criteria or details', '完了条件や補足')}<textarea className="rec-textarea" value={todoDescription} onChange={e => setTodoDescription(e.target.value)} /></label>
          <button disabled={!!busy || !todoTitle.trim()} className="rec-btn is-primary">{say('保存待办', 'Save todo', 'ToDoを保存')}</button>
        </form>}
        <details className="agreement-collection"><summary>{say('对应清单', 'Collection', '対応するリスト')} · {task.entries.length}</summary>
          {task.entries.length === 0 ? <p>{say('有明确的新信息时，逐条补充到这里。', 'Entries appear here as supported updates arrive.', '確かな新情報をここに追加します。')}</p> : <ul>{task.entries.map(entry => <li key={entry.key}><strong>{entry.label}</strong> · {ongoingLabel(entry.state, language)}{entry.detail && <p>{entry.detail}</p>}<blockquote>{entry.evidence.quote}</blockquote></li>)}</ul>}
        </details>
        {linked.length > 0 && <div className="agreement-todos"><h4>{say('关联待办', 'Linked todos', '関連ToDo')}</h4>{linked.map(todo => <div key={todo.id}><span>{todo.title}</span><span>{todo.completed ? say('已完成', 'Completed', '完了') : say('待完成', 'Pending', '未完了')}</span>{!todo.completed && <button type="button" className="rec-icon-btn" disabled={!!busy} onClick={() => void complete(todo.id)} aria-label={say('完成待办', 'Complete todo', 'ToDoを完了')}><Check size={15} /></button>}</div>)}</div>}
        <button type="button" className="rec-btn" onClick={() => nav?.navigateTo('planner', { sub: 'todo', agreementId: view.id, agreementOwner: view.owner })}>{say('打开关联待办', 'Open linked todos', '関連ToDoを開く')}<ArrowUpRight size={13} /></button>
      </article>;
    })}</div>}
  </div>;
}
