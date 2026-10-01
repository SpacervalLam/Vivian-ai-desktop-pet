/**
 * 房间「真实世界感知 → 时段/天气」映射的回归测试。
 *
 * 直接转译 worldEnvironment.ts 跑，不依赖任何浏览器或 Tauri 上下文。
 * 之所以值得单独跑一道：这段逻辑错了不会报错，只会让房间在夏天的傍晚提前天黑、
 * 或者在大晴天凭空下起雨来 —— 两种都很难靠肉眼 review 抓出来。
 *
 *   node plugins/3d-apartment/tools/room/check-world-environment.mjs
 *
 * 改动 worldEnvironment.ts 的窗口常量（DUSK_LEAD / MORNING_SPAN 等）
 * 或 WMO 码分档之后请务必重跑。
 */
import { readFileSync } from 'node:fs';
import { transformSync } from 'esbuild';

const SRC = new URL('../../../../plugins/3d-apartment/src/worldEnvironment.ts', import.meta.url);

const ts = transformSync(readFileSync(SRC, 'utf8'), { format: 'esm', loader: 'ts' });
const { resolveEnvironment, environmentFromLocalClock } = await import(
  `data:text/javascript;base64,${Buffer.from(ts.code).toString('base64')}`
);

/** 夏天 vs 冬天的日照差：这是选「真实日出日落」而非固定小时阈值的全部理由 */
const SUMMER = { sunriseHour: 4.9, sunsetHour: 19.1 };
const WINTER = { sunriseHour: 7.2, sunsetHour: 16.6 };

const periodCases = [
  ['summer 03:00 深夜', { hour: 3, ...SUMMER }, 'night'],
  ['summer 04:24 刚进晨光窗', { hour: 4.4, ...SUMMER }, 'morning'],
  ['summer 07:00 早晨', { hour: 7, ...SUMMER }, 'morning'],
  ['summer 08:12 出早晨窗', { hour: 8.2, ...SUMMER }, 'noon'],
  ['summer 13:00 正午', { hour: 13, ...SUMMER }, 'noon'],
  ['summer 18:18 日落前', { hour: 18.3, ...SUMMER }, 'dusk'],
  ['summer 19:18 日落后尾', { hour: 19.3, ...SUMMER }, 'dusk'],
  ['summer 20:00 已入夜', { hour: 20, ...SUMMER }, 'night'],
  ['summer 23:00 深夜', { hour: 23, ...SUMMER }, 'night'],
  // 冬天 18:00 天已经黑透。固定的 `<17=正午` 阈值会把它误判成 noon。
  ['winter 18:00 天已黑', { hour: 18, ...WINTER }, 'night'],
  ['winter 16:12 日落前', { hour: 16.2, ...WINTER }, 'dusk'],
  ['winter 08:00 早晨', { hour: 8, ...WINTER }, 'morning'],
  ['winter 12:00 正午', { hour: 12, ...WINTER }, 'noon'],
  // 未定位时的固定日照兜底
  ['fallback 22:00', { hour: 22, sunriseHour: null, sunsetHour: null }, 'night'],
  ['fallback 12:00', { hour: 12, sunriseHour: null, sunsetHour: null }, 'noon'],
  // 高纬短白天：dusk/morning 窗口几乎相接，中间的正午不能被吃掉
  ['polar 13:00 短白天', { hour: 13, sunriseHour: 9.5, sunsetHour: 15 }, 'noon'],
];

const weatherCases = [
  [0, 'clear'], [1, 'clear'], [3, 'clear'],
  [45, 'clear'], [48, 'clear'], // 雾 / 雾凇：clear 档语义是「无降水」
  [51, 'drizzle'], [55, 'drizzle'], [61, 'drizzle'], [63, 'drizzle'], [67, 'drizzle'],
  [65, 'storm'], // 大雨
  [71, 'snow'], [75, 'snow'], [85, 'snow'], [86, 'snow'],
  [80, 'drizzle'], [82, 'storm'], // 阵雨 / 强阵雨
  [95, 'storm'], [96, 'storm'], [99, 'storm'],
  [null, 'clear'], // 天气数据缺失
  [999, 'clear'], // 未知码必须退到无降水，而不是最强降水档
];

let failed = 0;
const check = (name, got, want) => {
  const ok = got === want;
  if (!ok) failed++;
  console.log(`${ok ? '  ok  ' : ' FAIL '} ${name.padEnd(26)} => ${got}${ok ? '' : `   (期望 ${want})`}`);
};

console.log('\n时段（设备所在的真实日出日落驱动）');
for (const [name, input, want] of periodCases) {
  check(name, resolveEnvironment({ ...input, weatherCode: 0 }).period, want);
}

console.log('\n天气（WMO 码）');
for (const [code, want] of weatherCases) {
  check(`code=${String(code).padEnd(20)}`, resolveEnvironment({ hour: 12, weatherCode: code }).weather, want);
}

const local = environmentFromLocalClock();
if (!Number.isFinite(local.hour) || local.hour < 0 || local.hour >= 24) {
  console.log(` FAIL  本地时钟兜底产出非法 hour: ${local.hour}`);
  failed++;
}

console.log(failed === 0 ? '\n全部通过\n' : `\n${failed} 项失败\n`);
process.exit(failed ? 1 : 0);
