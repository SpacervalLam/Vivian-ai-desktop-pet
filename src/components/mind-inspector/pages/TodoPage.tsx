/**
 * Todo 页 — 待办事件列表 + 表单
 *
 * 数据源：invoke('list_todos') / invoke('add_todo_item') / ...
 * 刷新：监听 todo:changed 事件
 *
 * 从 TodoWindow.tsx 改造：去除窗口外壳（标题栏/minimize/close/getCurrentWindow），
 * 适配 MindInspector 的 page 渲染模式。
 */

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import LoadingSpinner from '../../LoadingSpinner';

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

const PRIORITY_COLORS: Record<number, string> = {
  1: '#8E8E93',
  2: '#FF9500',
  3: '#FF453A',
};

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
            { key: 'pending', label: t('todo_window.tab_pending') },
            { key: 'completed', label: t('todo_window.tab_completed') },
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
            <div style={{ fontSize: 28, opacity: 0.4 }}>✓</div>
            <div>{t('todo_window.empty')}</div>
            <div style={{ fontSize: 12, opacity: 0.7 }}>
              {t('todo_window.empty_hint')}
            </div>
          </div>
        ) : (
          filtered.map((it) => (
            <div
              key={it.id}
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
                  alignItems: 'flex-start',
                  justifyContent: 'space-between',
                  gap: 8,
                }}
              >
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 8,
                      marginBottom: 4,
                    }}
                  >
                    <span
                      style={{
                        fontSize: 11,
                        padding: '2px 8px',
                        borderRadius: 8,
                        background: PRIORITY_COLORS[it.priority]
                          ? `${PRIORITY_COLORS[it.priority]}22`
                          : 'var(--panel-tag-bg)',
                        color: PRIORITY_COLORS[it.priority] || 'var(--panel-text-tertiary)',
                        fontWeight: 500,
                      }}
                    >
                      {t(`todo_window.priority_${it.priority}` as const)}
                    </span>
                    {it.due_date && (
                      <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)' }}>
                        📅 {formatDueDate(it.due_date)}
                      </span>
                    )}
                    {it.reminder_id && (
                      <span style={{ fontSize: 11, color: '#4CAF50' }}>🔔</span>
                    )}
                  </div>
                  <div
                    style={{
                      fontSize: 15,
                      fontWeight: 500,
                      textDecoration: it.completed ? 'line-through' : 'none',
                      opacity: it.completed ? 0.6 : 1,
                      wordBreak: 'break-word',
                      color: 'var(--panel-text)',
                    }}
                  >
                    {it.title}
                  </div>
                  {it.description && (
                    <div
                      style={{
                        fontSize: 13,
                        color: 'var(--panel-text-secondary)',
                        marginTop: 4,
                        whiteSpace: 'pre-wrap',
                        wordBreak: 'break-word',
                      }}
                    >
                      {it.description}
                    </div>
                  )}
                </div>
              </div>
              <div
                style={{
                  display: 'flex',
                  gap: 6,
                  marginTop: 10,
                  justifyContent: 'flex-end',
                }}
              >
                {!it.completed && (
                  <button
                    onClick={() => handleComplete(it.id)}
                    style={actionBtn}
                    onMouseEnter={(e) =>
                      (e.currentTarget.style.background = 'rgba(76,175,80,0.10)')
                    }
                    onMouseLeave={(e) =>
                      (e.currentTarget.style.background = 'transparent')
                    }
                  >
                    {t('todo_window.btn_complete')}
                  </button>
                )}
                {!it.completed && (
                  <button
                    onClick={() => openForm(it)}
                    style={actionBtn}
                    onMouseEnter={(e) =>
                      (e.currentTarget.style.background = 'var(--panel-bg-hover)')
                    }
                    onMouseLeave={(e) =>
                      (e.currentTarget.style.background = 'transparent')
                    }
                  >
                    {t('todo_window.btn_edit')}
                  </button>
                )}
                <button
                  onClick={() => handleDelete(it.id)}
                  style={actionBtn}
                  onMouseEnter={(e) =>
                    (e.currentTarget.style.background = 'rgba(229,57,53,0.10)')
                  }
                  onMouseLeave={(e) =>
                    (e.currentTarget.style.background = 'transparent')
                  }
                >
                  {t('todo_window.btn_delete')}
                </button>
              </div>
            </div>
          ))
        )}
      </div>

      {/* 底部添加按钮 */}
      <div
        style={{
          padding: '10px 16px calc(10px + env(safe-area-inset-bottom, 0px))',
          flexShrink: 0,
          background: 'var(--panel-bar-bg)',
          borderTop: '1.5px solid var(--panel-border)',
        }}
      >
        <button
          onClick={() => openForm()}
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
          + {t('todo_window.btn_add')}
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
              {editing ? t('todo_window.btn_edit') : t('todo_window.btn_add')}
            </div>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
              <div>
                <label style={labelStyle}>{t('todo_window.field_title')}</label>
                <input
                  value={formTitle}
                  onChange={(e) => setFormTitle(e.target.value)}
                  style={inputStyle}
                  autoFocus
                />
              </div>
              <div>
                <label style={labelStyle}>
                  {t('todo_window.field_description')}
                </label>
                <textarea
                  value={formDescription}
                  onChange={(e) => setFormDescription(e.target.value)}
                  style={{ ...inputStyle, minHeight: 60, resize: 'vertical' }}
                  rows={3}
                />
              </div>
              <div>
                <label style={labelStyle}>
                  {t('todo_window.field_priority')}
                </label>
                <div style={{ display: 'flex', gap: 6 }}>
                  {[1, 2, 3].map((p) => (
                    <button
                      key={p}
                      onClick={() => setFormPriority(p)}
                      style={{
                        flex: 1,
                        padding: '8px',
                        border: 'none',
                        borderRadius: 8,
                        background:
                          formPriority === p
                            ? PRIORITY_COLORS[p]
                            : 'var(--panel-tag-bg)',
                        color: formPriority === p ? 'var(--panel-selected-text)' : 'var(--panel-text-secondary)',
                        fontSize: 13,
                        fontWeight: 500,
                        cursor: 'pointer',
                      }}
                    >
                      {t(`todo_window.priority_${p}` as const)}
                    </button>
                  ))}
                </div>
              </div>
              <div>
                <label style={labelStyle}>
                  {t('todo_window.field_due_date')}
                </label>
                <input
                  type="datetime-local"
                  value={formDueDate}
                  onChange={(e) => setFormDueDate(e.target.value)}
                  style={inputStyle}
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
                disabled={!formTitle.trim() || saving}
                style={{
                  padding: '8px 16px',
                  border: 'none',
                  borderRadius: 10,
                  background:
                    formTitle.trim() && !saving ? 'var(--panel-accent)' : 'var(--panel-toggle-off)',
                  color: 'var(--panel-selected-text)',
                  fontSize: 14,
                  fontWeight: 600,
                  cursor: formTitle.trim() && !saving ? 'pointer' : 'not-allowed',
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

export default TodoPage;
