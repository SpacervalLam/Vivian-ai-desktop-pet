import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

/** Keep persistent auxiliary webviews in sync with the saved UI language. */
export function synchronizeWindowLanguage(changeLanguage: (language: string) => Promise<unknown>) {
  let disposed = false;
  let revision = 0;
  let unlisten: (() => void) | undefined;
  const ready = (async () => {
    const cleanup = await listen<{ language: string }>('config:language-changed', event => {
      if (disposed || !event.payload?.language) return;
      revision++;
      void changeLanguage(event.payload.language).catch(console.warn);
    });
    if (disposed) { cleanup(); return; }
    unlisten = cleanup;
    const before = revision;
    const language = await invoke<string | null>('get_config', { key: 'base.language' });
    if (!disposed && before === revision && language) await changeLanguage(language);
  })().catch(console.warn);
  return {
    ready,
    dispose: () => { disposed = true; unlisten?.(); },
  };
}
