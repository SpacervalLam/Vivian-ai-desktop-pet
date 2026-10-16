/** One shared ink front in page coordinates, clipped separately inside each shot. */
export function startMorningInkReveal(stage: HTMLElement) {
  const layers = [...stage.querySelectorAll<HTMLElement>('.morning-panel .image-wrap')].map(wrap => {
    const original = wrap.querySelector<HTMLImageElement>('img')!;
    const color = original.cloneNode() as HTMLImageElement;
    color.classList.add('morning-color-layer');
    color.alt = '';
    color.setAttribute('aria-hidden', 'true');
    color.style.clipPath = 'polygon(0 0,0 0,0 0)';
    original.style.filter = 'grayscale(1)';
    wrap.appendChild(color);
    return { wrap, original, color };
  });
  let frame = 0;
  const started = performance.now();
  const revealAt = 3800; // Wake entrance at 3300ms, then wait 500ms.
  const duration = 2000;
  function clean() {
    layers.forEach(({ original, color }) => { original.style.removeProperty('filter'); color.remove(); });
  }
  function draw() {
    const elapsed = performance.now() - started;
    if (elapsed >= revealAt + duration) { clean(); return; }
    if (elapsed >= revealAt) {
      const scene = stage.getBoundingClientRect();
      const wake = stage.querySelector<HTMLElement>('#morning-waking')!.parentElement!.getBoundingClientRect();
      const img = stage.querySelector<HTMLImageElement>('#morning-waking img')!;
      // Normalize the clock origin so both the source and extracted frames align.
      const scale = Math.max(wake.width / (img.naturalWidth || 1216), wake.height / (img.naturalHeight || 832));
      const cx = wake.left + (wake.width - (img.naturalWidth || 1216) * scale) / 2 + (70/1216) * (img.naturalWidth || 1216) * scale;
      const cy = wake.top + (wake.height - (img.naturalHeight || 832) * scale) / 2 + (465/832) * (img.naturalHeight || 832) * scale;
      const far = Math.max(...[scene.left,scene.right].flatMap(x => [scene.top,scene.bottom].map(y => Math.hypot(x-cx,y-cy))));
      const progress = (elapsed - revealAt) / duration;
      const radius = far * 1.5 * progress ** .8;
      layers.forEach(({wrap,color}) => {
        const rect = wrap.getBoundingClientRect();
        const points = Array.from({length:96},(_,i) => {
          const angle = i / 96 * Math.PI * 2;
          const rough = 1 + .16*Math.sin(angle*5+.7) + .12*Math.sin(angle*11+1.8) + .07*Math.cos(angle*19);
          return [cx-rect.left+Math.cos(angle)*radius*rough,cy-rect.top+Math.sin(angle)*radius*rough];
        });
        let path = points.map(([x,y],i)=>`${i?'L':'M'}${x.toFixed(1)},${y.toFixed(1)}`).join(' ')+' Z';
        // Detached splashes lead the jagged main front rather than a perfect circle.
        for(let i=0;i<9;i++) {
          const a=i*2.399, distance=radius*(1.16+.13*Math.sin(i*7));
          const x=cx-rect.left+Math.cos(a)*distance,y=cy-rect.top+Math.sin(a)*distance;
          const r=radius*(.012+.006*(i%3));
          path+=` M${x-r},${y} a${r},${r} 0 1,0 ${r*2},0 a${r},${r} 0 1,0 ${-r*2},0 Z`;
        }
        color.style.clipPath=`path('${path}')`;
      });
    }
    frame=requestAnimationFrame(draw);
  }
  frame=requestAnimationFrame(draw);
  return { stop() { cancelAnimationFrame(frame); clean(); } };
}
