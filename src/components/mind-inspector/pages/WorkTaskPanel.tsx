import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { ListChecks } from 'lucide-react';
import './WorkTaskPanel.css';

/**
 * 任务轨与 composer 上方任务坞共用同一份后端投影。
 * 卡片只是投影：状态权威在 WorkTaskStore，前端不自己推导状态、不缓存完整结果。
 * 数据量很小，事件只推一条变化的摘要，前端按 job_id 合并。
 */

type TaskActivity = {
  at: number;
  kind: string;
  text: string;
};

type TaskMandate = {
  tools?: string[];
  workspace_roots?: string[];
  permission?: string;
  depth?: number;
};

type WorkTaskView = {
  job_id: string;
  session_id?: string;
  status: string;
  bucket: string;
  label: string;
  task: string;
  deliverable?: string | null;
  requires_review: boolean;
  result_revision: number;
  accepted_revision?: number | null;
  updated_at: number;
  origin_message_id?: string | null;
  needs_you?: boolean;
  review_reason?: string | null;
  activity?: TaskActivity[];
  /** 等用户输入时那一个具体问题。 */
  blocker?: string | null;
  /** Worker 最近一步，只有运行中才有意义。 */
  latest_step?: string | null;
  /** 派发那一刻冻结的授权范围。 */
  mandate?: TaskMandate | null;
};

type ChangedEvent = { session_id: string; job: WorkTaskView };

const DOCK_ROWS = 5;
const MANUAL_SELECTION_HOLD_MS = 8_000;
const FOLLOW_PRIORITY = ['in_progress', 'needs_review', 'needs_input', 'done'];

