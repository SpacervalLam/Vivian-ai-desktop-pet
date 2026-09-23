/**
 * 富文本输入编辑器（工作页 composer 的输入层）。
 *
 * 形态：
 * - **粘贴**的 markdown 按消息区那套规则解析成块并渲染（与 `MarkdownText` 共用
 *   `parseBlocks` 与 `.codex-md-*` 类名，观感和上面的回复一致）；
 * - **手打**的字符保持原样，不做语法自动转换（敲 `# ` 不会自己变成标题）；
 * - **抹黑选中**浮出格式卡片：块类型（Text / Heading 1-3 / Numbered list / Bulleted
 *   list）+ 加粗 / 斜体 / 链接。没有「预览 / 编辑」模式切换。
 *
 * 数据流：外部真源仍是 markdown 字符串（发送、斜杠命令、@-mention 插入都基于它）。
 * - 内部编辑（打字、格式化）：DOM 是真相，**不触发块重建**；读回 DOM → 序列化 → onChange(md)
 * - 外部写入（value 与内部最近一次 emit 不同）：整篇重解析 → 重建块
 *
 * 块内容刻意由 ref **只写一次**、之后交给浏览器。若改用受控的
 * `dangerouslySetInnerHTML`，每次 render 都会重设内容：光标弹回开头、原生撤销失效。
 */

import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { Bold, Italic, Link2, ChevronDown, Check } from 'lucide-react';
import { parseBlocks } from './codeMarkdown';

// ============ 块模型 ============

type BlockType = 'p' | 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6' | 'ul' | 'ol' | 'quote' | 'code' | 'table' | 'hr';

interface EditorBlock {
  id: string;
  type: BlockType;
  /** 块内容：行内 HTML；`code` 块是**纯文本**（渲染时走 textContent），`table` 是完整 table 标签 */
  html: string;
  /** code 块的语言标记 */
  lang?: string;
}

let blockSeq = 0;
const nextBlockId = () => `cb${(blockSeq += 1)}`;

/** 浮卡下拉里的块类型（与 Codex 截图一致） */
const BLOCK_CHOICES: { type: BlockType; key: string }[] = [
  { type: 'p', key: 'text' },
  { type: 'h1', key: 'h1' },
  { type: 'h2', key: 'h2' },
  { type: 'h3', key: 'h3' },
  { type: 'ol', key: 'ol' },
  { type: 'ul', key: 'ul' },
];

const BLOCK_TAGS: Record<BlockType, string> = {
  p: 'p', h1: 'h1', h2: 'h2', h3: 'h3', h4: 'h4', h5: 'h5', h6: 'h6',
  ul: 'ul', ol: 'ol', quote: 'blockquote', code: 'pre', table: 'table', hr: 'hr',
};

/** 从元素标签反推块类型（浏览器可能在编辑中造出新元素，读回时按标签兜底） */
function typeFromTag(el: HTMLElement): BlockType {
  const tag = el.tagName.toLowerCase();
  switch (tag) {
    case 'h1': case 'h2': case 'h3': case 'h4': case 'h5': case 'h6':
      return tag as BlockType;
    case 'ul': return 'ul';
    case 'ol': return 'ol';
    case 'blockquote': return 'quote';
    case 'pre': return 'code';
    case 'table': return 'table';
    case 'hr': return 'hr';
    default: return 'p';
  }
}

// ============ 行内 markdown ⇄ HTML ============

const escapeHtml = (s: string): string =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

/** `2**3` 这类指数表达式不该被当成粗体（与 codeMarkdown 的判定保持一致） */
function looksLikeOperator(prev: string, next: string): boolean {
  return /[\w)]/.test(prev) && /[\w(]/.test(next);
}

/**
 * 行内 markdown → HTML。
 *
 * 规则与 `codeMarkdown.renderInline` 对齐：`` `code` `` / `**粗**` / `~~删~~` / `*斜*` /
 * `[文字](链接)`；不解析 `_` 系（标识符里的下划线太常见，误伤成本高于收益）。
 */
