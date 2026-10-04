/**
 * Todo 页 — 待办事件列表 + 表单
 *
 * 数据源：invoke('list_todos') / invoke('add_todo_item') / ...
 * 刷新：监听 todo:changed 事件
 *
 * 布局（看板型）：顶部筛选行（状态分段 + 计数 + 新增）/ 下方任务行列表。
 * 任务行不是卡片：左侧一条 2px 优先级竖条承担全部色彩编码，
 * 行间用发丝线分隔 —— 看板要的是扫视速度，不是卡片阴影。
 * 版式刻度与共用控件走 RecordTheme.css。
 */

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import {
  Bell,
  Check,
  CheckCheck,
  ListChecks,
  Pencil,
  Plus,
  Trash2,
} from 'lucide-react';
import LoadingSpinner from '../../LoadingSpinner';
import './RecordTheme.css';

interface TodoItem {
  id: string;
  title: string;
  description: string;
  completed: boolean;
  priority: number;
  created_at: number;
  completed_at?: number | null;
  due_date?: string | null;
  reminder_id?: string | null;
}

type Tab = 'pending' | 'completed' | 'all';

/** 优先级色阶：只驱动左侧竖条与状态点，不做色块底 */
const PRIORITY_COLORS: Record<number, string> = {
  1: '#8A8780',
  2: '#C08A3E',
  3: '#B4553F',
};

// 将 due_date 转换为 datetime-local input 所需的格式（YYYY-MM-DDTHH:MM）
function dueDateToInputValue(dueDate: string | null | undefined): string {
  if (!dueDate) return '';
  if (/^\d{4}-\d{2}-\d{2}$/.test(dueDate)) {
    return `${dueDate}T09:00`;
  }
  const match = dueDate.match(/^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})/);
  if (match) {
    return `${match[1]}T${match[2]}`;
  }
  return '';
}

// 格式化 due_date 用于列表显示
function formatDueDate(dueDate: string | null | undefined): string {
  if (!dueDate) return '';
  if (/^\d{4}-\d{2}-\d{2}$/.test(dueDate)) {
    return dueDate;
  }
  const match = dueDate.match(/^(\d{4}-\d{2}-\d{2})T(\d{2}):(\d{2})/);
  if (match) {
    return `${match[1]} ${match[2]}:${match[3]}`;
  }
  return dueDate;
}

