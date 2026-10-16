import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useVirtualizer } from '@tanstack/react-virtual';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { ArrowRight, GitFork, X } from 'lucide-react';
import { deriveWorkflow, mergeWorkflowRequests, workflowDuration, type WorkflowDetail, type WorkflowMessage, type WorkflowRequest, type WorkflowTool, type WorkflowView } from './workflowModel';
import TrajectoryPanel from './TrajectoryPanel';
import WorkflowJson from './WorkflowJson';
import './workflowStrings';
import './WorkflowPanel.css';

type Selection = { kind: 'request' | 'response'; request: WorkflowRequest } | { kind: 'tool'; tool: WorkflowTool };

function Detail({ sessionId, selection, onClose }: { sessionId: string; selection: Selection; onClose: () => void }) {
  const { t } = useTranslation();
  const dialog = useRef<HTMLDialogElement>(null);
  const [detail, setDetail] = useState<WorkflowDetail | null>(null);
  const [error, setError] = useState('');
  const [retry, setRetry] = useState(0);
  const requestId = selection.kind === 'tool' ? null : selection.request.id;
  const completion = selection.kind === 'tool' ? null : selection.request.completed_at;
  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => element?.close();
  }, []);
  useEffect(() => {
    if (!requestId) return;
    let disposed = false;
    setDetail(null); setError('');
    void invoke<WorkflowDetail>('coding_workflow_detail', { sessionId, requestId }).then(result => {
      if (!disposed) setDetail(result);
    }).catch(cause => { if (!disposed) setError(String(cause)); });
    return () => { disposed = true; };
  }, [sessionId, requestId, completion, retry]);
  let result: unknown = selection.kind === 'tool' ? selection.tool.message?.content : null;
  if (typeof result === 'string') { try { result = JSON.parse(result); } catch { /* Plain tool output. */ } }
  return <dialog ref={dialog} className="workflow-dialog" aria-labelledby="workflow-detail-title" onCancel={onClose}>
    <header><h3 id="workflow-detail-title">{selection.kind === 'tool' ? selection.tool.name : t('workflow.snapshot')}</h3>
      <button type="button" onClick={onClose} aria-label={t('workflow.close')} autoFocus><X size={18} /></button></header>
    <div className="workflow-dialog-body">
      {selection.kind === 'tool' ? <>
        <span className={`workflow-status ${selection.tool.status}`}>{t(`workflow.${selection.tool.status}`)}</span>
        <WorkflowJson label={t('workflow.arguments')} value={selection.tool.message?.tool_arguments ?? null} />
        <WorkflowJson label={t('workflow.result')} value={result ?? t('workflow.noResult')} />
      </> : <>
        <p className="workflow-capture-note">{t('workflow.captureNote')}</p>
        {error ? <div role="alert">{t('workflow.requestFailed', { error })} <button type="button" onClick={() => setRetry(n => n + 1)}>{t('workflow.retry')}</button></div>
          : !detail ? <p role="status">{t('workflow.loading')}</p>
          : selection.kind === 'response' ? <WorkflowJson label={t('workflow.response')} value={detail.response} />
          : <>
            <WorkflowJson label={t('workflow.system')} value={detail.system} />
            <WorkflowJson label={t('workflow.messages')} value={detail.messages} />
            <WorkflowJson label={t('workflow.tools')} value={detail.tools} />
            <WorkflowJson label={t('workflow.reasoning')} value={detail.reasoning} />
          </>}
      </>}
    </div>
  </dialog>;
}

