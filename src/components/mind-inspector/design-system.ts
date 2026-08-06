/**
 * Mind Inspector 设计系统
 *
 * 视觉参考：iOS 18 Dark Mode / Apple Vision Pro / Arc Browser / Linear
 * 风格：continuous corners + 磨砂玻璃 + 系统色 + 弹簧动画
 */

import type { LucideIcon } from 'lucide-react';
import {
  Brain,
  Globe,
  Zap,
  Network,
  BookHeart,
  UserCircle,
} from 'lucide-react';

// === 颜色（跟随 --panel-* 主题变量） ===
export const COLORS = {
  // 背景层级（从浅到深）
  bgDeep: 'var(--panel-bg)',
  bgBase: 'var(--panel-surface)',
  bgElevated: 'var(--panel-elevated)',
  bgSurface: 'var(--panel-bg-surface)',
  bgSurfaceElevated: 'var(--panel-bg-surface-elevated)',
  bgSurfaceHover: 'var(--panel-bg-active)',
  bgHover: 'var(--panel-bg-hover)',
  bgActive: 'var(--panel-bg-active)',
  sidebarBg: 'var(--panel-sidebar-bg)',
  subtleBg: 'var(--panel-subtle-bg)',
  subtleBorder: 'var(--panel-subtle-border)',
  gridLine: 'var(--panel-grid-line)',
  axisLine: 'var(--panel-axis-line)',

  // 主强调色
  accent: 'var(--panel-accent)',
  accentBright: 'var(--panel-accent-bright)',
  accentMuted: 'var(--panel-accent-muted)',
  accentSoft: 'var(--panel-accent-soft)',
  accentGlow: 'var(--panel-accent-muted)',
  accentLight: 'var(--panel-accent-muted)',
  accentGradient: 'linear-gradient(135deg, var(--panel-accent) 0%, var(--panel-text-secondary) 100%)',

  // 文本层级
  textPrimary: 'var(--panel-text)',
  textSecondary: 'var(--panel-text-secondary)',
  textTertiary: 'var(--panel-text-tertiary)',
  textQuaternary: 'var(--panel-text-quaternary)',

  // 选中态
  selectedBg: 'var(--panel-selected-bg)',
  selectedText: 'var(--panel-selected-text)',

  // 边框层级
  border: 'var(--panel-border)',
  borderHover: 'var(--panel-border-hover)',
  borderAccent: 'var(--panel-border-strong)',
  borderLight: 'var(--panel-border-light)',
  borderStrong: 'var(--panel-border-strong)',

  // 事件类型配色（功能色，不随主题变化）
  event: {
    dialogue: '#4CAF50',
    observation: '#9C27B0',
    belief: '#FF9800',
    goal: '#E91E63',
    relationship: '#FF5722',
    system: '#757575',
    mood: '#03A9F4',
    presence: '#4CAF50',
    inner: '#00BCD4',
    reading: '#2563EB',
  },

  // 系统色（功能色，不随主题变化）
  success: '#4CAF50',
  warning: '#FF9800',
  danger: '#E53935',
  info: '#2196F3',

  // 阴影系统
  shadow: {
    subtle: 'var(--panel-shadow-subtle)',
    card: 'var(--panel-shadow-card)',
    cardHover: 'var(--panel-shadow-elevated)',
    elevated: 'var(--panel-shadow-elevated)',
    glow: 'var(--panel-shadow-card)',
    inner: 'inset 0 1px 0 var(--panel-bg-surface)',
    sidebar: 'var(--panel-sidebar-shadow)',
  },
} as const;

