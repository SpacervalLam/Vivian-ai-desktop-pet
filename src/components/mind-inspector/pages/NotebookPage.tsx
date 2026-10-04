/**
 * Notebook 页 — 笔记索引 + 预览 / 编辑
 *
 * 数据源：invoke('list_notebooks') / invoke('get_notebook_html') / invoke('get_notebook_detail')
 * 写入：invoke('create_notebook') / invoke('update_notebook')
 * 刷新：监听 notebook:created / notebook:updated / notebook:deleted 事件
 *
 * 布局（编辑型）：左索引（紧凑行：标题 + 标签 + 时间）/ 右工作面（顶栏 + 预览或编辑）。
 * 索引行不做卡片、不做倾斜，靠行距与一条选中色条区分。
 * 版式刻度与共用控件走 RecordTheme.css，本文件只管这页的形状。
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { listen, emit, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { getCurrentWindow } from '@tauri-apps/api/window';
import {
  Check,
  ChevronDown,
  ChevronUp,
  FileText,
  LayoutTemplate,
  ListChecks,
  MousePointerClick,
  NotebookPen,
  Palette,
  Pencil,
  Plus,
  Tag,
  Trash2,
  Upload,
  X,
} from 'lucide-react';
import { useNavigation } from '../NavigationContext';
import {
  Block,
  Cover,
  NoteBook,
  CharacterId,
  BlockType,
} from './notebook-types';
import { WysiwygEditor } from './NoteWysiwyg';
import './RecordTheme.css';

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

const CHARACTERS: CharacterId[] = ['vivian', 'nana'];

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
// 共用片段
// ============================================================

const Blank: React.FC<{
  icon: React.ElementType;
  title: string;
  hint?: string;
  children?: React.ReactNode;
}> = ({ icon: Icon, title, hint, children }) => (
  <div className="rec-blank">
    <span className="rec-blank-icon">
      <Icon size={30} strokeWidth={1.2} />
    </span>
    <span className="rec-blank-title">{title}</span>
    {hint ? <span className="rec-blank-hint">{hint}</span> : null}
    {children}
  </div>
);

const CastSwitch: React.FC<{
  value: CharacterId;
  onChange: (c: CharacterId) => void;
}> = ({ value, onChange }) => (
  <div className="rec-cast" role="group">
    {CHARACTERS.map((id) => (
      <button
        key={id}
        type="button"
        onClick={() => onChange(id)}
        className={`rec-cast-item${value === id ? ' is-active' : ''}`}
        aria-pressed={value === id}
      >
        <span className="rec-cast-dot" />
        {CHAR_LABEL[id]}
      </button>
    ))}
  </div>
);

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

  // 供事件回调读取「当前选中的笔记 id」。
  // 不把它放进订阅依赖：否则每次点选笔记都会 cleanup + 重订阅，
  // 而重订阅存在「cleanup 早于 listen resolve」的竞态窗口，会漏掉解绑。
  const selectedIdRef = useRef<string | null>(null);
  useEffect(() => {
    selectedIdRef.current = selectedId;
  }, [selectedId]);

  // 监听笔记事件自动刷新
  useEffect(() => {
    let cancelled = false;
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
        const sid = selectedIdRef.current;
        if (e.payload?.char_id === character && sid) {
          void loadNoteHtml(character, sid);
        }
      });
      const u3 = await listen<{ char_id: string; note_id: string }>('notebook:deleted', (e) => {
        refresh(e.payload?.char_id);
        if (e.payload?.note_id === selectedIdRef.current) {
          setSelectedId(null);
          setHtml('');
        }
      });
      // cleanup 可能已在本轮 await 期间跑过（StrictMode 双挂载 / character 切换）：
      // 此时 unlistens 数组已无人读取，必须立即解绑，否则这 3 个订阅永久泄漏，
      // 残留的 notebook:updated 会反复触发 loadNotes/loadNoteHtml，形成 N 倍 IPC。
      if (cancelled) {
        u1();
        u2();
        u3();
        return;
      }
      unlistens.push(u1, u2, u3);
    })();

    return () => {
      cancelled = true;
      unlistens.forEach((u) => u());
    };
  }, [character, loadNotes, loadNoteHtml]);

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
    <div className="record-note">
      {/* === 左：笔记索引 === */}
      <div className="record-note-index">
        <div className="record-note-index-head">
          <CastSwitch
            value={character}
            onChange={(id) => {
              setCharacter(id);
              setSelectedId(null);
              setHtml('');
            }}
          />
          <span className="record-note-index-head-spacer" />
          <button
            type="button"
            onClick={startNew}
            disabled={editing}
            title={t('notebook.new_note')}
            className="rec-icon-btn is-accent"
          >
            <Plus size={16} />
          </button>
          <button
            type="button"
            onClick={() => void handlePickHtml()}
            disabled={editing}
            title={t('notebook.import_html', { defaultValue: '导入 HTML 文件' })}
            className="rec-icon-btn"
          >
            <Upload size={15} />
          </button>
        </div>

        <div className="record-note-index-scroll">
          {loading && notes.length === 0 ? (
            <Blank icon={NotebookPen} title={t('common.loading')} />
          ) : notes.length === 0 ? (
            <Blank
              icon={NotebookPen}
              title={t('notebook.empty_hint', { name: t(`mind_inspector.common.char_${character}`) })}
            />
          ) : (
            notes.map((note) => (
              <NoteRow
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

      {/* === 右：工作面 === */}
      <div className="record-note-work">
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
            onSave={() => void saveEdit()}
            onCancel={cancelEdit}
          />
        ) : selectedNote ? (
          <>
            <div className="record-note-work-head">
              <div className="record-note-work-title">
                <FileText size={15} />
                <span className="record-note-work-title-text">{selectedNote.title}</span>
              </div>
              <div className="rec-meta">
                <span>{formatTime(selectedNote.updated_at)}</span>
                <span>
                  {selectedNote.render_type === 'raw_html'
                    ? 'HTML'
                    : `${selectedNote.block_count} ${t('notebook.blocks')}`}
                </span>
              </div>
              {selectedNote.render_type === 'raw_html' ? (
                <span className="rec-tag">{t('notebook.readonly', { defaultValue: '只读' })}</span>
              ) : (
                <button
                  type="button"
                  onClick={() => void startEdit(selectedNote.id)}
                  title={t('notebook.edit')}
                  className="rec-btn"
                >
                  <Pencil size={13} /> {t('notebook.edit')}
                </button>
              )}
            </div>
            {/* 笔记预览（iframe src = asset URL 渲染完整文档，跨源隔离） */}
            <div className="record-note-work-body">
              {loadingHtml && (
                <div
                  style={{
                    position: 'absolute',
                    inset: 0,
                    zIndex: 10,
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    gap: 8,
                    background: 'var(--claude-surface)',
                    color: 'var(--rec-muted)',
                    fontSize: 'var(--rec-fs-body)',
                  }}
                >
                  {t('common.loading')}
                </div>
              )}
              <iframe
                ref={iframeRef}
                title="notebook-preview"
                src={html}
                sandbox="allow-same-origin"
                className="record-note-frame"
              />
            </div>
          </>
        ) : (
          <div className="record-note-work-body">
            <Blank
              icon={NotebookPen}
              title={t('notebook.select_hint')}
              hint={t('notebook.import_drop_hint', { defaultValue: '或将 .html 文件拖入此窗口' })}
            >
              <button type="button" onClick={() => void handlePickHtml()} className="rec-btn">
                <Upload size={14} />
                {t('notebook.import_html', { defaultValue: '导入 HTML 文件' })}
              </button>
            </Blank>
          </div>
        )}
      </div>
    </div>
  );
};

