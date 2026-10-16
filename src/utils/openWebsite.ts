import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-shell';

/** Website links share the desktop agent's foreground browser entry point. */
export async function openWebsite(url: string): Promise<void> {
  if (/^https?:\/\//i.test(url.trim())) {
    try {
      await invoke('open_embedded_browser', { url: url.trim() });
      return;
    } catch { /* Keep website links usable if the embedded browser is unavailable. */ }
  }
  await open(url);
}