// === 排版 ===
export const TYPO = {
  fontFamily:
    '"Caveat", "Ma Shan Zheng", "Dancing Script", "Hachi Maru Pop", -apple-system, BlinkMacSystemFont, "SF Pro Text", "SF Pro Display", "PingFang SC", "Microsoft YaHei", "Segoe UI", sans-serif',
  fontFamilyCN:
    '"Caveat", "Ma Shan Zheng", -apple-system, BlinkMacSystemFont, "SF Pro Text", "SF Pro Display", "PingFang SC", "Microsoft YaHei", "Segoe UI", sans-serif',
  fontFamilyEN:
    '"Caveat", "Dancing Script", -apple-system, BlinkMacSystemFont, "SF Pro Text", "SF Pro Display", "Segoe UI", sans-serif',
  fontFamilyJP:
    '"Hachi Maru Pop", -apple-system, BlinkMacSystemFont, "SF Pro Text", "SF Pro Display", "Hiragino Sans", "Yu Gothic", sans-serif',
  fontMono: 'ui-monospace, "SF Mono", Menlo, Consolas, monospace',
  largeTitle: { fontSize: 33.6, fontWeight: 700, letterSpacing: -0.6 },
  h1: { fontSize: 26.4, fontWeight: 600, letterSpacing: -0.4 },
  h2: { fontSize: 20.4, fontWeight: 600, letterSpacing: -0.2 },
  h3: { fontSize: 18, fontWeight: 600, letterSpacing: -0.1 },
  body: { fontSize: 16.8, fontWeight: 400, letterSpacing: -0.1 },
  caption: {
    fontSize: 13.2,
    fontWeight: 600,
    letterSpacing: 0.6,
    textTransform: 'uppercase' as const,
  },
  micro: { fontSize: 13.2, fontWeight: 500, letterSpacing: 0.1 },
} as const;

// === 间距 ===
export const SPACING = {
  xs: 4,
  sm: 8,
  md: 16,
  lg: 24,
  xl: 32,
  xxl: 48,
  cardPadding: 18,
  cardGap: 12,
  sectionGap: 24,
} as const;

// === 圆角（iOS continuous corners 风格） ===
export const RADIUS = {
  xs: 6,
  sm: 10,
  md: 14,
  lg: 18,
  xl: 22,
  xxl: 28,
  pill: 999,
} as const;

// === 动画曲线 ===
export const EASE = {
  // iOS 标准缓动（用于位移、缩放）
  ios: 'cubic-bezier(0.16, 1, 0.3, 1)',
  // iOS 弹簧效果（用于出现、状态变化）
  spring: 'cubic-bezier(0.34, 1.56, 0.64, 1)',
  // iOS Swift（用于颜色、透明度过渡）
  swift: 'cubic-bezier(0.2, 0.8, 0.2, 1)',
  // 减速曲线（用于进入动画）
  decel: 'cubic-bezier(0.1, 0.9, 0.2, 1)',
  // 加速曲线（用于退出动画）
  accel: 'cubic-bezier(0.4, 0, 1, 1)',
} as const;

export const DURATION = {
  fast: 0.18,
  normal: 0.28,
  slow: 0.45,
  slower: 0.6,
} as const;

// === 阴影（导出兼容旧引用） ===
export const SHADOW = COLORS.shadow;

// === 玻璃拟态 ===
export const GLASS = {
  base: 'backdrop-filter: blur(24px) saturate(180%); -webkit-backdrop-filter: blur(24px) saturate(180%);',
  strong: 'backdrop-filter: blur(40px) saturate(200%); -webkit-backdrop-filter: blur(40px) saturate(200%);',
  light: 'backdrop-filter: blur(16px) saturate(150%); -webkit-backdrop-filter: blur(16px) saturate(150%);',
} as const;

// === 侧边栏 ===
export const SIDEBAR = {
  widthCollapsed: 72,
  widthExpanded: 220,
  margin: 12,
} as const;

// === 角色配色（用于 Twin View 区分） ===
export const CHARACTER_ACCENT = {
  vivian: '#FFD60A',
  nana: '#C084FC',
} as const;

// === 导航项定义 ===
export type NavKey =
  | 'mind'
  | 'world'
  | 'graph'
  | 'diary'
  | 'profile';

export interface NavItem {
  key: NavKey;
  icon: LucideIcon;
  labelKey: string;
}

export const NAV_ITEMS: NavItem[] = [
  { key: 'mind', icon: Brain, labelKey: 'mind_inspector.nav_mind' },
  { key: 'world', icon: Globe, labelKey: 'mind_inspector.nav_world' },
  { key: 'graph', icon: Network, labelKey: 'mind_inspector.nav_graph' },
  { key: 'diary', icon: BookHeart, labelKey: 'mind_inspector.nav_diary' },
  { key: 'profile', icon: UserCircle, labelKey: 'mind_inspector.nav_profile' },
];
