/**
 * 工作侧 Markdown 渲染
 *
 * 智能体回复以 markdown 为主，页面需要的是排版而不是原文。这里把 markdown
 * 解析成 React 节点（不拼 HTML 字符串、不走 dangerouslySetInnerHTML），
 * 既避开注入面，也让样式全部落在 CSS 类上。
 *
 * 排版尺度对齐对话阅读场景：字号 F 为基准，行距取 1.6F，块间距 S = F/4，
 * 标题按 1.5 / 1.25 / 1.125 / 1 倍递减，代码块与引用块留白更大。
 *
 * 链接分两类：指向本地文件的（绝对路径 / file:// / 带扩展名的相对路径）渲染成
 * 文件卡片并走宿主回调在侧边栏打开；其余按外链处理。
 */

import React, { useCallback, useContext, useState } from 'react';
import {
  Braces, Check, File as FileIcon, FileCode, FileText, Hash, Image as ImageIcon,
} from 'lucide-react';

// ============ 本地文件链接 ============

export interface MarkdownFileTarget {
  path: string;
  line?: number;
  column?: number;
}

/**
 * 宿主提供的文件打开能力；未提供时本地文件链接退化为普通外链。
 *
 * 链接里给的可能是**相对工作目录**的路径，拼成绝对路径是宿主 `onOpenFile` 的职责
 * （打开面板时按会话工作目录还原），这里不做路径拼接。
 */
export const MarkdownFileContext = React.createContext<{
  onOpenFile?: (path: string, line?: number, column?: number) => void;
}>({});

const RE_FILE_URL = /^file:\/\//i;
const RE_WIN_ABS = /^[A-Za-z]:[\\/]/;
const RE_UNC = /^\\\\/;

/** 带扩展名的相对路径才当文件看待，避免把 `[文档](guide/index)` 这类站内链接误判 */
const FILE_EXTS = new Set([
  'ts', 'tsx', 'js', 'jsx', 'mjs', 'cjs', 'json', 'css', 'scss', 'less', 'sass',
  'html', 'htm', 'md', 'mdx', 'txt', 'yml', 'yaml', 'toml', 'ini', 'env',
  'py', 'rs', 'go', 'java', 'kt', 'c', 'h', 'cc', 'cpp', 'hpp', 'cs', 'rb', 'php', 'swift',
  'sh', 'bash', 'zsh', 'ps1', 'bat', 'cmd', 'sql', 'vue', 'svelte',
  'png', 'jpg', 'jpeg', 'webp', 'gif', 'svg', 'bmp', 'ico', 'avif',
  'mp3', 'wav', 'ogg', 'mp4', 'webm', 'mov',
  'glb', 'gltf', 'obj', 'fbx', 'atlas', 'ttf', 'woff', 'woff2',
]);

/** 从链接目标判断并规整出本地文件路径与可选行列；不是本地文件时返回 null。 */
export function localFileTarget(href: string): MarkdownFileTarget | null {
  let raw = href.trim().replace(/^<|>$/g, '');
  if (!raw || raw.includes('://') && !RE_FILE_URL.test(raw)) return null;
  if (RE_FILE_URL.test(raw)) {
    raw = decodeURI(raw.replace(RE_FILE_URL, '')).replace(/^\/([A-Za-z]:)/, '$1');
  }

  let path = raw;
  let line: number | undefined;
  let column: number | undefined;
  const hashLocation = /^(.*)#L(\d+)(?:C(\d+))?$/i.exec(path);
  const colonLocation = /^(.*\.[A-Za-z0-9_-]+):(\d+)(?::(\d+))?$/.exec(path);
  const location = hashLocation ?? colonLocation;
  if (location) {
    path = location[1];
    line = Number(location[2]);
    column = location[3] ? Number(location[3]) : undefined;
  }

  if (RE_WIN_ABS.test(path) || RE_UNC.test(path) || path.startsWith('/')) {
    return { path, line, column };
  }
  // 相对路径：必须以已知扩展名结尾。
  // 不要求含分隔符——`[Cargo.toml](Cargo.toml)` 这类根目录文件同样要能点开；
  // 无扩展名的 `[文档](guide/index)` 已被扩展名白名单挡掉。
  // 还原成绝对路径由宿主的 onOpenFile 负责（按会话工作目录拼接）。
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  return FILE_EXTS.has(ext) ? { path, line, column } : null;
}

