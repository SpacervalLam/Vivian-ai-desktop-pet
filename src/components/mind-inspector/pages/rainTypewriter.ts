import { fitComicText } from './comicBind';

/** Reveal narration glyphs without changing text metrics. */
export function startRainTypewriter(root: HTMLElement, startOffset = 350): () => void {
  if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) return () => {};
  const selector = '.narration';
  const seen = new WeakSet<HTMLElement>();
  const observer = new MutationObserver(wrap);
  function wrap() {
    observer.disconnect();
    fitComicText(root);
    root.querySelectorAll<HTMLElement>(selector).forEach(el => {
      if (el.querySelector('.rain-type-character')) return;
      const text = el.textContent || '';
      const delay = seen.has(el) ? 0 : (parseFloat(getComputedStyle(el).getPropertyValue('--delay')) || 0) + startOffset;
      seen.add(el);
      el.setAttribute('aria-label', text);
      const fragment = document.createDocumentFragment();
      Array.from(text).forEach((letter, index) => {
        const span = document.createElement('span');
        span.className = 'rain-type-character';
        span.textContent = letter;
        span.setAttribute('aria-hidden', 'true');
        span.style.animationDelay = `${delay + index * 55}ms`;
        fragment.appendChild(span);
      });
      el.replaceChildren(fragment);
    });
    observer.observe(root, { childList: true, characterData: true, subtree: true });
  }
  wrap();
  return () => observer.disconnect();
}
