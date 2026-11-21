import { invoke } from '@tauri-apps/api/core';

const cache = new Map<string, { request: Promise<string | null>; expires: number }>();
const MAX_SITES = 128;
const SUCCESS_TTL = 30 * 60_000;
const FAILURE_TTL = 30_000;

export function browserSiteOrigin(url: string): string | null {
  try {
    const parsed = new URL(url);
    return ['http:', 'https:'].includes(parsed.protocol) && !parsed.username && !parsed.password ? parsed.origin : null;
  } catch { return null; }
}

/** Share pending requests and decoded image data across tabs, history and bookmarks. */
export function browserSiteIcon(origin: string): Promise<string | null> {
  const cached = cache.get(origin);
  if (cached && cached.expires > Date.now()) return cached.request;
  if (cache.size >= MAX_SITES) cache.delete(cache.keys().next().value!);
  const entry = { request: Promise.resolve<string | null>(null), expires: Number.POSITIVE_INFINITY };
  entry.request = invoke<string>('browser_site_icon', { url: origin })
    .then(value => typeof value === 'string' && value.startsWith('data:image/') ? value : null)
    .catch(() => null)
    .then(value => { entry.expires = Date.now() + (value ? SUCCESS_TTL : FAILURE_TTL); return value; });
  cache.set(origin, entry);
  return entry.request;
}
