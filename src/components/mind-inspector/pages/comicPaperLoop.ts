/** A visual second copy follows the original prose/list, so its end joins its
 * beginning. Copies are inert and hidden from accessibility; React retains the
 * only live owner. Resetting by one full cycle produces identical visible pixels. */
export function startComicPaperLoop(viewport: HTMLElement): () => void {
  let frame = 0, refreshFrame = 0, last = 0, position = 0, period = 0;
  const copies: HTMLElement[] = [];
  let observed: HTMLElement[] = [];
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const removeCopies = () => { copies.splice(0).forEach(copy => copy.remove()); };
  const observer = new MutationObserver(() => {
    cancelAnimationFrame(refreshFrame);
    refreshFrame = requestAnimationFrame(refresh);
  });
  const resize = new ResizeObserver(() => {
    cancelAnimationFrame(refreshFrame);
    refreshFrame = requestAnimationFrame(refresh);
  });
  const step = (now: number) => {
    if (!document.hidden && period > 0) {
      position = (position + (last ? Math.min(now - last, 64) * .012 : 0)) % period;
      viewport.scrollTop = position;
      last = now;
    } else last = 0;
    frame = requestAnimationFrame(step);
  };
  function refresh() {
    observer.disconnect();
    cancelAnimationFrame(frame);
    removeCopies();
    const originals = [...viewport.children].filter((node): node is HTMLElement => node instanceof HTMLElement && !node.hidden);
    period = 0;
    if (!reduced && originals.length && viewport.scrollHeight > viewport.clientHeight + 2) {
      originals.forEach(original => {
        const copy = original.cloneNode(true) as HTMLElement;
        copy.classList.add('comic-loop-copy');
        copy.setAttribute('aria-hidden', 'true');
        copy.inert = true;
        for (const node of [copy, ...copy.querySelectorAll<HTMLElement>('*')]) {
          node.removeAttribute('id'); node.removeAttribute('name');
          node.removeAttribute('data-bind'); node.removeAttribute('aria-labelledby');
          node.removeAttribute('aria-describedby');
        }
        viewport.append(copy);
        copies.push(copy);
      });
      period = copies[0].offsetTop - originals[0].offsetTop;
      position = period > 0 ? position % period : 0;
      viewport.scrollTop = position;
      last = 0;
      if (period > 0) frame = requestAnimationFrame(step);
    } else { position = 0; viewport.scrollTop = 0; }
    observer.observe(viewport, { childList: true, subtree: true, characterData: true });
    if (originals.length !== observed.length || originals.some((original, index) => original !== observed[index])) {
      resize.disconnect();
      resize.observe(viewport);
      originals.forEach(original => resize.observe(original));
      observed = originals;
    }
  }
  refresh();
  return () => {
    cancelAnimationFrame(frame); cancelAnimationFrame(refreshFrame);
    observer.disconnect(); resize.disconnect(); removeCopies(); viewport.scrollTop = 0;
  };
}
