import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { currentMonitor, getCurrentWindow } from '@tauri-apps/api/window';

let creating: Promise<WebviewWindow> | undefined;
export async function openReminderWindow(important = false): Promise<void> {
  const existing = await WebviewWindow.getByLabel('reminders');
  let win = existing;
  if (!win) {
    creating ??= (async () => {
      const [monitor, scale] = await Promise.all([currentMonitor(), getCurrentWindow().scaleFactor()]);
      const window = new WebviewWindow('reminders', {
        url: 'index.html?view=reminders', title: 'Reminders', width: 380, height: 440,
        x: monitor ? (monitor.position.x + monitor.size.width) / scale - 404 : undefined,
        y: monitor ? monitor.position.y / scale + 60 : undefined,
        decorations: false, transparent: true, shadow: false, resizable: true,
        visible: false, focus: false, alwaysOnTop: important, skipTaskbar: false,
      });
      try {
        await new Promise<void>((resolve, reject) => {
          let settled = false;
          const listeners: Array<() => void> = [];
          const finish = (error?: Error) => {
            if (settled) return;
            settled = true; clearTimeout(timer); listeners.forEach(unlisten => unlisten());
            if (error) reject(error); else resolve();
          };
          const timer = setTimeout(() => finish(new Error('Reminder window creation timed out')), 10_000);
          const track = (unlisten: () => void) => { if (settled) unlisten(); else listeners.push(unlisten); };
          void window.once('tauri://created', () => finish()).then(track).catch(error => finish(new Error(String(error))));
          void window.once('tauri://error', event => finish(new Error(String(event.payload)))).then(track).catch(error => finish(new Error(String(error))));
        });
        return window;
      } catch (error) {
        // Another character webview may have created the shared inbox concurrently.
        const shared = await WebviewWindow.getByLabel('reminders');
        if (shared) return shared;
        throw error;
      }
    })().finally(() => { creating = undefined; });
    win = await creating;
  }
  if (important) await win.setAlwaysOnTop(true);
  await win.unminimize();
  await win.show();
  await win.emit('reminder:surface-shown');
}