/** 兼容已有调用者：只取不带行列后缀的文件路径。 */
export function localFilePath(href: string): string | null {
  return localFileTarget(href)?.path ?? null;
}

/** 按扩展名挑图标，贴合文件树里同类文件的观感 */
function fileIconFor(path: string): React.ElementType {
  const ext = (path.split('.').pop() ?? '').toLowerCase();
  if (['png', 'jpg', 'jpeg', 'webp', 'gif', 'svg', 'bmp', 'ico', 'avif'].includes(ext)) return ImageIcon;
  if (['ts', 'tsx', 'js', 'jsx', 'mjs', 'cjs', 'vue', 'svelte', 'py', 'rs', 'go', 'java', 'kt', 'c', 'h', 'cc', 'cpp', 'hpp', 'cs', 'rb', 'php', 'swift'].includes(ext)) return FileCode;
  if (['css', 'scss', 'less', 'sass'].includes(ext)) return Hash;
  if (['json', 'yaml', 'yml', 'toml', 'ini', 'env'].includes(ext)) return Braces;
  if (['md', 'mdx', 'txt', 'rst'].includes(ext)) return FileText;
  return FileIcon;
}

function fileKindFor(path: string): 'image' | 'code' | 'style' | 'data' | 'text' | 'file' {
  const ext = (path.split('.').pop() ?? '').toLowerCase();
  if (['png', 'jpg', 'jpeg', 'webp', 'gif', 'svg', 'bmp', 'ico', 'avif'].includes(ext)) return 'image';
  if (['css', 'scss', 'less', 'sass'].includes(ext)) return 'style';
  if (['json', 'yaml', 'yml', 'toml', 'ini', 'env'].includes(ext)) return 'data';
  if (['md', 'mdx', 'txt', 'rst'].includes(ext)) return 'text';
  if (FILE_EXTS.has(ext)) return 'code';
  return 'file';
}

// ============ 行内 ============

/** ASCII 字母数字：用于判定星号是"强调定界符"还是"运算符/指数" */
const isAsciiAlnum = (c: string): boolean => /[A-Za-z0-9]/.test(c);

/**
 * `*` / `**` 两侧都是 ASCII 字母数字时按运算符处理（`2*3*4`、`2**3`），
 * 不当强调定界符——代码类回复里这类表达式比强调更常见。
 * 中文两侧不受限制，`这是*斜体*文字` 仍正常渲染。
 */
const looksLikeOperator = (prev: string, next: string): boolean =>
  isAsciiAlnum(prev) && isAsciiAlnum(next);

/** 内联渲染上下文就是 `MarkdownFileContext` 的值，直接从它派生以免两处定义漂移。 */
type InlineCtx = React.ContextType<typeof MarkdownFileContext>;

/**
 * 行内语法：`code`、**粗**、*斜*、~~删除~~、[文字](链接)。
 *
 * 有意不解析 `_` 系（`_斜_` / `__粗__`）：标识符里的下划线太常见
 * （`some_var_name`、`__init__`），误伤成本高于收益。
 */
