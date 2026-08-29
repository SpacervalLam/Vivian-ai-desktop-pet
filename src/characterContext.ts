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
 * 解析角色头像（icon.png）的可加载 URL。
 *
 * 头像与 Live2D 模型同目录打包进加密 bundle；构建后 dist 中 {Vivian,Nana}
 * 目录会被 vite 的 strip-encrypted-assets 移除，硬编码 `/Nana/icon.png` 在
 * 生产环境 404（横幅 onError 会回退到 favicon，导致两个角色头像都是同一张图）。
 * 因此统一走后端 get_avatar_url：dev 返回 Vite 路径，release 返回 model 协议 URL。
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