export function inlineMdToHtml(text: string): string {
  let out = '';
  let i = 0;
  while (i < text.length) {
    const ch = text[i];
    const prevChar = i === 0 ? '' : text[i - 1];

    if (ch === '`') {
      const end = text.indexOf('`', i + 1);
      if (end > i + 1) {
        out += `<code class="codex-md-code">${escapeHtml(text.slice(i + 1, end))}</code>`;
        i = end + 1;
        continue;
      }
    }

    if (ch === '*' || ch === '~') {
      const two = text.slice(i, i + 2);
      if (two === '**' || two === '~~') {
        const close = text.indexOf(two, i + 2);
        if (close > i + 2 && !(two === '**' && looksLikeOperator(prevChar, text[i + 2] ?? ''))) {
          const inner = inlineMdToHtml(text.slice(i + 2, close));
          out += two === '**' ? `<strong>${inner}</strong>` : `<del>${inner}</del>`;
          i = close + 2;
          continue;
        }
        out += escapeHtml(two);
        i += 2;
        continue;
      }
    }

    if (ch === '*') {
      const close = text.indexOf('*', i + 1);
      if (close > i + 1 && text[i + 1] !== ' ' && text[close - 1] !== ' ' && !looksLikeOperator(prevChar, text[i + 1])) {
        out += `<em>${inlineMdToHtml(text.slice(i + 1, close))}</em>`;
        i = close + 1;
        continue;
      }
    }

    if (ch === '[') {
      const m = /^\[([^\]]*)\]\((<[^>]+>|[^)]+)\)/.exec(text.slice(i));
      if (m) {
        const label = m[1] || m[2];
        const href = m[2].trim().replace(/^<|>$/g, '');
        out += `<a class="codex-md-link" href="${escapeHtml(href)}" title="${escapeHtml(href)}">${escapeHtml(label)}</a>`;
        i += m[0].length;
        continue;
      }
    }

    out += escapeHtml(ch);
    i += 1;
  }
  return out;
}

/** 行内 HTML → markdown（只认白名单标签，其余取其文本） */
export function inlineHtmlToMd(html: string): string {
  if (!html) return '';
  const root = document.createElement('div');
  root.innerHTML = html;
  const walk = (node: Node): string => {
    if (node.nodeType === Node.TEXT_NODE) return node.textContent ?? '';
    if (node.nodeType !== Node.ELEMENT_NODE) return '';
    const el = node as HTMLElement;
    const inner = Array.from(el.childNodes).map(walk).join('');
    switch (el.tagName.toLowerCase()) {
      case 'strong': case 'b': return `**${inner}**`;
      case 'em': case 'i': return `*${inner}*`;
      case 'del': case 's': return `~~${inner}~~`;
      case 'code': return `\`${inner}\``;
      case 'br': return '\n';
      case 'a': {
        const href = el.getAttribute('href') ?? '';
        return href ? `[${inner}](${href})` : inner;
      }
      default: return inner;
    }
  };
  return Array.from(root.childNodes).map(walk).join('');
}

// ============ 块 ⇄ markdown ============

const cellMd = (html: string): string => inlineHtmlToMd(html).replace(/\|/g, '\\|').replace(/\n/g, ' ').trim();

function tableHtmlToMd(html: string): string {
  const root = document.createElement('div');
  root.innerHTML = html;
  const table = root.querySelector('table');
  if (!table) return '';
  const rows = Array.from(table.querySelectorAll('tr')).map((tr) =>
    Array.from(tr.querySelectorAll('th,td')).map((cell) => cellMd((cell as HTMLElement).innerHTML)),
  );
  if (rows.length === 0) return '';
  const header = rows[0];
  const line = (cells: string[]) => `| ${cells.join(' | ')} |`;
  return [line(header), line(header.map(() => '---')), ...rows.slice(1).map(line)].join('\n');
}

