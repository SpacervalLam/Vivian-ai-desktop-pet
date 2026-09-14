/**
 * 房间场景的独立预览入口。
 *
 * main.tsx 在非 Tauri 环境会直接报错退出（它依赖 __TAURI_INTERNALS__），
 * 所以想直接在浏览器里看美术效果得走这个入口。它只挂 RoomScene，
 * 不碰桌宠渲染 / Tauri / i18n。
 *
 * 仅用于开发调试：vite build 的默认入口只有 index.html，这个文件不会进产物。
 */

import { createRoot } from 'react-dom/client';
import { RoomScene } from './components/room/RoomScene';

const el = document.getElementById('root');
if (el) {
  createRoot(el).render(<RoomScene />);
}
