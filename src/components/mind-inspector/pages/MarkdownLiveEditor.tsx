/**
 * MarkdownLiveEditor — markdown 预览的「所见即所得」就地编辑
 *
 * 与旧的「每块右上角一个铅笔 → 弹 textarea 改原文」不同：这里直接在渲染结果上
 * 打字/退格，并且边打边渲染 markdown 语法 —— 敲完 `**加粗**` 的最后一个 `*`，
 * 星号立刻消失、只剩加粗的「加粗」。
 *
 * ## 核心手法：DOM 里存的就是 markdown 原文
 *
 * 渲染时不丢弃语法标记，而是把它们包进 `<span class="md-mark">`（CSS `display:none`），
 * 格式交给 `<strong>` / `<h2>` / `<ul>` 这些结构元素承担。于是
 *
 *   - `textContent`（跳过 `data-md-ui` 子树）恰好等于原文 —— 不需要把 DOM 反推成
 *     markdown（那条路在嵌套列表缩进、转义字符、行尾空格上都是有损的，见
 *     `codeMarkdown.tsx` 里同一条结论）；
 *   - 光标、选区、退格全部是浏览器原生行为，不需要任何 offset 换算表；
 *   - 用户敲出完整的语法构造时，标记被包起来藏掉，格式随即出现。
 *
 * 纯渲染部分在 `markdownLiveHtml.ts`（不依赖 React/DOM，可单独跑测试）。
 *
 * ## 什么时候重渲染
 *
 * 判据是「重新生成的 HTML（先经浏览器解析再序列化归一）与当前 DOM 的 innerHTML
 * 是否一致」。纯打字时两者相等 —— 都是同一段文本节点 —— 于是不重渲染、光标天然
 * 不动；只有敲出或破坏一个语法构造时才重建 DOM 并把光标按原文偏移放回去。
 *
 * ## 已知取舍
 *
 * - 重渲染会重建 DOM，浏览器原生 undo 栈随之失效，所以这里自己维护撤销栈。
 * - 中文输入法组合期间（composition）不重渲染，否则会把候选词打断。
 * - 只把**真的变了**的块写回原文，避免用户改一段、整个文件被重排成规范形式。
 */

