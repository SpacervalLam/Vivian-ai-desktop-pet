import { selectComicDialogue } from './comicDialogue';
import type { ComicCharacter } from './ComicOverview';

/**
 * 漫画数据就地更新（patch）与气泡文本自适应（fit）。
 *
 * 为什么需要这一层：分镜 HTML 由 comicMarkup 一次性生成整棵树，而心灵数据每 5 秒
 * 刷新一次。若把数据变化重新喂给 comicMarkup，innerHTML 会被整体替换——入场动画
 * 重放、图片重新解码、hover 漂浮与小手状态全部丢失。所以动态文本在 comicMarkup 里
 * 一律带 data-bind，数据变化时只改文本节点，DOM 结构原样保留。
 */

const DIALOGUE_CATEGORIES = ['mood', 'emotions', 'needs', 'relations'] as const;
// Authored narration has its own readable typography. Per-glyph reveal spans
// must never send it through the speech-bubble shrinking algorithm.
const FIT_SELECTOR = '.voice p, .line-chat p, .goal-line p';

/** Language switches patch existing labels; panels and their animation state survive. */
export function translateComicLabels(root: HTMLElement | null, translate: (key: string) => string): void {
  if (!root) return;
  root.querySelectorAll<HTMLElement>('[data-i18n]').forEach(label => {
    const text = translate(label.dataset.i18n!);
    if (label.textContent !== text) label.textContent = text;
  });
  root.querySelectorAll<HTMLImageElement>('[data-i18n-alt]').forEach(image => {
    image.alt = translate(image.dataset.i18nAlt!);
  });
  root.querySelectorAll<HTMLElement>('[data-i18n-label]').forEach(control => {
    control.setAttribute('aria-label', translate(control.dataset.i18nLabel!));
  });
  fitComicText(root);
}

/** 按 data-bind 就地刷新全部动态文本，并重新适配气泡排版。 */
export function patchComicData(root: HTMLElement | null, data: Record<'vivian' | 'nana', ComicCharacter>, visit: number, language = 'zh-CN'): void {
  if (!root) return;
  for (const who of ['vivian', 'nana'] as const) {
    const character = data[who];
    setText(root, `presence:${who}`, `${character.name} · ${character.presence}`);
    setText(root, `thought:${who}`, character.thought);
    setText(root, `goal:${who}`, character.goal);
    for (const category of DIALOGUE_CATEGORIES) {
      setText(root, `dialogue:${who}:${category}`, selectComicDialogue(who, category, character, visit, language));
    }
  }
  fitComicText(root);
}

function setText(root: HTMLElement, key: string, value: string): void {
  root.querySelectorAll<HTMLElement>(`[data-bind="${key}"]`).forEach((el) => {
    if (el.textContent !== value) el.textContent = value;
  });
}

/**
 * Fit complete speech to its bubble. Recompute from the authored font size on
 * every data/size change so a shorter next thought grows back again. Never
 * truncate original speech or introduce a line clamp.
 */
export function fitComicText(root: HTMLElement | null): void {
  if (!root) return;
  root.querySelectorAll<HTMLElement>(FIT_SELECTOR).forEach(fitTextBox);
}

function fitTextBox(el: HTMLElement): void {
  el.style.fontSize = '';
  el.style.removeProperty('display');
  el.style.removeProperty('-webkit-line-clamp');
  el.style.removeProperty('-webkit-box-orient');
  el.style.removeProperty('text-overflow');
  const parent = el.parentElement;
  if (!parent?.clientHeight || !el.clientWidth) return;
  const box = getComputedStyle(parent);
  const text = getComputedStyle(el);
  const available = parent.clientHeight - parseFloat(box.paddingTop) - parseFloat(box.paddingBottom)
    - parseFloat(text.marginTop) - parseFloat(text.marginBottom);
  const siblings = Array.from(parent.children).filter(node => node !== el && getComputedStyle(node).position !== 'absolute')
    .reduce((height, node) => height + (node as HTMLElement).offsetHeight, 0);
  const limit = Math.max(1, (available - siblings) * (parent.classList.contains('voice') ? .86 : 1));
  const start = parseFloat(getComputedStyle(el).fontSize) || 16;
  if (el.scrollHeight <= limit && el.scrollWidth <= el.clientWidth + 1) return;
  let low = 1;
  let high = start;
  // Binary search avoids dozens of forced layouts for a long thought.
  for (let i = 0; i < 12; i++) {
    const size = (low + high) / 2;
    el.style.fontSize = `${size}px`;
    if (el.scrollHeight <= limit && el.scrollWidth <= el.clientWidth + 1) low = size;
    else high = size;
  }
  el.style.fontSize = `${Math.floor(low * 10) / 10}px`;
}
