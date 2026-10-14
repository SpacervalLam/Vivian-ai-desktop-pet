import { comicTitles } from './comicMarkup.js';
// Extend the ORIGINAL compositions: each source occupies a former decorative
// text/photo area, not a separate appended management page.
export const bookSpreads = [9,6,8,1,4,2,3,7,5].map(mind => ({
  title: comicTitles[mind], mind, sources: ({ 3: ['diary'], 5: ['planner'], 6: ['memory'], 7: ['notebook'] })[mind] ?? [],
}));
export function adjacentBookSpread(index, direction) {
  return Math.max(0, Math.min(bookSpreads.length - 1, index + Math.sign(direction)));
}
export const bookSlots = {
  diary: { r: [48, 0, 52, 100], m: [3, 54, 94, 46] },
  planner: { r: [0, 24, 62, 76], m: [3, 54, 94, 46] },
  memory: { r: [47, 3, 53, 49], m: [3, 57, 94, 40] },
  notebook: { r: [0, 0, 29, 100], m: [0, 0, 47, 43] },
};

// Only ink/paper is slanted, never the interactive DOM. The notebook's right
// edge stays parallel to Vivian's left edge. The diary uses a full-height
// rectangular paper beside two stacked café panels.
export function bookFrame(source, mobile = false) {
  // The inset cuts the bottom-right corner; the reveal refines this in pixels
  // so its square image and 9px gutter remain aligned at every canvas size.
  if (source === 'notebook') return mobile ? '0,0 100,0 100,100 0,100' : '0,0 100,0 100,65 60.6,65 60.6,100 0,100';
  if (source === 'diary') return '0,0 100,0 100,100 0,100';
  if (source === 'planner') return mobile ? '3,0 100,0 97,100 0,100' : '4.9032,0 100,0 95.0968,100 0,100';
  return '0,0 100,0 100,100 0,100';
}

export function bookDestination(destination, params = {}) {
  const source = ({ graph: 'memory', profile: 'memory', diary: 'diary', journal: 'diary', notebook: 'notebook', planner: 'planner', todo: 'planner', scheduler: 'planner' })[destination];
  const spread = source ? bookSpreads.findIndex(page => page.sources.includes(source)) : 0;
  const detailed = params.diaryId || params.notebookId || params.sub === 'profile' || destination === 'profile';
  return { spread, focus: detailed ? source : null };
}