function renderInline(text: string, kp: string, ctx: InlineCtx): React.ReactNode[] {
  const out: React.ReactNode[] = [];
  let buf = '';
  let n = 0;
  let i = 0;

  const flush = () => {
    if (buf) {
      out.push(buf);
      buf = '';
    }
  };

  while (i < text.length) {
    const ch = text[i];
    const prevChar = i === 0 ? '' : text[i - 1];

    // 行内代码
    if (ch === '`') {
      const end = text.indexOf('`', i + 1);
      if (end > i + 1) {
        flush();
        out.push(
          <code key={`c${kp}${n++}`} className="codex-md-code">
            {text.slice(i + 1, end)}
          </code>,
        );
        i = end + 1;
        continue;
      }
    }

    // 粗体 / 删除线（两字符定界符）
    if (ch === '*' || ch === '~') {
      const two = text.slice(i, i + 2);
      if (two === '**' || two === '~~') {
        const close = text.indexOf(two, i + 2);
        const opLike = two === '**' && looksLikeOperator(prevChar, text[i + 2] ?? '');
        if (close > i + 2 && !opLike) {
          flush();
          const inner = renderInline(text.slice(i + 2, close), `${kp}b${n++}`, ctx);
          out.push(
            two === '**'
              ? <strong key={`b${kp}${n}`}>{inner}</strong>
              : <del key={`d${kp}${n}`}>{inner}</del>,
          );
          i = close + 2;
          continue;
        }
        // 构不成强调（`2**3` 这类指数或无闭合）：整对定界符按字面量消费。
        // 否则第二个星号会落进下面的斜体分支，把后续整段文本吞成斜体。
        buf += two;
        i += 2;
        continue;
      }
    }

    // 斜体（单星号；两侧不能是空白，且避开乘法表达式）
    if (ch === '*') {
      const close = text.indexOf('*', i + 1);
      if (
        close > i + 1
        && text[i + 1] !== ' '
        && text[close - 1] !== ' '
        && !looksLikeOperator(prevChar, text[i + 1])
      ) {
        flush();
        out.push(<em key={`i${kp}${n++}`}>{renderInline(text.slice(i + 1, close), `${kp}i${n}`, ctx)}</em>);
        i = close + 1;
        continue;
      }
    }

    // 链接：本地文件走卡片 + 宿主回调，其余按外链
    if (ch === '[') {
      const m = /^\[([^\]]*)\]\((<[^>]+>|[^)]+)\)/.exec(text.slice(i));
      if (m) {
        const label = m[1] || m[2];
        const href = m[2].trim().replace(/^<|>$/g, '');
        const fileTarget = localFileTarget(href);
        const filePath = fileTarget?.path ?? null;
        flush();
        if (filePath && ctx.onOpenFile) {
          const Icon = fileIconFor(filePath);
          out.push(
            <button
              key={`f${kp}${n++}`}
              type="button"
              className="codex-md-filelink"
              data-file-kind={fileKindFor(filePath)}
              title={`${filePath}${fileTarget?.line ? `:${fileTarget.line}` : ''}${fileTarget?.column ? `:${fileTarget.column}` : ''}`}
              onClick={() => ctx.onOpenFile?.(filePath, fileTarget?.line, fileTarget?.column)}
            >
              <span className="codex-md-filelink-icon"><Icon size={13} strokeWidth={2} /></span>
              <span className="codex-md-filelink-text">{label}</span>
            </button>,
          );
        } else {
          out.push(
            <a
              key={`a${kp}${n++}`}
              className="codex-md-link"
              href={filePath ? '#' : href}
              target={filePath ? undefined : '_blank'}
              rel="noreferrer noopener"
              title={href}
              onClick={filePath ? (e) => e.preventDefault() : undefined}
            >
              {label}
            </a>,
          );
        }
        i += m[0].length;
        continue;
      }
    }

    buf += ch;
    i += 1;
  }

  flush();
  return out;
}

const inline = (text: string, kp: string, ctx: InlineCtx): React.ReactNode => (
  <>{renderInline(text, kp, ctx)}</>
);

// ============ 代码块 ============

