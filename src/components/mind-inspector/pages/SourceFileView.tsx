/**
 * 源码查看 / 编辑视图（右侧预览面板的文本文件分支）
 *
 * - 只读：highlight.js 语法高亮 + 行号 gutter
 * - 编辑：等宽 textarea + 行号，保存走 `coding_write_file`
 * - 超大文件：初始只收首段，`coding_read_file_lines` 分页续读「加载更多」
 * - markdown 另有两种模式：源码（高亮）与渲染（块级就地编辑），工具栏可切换
 *
 * 高亮产物经 DOMPurify 白名单过滤后再注入，与 WidgetCard 同一套安全链路。
 *
 * 渲染态编辑刻意不碰渲染结果：点某一块的铅笔后，按块记的源码行区间取出那几行原文
 * 交给 textarea，提交时替换回同一区间。于是不存在「把 DOM 反推回 markdown」这一步
 * —— 那条路在嵌套列表缩进、转义字符、行尾空格换行上都是有损的。
 */

import React, { useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Loader2, Pencil, Save, X, Eye, Code2 } from 'lucide-react';
import DOMPurify from 'dompurify';
import hljs from 'highlight.js/lib/core';
import { MarkdownFileContext, renderMarkdownBlocks } from './codeMarkdown';
// 语言按需注册（各语言体积 2~8KB，静态引入避免运行时异步加载）
import javascript from 'highlight.js/lib/languages/javascript';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml';
import css from 'highlight.js/lib/languages/css';
import scss from 'highlight.js/lib/languages/scss';
import less from 'highlight.js/lib/languages/less';
import json from 'highlight.js/lib/languages/json';
import yaml from 'highlight.js/lib/languages/yaml';
import markdown from 'highlight.js/lib/languages/markdown';
import rust from 'highlight.js/lib/languages/rust';
import python from 'highlight.js/lib/languages/python';
import bash from 'highlight.js/lib/languages/bash';
import c from 'highlight.js/lib/languages/c';
import cpp from 'highlight.js/lib/languages/cpp';
import java from 'highlight.js/lib/languages/java';
import go from 'highlight.js/lib/languages/go';
import sql from 'highlight.js/lib/languages/sql';
import ini from 'highlight.js/lib/languages/ini';
import diff from 'highlight.js/lib/languages/diff';

// typescript 内嵌 JSX，依赖 xml + javascript，须先注册
hljs.registerLanguage('xml', xml);
hljs.registerLanguage('javascript', javascript);
hljs.registerLanguage('typescript', typescript);
hljs.registerLanguage('css', css);
hljs.registerLanguage('scss', scss);
hljs.registerLanguage('less', less);
hljs.registerLanguage('json', json);
hljs.registerLanguage('yaml', yaml);
hljs.registerLanguage('markdown', markdown);
hljs.registerLanguage('rust', rust);
hljs.registerLanguage('python', python);
hljs.registerLanguage('bash', bash);
hljs.registerLanguage('c', c);
hljs.registerLanguage('cpp', cpp);
hljs.registerLanguage('java', java);
hljs.registerLanguage('go', go);
hljs.registerLanguage('sql', sql);
hljs.registerLanguage('ini', ini);
hljs.registerLanguage('diff', diff);

/** 扩展名 → hljs 语言名 */
const EXT_LANG: Record<string, string> = {
  ts: 'typescript', tsx: 'typescript', js: 'javascript', jsx: 'javascript', mjs: 'javascript', cjs: 'javascript',
  css: 'css', scss: 'scss', less: 'less', sass: 'scss',
  json: 'json', yaml: 'yaml', yml: 'yaml', toml: 'toml', ini: 'ini', env: 'ini',
  html: 'xml', htm: 'xml', xml: 'xml', svg: 'xml', vue: 'xml',
  md: 'markdown', markdown: 'markdown', mdx: 'markdown',
  rs: 'rust', py: 'python', sh: 'bash', bash: 'bash', zsh: 'bash', ps1: 'bash', bat: 'bash', cmd: 'bash',
  c: 'c', h: 'c', cc: 'cpp', cpp: 'cpp', hpp: 'cpp', cxx: 'cpp',
  java: 'java', go: 'go', sql: 'sql', diff: 'diff', patch: 'diff',
};