export default function WorkflowPanel({ sessionId, messages, running, toolLabel, view, onViewChange }: {
  sessionId: string | null; messages: readonly WorkflowMessage[]; running: boolean; toolLabel: (name: string) => string;
  view: WorkflowView; onViewChange: (view: WorkflowView) => void;
}) {
  const { t } = useTranslation();
  const [requests, setRequests] = useState<WorkflowRequest[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [retry, setRetry] = useState(0);
  const [anchor, setAnchor] = useState<number | null>(null);
  const [showTurns, setShowTurns] = useState(false);
  const [selection, setSelection] = useState<Selection | null>(null);
  const scroller = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!sessionId) return;
    let disposed = false;
    const unlisten: UnlistenFn[] = [];
    setLoading(true); setError('');
    // Subscribe before listing to retain settlements arriving during the disk read.
    void (async () => {
      for (const event of ['coding:workflow', 'coding:workflow_error']) {
        const stop = await listen<{ session_id: string; request?: WorkflowRequest; message?: string }>(event, ({ payload }) => {
          if (disposed || payload.session_id !== sessionId) return;
          if (payload.request) setRequests(current => mergeWorkflowRequests(current, [payload.request!]));
          if (payload.message) setError(payload.message);
        });
        if (disposed) { stop(); return; }
        unlisten.push(stop);
      }
      const stored = await invoke<WorkflowRequest[]>('coding_workflow_list', { sessionId });
      if (!disposed) setRequests(current => mergeWorkflowRequests(current, stored));
    })().catch(cause => { if (!disposed) setError(String(cause)); })
      .finally(() => { if (!disposed) setLoading(false); });
    return () => { disposed = true; unlisten.forEach(stop => stop()); };
  }, [sessionId, retry]);

  const model = useMemo(() => deriveWorkflow(messages, requests, running), [messages, requests, running]);
  const turn = model.turns.find(item => item.anchor === anchor) ?? model.turns[model.turns.length - 1];
  const calls = turn?.calls ?? [];
  const virtual = useVirtualizer({ count: calls.length, getScrollElement: () => scroller.current,
    estimateSize: () => 190, overscan: 3, getItemKey: index => calls[index].request.id });
  useEffect(() => { scroller.current?.scrollTo({ top: 0 }); }, [turn?.anchor]);
  const assigned = new Set(calls.flatMap(call => call.tools.map(tool => tool.key)));
  const orphans = turn?.tools.filter(tool => !assigned.has(tool.key)) ?? [];
  const liveSelection = selection?.kind === 'tool'
    ? { ...selection, tool: model.turns.flatMap(item => [...item.tools, ...item.calls.flatMap(call => call.tools)]).find(tool => tool.key === selection.tool.key) ?? selection.tool }
    : selection ? { ...selection, request: requests.find(request => request.id === selection.request.id) ?? selection.request } : null;
  const toolCard = (tool: WorkflowTool) => <button type="button" key={tool.key} className={`workflow-card tool ${tool.status}`} onClick={() => setSelection({ kind: 'tool', tool })}>
    <span className="workflow-card-label">{t('workflow.tool')} · {t(`workflow.${tool.status}`)}</span>
    <strong>{toolLabel(tool.name)}</strong>
    <span className="workflow-preview">{tool.message?.content.slice(0, 160) || t('workflow.noResult')}</span>
    <small>{workflowDuration(tool.message?.tool_duration_ms ?? null)}</small>
  </button>;

  return <div className="workflow-panel">
    <div className="workflow-view-switch" role="group" aria-label={t('workflow.view')}>
      <button type="button" aria-pressed={view === 'flow'} onClick={() => onViewChange('flow')}>{t('workflow.flow')}</button>
      <button type="button" aria-pressed={view === 'events'} onClick={() => onViewChange('events')}>{t('workflow.events')}</button>
    </div>
    {view === 'events' ? <div className="workflow-events">
      <TrajectoryPanel messages={messages} running={running} toolLabel={toolLabel} />
    </div> : <>
    <div className="workflow-metrics">{[
      [t('workflow.turns'), model.turns.length], [t('workflow.requests'), model.requestCount],
      [t('workflow.toolsCount'), model.toolCount], [t('workflow.llmTime'), workflowDuration(model.llmMs)],
    ].map(([label, value]) => <div key={label}><strong>{value}</strong><span>{label}</span></div>)}</div>
    {error && <div className="workflow-error" role="alert">{t('workflow.requestFailed', { error })} <button type="button" onClick={() => setRetry(n => n + 1)}>{t('workflow.retry')}</button></div>}
    {loading && <span role="status">{t('workflow.loading')}</span>}
    {!turn ? <p className="workflow-empty"><GitFork size={22} />{t('workflow.empty')}</p> : <>
      <div className="workflow-turn-picker">
        <select value={turn.anchor} aria-label={t('workflow.selectTurn')} onChange={event => setAnchor(Number(event.target.value))}>
          {model.turns.map((item, index) => <option key={item.anchor} value={item.anchor}>{t('workflow.turn', { n: index + 1 })} · {item.prompt.slice(0, 40)}</option>)}
        </select>
        <button type="button" aria-expanded={showTurns} onClick={() => setShowTurns(value => !value)}>{t(showTurns ? 'workflow.hideTurns' : 'workflow.showTurns')}</button>
      </div>
      {showTurns && <nav className="workflow-turns" aria-label={t('workflow.turns')}>
        {model.turns.map((item, index) => <button type="button" key={item.anchor} className={item.anchor === turn.anchor ? 'active' : ''} aria-current={item.anchor === turn.anchor ? 'step' : undefined} onClick={() => setAnchor(item.anchor)}>
          <span>{t('workflow.turn', { n: index + 1 })} · {item.calls.length} / {item.tools.length}</span><strong>{item.prompt.slice(0, 100)}</strong>
          <span className={`workflow-status ${item.status}`}>{t(`workflow.${item.status}`)}</span>
        </button>)}
      </nav>}
      <div className="workflow-turn-heading"><strong title={turn.prompt}>{turn.prompt.slice(0, 160)}</strong><span className={`workflow-status ${turn.status}`}>{t(`workflow.${turn.status}`)}</span></div>
      <div ref={scroller} className="workflow-calls">
        {!calls.length && <p className="workflow-empty">{t('workflow.noCapture')}</p>}
        <div style={{ height: virtual.getTotalSize(), position: 'relative', width: '100%' }}>
          {virtual.getVirtualItems().map(item => {
            const { request, tools } = calls[item.index];
            const usage = request.usage;
            return <article key={item.key} ref={virtual.measureElement} data-index={item.index} className="workflow-call" style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${item.start}px)` }}>
              <header><strong>#{item.index + 1} · {t(`workflow.${request.purpose}`, { defaultValue: request.purpose })}</strong><span>{t('workflow.attempt', { n: request.attempt })} · {workflowDuration(request.duration_ms)}</span></header>
              <div className="workflow-chain">
                <button type="button" className={`workflow-card request ${request.status}`} onClick={() => setSelection({ kind: 'request', request })}>
                  <span className="workflow-card-label">{t('workflow.request')} · {t(`workflow.${request.status}`)}</span><strong>{request.model || '—'}</strong>
                  <span className="workflow-preview">{request.error || new Date(request.started_at).toLocaleTimeString()}</span><small>{t('workflow.inspect')}</small>
                </button>
                <ArrowRight size={16} className="workflow-arrow" />
                <button type="button" className="workflow-card response" onClick={() => setSelection({ kind: 'response', request })}>
                  <span className="workflow-card-label">{t('workflow.response')}</span><span className="workflow-preview">{request.response_preview || t('workflow.noResult')}</span>
                  <small>{usage ? `${t('workflow.input')} ${usage.input_tokens + usage.cache_read_tokens + usage.cache_write_tokens} · ${t('workflow.output')} ${usage.output_tokens}` : t('workflow.unknownUsage')}</small>
                </button>
                {tools.length > 0 && <ArrowRight size={16} className="workflow-arrow" />}{tools.map(toolCard)}
              </div>
              {usage && <div className="workflow-usage">{t('workflow.uncached')} {usage.input_tokens} · {t('workflow.cacheRead')} {usage.cache_read_tokens} · {t('workflow.cacheWrite')} {usage.cache_write_tokens}</div>}
            </article>;
          })}
        </div>
        {orphans.length > 0 && <section className="workflow-orphans"><h4>{t('workflow.orphanTools')}</h4><div className="workflow-chain">{orphans.map(toolCard)}</div></section>}
      </div>
    </>}
    </>}
    {liveSelection && sessionId && <Detail sessionId={sessionId} selection={liveSelection} onClose={() => setSelection(null)} />}
  </div>;
}
