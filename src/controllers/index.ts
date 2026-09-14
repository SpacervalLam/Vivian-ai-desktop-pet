/**
 * 控制器层
 *
 * UI 组件通过这些单例与后端交互，避免在组件中直接调用 invoke。
 */

export { BubbleController } from './BubbleController';
export { ChatController } from './ChatController';
export { StreamController } from './StreamController';
export { LifecycleController } from './LifecycleController';
export type { InitGreetingResult } from './LifecycleController';
export type { ChatHandlers } from './ChatController';
