import { characterPresentation } from '../characterPackages';
/**
 * 底边贴边姿态（`edge-bottom`）的按角色裁切位置。
 *
 * 贴边时窗口被移到「舞台竖向中线 = 屏幕下沿」，精灵格的中线因此就是屏幕边缘。
 * 内置角色的校准由 `scripts/calibrate-chibi-bottom-dock.mjs` 实测生成，
 * 让双手之间的脸部贴住屏幕下沿。角色包在 presentation.bottomDock 中配置自己的裁切位置。
 * 精灵保留默认比例，旧角色包的缩放配置只用于换算原裁切线的位置。
 */

import config from './bottomDock.json';
import { defaultSizeTransform, DEFAULT_SIZE_ORIGIN_PERCENT } from './defaultSize';

export const BOTTOM_DOCK_ORIGIN_PERCENT = DEFAULT_SIZE_ORIGIN_PERCENT;

interface DockSizeTransform {
  scale: number;
  /** CSS `translate` 的 Y 百分比；1% = 5.12 个格内像素。 */
  translateY: number;
}

const TABLE = config.characters as Record<string, DockSizeTransform>;

/**
 * 裁切线在格子高度的 50%。将原贴边变换逆算到默认比例下的同一行，
 * 再平移这一行到裁切线，保留角色包已有的接触位置。
 */
export function bottomDockStyle(character: string): { scale: string; translate: string } {
  const dock = characterPresentation(character, 'bottomDock') ?? TABLE[character];
  const base = defaultSizeTransform(character);
  const cropFromOrigin = 50 - BOTTOM_DOCK_ORIGIN_PERCENT;
  const dockOffset = dock ? cropFromOrigin - (cropFromOrigin - dock.translateY) / dock.scale : 0;
  const translateY = (base?.translateY ?? 0) + dockOffset;
  return {
    scale: String(base?.scale ?? 1),
    translate: `0% ${+translateY.toFixed(6)}%`,
  };
}
