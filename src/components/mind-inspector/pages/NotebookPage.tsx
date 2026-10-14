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
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { listen, emit, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { getCurrentWindow } from '@tauri-apps/api/window';
import {
  Check,
  Code,
  FileText,
  LayoutTemplate,
  NotebookPen,
  Maximize,
  Minimize,
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
  NoteBook,
  CharacterId,
} from './notebook-types';

import { NotebookPageEditor } from './NotebookPageEditor';
import './RecordTheme.css';
import { comicText } from '../../../i18n/comic';
import { ComicChapterHeading, ComicChapterInset } from './ComicChapter';

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
  warm: 'linear-gradient(135deg, #9C4F35, #D5B79B)',
  fresh: 'linear-gradient(135deg, #277268, #A3C4AF)',
  elegant: 'linear-gradient(135deg, #655477, #BEAEC9)',
  cute: 'linear-gradient(135deg, #A4546C, #E1B5BF)',
  cool: 'linear-gradient(135deg, #3F6593, #AAC1D8)',
  nature: 'linear-gradient(135deg, #596D44, #B7C49A)',
};

const PALETTE_KEYS = Object.keys(PALETTE_COLORS);

/** 开启自定义 CSS 时写入的起步样式：先给一组变量示范怎么改，而不是空白页 */
const CSS_PRESET_PLACEHOLDER = `:root {
    --accent: #4A7C9B;
    --ink: #1C2024;
    --rule: #CFD8DC;
    --accent-grad: linear-gradient(135deg, #31576B, #5E8FA3);
}
.heading { margin-top: 40px; }`;

const CSS_PLACEHOLDER = `:root {
    /* 主强调色 / 正文墨色 / 分隔线 / 封面渐变 */
    --accent: #4A7C9B;
    --ink: #1C2024;
    --rule: #CFD8DC;
    --accent-grad: linear-gradient(135deg, #31576B, #5E8FA3);
}

/* 可覆盖：.container .cover .cover-title .cover-subtitle .card .card-title
   .card-body .heading .heading-1/2/3 .paragraph .quote .list .list-item
   .callout .tag .nb-table .chart .mermaid .footer .divider */`;

const LAYOUT_OPTIONS: { value: string; labelKey: string }[] = [
  { value: 'cover_flow', labelKey: 'cover_flow' },
  { value: 'article', labelKey: 'article' },
  { value: 'gallery', labelKey: 'gallery' },
  { value: 'simple', labelKey: 'simple' },
];

const LAYOUT_LABELS: Record<string, Record<string, string>> = {
  'zh-CN': { cover_flow: '封面手册', article: '阅读文章', gallery: '图文画册', simple: '轻量短笺' },
  en: { cover_flow: 'Handbook', article: 'Article', gallery: 'Journal', simple: 'Short Note' },
  ja: { cover_flow: 'ハンドブック', article: '読みもの', gallery: '画帳', simple: '短いメモ' },
};

// ============================================================
// 工具函数
// ============================================================

function formatTime(ts: number, language: string): string {
  const d = new Date(ts * 1000);
  const now = new Date();
  const diff = now.getTime() - d.getTime();
  const mins = Math.floor(diff / 60000);
  const hours = Math.floor(diff / 3600000);
  const days = Math.floor(diff / 86400000);

  if (mins < 1) return comicText('justNow', language);
  if (mins < 60) return new Intl.RelativeTimeFormat(language, { numeric: 'always' }).format(-mins, 'minute');
  if (hours < 24) return new Intl.RelativeTimeFormat(language, { numeric: 'always' }).format(-hours, 'hour');
  if (days < 7) return new Intl.RelativeTimeFormat(language, { numeric: 'always' }).format(-days, 'day');
  return d.toLocaleDateString(language, { month: 'short', day: 'numeric' });
}

