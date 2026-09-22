import { StrictMode } from 'react';
import type { ReactElement } from 'react';
import { createRoot } from 'react-dom/client';
import { getCurrentWindow, LogicalPosition } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import { setCharacterId } from './characterContext';
import { bootMark, dismissBootLoader } from './utils/roomBoot';

// 主 chunk 求值完成。它与 index.html 那个 'html' 节点之间的差值，就是
// 「HTML 已可绘制 → 主 chunk 可执行」的网络+解析开销。
bootMark('main');

const root = document.getElementById('root');
if (!root) {
  throw new Error('找不到 #root 挂载节点');
}
const container: HTMLElement = root;
const initialParams = new URLSearchParams(window.location.search);

/** 显示错误消息（非透明背景，确保用户可见） */
function showError(msg: string): void {
  // 公寓窗口的首屏 loading 层必须一起撤掉，否则它会把错误信息整个盖住——
  // 用户看到的是「一直转圈」而不是「加载失败的原因」，是最难排查的那种表现。
  dismissBootLoader();
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
if (!isTauri && initialParams.get('view') === 'rig_preview') {
  void (async () => {
    await import('./styles/global.css');
    const Preview = (await import('./components/SkeletalRigPreview')).default;
    createRoot(container).render(<Preview />);
  })();
} else if (!isTauri) {
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

  // 各窗口按需动态加载，主桌宠使用轻量 Q 版图集，不再预载 Cubism SDK。
  void (async () => {
    try {
      // 隐藏控制器窗口不渲染任何 UI，也跳过 i18n/样式加载，保持最小内存足迹
      if (isHiddenController) {
        return;
      }

      // i18n 初始化有副作用（i18next init），必须在渲染任何窗口组件前完成
      await import('./i18n');
      await import('./styles/global.css');
      bootMark('i18n+css');

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
        case 'room': {
          // 3D 宿舍房间窗口。不用 cubism，但用同样的 dynamic import 模式
          // 避开 StrictMode 下的双执行副作用。
          // 这一段动态 import 是房间 chunk（three.js 全量，约 1.1MB）。实测生产
          // 构建里只占 ~120ms——真正的大头是后面的场景同步装配与首帧着色器编译，
          // 所以别看到这个 1MB 就去拆包，收益很小。单独打点只是为了留证据。
          bootMark('room:chunk-start');
          const RoomWindow = (await import('./components/room/RoomWindow')).default;
          bootMark('room:chunk-done');
          element = <RoomWindow />;
          break;
        }
        default: {
          // 仅主窗口（无 view 参数）加载桌宠应用。
          const AppLazy = (await import('./App')).default;
          element = <AppLazy />;
          break;
        }
      }

      // 主窗口仍由窗口级生命周期管理，子窗口保留 StrictMode 检测副作用。
      const tree: ReactElement = view
        ? <StrictMode>{element}</StrictMode>
        : element;
      createRoot(container).render(tree);

      // 房间窗口自己完成「落地」：定位 → 显形 → 聚焦 → 让位，全部不依赖调用方。
      //
      // 为什么不能挂在 React 渲染之后：RoomScene 那一段同步场景装配会阻塞主线程
      // 好几秒，rAF 与 setTimeout 在那期间都排不上，兜底形同虚设，窗口只能等装配
      // 完才冒出来。首屏是 index.html 里静态画好的 loading 层，创建即可见，本来
      // 就没有「渲染完才敢显形」的理由。触发点在 render() 之后、React commit 之前，
      // 仍在装配之前。
      //
      // 为什么必须自己做完：心智观察器入口点开公寓时会把自己关掉，它那个 JS 上下文
      // 一旦销毁，入口侧发给房间窗口的 show / setPosition / setFocus 全部丢失。
      // 这条就是那种情况下房间唯一的显形路径，不是可有可无的兜底。
      if (view === 'room' && !hidden) {
        void (async () => {
          const win = getCurrentWindow();
          // 窗口尺寸等于屏幕尺寸，居中与 (0,0) 其实是同一处；显式定死，为的是
          // 不依赖调用方那次 setPosition 有没有送达。
          await win.setPosition(new LogicalPosition(0, 0)).catch(() => {});
          await win.show().catch(() => {});
          await win.setFocus().catch(() => {});
          // 让位：心智观察器与桌宠都是置顶窗口，不让开的话房间窗口就算 show 了
          // 也整个压在它们下面。命令幂等，入口那边也会调一次。
          void invoke('set_room_mode', { active: true }).catch((e) =>
            console.warn('[room] set_room_mode(true) 失败', e)
          );
        })();
      } else if (view && view !== 'chat' && view !== 'bubble' && view !== 'toast' && view !== 'input' && view !== 'side_chat' && view !== 'message_banner' && !hidden) {
        // 子窗口 UI 渲染完成后显示窗口，避免空白窗口闪烁。
        // bubble/toast/input 为常驻隐藏窗口，由调用方主动 show，不自动显示。
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
