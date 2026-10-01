/**
 * 验证脚本用的环境注入表。
 *
 * 房间的时段/天气改由真实世界感知驱动后，脚本不再直接设 `period`/`weather`，
 * 而是注入一份「感知 input」（本地小时 + 日出日落 + WMO 码），由场景内部自己
 * 推导出去。这样无头验证和线上跑的是同一条映射路径，切时段的边界逻辑才算真被测到。
 *
 * dusk 取日落整点：用 DUSK_LEAD=1h 的窗口看，18.5 正好落在窗口内。
 */
const PERIOD_HOUR = { morning: 8, noon: 12, dusk: 18.5, night: 22 };
const WEATHER_CODE = { clear: 0, drizzle: 61, storm: 95, snow: 75 };

/** @param {'morning'|'noon'|'dusk'|'night'} period @param {'clear'|'drizzle'|'storm'|'snow'} weather */
export const envSrc = (period, weather) => ({
  hour: PERIOD_HOUR[period],
  sunriseHour: 6,
  sunsetHour: 18.5,
  weatherCode: WEATHER_CODE[weather],
});
