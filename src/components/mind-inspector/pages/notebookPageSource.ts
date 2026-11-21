import type { Block } from './notebook-types';

// Page source is a shorthand for HTML, with nestable element wrappers and raw HTML.
// It is deliberately independent of the chat/file Markdown renderer.
type Node = { kind: 'text'; lines: string[]; line: number } | {
  kind: 'element'; name: string; attrs: Record<string, string>; children: Node[]; raw: string; line: number;
};
const RAW = new Set(['html', 'code', 'chart', 'mermaid', 'block']);
const ELEMENTS = new Set(['card', 'callout', 'section', 'row', 'column', 'text', 'quote', 'image', ...RAW]);
export class PageSourceError extends Error {
  constructor(public line: number, message: string) { super(message); }
}
const fail = (line: number, message: string): never => { throw new PageSourceError(line, message); };
const escape = (value: string) => value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const attrValue = (value: string) => JSON.stringify(value);

function attributes(source: string, line: number): Record<string, string> {
  const attrs: Record<string, string> = {};
  const re = /([a-z][\w-]*)\s*=\s*("(?:\\.|[^"\\])*"|'[^']*'|[^\s]+)/gi;
  let end = 0;
  for (const match of source.matchAll(re)) {
    if (source.slice(end, match.index).trim()) fail(line, '属性请写成 name="value" / Use name="value" for attributes');
    const value = match[2];
    try { attrs[match[1]] = value.startsWith('"') ? JSON.parse(value) : value.startsWith("'") ? value.slice(1, -1) : value; }
    catch { fail(line, '属性引号不完整 / Invalid quoted attribute'); }
    end = match.index! + match[0].length;
  }
  if (source.slice(end).trim()) fail(line, '属性请写成 name="value" / Use name="value" for attributes');
  return attrs;
}