// ============================================================
// NoteRow — 索引行
// ============================================================

const NoteRow: React.FC<{
  note: NoteSummary;
  active: boolean;
  onClick: () => void;
  onDelete: (e: React.MouseEvent) => void;
}> = ({ note, active, onClick, onDelete }) => {
  const { t } = useTranslation();
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onClick();
        }
      }}
      className={`record-note-row${active ? ' is-active' : ''}`}
    >
      <span className="record-note-row-title">{note.title || '无标题'}</span>
      {note.tags.length > 0 && (
        <span className="record-note-row-tags">
          {note.tags.slice(0, 3).map((tag, i) => (
            <span key={i} className={`rec-tag${active ? ' is-accent' : ''}`}>
              {tag}
            </span>
          ))}
          {note.tags.length > 3 && (
            <span style={{ color: 'var(--rec-faint)', fontSize: 'var(--rec-fs-micro)' }}>
              +{note.tags.length - 3}
            </span>
          )}
        </span>
      )}
      <span className="record-note-row-foot">
        <span>{formatTime(note.updated_at)}</span>
        <span className="record-note-row-foot-spacer" />
        <button
          type="button"
          onClick={onDelete}
          title={t('notebook.delete_block')}
          className="rec-icon-btn is-danger record-note-row-del"
          style={{ width: 22, height: 22 }}
        >
          <Trash2 size={13} />
        </button>
      </span>
    </div>
  );
};