import React, { useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { MarkdownFileContext, parseBlocks } from './codeMarkdown';
import { docHtml } from './markdownLiveHtml';

// ============ DOM ↔ 原文 的偏移换算 ============
//
// 因为「textContent == 原文」，只需要按文档顺序累加文本节点长度即可。
// `data-md-ui` 子树是纯视觉件（复制按钮、勾选框），必须跳过，否则偏移会和原文错位。

/** 从 DOM 里读回 markdown 原文 */
function extractSource(root: HTMLElement): string {
  let out = '';
  const visit = (n: Node) => {
    if (n.nodeType === 3) {
      out += n.nodeValue ?? '';
      return;
    }
    if (n.nodeType !== 1) return;
    const el = n as HTMLElement;
    if (el.hasAttribute('data-md-ui')) return;
    if (el.tagName === 'BR') {
      out += '\n';
      return;
    }
    el.childNodes.forEach(visit);
  };
  visit(root);
  return out;
}

/** 某个节点贡献的原文长度 */
function sourceLength(n: Node): number {
  if (n.nodeType === 3) return (n.nodeValue ?? '').length;
  if (n.nodeType !== 1) return 0;
  const el = n as HTMLElement;
  if (el.hasAttribute('data-md-ui')) return 0;
  if (el.tagName === 'BR') return 1;
  let total = 0;
  el.childNodes.forEach((c) => { total += sourceLength(c); });
  return total;
}

/** 把光标所在的 (node, offset) 换算成原文里的字符偏移 */
function offsetOfPoint(root: HTMLElement, node: Node, offset: number): number {
  let total = 0;
  let result = -1;
  const visit = (n: Node) => {
    if (result >= 0) return;
    if (n === node) {
      if (n.nodeType === 3) {
        result = total + offset;
        return;
      }
      // 元素节点：offset 是子节点下标，累计它之前的兄弟
      for (const k of Array.from(n.childNodes).slice(0, offset)) {
        if (result >= 0) return;
        total += sourceLength(k);
      }
      result = total;
      return;
    }
    if (n.nodeType === 3) {
      total += (n.nodeValue ?? '').length;
      return;
    }
    if (n.nodeType !== 1) return;
    const el = n as HTMLElement;
    if (el.hasAttribute('data-md-ui')) return;
    if (el.tagName === 'BR') {
      total += 1;
      return;
    }
    el.childNodes.forEach((c) => { if (result < 0) visit(c); });
  };
  visit(root);
  return result >= 0 ? result : total;
}

/** 把原文偏移还原成光标位置 */
function setCaretAtOffset(root: HTMLElement, target: number): void {
  let total = 0;
  const visit = (n: Node): boolean => {
    if (n.nodeType === 3) {
      const len = (n.nodeValue ?? '').length;
      if (total + len >= target) {
        const range = document.createRange();
        range.setStart(n, Math.max(0, Math.min(len, target - total)));
        range.collapse(true);
        const sel = window.getSelection();
        sel?.removeAllRanges();
        sel?.addRange(range);
        return true;
      }
      total += len;
      return false;
    }
    if (n.nodeType !== 1) return false;
    const el = n as HTMLElement;
    if (el.hasAttribute('data-md-ui')) return false;
    if (el.tagName === 'BR') {
      total += 1;
      return false;
    }
    return Array.from(el.childNodes).some(visit);
  };
  if (visit(root)) return;
  // 偏移超出正文长度（例如撤销后变短）：落到末尾
  const range = document.createRange();
  range.selectNodeContents(root);
  range.collapse(false);
  const sel = window.getSelection();
  sel?.removeAllRanges();
  sel?.addRange(range);
}

/** 在当前光标处插入纯文本（Enter / 粘贴都走它，避免浏览器塞 <div>/<br> 破坏偏移） */
function insertPlainText(text: string): void {
  const sel = window.getSelection();
  if (!sel || sel.rangeCount === 0) return;
  const range = sel.getRangeAt(0);
  range.deleteContents();
  const node = document.createTextNode(text);
  range.insertNode(node);
  range.setStartAfter(node);
  range.collapse(true);
  sel.removeAllRanges();
  sel.addRange(range);
}

// ============ 组件 ============

/** 语法构造变化后的重渲染延迟：敲完最后一个 `*` 到加粗出现之间的观感间隔 */
const RENDER_DEBOUNCE_MS = 140;
/** 落盘延迟：比重渲染长得多，避免每敲一个字写一次盘 */
const SAVE_DEBOUNCE_MS = 900;
/** 撤销栈上限 */
const HISTORY_LIMIT = 200;

export const MarkdownLiveEditor: React.FC<{
  /** markdown 原文 */
  content: string;
  /** 落盘（宿主注入 `coding_write_file`）；抛错会被展示出来 */
  onSave: (next: string) => Promise<void>;
  /** 超大文件分页未完时禁用编辑：写盘会把没加载的部分截断 */
  disabled?: boolean;
  disabledNote?: string;
}> = ({ content, onSave, disabled = false, disabledNote }) => {
  const mdCtx = useContext(MarkdownFileContext);
  const onOpenFile = mdCtx.onOpenFile;

  const hostRef = useRef<HTMLDivElement | null>(null);
  /** 权威原文。不用 state：它每次击键都变，走 state 会把整棵树重渲染一遍 */
  const srcRef = useRef(content);
  /** 上一次画进 DOM 时各块的文本快照：用来判断「用户动了哪一块」 */
  const blockTextsRef = useRef<string[]>([]);
  const renderTimer = useRef<number | null>(null);
  const saveTimer = useRef<number | null>(null);
  const composingRef = useRef(false);
  /** 原文结尾原本有没有换行符：解析会把它丢掉，落盘时补回去 */
  const trailingNewline = useRef(content.endsWith('\n'));

  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 撤销栈：重渲染会重建 DOM，浏览器原生 undo 栈随之失效，只能自己记
  const historyRef = useRef<string[]>([content]);
  const historyIndex = useRef(0);

  const pushHistory = useCallback((snapshot: string) => {
    const hist = historyRef.current;
    if (hist[historyIndex.current] === snapshot) return;
    hist.splice(historyIndex.current + 1);
    hist.push(snapshot);
    if (hist.length > HISTORY_LIMIT) hist.shift();
    historyIndex.current = hist.length - 1;
  }, []);

  /** 按原文重建整篇 DOM，并把光标放回原处 */
  const paint = useCallback((caret: number | null) => {
    const host = hostRef.current;
    if (!host) return;
    const scrollTop = host.scrollTop;
    host.innerHTML = docHtml(srcRef.current, onOpenFile);
    host.scrollTop = scrollTop;
    const els = Array.from(host.querySelectorAll(':scope > [data-md-block]')) as HTMLElement[];
    blockTextsRef.current = els.map((el) => extractSource(el));
    if (caret !== null) setCaretAtOffset(host, caret);
  }, [onOpenFile]);

  /** 让重置逻辑始终拿到最新的 paint，又不把它的身份写进依赖里 */
  const paintRef = useRef(paint);
  paintRef.current = paint;

  /**
   * 首次挂载 / 外部内容变化时重建。
   *
   * 依赖刻意只留 `content`：`paint` 依赖 context 里的 `onOpenFile`，一旦它没被
   * memo 住就会每次渲染都变，写进依赖会把用户正在打的字整篇冲掉。
   *
   * 还要挡住「自己刚落盘、宿主把同一份内容回传回来」这种情况 —— 那时 DOM 已经是对的，
   * 再重建一次会把光标和滚动位置一起丢掉。
   */
  const lastPropRef = useRef(content);
  useEffect(() => {
    if (content === lastPropRef.current) return;
    lastPropRef.current = content;
    const norm = (s: string) => s.replace(/\n+$/, '');
    if (norm(content) === norm(srcRef.current)) return;
    srcRef.current = content;
    trailingNewline.current = content.endsWith('\n');
    historyRef.current = [content];
    historyIndex.current = 0;
    paintRef.current(null);
  }, [content]);

  // 首次挂载：把原文渲染进去（上面的重置逻辑只处理「内容变化」）
  useEffect(() => {
    paintRef.current(null);
  }, []);

  const flushSave = useCallback(async () => {
    const next = trailingNewline.current && !srcRef.current.endsWith('\n')
      ? `${srcRef.current}\n`
      : srcRef.current;
    setSaving(true);
    setError(null);
    try {
      await onSave(next);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }, [onSave]);

  const scheduleSave = useCallback(() => {
    if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
    saveTimer.current = window.setTimeout(() => { void flushSave(); }, SAVE_DEBOUNCE_MS);
  }, [flushSave]);

  /**
   * 语法构造可能变了 → 决定要不要重建 DOM。
   *
   * 先让浏览器把目标 HTML 解析再序列化一遍，再和现有 innerHTML 比：直接比字符串会
   * 栽在序列化规范化上（自闭合写成 `<hr/>` 会被吐回 `<hr>`、SVG 的 `<path/>` 会被
   * 吐回 `<path></path>`），于是每敲一下都误判成「结构变了」，白白重渲染、光标乱跳。
   */
  const scheduleRender = useCallback(() => {
    if (renderTimer.current !== null) window.clearTimeout(renderTimer.current);
    renderTimer.current = window.setTimeout(() => {
      const host = hostRef.current;
      if (!host || composingRef.current) return;
      const probe = document.createElement('div');
      probe.innerHTML = docHtml(srcRef.current, onOpenFile);
      if (host.innerHTML === probe.innerHTML) return;
      const sel = window.getSelection();
      const range = sel && sel.rangeCount > 0 ? sel.getRangeAt(0) : null;
      const caret = range && host.contains(range.startContainer)
        ? offsetOfPoint(host, range.startContainer, range.startOffset)
        : null;
      paint(caret);
    }, RENDER_DEBOUNCE_MS);
  }, [onOpenFile, paint]);

  const handleInput = useCallback(() => {
    const host = hostRef.current;
    if (!host || disabled) return;

    /*
     * 只把**真的变了**的那些块写回原文，其余块一个字节都不碰。
     *
     * 直接 `extractSource(host)` 取整篇也能跑，但那样会把整篇重排成规范形式：
     * 原文用 `*` 做项目符号会被改成 `-`、有序列表会被重新编号、表格单元格的空格
     * 会被压平 —— 用户只改了一段，却把整个文件重排了，很讨厌。
     *
     * 逐块对比的基准是 `blockTextsRef`（上一次画进 DOM 时的块文本）：没动过的块
     * 两者相等，直接跳过。替换从后往前做，免得前面块的行数变化把后面块的行号顶偏。
     */
    const els = Array.from(host.querySelectorAll(':scope > [data-md-block]')) as HTMLElement[];
    const oldBlocks = parseBlocks(srcRef.current);
    // `els.length > 0` 这个前置条件不能省：空文档（或纯空白文档）走的是 docHtml 的
    // 占位段 `<p><br></p>`，它**不带** `data-md-block`，于是 els 和 oldBlocks 同为 0，
    // 下面 `0 === 0` 会成立、循环体一次都不执行、touched 保持 false，srcRef 原地不动；
    // 紧接着 scheduleRender 看到 DOM 变了就把 innerHTML 重置回占位段 —— 用户敲的字被抹掉。
    if (els.length > 0 && els.length === oldBlocks.length && blockTextsRef.current.length === els.length) {
      const lines = srcRef.current.replace(/\r\n?/g, '\n').split('\n');
      let touched = false;
      for (let i = els.length - 1; i >= 0; i -= 1) {
        const next = extractSource(els[i]);
        if (next === blockTextsRef.current[i]) continue;
        const { start, end } = oldBlocks[i].span;
        lines.splice(start, end - start, ...next.split('\n'));
        touched = true;
      }
      if (touched) srcRef.current = lines.join('\n');
    } else {
      // 块数变了（刚敲出一个空行、把一段拆成两段），或本来就是空文档（占位段没有块壳）：
      // 整篇取回，接受一次规范化
      srcRef.current = extractSource(host);
    }

    pushHistory(srcRef.current);
    scheduleRender();
    scheduleSave();
  }, [disabled, pushHistory, scheduleRender, scheduleSave]);

  const undo = useCallback((dir: -1 | 1) => {
    const hist = historyRef.current;
    const next = historyIndex.current + dir;
    if (next < 0 || next >= hist.length) return;
    historyIndex.current = next;
    srcRef.current = hist[next];
    paint(null);
    scheduleSave();
  }, [paint, scheduleSave]);

  const onKeyDown = useCallback((e: React.KeyboardEvent<HTMLDivElement>) => {
    if (disabled) return;
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === 'z') {
      e.preventDefault();
      undo(e.shiftKey ? 1 : -1);
      return;
    }
    if (mod && e.key.toLowerCase() === 'y') {
      e.preventDefault();
      undo(1);
      return;
    }
    // Enter 自己插一个换行文本节点：交给浏览器会塞 <div>/<br>，破坏偏移换算
    if (e.key === 'Enter') {
      e.preventDefault();
      insertPlainText('\n');
      handleInput();
    }
  }, [disabled, undo, handleInput]);

  const onPaste = useCallback((e: React.ClipboardEvent<HTMLDivElement>) => {
    if (disabled) return;
    e.preventDefault();
    insertPlainText(e.clipboardData.getData('text/plain'));
    handleInput();
  }, [disabled, handleInput]);

  /** 视觉件的点击走事件委托：innerHTML 里注入的节点没有 React 处理器 */
  const onClick = useCallback((e: React.MouseEvent<HTMLDivElement>) => {
    const el = (e.target as HTMLElement).closest('[data-file-path], [data-copy-block]') as HTMLElement | null;
    if (!el) return;
    e.preventDefault();
    if (el.dataset.copyBlock !== undefined) {
      const code = el.closest('.codex-md-codeblock')?.querySelector('pre code');
      // 复制正文，去掉 md-mark 里的两行围栏
      const text = code ? extractSource(code as HTMLElement) : '';
      void navigator.clipboard?.writeText(text.replace(/^```[^\n]*\n/, '').replace(/\n```$/, ''));
      return;
    }
    const path = el.dataset.filePath;
    if (path) onOpenFile?.(path);
  }, [onOpenFile]);

  const isEmpty = useMemo(() => parseBlocks(content).length === 0, [content]);

  return (
    <div className="md-live-wrap">
      {/* 状态条只在**有稳定理由**时才占位（目前只有 disabled）。
          「保存中…」是毫秒级的瞬时状态（写本地文件），若也走这条占位条，
          它一出现/消失就会把正文顶下去 ~21px，每存一次抖一下 —— 所以改成
          不占布局的浮层（.md-live-saving-float）。 */}
      {disabled && disabledNote && (
        <div className="md-live-status">
          <span className="md-live-disabled">{disabledNote}</span>
        </div>
      )}
      {saving && (
        <span className="md-live-saving md-live-saving-float">
          <Loader2 size={11} className="codex-spin" /> 保存中…
        </span>
      )}
      {error && <div className="codex-src-error">{error}</div>}
      <div
        ref={hostRef}
        className="md-live codex-md"
        contentEditable={!disabled}
        suppressContentEditableWarning
        spellCheck={false}
        role="textbox"
        aria-multiline="true"
        aria-label="编辑 markdown"
        onInput={handleInput}
        onKeyDown={onKeyDown}
        onPaste={onPaste}
        onClick={onClick}
        onBlur={() => { void flushSave(); }}
        onCompositionStart={() => { composingRef.current = true; }}
        onCompositionEnd={() => { composingRef.current = false; handleInput(); }}
      />
      {isEmpty && <div className="codex-src-more">（空文档，直接输入即可）</div>}
    </div>
  );
};

export default MarkdownLiveEditor;
