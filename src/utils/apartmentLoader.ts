import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { getCurrentWindow, LogicalPosition } from '@tauri-apps/api/window';

interface ApartmentRuntime {
  mount(container: HTMLElement, assetRoot: string): void;
  unmount(): void;
}

/** Host bridge only. Scene code, loading UI and React are owned by the plugin. */
export async function mountApartment(container: HTMLElement): Promise<void> {
  const status = await invoke<{ enabled: boolean; asset_root: string | null; plugin_root: string | null }>('apartment_plugin_status');
  if (!status.enabled || !status.asset_root || !status.plugin_root) {
    throw new Error('3D 公寓插件未安装、已禁用或文件不完整');
  }
  const loading = document.createElement('div');
  loading.id = 'apartment-loading';
  loading.textContent = 'loading…';
  loading.style.cssText = 'position:fixed;inset:0;display:grid;place-items:center;color:#aaa;background:#1e1e28';
  document.body.appendChild(loading);
  try {
    const win = getCurrentWindow();
    await win.setPosition(new LogicalPosition(0, 0));
    await win.show();
    await win.setFocus();
    await invoke('set_room_mode', { active: true });
    if (import.meta.env.DEV) {
      const entry = '/plugins/3d-apartment/src/apartmentPlugin.tsx';
      await import(/* @vite-ignore */ entry);
    } else {
      await new Promise<void>((resolve, reject) => {
        const script = document.createElement('script');
        script.src = convertFileSrc(`${status.plugin_root}/ui/room.js`);
        script.onload = () => resolve();
        script.onerror = () => reject(new Error('加载 3D 公寓插件脚本失败'));
        document.head.appendChild(script);
      });
    }
    const runtime = (window as Window & { VivianApartment?: ApartmentRuntime }).VivianApartment;
    if (!runtime) throw new Error('3D 公寓插件入口未注册');
    runtime.mount(container, status.asset_root);
  } finally {
    loading.remove();
  }
}
