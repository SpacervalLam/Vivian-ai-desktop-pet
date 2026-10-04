/**
 * Scheduler 页 — 定时任务列表 + 表单
 *
 * 数据源：invoke('list_scheduled_tasks') / invoke('add_scheduled_reminder') / ...
 * 刷新：监听 scheduler:changed 事件
 *
 * 布局（看板型）：与待办共用一套任务行骨架 —— 顶部筛选行 + 行列表。
 * 两页共用 .record-plan-* 形态，差异只在注入的 --rec-accent-bar（状态色）
 * 与元信息字段，因此「待办」和「日程」在视觉上是一套语言。
 */

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import {
  AlarmClock,
  Bell,
  BellRing,
  Clock,
  Pause,
  Play,
  Plus,
  Repeat,
  Wrench,
  X,
} from 'lucide-react';
import LoadingSpinner from '../../LoadingSpinner';
import './RecordTheme.css';

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
  metadata?: { recovery_error?: string };
  delivery?: { confirmed_count: number; next_attempt_at?: number | null; last_delivered_at?: number | null };
}

type Tab = 'active' | 'history' | 'all';

const STATUS_COLORS: Record<TaskStatus, string> = {
  pending: '#C08A3E',
  running: '#788C5D',
  completed: '#8A8780',
  cancelled: '#8A8780',
  failed: '#B4553F',
  paused: '#B8823C',
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
  const { t, i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh');
  const ja = i18n.language.startsWith('ja');
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

  const counts = useMemo(
    () => ({
      active: tasks.filter(
        (t) => t.status === 'pending' || t.status === 'running' || t.status === 'paused',
      ).length,
      history: tasks.filter(
        (t) => t.status === 'completed' || t.status === 'cancelled' || t.status === 'failed',
      ).length,
      all: tasks.length,
    }),
    [tasks],
  );

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

  const statusLabel = (s: TaskStatus) => t(`scheduler_window.status_${s}` as const);

  const TABS: Array<{ key: Tab; label: string }> = [
    { key: 'active', label: t('scheduler_window.status_pending') },
    { key: 'history', label: t('scheduler_window.status_completed') },
    { key: 'all', label: t('todo_window.tab_all') },
  ];

  return (
    <div className="record-plan">
      <div className="record-plan-bar">
        <div className="rec-seg">
          {TABS.map((tb) => (
            <button
              key={tb.key}
              type="button"
              onClick={() => setTab(tb.key)}
              className={`rec-seg-item${tab === tb.key ? ' is-active' : ''}`}
            >
              {tb.label}
              <span style={{ color: 'var(--rec-faint)', fontVariantNumeric: 'tabular-nums' }}>
                {counts[tb.key]}
              </span>
            </button>
          ))}
        </div>
        <span className="record-plan-bar-spacer" />
        <button type="button" onClick={openForm} className="rec-btn is-primary">
          <Plus size={14} /> {t('scheduler_window.btn_add')}
        </button>
      </div>

      <div className="record-plan-list">
        {loading ? (
          <div style={{ display: 'flex', justifyContent: 'center', paddingTop: 40 }}>
            <LoadingSpinner size={16} color="var(--rec-faint)" thickness={1.5} />
          </div>
        ) : filtered.length === 0 ? (
          <div className="rec-blank">
            <span className="rec-blank-icon">
              <AlarmClock size={30} strokeWidth={1.2} />
            </span>
            <span className="rec-blank-title">{t('scheduler_window.empty')}</span>
            <span className="rec-blank-hint">{t('scheduler_window.empty_hint')}</span>
          </div>
        ) : (
          filtered.map((task) => {
            const isActive = task.status === 'pending' || task.status === 'running' || task.status === 'paused';
            const isPaused = task.status === 'paused';
            const isPending = task.status === 'pending';
            const statusColor = STATUS_COLORS[task.status];
            return (
              <div
                key={task.id}
                className={`record-plan-row${isActive ? '' : ' is-done'}`}
                style={{ ['--rec-accent-bar' as string]: statusColor }}
              >
                <div className="record-plan-row-top">
                  <div className="record-plan-row-main">
                    <span className="record-plan-title">
                      {task.message || task.tool_name || task.id}
                    </span>
                    {task.metadata?.recovery_error && <span role="status" style={{ color: 'var(--rec-accent)', fontSize: 12 }}>
                      {i18n.language.startsWith('en') ? 'Execution was interrupted. Check the result before retrying.'
                        : i18n.language.startsWith('ja') ? '実行が中断されました。再試行の前に結果を確認してください。'
                        : '执行曾中断，结果未确认；重试前请先检查实际结果。'}
                    </span>}
                  </div>
                  <div className="record-plan-actions">
                    {isPending && (
                      <button
                        type="button"
                        onClick={() => void handlePause(task.id)}
                        title={t('scheduler_window.btn_pause', '暂停')}
                        className="rec-icon-btn"
                      >
                        <Pause size={14} />
                      </button>
                    )}
                    {isPaused && (
                      <button
                        type="button"
                        onClick={() => void handleResume(task.id)}
                        title={t('scheduler_window.btn_resume', '恢复')}
                        className="rec-icon-btn"
                      >
                        <Play size={14} />
                      </button>
                    )}
                    {isActive && (
                      <button
                        type="button"
                        onClick={() => void handleCancel(task.id)}
                        title={t('scheduler_window.btn_cancel')}
                        className="rec-icon-btn is-danger"
                      >
                        <X size={14} />
                      </button>
                    )}
                  </div>
                </div>

                <div className="record-plan-foot">
                  <span className="record-plan-badge">
                    <span className="rec-dot" style={{ color: statusColor }} />
                    {statusLabel(task.status)}
                  </span>
                  {task.task_type === 'reminder' && (
                    <span className="record-plan-badge">
                      <Bell size={12} />
                    </span>
                  )}
                  {task.task_type === 'tool_call' && (
                    <span className="record-plan-badge">
                      <Wrench size={12} />
                    </span>
                  )}
                  <span className="record-plan-badge">
                    <Clock size={12} />
                    {formatDateTime(task.scheduled_time)}
                  </span>
                  {task.repeat_interval && (
                    <span className="record-plan-badge">
                      <Repeat size={12} />
                      {task.repeat_interval}s
                    </span>
                  )}
                  {isActive && (
                    <span className="record-plan-badge" style={{ color: statusColor }}>
                      <BellRing size={12} />
                      {t('scheduler_window.remaining', {
                        time: formatRemaining(task.scheduled_time, now),
                      })}
                    </span>
                  )}
                  <span className="record-plan-foot-spacer" />
                  {task.task_type === 'reminder' && task.delivery && task.delivery.confirmed_count > 0 && (
                    <span className="record-plan-badge">
                      {zh ? '已确认投递' : ja ? '配信確認済み' : 'Confirmed deliveries'}：
                      {task.delivery.confirmed_count}
                      {task.delivery.next_attempt_at && (
                        <>
                          {' · '}
                          {zh ? '下次重试' : ja ? '再試行予定' : 'Retry at'}{' '}
                          {formatDateTime(task.delivery.next_attempt_at)}
                        </>
                      )}
                    </span>
                  )}
                </div>
              </div>
            );
          })
        )}
      </div>

      {/* 表单弹窗 */}
      {showForm && (
        <div className="rec-overlay" onClick={() => setShowForm(false)}>
          <div className="rec-dialog" onClick={(e) => e.stopPropagation()}>
            <h3 className="rec-dialog-title">{t('scheduler_window.btn_add')}</h3>
            <div className="rec-dialog-body">
              <div className="rec-field">
                <label className="rec-label">{t('scheduler_window.field_message')}</label>
                <textarea
                  className="rec-textarea"
                  style={{ minHeight: 60 }}
                  value={formMessage}
                  onChange={(e) => setFormMessage(e.target.value)}
                  rows={3}
                  autoFocus
                />
              </div>
              <div className="rec-field">
                <label className="rec-label">{t('scheduler_window.field_time')}</label>
                <input
                  className="rec-input"
                  type="datetime-local"
                  value={formTime}
                  onChange={(e) => setFormTime(e.target.value)}
                />
              </div>
              <div className="rec-field">
                <label className="rec-label">{t('scheduler_window.field_repeat')}</label>
                <input
                  className="rec-input"
                  type="number"
                  value={formRepeat}
                  onChange={(e) => setFormRepeat(e.target.value)}
                  min={1}
                  placeholder="0"
                />
              </div>
            </div>
            <div className="rec-dialog-foot">
              <button type="button" onClick={() => setShowForm(false)} className="rec-btn">
                {t('todo_window.btn_cancel')}
              </button>
              <button
                type="button"
                onClick={() => void handleSave()}
                disabled={!formMessage.trim() || !formTime || saving}
                className="rec-btn is-primary"
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

export default SchedulerPage;
