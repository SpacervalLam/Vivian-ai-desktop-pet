/**
 * markdown 预览就地编辑的**纯渲染层**（不依赖 React / DOM，可单独测）。
 *
 * 这一层的全部职责是把 markdown 原文变成一段 HTML，并且守住一条不变量：
 *
 *   > 这段 HTML 的 textContent（跳过 `data-md-ui` 子树）必须**逐字符等于**输入的原文。
 *
 * 就地编辑靠的就是它 —— DOM 里存着原文，光标/退格交给浏览器原生行为，
 * 不需要把 DOM 反推成 markdown（那条路在嵌套列表缩进、转义、行尾空格上有损）。
 *
 * 实现手法：语法标记不丢弃，而是包进 `<span class="md-mark">`（CSS `display:none`），
 * 格式交给 `<strong>` / `<h2>` / `<ul>` 这些结构元素承担。
 *
 * ## 与 codeMarkdown 的关系
 *
 * 行内语法（`` ` `` / `**` / `~~` / `*` / `[..](..)`）与块级切分必须与
 * `codeMarkdown.tsx` 的 `renderInline` / `parseBlocks` 保持一致，否则
 * 「编辑态看到的」和「聊天区渲染的」会对不上。**改行内语法时两边都要改。**
 * 这里另写一份字符串版，是因为要往产物里插 `md-mark` 标记与 `data-file-path`。
 */

import { localFileTarget, parseBlocks, type Block } from './codeMarkdown';

// ============ 行内 ============

const isAsciiAlnum = (c: string): boolean => /[A-Za-z0-9]/.test(c);

/** `*` / `**` 两侧都是 ASCII 字母数字时按运算符处理（`2*3*4`、`2**3`），与聊天区一致 */
export const looksLikeOperator = (prev: string, next: string): boolean =>
  isAsciiAlnum(prev) && isAsciiAlnum(next);

export const escapeHtml = (s: string): string =>
  s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

/**
 * 语法标记：默认 `display:none`，唯一作用是让 `textContent` 能还原出原文。
 *
 * **不能**打 `data-md-ui` —— 那个属性是「这段不是正文」的排除标记，
 * 而标记恰恰属于正文，只是不显示。
 */
export const mark = (s: string): string => `<span class="md-mark">${escapeHtml(s)}</span>`;

/** 视觉件（不参与正文）：复制按钮、任务勾选框、水平线本体 */
export const UI = 'data-md-ui="1" contenteditable="false"';

export type LiveListNode = { text: string; checked: boolean | null; children: LiveListNode[] };

/** 行内 markdown → HTML（含 md-mark 标记） */
export function inlineHtml(text: string, onOpenFile?: (path: string) => void): string {
  let out = '';
  let buf = '';
  let i = 0;
  const flush = () => {
    if (buf) {
      out += escapeHtml(buf);
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
        out += `${mark('`')}<code class="codex-md-code">${escapeHtml(text.slice(i + 1, end))}</code>${mark('`')}`;
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
          const inner = inlineHtml(text.slice(i + 2, close), onOpenFile);
          const tag = two === '**' ? 'strong' : 'del';
          out += `${mark(two)}<${tag}>${inner}</${tag}>${mark(two)}`;
          i = close + 2;
          continue;
        }
        // 构不成强调：整对定界符按字面量消费，避免第二个星号落进斜体分支
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
        out += `${mark('*')}<em>${inlineHtml(text.slice(i + 1, close), onOpenFile)}</em>${mark('*')}`;
        i = close + 1;
        continue;
      }
    }

    // 链接：`[label](href)`。标记拆成 `[` 与 `](href)` 两段夹住真正的内容，
    // 这样拼起来的 textContent 仍等于原文。
    if (ch === '[') {
      const m = /^\[([^\]]*)\]\((<[^>]+>|[^)]+)\)/.exec(text.slice(i));
      if (m) {
        const label = m[1] || m[2];
        const href = m[2].trim().replace(/^<|>$/g, '');
        const filePath = localFileTarget(href)?.path ?? null;
        flush();
        if (filePath && onOpenFile) {
          out += `${mark('[')}<button type="button" class="codex-md-filelink" contenteditable="false" data-file-path="${escapeHtml(filePath)}" title="${escapeHtml(filePath)}"><span class="codex-md-filelink-text">${escapeHtml(label)}</span></button>${mark(`](${href})`)}`;
        } else {
          out += `${mark('[')}<a class="codex-md-link" href="${filePath ? '#' : escapeHtml(href)}" title="${escapeHtml(href)}">${escapeHtml(label)}</a>${mark(`](${href})`)}`;
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

