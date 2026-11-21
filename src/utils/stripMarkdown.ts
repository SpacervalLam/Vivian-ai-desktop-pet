/**
 * 把 markdown 原文转成纯文本（仅用于纯文本容器的显示层，如桌宠气泡）。
 *
 * 与 markdown 渲染器相反：这里不消费语法产生富文本，而是把语法标记剥掉、
 * 保留正文内容，避免 "**加粗**" 之类的符号裸露在纯文本气泡里。
 *
 * 设计约束（纯文本显示用途）：
 * - 保守优先：只剥确定是 markdown 语法的形态，正文中偶发出现的星号/下划线
 *   （如 "3 * 4"、文件名 a_b）不被误伤。
 * - 剥除只影响显示，调用方传入的原文不会被修改（字符串不可变，无副作用）。
 * - 代码块内容保留文本本身，仅丢弃 ``` 围栏行。
 */

// 行内代码：`code` → code（反引号在口语文本中几乎总是代码/关键词标记）
const INLINE_CODE_RE = /`([^`\n]+)`/g;

// 加粗 / 删除线：**x** / __x__ / ~~x~~ → x（成对定界符，内容不跨行）
const BOLD_RE = /\*\*([^*\n]+)\*\*|__([^_\n]+)__|~~([^~\n]+)~~/g;

// 链接/图片：[text](url) → text；![alt](url) → alt
const LINK_RE = /!\[([^\]]*)\]\([^)]*\)|\[([^\]]+)\]\([^)]*\)/g;

// 行首结构符：标题 #、引用 >、无序列表 - * +、有序列表 1. / 1)
const LINE_LEAD_RE = /^ {0,3}(#{1,6}\s+|>{1,2}\s?|[-*+]\s+|\d{1,3}[.)]\s+)/;

/** 剥离单组斜体：*x* / _x_，仅当定界符紧贴内容且内容不含空格 */
function stripItalicSegment(match: string, body: string): string {
  // 定界符与内容之间没有空白，且内容不是纯数字/符号（防 "3 * 4" / "*5*" 误伤）
  if (body.length > 0 && body[0] !== ' ' && body[body.length - 1] !== ' ' && /[^\d\s]/.test(body)) {
    return body;
  }
  return match;
}

/** 将一段 markdown 文本剥成纯文本 */
export function stripMarkdown(text: string): string {
  if (!text) return text;

  // 1. 先剥内联标记（顺序：反引号 → 加粗/删除线 → 斜体 → 链接）
  //    反引号优先：避免代码内容里的 * 等被后面的规则误剥
  let out = text.replace(INLINE_CODE_RE, '$1');
  out = out.replace(BOLD_RE, (_m, b1, b2, b3) => b1 ?? b2 ?? b3);
  out = out.replace(LINK_RE, (_m, alt, linkText) => alt ?? linkText ?? '');
  out = out.replace(/\*([^*\n]+)\*/g, stripItalicSegment);
  out = out.replace(/_([^_\n]+)_/g, stripItalicSegment);

  // 2. 逐行剥行首结构符，并丢弃代码围栏行（``` / ~~~），围栏内的正文保留
  const lines = out.split('\n');
  let inFence = false;
  const cleaned: string[] = [];
  for (let line of lines) {
    const fenceMatch = /^\s*(```|~~~)/.test(line);
    if (fenceMatch) {
      inFence = !inFence; // 围栏成对切换；围栏行本身不输出
      continue;
    }
    if (inFence) {
      cleaned.push(line); // 代码块内容：原样保留（已是文本）
      continue;
    }
    line = line.replace(LINE_LEAD_RE, '');
    cleaned.push(line);
  }
  out = cleaned.join('\n');

  return out.trim();
}

export default stripMarkdown;
