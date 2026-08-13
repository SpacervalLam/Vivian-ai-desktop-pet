/**
 * Scheduler 页 — 定时任务列表 + 表单
 *
 * 数据源：invoke('list_scheduled_tasks') / invoke('add_scheduled_reminder') / ...
 * 刷新：监听 scheduler:changed 事件
 *
 * 从 SchedulerWindow.tsx 改造：去除窗口外壳（标题栏/minimize/close/getCurrentWindow），
 * 适配 MindInspector 的 page 渲染模式。
 */

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import LoadingSpinner from '../../LoadingSpinner';

type TaskStatus = 'pending' | 'running' | 'completed' | 'cancelled' | 'failed' | 'paused';
type TaskType = 'reminder' | 'tool_call';

interface ScheduledTask {
  id: string;
  task_type: TaskType;
  scheduled_time: number;
  message?: string | null;
  tool_name?: string | null;
  tool_arguments?: unknown;
  repeat_interval?: number | null;
  status: TaskStatus;
  created_at: number;
}

type Tab = 'active' | 'history' | 'all';

const STATUS_COLORS: Record<TaskStatus, string> = {
  pending: '#FF9800',
  running: '#4CAF50',
  completed: '#9E9E9E',
  cancelled: '#9E9E9E',
  failed: '#E53935',
  paused: '#FFC107',
};

