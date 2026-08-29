/**
 * Notebook 页 — 卡片风格 HTML 笔记列表 + 预览 + 编辑
 *
 * 数据源：invoke('list_notebooks') / invoke('get_notebook_html') / invoke('get_notebook_detail')
 * 写入：invoke('create_notebook') / invoke('update_notebook')
 * 刷新：监听 notebook:created / notebook:updated / notebook:deleted 事件
 *
 * 布局：左右两栏（左 36% 笔记列表 + 右 64% 预览/编辑）
 * 顶部角色切换（vivian / nana）
 * 支持 pageParams.notebookId 直接定位笔记
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { listen, emit, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { Trash2, FileText, Plus, ChevronUp, ChevronDown, Pencil, X, Check, NotebookPen, MousePointerClick, ListChecks, Upload } from 'lucide-react';
import { COLORS, TYPO, SPACING, RADIUS, EASE, DURATION, SHADOW } from '../design-system';
import { EmptyState } from '../shared-components';
import { useNavigation } from '../NavigationContext';
import {
  Block,
  BlockStyle,
  Cover,
  NoteBook,
  CharacterId,
  BlockType,
} from './notebook-types';
import { WysiwygEditor } from './NoteWysiwyg';

// ============================================================
// 手账风格 CSS 关键帧
// ============================================================
const NOTEBOOK_STYLE_ID = 'notebook-page-journal-styles';
if (typeof document !== 'undefined' && !document.getElementById(NOTEBOOK_STYLE_ID)) {
  const style = document.createElement('style');
  style.id = NOTEBOOK_STYLE_ID;
  style.textContent = `
@keyframes journal-float {
  0%, 100% { transform: translateY(0) rotate(var(--float-rotate, 0deg)); }
  50% { transform: translateY(-3px) rotate(var(--float-rotate, 0deg)); }
}
@keyframes journal-shimmer {
  0% { background-position: -200% center; }
  100% { background-position: 200% center; }
}
@keyframes tape-peel {
  0% { clip-path: inset(0 100% 0 0); }
  100% { clip-path: inset(0 0 0 0); }
}
@keyframes card-enter {
  0% { opacity: 0; transform: translateY(8px) scale(0.97); }
  100% { opacity: 1; transform: translateY(0) scale(1); }
}
@keyframes ink-draw {
  0% { width: 0; }
  100% { width: 100%; }
}
@keyframes nb-fade-up {
  0% { opacity: 0; transform: translateY(12px); }
  100% { opacity: 1; transform: translateY(0); }
}
/* 输入/选择控件聚焦高亮（覆盖内联样式，仅作用于本页面） */
.notebook-page input:focus,
.notebook-page select:focus,
.notebook-page textarea:focus {
  border-color: var(--panel-accent) !important;
  outline: none;
  background: var(--panel-bg) !important;
  box-shadow: 0 0 0 3px var(--panel-accent-muted) !important;
}
/* 尊重系统减弱动效偏好：关闭入场/上浮动画 */
@media (prefers-reduced-motion: reduce) {
  .notebook-page .nb-fade,
  .notebook-page .nb-note-card {
    animation: none !important;
  }
}
`;
  document.head.appendChild(style);
}

// ============================================================
// 类型定义
// ============================================================

interface NoteSummary {
  id: string;
  title: string;
  char_id: string;
  created_at: number;
  updated_at: number;
  tags: string[];
  palette: string;
  layout: string;
  block_count: number;
  /** 渲染类型："structured"=结构化内容块渲染，"raw_html"=LLM 直接撰写完整 HTML */
  render_type?: string;
}

// ============================================================
// 常量映射
// ============================================================

const PALETTE_COLORS: Record<string, string> = {
  warm: 'linear-gradient(135deg, #FF6B6B 0%, #FFA07A 100%)',
  fresh: 'linear-gradient(135deg, #4ECDC4 0%, #45B7D1 100%)',
  elegant: 'linear-gradient(135deg, #9B59B6 0%, #6C5CE7 100%)',
  cute: 'linear-gradient(135deg, #FF8FB1 0%, #FFC75F 100%)',
  cool: 'linear-gradient(135deg, #5B8DEF 0%, #6C5CE7 100%)',
  nature: 'linear-gradient(135deg, #6B9E3F 0%, #C19A6B 100%)',
};

const PALETTE_KEYS = Object.keys(PALETTE_COLORS);

const LAYOUT_OPTIONS: { value: string; labelKey: string }[] = [
  { value: 'cover_flow', labelKey: 'cover_flow' },
  { value: 'article', labelKey: 'article' },
  { value: 'gallery', labelKey: 'gallery' },
  { value: 'simple', labelKey: 'simple' },
];

const LAYOUT_LABELS: Record<string, Record<string, string>> = {
  'zh-CN': { cover_flow: '封面卡片', article: '文章流', gallery: '图文混排', simple: '简洁卡片' },
  en: { cover_flow: 'Cover Flow', article: 'Article', gallery: 'Gallery', simple: 'Simple' },
  ja: { cover_flow: 'カバー', article: '記事', gallery: 'ギャラリー', simple: 'シンプル' },
};

const BLOCK_TYPES: BlockType[] = [
  'heading',
  'paragraph',
  'card',
  'quote',
  'list',
  'tags',
  'image',
  'divider',
  'callout',
  'table',
  'chart',
  'mermaid',
  'custom',
];

const CHAR_LABEL: Record<CharacterId, string> = {
  vivian: 'Vivian',
  nana: 'Nana',
};

// ============================================================
// 工具函数
// ============================================================

function formatTime(ts: number): string {
  const d = new Date(ts * 1000);
  const now = new Date();
  const diff = now.getTime() - d.getTime();
  const mins = Math.floor(diff / 60000);
  const hours = Math.floor(diff / 3600000);
  const days = Math.floor(diff / 86400000);

  if (mins < 1) return '刚刚';
  if (mins < 60) return `${mins}分钟前`;
  if (hours < 24) return `${hours}小时前`;
  if (days < 7) return `${days}天前`;
  return d.toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' });
}

function layoutLabel(value: string, lang: string): string {
  const table = LAYOUT_LABELS[lang] || LAYOUT_LABELS['zh-CN'];
  return table[value] || value;
}

function defaultBlock(type: BlockType): Block {
  switch (type) {
    case 'heading':
      return { type: 'heading', text: '', level: 2 };
    case 'paragraph':
      return { type: 'paragraph', text: '' };
    case 'card':
      return { type: 'card', title: '', body: '', emoji: '' };
    case 'quote':
      return { type: 'quote', text: '', author: '' };
    case 'list':
      return { type: 'list', items: [''], ordered: false };
    case 'tags':
      return { type: 'tags', items: [''] };
    case 'image':
      return { type: 'image', url: '', caption: '' };
    case 'divider':
      return { type: 'divider', emoji: '' };
    case 'callout':
      return { type: 'callout', text: '', emoji: '' };
    case 'table':
      return { type: 'table', headers: [''], rows: [['']], caption: '' };
    case 'chart':
      return {
        type: 'chart',
        chart_type: 'bar',
        title: '',
        categories: ['类别 A', '类别 B'],
        series: [{ name: '系列 1', data: [10, 20] }],
      };
    case 'mermaid':
      return {
        type: 'mermaid',
        code: 'graph TD\n  A[开始] --> B[过程]\n  B --> C[结束]',
        caption: '',
      };
    case 'custom':
      return { type: 'custom', html: '' };
  }
}

function emptyDraft(charId: CharacterId): NoteBook {
  const now = Date.now() / 1000;
  return {
    id: '',
    title: '',
    char_id: charId,
    created_at: now,
    updated_at: now,
    tags: [],
    layout: 'cover_flow',
    palette: 'warm',
    cover: null,
    blocks: [defaultBlock('paragraph')],
  };
}

// ============================================================
// NotebookPage 主组件
// ============================================================

const NotebookPage: React.FC = () => {
  const { t, i18n } = useTranslation();
  const nav = useNavigation();
  const [character, setCharacter] = useState<CharacterId>('vivian');
  const [notes, setNotes] = useState<NoteSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [html, setHtml] = useState<string>('');
  const [loading, setLoading] = useState(false);
  const [loadingHtml, setLoadingHtml] = useState(false);
  // 预览宿主：用 iframe（src = asset 协议 URL）渲染完整 HTML 文档。renderer/LLM 输出
  // 的笔记是自包含完整文档（html/body/:root 及各种复合选择器）。以 asset URL 加载使
  // 笔记文档与应用窗口跨源隔离——笔记内的脚本/按钮只能作用于笔记自身文档，无法影响
  // 整个窗口；Shadow DOM 无法承载 html/body 选择器，故不采用。
  const iframeRef = useRef<HTMLIFrameElement>(null);

  // 编辑态
  const [editing, setEditing] = useState(false);
  const [isNew, setIsNew] = useState(false);
  const [draft, setDraft] = useState<NoteBook | null>(null);
  const [dirty, setDirty] = useState(false);
  const [saving, setSaving] = useState(false);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [editError, setEditError] = useState<string | null>(null);

  // 加载笔记列表
  const loadNotes = useCallback(async (charId: string) => {
    setLoading(true);
    try {
      const list = await invoke<NoteSummary[]>('list_notebooks', { charId });
      setNotes(list);
    } catch (e) {
      console.error('加载笔记列表失败:', e);
      setNotes([]);
    } finally {
      setLoading(false);
    }
  }, []);

  // 加载笔记 HTML
  const loadNoteHtml = useCallback(async (charId: string, noteId: string) => {
    setLoadingHtml(true);
    try {
      const result = await invoke<{ html: string; note_id: string; font_path?: string | null; html_path?: string | null }>(
        'get_notebook_html',
        { charId, noteId },
      );
      // 以 asset 协议 URL 作为 iframe src 加载笔记（而非 srcDoc）：
      // - srcDoc 的文档与应用窗口同源，笔记内的 <script>/按钮/onclick 可访问整个窗口；
      // - asset 协议 URL（http://asset.localhost/...）与应用窗口（http://tauri.localhost）
      //   为跨源（cross-origin），笔记内脚本只能作用于笔记自身文档，实现隔离。
      // - 笔记内相对路径（如 fonts/ma-shan-zheng.woff2）相对 note.html 解析，
      //   与笔记文档同源，字体无需改写即可加载。
      setHtml(result.html_path ? convertFileSrc(result.html_path) : '');
    } catch (e) {
      console.error('加载笔记 HTML 失败:', e);
      // html 现为 asset URL（iframe src），加载失败时以 data URL 呈现错误提示
      setHtml('data:text/html;charset=utf-8,' + encodeURIComponent('<div style="padding:40px;text-align:center;color:#999;font-family:sans-serif;">加载失败</div>'));
    } finally {
      setLoadingHtml(false);
    }
  }, []);

  // 初始加载
  useEffect(() => {
    void loadNotes(character);
  }, [character, loadNotes]);

  // 从 pageParams 获取笔记 ID 并定位
  useEffect(() => {
    if (nav?.pageParams?.notebookId) {
      const nbChar = nav.pageParams.notebookCharacter as CharacterId | undefined;
      if (nbChar) setCharacter(nbChar);
      setSelectedId(nav.pageParams.notebookId as string);
      nav.clearPageParams();
    }
  }, [nav]);

  // 选中笔记变化时加载 HTML
  useEffect(() => {
    if (selectedId && !editing) {
      void loadNoteHtml(character, selectedId);
    } else {
      setHtml('');
    }
  }, [selectedId, character, loadNoteHtml, editing]);

  // 笔记 HTML 通过 iframe src（asset 协议 URL）渲染：笔记文档与应用窗口跨源隔离，
  // html/body/:root 等选择器在笔记自身文档内天然匹配；相对字体/图片路径相对
  // note.html 解析（与笔记文档同源），无需改写。
  useEffect(() => {
    const frame = iframeRef.current;
    if (!frame) return;
    if (html) {
      frame.src = html;
    } else {
      frame.src = 'about:blank';
    }
  }, [html]);

  // 监听笔记事件自动刷新
  useEffect(() => {
    const unlistens: (() => void)[] = [];
    const refresh = (charId?: string) => {
      if (!charId || charId === character) {
        void loadNotes(character);
      }
    };

    void (async () => {
      const u1 = await listen<{ char_id: string }>('notebook:created', (e) => refresh(e.payload?.char_id));
      const u2 = await listen<{ char_id: string }>('notebook:updated', (e) => {
        refresh(e.payload?.char_id);
        if (e.payload?.char_id === character && selectedId) {
          void loadNoteHtml(character, selectedId);
        }
      });
      const u3 = await listen<{ char_id: string; note_id: string }>('notebook:deleted', (e) => {
        refresh(e.payload?.char_id);
        if (e.payload?.note_id === selectedId) {
          setSelectedId(null);
          setHtml('');
        }
      });
      unlistens.push(u1, u2, u3);
    })();

    return () => unlistens.forEach((u) => u());
  }, [character, selectedId, loadNotes, loadNoteHtml]);

  // 删除笔记
  const handleDelete = useCallback(
    async (noteId: string, e: React.MouseEvent) => {
      e.stopPropagation();
      try {
        await invoke('delete_notebook', { charId: character, noteId });
        if (selectedId === noteId) {
          setSelectedId(null);
          setHtml('');
        }
      } catch (err) {
        console.error('删除笔记失败:', err);
      }
    },
    [character, selectedId],
  );

  // ===== 编辑相关 =====

  /** 导入本地 HTML 文件为笔记（绕过聊天通道的字符截断，直接读完整内容） */
  const handleImportHtml = useCallback(
    async (sourcePath: string) => {
      try {
        const res = await invoke<{ note_id: string; char_id: string; title: string }>(
          'import_html_note',
          { charId: character, sourcePath },
        );
        // notebook:created 事件会刷新列表；这里手动选中新导入的笔记
        setSelectedId(res.note_id);
        setHtml('');
      } catch (err) {
        console.error('导入 HTML 笔记失败:', err);
        void emit('toast:show', {
          message: `导入 HTML 失败：${String(err)}`,
          type: 'error', duration: 4000, key: Date.now(),
        });
      }
    },
    [character],
  );

  /** 文件选择器导入 HTML */
  const handlePickHtml = useCallback(async () => {
    try {
      const selected = await open({
        multiple: false,
        filters: [{ name: 'HTML', extensions: ['html', 'htm'] }],
      });
      if (!selected || Array.isArray(selected)) return;
      await handleImportHtml(selected as string);
    } catch (err) {
      console.error('选择 HTML 文件失败:', err);
    }
  }, [handleImportHtml]);

  // 笔记页原生拖放：拖入 .html/.htm 文件直接导入为笔记
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    void (async () => {
      unlisten = await getCurrentWindow().onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type !== 'drop') return;
        for (const p of payload.paths) {
          const ext = p.split('.').pop()?.toLowerCase();
          if (ext === 'html' || ext === 'htm') {
            void handleImportHtml(p);
          } else {
            void emit('toast:show', {
              message: `仅支持导入 .html/.htm 文件`,
              type: 'warning', duration: 3000, key: Date.now(),
            });
          }
        }
      });
    })();
    return () => { unlisten?.(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [handleImportHtml]);

  const patchDraft = useCallback((updater: (d: NoteBook) => NoteBook) => {
    setDraft((prev) => (prev ? updater(prev) : prev));
    setDirty(true);
  }, []);

  const startEdit = useCallback(
    async (noteId: string) => {
      setLoadingDetail(true);
      setEditError(null);
      try {
        const detail = await invoke<NoteBook>('get_notebook_detail', { charId: character, noteId });
        setDraft(detail);
        setIsNew(false);
        setEditing(true);
        setDirty(false);
      } catch (e) {
        console.error('加载笔记详情失败:', e);
        setEditError(t('notebook.load_detail_failed'));
      } finally {
        setLoadingDetail(false);
      }
    },
    [character, t],
  );

  const startNew = useCallback(() => {
    setDraft(emptyDraft(character));
    setIsNew(true);
    setEditing(true);
    setDirty(false);
    setEditError(null);
    setSelectedId(null);
    setHtml('');
  }, [character]);

  const cancelEdit = useCallback(() => {
    if (dirty && !window.confirm(t('notebook.discard_confirm'))) {
      return;
    }
    setEditing(false);
    setDraft(null);
    setDirty(false);
    setEditError(null);
  }, [dirty, t]);

  const saveEdit = useCallback(async () => {
    if (!draft) return;
    if (!draft.title.trim()) {
      setEditError(t('notebook.title') + ' ?');
      return;
    }
    if (draft.blocks.length === 0) {
      setEditError(t('notebook.add_block'));
      return;
    }
    setSaving(true);
    setEditError(null);
    try {
      const blocksJson = draft.blocks as unknown;
      const coverVal = draft.cover;
      if (isNew) {
        const res = await invoke<{ note_id: string }>('create_notebook', {
          charId: character,
          title: draft.title,
          blocks: blocksJson,
          layout: draft.layout,
          palette: draft.palette,
          tags: draft.tags,
          cover: coverVal,
        });
        setSelectedId(res.note_id);
      } else {
        await invoke('update_notebook', {
          charId: character,
          noteId: draft.id,
          title: draft.title,
          blocks: blocksJson,
          layout: draft.layout,
          palette: draft.palette,
          tags: draft.tags,
          cover: coverVal,
        });
      }
      setEditing(false);
      setDraft(null);
      setDirty(false);
      void loadNotes(character);
    } catch (e) {
      console.error('保存笔记失败:', e);
      setEditError(t('notebook.save_failed', { error: String(e) }));
    } finally {
      setSaving(false);
    }
  }, [draft, isNew, character, t, loadNotes]);

  // 块操作
  const updateBlock = useCallback(
    (idx: number, patch: Partial<Block>) => {
      patchDraft((d) => ({
        ...d,
        blocks: d.blocks.map((b, i) => (i === idx ? ({ ...b, ...patch } as Block) : b)),
      }));
    },
    [patchDraft],
  );

  const removeBlock = useCallback(
    (idx: number) => {
      patchDraft((d) => ({ ...d, blocks: d.blocks.filter((_, i) => i !== idx) }));
    },
    [patchDraft],
  );

  const moveBlock = useCallback(
    (idx: number, dir: -1 | 1) => {
      patchDraft((d) => {
        const next = idx + dir;
        if (next < 0 || next >= d.blocks.length) return d;
        const blocks = [...d.blocks];
        [blocks[idx], blocks[next]] = [blocks[next], blocks[idx]];
        return { ...d, blocks };
      });
    },
    [patchDraft],
  );

  const addBlock = useCallback(
    (type: BlockType) => {
      patchDraft((d) => ({ ...d, blocks: [...d.blocks, defaultBlock(type)] }));
    },
    [patchDraft],
  );

  const selectedNote = notes.find((n) => n.id === selectedId);

  // ============================================================
  // 渲染
  // ============================================================

  return (
    <div className="notebook-page" style={{ display: 'flex', height: '100%', gap: SPACING.md }}>
      {/* === 左侧：笔记列表 === */}
      <div
        style={{
          width: '36%',
          minWidth: 280,
          maxWidth: 420,
          display: 'flex',
          flexDirection: 'column',
          gap: SPACING.sm,
          position: 'relative',
        }}
      >
        {/* 装饰角标 */}
        <div
          style={{
            position: 'absolute',
            top: -4,
            right: -4,
            width: 20,
            height: 20,
            borderTop: '3px solid var(--panel-border-strong, #ddd)',
            borderRight: '3px solid var(--panel-border-strong, #ddd)',
            borderRadius: '0 4px 0 0',
            opacity: 0.3,
          }}
        />
        {/* 角色切换 + 新建 */}
        <div style={{ display: 'flex', gap: SPACING.xs, flexShrink: 0, alignItems: 'center' }}>
          {/* 角色胶囊切换控件 */}
          <div
            style={{
              flex: 1,
              display: 'flex',
              gap: 2,
              padding: 4,
              borderRadius: RADIUS.pill,
              background: COLORS.subtleBg,
              border: `1px solid ${COLORS.subtleBorder}`,
            }}
          >
            {(['vivian', 'nana'] as CharacterId[]).map((id) => {
              const active = character === id;
              return (
                <button
                  key={id}
                  onClick={() => {
                    setCharacter(id);
                    setSelectedId(null);
                    setHtml('');
                  }}
                  style={{
                    flex: 1,
                    padding: '7px 14px',
                    border: 'none',
                    borderRadius: RADIUS.pill,
                    background: active ? `${COLORS.accent}22` : 'transparent',
                    color: active ? COLORS.accent : COLORS.textSecondary,
                    fontWeight: 600,
                    fontSize: 14,
                    cursor: 'pointer',
                    transition: `all ${DURATION.normal}s ${EASE.swift}`,
                    fontFamily: TYPO.fontFamily,
                    letterSpacing: '0.5px',
                  }}
                  onMouseEnter={(e) => {
                    if (!active) e.currentTarget.style.background = COLORS.bgHover;
                  }}
                  onMouseLeave={(e) => {
                    e.currentTarget.style.background = active ? `${COLORS.accent}22` : 'transparent';
                  }}
                >
                  {CHAR_LABEL[id]}
                </button>
              );
            })}
          </div>
          {/* 新建笔记按钮（hover 反馈 + 按压反馈） */}
          <button
            onClick={startNew}
            disabled={editing}
            title={t('notebook.new_note')}
            style={{
              padding: '8px 12px',
              border: 'none',
              borderRadius: RADIUS.pill,
              background: COLORS.accentMuted,
              color: COLORS.accentBright,
              cursor: editing ? 'not-allowed' : 'pointer',
              opacity: editing ? 0.5 : 1,
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              transition: `all ${DURATION.normal}s ${EASE.swift}`,
            }}
            onMouseEnter={(e) => {
              e.currentTarget.style.background = `${COLORS.accent}22`;
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.background = COLORS.accentMuted;
            }}
            onMouseDown={(e) => {
              e.currentTarget.style.transform = 'scale(0.92)';
            }}
            onMouseUp={(e) => {
              e.currentTarget.style.transform = 'scale(1)';
            }}
          >
            <Plus size={16} />
          </button>
          {/* 导入 HTML 文件按钮（文件选择器，读取完整 HTML 转为笔记） */}
          <button
            onClick={() => void handlePickHtml()}
            disabled={editing}
            title={t('notebook.import_html', { defaultValue: '导入 HTML 文件' })}
            style={{
              padding: '8px 12px',
              border: 'none',
              borderRadius: RADIUS.pill,
              background: COLORS.subtleBg,
              color: COLORS.textSecondary,
              cursor: editing ? 'not-allowed' : 'pointer',
              opacity: editing ? 0.5 : 1,
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              transition: `all ${DURATION.normal}s ${EASE.swift}`,
            }}
            onMouseEnter={(e) => {
              e.currentTarget.style.background = COLORS.bgHover;
            }}
            onMouseLeave={(e) => {
              e.currentTarget.style.background = COLORS.subtleBg;
            }}
          >
            <Upload size={16} />
          </button>
        </div>

        {/* 笔记列表 */}
        <div
          style={{
            flex: 1,
            overflow: 'auto',
            display: 'flex',
            flexDirection: 'column',
            gap: SPACING.sm,
            paddingRight: 4,
          }}
        >
          {loading && notes.length === 0 ? (
            <div style={{ textAlign: 'center', color: COLORS.textTertiary, padding: SPACING.xl }}>
              {t('common.loading')}
            </div>
          ) : notes.length === 0 ? (
            <EmptyState
              icon={<NotebookPen size={36} strokeWidth={1.2} />}
              text={t('notebook.empty_hint', { name: t(`mind_inspector.common.char_${character}`) })}
            />
          ) : (
            notes.map((note) => (
              <NoteCard
                key={note.id}
                note={note}
                active={note.id === selectedId}
                onClick={() => setSelectedId(note.id)}
                onDelete={(e) => void handleDelete(note.id, e)}
              />
            ))
          )}
        </div>
      </div>

      {/* === 右侧：预览 / 编辑 === */}
      <div
        className="nb-fade"
        style={{
          flex: 1,
          minWidth: 0,
          borderRadius: '4px 18px 4px 18px',
          overflow: 'hidden',
          background: `linear-gradient(180deg, ${COLORS.bgSurface} 0%, ${COLORS.bgBase} 100%)`,
          boxShadow: SHADOW.card,
          border: `1.5px solid ${COLORS.border}`,
          display: 'flex',
          flexDirection: 'column',
          position: 'relative',
          animation: `nb-fade-up 0.45s ${EASE.decel} both`,
        }}
      >
        {editing && draft ? (
          <NoteEditor
            draft={draft}
            isNew={isNew}
            dirty={dirty}
            saving={saving}
            loadingDetail={loadingDetail}
            error={editError}
            lang={i18n.language}
            t={t}
            onPatch={patchDraft}
            onUpdateBlock={updateBlock}
            onRemoveBlock={removeBlock}
            onMoveBlock={moveBlock}
            onAddBlock={addBlock}
            onSave={saveEdit}
            onCancel={cancelEdit}
          />
        ) : selectedNote ? (
          <>
            {/* 预览顶栏 — 手账风格 */}
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                padding: `${SPACING.sm}px ${SPACING.md}px`,
                borderBottom: `1.5px solid ${COLORS.border}`,
                background: `linear-gradient(180deg, ${COLORS.bgSurface} 0%, ${COLORS.bgBase} 100%)`,
                flexShrink: 0,
                position: 'relative',
              }}
            >
              {/* 装饰胶带 */}
              <div
                style={{
                  position: 'absolute',
                  top: -6,
                  left: '30%',
                  width: 60,
                  height: 12,
                  background: 'rgba(255,255,255,0.35)',
                  borderRadius: 1,
                  transform: 'rotate(-2deg)',
                  boxShadow: '0 1px 2px rgba(0,0,0,0.06)',
                }}
              />
              <div style={{ display: 'flex', alignItems: 'center', gap: 8, overflow: 'hidden', flex: 1, minWidth: 0 }}>
                <span style={{ fontSize: 16, flexShrink: 0 }}>📄</span>
                <span
                  style={{
                    fontSize: 15,
                    fontWeight: 600,
                    color: COLORS.textPrimary,
                    fontFamily: TYPO.fontFamily,
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {selectedNote.title}
                </span>
              </div>
              <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.sm, flexShrink: 0 }}>
                <span style={{
                  fontSize: 12,
                  color: COLORS.textTertiary,
                  fontFamily: TYPO.fontFamily,
                  display: 'flex',
                  alignItems: 'center',
                  gap: 4,
                }}>
                  <span style={{ opacity: 0.5 }}>📝</span>
                  {formatTime(selectedNote.updated_at)}
                  <span style={{ opacity: 0.4 }}>·</span>
                  {selectedNote.render_type === 'raw_html'
                    ? 'HTML'
                    : `${selectedNote.block_count} ${t('notebook.blocks')}`}
                </span>
                {selectedNote.render_type === 'raw_html' ? (
                  <span
                    style={{
                      fontSize: 11,
                      color: COLORS.textTertiary,
                      fontFamily: TYPO.fontFamily,
                      border: `1px solid ${COLORS.border}`,
                      padding: '2px 8px',
                      borderRadius: '6px 2px 6px 2px',
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: 4,
                    }}
                  >
                    <FileText size={12} /> 只读
                  </span>
                ) : (
                  <button
                    onClick={() => void startEdit(selectedNote.id)}
                    title={t('notebook.edit')}
                    style={{
                      border: 'none',
                      background: 'transparent',
                      color: COLORS.accent,
                      cursor: 'pointer',
                      padding: '4px 10px',
                      borderRadius: '2px 8px 2px 8px',
                      display: 'inline-flex',
                      alignItems: 'center',
                      gap: 4,
                      fontFamily: TYPO.fontFamily,
                      fontSize: 12,
                      fontWeight: 600,
                      transition: `background ${DURATION.fast}s ${EASE.swift}`,
                    }}
                    onMouseEnter={(e) => {
                      e.currentTarget.style.background = COLORS.bgActive;
                    }}
                    onMouseLeave={(e) => {
                      e.currentTarget.style.background = 'transparent';
                    }}
                  >
                    <Pencil size={13} /> 编辑
                  </button>
                )}
              </div>
            </div>
            {/* 笔记预览（iframe src = asset URL 渲染完整文档，跨源隔离） */}
            <div style={{ flex: 1, overflow: 'auto', position: 'relative' }}>
              {loadingHtml ? (
                <div
                  style={{
                    position: 'absolute',
                    inset: 0,
                    display: 'flex',
                    flexDirection: 'column',
                    alignItems: 'center',
                    justifyContent: 'center',
                    color: COLORS.textTertiary,
                    fontSize: 14,
                    fontFamily: TYPO.fontFamily,
                    gap: 8,
                    background: 'var(--panel-bg-surface)',
                    zIndex: 10,
                  }}
                >
                  <span style={{ fontSize: 24, opacity: 0.5 }}>📖</span>
                  <span>{t('common.loading')}</span>
                </div>
              ) : null}
              <iframe
                ref={iframeRef}
                title="notebook-preview"
                src={html}
                style={{
                  width: '100%',
                  height: '100%',
                  minHeight: 0,
                  border: 'none',
                  display: 'block',
                }}
              />
            </div>
          </>
        ) : (
          <div
            style={{
              flex: 1,
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              justifyContent: 'center',
              gap: SPACING.md,
              color: COLORS.textTertiary,
              background: `linear-gradient(180deg, ${COLORS.bgSurface} 0%, ${COLORS.bgBase} 100%)`,
              position: 'relative',
              overflow: 'hidden',
            }}
          >
            {/* 装饰元素 */}
            <div style={{
              position: 'absolute',
              top: -20,
              right: -20,
              width: 120,
              height: 120,
              borderRadius: '50%',
              background: COLORS.accentMuted,
              opacity: 0.3,
            }} />
            <div style={{
              position: 'absolute',
              bottom: -30,
              left: -30,
              width: 80,
              height: 80,
              borderRadius: '50%',
              background: COLORS.accentMuted,
              opacity: 0.2,
            }} />
            <div style={{
              position: 'absolute',
              top: '30%',
              left: '10%',
              width: 40,
              height: 12,
              background: 'rgba(255,255,255,0.3)',
              borderRadius: 1,
              transform: 'rotate(-15deg)',
            }} />
            <div style={{
              position: 'absolute',
              bottom: '25%',
              right: '15%',
              width: 50,
              height: 12,
              background: 'rgba(255,255,255,0.25)',
              borderRadius: 1,
              transform: 'rotate(8deg)',
            }} />
            <EmptyState
              icon={<NotebookPen size={40} strokeWidth={1.2} />}
              text={t('notebook.select_hint')}
            />
            <button
              onClick={() => void handlePickHtml()}
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 8,
                padding: '10px 18px',
                border: `1.5px dashed ${COLORS.borderAccent}`,
                borderRadius: '4px 14px 4px 14px',
                background: COLORS.bgSurface,
                color: COLORS.accent,
                cursor: 'pointer',
                fontFamily: TYPO.fontFamily,
                fontSize: 13,
                fontWeight: 600,
                transition: `all ${DURATION.fast}s ${EASE.swift}`,
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.background = COLORS.accentMuted;
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = COLORS.bgSurface;
              }}
            >
              <Upload size={15} />
              {t('notebook.import_html', { defaultValue: '导入 HTML 文件' })}
            </button>
            <div
              style={{
                fontSize: 12,
                color: COLORS.textTertiary,
                fontFamily: TYPO.fontFamily,
                display: 'flex',
                alignItems: 'center',
                gap: 6,
              }}
            >
              <span style={{ opacity: 0.6 }}>↧</span>
              {t('notebook.import_drop_hint', { defaultValue: '或将 .html 文件拖入此窗口' })}
            </div>
          </div>
        )}
      </div>
    </div>
  );
};

// ============================================================
// NoteCard 子组件
// ============================================================

const NoteCard: React.FC<{
  note: NoteSummary;
  active: boolean;
  onClick: () => void;
  onDelete: (e: React.MouseEvent) => void;
}> = ({ note, active, onClick, onDelete }) => {
  const [hovered, setHovered] = useState(false);
  const paletteGradient = PALETTE_COLORS[note.palette] || PALETTE_COLORS.warm;
  const cardIndex = useRef(Math.floor(Math.random() * 3));

  return (
    <div
      className="nb-note-card"
      onClick={onClick}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        position: 'relative',
        padding: '14px 16px 12px 18px',
        borderRadius: '3px 14px 3px 14px',
        background: active
          ? `linear-gradient(135deg, ${COLORS.accentMuted} 0%, ${COLORS.bgSurface} 100%)`
          : hovered
            ? COLORS.bgHover
            : COLORS.bgSurface,
        border: `1.5px solid ${active ? COLORS.borderAccent : COLORS.border}`,
        cursor: 'pointer',
        transition: `all ${DURATION.normal}s ${EASE.swift}`,
        overflow: 'hidden',
        boxShadow: hovered ? SHADOW.cardHover : SHADOW.subtle,
        transform: active
          ? 'translateY(-1px) rotate(-0.3deg)'
          : hovered
            ? 'translateY(-2px) rotate(0deg)'
            : 'translateY(0) rotate(0deg)',
        animation: !active ? `card-enter 0.35s ${EASE.ios} backwards` : 'none',
        animationDelay: `${cardIndex.current * 0.04}s`,
      }}
    >
      {/* 纸胶带装饰条 */}
      <div
        style={{
          position: 'absolute',
          left: 0,
          top: 0,
          bottom: 0,
          width: 5,
          background: paletteGradient,
          borderRadius: '0 2px 2px 0',
          boxShadow: active ? `inset 0 0 8px rgba(0,0,0,0.1)` : 'none',
        }}
      />

      {/* 纸胶带顶部装饰 */}
      <div
        style={{
          position: 'absolute',
          top: -6,
          left: 24,
          width: 40,
          height: 12,
          background: active ? 'rgba(255,255,255,0.5)' : 'rgba(255,255,255,0.35)',
          borderRadius: 1,
          transform: 'rotate(-4deg)',
          boxShadow: '0 1px 2px rgba(0,0,0,0.06)',
          opacity: hovered ? 0.8 : 0.5,
          transition: `opacity ${DURATION.normal}s ${EASE.swift}`,
        }}
      />

      {/* 标题（手写体风格） */}
      <div
        style={{
          fontSize: 16,
          fontWeight: 600,
          color: active ? COLORS.accentBright : COLORS.textPrimary,
          marginBottom: 6,
          overflow: 'hidden',
          textOverflow: 'ellipsis',
          whiteSpace: 'nowrap',
          paddingLeft: 10,
          fontFamily: TYPO.fontFamily,
          letterSpacing: '0.3px',
        }}
      >
        {note.title || '无标题'}
      </div>

      {/* 标签（贴纸风格） */}
      {note.tags.length > 0 && (
        <div
          style={{
            display: 'flex',
            flexWrap: 'wrap',
            gap: 5,
            marginBottom: 6,
            paddingLeft: 10,
          }}
        >
          {note.tags.slice(0, 4).map((tag, i) => (
            <span
              key={i}
              style={{
                fontSize: 11,
                color: active ? COLORS.accentBright : COLORS.textSecondary,
                background: active ? COLORS.accentSoft : COLORS.subtleBg,
                padding: '2px 10px',
                borderRadius: '2px 10px 2px 10px',
                lineHeight: 1.5,
                transform: i % 2 === 0 ? 'rotate(-1deg)' : 'rotate(0.5deg)',
                letterSpacing: '0.3px',
                fontFamily: TYPO.fontFamily,
                border: active ? `1px solid ${COLORS.borderAccent}` : 'none',
              }}
            >
              {tag}
            </span>
          ))}
          {note.tags.length > 4 && (
            <span
              style={{
                fontSize: 10,
                color: COLORS.textTertiary,
                padding: '2px 6px',
                fontFamily: TYPO.fontFamily,
              }}
            >
              +{note.tags.length - 4}
            </span>
          )}
        </div>
      )}

      {/* 底栏：时间 + 删除 */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          paddingLeft: 10,
        }}
      >
        <span
          style={{
            fontSize: 12,
            color: COLORS.textTertiary,
            fontFamily: TYPO.fontFamily,
            display: 'flex',
            alignItems: 'center',
            gap: 4,
          }}
        >
          <span style={{ opacity: 0.5 }}>📝</span>
          {formatTime(note.updated_at)}
        </span>
        <button
          onClick={onDelete}
          title="删除"
          style={{
            border: 'none',
            background: 'transparent',
            color: COLORS.danger,
            cursor: 'pointer',
            padding: 4,
            borderRadius: RADIUS.xs,
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            visibility: hovered ? 'visible' : 'hidden',
            transition: `all ${DURATION.fast}s ${EASE.swift}`,
            opacity: hovered ? 0.7 : 0,
          }}
          onMouseEnter={(e) => {
            e.currentTarget.style.background = COLORS.bgActive;
            e.currentTarget.style.opacity = '1';
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.background = 'transparent';
            e.currentTarget.style.opacity = '0.7';
          }}
        >
          <Trash2 size={13} />
        </button>
      </div>
    </div>
  );
};

// ============================================================
// NoteEditor 子组件（编辑模式）
// ============================================================

const inputStyle: React.CSSProperties = {
  width: '100%',
  padding: '10px 12px',
  border: `1.5px solid ${COLORS.border}`,
  borderRadius: '3px 10px 3px 10px',
  background: COLORS.bgSurface,
  color: COLORS.textPrimary,
  fontSize: 14,
  fontFamily: TYPO.fontFamily,
  outline: 'none',
  transition: `border-color ${DURATION.fast}s ${EASE.swift}, box-shadow ${DURATION.fast}s ${EASE.swift}`,
  boxShadow: 'inset 0 1px 3px rgba(0,0,0,0.04)',
};

const textareaStyle: React.CSSProperties = {
  ...inputStyle,
  resize: 'vertical',
  minHeight: 72,
  lineHeight: 1.7,
  borderRadius: '3px 12px 3px 12px',
};

const labelStyle: React.CSSProperties = {
  fontSize: 12,
  fontWeight: 600,
  color: COLORS.textSecondary,
  marginBottom: 6,
  display: 'block',
  fontFamily: TYPO.fontFamily,
  letterSpacing: '0.5px',
  paddingLeft: 2,
};

const NoteEditor: React.FC<{
  draft: NoteBook;
  isNew: boolean;
  dirty: boolean;
  saving: boolean;
  loadingDetail: boolean;
  error: string | null;
  lang: string;
  t: (key: string, opts?: Record<string, unknown>) => string;
  onPatch: (updater: (d: NoteBook) => NoteBook) => void;
  onUpdateBlock: (idx: number, patch: Partial<Block>) => void;
  onRemoveBlock: (idx: number) => void;
  onMoveBlock: (idx: number, dir: -1 | 1) => void;
  onAddBlock: (type: BlockType) => void;
  onSave: () => void;
  onCancel: () => void;
}> = ({
  draft,
  isNew,
  saving,
  loadingDetail,
  error,
  lang,
  t,
  onPatch,
  onUpdateBlock,
  onRemoveBlock,
  onMoveBlock,
  onAddBlock,
  onSave,
  onCancel,
}) => {
  const [addType, setAddType] = useState<BlockType>('paragraph');
  const [mode, setMode] = useState<'form' | 'wysiwyg'>('wysiwyg');
  const showCover = draft.layout === 'cover_flow' || draft.layout === 'gallery';
  const cover = draft.cover;

  const tagsText = useMemo(() => draft.tags.join(', '), [draft.tags]);

  if (loadingDetail) {
    return (
      <div
        style={{
          flex: 1,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          color: COLORS.textTertiary,
        }}
      >
        {t('common.loading')}
      </div>
    );
  }

  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0 }}>
      {/* 编辑顶栏 — 手账风格 */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          padding: `${SPACING.sm}px ${SPACING.md}px`,
          borderBottom: `1.5px solid ${COLORS.border}`,
          background: `linear-gradient(180deg, ${COLORS.bgSurface} 0%, ${COLORS.bgBase} 100%)`,
          flexShrink: 0,
          position: 'relative',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <span style={{ fontSize: 16 }}>{isNew ? '✨' : '✏️'}</span>
          <span style={{
            fontSize: 15,
            fontWeight: 600,
            color: COLORS.textPrimary,
            fontFamily: TYPO.fontFamily,
          }}>
            {isNew ? t('notebook.new_note') : t('notebook.edit')}
          </span>
        </div>
        <div style={{ display: 'flex', gap: SPACING.xs, alignItems: 'center' }}>
          {/* 编辑模式切换（胶囊切换控件） */}
          <div
            style={{
              display: 'flex',
              gap: 2,
              padding: 4,
              borderRadius: RADIUS.pill,
              background: COLORS.subtleBg,
              border: `1px solid ${COLORS.subtleBorder}`,
            }}
          >
            <button
              onClick={() => setMode('wysiwyg')}
              title={t('notebook.mode_wysiwyg')}
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 4,
                padding: '6px 12px',
                borderRadius: RADIUS.pill,
                border: 'none',
                background: mode === 'wysiwyg' ? `${COLORS.accent}22` : 'transparent',
                color: mode === 'wysiwyg' ? COLORS.accent : COLORS.textSecondary,
                fontSize: 12,
                fontWeight: 600,
                cursor: 'pointer',
                fontFamily: TYPO.fontFamily,
                transition: `all ${DURATION.normal}s ${EASE.swift}`,
              }}
              onMouseEnter={(e) => {
                if (mode !== 'wysiwyg') e.currentTarget.style.background = COLORS.bgHover;
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = mode === 'wysiwyg' ? `${COLORS.accent}22` : 'transparent';
              }}
            >
              <MousePointerClick size={13} /> {t('notebook.mode_wysiwyg')}
            </button>
            <button
              onClick={() => setMode('form')}
              title={t('notebook.mode_form')}
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 4,
                padding: '6px 12px',
                borderRadius: RADIUS.pill,
                border: 'none',
                background: mode === 'form' ? `${COLORS.accent}22` : 'transparent',
                color: mode === 'form' ? COLORS.accent : COLORS.textSecondary,
                fontSize: 12,
                fontWeight: 600,
                cursor: 'pointer',
                fontFamily: TYPO.fontFamily,
                transition: `all ${DURATION.normal}s ${EASE.swift}`,
              }}
              onMouseEnter={(e) => {
                if (mode !== 'form') e.currentTarget.style.background = COLORS.bgHover;
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = mode === 'form' ? `${COLORS.accent}22` : 'transparent';
              }}
            >
              <ListChecks size={13} /> {t('notebook.mode_form')}
            </button>
          </div>
          <ActionButton onClick={onCancel} disabled={saving}>
            <X size={14} /> {t('common.cancel')}
          </ActionButton>
          <ActionButton primary onClick={onSave} disabled={saving}>
            {saving ? <Check size={14} /> : <Check size={14} />} {saving ? t('common.saving') : t('common.save')}
          </ActionButton>
        </div>
      </div>

      {/* 编辑区 — 手账笔记本风格 */}
      <div
        style={{
          flex: 1,
          overflow: 'auto',
          padding: `${SPACING.md}px ${SPACING.md}px ${SPACING.lg}px`,
          display: 'flex',
          flexDirection: 'column',
          gap: SPACING.md,
          background: `linear-gradient(180deg, ${COLORS.bgSurface} 0%, ${COLORS.bgBase} 100%)`,
          position: 'relative',
        }}
      >
        {/* 装饰横线 */}
        <div
          style={{
            position: 'absolute',
            top: 0, left: 0, right: 0,
            height: 3,
            background: `linear-gradient(90deg, transparent 0%, ${COLORS.accent} 20%, ${COLORS.accent} 80%, transparent 100%)`,
            opacity: 0.15,
          }}
        />

        {/* 标题 */}
        <div>
          <label style={labelStyle}>
            <span style={{ marginRight: 4 }}>📖</span>
            {t('notebook.title')}
          </label>
          <input
            style={{ ...inputStyle, fontSize: 17, fontWeight: 600, fontFamily: TYPO.fontFamily }}
            value={draft.title}
            placeholder={t('notebook.title')}
            onChange={(e) => onPatch((d) => ({ ...d, title: e.target.value }))}
          />
        </div>

        {/* 配色 */}
        <div>
          <label style={labelStyle}>
            <span style={{ marginRight: 4 }}>🎨</span>
            {t('notebook.palette')}
          </label>
          <div style={{ display: 'flex', gap: 10, flexWrap: 'wrap' }}>
            {PALETTE_KEYS.map((key) => (
              <button
                key={key}
                onClick={() => onPatch((d) => ({ ...d, palette: key }))}
                title={key}
                style={{
                  width: 34,
                  height: 34,
                  borderRadius: '3px 14px 3px 14px',
                  border: draft.palette === key ? `2.5px solid ${COLORS.accentBright}` : '2px solid transparent',
                  background: PALETTE_COLORS[key],
                  cursor: 'pointer',
                  padding: 0,
                  transition: `transform ${DURATION.fast}s ${EASE.spring}, box-shadow ${DURATION.fast}s ${EASE.swift}`,
                  transform: draft.palette === key ? 'scale(1.15) rotate(-5deg)' : 'scale(1)',
                  boxShadow: draft.palette === key ? `0 0 12px ${COLORS.accentGlow}` : 'none',
                }}
                onMouseEnter={(e) => {
                  e.currentTarget.style.transform = draft.palette === key ? 'scale(1.2) rotate(-5deg)' : 'scale(1.12)';
                  e.currentTarget.style.boxShadow = `0 0 12px ${COLORS.accentGlow}`;
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.transform = draft.palette === key ? 'scale(1.15) rotate(-5deg)' : 'scale(1)';
                  e.currentTarget.style.boxShadow = draft.palette === key ? `0 0 12px ${COLORS.accentGlow}` : 'none';
                }}
              />
            ))}
          </div>
        </div>

        {/* 布局 */}
        <div>
          <label style={labelStyle}>
            <span style={{ marginRight: 4 }}>📐</span>
            {t('notebook.layout')}
          </label>
          <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
            {LAYOUT_OPTIONS.map((opt) => {
              const active = draft.layout === opt.value;
              return (
                <button
                  key={opt.value}
                  onClick={() => onPatch((d) => ({ ...d, layout: opt.value }))}
                  style={{
                    padding: '6px 14px',
                    borderRadius: RADIUS.pill,
                    border: `1px solid ${active ? COLORS.accent : COLORS.subtleBorder}`,
                    background: active ? `${COLORS.accent}22` : 'transparent',
                    color: active ? COLORS.accent : COLORS.textSecondary,
                    fontSize: 12,
                    fontWeight: 600,
                    cursor: 'pointer',
                    fontFamily: TYPO.fontFamily,
                    transition: `all ${DURATION.fast}s ${EASE.swift}`,
                    transform: active ? 'translateY(-1px)' : 'translateY(0)',
                  }}
                  onMouseEnter={(e) => {
                    if (!active) e.currentTarget.style.background = COLORS.bgHover;
                  }}
                  onMouseLeave={(e) => {
                    e.currentTarget.style.background = active ? `${COLORS.accent}22` : 'transparent';
                  }}
                >
                  {layoutLabel(opt.value, lang)}
                </button>
              );
            })}
          </div>
        </div>

        {/* 标签 */}
        <div>
          <label style={labelStyle}>
            <span style={{ marginRight: 4 }}>🏷️</span>
            {t('notebook.tags')}
            <span style={{ fontWeight: 400, color: COLORS.textTertiary, fontSize: 11 }}> ({t('notebook.items_hint')})</span>
          </label>
          <input
            style={inputStyle}
            value={tagsText}
            placeholder="美食, 旅行, 攻略"
            onChange={(e) =>
              onPatch((d) => ({
                ...d,
                tags: e.target.value
                  .split(',')
                  .map((s) => s.trim())
                  .filter(Boolean),
              }))
            }
          />
        </div>

        {/* 封面 */}
        {showCover && (
          <div
            style={{
              padding: SPACING.md,
              borderRadius: '3px 14px 3px 14px',
              background: COLORS.subtleBg,
              border: `1.5px dashed ${COLORS.border}`,
              display: 'flex',
              flexDirection: 'column',
              gap: SPACING.sm,
              position: 'relative',
            }}
          >
            <div style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              borderBottom: `1px dashed ${COLORS.border}`,
              paddingBottom: 8,
            }}>
              <label style={{ ...labelStyle, marginBottom: 0, display: 'flex', alignItems: 'center', gap: 4 }}>
                <span>📄</span> {t('notebook.cover')}
              </label>
              <button
                onClick={() =>
                  onPatch((d) => ({
                    ...d,
                    cover: d.cover
                      ? null
                      : { title: d.title, subtitle: '', emoji: '', background: '' },
                  }))
                }
                style={{
                  border: 'none',
                  background: 'transparent',
                  color: COLORS.accent,
                  fontSize: 12,
                  cursor: 'pointer',
                  padding: '2px 8px',
                  borderRadius: '2px 8px 2px 8px',
                  fontFamily: TYPO.fontFamily,
                }}
              >
                {cover ? '✕ 删除' : '+ 添加'}
              </button>
            </div>
            {cover && (
              <>
                <div>
                  <label style={labelStyle}>{t('notebook.cover_title')}</label>
                  <input
                    style={inputStyle}
                    value={cover.title}
                    onChange={(e) =>
                      onPatch((d) => ({
                        ...d,
                        cover: { ...d.cover!, title: e.target.value },
                      }))
                    }
                  />
                </div>
                <div>
                  <label style={labelStyle}>{t('notebook.cover_subtitle')}</label>
                  <input
                    style={inputStyle}
                    value={cover.subtitle || ''}
                    onChange={(e) =>
                      onPatch((d) => ({
                        ...d,
                        cover: { ...d.cover!, subtitle: e.target.value },
                      }))
                    }
                  />
                </div>
                <div style={{ display: 'flex', gap: SPACING.sm }}>
                  <div style={{ flex: '0 0 120px' }}>
                    <label style={labelStyle}>{t('notebook.cover_emoji')}</label>
                    <input
                      style={inputStyle}
                      value={cover.emoji || ''}
                      onChange={(e) =>
                        onPatch((d) => ({
                          ...d,
                          cover: { ...d.cover!, emoji: e.target.value },
                        }))
                      }
                    />
                  </div>
                  <div style={{ flex: 1 }}>
                    <label style={labelStyle}>{t('notebook.cover_bg')}</label>
                    <input
                      style={inputStyle}
                      value={cover.background || ''}
                      placeholder="#FF6B6B / linear-gradient(...)"
                      onChange={(e) =>
                        onPatch((d) => ({
                          ...d,
                          cover: { ...d.cover!, background: e.target.value },
                        }))
                      }
                    />
                  </div>
                </div>
              </>
            )}
          </div>
        )}

        {/* 内容块：可视化编辑 / 表单编辑 */}
        {mode === 'wysiwyg' ? (
          <WysiwygEditor
            blocks={draft.blocks}
            onUpdateBlock={onUpdateBlock}
            onRemoveBlock={onRemoveBlock}
            onMoveBlock={onMoveBlock}
            onAddBlock={onAddBlock}
            t={t}
          />
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: SPACING.sm }}>
            <label style={labelStyle}>
              <span style={{ marginRight: 4 }}>📝</span>
              {t('notebook.blocks')}
              <span style={{ fontWeight: 400, color: COLORS.textTertiary, fontSize: 11 }}> ({draft.blocks.length})</span>
            </label>
            {draft.blocks.map((block, idx) => (
              <BlockEditor
                key={idx}
                block={block}
                index={idx}
                total={draft.blocks.length}
                t={t}
                onUpdate={(patch) => onUpdateBlock(idx, patch)}
                onRemove={() => onRemoveBlock(idx)}
                onMove={(dir) => onMoveBlock(idx, dir)}
              />
            ))}
            {draft.blocks.length === 0 && (
              <div style={{
                padding: SPACING.md,
                textAlign: 'center',
                color: COLORS.textTertiary,
                fontSize: 13,
                fontFamily: TYPO.fontFamily,
                border: `1.5px dashed ${COLORS.border}`,
                borderRadius: '3px 12px 3px 12px',
              }}>
                {t('notebook.add_block')}
              </div>
            )}
          </div>
        )}

        {/* 添加块 */}
        <div
          style={{
            display: 'flex',
            gap: SPACING.xs,
            padding: `${SPACING.sm}px 0`,
            borderTop: `1.5px dashed ${COLORS.border}`,
            marginTop: 4,
          }}
        >
          <select
            value={addType}
            onChange={(e) => setAddType(e.target.value as BlockType)}
            style={{
              ...inputStyle,
              flex: 1,
              cursor: 'pointer',
              appearance: 'auto',
              fontFamily: TYPO.fontFamily,
            }}
          >
            {BLOCK_TYPES.map((bt) => (
              <option key={bt} value={bt}>
                {bt}
              </option>
            ))}
          </select>
          <ActionButton primary onClick={() => onAddBlock(addType)} style={{ whiteSpace: 'nowrap' }}>
            <Plus size={14} /> {t('notebook.add_block')}
          </ActionButton>
        </div>

        {/* 错误提示 */}
        {error && (
          <div
            style={{
              padding: `${SPACING.sm}px ${SPACING.md}px`,
              borderRadius: '3px 10px 3px 10px',
              background: 'rgba(229, 57, 53, 0.08)',
              color: COLORS.danger,
              fontSize: 13,
              border: `1.5px solid rgba(229, 57, 53, 0.2)`,
              fontFamily: TYPO.fontFamily,
            }}
          >
            {error}
          </div>
        )}
      </div>
    </div>
  );
};

// ============================================================
// BlockEditor 子组件（单块编辑器）
// ============================================================

const BlockEditor: React.FC<{
  block: Block;
  index: number;
  total: number;
  t: (key: string, opts?: Record<string, unknown>) => string;
  onUpdate: (patch: Partial<Block>) => void;
  onRemove: () => void;
  onMove: (dir: -1 | 1) => void;
}> = ({ block, index, total, t, onUpdate, onRemove, onMove }) => {
  const type = block.type;

  const itemsText = useMemo(() => {
    if (type === 'list' || type === 'tags') {
      return block.items.join('\n');
    }
    return '';
  }, [block, type]);

  const tableHeadersText = useMemo(() => {
    if (type === 'table') return block.headers.join('\n');
    return '';
  }, [block, type]);

  const tableRowsText = useMemo(() => {
    if (type === 'table') return block.rows.map((r) => r.join('\t')).join('\n');
    return '';
  }, [block, type]);

  const chartCatsText = useMemo(() => {
    if (type === 'chart') return block.categories.join('\n');
    return '';
  }, [block, type]);

  const chartSeriesText = useMemo(() => {
    if (type === 'chart')
      return block.series.map((s) => `${s.name}\t${s.data.join(',')}`).join('\n');
    return '';
  }, [block, type]);

  return (
    <div
      style={{
        padding: SPACING.md,
        borderRadius: '3px 14px 3px 14px',
        background: COLORS.bgSurface,
        border: `1.5px solid ${COLORS.border}`,
        display: 'flex',
        flexDirection: 'column',
        gap: SPACING.sm,
        position: 'relative',
        transition: `border-color ${DURATION.fast}s ${EASE.swift}`,
      }}
    >
      {/* 装饰胶带 */}
      <div
        style={{
          position: 'absolute',
          top: -6,
          right: 20,
          width: 36,
          height: 12,
          background: 'rgba(255,255,255,0.4)',
          borderRadius: 1,
          transform: 'rotate(3deg)',
          boxShadow: '0 1px 2px rgba(0,0,0,0.06)',
        }}
      />
      {/* 块顶栏：类型 + 操作 */}
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
        <span
          style={{
            fontSize: 11,
            fontWeight: 600,
            color: COLORS.accent,
            background: COLORS.accentMuted,
            padding: '2px 10px',
            borderRadius: '2px 10px 2px 10px',
            letterSpacing: 0.5,
            fontFamily: TYPO.fontFamily,
          }}
        >
          {type}
        </span>
        <div style={{ display: 'flex', gap: 2 }}>
          <IconBtn disabled={index === 0} title={t('notebook.move_up')} onClick={() => onMove(-1)}>
            <ChevronUp size={14} />
          </IconBtn>
          <IconBtn disabled={index === total - 1} title={t('notebook.move_down')} onClick={() => onMove(1)}>
            <ChevronDown size={14} />
          </IconBtn>
          <IconBtn title={t('notebook.delete_block')} danger onClick={onRemove}>
            <Trash2 size={14} />
          </IconBtn>
        </div>
      </div>

      {/* 类型特定字段 */}
      {type === 'heading' && (
        <>
          <div style={{ display: 'flex', gap: SPACING.sm }}>
            <div style={{ flex: '0 0 90px' }}>
              <label style={labelStyle}>{t('notebook.level')}</label>
              <select
                style={{ ...inputStyle, cursor: 'pointer', appearance: 'auto' }}
                value={block.level}
                onChange={(e) => onUpdate({ level: Number(e.target.value) } as Partial<Block>)}
              >
                <option value={1}>H1</option>
                <option value={2}>H2</option>
                <option value={3}>H3</option>
              </select>
            </div>
            <div style={{ flex: 1 }}>
              <label style={labelStyle}>{t('notebook.text')}</label>
              <input
                style={inputStyle}
                value={block.text}
                onChange={(e) => onUpdate({ text: e.target.value } as Partial<Block>)}
              />
            </div>
          </div>
        </>
      )}

      {type === 'paragraph' && (
        <div>
          <label style={labelStyle}>{t('notebook.text')}</label>
          <textarea
            style={textareaStyle}
            value={block.text}
            onChange={(e) => onUpdate({ text: e.target.value } as Partial<Block>)}
          />
        </div>
      )}

      {type === 'card' && (
        <>
          <div>
            <label style={labelStyle}>{t('notebook.cover_title')}</label>
            <input
              style={inputStyle}
              value={block.title || ''}
              onChange={(e) => onUpdate({ title: e.target.value } as Partial<Block>)}
            />
          </div>
          <div>
            <label style={labelStyle}>{t('notebook.body')}</label>
            <textarea
              style={textareaStyle}
              value={block.body}
              onChange={(e) => onUpdate({ body: e.target.value } as Partial<Block>)}
            />
          </div>
          <div style={{ flex: '0 0 120px' }}>
            <label style={labelStyle}>{t('notebook.emoji')}</label>
            <input
              style={inputStyle}
              value={block.emoji || ''}
              onChange={(e) => onUpdate({ emoji: e.target.value } as Partial<Block>)}
            />
          </div>
        </>
      )}

      {type === 'quote' && (
        <>
          <div>
            <label style={labelStyle}>{t('notebook.text')}</label>
            <textarea
              style={textareaStyle}
              value={block.text}
              onChange={(e) => onUpdate({ text: e.target.value } as Partial<Block>)}
            />
          </div>
          <div>
            <label style={labelStyle}>{t('notebook.author')}</label>
            <input
              style={inputStyle}
              value={block.author || ''}
              onChange={(e) => onUpdate({ author: e.target.value } as Partial<Block>)}
            />
          </div>
        </>
      )}

      {type === 'list' && (
        <>
          <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.sm }}>
            <label
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 6,
                fontSize: 12,
                color: COLORS.textSecondary,
                cursor: 'pointer',
              }}
            >
              <input
                type="checkbox"
                checked={block.ordered || false}
                onChange={(e) => onUpdate({ ordered: e.target.checked } as Partial<Block>)}
              />
              {t('notebook.ordered')}
            </label>
          </div>
          <div>
            <label style={labelStyle}>
              {t('notebook.text')}{' '}
              <span style={{ fontWeight: 400, color: COLORS.textTertiary }}>{t('notebook.items_hint')}</span>
            </label>
            <textarea
              style={textareaStyle}
              value={itemsText}
              onChange={(e) =>
                onUpdate({
                  items: e.target.value.split('\n'),
                } as Partial<Block>)
              }
            />
          </div>
        </>
      )}

      {type === 'tags' && (
        <div>
          <label style={labelStyle}>
            {t('notebook.tags')}{' '}
            <span style={{ fontWeight: 400, color: COLORS.textTertiary }}>{t('notebook.items_hint')}</span>
          </label>
          <textarea
            style={textareaStyle}
            value={itemsText}
            onChange={(e) =>
              onUpdate({
                items: e.target.value.split('\n'),
              } as Partial<Block>)
            }
          />
        </div>
      )}

      {type === 'image' && (
        <>
          <div>
            <label style={labelStyle}>{t('notebook.url')}</label>
            <input
              style={inputStyle}
              value={block.url}
              onChange={(e) => onUpdate({ url: e.target.value } as Partial<Block>)}
            />
          </div>
          <div>
            <label style={labelStyle}>{t('notebook.caption')}</label>
            <input
              style={inputStyle}
              value={block.caption || ''}
              onChange={(e) => onUpdate({ caption: e.target.value } as Partial<Block>)}
            />
          </div>
        </>
      )}

      {type === 'divider' && (
        <div style={{ flex: '0 0 120px' }}>
          <label style={labelStyle}>{t('notebook.emoji')}</label>
          <input
            style={inputStyle}
            value={block.emoji || ''}
            onChange={(e) => onUpdate({ emoji: e.target.value } as Partial<Block>)}
          />
        </div>
      )}

      {type === 'callout' && (
        <>
          <div>
            <label style={labelStyle}>{t('notebook.text')}</label>
            <textarea
              style={textareaStyle}
              value={block.text}
              onChange={(e) => onUpdate({ text: e.target.value } as Partial<Block>)}
            />
          </div>
          <div style={{ flex: '0 0 120px' }}>
            <label style={labelStyle}>{t('notebook.emoji')}</label>
            <input
              style={inputStyle}
              value={block.emoji || ''}
              onChange={(e) => onUpdate({ emoji: e.target.value } as Partial<Block>)}
            />
          </div>
        </>
      )}

      {type === 'table' && (
        <>
          <div>
            <label style={labelStyle}>
              {t('notebook.table_headers')}{' '}
              <span style={{ fontWeight: 400, color: COLORS.textTertiary }}>{t('notebook.items_hint')}</span>
            </label>
            <textarea
              style={textareaStyle}
              value={tableHeadersText}
              placeholder={'城市\t人均预算'}
              onChange={(e) =>
                onUpdate({
                  headers: e.target.value.split('\n').filter(Boolean),
                } as Partial<Block>)
              }
            />
          </div>
          <div>
            <label style={labelStyle}>
              {t('notebook.table_rows')}{' '}
              <span style={{ fontWeight: 400, color: COLORS.textTertiary }}>{t('notebook.table_rows_hint')}</span>
            </label>
            <textarea
              style={{ ...textareaStyle, minHeight: 100, fontFamily: TYPO.fontMono, fontSize: 12 }}
              value={tableRowsText}
              placeholder={'成都\t1200\n重庆\t800'}
              onChange={(e) =>
                onUpdate({
                  rows: e.target.value
                    .split('\n')
                    .filter((line) => line.trim() !== '')
                    .map((line) => line.split('\t')),
                } as Partial<Block>)
              }
            />
          </div>
          <div>
            <label style={labelStyle}>{t('notebook.caption')}</label>
            <input
              style={inputStyle}
              value={block.caption || ''}
              onChange={(e) => onUpdate({ caption: e.target.value } as Partial<Block>)}
            />
          </div>
        </>
      )}

      {type === 'chart' && (
        <>
          <div style={{ display: 'flex', gap: SPACING.sm }}>
            <div style={{ flex: '0 0 110px' }}>
              <label style={labelStyle}>{t('notebook.chart_type')}</label>
              <select
                style={{ ...inputStyle, cursor: 'pointer', appearance: 'auto' }}
                value={block.chart_type}
                onChange={(e) => onUpdate({ chart_type: e.target.value } as Partial<Block>)}
              >
                <option value="bar">柱状图</option>
                <option value="line">折线图</option>
                <option value="pie">饼图</option>
              </select>
            </div>
            <div style={{ flex: 1 }}>
              <label style={labelStyle}>{t('notebook.chart_title')}</label>
              <input
                style={inputStyle}
                value={block.title || ''}
                onChange={(e) => onUpdate({ title: e.target.value } as Partial<Block>)}
              />
            </div>
          </div>
          <div>
            <label style={labelStyle}>
              {t('notebook.chart_categories')}{' '}
              <span style={{ fontWeight: 400, color: COLORS.textTertiary }}>{t('notebook.items_hint')}</span>
            </label>
            <textarea
              style={textareaStyle}
              value={chartCatsText}
              placeholder={'季度1\n季度2\n季度3'}
              onChange={(e) =>
                onUpdate({
                  categories: e.target.value.split('\n').filter(Boolean),
                } as Partial<Block>)
              }
            />
          </div>
          <div>
            <label style={labelStyle}>
              {t('notebook.chart_series')}{' '}
              <span style={{ fontWeight: 400, color: COLORS.textTertiary }}>{t('notebook.chart_series_hint')}</span>
            </label>
            <textarea
              style={{ ...textareaStyle, minHeight: 90, fontFamily: TYPO.fontMono, fontSize: 12 }}
              value={chartSeriesText}
              placeholder={'销售额\t120,180,240'}
              onChange={(e) =>
                onUpdate({
                  series: e.target.value
                    .split('\n')
                    .filter((line) => line.trim() !== '')
                    .map((line) => {
                      const [name, dataStr] = line.split('\t');
                      const data = (dataStr || '')
                        .split(',')
                        .map((v) => Number(v.trim()))
                        .filter((n) => !Number.isNaN(n));
                      return { name: name || '系列', data };
                    }),
                } as Partial<Block>)
              }
            />
          </div>
        </>
      )}

      {type === 'mermaid' && (
        <>
          <div>
            <label style={labelStyle}>{t('notebook.mermaid_code')}</label>
            <textarea
              style={{ ...textareaStyle, minHeight: 140, fontFamily: TYPO.fontMono, fontSize: 12 }}
              value={block.code}
              placeholder={'graph TD\n  A[开始] --> B[过程]\n  B --> C[结束]'}
              onChange={(e) => onUpdate({ code: e.target.value } as Partial<Block>)}
            />
          </div>
          <div>
            <label style={labelStyle}>{t('notebook.caption')}</label>
            <input
              style={inputStyle}
              value={block.caption || ''}
              onChange={(e) => onUpdate({ caption: e.target.value } as Partial<Block>)}
            />
          </div>
        </>
      )}

      {type === 'custom' && (
        <div>
          <label style={labelStyle}>{t('notebook.html')}</label>
          <textarea
            style={{ ...textareaStyle, minHeight: 100, fontFamily: TYPO.fontMono, fontSize: 12 }}
            value={block.html}
            onChange={(e) => onUpdate({ html: e.target.value } as Partial<Block>)}
          />
        </div>
      )}
    </div>
  );
};

// ============================================================
// 辅助组件/样式
// ============================================================

function pillBtn(primary: boolean): React.CSSProperties {
  return {
    display: 'inline-flex',
    alignItems: 'center',
    gap: 4,
    padding: '6px 14px',
    border: 'none',
    borderRadius: primary ? '3px 14px 3px 14px' : '3px 10px 3px 10px',
    background: primary ? COLORS.accent : COLORS.bgHover,
    color: primary ? '#fff' : COLORS.textSecondary,
    fontSize: 13,
    fontWeight: 600,
    fontFamily: TYPO.fontFamily,
    transition: `all ${DURATION.fast}s ${EASE.swift}`,
  };
}

// 胶囊交互按钮：统一 hover / 按压反馈
const ActionButton: React.FC<{
  primary?: boolean;
  disabled?: boolean;
  onClick: () => void;
  children: React.ReactNode;
  style?: React.CSSProperties;
}> = ({ primary, disabled, onClick, children, style }) => {
  const [hovered, setHovered] = useState(false);
  const [pressed, setPressed] = useState(false);
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => {
        setHovered(false);
        setPressed(false);
      }}
      onMouseDown={() => setPressed(true)}
      onMouseUp={() => setPressed(false)}
      style={{
        ...pillBtn(Boolean(primary)),
        cursor: disabled ? 'not-allowed' : 'pointer',
        opacity: disabled ? 0.5 : 1,
        background: primary
          ? hovered
            ? COLORS.accentBright
            : COLORS.accent
          : hovered
            ? COLORS.accentMuted
            : COLORS.bgHover,
        color: primary ? '#fff' : COLORS.textSecondary,
        transform: pressed && !disabled ? 'scale(0.95)' : 'scale(1)',
        ...style,
      }}
    >
      {children}
    </button>
  );
};

const IconBtn: React.FC<{
  children: React.ReactNode;
  onClick: () => void;
  title: string;
  disabled?: boolean;
  danger?: boolean;
}> = ({ children, onClick, title, disabled, danger }) => (
  <button
    onClick={onClick}
    title={title}
    disabled={disabled}
    style={{
      border: 'none',
      background: 'transparent',
      color: disabled
        ? COLORS.textTertiary
        : danger
          ? COLORS.danger
          : COLORS.textSecondary,
      cursor: disabled ? 'not-allowed' : 'pointer',
      padding: 4,
      borderRadius: RADIUS.xs,
      display: 'inline-flex',
      alignItems: 'center',
      justifyContent: 'center',
      opacity: disabled ? 0.4 : 1,
      transition: `background ${DURATION.fast}s ${EASE.swift}`,
    }}
    onMouseEnter={(e) => {
      if (!disabled) e.currentTarget.style.background = COLORS.bgActive;
    }}
    onMouseLeave={(e) => {
      e.currentTarget.style.background = 'transparent';
    }}
  >
    {children}
  </button>
);

export default NotebookPage;