function parse(source: string): Node[] {
  const lines = source.replace(/\r\n?/g, '\n').split('\n');
  let index = 0;
  const walk = (nested: boolean): Node[] => {
    const nodes: Node[] = [];
    while (index < lines.length) {
      const line = lines[index];
      if (/^\s*:::\s*$/.test(line)) {
        if (!nested) fail(index + 1, '这个结束标记没有对应的元素 / Unexpected closing wrapper');
        index++;
        return nodes;
      }
      const open = /^\s*:::([a-z][\w-]*)(?:\s+(.*))?\s*$/.exec(line);
      if (open) {
        const lineNumber = index + 1;
        const name = open[1];
        if (!ELEMENTS.has(name)) fail(lineNumber, `未知元素 / Unknown element: ${name}`);
        const attrs = attributes(open[2] ?? '', lineNumber);
        index++;
        let raw = '';
        let children: Node[] = [];
        if (RAW.has(name)) {
          const start = index;
          while (index < lines.length && !/^\s*:::\s*$/.test(lines[index])) index++;
          if (index === lines.length) fail(lineNumber, `请用 ::: 结束 ${name} / Close ${name} with :::`);
          raw = lines.slice(start, index).join('\n');
          index++;
        } else children = walk(true);
        nodes.push({ kind: 'element', name, attrs, children, raw, line: lineNumber });
      } else {
        const start = index;
        // Code fences contain literal text, including wrapper-like lines.
        if (/^\s*```/.test(line)) {
          index++;
          while (index < lines.length && !/^\s*```\s*$/.test(lines[index])) index++;
          if (index === lines.length) fail(start + 1, '请结束代码区域 / Close the code fence');
          index++;
        } else {
          index++;
          while (index < lines.length && !/^\s*:::/.test(lines[index]) && !/^\s*```/.test(lines[index])) index++;
        }
        nodes.push({ kind: 'text', lines: lines.slice(start, index), line: start + 1 });
      }
    }
    if (nested) fail(lines.length, '缺少元素的结束标记 ::: / Missing closing wrapper :::');
    return nodes;
  };
  return walk(false);
}

function url(value: string): string | null {
  const compact = value.replace(/[\u0000-\u0020]/g, '');
  if (/^[a-z]:[\\/]/i.test(value)) return value;
  if (/^(?:https?:|mailto:|tel:|asset:|data:image\/(?:png|jpeg|gif|webp);base64,)/i.test(compact)) return value;
  return !/^[a-z][\w+.-]*:/i.test(compact) ? value : null;
}

/** Inline conveniences compile directly to safe HTML elements. */
export function pageInline(source: string): string {
  let out = '';
  const token = /(\\[\\*`\[\]|#>:+\-.0-9]|`[^`\n]+`|\*\*[^*\n]+\*\*|\*[^*\n]+\*|!?\[[^\]\n]*\]\((?:[^()\n]|\([^()\n]*\))*\))/g;
  let end = 0;
  for (const match of source.matchAll(token)) {
    out += escape(source.slice(end, match.index));
    const value = match[0];
    if (value.startsWith('\\')) out += escape(value.slice(1));
    else if (value.startsWith('`')) out += `<code>${escape(value.slice(1, -1))}</code>`;
    else if (value.startsWith('**')) out += `<strong>${escape(value.slice(2, -2))}</strong>`;
    else if (value.startsWith('*')) out += `<em>${escape(value.slice(1, -1))}</em>`;
    else {
      const link = /^(!?)\[([^\]]*)\]\((.*)\)$/.exec(value)!;
      const target = url(link[3]);
      if (!target) out += escape(value);
      else if (link[1]) out += `<img src="${escape(target)}" alt="${escape(link[2])}" style="max-width:100%;height:auto" loading="lazy">`;
      else out += `<a href="${escape(target)}" target="_blank" rel="noopener noreferrer">${escape(link[2])}</a>`;
    }
    end = match.index! + value.length;
  }
  return out + escape(source.slice(end));
}

function textHtml(lines: string[]): string {
  let out = '';
  let i = 0;
  const special = (line: string) => !line.trim() || /^(#{1,6}\s|>\s?|\s*(?:[-+*]|\d+[.)])\s|---\s*$|\s*```)/.test(line);
  while (i < lines.length) {
    const line = lines[i];
    if (!line.trim()) { i++; continue; }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) { out += `<h${heading[1].length} class="heading heading-${heading[1].length}">${pageInline(heading[2])}</h${heading[1].length}>`; i++; continue; }
    if (/^---\s*$/.test(line)) { out += '<div class="divider"></div>'; i++; continue; }
    if (/^\s*```/.test(line)) {
      const body: string[] = [];
      i++;
      while (i < lines.length && !/^\s*```\s*$/.test(lines[i])) body.push(lines[i++]);
      i++;
      out += `<pre style="overflow:auto;padding:16px;background:rgba(127,127,127,.08);border-radius:8px"><code>${escape(body.join('\n'))}</code></pre>`;
      continue;
    }
    if (line.includes('|') && i + 1 < lines.length && /^\s*\|?\s*:?-{2,}/.test(lines[i + 1])) {
      const cells = (row: string) => row.trim().replace(/^\|/, '').replace(/\|$/, '').split('|').map(cell => cell.trim());
      const cellHtml = (row: string, tag: string) => cells(row).map(cell => `<${tag}>${pageInline(cell)}</${tag}>`).join('');
      out += `<div style="overflow-x:auto"><table class="nb-table"><thead><tr>${cellHtml(line, 'th')}</tr></thead><tbody>`;
      i += 2;
      while (i < lines.length && lines[i].includes('|') && lines[i].trim()) out += `<tr>${cellHtml(lines[i++], 'td')}</tr>`;
      out += '</tbody></table></div>';
      continue;
    }
    if (/^>/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && /^>/.test(lines[i])) body.push(lines[i++].replace(/^>\s?/, ''));
      out += `<blockquote class="quote">${body.map(pageInline).join('<br>')}</blockquote>`;
      continue;
    }
    if (/^\s*(?:[-+*]|\d+[.)])\s/.test(line)) {
      // Keep nested lists in the HTML tree, including task checkboxes.
      const stack: { indent: number; tag: string }[] = [];
      while (i < lines.length) {
        const item = /^(\s*)([-+*]|\d+[.)])\s+(.*)$/.exec(lines[i]);
        if (!item) break;
        const indent = item[1].length;
        const tag = /^\d/.test(item[2]) ? 'ol' : 'ul';
        while (stack.length && (stack[stack.length - 1].indent > indent || (stack[stack.length - 1].indent === indent && stack[stack.length - 1].tag !== tag))) {
          out += `</li></${stack.pop()!.tag}>`;
        }
        if (!stack.length || indent > stack[stack.length - 1].indent) {
          out += `<${tag} class="list"${tag === 'ol' ? ` start="${parseInt(item[2], 10)}"` : ''}><li>`;
          stack.push({ indent, tag });
        } else out += '</li><li>';
        const task = /^\[([ xX])\]\s+(.*)$/.exec(item[3]);
        out += task ? `<input type="checkbox" disabled${task[1] !== ' ' ? ' checked' : ''}> ${pageInline(task[2])}` : pageInline(item[3]);
        i++;
      }
      while (stack.length) out += `</li></${stack.pop()!.tag}>`;
      continue;
    }
    const paragraph = [line];
    i++;
    while (i < lines.length && !special(lines[i]) && !(lines[i].includes('|') && /^\s*\|?\s*:?-{2,}/.test(lines[i + 1] ?? ''))) paragraph.push(lines[i++]);
    out += `<p class="paragraph">${paragraph.map(pageInline).join('<br>')}</p>`;
  }
  return out;
}

