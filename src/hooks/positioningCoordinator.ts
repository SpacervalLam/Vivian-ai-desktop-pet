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
  /** 分层桌宠正在执行自主走路舞台调度；此时智能避让暂缓，避免争抢窗口位置。 */
  ambientMoveInFlight: boolean;
  /** 智能避让正在平滑移动窗口；自主走路必须等它完成。 */
  smartPositioningInFlight: boolean;
  /**
   * 桌宠正在执行「戳烦了逃离」的位移；此时智能避让与自主漫步都必须让路。
   *
   * 与 ambientMoveInFlight 分开记，是因为两者的优先级不同：自主漫步是可选调度，
   * 可以让给避让；而逃离是用户戳出来的当场反应，一旦起步就该独占窗口——所以它
   * 不仅拦住后来者，还会主动按停正在进行的避让（见 abortSmartMove）。
   */
  fleeInFlight: boolean;
  /**
   * 用户已经**把窗口拖起来了**（后端拖动会话进行中，窗口在跟着光标走）。
   *
   * 逃离要问的是「窗口归谁」：一旦人真的抓住了它，位移就该当场让位，否则两个写手
   * 同时往 `set_window_position` 里塞坐标，桌宠来回抖。光看「手指按在桌宠身上多久」
   * 不够——窗口一旦滑开，光标就落到桌宠旁边的透明区上，按在那里画布是看不见的
   * （mousedown 走的是背景层），所以由 App 在真正开拖时把这件事记在这里。
   */
  dragInFlight: boolean;
  /**
   * 中止正在进行的智能避让滑动（useSmartPositioning 注册）。
   *
   * 逃离起步时调用：避让的滑动循环每一帧都会重读会话代号，自增即当帧退出，
   * 否则两个写手会同时往 set_window_position 里塞坐标，桌宠来回抖。
   */
  abortSmartMove: (() => void) | null;
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
  ambientMoveInFlight: false,
  smartPositioningInFlight: false,
  fleeInFlight: false,
  dragInFlight: false,
  abortSmartMove: null,
  triggerSmartCheck: null,
};
