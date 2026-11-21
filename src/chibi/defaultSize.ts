import { characterPresentation } from '../characterPackages';
/**
 * 四个角色默认 idle 素材的尺寸归一。
 *
 * 四套素材是在不同镜头尺度下画的：包围盒高度 419~468px（同一个 512 格），脚底还分
 * 480 / 493 两组，而地面阴影是固定的。直接改素材会连带破坏所有入场帧与逐帧校准表，
 * 所以改为在**精灵的父节点**上挂一层统一变换（见 `.chibi-pet-default-size`）。
 *
 * 为什么必须挂在父节点：`frameStyle()` 用 CSS 的独立属性 `scale` / `translate`
 * 承载逐帧校准，若在同一元素上再写全局缩放，内联样式会互相覆盖，而且
 * `frameScales.json` 里存在 90% 之外的多种 transform-origin，没法简单相乘。
 * 父节点用自己的 `transform-origin: 50% 90%` 整体缩放，浏览器会把子节点的逐帧
 * 变换先算完再复合，等于「整段人物镜头缩放」，idle 与所有动作帧一视同仁。
 *
 * CSS 组合矩阵（O = transform-origin，T = translate，S = scale）：
 *   p' = O + T + S * (p - O)
 * 数值由 `scripts/calibrate-chibi-default-size.mjs` 实测素材生成，勿手改。
 */

import config from './defaultSize.json';

const TABLE = config.characters as Record<string, { scale: number; translateY: number }>;

export const DEFAULT_SIZE_TARGET = config.target;
export const DEFAULT_SIZE_ORIGIN_PERCENT = config.originPercent;

export interface DefaultSizeTransform {
  scale: number;
  /** CSS `translate` 的 Y 百分比；1% = 5.12 个格内像素。 */
  translateY: number;
}

export function defaultSizeTransform(character: string): DefaultSizeTransform | null {
  return (characterPresentation(character, 'defaultSize') ?? TABLE[character]) ?? null;
}

/**
 * 供 `.chibi-pet-default-size` 包裹层直接作为 React `style` 使用。
 * 未收录的角色（将来新增）返回恒等变换，保持原有渲染不变。
 */
export function defaultSizeStyle(character: string): { scale: string; translate: string } {
  const entry = defaultSizeTransform(character);
  if (!entry) return { scale: '1', translate: '0% 0%' };
  return {
    scale: String(entry.scale),
    translate: `0% ${entry.translateY}%`,
  };
}