function formatRemaining(ts: number, now: number): string {
  const diff = ts - now;
  if (diff <= 0) return '0s';
  const days = Math.floor(diff / 86400);
  const hours = Math.floor((diff % 86400) / 3600);
  const minutes = Math.floor((diff % 3600) / 60);
  const seconds = Math.floor(diff % 60);
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

function formatDateTime(ts: number): string {
  const d = new Date(ts * 1000);
  const pad = (n: number) => n.toString().padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** 将本地 datetime-local 字符串（YYYY-MM-DDTHH:MM）转换为 Unix 秒时间戳 */
function localDateTimeToTimestamp(value: string): number {
  return Math.floor(new Date(value).getTime() / 1000);
}

/** 将 Unix 秒时间戳转换为 datetime-local 字符串 */
function timestampToLocalDateTime(ts: number): string {
  const d = new Date(ts * 1000);
  const pad = (n: number) => n.toString().padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

const SchedulerPage: React.FC = () => {
  const { t } = useTranslation();
  const [tasks, setTasks] = useState<ScheduledTask[]>([]);
  const [tab, setTab] = useState<Tab>('active');
  const [loading, setLoading] = useState(true);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  const [showForm, setShowForm] = useState(false);

  // 表单字段
  const [formMessage, setFormMessage] = useState('');
  const [formTime, setFormTime] = useState('');
  const [formRepeat, setFormRepeat] = useState('');
  const [saving, setSaving] = useState(false);

  const loadTasks = useCallback(async () => {
    try {
      const resp = await invoke<{ tasks: ScheduledTask[] }>('list_scheduled_tasks');
      setTasks(resp.tasks || []);
    } catch (e) {
      console.error('加载定时任务失败:', e);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadTasks();
  }, [loadTasks]);

  // 每秒刷新 now，用于显示剩余时间
  useEffect(() => {
    const timer = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
    return () => clearInterval(timer);
  }, []);

  // 监听 scheduler:changed 自动刷新
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    void (async () => {
      try {
        unlisten = await listen('scheduler:changed', () => {
          void loadTasks();
        });
        if (cancelled) { unlisten(); return; }
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [loadTasks]);

  const filtered = useMemo(() => {
    return tasks.filter((t) => {
      if (tab === 'active') return t.status === 'pending' || t.status === 'running' || t.status === 'paused';
      if (tab === 'history') return t.status === 'completed' || t.status === 'cancelled' || t.status === 'failed';
      return true;
    });
  }, [tasks, tab]);

  const openForm = useCallback(() => {
    // 默认时间为 1 小时后
    const defaultTs = Math.floor(Date.now() / 1000) + 3600;
    setFormMessage('');
    setFormTime(timestampToLocalDateTime(defaultTs));
    setFormRepeat('');
    setShowForm(true);
  }, []);

  const handleSave = useCallback(async () => {
    const message = formMessage.trim();
    if (!message || !formTime || saving) return;
    setSaving(true);
    try {
      const ts = localDateTimeToTimestamp(formTime);
      await invoke('add_scheduled_reminder', {
        message,
        scheduledTime: ts,
        repeatInterval: formRepeat ? Number(formRepeat) : null,
      });
      setShowForm(false);
    } catch (e) {
      console.error('添加定时任务失败:', e);
    } finally {
      setSaving(false);
    }
  }, [formMessage, formTime, formRepeat, saving]);

  const handleCancel = useCallback(
    async (id: string) => {
      if (!window.confirm(t('scheduler_window.confirm_cancel'))) return;
      try {
        await invoke('cancel_scheduled_task', { id });
      } catch (e) {
        console.error('取消定时任务失败:', e);
      }
    },
    [t],
  );

  const handlePause = useCallback(async (id: string) => {
    try {
      await invoke('pause_scheduled_task', { id });
    } catch (e) {
      console.error('暂停定时任务失败:', e);
    }
  }, []);

  const handleResume = useCallback(async (id: string) => {
    try {
      await invoke('resume_scheduled_task', { id });
    } catch (e) {
      console.error('恢复定时任务失败:', e);
    }
  }, []);

  const inputStyle: React.CSSProperties = {
    width: '100%',
    padding: '8px 12px',
    background: 'var(--panel-surface)',
    border: '1.5px solid var(--panel-border)',
    borderRadius: 8,
    color: 'var(--panel-text)',
    fontSize: 14,
    fontFamily: 'inherit',
    outline: 'none',
    boxSizing: 'border-box',
    boxShadow: '0 1px 2px rgba(0,0,0,0.02)',
  };

  const statusLabel = (s: TaskStatus) => t(`scheduler_window.status_${s}` as const);

  return (
    <div
      className="vivian-scroll"
      style={{
        display: 'flex',
        flexDirection: 'column',
        height: '100%',
        overflow: 'hidden',
        color: 'var(--panel-text)',
      }}
    >
      {/* Tab 切换 */}
      <div
        style={{
          display: 'flex',
          gap: 4,
          padding: '10px 16px',
          flexShrink: 0,
          borderBottom: '1.5px solid var(--panel-border)',
          background: 'var(--panel-bar-bg)',
        }}
      >
        {(
          [
            { key: 'active', label: t('scheduler_window.status_pending') },
            { key: 'history', label: t('scheduler_window.status_completed') },
            { key: 'all', label: t('todo_window.tab_all') },
          ] as { key: Tab; label: string }[]
        ).map((tb) => (
          <button
            key={tb.key}
            onClick={() => setTab(tb.key)}
            style={{
              padding: '6px 14px',
              border: 'none',
              borderRadius: 16,
              background: tab === tb.key ? 'var(--panel-selected-bg)' : 'transparent',
              color: tab === tb.key ? 'var(--panel-selected-text)' : 'var(--panel-text-secondary)',
              fontSize: 13,
              fontWeight: 500,
              cursor: 'pointer',
              transition: 'background 0.15s ease, color 0.15s ease',
            }}
          >
            {tb.label}
          </button>
        ))}
      </div>

      {/* 列表 */}
      <div
        className="vivian-scroll"
        style={{
          flex: 1,
          overflowY: 'auto',
          padding: '12px 16px',
        }}
      >
        {loading ? (
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              gap: 8,
              color: 'var(--panel-text-tertiary)',
              fontSize: 13,
              marginTop: 40,
            }}
          >
            <LoadingSpinner size={16} color="var(--panel-text-tertiary)" thickness={1.5} />
          </div>
        ) : filtered.length === 0 ? (
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              justifyContent: 'center',
              gap: 8,
              color: 'var(--panel-text-tertiary)',
              fontSize: 14,
              marginTop: 60,
            }}
          >
            <div style={{ fontSize: 28, opacity: 0.4 }}>⏰</div>
            <div>{t('scheduler_window.empty')}</div>
            <div style={{ fontSize: 12, opacity: 0.7 }}>
              {t('scheduler_window.empty_hint')}
            </div>
          </div>
        ) : (
          filtered.map((task) => {
            const isActive = task.status === 'pending' || task.status === 'running' || task.status === 'paused';
            const isPaused = task.status === 'paused';
            const isPending = task.status === 'pending';
            return (
              <div
                key={task.id}
                style={{
                  background: 'var(--panel-surface)',
                  borderRadius: 12,
                  padding: '12px 14px',
                  marginBottom: 10,
                  border: '1.5px solid var(--panel-border)',
                  boxShadow: '0 1px 3px rgba(0,0,0,0.04)',
                }}
              >
                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 8,
                    marginBottom: 6,
                    flexWrap: 'wrap',
                  }}
                >
                  <span
                    style={{
                      fontSize: 11,
                      padding: '2px 8px',
                      borderRadius: 8,
                      background: `${STATUS_COLORS[task.status]}22`,
                      color: STATUS_COLORS[task.status],
                      fontWeight: 500,
                    }}
                  >
                    {statusLabel(task.status)}
                  </span>
                  {task.task_type === 'reminder' && (
                    <span style={{ fontSize: 11, color: '#4CAF50' }}>🔔</span>
                  )}
                  {task.task_type === 'tool_call' && (
                    <span style={{ fontSize: 11, color: '#2196F3' }}>🛠</span>
                  )}
                  {task.repeat_interval && (
                    <span style={{ fontSize: 11, color: '#FF9800' }}>
                      ↻ {task.repeat_interval}s
                    </span>
                  )}
                </div>

                <div
                  style={{
                    fontSize: 15,
                    fontWeight: 500,
                    wordBreak: 'break-word',
                    opacity: isActive ? 1 : 0.6,
                    color: 'var(--panel-text)',
                  }}
                >
                  {task.message || task.tool_name || task.id}
                </div>

                <div
                  style={{
                    fontSize: 12,
                    color: 'var(--panel-text-secondary)',
                    marginTop: 6,
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 2,
                  }}
                >
                  <div>⏰ {formatDateTime(task.scheduled_time)}</div>
                  {isActive && (
                    <div style={{ color: STATUS_COLORS[task.status] }}>
                      {t('scheduler_window.remaining', {
                        time: formatRemaining(task.scheduled_time, now),
                      })}
                    </div>
                  )}
                </div>

                {isActive && (
                  <div
                    style={{
                      display: 'flex',
                      gap: 6,
                      marginTop: 10,
                      justifyContent: 'flex-end',
                    }}
                  >
                    {isPending && (
                      <button
                        onClick={() => handlePause(task.id)}
                        style={actionBtn}
                        onMouseEnter={(e) =>
                          (e.currentTarget.style.background = 'rgba(255,152,0,0.10)')
                        }
                        onMouseLeave={(e) =>
                          (e.currentTarget.style.background = 'transparent')
                        }
                      >
                        {t('scheduler_window.btn_pause', '暂停')}
                      </button>
                    )}
                    {isPaused && (
                      <button
                        onClick={() => handleResume(task.id)}
                        style={actionBtn}
                        onMouseEnter={(e) =>
                          (e.currentTarget.style.background = 'rgba(76,175,80,0.10)')
                        }
                        onMouseLeave={(e) =>
                          (e.currentTarget.style.background = 'transparent')
                        }
                      >
                        {t('scheduler_window.btn_resume', '恢复')}
                      </button>
                    )}
                    <button
                      onClick={() => handleCancel(task.id)}
                      style={actionBtn}
                      onMouseEnter={(e) =>
                        (e.currentTarget.style.background = 'rgba(229,57,53,0.10)')
                      }
                      onMouseLeave={(e) =>
                        (e.currentTarget.style.background = 'transparent')
                      }
                    >
                      {t('scheduler_window.btn_cancel')}
                    </button>
                  </div>
                )}
              </div>
            );
          })
        )}
      </div>

      {/* 底部添加按钮 */}
      <div
        style={{
          padding: '10px 16px calc(10px + env(safe-area-inset-bottom, 0px))',
          flexShrink: 0,
          background: 'var(--panel-bar-bg)',
          backdropFilter: 'blur(12px)',
          WebkitBackdropFilter: 'blur(12px)',
          borderTop: '1.5px solid var(--panel-border)',
        }}
      >
        <button
          onClick={openForm}
          style={{
            width: '100%',
            padding: '10px',
            border: 'none',
            borderRadius: 12,
            background: 'var(--panel-accent)',
            color: 'var(--panel-selected-text)',
            fontSize: 14,
            fontWeight: 600,
            cursor: 'pointer',
            boxShadow: '0 1px 3px rgba(0,0,0,0.08)',
          }}
        >
          + {t('scheduler_window.btn_add')}
        </button>
      </div>

      {/* 表单弹窗 */}
      {showForm && (
        <div
          style={{
            position: 'fixed',
            inset: 0,
            background: 'var(--panel-overlay)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            zIndex: 100,
          }}
          onClick={() => setShowForm(false)}
        >
          <div
            style={{
              background: 'var(--panel-surface)',
              borderRadius: 16,
              padding: 20,
              width: '80%',
              maxWidth: 400,
              border: '1.5px solid var(--panel-border)',
              boxShadow: '0 8px 32px rgba(0,0,0,0.12)',
            }}
            onClick={(e) => e.stopPropagation()}
          >
            <div style={{ fontSize: 16, fontWeight: 700, marginBottom: 16, color: 'var(--panel-text)' }}>
              {t('scheduler_window.btn_add')}
            </div>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
              <div>
                <label style={labelStyle}>
                  {t('scheduler_window.field_message')}
                </label>
                <textarea
                  value={formMessage}
                  onChange={(e) => setFormMessage(e.target.value)}
                  style={{ ...inputStyle, minHeight: 60, resize: 'vertical' }}
                  rows={3}
                  autoFocus
                />
              </div>
              <div>
                <label style={labelStyle}>
                  {t('scheduler_window.field_time')}
                </label>
                <input
                  type="datetime-local"
                  value={formTime}
                  onChange={(e) => setFormTime(e.target.value)}
                  style={inputStyle}
                />
              </div>
              <div>
                <label style={labelStyle}>
                  {t('scheduler_window.field_repeat')}
                </label>
                <input
                  type="number"
                  value={formRepeat}
                  onChange={(e) => setFormRepeat(e.target.value)}
                  style={inputStyle}
                  min={1}
                  placeholder="0"
                />
              </div>
            </div>
            <div
              style={{
                display: 'flex',
                gap: 8,
                marginTop: 20,
                justifyContent: 'flex-end',
              }}
            >
              <button
                onClick={() => setShowForm(false)}
                style={{
                  padding: '8px 16px',
                  border: 'none',
                  borderRadius: 10,
                  background: 'var(--panel-tag-bg)',
                  color: 'var(--panel-text)',
                  fontSize: 14,
                  cursor: 'pointer',
                }}
              >
                {t('todo_window.btn_cancel')}
              </button>
              <button
                onClick={handleSave}
                disabled={!formMessage.trim() || !formTime || saving}
                style={{
                  padding: '8px 16px',
                  border: 'none',
                  borderRadius: 10,
                  background:
                    formMessage.trim() && formTime && !saving ? 'var(--panel-accent)' : 'var(--panel-toggle-off)',
                  color: 'var(--panel-selected-text)',
                  fontSize: 14,
                  fontWeight: 600,
                  cursor:
                    formMessage.trim() && formTime && !saving ? 'pointer' : 'not-allowed',
                }}
              >
                {t('todo_window.btn_save')}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};

const actionBtn: React.CSSProperties = {
  padding: '4px 12px',
  border: 'none',
  borderRadius: 8,
  background: 'transparent',
  color: 'var(--panel-text-secondary)',
  fontSize: 12,
  fontWeight: 500,
  cursor: 'pointer',
  transition: 'background 0.15s ease',
};

const labelStyle: React.CSSProperties = {
  display: 'block',
  fontSize: 12,
  color: 'var(--panel-text-secondary)',
  marginBottom: 4,
};

export default SchedulerPage;