function listHtmlToMd(html: string, ordered: boolean): string {
  const root = document.createElement('div');
  root.innerHTML = html;
  const out: string[] = [];
  const walk = (list: Element, depth: number) => {
    const pad = '  '.repeat(depth);
    Array.from(list.children).forEach((li, idx) => {
      if (li.tagName.toLowerCase() !== 'li') return;
      const clone = li.cloneNode(true) as HTMLElement;
      clone.querySelectorAll('ul,ol').forEach((sub) => sub.remove());
      out.push(`${pad}${ordered ? `${idx + 1}.` : '-'} ${inlineHtmlToMd(clone.innerHTML).trim()}`);
      Array.from(li.children).forEach((sub) => {
        const tag = sub.tagName.toLowerCase();
        if (tag === 'ul' || tag === 'ol') walk(sub, depth + 1);
      });
    });
  };
  const first = root.querySelector('ul,ol');
  if (first) walk(first, 0);
  return out.join('\n');
}

/** 块序列 → markdown（发送给模型的就是它） */
export function blocksToMarkdown(blocks: readonly EditorBlock[]): string {
  const parts: string[] = [];
  for (const b of blocks) {
    switch (b.type) {
      case 'p':
        parts.push(inlineHtmlToMd(b.html).trim());
        break;
      case 'h1': case 'h2': case 'h3': case 'h4': case 'h5': case 'h6':
        parts.push(`${'#'.repeat(Number(b.type[1]))} ${inlineHtmlToMd(b.html).trim()}`);
        break;
      case 'ul':
        parts.push(listHtmlToMd(b.html, false));
        break;
      case 'ol':
        parts.push(listHtmlToMd(b.html, true));
        break;
      case 'quote':
        parts.push(inlineHtmlToMd(b.html).split('\n').map((l) => `> ${l}`).join('\n'));
        break;
      case 'code':
        parts.push(`\`\`\`${b.lang ?? ''}\n${b.html}\`\`\``);
        break;
      case 'table':
        parts.push(tableHtmlToMd(b.html));
        break;
      case 'hr':
        parts.push('---');
        break;
      default:
        parts.push(inlineHtmlToMd(b.html).trim());
    }
  }
  return parts.filter((p) => p.trim() !== '').join('\n\n');
}

// ============ 组件 ============

export interface ComposerEditorProps {
  /** markdown 真源 */
  value: string;
  onChange: (md: string) => void;
  /** Enter（无 Shift）时触发，通常为发送 */
  onSubmit: () => void;
  onEscape?: () => void;
  /** 粘贴了图片等文件时交给外层（原 textarea 的 onPaste 职责） */
  onPasteFiles?: (files: File[]) => void;
  placeholder?: string;
}

export interface ComposerEditorHandle {
  focus: () => void;
  /** 把文本插到光标处（斜杠命令、@-mention 等外部写入用） */
  insertText: (text: string) => void;
  /** 读当前 markdown（语音输入算基线长度等场景要即时值，不能依赖 React state） */
  getMarkdown: () => string;
}

