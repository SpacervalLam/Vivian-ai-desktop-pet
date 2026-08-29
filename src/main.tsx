import { StrictMode } from 'react';
import type { ReactElement } from 'react';
import { createRoot } from 'react-dom/client';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { setCharacterId } from './characterContext';

// Live2D Cubism Core SDK 通过 script 标签动态注入到 window，扩展全局 Window 类型
declare global {
  interface Window {
    Live2DCubismCore?: unknown;
  }
}

const root = document.getElementById('root');
if (!root) {
  throw new Error('找不到 #root 挂载节点');
}
const container: HTMLElement = root;

/** 动态加载 Live2D Cubism Core SDK（仅在主窗口需要） */
async function loadCubismCore(): Promise<void> {
  if (typeof window !== 'undefined' && window.Live2DCubismCore) return;
  await new Promise<void>((resolve, reject) => {
    const script = document.createElement('script');
    script.src = '/live2dcubismcore.min.js';
    script.onload = () => resolve();
    script.onerror = () => reject(new Error('Failed to load Live2D Cubism Core SDK'));
    document.head.appendChild(script);
  });
}

/** 显示错误消息（非透明背景，确保用户可见） */
function showError(msg: string): void {
  document.documentElement.classList.remove('is-transparent');
  document.body.style.background = '#1e1e28';
  createRoot(container).render(
    <div style={{ padding: 20, color: '#ff6b6b', fontFamily: 'monospace', fontSize: 14, background: '#1e1e28', minHeight: '100vh' }}>
      <pre style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-all' }}>{msg}</pre>
    </div>
  );
}

// 非-Tauri 环境（如 trae-preview 浏览器标签页）没有 __TAURI_INTERNALS__，
// getCurrentWindow() 会抛 "Cannot read properties of undefined (reading 'metadata')"。
const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
if (!isTauri) {
  showError('Non-Tauri environment. This app must run in a Tauri window.');
} else {
  const params = new URLSearchParams(window.location.search);
  const view = params.get('view');
  // 预创建的隐藏窗口（如 bubble/toast）由父窗口主动 show，main.tsx 不自动显示
  const hidden = params.get('hidden') === '1';

  // 设置当前窗口的角色身份
  const currentWindowLabel = getCurrentWindow().label;
  const characterIdParam = params.get('character_id');
  if (characterIdParam) {
    // 角色私有子窗口：显式传了 character_id（如 vivian_bubble / nana_bubble）
    setCharacterId(characterIdParam);
  } else if (!view) {
    // 角色主窗口（无 view 参数）：label 就是 character_id（如 vivian / nana）
    setCharacterId(currentWindowLabel !== 'main' ? currentWindowLabel : null);
  } else {
    // 共享子窗口（有 view 但无 character_id）：不绑定角色，由内部三视图切换决定
    setCharacterId(null);
  }

  // main 窗口是 tauri.conf.json 预定义的隐藏控制器窗口（visible:false），
  // 不加载任何 UI — 角色窗口由 lib.rs 按需创建（label = character_id）。
  const isHiddenController = currentWindowLabel === 'main' && !view;

  // ⚠️ App 必须动态 import()：App.tsx → Live2DCanvas → pixi-live2d-display/cubism4
  // 模块链在求值阶段会检查 window.Live2DCubismCore，
  // 必须在 dynamic import('./App') 之前加载 cubism SDK。
  void (async () => {
    try {
      // 隐藏控制器窗口不渲染任何 UI，也跳过 i18n/样式加载，保持最小内存足迹
      if (isHiddenController) {
        return;
      }

      // i18n 初始化有副作用（i18next init），必须在渲染任何窗口组件前完成
      await import('./i18n');
      await import('./styles/global.css');

      let element: React.ReactElement;
      switch (view) {
        case 'chat': {
          const ChatWindow = (await import('./components/ChatWindow')).default;
          element = <ChatWindow />;
          break;
        }
        case 'input': {
          // 群发总框：独立窗口，居中显示，broadcast 模式
          const InputDialog = (await import('./components/InputDialog')).default;
          element = <InputDialog broadcast visible />;
          break;
        }
        case 'config': {
          const ConfigWindow = (await import('./components/ConfigWindow')).default;
          element = <ConfigWindow />;
          break;
        }
        case 'memory': {
          const MemoryWindow = (await import('./components/MemoryWindow')).default;
          element = <MemoryWindow />;
          break;
        }
        case 'bubble': {
          const BubbleWindow = (await import('./components/BubbleWindow')).default;
          element = <BubbleWindow />;
          break;
        }
        case 'toast': {
          const ToastWindow = (await import('./components/ToastWindow')).default;
          element = <ToastWindow />;
          break;
        }
        case 'side_chat': {
          const SideChatPanel = (await import('./components/SideChatPanel')).default;
          element = <SideChatPanel />;
          break;
        }
        case 'message_banner': {
          const MessageBannerWindow = (await import('./components/MessageBannerWindow')).default;
          element = <MessageBannerWindow />;
          break;
        }
        default: {
          // 仅主窗口（无 view 参数）加载 App 及其 Live2D 依赖链
          // cubism SDK 必须在 App 模块求值前加载完成，否则 pixi-live2d-display 会抛错
          await loadCubismCore();
          const AppLazy = (await import('./App')).default;
          element = <AppLazy />;
          break;
        }
      }

      // 主窗口（无 view）含 Live2D 重型初始化，StrictMode 双执行会触发两次加载，
      // 子窗口为纯 React 组件，保留 StrictMode 检测副作用。
      const tree: ReactElement = view
        ? <StrictMode>{element}</StrictMode>
        : element;
      createRoot(container).render(tree);

      // 子窗口 UI 渲染完成后显示窗口，避免空白窗口闪烁。
      // bubble/toast/input 为常驻隐藏窗口，由调用方主动 show，不自动显示。
      if (view && view !== 'chat' && view !== 'bubble' && view !== 'toast' && view !== 'input' && view !== 'side_chat' && view !== 'message_banner' && !hidden) {
        const showWindow = () => {
          void getCurrentWindow().show().then(() => {
            window.dispatchEvent(new CustomEvent('window-shown'));
          }).catch(() => {});
        };
        requestAnimationFrame(() => requestAnimationFrame(showWindow));
        setTimeout(showWindow, 2000);
      }
    } catch (e) {
      showError(String(e instanceof Error ? e.message : e));
    }
  })();
}
