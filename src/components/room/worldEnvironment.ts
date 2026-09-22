/**
 * 真实世界感知 → 3D 房间环境的映射。
 *
 * 房间的时段与天气不再由 UI 手动指定，而是统一从这里推导：
 * 输入端只有三个真实世界观测量（本地小时、日出、日落）+ 一个 WMO 天气码，
 * 输出端是 RoomScene 内部那套 DayPeriod / WeatherKind。
 *
 * 这里刻意做成纯函数、零副作用、零 three.js 依赖：
 *  - 便于单测（时段切窗口在城市 job 里最容易算错边界）；
 *  - 让无头验证脚本能注入任意 input，走和真实完全相同的这条路径。
 */

export type DayPeriod = 'morning' | 'noon' | 'dusk' | 'night';
export type WeatherKind = 'clear' | 'drizzle' | 'storm' | 'snow';

export const PERIOD_LABELS: Record<DayPeriod, string> = {
  morning: '早晨',
  noon: '正午',
  dusk: '黄昏',
  night: '深夜',
};

export const WEATHER_LABELS: Record<WeatherKind, string> = {
  clear: '晴',
  drizzle: '小雨',
  storm: '暴雨',
  snow: '雪',
};

/** 映射输入：真实世界感知给出的那一小撮字段。hour 允许小数便于测试。 */
export interface EnvironmentInput {
  hour: number;
  sunriseHour?: number | null;
  sunsetHour?: number | null;
  /** WMO weather code（Open-Meteo / OpenWeatherMap 通用口径） */
  weatherCode?: number | null;
}

/** 时段窗口相对日出日落的偏移，单位小时。 */
const DUSK_LEAD = 1.0; // 日落前 1h 起算黄昏
const DUSK_TRAIL = 0.5; // 日落后 0.5h 仍留一段黄昏尾巴
const DAWN_LEAD = 0.5; // 日出前 0.5h 开始泛晨光
const MORNING_SPAN = 3.0; // 日出后 3h 算早晨

/** 定位失败时的缺省日照区间（近似本地平太阳时，够用于视觉分级）。 */
const FALLBACK_SUNRISE = 6;
const FALLBACK_SUNSET = 18.5;

const mod24 = (h: number) => ((h % 24) + 24) % 24;

const finiteOr = (v: number | null | undefined, fallback: number) =>
  typeof v === 'number' && Number.isFinite(v) ? v : fallback;

/**
 * 按真实日出日落切分四个时段。
 *
 * 之所以不用固定小时阈值：同一天的黄昏在夏天是 19:30、冬天是 17:00，
 * 固定阈值会让房间在夏天的傍晚提前天黑。既然后端已经给了按经纬度和日期
 * 算出的日出日落（NOAA 算法，或天气 API 的日字段），就没理由再退回常量。
 */
export function resolveDayPeriod(input: EnvironmentInput): DayPeriod {
  const hour = mod24(finiteOr(input.hour, 0));
  const sunrise = mod24(finiteOr(input.sunriseHour, FALLBACK_SUNRISE));
  const sunset = mod24(finiteOr(input.sunsetHour, FALLBACK_SUNSET));
  const daylight = mod24(sunset - sunrise);

  // 各观测量都取「自该事件起经过的小时数」，天然处理跨零点。
  const sinceSunrise = mod24(hour - sunrise);
  const sinceSunset = mod24(hour - sunset);

  if (sinceSunset >= 24 - DUSK_LEAD || sinceSunset <= DUSK_TRAIL) return 'dusk';
  if (sinceSunrise >= 24 - DAWN_LEAD || sinceSunrise <= MORNING_SPAN) return 'morning';
  if (sinceSunrise > daylight + DUSK_TRAIL && sinceSunrise < 24 - DAWN_LEAD) return 'night';
  return 'noon';
}

/**
 * WMO 天气码 → 房间可用的四档天气。
 *
 * 房间的 clear 档语义是「无降水」而非「无云」，所以雾（45/48）和阴天（<=3）
 * 一并落到这里 —— 这几档会触发降水粒子，错分会让房间凭空下起雨来。
 */
export function resolveWeatherKind(code?: number | null): WeatherKind {
  if (typeof code !== 'number' || !Number.isFinite(code)) return 'clear';
  const c = Math.trunc(code);

  if (c === 0 || c === 1 || c === 2 || c === 3) return 'clear';
  if (c === 45 || c === 48) return 'clear'; // 雾 / 沉积雾凇雾
  if (c >= 51 && c <= 67) return c === 65 ? 'storm' : 'drizzle'; // 毛毛雨→小雨档，大雨(65)算暴雨
  if (c >= 71 && c <= 77) return 'snow';
  if (c === 80 || c === 81) return 'drizzle';
  if (c === 82) return 'storm'; // 强阵雨
  if (c === 85 || c === 86) return 'snow';
  // 只认已知的雷暴码。写成 >= 95 的话，任何超出 WMO 表的未知码都会被当成暴雨，
  // 房间就凭空下起雨来了 —— 未知该往无降水档退，而不是往最强降水档退。
  if (c === 95 || c === 96 || c === 99) return 'storm';
  return 'clear';
}

export function resolveEnvironment(input: EnvironmentInput): { period: DayPeriod; weather: WeatherKind } {
  return {
    period: resolveDayPeriod(input),
    weather: resolveWeatherKind(input.weatherCode),
  };
}

/** 无 Tauri 上下文（room-preview.html）时的兜底输入：只有本地时钟，天气不可知。 */
export function environmentFromLocalClock(now: Date = new Date()): EnvironmentInput {
  return {
    hour: now.getHours() + now.getMinutes() / 60,
    sunriseHour: null,
    sunsetHour: null,
    weatherCode: null,
  };
}

/** 供 HUD 展示的一行摘要。 */
export function describeEnvironment(env: { period: DayPeriod; weather: WeatherKind }, source: string): string {
  return `${PERIOD_LABELS[env.period]} · ${WEATHER_LABELS[env.weather]} · ${source}`;
}