function layoutLabel(value: string, lang: string): string {
  const table = LAYOUT_LABELS[lang] || LAYOUT_LABELS['zh-CN'];
  return table[value] || value;
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
    layout: 'article',
    palette: 'warm',
    cover: null,
    blocks: [{ type: 'paragraph', text: '' }],
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

// ============================================================
// NotebookPage 主组件
// ============================================================

const CHARACTER_IDS: CharacterId[] = ['vivian', 'nana'];

const NotebookPage: React.FC<{ embedded?: boolean; compact?: boolean }> = ({ embedded = false, compact = false }) => {
  const { t, i18n } = useTranslation();
  const nav = useNavigation();
  const [character, setCharacter] = useState<CharacterId>('vivian');
  const [notes, setNotes] = useState<NoteSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // 标签筛选：单选，再次点击同一标签取消。null = 不筛选。
  const [activeTag, setActiveTag] = useState<string | null>(null);
  const [html, setHtml] = useState<string>('');
  const [loading, setLoading] = useState(false);
  const [loadingHtml, setLoadingHtml] = useState(false);
  const [readingFullscreen, setReadingFullscreen] = useState(false);
  const readingRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!readingFullscreen) return;
    const previousFocus = document.activeElement as HTMLElement | null;
    readingRef.current?.focus();
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); setReadingFullscreen(false); }
    };
    window.addEventListener('keydown', escape, true);
    return () => { window.removeEventListener('keydown', escape, true); previousFocus?.focus(); };
  }, [readingFullscreen]);
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
  const loadNotes = useCallback(async (_charId: string) => {
    setLoading(true);
    try {
      const lists = await Promise.all(CHARACTER_IDS.map(charId => invoke<NoteSummary[]>('list_notebooks', { charId })));
      setNotes(lists.flat().sort((a, b) => b.updated_at - a.updated_at || a.id.localeCompare(b.id)));
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
      setHtml(result.html_path ? convertFileSrc(result.html_path) : `data:text/html;charset=utf-8,${encodeURIComponent(result.html)}`);
    } catch (e) {
      console.error('加载笔记 HTML 失败:', e);
      // html 现为 asset URL（iframe src），加载失败时以 data URL 呈现错误提示
      setHtml('data:text/html;charset=utf-8,' + encodeURIComponent(`<div style="padding:40px;text-align:center;color:#999;font-family:sans-serif;">${t('comic.readFailed')}</div>`));
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
      // 采集归档保留在知识库中，不通过跳转在笔记页打开。
      if (String(nav.pageParams.notebookId).startsWith('note_collected_')) {
        nav.clearPageParams();
        return;
      }
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
      if (!charId || CHARACTER_IDS.includes(charId as CharacterId)) {
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

  // 标签聚合：统计每个标签的笔记数，按数量降序（同数按名称稳定排序）。
  // 标签池随笔记自然生长（不设受控词表），所以这里只聚合现有数据。
  const tagStats = useMemo(() => {
    const counts = new Map<string, number>();
    for (const note of notes) {
      for (const tag of note.tags ?? []) {
        const key = tag.trim();
        if (key) counts.set(key, (counts.get(key) ?? 0) + 1);
      }
    }
    return [...counts.entries()]
      .map(([tag, count]) => ({ tag, count }))
      .sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag));
  }, [notes]);

  // 筛选后的笔记列表。标签为空（trim 后）的笔记视为无分类，任何标签筛选下都不出现。
  const visibleNotes = useMemo(() => {
    if (!activeTag) return notes;
    return notes.filter((note) => (note.tags ?? []).some((t) => t.trim() === activeTag));
  }, [notes, activeTag]);

  // 被筛掉的笔记正好是当前选中项时清空右侧预览，避免「列表看不到、正文还在」
  // 再次点击同一标签即取消筛选
  const toggleTag = useCallback((tag: string) => {
    setActiveTag((prev) => (prev === tag ? null : tag));
  }, []);

  useEffect(() => {
    if (!activeTag) return;
    // 标签被删或笔记不再命中该标签时，退出筛选而不是留一个空列表
    if (!tagStats.some((s) => s.tag === activeTag)) setActiveTag(null);
  }, [tagStats, activeTag]);

  // 删除笔记
  const handleDelete = useCallback(
    async (noteId: string, e: React.MouseEvent) => {
      e.stopPropagation();
      try {
        await invoke('delete_notebook', { charId: notes.find(note => note.id === noteId)?.char_id ?? character, noteId });
        if (selectedId === noteId) {
          setSelectedId(null);
          setHtml('');
        }
      } catch (err) {
        console.error('删除笔记失败:', err);
      }
    },
    [character, selectedId, notes],
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
    if (!draft || saving) return;
    if (!draft.title.trim()) {
      setEditError(t('notebook.title') + ' ?');
      return;
    }
    if (draft.blocks.length === 0) {
      setEditError(t('notebook.content_required', { defaultValue: '请先写一点页面正文' }));
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
          // 传undefined 而非空串：create 侧空串会被 trim 过滤成None
          customCss: draft.custom_css ?? undefined,
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
          // 必传：空串表示「显式撤销自定义 CSS 回到预设配色」，
          // 不传则 update_notebook 保持原值
          customCss: draft.custom_css ?? '',
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
  }, [draft, saving, isNew, character, t, loadNotes]);

  const selectedNote = notes.find((n) => n.id === selectedId);
  // A reading panel in the shared book opens with actual content. Never replace
  // an explicit deep link or an in-progress new note / edit.
  useEffect(() => {
    if (embedded && !selectedId && !editing && notes.length) { setCharacter(notes[0].char_id as CharacterId); setSelectedId(notes[0].id); }
  }, [embedded, selectedId, editing, notes]);

  // ============================================================
  // 渲染
  // ============================================================

  return (
    <section className={`comic-chapter comic-notebook${editing ? ' is-editing' : ''}`}>
    {!embedded && <ComicChapterHeading number="03" title={editing ? '写到这一格' : '还没说完的话'} caption="灵感、摘录，还有想留下的片段。" image="notebook-letter-v31" position="50% 50%" />}
    <div className="record-note">
      {/* === 左：笔记索引 === */}
      <div className="record-note-index" hidden={compact}>
        {!embedded && <ComicChapterInset image={character === 'vivian' ? 'vivian-line-phone-v25' : 'nana-listening-v22'}
          caption={character === 'vivian' ? '先记住，再慢慢想。' : '把喜欢的片段收好。'} />}
        <div className="record-note-index-head">
          <span className="record-note-library-title"><NotebookPen size={18} />{t('notebook.tag_all')}<span>{notes.length}</span></span>
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

        {tagStats.length > 0 && (
          <div className="record-note-tagbar" role="group" aria-label={t('notebook.filter_by_tag', { defaultValue: '按标签筛选' })}>
            <button
              type="button"
              className={`record-note-tagchip${activeTag === null ? ' is-active' : ''}`}
              aria-pressed={activeTag === null}
              onClick={() => setActiveTag(null)}
            >
              {t('notebook.tag_all', { defaultValue: '全部' })}
              <span className="record-note-tagchip-count">{notes.length}</span>
            </button>
            {tagStats.map(({ tag, count }) => (
              <button
                key={tag}
                type="button"
                className={`record-note-tagchip${activeTag === tag ? ' is-active' : ''}`}
                aria-pressed={activeTag === tag}
                onClick={() => toggleTag(tag)}
              >
                {tag}
                <span className="record-note-tagchip-count">{count}</span>
              </button>
            ))}
          </div>
        )}

        <div className="record-note-index-scroll">
          {loading && notes.length === 0 ? (
            <Blank icon={NotebookPen} title={t('common.loading')} />
          ) : notes.length === 0 ? (
            <Blank
              icon={NotebookPen}
              title={t('notebook.empty_hint', { name: t(`mind_inspector.common.char_${character}`) })}
            />
          ) : visibleNotes.length === 0 ? (
            <Blank
              icon={Tag}
              title={t('notebook.tag_filter_empty', { defaultValue: '该标签下没有笔记' })}
            />
          ) : (
            visibleNotes.map((note) => (
              <NoteRow
                key={note.id}
                note={note}
                active={note.id === selectedId}
                onClick={() => { setCharacter(note.char_id as CharacterId); setSelectedId(note.id); }}
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
                <span>{formatTime(selectedNote.updated_at, i18n.resolvedLanguage ?? i18n.language)}</span>
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
              {!compact && <button type="button" className="rec-icon-btn" onClick={() => setReadingFullscreen(true)}
                title={t('comic.noteFullscreen')} aria-label={t('comic.noteFullscreen')} disabled={loadingHtml || !html}>
                <Maximize size={16} />
              </button>}
            </div>
            {/* 笔记预览（iframe src = asset URL 渲染完整文档，跨源隔离） */}
            <div className="record-note-work-body">
              {loadingHtml && !compact && (
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
                scrolling={compact ? 'no' : undefined}
              />
            </div>
          </>
        ) : (
          <div className="record-note-work-body">
            {!compact && <Blank
              icon={NotebookPen}
              title={t('notebook.select_hint')}
              hint={t('notebook.import_drop_hint', { defaultValue: '或将 .html 文件拖入此窗口' })}
            >
              <button type="button" onClick={() => void handlePickHtml()} className="rec-btn">
                <Upload size={14} />
                {t('notebook.import_html', { defaultValue: '导入 HTML 文件' })}
              </button>
            </Blank>}
          </div>
        )}
      </div>
    </div>
    {readingFullscreen && selectedNote && createPortal(
      <div className="notebook-fullscreen-reader" ref={readingRef} tabIndex={-1} role="dialog" aria-modal="true" aria-label={selectedNote.title}>
        <header><span>{selectedNote.title}</span><button type="button" className="rec-icon-btn" title={t('comic.noteExitFullscreen')} aria-label={t('comic.noteExitFullscreen')} onClick={() => setReadingFullscreen(false)}>
          <Minimize size={16} />
        </button></header>
        <iframe title={selectedNote.title} src={html} sandbox="allow-same-origin" />
      </div>, document.body)}
    </section>
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
  const { t, i18n } = useTranslation();
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
        <span>{formatTime(note.updated_at, i18n.resolvedLanguage ?? i18n.language)}</span>
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
  onSave,
  onCancel,
}) => {
  const [sourceError, setSourceError] = useState(false);
  const showCover = draft.layout === 'cover_flow' || draft.layout === 'gallery';
  const cover = draft.cover;
  // 自定义 CSS 与palette 互斥：开启时 palette 选择器要禁用（后端会忽略它）
  const cssMode = (draft.custom_css ?? '').trim().length > 0;

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

        <button type="button" onClick={onCancel} disabled={saving} className="rec-btn">
          <X size={13} /> {t('common.cancel')}
        </button>
        <button type="button" onClick={onSave} disabled={saving || sourceError} className="rec-btn is-primary">
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

          <details className="note-page-settings">
            <summary>{lang.startsWith('zh') ? '外观与属性 · 配色、布局、标签、封面、CSS' : 'Appearance & properties · palette, layout, tags, cover, CSS'}</summary>
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
                  // 启用自定义 CSS 时改palette 无效（后端以 CSS 为唯一主题来源），
                  // 所以禁用并给出说明，而不是让用户改了却看不到变化
                  disabled={cssMode}
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
                    cursor: cssMode ? 'not-allowed' : 'pointer',
                    padding: 0,
                    opacity: cssMode ? 0.4 : 1,
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
                <Code size={13} /> {t('notebook.custom_css')}
              </span>
            </label>
            <div style={{ display: 'flex', gap: 6, alignItems: 'center', marginBottom: cssMode ? 8 : 0 }}>
              <button
                type="button"
                className={`rec-btn${cssMode ? ' is-primary' : ''}`}
                aria-pressed={cssMode}
                onClick={() =>
                  onPatch((d) => ({
                    ...d,
                    // 关闭时写入空串而非 undefined：后端 update_notebook 靠
                    // Some("") 区分「显式撤销 CSS」与「这次没改这个字段」
                    custom_css: cssMode ? null : CSS_PRESET_PLACEHOLDER,
                  }))
                }
              >
                {cssMode
                  ? t('notebook.custom_css_on', { defaultValue: '使用自定义 CSS' })
                  : t('notebook.custom_css_off', { defaultValue: '启用自定义 CSS' })}
              </button>
              {cssMode && (
                <span className="rec-label-hint">
                  {t('notebook.custom_css_hint', {
                    defaultValue: '与配色互斥：自定义 CSS 是唯一主题来源',
                  })}
                </span>
              )}
            </div>
            {cssMode && (
              <>
                <textarea
                  className="rec-textarea"
                  style={{ minHeight: 180, fontFamily: 'ui-monospace, monospace', fontSize: 12.5 }}
                  value={draft.custom_css ?? ''}
                  placeholder={CSS_PLACEHOLDER}
                  onChange={(e) => onPatch((d) => ({ ...d, custom_css: e.target.value }))}
                />
                <div style={{ display: 'flex', gap: 6, marginTop: 6 }}>
                  <button
                    type="button"
                    className="rec-btn is-ghost"
                    onClick={() => onPatch((d) => ({ ...d, custom_css: CSS_PRESET_PLACEHOLDER }))}
                  >
                    {t('notebook.custom_css_example', { defaultValue: '插入示例' })}
                  </button>
                  <button
                    type="button"
                    className="rec-btn is-ghost"
                    disabled={!(draft.custom_css ?? '').trim()}
                    onClick={() => onPatch((d) => ({ ...d, custom_css: '' }))}
                  >
                    {t('notebook.custom_css_clear', { defaultValue: '清空' })}
                  </button>
                </div>
              </>
            )}
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
              placeholder={t('comic.noteTagsExample')}
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
                          : { title: d.title, subtitle: '', background: '' },
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
                    <div className="rec-field">
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
                  </>
                )}
              </div>
            </div>
          )}

          </details>

          <NotebookPageEditor note={draft} lang={lang} onSave={onSave} saving={saving}
            onValidityChange={setSourceError}
            onChange={blocks => onPatch(d => blocks === null ? { ...d } : { ...d, blocks })} />
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

export default NotebookPage;
