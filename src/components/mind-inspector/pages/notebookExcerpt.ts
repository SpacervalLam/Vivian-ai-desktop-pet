export interface NotebookExcerpt { kind: 'heading' | 'text'; text: string }

/** Plain reading copy for the comic paper; the isolated original HTML remains
 * available in the full notebook view. Never execute or inject notebook HTML. */
export function notebookExcerpt(html: string): NotebookExcerpt[] {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  doc.querySelectorAll('script,style,noscript,iframe,object,embed,template').forEach(node => node.remove());
  const blocks: NotebookExcerpt[] = [];
  const walk = (node: Node) => {
    if (node.nodeType === Node.TEXT_NODE) {
      const text = node.textContent?.trim();
      if (text) blocks.push({ kind: 'text', text });
      return;
    }
    if (!(node instanceof Element)) return;
    if (node.matches('h1,h2,h3,h4,h5,h6,p,li,pre,blockquote,td,th,figcaption')) {
      const text = node.textContent?.trim();
      if (text) blocks.push({ kind: /^H[1-6]$/.test(node.tagName) ? 'heading' : 'text', text });
    } else if (node.tagName === 'IMG') {
      const text = node.getAttribute('alt')?.trim();
      if (text) blocks.push({ kind: 'text', text });
    } else node.childNodes.forEach(walk);
  };
  walk(doc.body);
  return blocks;
}