/** 代码块：语言标签 + 复制。复制状态只服务本块，不向上冒泡。 */
const CodeBlock: React.FC<{ lang: string; code: string }> = ({ lang, code }) => {
  const [copied, setCopied] = useState(false);
  const copy = useCallback(() => {
    void navigator.clipboard?.writeText(code).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    }).catch(() => { /* 剪贴板不可用时静默 */ });
  }, [code]);

  return (
    <div className="codex-md-codeblock">
      <div className="codex-md-codeblock-head">
        <span className="codex-md-codeblock-lang">{lang || 'text'}</span>
        <button type="button" className="codex-md-codeblock-copy" onClick={copy}>
          {copied ? '已复制' : '复制'}
        </button>
      </div>
      <pre className="codex-md-pre">
        <code>{code}</code>
      </pre>
    </div>
  );
};

// ============ 块级解析 ============

/** `checked` 为 null 表示普通条目，true / false 表示任务清单条目 */
type ListNode = { text: string; checked: boolean | null; children: ListNode[] };

type Block =
  | { kind: 'p'; text: string }
  | { kind: 'h'; level: number; text: string }
  | { kind: 'hr' }
  | { kind: 'quote'; lines: string[] }
  | { kind: 'list'; ordered: boolean; items: ListNode[] }
  | { kind: 'code'; lang: string; code: string }
  | { kind: 'table'; header: string[]; rows: string[][] };

const RE_FENCE = /^\s*```\s*([A-Za-z0-9+#._-]*)\s*$/;
const RE_HEADING = /^(#{1,6})\s+(.*)$/;
const RE_HR = /^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/;
const RE_QUOTE = /^\s*>\s?(.*)$/;
const RE_ITEM = /^(\s*)([-*+]|(\d+)[.)])\s+(.*)$/;
const RE_TABLE_SEP = /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/;

/** 拆分表格行：去掉首尾竖线后按 `|` 切分（不处理转义竖线，够用且不误伤） */
function splitRow(line: string): string[] {
  const t = line.trim().replace(/^\|/, '').replace(/\|$/, '');
  return t.split('|').map((c) => c.trim());
}

/** 任务清单标记：`[ ]` 未完成 / `[x]` 已完成 */
const RE_TASK = /^\[([ xX])\]\s+(.*)$/;

/** 列表行 → 树。缩进每 2 空格算一级，逐行挂到对应层级。 */
function buildList(rows: { indent: number; text: string }[]): ListNode[] {
  const roots: ListNode[] = [];
  const stack: { indent: number; node: ListNode }[] = [];
  for (const row of rows) {
    const task = RE_TASK.exec(row.text);
    const node: ListNode = {
      text: task ? task[2] : row.text,
      checked: task ? task[1] !== ' ' : null,
      children: [],
    };
    while (stack.length > 0 && stack[stack.length - 1].indent >= row.indent) stack.pop();
    if (stack.length === 0) roots.push(node);
    else stack[stack.length - 1].node.children.push(node);
    stack.push({ indent: row.indent, node });
  }
  return roots;
}

function parseBlocks(src: string): Block[] {
  const lines = src.replace(/\r\n?/g, '\n').split('\n');
  const blocks: Block[] = [];
  let i = 0;

  while (i < lines.length) {
    const line = lines[i];

    if (!line.trim()) {
      i += 1;
      continue;
    }

    // 围栏代码块：未闭合时把余下内容整体当代码（流式输出会走到这里）
    const fence = RE_FENCE.exec(line);
    if (fence) {
      const lang = fence[1] || '';
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !/^\s*```/.test(lines[i])) {
        body.push(lines[i]);
        i += 1;
      }
      if (i < lines.length) i += 1; // 跳过收尾围栏
      blocks.push({ kind: 'code', lang, code: body.join('\n') });
      continue;
    }

    const heading = RE_HEADING.exec(line);
    if (heading) {
      blocks.push({ kind: 'h', level: heading[1].length, text: heading[2].trim() });
      i += 1;
      continue;
    }

    if (RE_HR.test(line)) {
      blocks.push({ kind: 'hr' });
      i += 1;
      continue;
    }

    // 引用块：连续 `>` 行合并，内部再按块解析
    if (RE_QUOTE.test(line)) {
      const inner: string[] = [];
      while (i < lines.length && RE_QUOTE.test(lines[i])) {
        inner.push(RE_QUOTE.exec(lines[i])![1]);
        i += 1;
      }
      blocks.push({ kind: 'quote', lines: inner });
      continue;
    }

    // 表格：本行含 `|` 且下一行是分隔行
    if (line.includes('|') && i + 1 < lines.length && RE_TABLE_SEP.test(lines[i + 1])) {
      const header = splitRow(line);
      const rows: string[][] = [];
      i += 2;
      while (i < lines.length && lines[i].includes('|') && lines[i].trim()) {
        rows.push(splitRow(lines[i]));
        i += 1;
      }
      blocks.push({ kind: 'table', header, rows });
      continue;
    }

    // 列表：连续列表行（含空行后的续行归上一项不处理，保持简单）
    const item = RE_ITEM.exec(line);
    if (item) {
      const ordered = /^\d/.test(item[2]);
      const rows: { indent: number; text: string }[] = [];
      while (i < lines.length) {
        const m = RE_ITEM.exec(lines[i]);
        if (!m) break;
        if (/^\d/.test(m[2]) !== ordered) break;
        rows.push({ indent: m[1].replace(/\t/g, '  ').length, text: m[4] });
        i += 1;
      }
      blocks.push({ kind: 'list', ordered, items: buildList(rows) });
      continue;
    }

    // 段落：吃到空行或下一个块级起点
    const para: string[] = [line];
    i += 1;
    while (i < lines.length) {
      const cur = lines[i];
      if (!cur.trim()) break;
      if (RE_FENCE.test(cur) || RE_HEADING.test(cur) || RE_HR.test(cur) || RE_QUOTE.test(cur) || RE_ITEM.test(cur)) break;
      if (cur.includes('|') && i + 1 < lines.length && RE_TABLE_SEP.test(lines[i + 1])) break;
      para.push(cur);
      i += 1;
    }
    blocks.push({ kind: 'p', text: para.join('\n') });
  }

  return blocks;
}

