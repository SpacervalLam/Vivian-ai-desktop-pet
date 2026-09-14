/**
 * NavigationContext — Mind Inspector 跨页导航上下文
 *
 * 允许任意子组件（如 MindPage 的 attention top-3）触发页面跳转，
 * 而无需层层传递 setActiveNav 回调。
 *
 * Provider 在 MindInspector.tsx 中提供。
 */

import { createContext, useContext } from 'react';
import type { ReactNode } from 'react';
import type { NavKey } from './design-system';

export type PageParams = {
  diaryId?: string;
  diaryCharacter?: 'vivian' | 'nana';
  [key: string]: unknown;
};

interface NavigationContextValue {
  /** 跳转到指定页面 */
  navigateTo: (page: NavKey, params?: PageParams) => void;
  /** 当前激活的页面 */
  activePage: NavKey;
  /** 当前页面参数 */
  pageParams: PageParams;
  /** 清除页面参数 */
  clearPageParams: () => void;
  /** 注入到共享标题行右侧的页面工具栏节点 */
  headerExtra: ReactNode;
  /** 设置标题行工具栏节点（页面卸载时应清空） */
  setHeaderExtra: (node: ReactNode) => void;
}

const NavigationContext = createContext<NavigationContextValue | null>(null);

export const NavigationProvider = NavigationContext.Provider;

/** 获取跨页导航能力。在非 MindInspector 子树中调用会返回 null。 */
export const useNavigation = (): NavigationContextValue | null =>
  useContext(NavigationContext);

export default NavigationContext;
