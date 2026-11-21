import { convertFileSrc } from '@tauri-apps/api/core';

/** Parse image destinations with spaces, balanced parentheses and escaped delimiters. */
export function parseMarkdownImage(text: string): { raw: string; alt: string; href: string } | null {
  const head = /^!\[([^\]]*)\](\\?\()/.exec(text);
  if (!head) return null;
  const escaped = head[2].startsWith('\\');
  let depth = 1;
  let end = head[0].length;
  for (; end < text.length; end++) {
    if (text[end] === '\\' && /[()]/.test(text[end + 1] ?? '')) {
      if (escaped && text[end + 1] === ')' && depth === 1) break;
      end++;
      continue;
    }
    if (text[end] === '(') depth++;
    if (text[end] === ')' && --depth === 0) break;
  }
  if (end === text.length) return null;
  const destination = text.slice(head[0].length, end).trim();
  const href = destination.startsWith('<')
    ? /^<([^>]+)>/.exec(destination)?.[1]
    : destination.replace(/\s+["'][^"']*["']\s*$/, '');
  if (!href) return null;
  return { raw: text.slice(0, end + (escaped && text[end] === '\\' ? 2 : 1)), alt: head[1], href: href.replace(/\\([()])/g, '$1') };
}

export function markdownImageSrc(href: string, documentPath?: string): string | null {
  let path = href.trim();
  if (/^https?:\/\//i.test(path)) return path;
  if (/^data:image\/(?:png|jpeg|gif|webp|avif);base64,/i.test(path)) return path;
  if (/^file:\/\//i.test(path)) {
    try { path = decodeURI(path.replace(/^file:\/\//i, '')).replace(/^\/([A-Za-z]:)/, '$1'); } catch { return null; }
  } else if (/^[a-z][a-z\d+.-]*:/i.test(path) && !/^[A-Za-z]:[\\/]/.test(path)) return null;
  if (!/^(?:[A-Za-z]:[\\/]|\/|\\\\)/.test(path)) {
    if (!documentPath) return null;
    path = `${documentPath.replace(/[\\/][^\\/]*$/, '')}/${path}`;
  }
  return convertFileSrc(path);
}
