/**
 * 主动位移协调器 —— 模块级单例，在 useFullscreenHiding 与 useSmartPositioning
 * 之间共享状态，避免两个 hook 同时驱动窗口位移导致桌宠闪烁。
 *
 * 冲突场景：
 * - 切换到全屏窗口瞬间：useFullscreenHiding 启动 hide 动画，同时
 *   useSmartPositioning 的失焦事件触发 check(true)，可能与 hide 动画并发。
 * - 切回普通窗口瞬间：useFullscreenHiding 启动 restore 动画移回原位，
 *   同时屏幕已变化触发 useSmartPositioning 把桌宠拉到纯色位置，与 restore 抢夺控制权。
 *
 * 协调规则（useFullscreenHiding 优先级更高，因为是用户主动切换全屏的强信号）：
 * 1. `fullscreenInFlight=true` 时，smart positioning 完全跳过
 * 2. `fullscreenHidden=true` 时，smart positioning 完全跳过（桌宠已隐藏到角落）
 * 3. restore 完成后通过 `triggerSmartCheck` 立即触发一次强制屏幕捕获，
 *    把桌宠移动到当前屏幕最纯色位置
 */

export interface PositioningCoordinator {
  /** 桌宠当前是否处于全屏隐藏状态（已退到角落） */
  fullscreenHidden: boolean;
  /** 全屏隐藏 hook 是否正在执行 hide/restore 动画 */
  fullscreenInFlight: boolean;
  /**
   * useSmartPositioning 启动时注册的强制检查回调。
   * restore 完成后调用，立即触发一次跳过 unchanged 优化的屏幕捕获，
   * 将桌宠移动到当前屏幕最纯色位置。
   */
  triggerSmartCheck: (() => void) | null;
}

export const positioningCoordinator: PositioningCoordinator = {
  fullscreenHidden: false,
  fullscreenInFlight: false,
  triggerSmartCheck: null,
};