const ComposerEditor = React.forwardRef<ComposerEditorHandle, ComposerEditorProps>(
  ({ value, onChange, onSubmit, onEscape, onPasteFiles, placeholder }, ref) => {
    const { t } = useTranslation();
    const rootRef = useRef<HTMLDivElement | null>(null);
    const blocksRef = useRef<EditorBlock[]>([]);
    /** 内部最近一次 emit 出去的 markdown：value 与它不同 = 外部写入 */
    const lastEmittedRef = useRef<string>('');
    /** 输入法组合中：组合文本是「内容但未提交」，期间不闪占位符 */
    const composingRef = useRef(false);
    const [blocks, setBlocks] = useState<EditorBlock[]>([]);
    const [bubble, setBubble] = useState<{ x: number; y: number; blockId: string } | null>(null);
    const [menuOpen, setMenuOpen] = useState(false);
    const [linkMode, setLinkMode] = useState(false);
    const [linkDraft, setLinkDraft] = useState('');

    /**
     * 读回整篇 markdown。顺带把浏览器在编辑中造出的、没有标记的顶层元素补上
     * data-block-id / data-block-type —— 直接改 dataset 不动内容，所以不会影响光标。
     */
    const readMarkdown = useCallback((): string => {
      const root = rootRef.current;
      if (!root) return '';
      const next: EditorBlock[] = [];
      for (const child of Array.from(root.children)) {
        const el = child as HTMLElement;
        if (!el.dataset.blockId) {
          el.dataset.blockId = nextBlockId();
          el.dataset.blockType = typeFromTag(el);
        }
        const type = (el.dataset.blockType ?? 'p') as BlockType;
        next.push({
          id: el.dataset.blockId,
          type,
          html: type === 'code' ? (el.textContent ?? '') : el.innerHTML,
          lang: el.dataset.lang || undefined,
        });
      }
      blocksRef.current = next;
      return blocksToMarkdown(next);
    }, []);

    /**
     * 占位符的显隐必须由「真实 DOM 内容」决定，而不是 React 的 value 状态：
     * 打字（尤其输入法组合中）时 DOM 立刻有内容，但 value 是 emit → onChange →
     * setState → 重渲染之后才变，中间这段时间占位符会叠在实际文字上。
     * 直接同步 dataset，不触发渲染，光标不会被打断。
     */
    const syncPlaceholder = useCallback(() => {
      const el = rootRef.current;
      if (!el) return;
      const empty = !composingRef.current && readMarkdown().trim() === '';
      el.dataset.empty = empty ? 'true' : 'false';
    }, [readMarkdown]);

    const emit = useCallback(() => {
      const md = readMarkdown();
      lastEmittedRef.current = md;
      syncPlaceholder();
      onChange(md);
    }, [readMarkdown, syncPlaceholder, onChange]);

    // 外部写入：value 与内部最近一次 emit 不一致 → 整篇重解析
    useEffect(() => {
      if (value === lastEmittedRef.current) return;
      const parsed = markdownToBlocks(value);
      blocksRef.current = parsed;
      setBlocks(parsed);
      lastEmittedRef.current = value;
    }, [value]);

    // 首挂载
    useLayoutEffect(() => {
      if (blocksRef.current.length > 0) return;
      const parsed = markdownToBlocks(value);
      blocksRef.current = parsed;
      setBlocks(parsed);
      lastEmittedRef.current = value;
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    // 块有任何变化（挂载 / 外部写入 / 粘贴 / 切块）后，DOM 已与 blocks 对齐，
    // 此时再按真实内容刷新占位符显隐——不能提前 syncPlaceholder，否则读到的
    // 还是旧 DOM（内容已存在却误判为空，占位符盖住正文）。
    useLayoutEffect(() => {
      syncPlaceholder();
    }, [blocks, syncPlaceholder]);

    // 选区 → 浮卡
    useEffect(() => {
      const onSelectionChange = () => {
        const sel = window.getSelection();
        const root = rootRef.current;
        if (!sel || !root || sel.rangeCount === 0 || sel.isCollapsed) {
          setBubble(null);
          return;
        }
        const range = sel.getRangeAt(0);
        if (!root.contains(range.commonAncestorContainer)) {
          setBubble(null);
          return;
        }
        const rect = range.getBoundingClientRect();
        const anchor = (range.startContainer instanceof Element
          ? range.startContainer
          : range.startContainer.parentElement)?.closest<HTMLElement>('[data-block-id]');
        const placeBelow = rect.bottom + 56 < window.innerHeight;
        setBubble({
          x: Math.min(Math.max(rect.left + rect.width / 2, 150), window.innerWidth - 150),
          y: placeBelow ? rect.bottom + 8 : Math.max(rect.top - 48, 8),
          blockId: anchor?.dataset.blockId ?? '',
        });
        setMenuOpen(false);
        setLinkMode(false);
      };
      document.addEventListener('selectionchange', onSelectionChange);
      return () => document.removeEventListener('selectionchange', onSelectionChange);
    }, []);

    const applyInline = useCallback((command: 'bold' | 'italic') => {
      document.execCommand(command);
      emit();
    }, [emit]);

    const applyLink = useCallback((href: string) => {
      if (!href) document.execCommand('unlink');
      else document.execCommand('createLink', false, href);
      setLinkMode(false);
      setLinkDraft('');
      emit();
    }, [emit]);

    /** 切块类型：先同步 DOM 现状（否则会拿旧 html 覆盖用户刚敲的字），再换标签 */
    const applyBlockType = useCallback((type: BlockType) => {
      const id = bubble?.blockId;
      setMenuOpen(false);
      if (!id) return;
      readMarkdown();
      const next = blocksRef.current.map((b) => (b.id === id ? { ...b, type } : b));
      blocksRef.current = next;
      setBlocks(next);
      window.setTimeout(() => {
        const el = rootRef.current?.querySelector<HTMLElement>(`[data-block-id="${id}"]`);
        if (el) placeCaretAtEnd(el);
        emit();
      }, 0);
    }, [bubble, emit, readMarkdown]);

    const onKeyDown = useCallback((e: React.KeyboardEvent) => {
      if (e.key === 'Enter') {
        // Shift+Enter 换行：走 insertLineBreak，避免浏览器在根节点里造出「没有块标记」的新块
        if (e.shiftKey) {
          e.preventDefault();
          document.execCommand('insertLineBreak');
          emit();
          return;
        }
        if (!menuOpen && !linkMode) {
          e.preventDefault();
          onSubmit();
        }
        return;
      }
      if (e.key === 'Escape') {
        if (menuOpen) { setMenuOpen(false); return; }
        if (linkMode) { setLinkMode(false); return; }
        onEscape?.();
      }
    }, [onSubmit, onEscape, menuOpen, linkMode, emit]);

    /** 粘贴：markdown 解析成块；纯文本交回浏览器默认插入 */
    const onPaste = useCallback((e: React.ClipboardEvent) => {
      const files = Array.from(e.clipboardData?.files ?? []);
      if (files.length > 0) {
        e.preventDefault();
        onPasteFiles?.(files);
        return;
      }
      const text = e.clipboardData?.getData('text/plain') ?? '';
      if (!text) return;
      const parsed = markdownToBlocks(text);
      if (parsed.length <= 1 && parsed.every((b) => b.type === 'p')) return; // 纯文本不接管
      e.preventDefault();
      readMarkdown();
      const live = blocksRef.current;
      const targetEl = (e.target as HTMLElement).closest<HTMLElement>('[data-block-id]');
      const idx = live.findIndex((b) => b.id === targetEl?.dataset.blockId);
      // 光标所在块是空的：让第一个粘贴块接管它，视觉上就是「粘在这里」
      const reuse = idx >= 0 && !!targetEl && (targetEl.textContent ?? '').trim() === '';
      const head = live.slice(0, reuse ? idx : idx < 0 ? live.length : idx + 1);
      const tail = reuse ? live.slice(idx + 1) : idx < 0 ? [] : live.slice(idx + 1);
      const next = [...head, ...parsed, ...tail];
      blocksRef.current = next;
      setBlocks(next);
      window.setTimeout(emit, 0);
    }, [emit, readMarkdown, onPasteFiles]);

    React.useImperativeHandle(ref, () => ({
      focus: () => {
        const last = rootRef.current?.querySelector<HTMLElement>('[data-block-id]:last-of-type');
        if (last) placeCaretAtEnd(last);
        else rootRef.current?.focus();
      },
      insertText: (text: string) => {
        const root = rootRef.current;
        if (!root) return;
        const sel = window.getSelection();
        const inEditor = document.activeElement instanceof Node && root.contains(document.activeElement);
        if (inEditor && sel && sel.rangeCount > 0) {
          const range = sel.getRangeAt(0);
          range.deleteContents();
          range.insertNode(range.createContextualFragment(inlineMdToHtml(text)));
          range.collapse(false);
          sel.removeAllRanges();
          sel.addRange(range);
          emit();
          return;
        }
        // 焦点不在编辑器里：追加一个新块
        readMarkdown();
        const next = [...blocksRef.current, { id: nextBlockId(), type: 'p' as BlockType, html: inlineMdToHtml(text) }];
        blocksRef.current = next;
        setBlocks(next);
        window.setTimeout(emit, 0);
      },
      getMarkdown: () => readMarkdown(),
    }), [emit, readMarkdown]);

    return (
      <div
        ref={rootRef}
        /* 带 `codex-md` 是为了拿到消息区那套排版变量与块样式（--md-fs 等） */
        className="codex-md codex-composer-editor"
        contentEditable
        role="textbox"
        aria-multiline="true"
        suppressContentEditableWarning
        /* data-empty 完全由 syncPlaceholder 直接写 dataset（见 syncPlaceholder 注释），
           这里不能交给 React：父组件一重渲染就会拿滞后的 value 覆盖掉，
           打字/输入法组合中的真实内容会被占位符叠住。首帧由挂载 layout effect 补齐。 */
        data-placeholder={placeholder ?? ''}
        onKeyDown={onKeyDown}
        onPaste={onPaste}
        onInput={emit}
        onCompositionStart={() => {
          composingRef.current = true;
          if (rootRef.current) rootRef.current.dataset.empty = 'false';
        }}
        onCompositionEnd={() => {
          composingRef.current = false;
          // 组合提交后 DOM 才是最终内容，立即同步占位符与 value
          emit();
        }}
      >
        {blocks.map((b) => (
          <BlockNode key={b.id} block={b} />
        ))}

        {bubble && createPortal(
          <div
            /* portal 到 body，带上 codex-theme 才拿得到主题变量 */
            className="codex-theme codex-composer-bubble"
            style={{ left: bubble.x, top: bubble.y }}
            onMouseDown={(e) => e.preventDefault()}
          >
            {linkMode ? (
              <span className="codex-composer-bubble-link">
                <input
                  autoFocus
                  value={linkDraft}
                  placeholder="https://"
                  onChange={(e) => setLinkDraft(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') { e.preventDefault(); applyLink(linkDraft.trim()); }
                    if (e.key === 'Escape') { e.preventDefault(); setLinkMode(false); }
                  }}
                />
                <button type="button" title={t('mind_inspector.code_fmt_link_apply')} onClick={() => applyLink(linkDraft.trim())}>
                  <Check size={13} />
                </button>
              </span>
            ) : (
              <>
                <button type="button" className="codex-composer-bubble-btn" title={t('mind_inspector.code_fmt_link')} onClick={() => setLinkMode(true)}>
                  <Link2 size={14} />
                </button>
                <button type="button" className="codex-composer-bubble-btn" title={t('mind_inspector.code_fmt_bold')} onClick={() => applyInline('bold')}>
                  <Bold size={14} />
                </button>
                <button type="button" className="codex-composer-bubble-btn" title={t('mind_inspector.code_fmt_italic')} onClick={() => applyInline('italic')}>
                  <Italic size={14} />
                </button>
                <span className="codex-composer-bubble-sep" />
                <div className="codex-composer-bubble-typewrap">
                  <button type="button" className="codex-composer-bubble-type" onClick={() => setMenuOpen((v) => !v)}>
                    {t(`mind_inspector.code_block_${
                      BLOCK_CHOICES.find(
                        (c) => c.type === (blocksRef.current.find((b) => b.id === bubble.blockId)?.type ?? 'p'),
                      )?.key ?? 'text'
                    }`)}
                    <ChevronDown size={12} />
                  </button>
                  {menuOpen && (
                    <div className="codex-composer-bubble-menu">
                      {BLOCK_CHOICES.map((c) => (
                        <button key={c.key} type="button" className="codex-composer-bubble-item" onClick={() => applyBlockType(c.type)}>
                          {t(`mind_inspector.code_block_${c.key}`)}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              </>
            )}
          </div>,
          document.body,
        )}
      </div>
    );
  },
);

// ============ 块节点 ============

const BlockNode: React.FC<{ block: EditorBlock }> = ({ block }) => {
  const setRef = useCallback((el: HTMLElement | null) => {
    if (!el || el.dataset.hydrated === '1') return;
    el.dataset.hydrated = '1';
    if (block.type === 'hr') return;
    if (block.type === 'code') el.textContent = block.html;
    else el.innerHTML = block.html;
  }, [block.html, block.type]);

  const common: Record<string, unknown> = {
    'data-block-id': block.id,
    'data-block-type': block.type,
    'data-lang': block.lang,
    ref: setRef,
    contentEditable: block.type !== 'hr',
    suppressContentEditableWarning: true,
    className: `codex-md-${block.type === 'p' ? 'p' : block.type}`,
  };

  switch (block.type) {
    case 'h1': return <h1 {...common} className="codex-md-h codex-md-h1" />;
    case 'h2': return <h2 {...common} className="codex-md-h codex-md-h2" />;
    case 'h3': return <h3 {...common} className="codex-md-h codex-md-h3" />;
    case 'h4': return <h4 {...common} className="codex-md-h codex-md-h4" />;
    case 'h5': return <h5 {...common} className="codex-md-h codex-md-h5" />;
    case 'h6': return <h6 {...common} className="codex-md-h codex-md-h6" />;
    case 'ul': return <ul {...common} />;
    case 'ol': return <ol {...common} />;
    case 'quote': return <blockquote {...common} className="codex-md-quote" />;
    case 'code': return <pre {...common} className="codex-md-pre" />;
    case 'table': return <table {...common} className="codex-md-table" />;
    case 'hr': return <hr {...common} className="codex-md-hr" />;
    default: return <p {...common} />;
  }
};

// ============ 辅助 ============

function placeCaretAtEnd(el: HTMLElement): void {
  const range = document.createRange();
  range.selectNodeContents(el);
  range.collapse(false);
  const sel = window.getSelection();
  sel?.removeAllRanges();
  sel?.addRange(range);
}

/** markdown → 块序列（粘贴解析与外部写入共用，规则与消息区渲染一致） */
export function markdownToBlocks(md: string): EditorBlock[] {
  const src = md ?? '';
  const out: EditorBlock[] = [];
  if (src.trim()) {
    for (const b of parseBlocks(src)) {
      switch (b.kind) {
        case 'h':
          out.push({
            id: nextBlockId(),
            type: (`h${Math.min(Math.max(b.level, 1), 6)}`) as BlockType,
            html: inlineMdToHtml(b.text),
          });
          break;
        case 'hr':
          out.push({ id: nextBlockId(), type: 'hr', html: '' });
          break;
        case 'quote':
          out.push({ id: nextBlockId(), type: 'quote', html: inlineMdToHtml(b.lines.join('\n')) });
          break;
        case 'list':
          out.push({ id: nextBlockId(), type: b.ordered ? 'ol' : 'ul', html: listItemsToHtml(b.items) });
          break;
        case 'code':
          out.push({ id: nextBlockId(), type: 'code', html: b.code, lang: b.lang });
          break;
        case 'table':
          out.push({ id: nextBlockId(), type: 'table', html: tableToHtml(b.header, b.rows) });
          break;
        default:
          out.push({ id: nextBlockId(), type: 'p', html: inlineMdToHtml(b.text).replace(/\n/g, '<br>') });
      }
    }
  }
  if (out.length === 0) out.push({ id: nextBlockId(), type: 'p', html: '' });
  return out;
}

interface ListItemLike {
  text: string;
  children: ListItemLike[];
}

/** 列表项树 → `<li>` 序列（嵌套列表原样带上） */
function listItemsToHtml(items: readonly ListItemLike[]): string {
  return items
    .map(
      (it) =>
        `<li>${inlineMdToHtml(it.text)}${
          it.children.length > 0 ? `<ul>${listItemsToHtml(it.children)}</ul>` : ''
        }</li>`,
    )
    .join('');
}

function tableToHtml(header: readonly string[], rows: readonly (readonly string[])[]): string {
  const th = header.map((c) => `<th>${inlineMdToHtml(c)}</th>`).join('');
  const tr = rows
    .map((r) => `<tr>${r.map((c) => `<td>${inlineMdToHtml(c)}</td>`).join('')}</tr>`)
    .join('');
  return `<thead><tr>${th}</tr></thead><tbody>${tr}</tbody>`;
}

export default ComposerEditor;
