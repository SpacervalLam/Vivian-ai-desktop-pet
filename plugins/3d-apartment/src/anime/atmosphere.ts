/**
 * 大气状态的唯一真源。
 *
 * 旧实现里 `scene.fog` 有三处写入：RoomScene 的 roomFog、RoomScene.setEnvironment、
 * districtArt.setEnvironment。最后一次写入来自 districtArt（near 85 / far 300），
 * 把 `dormLayout.json` 里 postfx.fog 配的 18 / 115 整个盖掉——配置改了画面不动，
 * 而且按时段调雾时两套参数互相打架。现在：
 *   - 距离基准只从 postfx.fog 读，时段/天气只做倍率；
 *   - 天空色只从 skyPalette 出，雾色跟着地平线走，远景不会和天空断成两条色带；
 *   - 只有 RoomScene 写 scene.fog，districtArt 只读本模块、不再碰全局状态。
 */
import * as THREE from 'three';

export interface FogSpec {
  color?: string;
  near?: number;
  far?: number;
}

export interface SkyPalette {
  top: string;
  horizon: string;
  night: number;
  cloud: number;
  glow: number;
}

/** 时段 / 天气 → 天空与雾色。原来散在 districtArt.setEnvironment 里的字面量集中到这里。 */
export function skyPalette(period: string, weather: string): SkyPalette {
  const overcast = weather === 'storm';
  const night = period === 'night';
  const dusk = period === 'dusk';
  return {
    top: overcast ? '#182b3c' : night ? '#09172e' : dusk ? '#424d79' : '#468ab8',
    horizon: overcast ? '#4b626f' : night ? '#334b60' : dusk ? '#e6ae94' : '#c1d9dc',
    night: night ? 1 : 0,
    cloud: weather === 'clear' ? .22 : overcast ? 1 : .7,
    glow: night ? 1.5 : dusk ? .9 : 0,
  };
}

/**
 * 分档倍率。分子是旧 districtArt 的绝对值（暴雨 32/140、夜 55/230），
 * 分母是当前晴天基准（85/300）——折算成倍率后，改 postfx.fog 一个数字，
 * 三个时段会按同一比例跟着变，不再各写一份。
 */
const FOG_SCALE: Record<string, { near: number; far: number }> = {
  storm: { near: 32 / 85, far: 140 / 300 },
  night: { near: 55 / 85, far: 230 / 300 },
  clear: { near: 1, far: 1 },
};

/** 唯一的 THREE.Fog 构造器。返回 null 表示 postfx 里没配雾。 */
export function fogFor(
  spec: FogSpec | null | undefined,
  period: string,
  weather: string,
  sky: SkyPalette,
  fallbackNear: number,
  fallbackFar: number,
): THREE.Fog | null {
  if (!spec) return null;
  const scale = weather === 'storm' ? FOG_SCALE.storm : period === 'night' ? FOG_SCALE.night : FOG_SCALE.clear;
  const color = new THREE.Color(sky.horizon);
  if (spec.color) color.lerp(new THREE.Color(spec.color), 0.12);
  return new THREE.Fog(color, (spec.near ?? fallbackNear) * scale.near, (spec.far ?? fallbackFar) * scale.far);
}
