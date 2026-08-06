/**
 * 当前窗口的角色身份上下文。
 *
 * 每个角色窗口（label = character_id）和子窗口（URL 携带 character_id 参数）
 * 在 main.tsx 启动时调用 setCharacterId 设置身份，之后全窗口生命周期不变。
 *
 * 非 React 组件（ChatController、BubbleController、TtsStreamQueue 等）
 * 直接调用 getCharacterId() 获取当前角色 ID，无需 React Context。
 */

let currentCharacterId: string | null = null;

export function setCharacterId(id: string | null): void {
  currentCharacterId = id;
}

export function getCharacterId(): string | null {
  return currentCharacterId;
}