const renderList = (items: ListNode[], ordered: boolean, kp: string, ctx: InlineCtx): React.ReactNode => {
  const Tag = ordered ? 'ol' : 'ul';
  const isTaskList = !ordered && items.some((it) => it.checked !== null);
  return (
    <Tag className={`${ordered ? 'codex-md-ol' : 'codex-md-ul'}${isTaskList ? ' codex-md-tasklist' : ''}`}>
      {items.map((it, idx) => {
        const sub = it.children.length > 0 ? renderList(it.children, false, `${kp}c${idx}`, ctx) : null;
        const key = `li${kp}${idx}`;
        if (it.checked === null) {
          return (
            <li key={key} className="codex-md-li">
              {inline(it.text, `${kp}l${idx}`, ctx)}
              {sub}
            </li>
          );
        }
        return (
          <li key={key} className="codex-md-li codex-md-task" data-checked={it.checked ? '1' : undefined}>
            <span className="codex-md-taskbox">{it.checked ? <Check size={11} strokeWidth={3} /> : null}</span>
            <div className="codex-md-task-body">
              {inline(it.text, `${kp}l${idx}`, ctx)}
              {sub}
            </div>
          </li>
        );
      })}
    </Tag>
  );
};

/**
 * 块序列 → React 节点。引用块内部递归复用，保证嵌套结构也能渲染。
 *
 * `animate` 打开时给每个块挂递增的入场延迟，流式输出下新块逐个淡入；
 * 块 key 由下标决定，已存在的块 React 复用 DOM，动画不会重放。
 */
