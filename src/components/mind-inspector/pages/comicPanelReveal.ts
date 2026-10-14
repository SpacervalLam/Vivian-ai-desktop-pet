import { handSceneTiming } from './comicTour';

/** A full-page shot closes into its final portrait panel. Shrinking leads the
 * rightward travel slightly, so both edges visibly squeeze inward first. */
export function startFullPanelReveal(panel: HTMLElement, enabled: () => boolean) {
  const shot = panel.querySelector<HTMLElement>('.shot')!;
  const final = panel.dataset.r!.split(',').map(Number);
  const finalPoints: number[][] = JSON.parse(shot.dataset.points!);
  const rectangle = [[0,0],[100,0],[100,100],[0,100]];
  const edge = shot.querySelector('polygon');
  const start = performance.now();
  const duration = 1100;
  let frame = 0;
  const refresh = () => {
    if (!enabled()) return;
    const elapsed = Math.max(0, performance.now() - start);
    const progress = Math.min(1, elapsed / duration);
    const squeeze = progress * progress * (3 - 2 * progress);
    const travel = squeeze ** 2;
    const width = 100 + (final[2] - 100) * squeeze;
    const inset = (100 - width) / 2;
    const left = inset + (final[0] - (100 - final[2]) / 2) * travel;
    Object.assign(panel.style, { left: `${left}%`, top: `${final[1] * squeeze}%`, width: `${width}%`, height: `${100 + (final[3] - 100) * squeeze}%` });
    const points = finalPoints.map((point, index) => point.map((value, axis) => rectangle[index][axis] + (value - rectangle[index][axis]) * squeeze));
    shot.style.clipPath = `polygon(${points.map(point => `${point[0]}% ${point[1]}%`).join(',')})`;
    edge?.setAttribute('points', points.map(point => point.join(',')).join(' '));
  };
  const step = () => {
    refresh();
    if (performance.now() - start < duration) frame = requestAnimationFrame(step);
  };
  refresh();
  frame = requestAnimationFrame(step);
  return { refresh, stop: () => cancelAnimationFrame(frame) };
}

/** The inset pushes up from the main frame's bottom; its paper gutter cuts
 * the artwork and ink together, on the same clock as the incoming panel. */
export function startInsetPanelReveal(stage: HTMLElement, enabled: () => boolean, animate: boolean, onSettled?: () => void) {
  const main = stage.querySelector<HTMLElement>('#skirt-full')!;
  const inset = stage.querySelector<HTMLElement>('#skirt-hand')!;
  const mainPanel = main.parentElement!;
  const insetPanel = inset.parentElement!;
  const edge = main.querySelector('polygon');
  const points: number[][] = JSON.parse(main.dataset.points!);
  const notebook = stage.closest('.unified-comic-book')?.querySelector<HTMLElement>('.source-notebook');
  const start = performance.now();
  const delay = handSceneTiming.insetDelay;
  const duration = handSceneTiming.insetDuration;
  let settled = false;
  let frame = 0;
  let geometry: { width: number; height: number; left: number; right: number; top: number; travel: number };
  const cropNotebook = (shape: number[][] | null) => {
    if (!notebook || notebook.classList.contains('is-expanded')) return;
    notebook.querySelectorAll<SVGPolygonElement>('.unified-panel-ink polygon, .unified-panel-gutter polygon')
      .forEach(polygon => polygon.setAttribute('points', (shape ?? [[0,0],[100,0],[100,100],[0,100]]).map(point => point.join(',')).join(' ')));
    if (shape) notebook.style.setProperty('--notebook-crop', `polygon(${shape.map(point => `${point[0]}% ${point[1]}%`).join(',')})`);
    else notebook.style.removeProperty('--notebook-crop');
  };

  // Same cubic-bezier(.22,.61,.36,1) as page 04's crop.
  const ease = (progress: number) => {
    let low = 0, high = 1;
    for (let i = 0; i < 18; i++) {
      const t = (low + high) / 2;
      const x = 3 * (1 - t) ** 2 * t * .22 + 3 * (1 - t) * t ** 2 * .36 + t ** 3;
      if (x < progress) low = t; else high = t;
    }
    const t = (low + high) / 2;
    return 3 * (1 - t) ** 2 * t * .61 + 3 * (1 - t) * t ** 2 + t ** 3;
  };
  const paint = () => {
    if (!enabled()) {
      stage.classList.remove('inset-crop');
      insetPanel.style.translate = '';
      insetPanel.style.visibility = '';
      insetPanel.style.opacity = '';
      cropNotebook(null);
      return;
    }
    if (!geometry.width || !geometry.height) return;
    stage.classList.add('inset-crop');
    const elapsed = performance.now() - start - delay;
    const progress = animate ? Math.max(0, Math.min(1, elapsed / duration)) : 1;
    const eased = progress === 0 || progress === 1 ? progress : ease(progress);
    const offset = geometry.travel * (1 - eased);
    insetPanel.style.translate = `0 ${offset}px`;
    insetPanel.style.opacity = String(eased);
    insetPanel.style.visibility = animate && elapsed < 0 ? 'hidden' : 'visible';
    const right = Math.max(0, Math.min(100, geometry.right / geometry.width * 100));
    const top = Math.max(0, Math.min(100, (geometry.top + offset) / geometry.height * 100));
    const cropped = [...points.slice(0, 3), [right, 100], [right, top], [0, top]];
    main.style.clipPath = `polygon(${cropped.map(point => `${point[0]}% ${point[1]}%`).join(',')})`;
    edge?.setAttribute('points', cropped.map(point => point.join(',')).join(' '));
    if (notebook && !notebook.classList.contains('is-expanded')) {
      const noteRect = notebook.getBoundingClientRect();
      const mainRect = mainPanel.getBoundingClientRect();
      if (noteRect.width && noteRect.height) {
        const left = Math.max(0, Math.min(100, (mainRect.left + geometry.left - noteRect.left) / noteRect.width * 100));
        const top = Math.max(0, Math.min(100, (mainRect.top + geometry.top + offset - noteRect.top) / noteRect.height * 100));
        cropNotebook([[0,0],[100,0],[100,top],[left,top],[left,100],[0,100]]);
      }
    }
    if (progress === 1 && !settled) {
      settled = true;
      onSettled?.();
    }
  };
  const refresh = () => {
    // Measure final layout, without the temporary entrance translation.
    insetPanel.style.translate = '';
    const outer = mainPanel.getBoundingClientRect();
    const inner = insetPanel.getBoundingClientRect();
    const gutter = parseFloat(getComputedStyle(inset).getPropertyValue('--hand-gutter')) || 9;
    geometry = {
      width: outer.width, height: outer.height,
      left: inner.left - gutter - outer.left,
      right: inner.right + gutter - outer.left,
      top: inner.top - gutter - outer.top,
      travel: Math.max(0, outer.bottom + gutter - inner.top),
    };
    if (outer.width && outer.height) paint();
  };
  const step = () => {
    paint();
    if (performance.now() - start < delay + duration) frame = requestAnimationFrame(step);
  };
  refresh();
  // Book slot measurements arrive separately from the illustrated stage.
  const resize = notebook ? new ResizeObserver(refresh) : undefined;
  if (notebook) resize?.observe(notebook);
  if (animate) frame = requestAnimationFrame(step);
  return { refresh, stop: () => {
    cancelAnimationFrame(frame);
    resize?.disconnect();
    stage.classList.remove('inset-crop');
    insetPanel.style.translate = '';
    insetPanel.style.visibility = '';
    insetPanel.style.opacity = '';
  } };
}