// ============================================================
// NoteEditor — 编辑态
// ============================================================

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
  dirty,
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
      <div style={{ flex: 1, display: 'flex', alignItems: 'center', justifyContent: 'center', color: 'var(--rec-muted)' }}>
        {t('common.loading')}
      </div>
    );
  }

  return (
    <div style={{ flex: 1, display: 'flex', flexDirection: 'column', minHeight: 0 }}>
      {/* 编辑顶栏 */}
      <div className="record-note-work-head">
        <div className="record-note-work-title">
          <Pencil size={15} />
          <span className="record-note-work-title-text">
            {isNew ? t('notebook.new_note') : t('notebook.edit')}
          </span>
          {dirty && (
            <span className="record-note-dirty">
              <span className="rec-dot" />
              {t('notebook.unsaved', { defaultValue: '未保存' })}
            </span>
          )}
        </div>

        <div className="rec-seg">
          <button
            type="button"
            onClick={() => setMode('wysiwyg')}
            title={t('notebook.mode_wysiwyg')}
            className={`rec-seg-item${mode === 'wysiwyg' ? ' is-active' : ''}`}
          >
            <MousePointerClick size={13} /> {t('notebook.mode_wysiwyg')}
          </button>
          <button
            type="button"
            onClick={() => setMode('form')}
            title={t('notebook.mode_form')}
            className={`rec-seg-item${mode === 'form' ? ' is-active' : ''}`}
          >
            <ListChecks size={13} /> {t('notebook.mode_form')}
          </button>
        </div>

        <button type="button" onClick={onCancel} disabled={saving} className="rec-btn">
          <X size={13} /> {t('common.cancel')}
        </button>
        <button type="button" onClick={onSave} disabled={saving} className="rec-btn is-primary">
          <Check size={13} /> {saving ? t('common.saving') : t('common.save')}
        </button>
      </div>

      {/* 编辑区 */}
      <div className="record-note-work-body">
        <div className="record-note-blocks">
          <div className="rec-field">
            <label className="rec-label">{t('notebook.title')}</label>
            <input
              className="rec-input"
              style={{ fontSize: 17, fontWeight: 500 }}
              value={draft.title}
              placeholder={t('notebook.title')}
              onChange={(e) => onPatch((d) => ({ ...d, title: e.target.value }))}
            />
          </div>

          <div className="rec-field">
            <label className="rec-label">
              <span style={{ display: 'inline-flex', gap: 6, alignItems: 'center' }}>
                <Palette size={13} /> {t('notebook.palette')}
              </span>
            </label>
            <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
              {PALETTE_KEYS.map((key) => (
                <button
                  key={key}
                  type="button"
                  onClick={() => onPatch((d) => ({ ...d, palette: key }))}
                  title={key}
                  aria-label={key}
                  aria-pressed={draft.palette === key}
                  style={{
                    width: 30,
                    height: 30,
                    borderRadius: 999,
                    border: draft.palette === key ? '2px solid var(--rec-accent)' : '2px solid transparent',
                    background: PALETTE_COLORS[key],
                    cursor: 'pointer',
                    padding: 0,
                    boxShadow: '0 0 0 0.5px var(--claude-line)',
                    transition: 'border-color 0.16s cubic-bezier(0.2,0.8,0.2,1)',
                  }}
                />
              ))}
            </div>
          </div>

          <div className="rec-field">
            <label className="rec-label">
              <span style={{ display: 'inline-flex', gap: 6, alignItems: 'center' }}>
                <LayoutTemplate size={13} /> {t('notebook.layout')}
              </span>
            </label>
            <div className="rec-seg" style={{ alignSelf: 'flex-start' }}>
              {LAYOUT_OPTIONS.map((opt) => (
                <button
                  key={opt.value}
                  type="button"
                  onClick={() => onPatch((d) => ({ ...d, layout: opt.value }))}
                  className={`rec-seg-item${draft.layout === opt.value ? ' is-active' : ''}`}
                >
                  {layoutLabel(opt.value, lang)}
                </button>
              ))}
            </div>
          </div>

          <div className="rec-field">
            <label className="rec-label">
              <span style={{ display: 'inline-flex', gap: 6, alignItems: 'center' }}>
                <Tag size={13} /> {t('notebook.tags')}
                <span className="rec-label-hint">({t('notebook.items_hint')})</span>
              </span>
            </label>
            <input
              className="rec-input"
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
            <div className="rec-field">
              <label className="rec-label">
                <span style={{ display: 'inline-flex', gap: 6, alignItems: 'center' }}>
                  <FileText size={13} /> {t('notebook.cover')}
                </span>
              </label>
              <div
                style={{
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 'var(--rec-gap-3)',
                  padding: 'var(--rec-gap-4)',
                  border: 'var(--rec-line)',
                  borderRadius: 'var(--rec-radius)',
                  background: 'var(--rec-sunken)',
                }}
              >
                <div style={{ display: 'flex', justifyContent: 'flex-end' }}>
                  <button
                    type="button"
                    onClick={() =>
                      onPatch((d) => ({
                        ...d,
                        cover: d.cover
                          ? null
                          : { title: d.title, subtitle: '', emoji: '', background: '' },
                      }))
                    }
                    className="rec-btn is-ghost"
                  >
                    {cover ? t('common.delete', { defaultValue: '删除' }) : t('common.add', { defaultValue: '添加' })}
                  </button>
                </div>
                {cover && (
                  <>
                    <div className="rec-field">
                      <label className="rec-label">{t('notebook.cover_title')}</label>
                      <input
                        className="rec-input"
                        value={cover.title}
                        onChange={(e) =>
                          onPatch((d) => ({ ...d, cover: { ...d.cover!, title: e.target.value } }))
                        }
                      />
                    </div>
                    <div className="rec-field">
                      <label className="rec-label">{t('notebook.cover_subtitle')}</label>
                      <input
                        className="rec-input"
                        value={cover.subtitle || ''}
                        onChange={(e) =>
                          onPatch((d) => ({ ...d, cover: { ...d.cover!, subtitle: e.target.value } }))
                        }
                      />
                    </div>
                    <div className="record-note-block-row">
                      <div style={{ flex: '0 0 120px' }}>
                        <label className="rec-label">{t('notebook.cover_emoji')}</label>
                        <input
                          className="rec-input"
                          value={cover.emoji || ''}
                          onChange={(e) =>
                            onPatch((d) => ({ ...d, cover: { ...d.cover!, emoji: e.target.value } }))
                          }
                        />
                      </div>
                      <div style={{ flex: 1 }}>
                        <label className="rec-label">{t('notebook.cover_bg')}</label>
                        <input
                          className="rec-input"
                          value={cover.background || ''}
                          placeholder="#FF6B6B / linear-gradient(...)"
                          onChange={(e) =>
                            onPatch((d) => ({ ...d, cover: { ...d.cover!, background: e.target.value } }))
                          }
                        />
                      </div>
                    </div>
                  </>
                )}
              </div>
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
            <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--rec-gap-3)' }}>
              <div className="rec-label">
                <span style={{ display: 'inline-flex', gap: 6, alignItems: 'center' }}>
                  <ListChecks size={13} /> {t('notebook.blocks')}
                  <span className="rec-label-hint">({draft.blocks.length})</span>
                </span>
              </div>
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
                <div
                  style={{
                    padding: 'var(--rec-gap-5)',
                    textAlign: 'center',
                    border: 'var(--rec-line)',
                    borderRadius: 'var(--rec-radius)',
                    color: 'var(--rec-faint)',
                    fontSize: 'var(--rec-fs-meta)',
                  }}
                >
                  {t('notebook.add_block')}
                </div>
              )}
            </div>
          )}

          {/* 添加块 */}
          <div style={{ display: 'flex', gap: 'var(--rec-gap-2)', paddingTop: 'var(--rec-gap-2)' }}>
            <select
              value={addType}
              onChange={(e) => setAddType(e.target.value as BlockType)}
              className="rec-select"
              style={{ flex: 1 }}
            >
              {BLOCK_TYPES.map((bt) => (
                <option key={bt} value={bt}>
                  {bt}
                </option>
              ))}
            </select>
            <button type="button" onClick={() => onAddBlock(addType)} className="rec-btn is-primary">
              <Plus size={14} /> {t('notebook.add_block')}
            </button>
          </div>

          {/* 错误提示 */}
          {error && (
            <div
              style={{
                padding: 'var(--rec-gap-3) var(--rec-gap-4)',
                border: '0.5px solid color-mix(in srgb, #b4553f 34%, transparent)',
                borderRadius: 'var(--rec-radius-sm)',
                background: 'color-mix(in srgb, #b4553f 9%, transparent)',
                color: '#b4553f',
                fontSize: 'var(--rec-fs-meta)',
              }}
            >
              {error}
            </div>
          )}
        </div>
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
  const { type } = block;

  // 各类型字段的文本化取值。Block 是判别联合，字段挂在具体成员上，
  // 所以这里按 type 收窄后取，而不是在联合上直接读（那会编译不过）。
  const itemsText = type === 'list' || type === 'tags' ? (block.items || []).join('\n') : '';
  const tagsValue = type === 'tags' ? (block.items || []).join(', ') : '';
  const tableHeadersText = type === 'table' ? (block.headers || []).join('\n') : '';
  const tableRowsText =
    type === 'table' ? (block.rows || []).map((r: string[]) => r.join('\t')).join('\n') : '';
  const chartCatsText = type === 'chart' ? (block.categories || []).join('\n') : '';
  const chartSeriesText =
    type === 'chart'
      ? (block.series || [])
          .map((s: { name: string; data: number[] }) => `${s.name}\t${s.data.join(',')}`)
          .join('\n')
      : '';

  return (
    <div className="record-note-block">
      <div className="record-note-block-head">
        <span className="rec-tag is-accent">{type}</span>
        <span className="record-note-block-head-spacer" />
        <button
          type="button"
          disabled={index === 0}
          title={t('notebook.move_up')}
          onClick={() => onMove(-1)}
          className="rec-icon-btn"
        >
          <ChevronUp size={14} />
        </button>
        <button
          type="button"
          disabled={index === total - 1}
          title={t('notebook.move_down')}
          onClick={() => onMove(1)}
          className="rec-icon-btn"
        >
          <ChevronDown size={14} />
        </button>
        <button
          type="button"
          title={t('notebook.delete_block')}
          onClick={onRemove}
          className="rec-icon-btn is-danger"
        >
          <Trash2 size={14} />
        </button>
      </div>

      <div className="record-note-block-fields">
        {type === 'heading' && (
          <div className="record-note-block-row">
            <div style={{ flex: '0 0 90px' }}>
              <label className="rec-label">{t('notebook.level')}</label>
              <select
                className="rec-select"
                value={block.level}
                onChange={(e) => onUpdate({ level: Number(e.target.value) } as Partial<Block>)}
              >
                <option value={1}>H1</option>
                <option value={2}>H2</option>
                <option value={3}>H3</option>
              </select>
            </div>
            <div style={{ flex: 1 }}>
              <label className="rec-label">{t('notebook.text')}</label>
              <input
                className="rec-input"
                value={block.text}
                onChange={(e) => onUpdate({ text: e.target.value } as Partial<Block>)}
              />
            </div>
          </div>
        )}

        {type === 'paragraph' && (
          <div className="rec-field">
            <label className="rec-label">{t('notebook.text')}</label>
            <textarea
              className="rec-textarea"
              value={block.text}
              onChange={(e) => onUpdate({ text: e.target.value } as Partial<Block>)}
            />
          </div>
        )}

        {type === 'card' && (
          <>
            <div className="rec-field">
              <label className="rec-label">{t('notebook.cover_title')}</label>
              <input
                className="rec-input"
                value={block.title || ''}
                onChange={(e) => onUpdate({ title: e.target.value } as Partial<Block>)}
              />
            </div>
            <div className="rec-field">
              <label className="rec-label">{t('notebook.body')}</label>
              <textarea
                className="rec-textarea"
                value={block.body}
                onChange={(e) => onUpdate({ body: e.target.value } as Partial<Block>)}
              />
            </div>
            <div style={{ flex: '0 0 120px' }}>
              <label className="rec-label">{t('notebook.emoji')}</label>
              <input
                className="rec-input"
                value={block.emoji || ''}
                onChange={(e) => onUpdate({ emoji: e.target.value } as Partial<Block>)}
              />
            </div>
          </>
        )}

        {type === 'quote' && (
          <>
            <div className="rec-field">
              <label className="rec-label">{t('notebook.text')}</label>
              <textarea
                className="rec-textarea"
                value={block.text}
                onChange={(e) => onUpdate({ text: e.target.value } as Partial<Block>)}
              />
            </div>
            <div className="rec-field">
              <label className="rec-label">{t('notebook.author')}</label>
              <input
                className="rec-input"
                value={block.author || ''}
                onChange={(e) => onUpdate({ author: e.target.value } as Partial<Block>)}
              />
            </div>
          </>
        )}

        {type === 'list' && (
          <>
            <label className="record-note-block-check">
              <input
                type="checkbox"
                checked={block.ordered || false}
                onChange={(e) => onUpdate({ ordered: e.target.checked } as Partial<Block>)}
              />
              {t('notebook.ordered')}
            </label>
            <div className="rec-field">
              <label className="rec-label">
                {t('notebook.text')}{' '}
                <span className="rec-label-hint">{t('notebook.items_hint')}</span>
              </label>
              <textarea
                className="rec-textarea"
                value={itemsText}
                onChange={(e) =>
                  onUpdate({ items: e.target.value.split('\n') } as Partial<Block>)
                }
              />
            </div>
          </>
        )}

        {type === 'tags' && (
          <div className="rec-field">
            <label className="rec-label">
              {t('notebook.tags')}{' '}
              <span className="rec-label-hint">{t('notebook.items_hint')}</span>
            </label>
            <textarea
              className="rec-textarea"
              value={tagsValue}
              onChange={(e) =>
                onUpdate({
                  items: e.target.value.split(',').map((s) => s.trim()).filter(Boolean),
                } as Partial<Block>)
              }
            />
          </div>
        )}

        {type === 'image' && (
          <>
            <div className="rec-field">
              <label className="rec-label">{t('notebook.url')}</label>
              <input
                className="rec-input"
                value={block.url}
                onChange={(e) => onUpdate({ url: e.target.value } as Partial<Block>)}
              />
            </div>
            <div className="rec-field">
              <label className="rec-label">{t('notebook.caption')}</label>
              <input
                className="rec-input"
                value={block.caption || ''}
                onChange={(e) => onUpdate({ caption: e.target.value } as Partial<Block>)}
              />
            </div>
          </>
        )}

        {type === 'divider' && (
          <div style={{ flex: '0 0 120px' }}>
            <label className="rec-label">{t('notebook.emoji')}</label>
            <input
              className="rec-input"
              value={block.emoji || ''}
              onChange={(e) => onUpdate({ emoji: e.target.value } as Partial<Block>)}
            />
          </div>
        )}

        {type === 'callout' && (
          <>
            <div className="rec-field">
              <label className="rec-label">{t('notebook.text')}</label>
              <textarea
                className="rec-textarea"
                value={block.text}
                onChange={(e) => onUpdate({ text: e.target.value } as Partial<Block>)}
              />
            </div>
            <div style={{ flex: '0 0 120px' }}>
              <label className="rec-label">{t('notebook.emoji')}</label>
              <input
                className="rec-input"
                value={block.emoji || ''}
                onChange={(e) => onUpdate({ emoji: e.target.value } as Partial<Block>)}
              />
            </div>
          </>
        )}

        {type === 'table' && (
          <>
            <div className="rec-field">
              <label className="rec-label">
                {t('notebook.table_headers')}{' '}
                <span className="rec-label-hint">{t('notebook.items_hint')}</span>
              </label>
              <textarea
                className="rec-textarea"
                value={tableHeadersText}
                placeholder={'城市\t人均预算'}
                onChange={(e) =>
                  onUpdate({
                    headers: e.target.value.split('\n').filter(Boolean),
                  } as Partial<Block>)
                }
              />
            </div>
            <div className="rec-field">
              <label className="rec-label">
                {t('notebook.table_rows')}{' '}
                <span className="rec-label-hint">{t('notebook.table_rows_hint')}</span>
              </label>
              <textarea
                className="rec-textarea"
                style={{ minHeight: 100, fontFamily: 'var(--claude-sans)', fontSize: 12 }}
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
            <div className="rec-field">
              <label className="rec-label">{t('notebook.caption')}</label>
              <input
                className="rec-input"
                value={block.caption || ''}
                onChange={(e) => onUpdate({ caption: e.target.value } as Partial<Block>)}
              />
            </div>
          </>
        )}

        {type === 'chart' && (
          <>
            <div className="record-note-block-row">
              <div style={{ flex: '0 0 110px' }}>
                <label className="rec-label">{t('notebook.chart_type')}</label>
                <select
                  className="rec-select"
                  value={block.chart_type}
                  onChange={(e) => onUpdate({ chart_type: e.target.value } as Partial<Block>)}
                >
                  <option value="bar">柱状图</option>
                  <option value="line">折线图</option>
                  <option value="pie">饼图</option>
                </select>
              </div>
              <div style={{ flex: 1 }}>
                <label className="rec-label">{t('notebook.chart_title')}</label>
                <input
                  className="rec-input"
                  value={block.title || ''}
                  onChange={(e) => onUpdate({ title: e.target.value } as Partial<Block>)}
                />
              </div>
            </div>
            <div className="rec-field">
              <label className="rec-label">
                {t('notebook.chart_categories')}{' '}
                <span className="rec-label-hint">{t('notebook.items_hint')}</span>
              </label>
              <textarea
                className="rec-textarea"
                value={chartCatsText}
                placeholder={'季度1\n季度2\n季度3'}
                onChange={(e) =>
                  onUpdate({
                    categories: e.target.value.split('\n').filter(Boolean),
                  } as Partial<Block>)
                }
              />
            </div>
            <div className="rec-field">
              <label className="rec-label">
                {t('notebook.chart_series')}{' '}
                <span className="rec-label-hint">{t('notebook.chart_series_hint')}</span>
              </label>
              <textarea
                className="rec-textarea"
                style={{ minHeight: 90, fontFamily: 'var(--claude-sans)', fontSize: 12 }}
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
            <div className="rec-field">
              <label className="rec-label">{t('notebook.mermaid_code')}</label>
              <textarea
                className="rec-textarea"
                style={{ minHeight: 140, fontFamily: 'var(--claude-sans)', fontSize: 12 }}
                value={block.code}
                placeholder={'graph TD\n  A[开始] --> B[过程]\n  B --> C[结束]'}
                onChange={(e) => onUpdate({ code: e.target.value } as Partial<Block>)}
              />
            </div>
            <div className="rec-field">
              <label className="rec-label">{t('notebook.caption')}</label>
              <input
                className="rec-input"
                value={block.caption || ''}
                onChange={(e) => onUpdate({ caption: e.target.value } as Partial<Block>)}
              />
            </div>
          </>
        )}

        {type === 'custom' && (
          <div className="rec-field">
            <label className="rec-label">{t('notebook.html')}</label>
            <textarea
              className="rec-textarea"
              style={{ minHeight: 100, fontFamily: 'var(--claude-sans)', fontSize: 12 }}
              value={block.html}
              onChange={(e) => onUpdate({ html: e.target.value } as Partial<Block>)}
            />
          </div>
        )}
      </div>
    </div>
  );
};

export default NotebookPage;
