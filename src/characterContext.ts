/**
 * 当前窗口的角色身份上下文。
 *
 * 每个角色窗口（label = character_id）和子窗口（URL 携带 character_id 参数）
 * 在 main.tsx 启动时调用 setCharacterId 设置身份，之后全窗口生命周期不变。
 *
 * 非 React 组件（ChatController、BubbleController、TtsStreamQueue 等）
 * 直接调用 getCharacterId() 获取当前角色 ID，无需 React Context。
 */

import { invoke } from '@tauri-apps/api/core';

let currentCharacterId: string | null = null;

export function setCharacterId(id: string | null): void {
  currentCharacterId = id;
}

export function getCharacterId(): string | null {
  return currentCharacterId;
}

// 头像 URL 解析缓存：同一角色 URL 不变，只 rpc 一次，后续同步返回
const avatarUrlCache = new Map<string, Promise<string>>();

/**
 * 解析角色头像（public/icons/charaters/<id>.webp）的可加载 URL。
 *
 * 头像美术资源现放在普通 public 静态目录 `public/icons/charaters/<character_id>.webp`
 * （与 chibi 图集、provider logo 同机制），dev / release 均走同源绝对路径，不依赖
 * model.localhost 加密协议。统一走后端 get_avatar_url 解析，便于未来切换资源位置时
 * 只改后端、不动前端。后端解析失败时回退到 favicon。
 */
export function resolveAvatarUrl(characterId: string): Promise<string> {
  const cached = avatarUrlCache.get(characterId);
  if (cached) return cached;
  const pending = invoke<string>('get_avatar_url', {
    characterId,
  }).catch(() => '/favicon.ico');
  avatarUrlCache.set(characterId, pending);
  return pending;
}
