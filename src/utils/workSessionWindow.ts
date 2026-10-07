import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { emitTo, listen } from '@tauri-apps/api/event';
import { raiseWindow } from './windowRaiser';

/** Open the exact delegated session, including a hidden or still-mounting inspector. */
export async function openWorkSession(sessionId: string): Promise<void> {
  if (!sessionId.trim()) throw new Error('Missing work session');
  const existing = await WebviewWindow.getByLabel('memory');
  if (!existing) {
    const win = new WebviewWindow('memory', {
      url: `/?view=memory&nav=code&work_session=${encodeURIComponent(sessionId)}`,
      title: '心智观察 · 办公', width: window.screen.width, height: window.screen.height,
      minWidth: 1260, minHeight: 896, resizable: true, decorations: false,
      transparent: false, shadow: true, alwaysOnTop: false, visible: false, dragDropEnabled: true,
    });
    await new Promise<void>((resolve, reject) => {
      void win.once('tauri://created', () => resolve());
      void win.once('tauri://error', event => reject(new Error(String(event.payload))));
    });
    return;
  }
  // Acknowledgement avoids losing a navigation event while a prewarmed window mounts.
  const requestId = crypto.randomUUID();
  let accepted = false;
  const unlisten = await listen<{ requestId: string }>('memory:navigation-accepted', event => {
    if (event.payload.requestId === requestId) accepted = true;
  });
  try {
    await raiseWindow(existing, 'memory');
    for (let attempt = 0; attempt < 20 && !accepted; attempt++) {
      await emitTo('memory', 'memory:navigate', { page: 'code', workSessionId: sessionId, requestId });
      await new Promise(resolve => window.setTimeout(resolve, 150));
    }
    if (!accepted) throw new Error('办公页尚未就绪，请重试');
  } finally { unlisten(); }
}