const langForPath = (path: string): string => {
  const ext = (path.split('.').pop() ?? '').toLowerCase();
  return EXT_LANG[ext] ?? '';
};

const escapeHtml = (s: string): string =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

/** 高亮整段文本，返回 DOMPurify 过滤后的 HTML；未知语言退化为转义纯文本 */
function highlightHtml(code: string, lang: string): string {
  let html: string;
  if (lang && hljs.getLanguage(lang)) {
    try {
      html = hljs.highlight(code, { language: lang, ignoreIllegals: true }).value;
    } catch {
      html = escapeHtml(code);
    }
  } else {
    html = escapeHtml(code);
  }
  return DOMPurify.sanitize(html, { ALLOWED_TAGS: ['span'], ALLOWED_ATTR: ['class'] });
}

export interface SourceFileData {
  path: string;
  content?: string | null;
  total_lines?: number | null;
  truncated?: boolean;
}

/** 每页续读的行数（与后端 `coding_read_file_lines` 的 count 上限对齐） */
const CHUNK_LINES = 3000;

export const SourceFileView: React.FC<{
  data: SourceFileData;
  targetLine?: number;
  navigationRevision?: number;
}> = ({ data, targetLine, navigationRevision }) => {
  const [lines, setLines] = useState<string[]>(() => (data.content ?? '').split('\n'));
  const [totalLines, setTotalLines] = useState<number>(data.total_lines ?? (data.content ?? '').split('\n').length);
  const [loadingMore, setLoadingMore] = useState(false);
  const [moreError, setMoreError] = useState<string | null>(null);

  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const [saving, setSaving] = useState(false);
  const [savedFlash, setSavedFlash] = useState(false);
  const [editError, setEditError] = useState<string | null>(null);

  // markdown 的两种模式：源码（语法高亮）与渲染（块级就地编辑）
  const [mode, setMode] = useState<'source' | 'rendered'>('source');
  const [editingBlock, setEditingBlock] = useState<number | null>(null);
  const [blockDraft, setBlockDraft] = useState('');
  const [blockSaving, setBlockSaving] = useState(false);
  const [blockError, setBlockError] = useState<string | null>(null);

  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const gutterRef = useRef<HTMLDivElement | null>(null);
  const editHighlightRef = useRef<HTMLPreElement | null>(null);
  const readonlyScrollRef = useRef<HTMLDivElement | null>(null);
  const mdCtx = useContext(MarkdownFileContext);

  const lang = useMemo(() => langForPath(data.path), [data.path]);
  const isMarkdown = lang === 'markdown';

  /** 原文。渲染态直接用它；源码态另外走高亮后的 codeLines。 */
  const content = useMemo(() => lines.join('\n'), [lines]);

  /**
   * 渲染态的块序列（每项带源码行区间）。只在 markdown + 渲染模式 + 未进入整体编辑时
   * 计算：解析整篇是 O(行数)，没必要在源码态白跑一遍。
   */
  const mdBlocks = useMemo(
    () => (isMarkdown && mode === 'rendered' && !editing
      ? renderMarkdownBlocks(content, 'srcmd', mdCtx)
      : []),
    [isMarkdown, mode, editing, content, mdCtx],
  );

  // 只读态：高亮整段并拆成行（hljs 的 span 不跨行，拆分安全）
  const codeLines = useMemo(() => highlightHtml(lines.join('\n'), lang).split('\n'), [lines, lang]);
  // 编辑态：textarea 负责输入，其下方的 pre 负责实时语法着色。
  const draftHtml = useMemo(() => highlightHtml(draft, lang), [draft, lang]);

  const startEdit = useCallback(() => {
    setDraft(lines.join('\n'));
    setEditError(null);
    setEditing(true);
  }, [lines]);

  const cancelEdit = useCallback(() => {
    setEditing(false);
    setDraft('');
    setEditError(null);
  }, []);

  const save = useCallback(async () => {
    if (saving) return;
    setSaving(true);
    setEditError(null);
    try {
      await invoke('coding_write_file', { path: data.path, content: draft });
      const next = draft.split('\n');
      setLines(next);
      setTotalLines(next.length);
      setEditing(false);
      setDraft('');
      setSavedFlash(true);
      window.setTimeout(() => setSavedFlash(false), 1800);
    } catch (e) {
      setEditError(String(e));
    } finally {
      setSaving(false);
    }
  }, [draft, data.path, saving]);

  // ---- 渲染态：块级就地编辑 ----

  const startBlockEdit = useCallback(
    (index: number, start: number, end: number) => {
      setBlockDraft(lines.slice(start, end).join('\n'));
      setEditingBlock(index);
      setBlockError(null);
    },
    [lines],
  );

  const cancelBlockEdit = useCallback(() => {
    setEditingBlock(null);
    setBlockDraft('');
    setBlockError(null);
  }, []);

  /**
   * 提交块编辑：把 draft 替换回它原来占的那几行，然后写盘。
   *
   * 走的是与整体编辑同一个 `coding_write_file`。改完就地重新解析，块的行区间随
   * 行数变化自然更新 —— 不需要手工维护后面块的偏移。
   */
  const saveBlockEdit = useCallback(
    async (start: number, end: number) => {
      if (blockSaving) return;
      setBlockSaving(true);
      setBlockError(null);
      try {
        const next = [...lines.slice(0, start), ...blockDraft.split('\n'), ...lines.slice(end)];
        await invoke('coding_write_file', { path: data.path, content: next.join('\n') });
        setLines(next);
        setTotalLines(next.length);
        setEditingBlock(null);
        setBlockDraft('');
        setSavedFlash(true);
        window.setTimeout(() => setSavedFlash(false), 1800);
      } catch (e) {
        setBlockError(String(e));
      } finally {
        setBlockSaving(false);
      }
    },
    [blockDraft, blockSaving, data.path, lines],
  );

  const loadMore = useCallback(async () => {
    if (loadingMore) return;
    setLoadingMore(true);
    setMoreError(null);
    try {
      const chunk = await invoke<string[]>('coding_read_file_lines', {
        path: data.path, start: lines.length, count: CHUNK_LINES,
      });
      setLines((prev) => [...prev, ...chunk]);
    } catch (e) {
      setMoreError(String(e));
    } finally {
      setLoadingMore(false);
    }
  }, [loadingMore, lines.length, data.path]);

  const syncGutter = useCallback(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    if (gutterRef.current) gutterRef.current.scrollTop = textarea.scrollTop;
    if (editHighlightRef.current) {
      editHighlightRef.current.scrollTop = textarea.scrollTop;
      editHighlightRef.current.scrollLeft = textarea.scrollLeft;
    }
  }, []);

  const hasMore = lines.length < totalLines;

  useEffect(() => {
    if (!targetLine || targetLine < 1 || editing) return;
    const frame = window.requestAnimationFrame(() => {
      const scroller = readonlyScrollRef.current;
      const row = scroller?.querySelector<HTMLElement>(`[data-source-line="${targetLine}"]`);
      if (!scroller || !row) return;
      const top = Math.max(0, row.offsetTop - (scroller.clientHeight - row.offsetHeight) / 2);
      scroller.scrollTo({ top, behavior: 'smooth' });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [targetLine, navigationRevision, codeLines.length, editing]);

  return (
    <div className="codex-src">
      <div className="codex-src-toolbar">
        <span className="codex-src-lang">{lang || 'text'}</span>
        <span className="codex-src-meta">
          {totalLines.toLocaleString()} {totalLines === 1 ? 'line' : 'lines'}
          {hasMore ? ` · ${lines.length.toLocaleString()} loaded` : ''}
        </span>
        <span className="codex-src-spacer" />
        {savedFlash && <span className="codex-src-saved">已保存</span>}
        {isMarkdown && !editing && (
          <button
            type="button"
            className="codex-src-btn"
            onClick={() => {
              cancelBlockEdit();
              setMode((m) => (m === 'rendered' ? 'source' : 'rendered'));
            }}
            title={mode === 'rendered' ? '查看源码' : '渲染预览'}
          >
            {mode === 'rendered' ? <Code2 size={13} /> : <Eye size={13} />}
          </button>
        )}
        {editing ? (
          <>
            <button type="button" className="codex-src-btn" onClick={cancelEdit} title="取消">
              <X size={13} />
            </button>
            <button type="button" className="codex-src-btn codex-src-btn-primary" onClick={save} disabled={saving} title="保存">
              {saving ? <Loader2 size={13} className="codex-spin" /> : <Save size={13} />}
            </button>
          </>
        ) : mode === 'source' ? (
          <button
            type="button"
            className="codex-src-btn"
            onClick={startEdit}
            disabled={hasMore}
            title={hasMore ? '超大文件需先加载全部再编辑' : '编辑'}
          >
            <Pencil size={13} />
          </button>
        ) : null}
      </div>

      {editError && <div className="codex-src-error">{editError}</div>}
      {blockError && <div className="codex-src-error">{blockError}</div>}

      {editing ? (
        <div className="codex-src-scroll codex-src-editor">
          <div className="codex-src-gutter" ref={gutterRef} aria-hidden="true">
            {draft.split('\n').map((_, i) => (
              <div key={i} className="codex-src-ln">{i + 1}</div>
            ))}
          </div>
          <div className="codex-src-editor-stage">
            <pre
              ref={editHighlightRef}
              className="codex-src-edit-highlight"
              aria-hidden="true"
              dangerouslySetInnerHTML={{ __html: draftHtml || ' ' }}
            />
            <textarea
              ref={textareaRef}
              className="codex-src-textarea"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onScroll={syncGutter}
              spellCheck={false}
              autoCapitalize="off"
              autoComplete="off"
              autoCorrect="off"
              wrap="off"
              aria-label={`Edit ${data.path}`}
            />
          </div>
        </div>
      ) : mode === 'rendered' ? (
        <div className="codex-src-scroll">
          {mdBlocks.length === 0 ? (
            <div className="codex-src-more">（空文档）</div>
          ) : (
            <div className="codex-md codex-src-md">
              {mdBlocks.map((b, i) => (
                <div key={b.key} className="codex-src-mdblock" data-md-block={i}>
                  {editingBlock === i ? (
                    <div className="codex-src-mdblock-editor">
                      <textarea
                        className="codex-src-mdblock-area"
                        value={blockDraft}
                        onChange={(e) => setBlockDraft(e.target.value)}
                        spellCheck={false}
                        autoFocus
                        aria-label="编辑这一段"
                      />
                      <div className="codex-src-mdblock-actions">
                        <button type="button" className="codex-src-btn" onClick={cancelBlockEdit} title="取消">
                          <X size={13} />
                        </button>
                        <button
                          type="button"
                          className="codex-src-btn codex-src-btn-primary"
                          onClick={() => void saveBlockEdit(b.span.start, b.span.end)}
                          disabled={blockSaving}
                          title="保存"
                        >
                          {blockSaving ? <Loader2 size={13} className="codex-spin" /> : <Save size={13} />}
                        </button>
                      </div>
                    </div>
                  ) : (
                    <>
                      {/* 已有块在编辑时不挂铅笔：避免开着编辑框又去点另一块；
                          超大文件（分页未完）也不给改，否则写盘会截断未加载的部分 */}
                      {!hasMore && editingBlock === null && (
                        <button
                          type="button"
                          className="codex-src-mdblock-btn"
                          onClick={() => startBlockEdit(i, b.span.start, b.span.end)}
                          title="编辑这一段"
                        >
                          <Pencil size={11} />
                        </button>
                      )}
                      {b.node}
                    </>
                  )}
                </div>
              ))}
            </div>
          )}
        </div>
      ) : (
        <div className="codex-src-scroll" ref={readonlyScrollRef}>
          <div className="codex-src-code">
            {codeLines.map((html, i) => (
              <div
                key={i}
                className={`codex-src-row${targetLine === i + 1 ? ' is-target-line' : ''}`}
                data-source-line={i + 1}
              >
                <span className="codex-src-ln">{i + 1}</span>
                <span className="codex-src-cell" dangerouslySetInnerHTML={{ __html: html }} />
              </div>
            ))}
          </div>
        </div>
      )}

      {!editing && hasMore && (
        <div className="codex-src-more">
          {moreError ? (
            <span className="codex-src-more-error">{moreError}</span>
          ) : (
            <button type="button" className="codex-src-more-btn" onClick={loadMore} disabled={loadingMore}>
              {loadingMore ? <Loader2 size={13} className="codex-spin" /> : null}
              加载更多（已加载 {lines.length.toLocaleString()} / 共 {totalLines.toLocaleString()} 行）
            </button>
          )}
        </div>
      )}
    </div>
  );
};

export default SourceFileView;
