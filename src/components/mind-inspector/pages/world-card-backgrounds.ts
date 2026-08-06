/**
 * 世界状态卡片背景图映射
 */

export type TimePeriod = 'morning' | 'noon' | 'night' | 'latenight';

export type SeasonKey = 'spring' | 'summer' | 'autumn' | 'winter';
export type WeatherKey = 'sunny' | 'cloudy' | 'rainy' | 'snowy' | 'foggy' | 'storm';

export function getTimePeriod(hour: number): TimePeriod {
  if (hour < 5) return 'latenight';
  if (hour < 11) return 'morning';
  if (hour < 17) return 'noon';
  return 'night';
}

export function getSeasonKey(season: string | undefined): SeasonKey {
  switch (season?.toLowerCase()) {
    case 'spring': return 'spring';
    case 'summer': return 'summer';
    case 'autumn': return 'autumn';
    case 'winter': return 'winter';
    default: return 'spring';
  }
}

export function getWeatherKey(code?: number): WeatherKey {
  if (code == null) return 'cloudy';
  if (code === 0) return 'sunny';
  if (code <= 3) return 'cloudy';
  if (code <= 48) return 'foggy';
  if (code <= 67) return 'rainy';
  if (code <= 77) return 'snowy';
  if (code <= 82) return 'rainy';
  if (code <= 86) return 'snowy';
  if (code >= 95) return 'storm';
  return 'cloudy';
}

// dev 模式走 Vite dev server，prod 模式走 model:// 自定义协议加载嵌入的加密资源
const BASE = import.meta.env.PROD ? 'http://model.localhost/world-bg' : '/world-bg';

export const CARD_BG: Record<string, string> = {
  'time-morning': `${BASE}/time-morning.webp`,
  'time-noon': `${BASE}/time-noon.webp`,
  'time-night': `${BASE}/time-night.webp`,
  'time-latenight': `${BASE}/time-latenight.webp`,

  'season-spring': `${BASE}/season-spring.webp`,
  'season-summer': `${BASE}/season-summer.webp`,
  'season-autumn': `${BASE}/season-autumn.webp`,
  'season-winter': `${BASE}/season-winter.webp`,

  'weather-sunny': `${BASE}/weather-sunny.webp`,
  'weather-cloudy': `${BASE}/weather-cloudy.webp`,
  'weather-rainy': `${BASE}/weather-rainy.webp`,
  'weather-snowy': `${BASE}/weather-snowy.webp`,
  'weather-foggy': `${BASE}/weather-foggy.webp`,
  'weather-storm': `${BASE}/weather-storm.webp`,

  media: `${BASE}/media.webp`,
  cpu: `${BASE}/cpu.webp`,
  memory: `${BASE}/memory.webp`,
  foreground: `${BASE}/foreground.webp`,
  network: `${BASE}/network.webp`,
  speed: `${BASE}/speed.webp`,
  volume: `${BASE}/volume.webp`,
  location: `${BASE}/location.webp`,
  presence: `${BASE}/presence.webp`,
  observation: `${BASE}/observation.webp`,
};