function useWorkTasks(sessionId: string | null) {
  const [items, setItems] = useState<WorkTaskView[]>([]);
  const reload = useCallback(async () => {
    if (!sessionId) {
      setItems([]);
      return;
    }
    try {
      setItems(await invoke<WorkTaskView[]>('work_task_list', { sessionId }));
    } catch {
      // 命令不可用时保持上一份投影，不放假数据充数。
    }
  }, [sessionId]);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    if (!sessionId) return;
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    void listen<ChangedEvent>('work_task:changed', (event) => {
      if (event.payload.session_id !== sessionId) return;
      const job = event.payload.job;
      setItems((prev) => {
        const index = prev.findIndex((item) => item.job_id === job.job_id);
        if (index < 0) return [...prev, job];
        const next = prev.slice();
        next[index] = job;
        return next;
      });
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [sessionId]);

  return { items, reload };
}

/**
 * 跨会话「需要你」投影。
 *
 * 任务属于会话，但人的注意力不属于任何一个会话：任何任务发生变化都重读
 * 一份全局摘要，而不是让用户逐个切会话去找等自己的事。
 */
function useAttentionTasks() {
  const [items, setItems] = useState<WorkTaskView[]>([]);
  const reload = useCallback(async () => {
    try {
      setItems(await invoke<WorkTaskView[]>('work_task_attention'));
    } catch {
      // 命令不可用时保持上一份投影。
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    void listen<ChangedEvent>('work_task:changed', () => {
      void reload();
    }).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [reload]);

  return items;
}

function useLabels() {
  const { i18n } = useTranslation();
  return (zh: string, en: string, ja: string) =>
    i18n.language.startsWith('ja') ? ja : i18n.language.startsWith('en') ? en : zh;
}

function statusDot(bucket: string) {
  if (bucket === 'needs_input') return 'work-task-dot-input';
  if (bucket === 'needs_review') return 'work-task-dot-review';
  if (bucket === 'done') return 'work-task-dot-done';
  return 'work-task-dot-run';
}

type AttentionSession = { session_id: string; title: string };

/**
 * composer 上方的全局「需要你」条。
 *
 * 对齐 Comma 的注意力原则：健康状态不占空间，只有真等人的事才出现，
 * 并且一行就能跳到对应会话——用户不必满屏切窗口找任务。
 */
export function WorkAttentionBar({
  currentSessionId,
  sessions,
  onOpenSession,
}: {
  currentSessionId: string | null;
  sessions: AttentionSession[];
  onOpenSession: (sessionId: string) => void;
}) {
  const items = useAttentionTasks();
  const pick = useLabels();
  const [expanded, setExpanded] = useState(
    () => localStorage.getItem('vivian.workAttention.expanded') === '1',
  );
  if (!items.length) return null;

  const toggle = () => {
    const next = !expanded;
    setExpanded(next);
    localStorage.setItem('vivian.workAttention.expanded', next ? '1' : '0');
  };

  const titleOf = (sessionId: string | undefined) => {
    if (!sessionId) return '';
    const match = sessions.find((session) => session.session_id === sessionId);
    const title = match?.title?.trim();
    if (title) return title;
    return sessionId === currentSessionId
      ? pick('本会话', 'This session', 'この会話')
      : sessionId;
  };
  const waitingInput = items.filter((item) => item.bucket === 'needs_input').length;
  const waitingReview = items.filter((item) => item.bucket === 'needs_review').length;
  const failed = items.filter((item) => item.bucket === 'closed').length;
  const visible = expanded ? items : items.slice(0, 2);

  return (
    <section className="work-attention" aria-label={pick('需要你处理', 'Needs you', 'あなたの対応待ち')}>
      <button type="button" className="work-attention-head" onClick={toggle} aria-expanded={expanded}>
        <span className="work-attention-mark" aria-hidden />
        <span className="work-attention-title">
          {pick('需要你', 'Needs you', '対応待ち')} · {items.length}
        </span>
        <span className="work-attention-counts">
          {waitingInput > 0 && (
            <span className="work-attention-count is-input">
              {pick('待输入', 'Input', '入力')} {waitingInput}
            </span>
          )}
          {waitingReview > 0 && (
            <span className="work-attention-count is-review">
              {pick('待验收', 'Review', '確認')} {waitingReview}
            </span>
          )}
          {failed > 0 && (
            <span className="work-attention-count is-failed">
              {pick('失败', 'Failed', '失敗')} {failed}
            </span>
          )}
        </span>
        <span className="work-attention-chevron">{expanded ? '▾' : '▸'}</span>
      </button>
      <div className="work-attention-rows">
        {visible.map((item) => {
          const isCurrent = item.session_id === currentSessionId;
          return (
            <button
              type="button"
              key={item.job_id}
              className={`work-attention-row ${isCurrent ? 'is-current' : ''}`}
              title={item.review_reason || item.task}
              onClick={() => {
                if (item.session_id && !isCurrent) onOpenSession(item.session_id);
              }}
            >
              <span className={`work-task-dot ${statusDot(item.bucket)}`} />
              <span className="work-attention-task">{item.task}</span>
              <span className="work-attention-label">{item.label}</span>
              <span className="work-attention-session">{titleOf(item.session_id)}</span>
            </button>
          );
        })}
        {!expanded && items.length > visible.length && (
          <button type="button" className="work-attention-more" onClick={toggle}>
            {pick(
              `另有 ${items.length - visible.length} 项`,
              `${items.length - visible.length} more`,
              `ほか ${items.length - visible.length} 件`,
            )}
          </button>
        )}
      </div>
    </section>
  );
}

/** composer 上方：本会话创建的任务，按创建顺序；折叠状态跨会话保留。 */
const DISMISSED_KEY = 'vivian.workTaskDock.dismissed';

export function WorkTaskDock({ sessionId }: { sessionId: string | null }) {
  const { items, reload } = useWorkTasks(sessionId);
  const pick = useLabels();
  const [collapsed, setCollapsed] = useState(
    () => localStorage.getItem('vivian.workTaskDock.collapsed') === '1',
  );
  const [dismissed, setDismissed] = useState<string[]>(() => {
    try {
      const saved = JSON.parse(localStorage.getItem(DISMISSED_KEY) || '[]');
      return Array.isArray(saved) ? saved.filter((id): id is string => typeof id === 'string') : [];
    } catch {
      return [];
    }
  });
  const [busy, setBusy] = useState<string | null>(null);

  const toggle = () => {
    const next = !collapsed;
    setCollapsed(next);
    localStorage.setItem('vivian.workTaskDock.collapsed', next ? '1' : '0');
  };

  const reveal = (jobId: string) => {
    // 消息内任务卡与任务坞读同一份投影，按 job_id 直接定位，不另建一套索引。
    document
      .querySelector(`[data-task-id="${jobId}"]`)
      ?.scrollIntoView({ behavior: 'smooth', block: 'center' });
  };
  const dismiss = (jobId: string) => {
    const next = [...dismissed, jobId].slice(-200);
    setDismissed(next);
    localStorage.setItem(DISMISSED_KEY, JSON.stringify(next));
  };
  const cancel = async (jobId: string) => {
    if (!sessionId || busy) return;
    setBusy(jobId);
    try {
      await invoke('work_task_cancel', { sessionId, jobId });
      await reload();
    } catch {
      // 已终结的任务由后端拒绝，前端不伪造状态。
    } finally {
      setBusy(null);
    }
  };

  const rows = items.filter((item) => !dismissed.includes(item.job_id));
  if (!rows.length) return null;

  return (
    <div className="work-task-dock" data-collapsed={collapsed || undefined}>
      <button type="button" className="work-task-dock-toggle" onClick={toggle} aria-expanded={!collapsed}>
        <span>{pick('本会话的任务', 'Tasks in this session', 'この会話のタスク')} · {rows.length}</span>
        <span className="work-task-dock-chevron">{collapsed ? '▸' : '▾'}</span>
      </button>
      {!collapsed && (
        <div className="work-task-dock-rows">
          {rows.slice(0, DOCK_ROWS).map((item) => (
            <div className="work-task-dock-row" key={item.job_id} title={item.task}>
              <span className={`work-task-dot ${statusDot(item.bucket)}`} />
              <span className="work-task-dock-title">{item.task}</span>
              <span className="work-task-dock-label">{item.label}</span>
              <span className="work-task-dock-acts" onClick={(event) => event.stopPropagation()}>
                <button type="button" className="work-task-dock-act" onClick={() => reveal(item.job_id)}>
                  {pick('跳到原消息', 'Reveal in chat', '元の発言へ')}
                </button>
                <button type="button" className="work-task-dock-act" onClick={() => dismiss(item.job_id)}>
                  {pick('忽略', 'Dismiss', '非表示')}
                </button>
                {!item.bucket || item.bucket === 'in_progress' || item.bucket === 'needs_input' ? (
                  <button
                    type="button"
                    className="work-task-dock-act"
                    disabled={busy === item.job_id}
                    onClick={() => void cancel(item.job_id)}
                  >
                    {pick('取消', 'Cancel', 'キャンセル')}
                  </button>
                ) : null}
              </span>
            </div>
          ))}
          {rows.length > DOCK_ROWS && (
            <div className="work-task-dock-more">
              {pick(`另有 ${rows.length - DOCK_ROWS} 项在右侧任务轨`, `${rows.length - DOCK_ROWS} more in the rail`, `ほか ${rows.length - DOCK_ROWS} 件は右のレーン`)}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/**
 * 消息内任务卡：紧跟「派发任务」的那条工具结果。
 *
 * 与任务坞、任务轨读同一份后端投影，只按 job_id 取一条；不自己推导状态、
 * 不缓存结果正文。任务号由宿主生成的 `job_<n>` 文本里取出——工具结果是宿主
 * 写的，不依赖模型自述。
 */
export function WorkTaskInlineCard({
  sessionId,
  jobId,
}: {
  sessionId: string | null;
  jobId: string;
}) {
  const pick = useLabels();
  const [task, setTask] = useState<WorkTaskView | null>(null);

  useEffect(() => {
    if (!sessionId) return;
    let alive = true;
    void invoke<WorkTaskView[]>('work_task_list', { sessionId })
      .then((items) => {
        if (alive) setTask(items.find((item) => item.job_id === jobId) ?? null);
      })
      .catch(() => {
        // 命令不可用时保持空白，不放占位假数据。
      });
    let unlisten: UnlistenFn | undefined;
    void listen<ChangedEvent>('work_task:changed', (event) => {
      if (event.payload.session_id !== sessionId || event.payload.job.job_id !== jobId) return;
      setTask(event.payload.job);
    }).then((fn) => {
      if (alive) unlisten = fn;
      else fn();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [sessionId, jobId]);

  if (!task) return null;
  const waiting = task.bucket === 'needs_review' && task.accepted_revision !== task.result_revision;
  return (
    <div
      className="work-task-inline"
      data-bucket={task.bucket}
      data-task-id={task.job_id}
      title={task.review_reason || task.deliverable || task.task}
    >
      <span className={`work-task-dot ${statusDot(task.bucket)}`} />
      <span className="work-task-inline-title">{task.task}</span>
      <span className="work-task-inline-label">
        {waiting ? pick('待你验收', 'Needs your review', '確認待ち') : task.label}
      </span>
    </div>
  );
}

/** 右栏常驻：四态分组、自动跟随、精确版本验收与打回。 */
export function WorkTaskRail({ sessionId }: { sessionId: string | null }) {
  const { items, reload } = useWorkTasks(sessionId);
  const pick = useLabels();
  const [bucket, setBucket] = useState('in_progress');
  const [openId, setOpenId] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [replyFor, setReplyFor] = useState<string | null>(null);
  const [reply, setReply] = useState('');
  const manualAt = useRef(0);
  const knownIds = useRef<Set<string> | null>(null);

  const buckets = [
    { key: 'in_progress', label: pick('进行中', 'In progress', '進行中') },
    { key: 'needs_input', label: pick('待你输入', 'Needs input', '入力待ち') },
    { key: 'needs_review', label: pick('待验收', 'Needs review', '確認待ち') },
    { key: 'done', label: pick('已完成', 'Done', '完了') },
  ];
  const count = (key: string) => items.filter((item) => item.bucket === key).length;

  // 新任务到达时跟随到它的状态桶；用户自己选过则保留 8 秒。
  useEffect(() => {
    const ids = new Set(items.map((item) => item.job_id));
    const known = knownIds.current;
    knownIds.current = ids;
    if (known && Date.now() - manualAt.current > MANUAL_SELECTION_HOLD_MS) {
      const arrived = items.find((item) => !known.has(item.job_id));
      if (arrived) {
        setBucket(arrived.bucket);
        return;
      }
    }
    if (count(bucket) === 0 && Date.now() - manualAt.current > MANUAL_SELECTION_HOLD_MS) {
      const fallback = FOLLOW_PRIORITY.find((key) => count(key) > 0);
      if (fallback) setBucket(fallback);
    }
  }, [items]); // eslint-disable-line react-hooks/exhaustive-deps

  const act = async (
    jobId: string,
    action: 'accept' | 'reject' | 'cancel' | 'provide_input',
    extra?: { revision?: number; note?: string },
  ) => {
    if (!sessionId || busy) return;
    setBusy(jobId);
    const commands = {
      accept: 'work_task_accept',
      reject: 'work_task_reject',
      cancel: 'work_task_cancel',
      provide_input: 'work_task_provide_input',
    };
    try {
      await invoke(commands[action], {
        sessionId,
        jobId,
        ...(action === 'accept' ? { revision: extra?.revision } : {}),
        ...(action === 'reject' ? { reason: extra?.note || '请再检查一次结果' } : {}),
        ...(action === 'provide_input' ? { note: extra?.note || '' } : {}),
      });
      await reload();
      if (action === 'provide_input') {
        setReplyFor(null);
        setReply('');
      }
    } catch {
      // 状态迁移被后端拒绝时保留原卡片；不本地伪造一个成功状态。
    } finally {
      setBusy(null);
    }
  };

  const visible = items.filter((item) => item.bucket === bucket);

  return (
    <div className="codex-info-card work-task-rail">
      <div className="codex-info-title">
        <ListChecks size={13} />
        {pick('任务轨', 'Tasks', 'タスク')}
        {items.length > 0 && <span className="codex-deliverable-count">{items.length}</span>}
      </div>
      <div className="work-task-buckets">
        {buckets.map((entry) => (
          <button
            type="button"
            key={entry.key}
            className={`work-task-bucket ${bucket === entry.key ? 'active' : ''}`}
            onClick={() => {
              manualAt.current = Date.now();
              setBucket(entry.key);
            }}
          >
            {entry.label}<span className="work-task-bucket-n">{count(entry.key)}</span>
          </button>
        ))}
      </div>
      <div className="work-task-cards">
        {visible.length === 0 && (
          <div className="codex-empty-note">
            {pick('这里没有任务', 'Nothing here', 'タスクはありません')}
          </div>
        )}
        {visible.map((item) => {
          const attention = item.bucket === 'needs_review'
            && item.accepted_revision !== item.result_revision;
          return (
            <div
              className={`work-task-card ${openId === item.job_id ? 'open' : ''}`}
              key={item.job_id}
              onClick={() => setOpenId(openId === item.job_id ? null : item.job_id)}
            >
              {attention && <span className="work-task-attention" title={pick('待验收', 'Needs review', '確認待ち')} />}
              <div className="work-task-card-top">
                <span className={`work-task-dot ${statusDot(item.bucket)}`} />
                <span className="work-task-card-title">{item.task}</span>
              </div>
              <div className="work-task-card-meta">
                <span>{item.label}</span>
                <span>rev {item.result_revision}</span>
                <span>{item.job_id}</span>
              </div>
              {item.bucket === 'in_progress' && item.latest_step && (
                <div className="work-task-latest-step">{item.latest_step}</div>
              )}
              {item.bucket === 'needs_input' && item.blocker && (
                <div className="work-task-latest-step is-blocked">{item.blocker}</div>
              )}
              {openId === item.job_id && (
                <div className="work-task-card-detail" onClick={(event) => event.stopPropagation()}>
                  {item.deliverable && (
                    <div className="work-task-detail-line">
                      {pick('交付物', 'Deliverable', '成果物')}：{item.deliverable}
                    </div>
                  )}
                  {item.origin_message_id && (
                    <div className="work-task-detail-line">
                      {pick('来源', 'Origin', '出所')}：{item.origin_message_id}
                    </div>
                  )}
                  {item.mandate && (
                    <div className="work-task-mandate">
                      <div className="work-task-mandate-head">
                        {pick('授权快照', 'Mandate', '権限スナップショット')} · {item.mandate.permission || '—'}
                      </div>
                      {(item.mandate.tools?.length ?? 0) > 0 && (
                        <div className="work-task-mandate-line">
                          {pick('工具', 'Tools', 'ツール')}：{item.mandate.tools!.slice(0, 8).join('、')}
                          {item.mandate.tools!.length > 8 ? '…' : ''}
                        </div>
                      )}
                      {(item.mandate.workspace_roots?.length ?? 0) > 0 && (
                        <div className="work-task-mandate-line">
                          {pick('目录', 'Roots', 'ディレクトリ')}：{item.mandate.workspace_roots!.slice(0, 2).join('、')}
                        </div>
                      )}
                    </div>
                  )}
                  {item.bucket === 'needs_input' && (
                    <div className="work-task-question">
                      <div className="work-task-detail-line">
                        {item.blocker || pick('这个任务需要你补充信息。', 'This task needs your input.', '追加情報が必要です。')}
                      </div>
                      {replyFor === item.job_id ? (
                        <>
                          <textarea
                            className="work-task-input"
                            rows={2}
                            value={reply}
                            placeholder={pick('补充信息…', 'Your answer…', '追加情報…')}
                            onChange={(event) => setReply(event.target.value)}
                          />
                          <div className="work-task-actions">
                            <button
                              type="button"
                              className="work-task-btn primary"
                              disabled={busy === item.job_id || !reply.trim()}
                              onClick={() => void act(item.job_id, 'provide_input', { note: reply })}
                            >
                              {pick('提交', 'Send', '送信')}
                            </button>
                            <button type="button" className="work-task-btn" onClick={() => setReplyFor(null)}>
                              {pick('取消输入', 'Cancel', 'やめる')}
                            </button>
                          </div>
                        </>
                      ) : (
                        <div className="work-task-actions">
                          <button
                            type="button"
                            className="work-task-btn"
                            onClick={() => {
                              setReplyFor(item.job_id);
                              setReply('');
                            }}
                          >
                            {pick('回答这一个问题', 'Answer this question', 'この質問に答える')}
                          </button>
                        </div>
                      )}
                    </div>
                  )}
                  {item.activity && item.activity.length > 0 && (
                    <div className="work-task-timeline">
                      {item.activity.slice(0, 6).map((entry, index) => (
                        <div className="work-task-timeline-row" key={`${entry.at}-${index}`}>
                          <span className={`work-task-timeline-dot is-${entry.kind}`} />
                          <span className="work-task-timeline-text">{entry.text}</span>
                          <span className="work-task-timeline-time">
                            {new Date(entry.at * 1000).toLocaleTimeString([], {
                              hour: '2-digit',
                              minute: '2-digit',
                            })}
                          </span>
                        </div>
                      ))}
                    </div>
                  )}
                  {item.bucket === 'needs_review' && (
                    <>
                      <div className="work-task-detail-line">
                        {item.review_reason
                          || pick('改动过工作区文件或结果不完整，需要你确认。', 'Workspace files changed or the result is incomplete.', 'ファイル変更または結果が不完全なため確認が必要です。')}
                      </div>
                      <div className="work-task-actions">
                        <button
                          type="button"
                          className="work-task-btn primary"
                          disabled={busy === item.job_id}
                          onClick={() => void act(item.job_id, 'accept', { revision: item.result_revision })}
                        >
                          {pick('验收通过', 'Accept', '承認')}
                        </button>
                        <button
                          type="button"
                          className="work-task-btn"
                          disabled={busy === item.job_id}
                          onClick={() => void act(item.job_id, 'reject')}
                        >
                          {pick('打回修改', 'Send back', '差し戻し')}
                        </button>
                      </div>
                    </>
                  )}
                  {!item.bucket?.startsWith('closed') && item.bucket !== 'done' && (
                    <div className="work-task-actions">
                      <button
                        type="button"
                        className="work-task-btn ghost"
                        disabled={busy === item.job_id}
                        onClick={() => void act(item.job_id, 'cancel')}
                      >
                        {pick('取消任务', 'Cancel task', 'タスクを中止')}
                      </button>
                    </div>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