function style(attrs: Record<string, string>, line: number): string {
  const rules: string[] = [];
  const cssKeys: Record<string, string> = { color: 'color', background: 'background', padding: 'padding', radius: 'border-radius', gap: 'gap', size: 'font-size', align: 'text-align', width: 'width' };
  for (const [key, value] of Object.entries(attrs)) {
    if (!(key in cssKeys)) continue;
    const safe = key === 'align' ? /^(left|center|right|justify)$/.test(value)
      : key === 'color' || key === 'background' ? /^(#[0-9a-f]{3,8}|[a-z]+|rgba?\([\d\s.,%]+\)|var\(--[\w-]+\))$/i.test(value)
      : /^(?:\d+(?:\.\d+)?(?:px|em|rem|%|vh|vw)?|auto)$/.test(value);
    if (!safe) fail(line, `无效样式 / Invalid style: ${key}="${value}"`);
    rules.push(`${cssKeys[key]}:${/^\d+(\.\d+)?$/.test(value) ? `${value}px` : value}`);
  }
  return rules.join(';');
}

function elementHtml(node: Node): string {
  if (node.kind === 'text') return textHtml(node.lines);
  const { name, attrs, line, raw } = node;
  const allowed = new Set(['title', 'author', 'src', 'caption', 'alt', 'color', 'background', 'padding', 'radius', 'gap', 'size', 'align', 'width', 'columns', 'lang']);
  for (const key of Object.keys(attrs)) if (!allowed.has(key)) fail(line, `未知属性 / Unknown attribute: ${key}`);
  if (name === 'chart' || name === 'mermaid' || name === 'block') fail(line, '图表和流程图请放在容器外 / Place charts and diagrams outside wrappers');
  if (name === 'html') return raw;
  if (name === 'code') return `<pre style="overflow:auto;${style(attrs, line)}"><code>${escape(raw)}</code></pre>`;
  const body = node.children.map(elementHtml).join('');
  let css = style(attrs, line);
  if (name === 'row') {
    const count = Number(attrs.columns ?? '2');
    if (!Number.isInteger(count) || count < 1 || count > 6) fail(line, '分栏数量应为 1–6 / Columns must be 1–6');
    css = `display:grid;grid-template-columns:repeat(${count},minmax(0,1fr));gap:16px;${css}`;
  }
  const extra = css ? ` style="${escape(css)}"` : '';
  const title = attrs.title ? `<div class="card-header"><span class="card-title">${pageInline(attrs.title)}</span></div>` : '';
  switch (name) {
    case 'image': {
      if (body) return fail(line, '图片说明请写在 caption 属性中 / Put the image description in caption');
      const src = url(attrs.src ?? '');
      if (src === null) return fail(line, '图片地址不可用 / Invalid image URL');
      return `<figure class="image-wrap"${extra}><img src="${escape(src)}" alt="${escape(attrs.alt ?? attrs.caption ?? '')}" style="max-width:100%;height:auto" loading="lazy">${attrs.caption ? `<figcaption class="image-caption">${pageInline(attrs.caption)}</figcaption>` : ''}</figure>`;
    }
    case 'card': return `<section class="card"${extra}>${title}<div class="card-body">${body}</div></section>`;
    case 'callout': return `<aside class="callout"${extra}>${title}${body}</aside>`;
    case 'quote': return `<blockquote class="quote"${extra}>${body}${attrs.author ? `<cite class="quote-author">${escape(attrs.author)}</cite>` : ''}</blockquote>`;
    case 'section': return `<section${extra}>${attrs.title ? `<h2 class="heading heading-2">${pageInline(attrs.title)}</h2>` : ''}${body}</section>`;
    default: return `<div class="page-${name}"${extra}>${body}</div>`;
  }
}

function dataBlock(node: Extract<Node, { kind: 'element' }>): Block {
  for (const key of Object.keys(node.attrs)) {
    if (!(node.name === 'mermaid' && key === 'caption') && !(node.name === 'chart' && key === 'title')) fail(node.line, `未知属性 / Unknown attribute: ${key}`);
  }
  if (node.name === 'mermaid') return { type: 'mermaid', code: node.raw, caption: node.attrs.caption };
  let value: unknown;
  try { value = JSON.parse(node.raw); } catch { return fail(node.line, '数据内容需要有效 JSON / Invalid JSON data'); }
  if (!value || typeof value !== 'object' || Array.isArray(value)) fail(node.line, '数据应为一个对象 / Data must be an object');
  const b: Record<string, unknown> = { ...value as Record<string, unknown>, ...(node.name === 'chart' ? { type: 'chart' } : {}) };
  if (node.name === 'chart' && node.attrs.title !== undefined) b.title = node.attrs.title;
  const strings = (x: unknown): x is string[] => Array.isArray(x) && x.every(v => typeof v === 'string');
  const str = (key: string) => typeof b[key] === 'string';
  const optionalString = (key: string) => b[key] === undefined || b[key] === null || str(key);
  let valid = false;
  switch (b.type) {
    case 'heading': valid = str('text') && Number.isInteger(b.level) && Number(b.level) >= 1 && Number(b.level) <= 6; break;
    case 'paragraph': case 'callout': valid = str('text'); break;
    case 'quote': valid = str('text') && optionalString('author'); break;
    case 'card': valid = str('body') && optionalString('title'); break;
    case 'list': valid = strings(b.items) && (b.ordered === undefined || typeof b.ordered === 'boolean'); break;
    case 'tags': valid = strings(b.items); break;
    case 'image': valid = str('url') && optionalString('caption'); break;
    case 'divider': valid = true; break;
    case 'custom': valid = str('html') && optionalString('markdown') && optionalString('page_source'); break;
    case 'table': valid = strings(b.headers) && Array.isArray(b.rows) && b.rows.every(strings) && optionalString('caption'); break;
    case 'mermaid': valid = str('code') && optionalString('caption'); break;
    case 'chart': valid = str('chart_type') && optionalString('title') && strings(b.categories) && Array.isArray(b.series) && b.series.every((entry: unknown) => {
      if (!entry || typeof entry !== 'object') return false;
      const s = entry as Record<string, unknown>;
      return typeof s.name === 'string' && Array.isArray(s.data) && s.data.every((n: unknown) => typeof n === 'number' && Number.isFinite(n));
    }); break;
  }
  if (b.style !== undefined && b.style !== null) {
    const s = b.style as Record<string, unknown>;
    valid = valid && typeof s === 'object' && !Array.isArray(s)
      && (s.color === undefined || s.color === null || typeof s.color === 'string')
      && (s.font_size === undefined || s.font_size === null || typeof s.font_size === 'number')
      && (s.bold === undefined || s.bold === null || typeof s.bold === 'boolean')
      && (s.italic === undefined || s.italic === null || typeof s.italic === 'boolean')
      && (s.align === undefined || s.align === null || ['left', 'center', 'right'].includes(String(s.align)));
  }
  if (!valid) fail(node.line, '元素数据字段不完整或类型错误 / Invalid element fields');
  return b as unknown as Block;
}

export function compilePageSource(source: string): Block[] {
  if (!source.trim()) return [];
  const nodes = parse(source);
  const blocks: Block[] = [];
  let html = '';
  const flush = () => {
    if (html || !blocks.length) blocks.push({ type: 'custom', html, ...(!blocks.length ? { page_source: source } : {}) });
    html = '';
  };
  for (const node of nodes) {
    if (node.kind === 'element' && ['chart', 'mermaid', 'block'].includes(node.name)) {
      flush(); blocks.push(dataBlock(node));
    } else html += elementHtml(node);
  }
  flush();
  return blocks;
}

export function sourceFromBlocks(blocks: Block[]): string {
  const authored = blocks.find((b): b is Extract<Block, { type: 'custom' }> => b.type === 'custom' && b.page_source !== undefined);
  if (authored) return authored.page_source!;
  const literal = (value: string) => value.split('\n').map(line => line.replace(/[\\*`\[\]|]/g, '\\$&').replace(/^(#{1,6}\s|>\s?|---(?:\s|$)|:::|[-+]\s|\d+[.)]\s)/, '\\$&')).join('\n');
  return blocks.filter(b => b.type !== 'meta').map(block => {
    // Keep imported content exact. Editing a wrapper never discards old styles.
    if ('style' in block && block.style && Object.keys(block.style).length) return `:::block\n${JSON.stringify(block, null, 2)}\n:::`;
    switch (block.type) {
      case 'paragraph': return literal(block.text);
      case 'heading': return `${'#'.repeat(Math.max(1, Math.min(6, block.level)))} ${literal(block.text)}`;
      case 'list': return block.items.some(item => item.includes('\n')) ? `:::block\n${JSON.stringify(block, null, 2)}\n:::` : block.items.map((item, i) => `${block.ordered ? `${i + 1}.` : '-'} ${literal(item)}`).join('\n');
      case 'divider': return '---';
      case 'card': return `:::card${block.title ? ` title=${attrValue(literal(block.title))}` : ''}\n${literal(block.body)}\n:::`;
      case 'callout': return `:::callout\n${literal(block.text)}\n:::`;
      case 'quote': return `:::quote${block.author ? ` author=${attrValue(block.author)}` : ''}\n${literal(block.text)}\n:::`;
      case 'image': return `:::image src=${attrValue(block.url)}${block.caption ? ` caption=${attrValue(literal(block.caption))}` : ''}\n:::`;
      case 'mermaid': return `:::mermaid${block.caption ? ` caption=${attrValue(block.caption)}` : ''}\n${block.code}\n:::`;
      case 'chart': return `:::chart\n${JSON.stringify(block, null, 2)}\n:::`;
      case 'custom': return block.markdown ?? (/^\s*:::\s*$/m.test(block.html) ? `:::block\n${JSON.stringify(block, null, 2)}\n:::` : `:::html\n${block.html}\n:::`);
      default: return `:::block\n${JSON.stringify(block, null, 2)}\n:::`;
    }
  }).join('\n\n');
}

/** Lists and quotes are writing conveniences, not a document format. */
export function pageEnter(source: string, start: number, end: number): { start: number; end: number; text: string; caret?: number } {
  const preceding = source.slice(0, start);
  // Raw elements and code fences must not inherit prose editing shortcuts.
  const rawOpen = preceding.match(/(?:^|\n)\s*:::(html|code|chart|mermaid|block)[^\n]*\n/g);
  if (rawOpen) {
    const last = preceding.lastIndexOf(rawOpen[rawOpen.length - 1]);
    if (!/^\s*:::\s*$/m.test(preceding.slice(last + rawOpen[rawOpen.length - 1].length))) return { start, end, text: '\n' };
  }
  if ((preceding.match(/^\s*```/gm)?.length ?? 0) % 2) return { start, end, text: '\n' };
  const currentLine = preceding.slice(preceding.lastIndexOf('\n') + 1);
  const opener = /^\s*:::([a-z][\w-]*)(?:\s+.*)?$/.exec(currentLine);
  if (opener && ELEMENTS.has(opener[1]) && !source.slice(end).trim()) {
    return { start, end, text: '\n\n:::', caret: start + 1 };
  }
  const lineStart = start === 0 ? 0 : source.lastIndexOf('\n', start - 1) + 1;
  const match = /^(\s*)(?:(\d+)[.)] |([-+*]) (?:\[([ xX])\] )?|(>) )(.*)$/.exec(source.slice(lineStart, start));
  if (!match) return { start, end, text: '\n' };
  if (!match[6].trim()) return { start: lineStart, end, text: '\n' };
  return { start, end, text: `\n${match[1]}${match[2] ? `${Number(match[2]) + 1}. ` : match[3] ? `${match[3]} ${match[4] !== undefined ? '[ ] ' : ''}` : '> '}` };
}