function renderBlocks(blocks: Block[], kp: string, ctx: InlineCtx, animate = false): React.ReactNode[] {
  return blocks.map((b, idx) => {
    const key = `${kp}-${idx}`;
    const style = animate ? { animationDelay: `${Math.min(idx, 10) * 26}ms` } : undefined;
    switch (b.kind) {
      case 'h': {
        const Tag = (`h${Math.min(b.level, 6)}`) as 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6';
        return <Tag key={key} style={style} className={`codex-md-h codex-md-h${b.level}`}>{inline(b.text, key, ctx)}</Tag>;
      }
      case 'hr':
        return <hr key={key} style={style} className="codex-md-hr" />;
      case 'quote':
        return (
          <blockquote key={key} style={style} className="codex-md-quote">
            {renderBlocks(parseBlocks(b.lines.join('\n')), `${key}q`, ctx, animate)}
          </blockquote>
        );
      case 'list':
        return <React.Fragment key={key}>{renderList(b.items, b.ordered, key, ctx)}</React.Fragment>;
      case 'code':
        return <CodeBlock key={key} lang={b.lang} code={b.code} />;
      case 'table':
        return (
          <div key={key} style={style} className="codex-md-table-wrap">
            <table className="codex-md-table">
              <thead>
                <tr>
                  {b.header.map((c, j) => (
                    <th key={`${key}th${j}`} className="codex-md-th">{inline(c, `${key}th${j}`, ctx)}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {b.rows.map((row, r) => (
                  <tr key={`${key}tr${r}`} className="codex-md-tr">
                    {row.map((c, j) => (
                      <td key={`${key}td${r}-${j}`} className="codex-md-td">{inline(c, `${key}td${r}-${j}`, ctx)}</td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        );
      default:
        return <p key={key} style={style} className="codex-md-p">{inline(b.text, key, ctx)}</p>;
    }
  });
}

/**
 * 渲染 markdown 正文。
 *
 * `keyPrefix` 用于区分同一页面里的多段文本，避免同层 key 冲突。
 * `animate` 仅流式输出时打开：逐块淡入，历史消息保持静态。
 * 本地文件链接的打开能力由外层 `MarkdownFileContext` 提供。
 */
export const MarkdownText: React.FC<{ text: string; keyPrefix?: string; animate?: boolean }> = ({
  text, keyPrefix = 'md', animate = false,
}) => {
  const ctx = useContext(MarkdownFileContext);
  return (
    <div className="codex-md" data-md-animated={animate ? '1' : undefined}>
      {renderBlocks(parseBlocks(text || ''), keyPrefix, ctx, animate)}
    </div>
  );
};

export default MarkdownText;

/**
 * 单行文件引用块：类型标 + 文件名（+ 可选增删行数），点击打开文件。
 *
 * 正文里的文件链接与回复末尾的「修改文件」清单共用这一个组件，保证两处观感一致。
 * 打开能力取自 `MarkdownFileContext`，相对路径由宿主按会话工作目录还原。
 */
export const FileChip: React.FC<{
  path: string;
  line?: number;
  added?: number;
  removed?: number;
}> = ({ path, line, added, removed }) => {
  const { onOpenFile } = useContext(MarkdownFileContext);
  const Icon = fileIconFor(path);
  const name = path.split(/[\\/]/).pop() || path;
  const hasStats = added !== undefined || removed !== undefined;
  return (
    <button
      type="button"
      className="codex-md-filelink"
      data-file-kind={fileKindFor(path)}
      title={`${path}${line ? `:${line}` : ''}`}
      onClick={() => onOpenFile?.(path, line)}
      disabled={!onOpenFile}
    >
      <span className="codex-md-filelink-icon"><Icon size={13} strokeWidth={2} /></span>
      <span className="codex-md-filelink-text">{name}</span>
      {line !== undefined && <span className="codex-filelink-line">(line {line})</span>}
      {hasStats && (
        <span className="codex-filelink-stats">
          {added ? <span className="codex-filelink-add">+{added}</span> : null}
          {removed ? <span className="codex-filelink-del">-{removed}</span> : null}
        </span>
      )}
    </button>
  );
};
