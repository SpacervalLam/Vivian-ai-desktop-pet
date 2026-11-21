export interface WebSource {
  url: string; title?: string; snippet?: string; published_at?: string;
  source_id?: string; retrieved_at?: string; engines?: string[];
}
export interface WebEvidence {
  sources: WebSource[]; warnings: string[]; summary?: string;
  text?: string; nextOffset?: number; artifact?: { text_path?: string; pdf_path?: string };
}
export function parseWebEvidence(raw: string | undefined): WebEvidence | null {
  if (!raw) return null;
  try {
    const envelope = JSON.parse(raw);
    let data = envelope;
    const warnings: string[] = [];
    for (let depth = 0; depth < 3 && data && typeof data === 'object'; depth++) {
      if (data.success === false && typeof data.message === 'string') warnings.push(data.message);
      if (data.data && typeof data.data === 'object' && !data.results && !data.queries && !data.url && !data.text) data = data.data;
      else break;
    }
    if (!data || typeof data !== 'object') return null;
    const sources: WebSource[] = [];
    const seen = new Set<string>();
    const add = (source: WebSource) => {
      if (!source || typeof source.url !== 'string') return;
      try { const url = new URL(source.url); if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password) return; } catch { return; }
      if (seen.has(source.url)) return;
      seen.add(source.url);
      sources.push({ ...source, title: typeof source.title === 'string' ? source.title : source.url,
        snippet: typeof source.snippet === 'string' ? source.snippet : undefined,
        published_at: typeof source.published_at === 'string' ? source.published_at : undefined,
        engines: Array.isArray(source.engines) ? source.engines.filter((e): e is string => typeof e === 'string') : [] });
    };
    for (const batch of Array.isArray(data.queries) ? data.queries : [data]) {
      if (!batch || typeof batch !== 'object') continue;
      if (Array.isArray(batch.results)) batch.results.forEach(add);
      if (typeof batch.error === 'string') warnings.push(batch.error);
      if (Array.isArray(batch.warnings)) warnings.push(...batch.warnings.filter((w: unknown): w is string => typeof w === 'string'));
    }
    if (typeof data.url === 'string') add(data);
    if (!sources.length && !warnings.length && typeof data.text !== 'string') return null;
    return { sources, warnings, summary: typeof data.focused_summary === 'string' ? data.focused_summary : undefined,
      text: typeof data.text === 'string' ? data.text : undefined,
      nextOffset: typeof data.next_offset === 'number' ? data.next_offset : undefined,
      artifact: data.artifact && typeof data.artifact === 'object' ? {
        text_path: typeof data.artifact.text_path === 'string' ? data.artifact.text_path : undefined,
        pdf_path: typeof data.artifact.pdf_path === 'string' ? data.artifact.pdf_path : undefined,
      } : undefined };
  } catch { return null; }
}