// ============ 块级 ============

const bulletFor = (ordered: boolean, index: number): string => (ordered ? `${index + 1}. ` : '- ');

/** 列表 → HTML。缩进按每级 2 空格回写，与 parseBlocks 的层级判定一致。 */
export function listHtml(
  items: LiveListNode[],
  ordered: boolean,
  depth: number,
  onOpenFile?: (p: string) => void,
): string {
  const tag = ordered ? 'ol' : 'ul';
  const cls = ordered ? 'codex-md-ol' : 'codex-md-ul';
  const isTask = items.some((it) => it.checked !== null);
  const parts = items.map((it, idx) => {
    const isItemTask = it.checked !== null;
    const task = isItemTask ? (it.checked ? '[x] ' : '[ ] ') : '';
    const head = mark(`${'  '.repeat(depth)}${bulletFor(ordered, idx)}${task}`);
    const box = isItemTask
      ? `<span class="codex-md-taskbox" ${UI}>${it.checked ? '<svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3"><path d="M20 6 9 17l-5-5"></path></svg>' : ''}</span>`
      : '';
    const sub = it.children.length
      ? `${mark('\n')}${listHtml(it.children, false, depth + 1, onOpenFile)}`
      : '';
    const body = `${inlineHtml(it.text, onOpenFile)}${sub}`;
    // 任务条目要挂 `codex-md-task`（CSS 的 flex 布局与勾选框底色都挂在它上面），
    // 正文另外包一层 `codex-md-task-body`，否则文字不参与 flex、会被挤成一列
    const itemCls = `codex-md-li${isItemTask ? ' codex-md-task' : ''}`;
    const attr = it.checked ? ' data-checked="1"' : '';
    return `<li class="${itemCls}"${attr}>${head}${box}${isItemTask ? `<div class="codex-md-task-body">${body}</div>` : body}</li>`;
  });
  return `<${tag} class="${cls}${isTask ? ' codex-md-tasklist' : ''}">${parts.join(mark('\n'))}</${tag}>`;
}

