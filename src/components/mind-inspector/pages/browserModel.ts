export interface BrowserTab { id: string; url: string; title: string; start: boolean; private: boolean; loading: boolean; can_back: boolean; can_forward: boolean; zoom: number; error: string | null }
export const browserLibraryPages = ['bookmarks', 'downloads', 'history', 'extensions'] as const;
export type BrowserLibraryPage = typeof browserLibraryPages[number];
export function browserLibraryPage(url?: string): BrowserLibraryPage | null {
  return browserLibraryPages.find(page => url === `about:blank#vivian-${page}`) ?? null;
}
export const browserLibraryUrl = (page: BrowserLibraryPage) => `about:blank#vivian-${page}`;
export interface BrowserEntry { url: string; title: string; at: number }
export interface BrowserState {
  background_tasks: number;
  revision: number; tabs: BrowserTab[]; active: string | null; history: BrowserEntry[]; bookmarks: BrowserEntry[];
  permissions: { id: string; tab_id: string; origin: string; kind: number }[];
  downloads: { id: string; path: string; received: number; total: number; state: number }[];
  extensions: { id: string; name: string; enabled: boolean }[]; error: string | null;
}
export const emptyBrowser: BrowserState = { background_tasks: 0, revision: -1, tabs: [], active: null, history: [], bookmarks: [], permissions: [], downloads: [], extensions: [], error: null };
/** Native events can overtake command replies. Never replace a newer state. */
export function mergeBrowserState(current: BrowserState, incoming: BrowserState): BrowserState { return incoming.revision >= current.revision ? incoming : current; }
export function browserEntries(entries: readonly BrowserEntry[], query: string): BrowserEntry[] {
  const terms = query.toLocaleLowerCase().trim().split(/\s+/).filter(Boolean);
  return [...entries].reverse().filter(entry => terms.every(term => `${entry.title}\n${entry.url}`.toLocaleLowerCase().includes(term)));
}
export function browserBytes(bytes: number): string {
  if (bytes < 0) return '—';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1048576).toFixed(1)} MB`;
}
