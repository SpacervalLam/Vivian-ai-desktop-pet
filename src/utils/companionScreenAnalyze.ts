/**
 * 聊天窗口头部下拉里的「截屏分析」。
 *
 * 单独成模块而不是埋在 ChatWindow 里：那个文件 5000+ 行，一个需要被测试的 async
 * 动作挂在 JSX 上方根本没法单独覆盖。角色归因由后端按当前活跃角色决定，前端不传
 * character_id —— 聊天窗口没有「当前会话角色」这个概念。
 */
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';

export interface ScreenAnalyzeResult {
  cancelled?: boolean;
  handled?: boolean;
  saved_path?: string;
  copied?: boolean;
}

export interface ScreenAnalyzeMessages {
  /** 截图已落盘到系统截图目录 */
  saved: string;
  /** 截图已进剪贴板 */
  copied: string;
}

/**
 * 框选屏幕并交给视觉模型。返回值只用于调用方判断是否需要额外反馈。
 *
 * **分析走桌宠转述，不走 toast**：识图完成后后端会把客观分析交给桌宠，用角色自己的
 * 口吻 emit 成气泡（`companion:interaction`）。这里只剩「纯保存 / 纯复制」这两种
 * 没有分析可转述的情况需要落点提示，以及请求本身失败时的错误提示。
 */
export async function analyzeScreen(messages: ScreenAnalyzeMessages): Promise<ScreenAnalyzeResult | null> {
  let result: ScreenAnalyzeResult;
  try {
    result = await invoke<ScreenAnalyzeResult>('companion_screen_analyze');
  } catch (error) {
    void emit('toast:show', { message: String(error), type: 'error', duration: 4000, key: Date.now() });
    return null;
  }
  if (result.cancelled || result.handled) return result;
  if (result.saved_path) void emit('toast:show', { message: messages.saved, type: 'success', duration: 2500, key: Date.now() });
  else if (result.copied) void emit('toast:show', { message: messages.copied, type: 'success', duration: 2500, key: Date.now() });
  return result;
}
