import { StrictMode } from 'react';
import type { ComponentType, ReactElement } from 'react';
import { createRoot } from 'react-dom/client';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import { setCharacterId } from './characterContext';
import { mountApartment } from './utils/apartmentLoader';
import { initializeCharacterPackages } from './characterPackages';
import { initializeOptionalResources } from './utils/optionalResources';

const WINDOW_COMPONENTS = new Map<string, () => Promise<{ default: ComponentType }>>([
  ['screen_watch', () => import('./components/ScreenWatchWindow')],
  ['screen_freeze', () => import('./components/ScreenFreezeWindow')],
  ['screen_selection', () => import('./components/ScreenSelectionWindow')],
  ['edge_menu', () => import('./components/EdgeMenuWindow')],
  ['chat', () => import('./components/ChatWindow')],
  ['input', () => import('./components/QuickInputWindow')],
  ['config', () => import('./components/ConfigWindow')],
  ['reminders', () => import('./components/ReminderWindow')],
  ['memory', () => import('./components/MemoryWindow')],
  ['bubble', () => import('./components/BubbleWindow')],
  ['toast', () => import('./components/ToastWindow')],
  ['message_banner', () => import('./components/MessageBannerWindow')],
]);

// These windows control when they become visible.
const MANUALLY_SHOWN_VIEWS = new Set(['chat', 'bubble', 'toast', 'input', 'message_banner', 'reminders']);

const root = document.getElementById('root');
if (!root) {
  throw new Error('找不到 #root 挂载节点');
}
const container: HTMLElement = root;
const params = new URLSearchParams(window.location.search);

function showError(msg: string): void {
  document.documentElement.classList.remove('is-transparent');
  document.body.style.background = '#1e1e28';
  createRoot(container).render(
    <div style={{ padding: 20, color: '#ff6b6b', fontFamily: 'monospace', fontSize: 14, background: '#1e1e28', minHeight: '100vh' }}>
      <pre style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-all' }}>{msg}</pre>
    </div>
  );
}

const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
if (!isTauri && params.get('view') === 'rig_preview') {
  void (async () => {
    await import('./styles/global.css');
    const Preview = (await import('./components/SkeletalRigPreview')).default;
    createRoot(container).render(<Preview />);
  })();
} else if (!isTauri) {
  showError('Non-Tauri environment. This app must run in a Tauri window.');
} else {
  const view = params.get('view');
  // 预创建的隐藏窗口（如 bubble/toast）由父窗口主动 show，main.tsx 不自动显示
  const hidden = params.get('hidden') === '1';

  const currentWindowLabel = getCurrentWindow().label;
  const characterIdParam = params.get('character_id');
  if (characterIdParam) {
    // 角色私有子窗口：显式传了 character_id（如 vivian_bubble / nana_bubble）
    setCharacterId(characterIdParam);
  } else if (!view) {
    // 角色主窗口（无 view 参数）：label 就是 character_id（如 vivian / nana）
    setCharacterId(currentWindowLabel);
  } else {
    // 共享子窗口（有 view 但无 character_id）：不绑定角色，由内部三视图切换决定
    setCharacterId(null);
  }

  void (async () => {
    try {
      const { listen } = await import('@tauri-apps/api/event');
      await listen<{ id: string; phase: string }>('character-package:changed', async event => {
        if (event.payload.phase !== 'loaded') return;
        await initializeCharacterPackages();
        window.dispatchEvent(new Event('character-packages:changed'));
        if (!view && currentWindowLabel === event.payload.id) window.location.reload();
      });
      await initializeCharacterPackages();
      await initializeOptionalResources();
      if (view === 'room') {
        await mountApartment(container);
        return;
      }

      // i18n 初始化有副作用（i18next init），必须在渲染任何窗口组件前完成
      await import('./i18n');
      await import('./styles/global.css');

      const loadWindow = view ? WINDOW_COMPONENTS.get(view) : undefined;
      const { default: WindowComponent } = await (loadWindow ? loadWindow() : import('./App'));
      const element = <WindowComponent />;

      // 主窗口仍由窗口级生命周期管理，子窗口保留 StrictMode 检测副作用。
      const tree: ReactElement = view
        ? <StrictMode>{element}</StrictMode>
        : element;
      createRoot(container).render(tree);

      if (view && !MANUALLY_SHOWN_VIEWS.has(view) && !hidden) {
        // 子窗口 UI 渲染完成后显示窗口，避免空白窗口闪烁。
        const showWindow = () => {
          void getCurrentWindow().show().then(() => {
            window.dispatchEvent(new CustomEvent('window-shown'));
          }).catch(() => {});
        };
        requestAnimationFrame(() => requestAnimationFrame(showWindow));
        setTimeout(showWindow, 2000);
      }
    } catch (e) {
      if (view === 'room') void invoke('set_room_mode', { active: false }).catch(() => {});
      showError(String(e instanceof Error ? e.message : e));
    }
  })();
}