const TodoPage: React.FC = () => {
  const { t } = useTranslation();
  const [items, setItems] = useState<TodoItem[]>([]);
  const [tab, setTab] = useState<Tab>('pending');
  const [loading, setLoading] = useState(true);
  const [showForm, setShowForm] = useState(false);
  const [editing, setEditing] = useState<TodoItem | null>(null);

  // 表单字段
  const [formTitle, setFormTitle] = useState('');
  const [formDescription, setFormDescription] = useState('');
  const [formPriority, setFormPriority] = useState(1);
  const [formDueDate, setFormDueDate] = useState('');
  const [saving, setSaving] = useState(false);

  const loadTodos = useCallback(async () => {
    try {
      const resp = await invoke<{ items: TodoItem[] }>('list_todos', {
        includeCompleted: true,
      });
      setItems(resp.items || []);
    } catch (e) {
      console.error('加载待办失败:', e);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadTodos();
  }, [loadTodos]);

  // 监听 todo:changed 自动刷新
  useEffect(() => {
    let cancelled = false;
    let unlisten: UnlistenFn | undefined;
    void (async () => {
      try {
        unlisten = await listen('todo:changed', () => {
          void loadTodos();
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
  }, [loadTodos]);

  const filtered = useMemo(() => {
    return items.filter((it) => {
      if (tab === 'pending') return !it.completed;
      if (tab === 'completed') return it.completed;
      return true;
    });
  }, [items, tab]);

  // 各分段的条目数，供筛选行上的计数使用
  const counts = useMemo(
    () => ({
      pending: items.filter((it) => !it.completed).length,
      completed: items.filter((it) => it.completed).length,
      all: items.length,
    }),
    [items],
  );

  const openForm = useCallback((item?: TodoItem) => {
    if (item) {
      setEditing(item);
      setFormTitle(item.title);
      setFormDescription(item.description);
      setFormPriority(item.priority);
      setFormDueDate(dueDateToInputValue(item.due_date));
    } else {
      setEditing(null);
      setFormTitle('');
      setFormDescription('');
      setFormPriority(1);
      setFormDueDate('');
    }
    setShowForm(true);
  }, []);

  const handleSave = useCallback(async () => {
    const title = formTitle.trim();
    if (!title || saving) return;
    setSaving(true);
    try {
      if (editing) {
        await invoke('update_todo_item', {
          id: editing.id,
          title,
          description: formDescription,
          priority: formPriority,
          dueDate: formDueDate || null,
        });
      } else {
        await invoke('add_todo_item', {
          title,
          description: formDescription,
          priority: formPriority,
          dueDate: formDueDate || null,
        });
      }
      setShowForm(false);
    } catch (e) {
      console.error('保存待办失败:', e);
    } finally {
      setSaving(false);
    }
  }, [editing, formTitle, formDescription, formPriority, formDueDate, saving]);

  const handleComplete = useCallback(async (id: string) => {
    try {
      await invoke('complete_todo_item', { id });
    } catch (e) {
      console.error('完成待办失败:', e);
    }
  }, []);

  const handleDelete = useCallback(
    async (id: string) => {
      if (!window.confirm(t('todo_window.confirm_delete'))) return;
      try {
        await invoke('delete_todo_item', { id });
      } catch (e) {
        console.error('删除待办失败:', e);
      }
    },
    [t],
  );

  const TABS: Array<{ key: Tab; label: string }> = [
    { key: 'pending', label: t('todo_window.tab_pending') },
    { key: 'completed', label: t('todo_window.tab_completed') },
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
        <button type="button" onClick={() => openForm()} className="rec-btn is-primary">
          <Plus size={14} /> {t('todo_window.btn_add')}
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
              <CheckCheck size={30} strokeWidth={1.2} />
            </span>
            <span className="rec-blank-title">{t('todo_window.empty')}</span>
            <span className="rec-blank-hint">{t('todo_window.empty_hint')}</span>
          </div>
        ) : (
          filtered.map((it) => (
            <div
              key={it.id}
              className={`record-plan-row${it.completed ? ' is-done' : ''}`}
              style={{ ['--rec-accent-bar' as string]: PRIORITY_COLORS[it.priority] ?? 'var(--rec-faint)' }}
            >
              <div className="record-plan-row-top">
                <div className="record-plan-row-main">
                  <span className="record-plan-title">{it.title}</span>
                  {it.description && <span className="record-plan-desc">{it.description}</span>}
                </div>
                <div className="record-plan-actions">
                  {!it.completed && (
                    <button
                      type="button"
                      onClick={() => void handleComplete(it.id)}
                      title={t('todo_window.btn_complete')}
                      className="rec-icon-btn"
                    >
                      <Check size={15} />
                    </button>
                  )}
                  <button
                    type="button"
                    onClick={() => openForm(it)}
                    title={t('todo_window.btn_edit')}
                    className="rec-icon-btn"
                  >
                    <Pencil size={14} />
                  </button>
                  <button
                    type="button"
                    onClick={() => void handleDelete(it.id)}
                    title={t('todo_window.btn_delete')}
                    className="rec-icon-btn is-danger"
                  >
                    <Trash2 size={14} />
                  </button>
                </div>
              </div>

              {(it.priority > 1 || it.due_date || it.reminder_id) && (
                <div className="record-plan-foot">
                  {it.priority > 1 && (
                    <span className="record-plan-badge">
                      <span className="rec-dot" style={{ color: PRIORITY_COLORS[it.priority] }} />
                      {t(`todo_window.priority_${it.priority}` as const)}
                    </span>
                  )}
                  {it.due_date && (
                    <span className="record-plan-badge">
                      <ListChecks size={12} />
                      {formatDueDate(it.due_date)}
                    </span>
                  )}
                  {it.reminder_id && (
                    <span className="record-plan-badge">
                      <Bell size={12} />
                    </span>
                  )}
                </div>
              )}
            </div>
          ))
        )}
      </div>

      {/* 表单弹窗 */}
      {showForm && (
        <div className="rec-overlay" onClick={() => setShowForm(false)}>
          <div className="rec-dialog" onClick={(e) => e.stopPropagation()}>
            <h3 className="rec-dialog-title">
              {editing ? t('todo_window.btn_edit') : t('todo_window.btn_add')}
            </h3>
            <div className="rec-dialog-body">
              <div className="rec-field">
                <label className="rec-label">{t('todo_window.field_title')}</label>
                <input
                  className="rec-input"
                  value={formTitle}
                  onChange={(e) => setFormTitle(e.target.value)}
                  autoFocus
                />
              </div>
              <div className="rec-field">
                <label className="rec-label">{t('todo_window.field_description')}</label>
                <textarea
                  className="rec-textarea"
                  style={{ minHeight: 60 }}
                  value={formDescription}
                  onChange={(e) => setFormDescription(e.target.value)}
                  rows={3}
                />
              </div>
              <div className="rec-field">
                <label className="rec-label">{t('todo_window.field_priority')}</label>
                <div className="rec-seg" style={{ alignSelf: 'stretch' }}>
                  {[1, 2, 3].map((p) => (
                    <button
                      key={p}
                      type="button"
                      onClick={() => setFormPriority(p)}
                      className={`rec-seg-item${formPriority === p ? ' is-active' : ''}`}
                      style={{ flex: 1, justifyContent: 'center' }}
                    >
                      {formPriority === p && (
                        <span
                          className="rec-dot"
                          style={{ color: PRIORITY_COLORS[p] }}
                        />
                      )}
                      {t(`todo_window.priority_${p}` as const)}
                    </button>
                  ))}
                </div>
              </div>
              <div className="rec-field">
                <label className="rec-label">{t('todo_window.field_due_date')}</label>
                <input
                  className="rec-input"
                  type="datetime-local"
                  value={formDueDate}
                  onChange={(e) => setFormDueDate(e.target.value)}
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
                disabled={!formTitle.trim() || saving}
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

export default TodoPage;
