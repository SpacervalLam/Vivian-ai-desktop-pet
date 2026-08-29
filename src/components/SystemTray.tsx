/**
 * 系统托盘事件路由组件
 *
 * 后端 `commands/system_tray.rs` 在托盘右键菜单点击时
 * 通过 `tray:menu_action` 事件广播 payload：{ action, character_id }
 *
 * 菜单项 ID 与 ContextMenu.tsx 中的 items 一一对应：
 *   memory / settings / chat / voice / smart_positioning / quit
 *
 * 多角色架构下，托盘点击只作用于活跃角色（后端在 payload 中带 character_id），
 * 每个角色窗口的 SystemTray 只响应对应自己的事件，避免两个桌宠同时响应。
 *
 * 注意：托盘左键单击不再触发任何动作（后端已禁用），右键弹出原生菜单。
 * 这样即使在两个角色都 Offline、窗口被 hide_window 隐藏时，
 * 用户仍可通过托盘右键菜单访问记忆管理 / 微信 / 设置等子窗口，
 * 也可通过「微信」入口发消息唤醒离线智能体。
 *
 * 窗口内右键菜单由 `ContextMenu` 组件独立实现，不经过本组件。
 * 该组件不渲染任何 UI，仅作为事件监听器挂载在 App 树中。
 */

import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { getCharacterId } from '../characterContext';

/** 系统托盘菜单动作类型（与后端 menu_id 模块一一对应） */
export type TrayMenuAction =
  | 'memory'
  | 'settings'
  | 'chat'
  | 'voice'
  | 'smart_positioning'
  | 'quit';

export interface SystemTrayProps {
  /** 打开记忆管理子窗口 */
  onOpenMemory?: () => void;
  /** 打开设置子窗口 */
  onOpenSettings?: () => void;
  /** 打开微信（AI Chat）子窗口 */
  onOpenChat?: () => void;
  /** 切换语音开关 */
  onToggleVoice?: () => void;
  /** 切换智能避让开关 */
  onToggleSmartPositioning?: () => void;
  /** 退出应用 */
  onQuit?: () => void;
}

interface TrayMenuActionPayload {
  action: string;
  character_id?: string;
}

const SystemTray: React.FC<SystemTrayProps> = (props) => {
  // 用 ref 持有最新的 props，让事件监听器无需重新注册
  const propsRef = useRef(props);
  useEffect(() => {
    propsRef.current = props;
  }, [props]);

  useEffect(() => {
    let unlistenFn: (() => void) | undefined;
    let cancelled = false;

    const setup = async () => {
      try {
        unlistenFn = await listen<TrayMenuActionPayload>('tray:menu_action', async (event) => {
          const payload = event.payload;
          // 多角色过滤：仅响应发给当前角色的托盘动作
          if (payload?.character_id && payload.character_id !== getCharacterId()) return;
          const action = (payload?.action ?? '') as TrayMenuAction;
          await routeMenuAction(action, propsRef.current);
        });
        if (cancelled) {
          unlistenFn();
          unlistenFn = undefined;
        }
      } catch (err) {
        console.warn('[SystemTray] 监听 tray:menu_action 失败:', err);
      }
    };

    void setup();

    return () => {
      cancelled = true;
      unlistenFn?.();
    };
  }, []);

  return null;
};

/** 路由菜单动作到对应回调 */
async function routeMenuAction(action: string, props: SystemTrayProps): Promise<void> {
  try {
    switch (action) {
      case 'memory':
        props.onOpenMemory?.();
        break;
      case 'settings':
        props.onOpenSettings?.();
        break;
      case 'chat':
        props.onOpenChat?.();
        break;
      case 'voice':
        props.onToggleVoice?.();
        break;
      case 'smart_positioning':
        props.onToggleSmartPositioning?.();
        break;
      case 'quit':
        props.onQuit?.();
        break;
      default:
        console.warn('[SystemTray] 未知托盘菜单动作:', action);
    }
  } catch (err) {
    console.warn(`[SystemTray] 处理动作 '${action}' 失败:`, err);
  }
}

/** 同步托盘菜单 CheckMenuItem 的勾选状态到后端
 *
 * 由前端在 voiceEnabled / smartPositioningEnabled 变化时调用，
 * 让后端原生菜单的勾选标记与前端 store 保持一致。
 * item_id 取值：'voice' / 'smart_positioning'
 */
export async function syncTrayMenuCheck(item_id: 'voice' | 'smart_positioning', checked: boolean): Promise<void> {
  try {
    await invoke('set_tray_menu_check', { itemId: item_id, checked });
  } catch (err) {
    console.warn(`[SystemTray] 同步菜单勾选失败 (${item_id}=${checked}):`, err);
  }
}

export default SystemTray;