/** 单个块 → HTML（含块级前缀标记，如 `## `、`> `、``` 围栏） */
export function blockHtml(b: Block, onOpenFile?: (p: string) => void): string {
  switch (b.kind) {
    case 'h': {
      const lvl = Math.min(b.level, 6);
      return `<h${lvl} class="codex-md-h codex-md-h${b.level}">${mark(`${'#'.repeat(b.level)} `)}${inlineHtml(b.text, onOpenFile)}</h${lvl}>`;
    }
    case 'hr':
      return `${mark('---')}<hr class="codex-md-hr" ${UI}>`;
    case 'quote':
      // 每行套一层 `md-line`（CSS `display:block`）来断行。
      // **不能只靠 mark('\n') 断行** —— md-mark 是 display:none，它里面的换行符不产生
      // 任何断行，多行引用会挤成一行（聊天区没这问题，因为那边是把引用内容重新
      // parseBlocks 成块来渲染的）。换行标记仍留在行内，只为让 textContent 还原出原文。
      return `<blockquote class="codex-md-quote">${b.lines
        .map((ln, idx) => `<span class="md-line">${mark('> ')}${inlineHtml(ln, onOpenFile)}${idx < b.lines.length - 1 ? mark('\n') : ''}</span>`)
        .join('')}</blockquote>`;
    case 'list':
      return listHtml(b.items as unknown as LiveListNode[], b.ordered, 0, onOpenFile);
    case 'code':
      return (
        `<div class="codex-md-codeblock">`
        + `<div class="codex-md-codeblock-head" ${UI}><span class="codex-md-codeblock-lang">${escapeHtml(b.lang || 'text')}</span>`
        + `<button type="button" class="codex-md-codeblock-copy" data-copy-block="1" ${UI}>复制</button></div>`
        // 空代码块只有一个换行（```\n```），不能按常规拼成 ```\n\n```，否则
        // 用户一编辑就凭空多出一个空行
        + `<pre class="codex-md-pre"><code>${mark(`\`\`\`${b.lang}\n`)}${escapeHtml(b.code)}${b.code ? mark('\n```') : mark('```')}</code></pre>`
        + `</div>`
      );
    case 'table': {
      const cols = b.header.length;
      const bodyRows = b.rows.length;
      // 每个单元格拆成「`| ` + 内容 + ` `」，行尾竖线补在**最后一个单元格内**
      // —— `<tr>` / `<tbody>` 里放不了裸文本节点，只能寄生在单元格里。
      // 相邻单元格各自带尾空格，拼起来才正好是 `| 甲 | 乙 |`；早先写成
      // 每个单元格自带首尾竖线，拼出来是 `| 甲 || 乙 |`，中间多出一个空单元格。
      // 换行同理放在末个单元格，且**末行不补**，否则块文本以 `\n` 收尾，
      // 回写源码时会凭空多出一个空行。
      const cells = (arr: string[], tag: 'th' | 'td', isLastRow: boolean) =>
        arr.map((c, j) => {
          const last = j === arr.length - 1;
          const tail = last ? `${mark('|')}${isLastRow ? '' : mark('\n')}` : '';
          return `<${tag} class="codex-md-${tag}">${mark('| ')}${inlineHtml(c, onOpenFile)}${mark(' ')}${tail}</${tag}>`;
        }).join('');
      // 分隔行的换行也要补，否则它会和第一个数据行粘成一行，重解析时表格退化成段落
      const sep = `${mark(`|${b.header.map(() => '---').join('|')}|`)}${bodyRows ? mark('\n') : ''}`;
      return (
        `<div class="codex-md-table-wrap"><table class="codex-md-table">`
        + `<thead><tr>${cells(b.header, 'th', false)}</tr>`
        + `<tr class="md-live-tablesep"><td colspan="${cols}">${sep}</td></tr></thead>`
        + `<tbody>${b.rows.map((r, k) => `<tr class="codex-md-tr">${cells(r, 'td', k === bodyRows - 1)}</tr>`).join('')}</tbody>`
        + `</table></div>`
      );
    }
    default:
      return `<p class="codex-md-p">${inlineHtml(b.text, onOpenFile)}</p>`;
  }
}

/**
 * 整篇 → HTML。块之间补回空行（markdown 里块之间就是空行分隔）。
 *
 * 每块外面套一层 `data-md-block="下标"` 的壳：预览页的「抹黑选中 → 浮卡改写」
 * 靠这个属性把选区映射回 `parseBlocks` 的块下标与源码行区间
 * （见 `PreviewSelectionEdit.resolveSelectionTarget`）。块顺序与 `parseBlocks` 一致，
 * 所以下标能对上。
 */
export function docHtml(src: string, onOpenFile?: (p: string) => void): string {
  const blocks = parseBlocks(src);
  if (blocks.length === 0) return `<p class="codex-md-p"><br></p>`;
  return blocks
    .map((b, i) => `<div class="md-live-block" data-md-block="${i}">${blockHtml(b, onOpenFile)}</div>`)
    .join(mark('\n\n'));
}
