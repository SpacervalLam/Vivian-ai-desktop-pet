import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { open as openFileDialog } from '@tauri-apps/plugin-dialog';
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import { Album, Check, CloudSun, Droplets, Globe, Heart, Images, LayoutGrid, Leaf, Monitor, MoreHorizontal, Moon, NotebookPen, ScanLine, Search, Settings, Share, ListFilter, Sun, Sunrise, Sunset, Thermometer, Trash2, Users, Wind, X } from 'lucide-react';
import { groupShortcuts, shortcutGroupNames } from '../utils/shortcutGroups';
import { shortcutIconBackground } from '../utils/shortcutIconBackground';
import { ShortcutIconCache } from '../utils/shortcutIconCache';
import { ShortcutBackgroundCache } from '../utils/shortcutBackgroundCache';
import { resolveAvatarUrl } from '../characterContext';
import { PhoneLockClock } from './PhoneScene';
import { ClockAppArtwork, PhotosAppArtwork, LockScreenFlashlight, LockScreenCamera } from './PhoneAppArtwork';
import './PhoneHomeScreen.css';

interface Shortcut { id: string; name: string; kind: 'application' | 'website'; target: string; group: string }
export interface PhoneNotification {
  id: string;
  title: string;
  content: string;
  timestamp: number;
  count: number;
  target: { kind: 'private'; characterId: string } | { kind: 'group' };
}
interface WeatherSourceSummary {
  source?: string;
  temp_max?: number | null;
  temp_min?: number | null;
  humidity_pct?: number | null;
  attribution_url?: string;
}
interface Weather {
  temperature: number;
  feels_like?: number;
  description: string;
  humidity?: number;
  wind_speed?: number;
  weather_code?: number;
  is_day?: boolean | null;
  sunrise_hour?: number | null;
  sunset_hour?: number | null;
  weather_source?: string;
  cached_at?: number;
  sources?: WeatherSourceSummary[];
  air_quality?: { aqi?: number | null; pm2_5_ug_m3?: number | null; standard?: string; source?: string } | null;
}
interface GalleryImageRecord {
  name: string;
  path: string;
  modified: number;
}
interface Props {
  awake: boolean;
  visible: boolean;
  unread: number;
  characterId: string;
  page: 'lock' | 'home';
  notifications: PhoneNotification[];
  onUnlock: () => void;
  onNotification: (notification: PhoneNotification) => void;
  onLine: () => void;
  onNotes: () => void;
  onScreen: () => void;
  onSettings: () => void;
  onContacts: () => void;
  onCamera: () => void;
  onClose: () => void;
}
const iconCache = new ShortcutIconCache();
const backgroundCache = new ShortcutBackgroundCache();

function AppIcon({ shortcut }: { shortcut: Shortcut }) {
  const [icon, setIcon] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    setIcon(null);
    void iconCache.get(`${shortcut.id}:${shortcut.kind}:${shortcut.target}`,
      () => invoke<string>('companion_shortcut_icon', { shortcutId: shortcut.id }))
      .then(value => { if (active) setIcon(value); });
    return () => { active = false; };
  }, [shortcut.id, shortcut.kind, shortcut.target]);
  return icon ? <img src={icon} alt="" onLoad={event => {
    const img = event.currentTarget;
    try {
      const color = backgroundCache.get(`${shortcut.id}:${shortcut.kind}:${shortcut.target}`, () => {
      const canvas = document.createElement('canvas'); canvas.width = canvas.height = 64;
      const context = canvas.getContext('2d', { willReadFrequently: true });
      if (!context) throw new Error('Canvas unavailable');
      context.drawImage(img, 0, 0, 64, 64);
      return shortcutIconBackground(context.getImageData(0, 0, 64, 64).data, 64, 64);
      });
      img.parentElement?.style.setProperty('background', color);
    } catch { /* Unreadable icons retain their fallback background. */ }
  }} onError={event => { event.currentTarget.parentElement?.style.removeProperty('background'); setIcon(null); }} />
    : shortcut.kind === 'application' ? <Monitor size={29} /> : <span>{shortcut.name.trim().slice(0, 1).toUpperCase() || <Globe size={29} />}</span>;
}

function LineIcon() {
  return <svg viewBox="0 0 60 60" aria-hidden="true"><path fill="white" d="M49 27c0-10-9-17-20-17S9 17 9 27c0 9 8 16 18 17l-1 6c0 1 1 1 2 0 5-3 21-13 21-23Z" /><text x="29" y="31" textAnchor="middle" fill="#06b947" fontSize="12" fontWeight="800" fontFamily="Arial,sans-serif">LINE</text></svg>;
}

/** 通知来源图标：私聊用角色头像，群聊用群组 glyph，加载失败退回首字。 */
function NotificationAvatar({ notification }: { notification: PhoneNotification }) {
  const characterId = notification.target.kind === 'private' ? notification.target.characterId : null;
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    if (!characterId) { setSrc(null); return; }
    void resolveAvatarUrl(characterId).then(url => { if (active) setSrc(url); });
    return () => { active = false; };
  }, [characterId]);
  if (!characterId) return <span className="phone-lock-app-icon is-group"><Users size={19} /></span>;
  return <span className="phone-lock-app-icon">
    {src ? <img src={src} alt="" onError={() => setSrc(null)} /> : <span className="phone-lock-app-fallback">{notification.title.trim().slice(0, 1).toUpperCase() || <LineIcon />}</span>}
  </span>;
}

const WORLD_CLOCKS: Array<{ timeZone: string; zh: string; ja: string; en: string }> = [
  { timeZone: 'Asia/Tokyo', zh: '东京', ja: '東京', en: 'Tokyo' },
  { timeZone: 'Europe/London', zh: '伦敦', ja: 'ロンドン', en: 'London' },
  { timeZone: 'America/New_York', zh: '纽约', ja: 'ニューヨーク', en: 'New York' },
  { timeZone: 'America/Los_Angeles', zh: '洛杉矶', ja: 'ロサンゼルス', en: 'Los Angeles' },
];

const pad2 = (value: number) => String(value).padStart(2, '0');

function weatherTone(weather: Weather | null) {
  if (!weather) return 'is-cloudy';
  if (weather.is_day === false) return 'is-night';
  const code = weather.weather_code ?? 0;
  if (code >= 95) return 'is-storm';
  if ((code >= 71 && code <= 77) || code === 85 || code === 86) return 'is-snow';
  if ((code >= 51 && code <= 67) || (code >= 80 && code <= 82)) return 'is-rain';
  if (code === 45 || code === 48) return 'is-fog';
  if (code === 2 || code === 3) return 'is-cloudy';
  return 'is-clear';
}

function ClockPage({ onBack }: { onBack: () => void }) {
  const { i18n } = useTranslation();
  const label = (zh: string, ja: string, en: string) => i18n.language.startsWith('zh') ? zh : i18n.language.startsWith('ja') ? ja : en;
  const [now, setNow] = useState(() => new Date());
  const [hour12, setHour12] = useState(() => localStorage.getItem('phone-clock-12h') === 'true');
  useEffect(() => {
    let timer = 0;
    const tick = () => {
      setNow(new Date());
      timer = window.setTimeout(tick, 1000 - (Date.now() % 1000) + 10);
    };
    tick();
    return () => window.clearTimeout(timer);
  }, []);
  const hours = now.getHours();
  const minutes = now.getMinutes();
  const seconds = now.getSeconds();
  const hourAngle = ((hours % 12) + minutes / 60 + seconds / 3600) * 30;
  const minuteAngle = (minutes + seconds / 60) * 6;
  const secondAngle = seconds * 6;
  const date = now.toLocaleDateString(i18n.language, { month: 'long', day: 'numeric', weekday: 'long' }).replace('星期', '周');
  const zoneRows = WORLD_CLOCKS.map(zone => {
    const parts = new Intl.DateTimeFormat('en-GB', { timeZone: zone.timeZone, hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).formatToParts(now);
    const part = (type: string) => parts.find(item => item.type === type)?.value ?? '--';
    const day = new Intl.DateTimeFormat(i18n.language, { timeZone: zone.timeZone, month: 'numeric', day: 'numeric' }).format(now);
    const hour = Number(part('hour'));
    return { ...zone, clock: hour12 ? `${hour % 12 || 12}:${part('minute')}` : `${part('hour')}:${part('minute')}`, day, hour };
  });
  return <section className="phone-app-page phone-clock-page" aria-label={label('时钟', '時計', 'Clock')}>
    <header className="phone-app-page-nav">
      <button type="button" className="phone-app-back" aria-label={label('返回主屏', 'ホームに戻る', 'Back to home')} onClick={onBack}>
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><path d="M15 19l-7-7 7-7" /></svg>
      </button>
      <span className="phone-app-page-title">{label('时钟', '時計', 'Clock')}</span>
      <button type="button" className="phone-app-back phone-liquid-glass phone-clock-format" aria-label={label('切换时间格式', '時刻表示を切替', 'Change time format')} aria-pressed={hour12} onClick={() => setHour12(previous => { localStorage.setItem('phone-clock-12h', String(!previous)); return !previous; })}>{hour12 ? '12h' : '24h'}</button>
    </header>
    <div className="phone-clock-hero">
      <span className="phone-clock-eyebrow">{label('本地时间', '現地時刻', 'Local time')}</span>
      <div className="phone-clock-dial">
        <svg viewBox="0 0 240 240" aria-hidden="true">
          <defs>
            <radialGradient id="phone-clock-face" cx="46%" cy="36%" r="72%">
              <stop offset="0" stopColor="var(--clock-face-top)" /><stop offset="1" stopColor="var(--clock-face-bottom)" />
            </radialGradient>
            <linearGradient id="phone-clock-rim" x1="0" y1="0" x2="1" y2="1">
              <stop stopColor="#ffffff66" /><stop offset=".42" stopColor="#ffffff12" /><stop offset=".72" stopColor="#ffffff40" /><stop offset="1" stopColor="#ffffff0d" />
            </linearGradient>
          </defs>
          <circle cx="120" cy="120" r="112" fill="url(#phone-clock-face)" stroke="url(#phone-clock-rim)" strokeWidth="2" />
          <circle cx="120" cy="120" r="102" fill="none" stroke="#ffffff12" />
          {Array.from({ length: 60 }, (_, index) => {
            const major = index % 5 === 0;
            return <line key={index} x1="120" y1={major ? 14 : 16} x2="120" y2={major ? 25 : 20}
              stroke={major ? 'var(--clock-ink)' : 'var(--clock-tick)'} strokeWidth={major ? 2 : 1} strokeLinecap="round"
              transform={`rotate(${index * 6} 120 120)`} />;
          })}
          <g fill="var(--clock-ink)" fontSize="13" fontWeight="500" textAnchor="middle" dominantBaseline="middle">
            <text x="120" y="45">12</text><text x="195" y="121">3</text><text x="120" y="197">6</text><text x="45" y="121">9</text>
          </g>
          <line x1="120" y1="120" x2="120" y2="66" stroke="var(--clock-ink)" strokeWidth="7" strokeLinecap="round" transform={`rotate(${hourAngle} 120 120)`} />
          <line x1="120" y1="120" x2="120" y2="42" stroke="var(--clock-ink)" strokeWidth="4" strokeLinecap="round" transform={`rotate(${minuteAngle} 120 120)`} />
          <line x1="120" y1="133" x2="120" y2="32" stroke="#ff453a" strokeWidth="1.7" strokeLinecap="round" transform={`rotate(${secondAngle} 120 120)`} />
          <circle cx="120" cy="120" r="5" fill="#ff453a" />
          <circle cx="120" cy="120" r="2" fill="#0b0e14" />
        </svg>
      </div>
      <div className="phone-clock-digital">{hour12 ? hours % 12 || 12 : pad2(hours)}:{pad2(minutes)}<small>{pad2(seconds)}</small>{hour12 && <em>{hours < 12 ? 'AM' : 'PM'}</em>}</div>
      <div className="phone-clock-date">{date}</div>
    </div>
    <section className="phone-world-clocks">
      <h2>{label('世界时钟', '世界時計', 'World clock')}</h2>
      {zoneRows.map(zone => <div className="phone-world-clock-row" key={zone.timeZone}>
        <span className="phone-world-clock-city">{label(zone.zh, zone.ja, zone.en)}<small>{zone.day}</small></span>
        <span className="phone-world-clock-period" aria-label={zone.hour >= 6 && zone.hour < 18 ? label('白天', '昼', 'Day') : label('夜晚', '夜', 'Night')}>{zone.hour >= 6 && zone.hour < 18 ? <Sun size={16} /> : <Moon size={16} />}</span>
        <span className="phone-world-clock-time">{zone.clock}{hour12 && <small>{zone.hour < 12 ? 'AM' : 'PM'}</small>}</span>
      </div>)}
    </section>
  </section>;
}

function WeatherPage({ weather, city, now, loading, onBack, onSettings }: {
  weather: Weather | null; city: string; now: Date; loading: boolean; onBack: () => void; onSettings: () => void;
}) {
  const { i18n } = useTranslation();
  const label = (zh: string, ja: string, en: string) => i18n.language.startsWith('zh') ? zh : i18n.language.startsWith('ja') ? ja : en;
  const source = weather?.sources?.[0];
  const high = source?.temp_max;
  const low = source?.temp_min;
  const feels = weather?.feels_like;
  const humidity = weather?.humidity ?? source?.humidity_pct;
  const wind = weather?.wind_speed;
  const aqi = weather?.air_quality?.aqi ?? null;
  const formatHour = (value?: number | null) => {
    if (value == null || !Number.isFinite(value)) return '—';
    const total = Math.round(value * 60);
    const hour = Math.floor(total / 60) % 24;
    const minute = total % 60;
    return `${pad2(hour)}:${pad2(minute)}`;
  };
  const minutesAgo = weather?.cached_at != null ? Math.max(0, Math.floor((now.getTime() / 1000 - weather.cached_at) / 60)) : null;
  const updated = minutesAgo == null ? '' : minutesAgo < 1
    ? label('刚刚更新', 'たった今更新', 'Updated just now')
    : label(`${minutesAgo} 分钟前更新`, `${minutesAgo} 分前に更新`, `Updated ${minutesAgo} min ago`);
  return <section className={`phone-app-page phone-weather-page ${weatherTone(weather)}`} aria-label={label('天气', '天気', 'Weather')}>
    <div className="phone-weather-sky" aria-hidden="true" />
    <header className="phone-app-page-nav">
      <button type="button" className="phone-app-back" aria-label={label('返回主屏', 'ホームに戻る', 'Back to home')} onClick={onBack}>
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round"><path d="M15 19l-7-7 7-7" /></svg>
      </button>
      <span className="phone-app-page-title">{city || label('本地天气', '現在地の天気', 'Local weather')}</span>
      <button type="button" className="phone-app-back phone-liquid-glass" onClick={onSettings} aria-label={label('天气设置', '天気設定', 'Weather settings')}><Settings size={20} /></button>
    </header>
    {weather ? <div className="phone-weather-content">
      <div className="phone-weather-hero">
        <span className="phone-weather-location">{label('我的位置', '現在地', 'My location')}</span>
        <h1>{city || label('本地天气', '現在地の天気', 'Local weather')}</h1>
        <strong>{Math.round(weather.temperature)}°</strong>
        <span>{weather.description}</span>
        <small>
          {high != null && low != null ? label(`最高 ${Math.round(high)}°  最低 ${Math.round(low)}°`, `最高 ${Math.round(high)}°  最低 ${Math.round(low)}°`, `H ${Math.round(high)}°  L ${Math.round(low)}°`) : ''}
          {feels != null ? label(` · 体感 ${Math.round(feels)}°`, ` · 体感 ${Math.round(feels)}°`, ` · Feels like ${Math.round(feels)}°`) : ''}
        </small>
      </div>
      {high != null && low != null && <div className="phone-weather-summary"><span>{label('今日温度', '今日の気温', 'Today’s temperature')}</span><div className="phone-weather-range"><b>{Math.round(low)}°</b><span><i style={{left: `${Math.max(0,Math.min(100,(weather.temperature-low)/Math.max(1,high-low)*100))}%`}} /></span><b>{Math.round(high)}°</b></div></div>}
      <div className="phone-weather-grid">
        <div className="phone-weather-card"><Thermometer size={17} /><span>{label('体感温度', '体感温度', 'Feels like')}</span><b>{feels != null ? `${Math.round(feels)}°` : '—'}</b></div>
        <div className="phone-weather-card"><Droplets size={17} /><span>{label('湿度', '湿度', 'Humidity')}</span><b>{humidity != null ? `${Math.round(humidity)}%` : '—'}</b></div>
        <div className="phone-weather-card"><Wind size={17} /><span>{label('风速', '風速', 'Wind')}</span><b>{wind != null ? `${Math.round(wind)} km/h` : '—'}</b></div>
        {aqi != null && <div className="phone-weather-card"><Leaf size={17} /><span>{label('空气质量', '空気質', 'Air quality')}</span><b>{`AQI ${Math.round(aqi)}`}</b></div>}
        <div className="phone-weather-card"><Sunrise size={17} /><span>{label('日出', '日の出', 'Sunrise')}</span><b>{formatHour(weather.sunrise_hour)}</b></div>
        <div className="phone-weather-card"><Sunset size={17} /><span>{label('日落', '日の入り', 'Sunset')}</span><b>{formatHour(weather.sunset_hour)}</b></div>
      </div>
      <p className="phone-weather-source"><Sun size={11} /> {weather.weather_source || 'Open-Meteo'}{updated ? ` · ${updated}` : ''}</p>
    </div> : <div className="phone-weather-empty">
      <CloudSun size={46} />
      <p>{loading ? label('正在获取天气…', '天気を取得中…', 'Loading weather…') : label('暂未获取到天气，请确认已启用天气并配置经纬度；若已配置，请检查网络后重新打开天气。', '天気と位置情報の設定、ネットワーク接続を確認してから開き直してください。', 'Check weather settings, location coordinates and your connection, then reopen Weather.')}</p>
    </div>}
  </section>;
}

const GALLERY_WALLPAPERS = [
  { id: 'wallpaper-manhattan', src: '/wallpapers/manhattan-dusk.webp', name: 'Manhattan Dusk' },
  { id: 'wallpaper-riverside', src: '/wallpapers/riverside-original-enhanced.webp', name: 'Riverside' },
];

function resolveGallerySrc(path: string) {
  return /^(data:|https?:|blob:|\/)/i.test(path) ? path : convertFileSrc(path);
}

function GalleryPage({ images, loading, onBack, onRefresh, onShared, onImport, onWallpaper }: {
  images: GalleryImageRecord[];
  loading: boolean;
  onBack: () => void;
  onRefresh: () => void;
  onShared: (id: string) => void;
  onImport: (folder: boolean) => Promise<void>;
  onWallpaper: (src: string) => void;
}) {
  const { i18n } = useTranslation();
  const label = (zh: string, ja: string, en: string) => i18n.language.startsWith('zh') ? zh : i18n.language.startsWith('ja') ? ja : en;
  const [tab, setTab] = useState<'library' | 'collections'>('library');
  const [filter, setFilter] = useState<'all' | 'chat' | 'wallpaper'>('all');
  const [columns, setColumns] = useState(5);
  const [selectMode, setSelectMode] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  const [searchOpen, setSearchOpen] = useState(false);
  const [search, setSearch] = useState('');
  const [menuOpen, setMenuOpen] = useState(false);
  const [viewer, setViewer] = useState<number | null>(null);
  const [hidden, setHidden] = useState<string[]>(() => { try { return JSON.parse(localStorage.getItem('phone-gallery-hidden') || '[]'); } catch { return []; } });
  const [selectionAction, setSelectionAction] = useState<'share' | 'remove' | null>(null);
  const [recipients, setRecipients] = useState<{ id: string; name: string }[]>([]);
  const [shareBusy, setShareBusy] = useState(false);
  const sharing = useRef(false);
  const [shareError, setShareError] = useState('');
  useEffect(() => {
    if (selectionAction !== 'share') return;
    let cancelled = false;
    setShareError('');
    void invoke<{ characters: { id: string; name: string }[] }>('list_characters').then(result => {
      if (!cancelled) setRecipients(result.characters);
    }).catch(error => { if (!cancelled) setShareError(String(error)); });
    return () => { cancelled = true; };
  }, [selectionAction]);
  const searchRef = useRef<HTMLInputElement>(null);
  const swipe = useRef<{x: number; y: number} | null>(null);
  const galleryRef = useRef<HTMLElement>(null);
  useEffect(() => {
    setMenuOpen(false);
    galleryRef.current?.querySelector('.phone-gallery-scroll')?.scrollTo({top: 0});
  }, [tab, filter]);
  useEffect(() => {
    if (!selectionAction && viewer == null) return;
    const frame = requestAnimationFrame(() => galleryRef.current?.querySelector<HTMLElement>('.phone-gallery-action-sheet button,.phone-gallery-viewer-bar button')?.focus());
    return () => cancelAnimationFrame(frame);
  }, [selectionAction, viewer]);
  useEffect(() => {
    if (!menuOpen) return;
    const dismiss = (event: PointerEvent) => {
      if (event.target instanceof Element && !event.target.closest('.phone-gallery-menu,.phone-gallery-actions')) setMenuOpen(false);
    };
    document.addEventListener('pointerdown', dismiss);
    return () => document.removeEventListener('pointerdown', dismiss);
  }, [menuOpen]);

  const chatItems = useMemo(() => images.map(image => ({
    id: image.path,
    src: resolveGallerySrc(image.path),
    name: image.name,
    modified: image.modified,
    category: 'chat' as const,
  })), [images]);
  const wallpaperItems = useMemo(() => GALLERY_WALLPAPERS.map(item => ({ ...item, modified: 0, category: 'wallpaper' as const })), []);
  const items = useMemo(() => [...chatItems, ...wallpaperItems], [chatItems, wallpaperItems]);
  const visible = useMemo(() => {
    const keyword = search.trim().toLowerCase();
    return items.filter(item => !hidden.includes(item.id) &&
      (filter === 'all' || item.category === filter)
      && (!keyword || item.name.toLowerCase().includes(keyword)));
  }, [items, filter, search, hidden]);

  useEffect(() => { setViewer(null); setSelected([]); setSelectMode(false); setSelectionAction(null); }, [tab, filter, search]);
  useEffect(() => { if (searchOpen) searchRef.current?.focus(); }, [searchOpen]);
  useEffect(() => {
    if (viewer == null) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.stopPropagation(); setViewer(null); }
      if (!visible.length) return;
      if (event.key === 'ArrowLeft') { event.preventDefault(); setViewer(index => index == null ? null : (index - 1 + visible.length) % visible.length); }
      if (event.key === 'ArrowRight') { event.preventDefault(); setViewer(index => index == null ? null : (index + 1) % visible.length); }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [viewer, visible.length]);

  const current = viewer == null ? null : visible[viewer];
  useEffect(() => {
    if (viewer == null || visible.length < 2) return;
    for (const offset of [-1, 1]) { const image = new Image(); image.src = visible[(viewer + offset + visible.length) % visible.length].src; }
  }, [viewer, visible]);
  const closeSelection = () => { setSelectMode(false); setSelected([]); setSelectionAction(null); setMenuOpen(false); };
  const albumTitle = filter === 'chat' ? label('最近保存', '最近保存', 'Recently saved') : label('壁纸', '壁紙', 'Wallpapers');
  const allSelected = visible.length > 0 && visible.every(item => selected.includes(item.id));
  const shareTo = async (id: string) => {
    if (sharing.current) return;
    sharing.current = true; setShareBusy(true); setShareError('');
    try {
      for (const item of visible.filter(item => selected.includes(item.id))) {
        let sourcePath = item.id;
        if (item.category === 'wallpaper' || /^(data:|https?:|blob:)/i.test(item.id)) {
          const response = await fetch(item.src);
          if (!response.ok) throw new Error(label('无法读取图片', '画像を読み込めません', 'Cannot read image'));
          const blob = await response.blob();
          const data = await new Promise<string>((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(String(reader.result)); reader.onerror = () => reject(reader.error); reader.readAsDataURL(blob); });
          sourcePath = await invoke<string>('save_temp_image', { base64Data: data.split(',')[1], mime: blob.type || 'image/webp' });
        }
        await invoke('send_image_message', { sourcePath, characterId: id, channel: 'wechat' });
        setSelected(previous => previous.filter(value => value !== item.id));
      }
      closeSelection(); onShared(id);
    } catch (error) { setShareError(String(error)); }
    finally { sharing.current = false; setShareBusy(false); }
  };
  const openItem = (item: (typeof items)[number]) => {
    if (selectMode) {
      setSelected(previous => previous.includes(item.id) ? previous.filter(id => id !== item.id) : [...previous, item.id]);
      return;
    }
    const index = visible.findIndex(entry => entry.id === item.id);
    if (index >= 0) setViewer(index);
  };
  const goToAlbum = (category: 'chat' | 'wallpaper') => {
    setTab('library');
    setFilter(category);
    setSearchOpen(false);
    setSearch('');
  };

  return <section ref={galleryRef} className={`phone-app-page phone-gallery-page${selectMode ? ' is-selecting' : ''}${filter !== 'all' ? ' is-album' : ''}`} aria-label={label('图库', 'ライブラリ', 'Library')} onKeyDownCapture={event => {
    if (event.key !== 'Escape') return;
    event.preventDefault(); event.stopPropagation();
    if (shareBusy) return;
    if (selectionAction) setSelectionAction(null); else if (viewer != null) setViewer(null); else if (menuOpen) setMenuOpen(false); else if (selectMode) closeSelection(); else onBack();
  }}>
    <div className="phone-gallery-scroll">
      {tab === 'library' ? <>
        <div className="phone-gallery-top is-static">
          {selectMode && filter !== 'all' && <button type="button" className="phone-gallery-select-all phone-liquid-glass" onClick={() => setSelected(allSelected ? [] : visible.map(item => item.id))}>{allSelected ? label('取消全选', '選択解除', 'Deselect all') : label('全选', 'すべて選択', 'Select all')}</button>}
          <div className="phone-gallery-title-row">
            <div className="phone-gallery-title-copy">
              <h1>{filter === 'all' ? label('图库', 'ライブラリ', 'Library') : albumTitle}</h1>
              <span>{visible.length.toLocaleString(i18n.language)} {label('个项目', '項目', 'items')}</span>
            </div>
            <div className="phone-gallery-actions">
              {selectMode ? <>
                {filter === 'all' ? <div className="phone-gallery-selection-tools phone-liquid-glass">
                  <button type="button" aria-label={label('切换网格', 'グリッドを切り替え', 'Change grid')} onClick={() => setColumns(columns === 5 ? 3 : 5)}><ListFilter size={23} /></button>
                  <button type="button" aria-label={label('更多', 'その他', 'More')} onClick={() => setMenuOpen(value => !value)}><MoreHorizontal size={23} /></button>
                </div> : <button type="button" className="phone-gallery-circle phone-liquid-glass" aria-label={label('更多', 'その他', 'More')} onClick={() => setMenuOpen(value => !value)}><MoreHorizontal size={23} /></button>}
                <button type="button" className="phone-gallery-circle phone-liquid-glass" onClick={closeSelection} aria-label={label('取消选择', '選択を終了', 'Cancel selection')}><X size={27} strokeWidth={1.8} /></button>
              </> : <>
              <button type="button" className="phone-gallery-circle phone-liquid-glass" onClick={onBack} aria-label={label('返回主屏', 'ホームに戻る', 'Back to home')}><X size={16} /></button>
              <button type="button" className="phone-gallery-circle phone-liquid-glass" onClick={() => setColumns(columns === 5 ? 3 : 5)} aria-label={label('切换网格', 'グリッドを切り替え', 'Change grid')} aria-pressed={columns === 3}><LayoutGrid size={17} /></button>
              <button type="button" className={`phone-gallery-select${selectMode ? ' is-active' : ''}`} onClick={() => { setSelectMode(!selectMode); setSelected([]); }}>
                {selectMode ? label('完成', '完了', 'Done') : label('选择', '選択', 'Select')}
              </button>
              <button type="button" className="phone-gallery-circle phone-liquid-glass" aria-label={label('导入照片', '写真を追加', 'Import photos')} onClick={() => setMenuOpen(value => !value)}>＋</button>
              </>}
            </div>
          </div>
          {selectMode && menuOpen && <div className="phone-gallery-menu phone-liquid-glass"><button type="button" onClick={() => { setSelected(allSelected ? [] : visible.map(item => item.id)); setMenuOpen(false); }}>{allSelected ? label('取消全选', '選択解除', 'Deselect all') : label('全选', 'すべて選択', 'Select all')}</button></div>}
          {!selectMode && menuOpen && <div className="phone-gallery-menu phone-liquid-glass"><button type="button" onClick={() => { setMenuOpen(false); void onImport(false); }}>{label('导入图片', '画像を追加', 'Import images')}</button><button type="button" onClick={() => { setMenuOpen(false); void onImport(true); }}>{label('导入整个文件夹', 'フォルダを追加', 'Import folder')}</button></div>}
          {!selectMode && filter !== 'all' && <button type="button" className="phone-gallery-filter-chip phone-liquid-glass" onClick={() => setFilter('all')}>
            {filter === 'chat' ? label('聊天图片', 'チャット画像', 'Chat photos') : label('壁纸', '壁紙', 'Wallpapers')}<X size={11} />
          </button>}
          {!selectMode && searchOpen && <label className="phone-gallery-searchbar phone-liquid-glass">
            <Search size={14} />
            <input ref={searchRef} value={search} onChange={event => setSearch(event.target.value)} placeholder={label('搜索照片', '写真を検索', 'Search photos')} aria-label={label('搜索照片', '写真を検索', 'Search photos')} />
            <button type="button" onClick={() => { setSearch(''); setSearchOpen(false); }} aria-label={label('关闭搜索', '検索を閉じる', 'Close search')}><X size={13} /></button>
          </label>}
        </div>
        {visible.length ? <div className="phone-gallery-grid" style={{ '--gallery-columns': filter === 'all' ? columns : 3 } as React.CSSProperties}>
          {visible.map(item => <button type="button" className={`phone-gallery-cell${selected.includes(item.id) ? ' is-selected' : ''}`} key={item.id} onClick={() => openItem(item)} aria-label={item.name} aria-pressed={selectMode ? selected.includes(item.id) : undefined}>
            <img src={item.src} alt="" loading="lazy" />
            {selectMode && <span className="phone-gallery-check">{selected.includes(item.id) && <Check size={12} />}</span>}
          </button>)}
        </div> : <div className="phone-gallery-empty">
          {loading ? <span className="phone-gallery-spinner" aria-label={label('加载中', '読み込み中', 'Loading')} /> : <>
            <span className="phone-gallery-empty-art" aria-hidden="true" />
            <p>{filter === 'chat' ? label('还没有聊天图片', 'チャット画像はまだありません', 'No chat photos yet') : label('暂无照片', '写真はありません', 'No photos')}</p>
          </>}
        </div>}
      </> : <div className="phone-gallery-collections">
        <div className="phone-gallery-top is-static">
          <div className="phone-gallery-title-row">
            <div className="phone-gallery-title-copy"><h1>{label('精选集', 'コレクション', 'Collections')}</h1></div>
            <div className="phone-gallery-actions">
              <button type="button" className="phone-gallery-circle phone-liquid-glass" onClick={onBack} aria-label={label('返回主屏', 'ホームに戻る', 'Back to home')}><X size={16} /></button>
              <button type="button" className="phone-gallery-circle phone-liquid-glass" onClick={() => setMenuOpen(value => !value)} aria-label={label('更多', 'その他', 'More')}><MoreHorizontal size={18} /></button>
            </div>
          </div>
          {menuOpen && <div className="phone-gallery-menu phone-liquid-glass">
            <button type="button" onClick={() => { setMenuOpen(false); void onImport(false); }}>{label('导入图片', '画像を追加', 'Import images')}</button>
            <button type="button" onClick={() => { setMenuOpen(false); void onImport(true); }}>{label('导入整个文件夹', 'フォルダを追加', 'Import folder')}</button>
            <button type="button" onClick={() => { onRefresh(); setMenuOpen(false); }}>{label('刷新图库', 'ライブラリを更新', 'Refresh library')}</button>
            {hidden.length > 0 && <button type="button" onClick={() => { setHidden([]); localStorage.removeItem('phone-gallery-hidden'); setMenuOpen(false); }}>{label('显示隐藏项目', '非表示項目を再表示', 'Show hidden items')}</button>}
          </div>}
        </div>
        <section className="phone-gallery-section">
          <header><h2>{label('回忆', '思い出', 'Memories')}</h2></header>
          <div className="phone-gallery-memories">
            {wallpaperItems.slice(0, 2).map(paper => <button type="button" className="phone-gallery-memory" key={paper.id} onClick={() => goToAlbum('wallpaper')}>
              <img src={paper.src} alt="" />
              <span className="phone-gallery-memory-scrim" aria-hidden="true" />
              <span className="phone-gallery-memory-copy"><strong>{paper.name}</strong><small>{label('壁纸', '壁紙', 'Wallpaper')}</small></span>
            </button>)}
          </div>
        </section>
        <section className="phone-gallery-section">
          <header><h2>{label('固定', '固定', 'Pinned')}</h2></header>
          <div className="phone-gallery-fixed">
            <button type="button" className="phone-gallery-fixed-card" onClick={() => { setTab('library'); setFilter('all'); }}>
              <span className="phone-gallery-fixed-art"><Heart size={18} /></span>
              <span>{label('全部照片', 'すべての写真', 'All photos')}</span>
              <small>{items.length}</small>
            </button>
            <button type="button" className="phone-gallery-fixed-card" onClick={() => goToAlbum('chat')}>
              <span className="phone-gallery-fixed-art">{chatItems[0] ? <img src={chatItems[0].src} alt="" /> : <Images size={18} />}</span>
              <span>{label('最近保存', '最近保存', 'Recently saved')}</span>
              <small>{chatItems.length}</small>
            </button>
          </div>
        </section>
        <section className="phone-gallery-section">
          <header><h2>{label('相簿', 'アルバム', 'Albums')}</h2></header>
          <div className="phone-gallery-albums">
            <button type="button" className="phone-gallery-album" onClick={() => goToAlbum('chat')} aria-label={label('聊天图片', 'チャット画像', 'Chat photos')}>
              <span className="phone-gallery-album-cover">
                {chatItems.slice(0, 4).map(item => <img src={item.src} alt="" key={item.id} />)}
                {!chatItems.length && <Images size={20} />}
              </span>
              <span className="phone-gallery-album-copy"><strong>{label('聊天图片', 'チャット画像', 'Chat photos')}</strong><small>{chatItems.length}</small></span>
            </button>
            <button type="button" className="phone-gallery-album" onClick={() => goToAlbum('wallpaper')} aria-label={label('壁纸', '壁紙', 'Wallpapers')}>
              <span className="phone-gallery-album-cover">{wallpaperItems.slice(0, 4).map(item => <img src={item.src} alt="" key={item.id} />)}</span>
              <span className="phone-gallery-album-copy"><strong>{label('壁纸', '壁紙', 'Wallpapers')}</strong><small>{wallpaperItems.length}</small></span>
            </button>
          </div>
        </section>
        <p className="phone-gallery-note">{label('打开相簿可按分类浏览照片', 'アルバムを開いて分類ごとに表示', 'Open an album to browse by category')}</p>
      </div>}
    </div>
    {selectMode && <div className="phone-gallery-selection-footer" role="toolbar" aria-label={label('照片选择操作', '写真の選択操作', 'Photo selection actions')}>
      <button type="button" className="phone-gallery-circle phone-liquid-glass" disabled={!selected.length} onClick={() => setSelectionAction('share')} aria-label={label('分享所选照片', '選択した写真を共有', 'Share selected photos')}><Share size={24} strokeWidth={1.7} /></button>
      <strong aria-live="polite">{selected.length ? `${label('已选择', '選択中', 'Selected')} ${selected.length} ${label('项', '項目', 'items')}` : label('选择项目', '項目を選択', 'Select items')}</strong>
      <button type="button" className="phone-gallery-circle phone-liquid-glass" disabled={!selected.length} onClick={() => setSelectionAction('remove')} aria-label={label('从图库移除', 'ライブラリから非表示', 'Remove from library')}><Trash2 size={24} strokeWidth={1.7} /></button>
    </div>}
    {selectionAction && <div className="phone-gallery-action-scrim" onClick={() => { if (!shareBusy) setSelectionAction(null); }}><div className="phone-gallery-action-sheet phone-liquid-glass" role="dialog" aria-modal="true" aria-label={selectionAction === 'share' ? label('分享照片', '写真を共有', 'Share photos') : label('移除照片', '写真を非表示', 'Remove photos')} onClick={event => event.stopPropagation()}>
      {selectionAction === 'share' ? <><p>LINE · {selected.length} {label('张图片', '枚の写真', 'photos')}</p>{recipients.map(recipient => <button type="button" disabled={shareBusy} key={recipient.id} onClick={() => void shareTo(recipient.id)}>{label('发送给', '送信先', 'Send to')} {recipient.name}</button>)}{!recipients.length && <p>{label('暂无可用角色', '利用可能なキャラクターがいません', 'No available characters')}</p>}{shareBusy && <p role="status">{label('正在发送…', '送信中…', 'Sending…')}</p>}{shareError && <p role="alert">{shareError}</p>}</> : <><p>{label('从图库隐藏所选项目，保留聊天中的原始图片。', '選択した項目を非表示にします。チャットの原本は保持されます。', 'Hide selected items from the library; keep original chat images.')}</p><button type="button" className="is-destructive" onClick={() => { const next = [...new Set([...hidden, ...selected])]; localStorage.setItem('phone-gallery-hidden', JSON.stringify(next)); setHidden(next); closeSelection(); }}>{label('从图库移除', 'ライブラリから非表示', 'Remove from library')}</button></>}
      <button type="button" disabled={shareBusy} onClick={() => setSelectionAction(null)}>{label('取消', 'キャンセル', 'Cancel')}</button>
    </div></div>}
    {!selectMode && <><nav className="phone-gallery-tabbar phone-liquid-glass" aria-label={label('照片导航', '写真ナビ', 'Photos navigation')}>
      <button type="button" className={tab === 'library' ? 'is-active' : ''} onClick={() => setTab('library')} aria-current={tab === 'library' ? 'page' : undefined}>
        <Images size={21} /><span>{label('图库', 'ライブラリ', 'Library')}</span>
      </button>
      <button type="button" className={tab === 'collections' ? 'is-active' : ''} onClick={() => setTab('collections')} aria-current={tab === 'collections' ? 'page' : undefined}>
        <Album size={21} /><span>{label('精选集', 'コレクション', 'Collections')}</span>
      </button>
    </nav>
    <button type="button" className="phone-gallery-search-fab phone-liquid-glass" onClick={() => { setTab('library'); setSearchOpen(value => !value); }} aria-label={label('搜索照片', '写真を検索', 'Search photos')}>
      <Search size={20} />
    </button>
    </>}
    {current && <div className="phone-gallery-viewer" role="dialog" aria-modal="true" aria-label={current.name} onClick={() => setViewer(null)}
      onPointerDown={event => { swipe.current = (event.target as Element).closest('button') ? null : {x:event.clientX,y:event.clientY}; }}
      onPointerUp={event => { const start = swipe.current; swipe.current = null; if (!start) return; const dx = event.clientX-start.x, dy=event.clientY-start.y; if (Math.abs(dx)>50 && Math.abs(dx)>Math.abs(dy)*1.4) setViewer((viewer! + (dx<0 ? 1 : -1) + visible.length)%visible.length); }} onPointerCancel={() => { swipe.current=null; }}>
      <div className="phone-gallery-viewer-bar">
        <button type="button" onClick={event => { event.stopPropagation(); setViewer(null); }} aria-label={label('关闭', '閉じる', 'Close')}>×</button>
        <span>{viewer! + 1} / {visible.length}</span>
        <span className="phone-gallery-viewer-spacer" />
      </div>
      <img src={current.src} alt={current.name} draggable={false} onClick={event => event.stopPropagation()} />
      <div className="phone-gallery-viewer-caption" onClick={event => event.stopPropagation()}>
        <button type="button" className="phone-wallpaper-choose phone-liquid-glass" onClick={() => { setSelected([current.id]); setViewer(null); setSelectMode(true); setSelectionAction('share'); }}><Share size={17} /> LINE</button>
        <button type="button" className="phone-wallpaper-choose" onClick={() => onWallpaper(current.src)}>{label('设为壁纸', '壁紙に設定', 'Set as wallpaper')}</button>
        <span>{current.name}</span>
        <small>{current.modified ? new Date(current.modified * 1000).toLocaleString(i18n.language) : label('壁纸', '壁紙', 'Wallpaper')}</small>
      </div>
      {visible.length > 1 && <>
        <button type="button" className="phone-gallery-arrow is-prev" aria-label={label('上一张', '前へ', 'Previous')} onClick={event => { event.stopPropagation(); setViewer((viewer! - 1 + visible.length) % visible.length); }}>‹</button>
        <button type="button" className="phone-gallery-arrow is-next" aria-label={label('下一张', '次へ', 'Next')} onClick={event => { event.stopPropagation(); setViewer((viewer! + 1) % visible.length); }}>›</button>
      </>}
    </div>}
  </section>;
}


export default function PhoneHomeScreen({ awake, visible, unread, characterId, page, notifications, onUnlock, onNotification, onLine, onNotes, onScreen, onSettings, onCamera }: Props) {
  const { i18n } = useTranslation();
  const label = (zh: string, ja: string, en: string) => i18n.language.startsWith('zh') ? zh : i18n.language.startsWith('ja') ? ja : en;
  const [notificationCenter, setNotificationCenter] = useState(false);
  const surfaceRef = useRef<HTMLElement>(null);
  const clockCompression = useRef(0);
  const collapseNotifications = useCallback(() => {
    clockCompression.current = 0;
    surfaceRef.current?.style.setProperty('--clock-compression', '0');
    setNotificationCenter(false);
  }, []);
  const [flashlight, setFlashlight] = useState(false);
  const [flashlightBusy, setFlashlightBusy] = useState(false);
  /** 当前桌宠的安静原因（勿扰 / 专注 / 静默 / 游戏），用于锁屏胶囊 */
  const [quietReason, setQuietReason] = useState<'focus' | 'game_companion' | 'quiet_hours' | 'do_not_disturb' | null>(null);
  const [weather, setWeather] = useState<Weather | null>(null);
  const [weatherLoading, setWeatherLoading] = useState(false);
  const [city, setCity] = useState('');
  /** 主屏内打开的全屏 App 页面（时钟 / 天气 / 图库） */
  const [appPage, setAppPage] = useState<'clock' | 'weather' | 'photos' | null>(null);
  const [galleryImages, setGalleryImages] = useState<GalleryImageRecord[]>([]);
  const [galleryLoading, setGalleryLoading] = useState(false);
  const [wallpaper, setWallpaper] = useState(() => localStorage.getItem('phone-wallpaper') || '');
  const [layout, setLayout] = useState<string[]>(() => { try { return JSON.parse(localStorage.getItem('phone-home-layout') || '[]'); } catch { return []; } });
  const dragItem = useRef<string | null>(null);
  const pointerDrag = useRef<{id: number; x: number; y: number; moved: boolean; target: string | null} | null>(null);
  const suppressClick = useRef(false);
  const [photoIndex, setPhotoIndex] = useState(0);
  const importing = useRef(false);
  const widgetPhotos = useMemo(() => {
    let hidden: string[] = []; try { hidden = JSON.parse(localStorage.getItem('phone-gallery-hidden') || '[]'); } catch { /* default album */ }
    return [...galleryImages.map(item => ({id: item.path, src: resolveGallerySrc(item.path)})), ...GALLERY_WALLPAPERS].filter(item => !hidden.includes(item.id));
  }, [galleryImages]);
  const isLockScreen = !awake || page === 'lock';
  const [now, setNow] = useState(() => new Date());
  const currentHour = now.getHours();
  const [shortcuts, setShortcuts] = useState<Shortcut[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [editing, setEditing] = useState(false);
  const hold = useRef<ReturnType<typeof setTimeout> | null>(null);
  const holdPoint = useRef({ x: 0, y: 0 });
  const cancelHold = () => { if (hold.current !== null) clearTimeout(hold.current); hold.current = null; };
  useEffect(() => () => { if (hold.current !== null) clearTimeout(hold.current); }, []);
  useEffect(() => { if (!awake || page !== 'home') { cancelHold(); setEditing(false); } }, [awake, page]);
  const [query, setQuery] = useState('');
  const [form, setForm] = useState<Shortcut | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [deletePending, setDeletePending] = useState(false);
  const busyRef = useRef(false);
  const mounted = useRef(false);
  const formName = useRef<HTMLInputElement>(null);
  const refresh = useCallback(async (refreshWeather = false) => {
    if (refreshWeather) setWeatherLoading(true);
    try {
      const next = await invoke<{ shortcuts: Shortcut[]; weather?: Weather | null; city?: string | null }>('companion_desktop_context', { refreshWeather });
      if (mounted.current) { setShortcuts(next.shortcuts); setWeather(next.weather ?? null); setCity(next.city ?? ''); setLoaded(true); setError(''); }
    } finally { if (refreshWeather && mounted.current) setWeatherLoading(false); }
  }, []);
  const loadGallery = useCallback(async () => {
    setGalleryLoading(true);
    try {
      setGalleryImages(await invoke<GalleryImageRecord[]>('list_gallery_images'));
    } catch {
      setGalleryImages([]);
    } finally {
      setGalleryLoading(false);
    }
  }, []);
  useEffect(() => { void loadGallery(); }, [loadGallery]);
  useEffect(() => {
    if (!awake || !visible || isLockScreen || appPage || widgetPhotos.length < 2) return;
    const timer = setInterval(() => setPhotoIndex(index => (index + 1) % widgetPhotos.length), 8000);
    return () => clearInterval(timer);
  }, [awake, visible, isLockScreen, appPage, widgetPhotos.length]);
  useEffect(() => {
    const device = surfaceRef.current?.closest<HTMLElement>('.chat-device');
    if (wallpaper) device?.style.setProperty('--phone-wallpaper', `url(${JSON.stringify(wallpaper)})`);
    else device?.style.removeProperty('--phone-wallpaper');
  }, [wallpaper]);
  const chooseWallpaper = (src: string) => { setWallpaper(src); if (src) localStorage.setItem('phone-wallpaper', src); else localStorage.removeItem('phone-wallpaper'); };
  const importPhotos = async (folder: boolean, asWallpaper = false) => {
    if (importing.current) return;
    importing.current = true;
    try {
      const result = await openFileDialog({ directory: folder, multiple: !asWallpaper, filters: folder ? undefined : [{ name: 'Images', extensions: ['png','jpg','jpeg','webp','gif','bmp'] }] });
      if (!result) return;
      const paths = Array.isArray(result) ? result : [result];
      const imported = await invoke<string[]>('import_gallery_images', { paths });
      await loadGallery();
      if (asWallpaper && imported[0]) chooseWallpaper(resolveGallerySrc(imported[0]));
      if (!imported.length) void emit('toast:show', { message: label('没有找到支持的图片', '対応する画像がありません', 'No supported images found'), type: 'info' });
    } catch (error) { void loadGallery(); void emit('toast:show', { message: String(error), type: 'error' }); }
    finally { importing.current = false; }
  };
  const allLayoutIds = ['weather','photo','clock','photos','notes','screen', ...shortcuts.map(item => item.id)];
  const layoutIds = [...layout.filter(id => allLayoutIds.includes(id)), ...allLayoutIds.filter(id => !layout.includes(id))];
  const orderOf = (id: string) => layoutIds.indexOf(id);
  const reorder = (target: string) => {
    const source = dragItem.current;
    if (!source || source === target) return;
    const next = layoutIds.filter(id => id !== source);
    next.splice(next.indexOf(target), 0, source);
    setLayout(next); localStorage.setItem('phone-home-layout', JSON.stringify(next));
  };
  useEffect(() => {
    if (appPage !== 'photos') return;
    void loadGallery();
    let active = true;
    const cleanups: UnlistenFn[] = [];
    void (async () => {
      for (const name of ['chat:user_image', 'chat:assistant_image']) {
        const off = await listen(name, () => { void loadGallery(); });
        if (active) cleanups.push(off); else off();
      }
    })();
    return () => { active = false; cleanups.forEach(off => off()); };
  }, [appPage, loadGallery]);
  useEffect(() => {
    let timer = 0;
    /* 读取系统时间，只把刷新点对齐到分钟边界，避免分钟数最多延迟 15 秒翻转。 */
    const tick = () => {
      setNow(new Date());
      timer = window.setTimeout(tick, 60_000 - (Date.now() % 60_000) + 50);
    };
    const resync = () => {
      if (document.hidden) return;
      window.clearTimeout(timer);
      tick();
    };
    tick();
    document.addEventListener('visibilitychange', resync);
    window.addEventListener('focus', resync);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener('visibilitychange', resync);
      window.removeEventListener('focus', resync);
    };
  }, []);

  const refreshQuiet = useCallback(async () => {
    try {
      const status = await invoke<{ quiet_reason?: string | null }>('companion_status');
      const reason = status?.quiet_reason;
      setQuietReason(reason === 'focus' || reason === 'game_companion' || reason === 'quiet_hours' || reason === 'do_not_disturb' ? reason : null);
    } catch {
      setQuietReason(null);
    }
  }, []);
  useEffect(() => {
    let active = true;
    const cleanups: UnlistenFn[] = [];
    void (async () => {
      for (const name of ['config:saved', 'companion:quiet-changed']) {
        const off = await listen(name, () => { void refreshQuiet(); });
        if (active) cleanups.push(off); else off();
      }
      if (active) await refreshQuiet();
    })();
    return () => { active = false; cleanups.forEach(off => off()); };
  }, [refreshQuiet]);
  /* 专注模式/静默时段按小时生效：小时变化时重新取一次状态。 */
  useEffect(() => {
    if (awake) void refreshQuiet();
  }, [awake, currentHour, refreshQuiet]);
  /* 离开锁屏或熄屏时自动关掉相机闪光灯，避免硬件灯一直亮着。 */
  useEffect(() => {
    if (awake && isLockScreen) return;
    if (!flashlight) return;
    setFlashlight(false);
    void invoke('set_system_flashlight', { enabled: false }).catch(() => {});
  }, [awake, isLockScreen, flashlight]);
  /* 手机窗口销毁时同样兜底关灯。 */
  useEffect(() => () => { void invoke('set_system_flashlight', { enabled: false }).catch(() => {}); }, []);
  const toggleFlashlight = useCallback(async () => {
    if (flashlightBusy) return;
    setFlashlightBusy(true);
    const next = !flashlight;
    try {
      await invoke('set_system_flashlight', { enabled: next });
      setFlashlight(next);
    } catch (error) {
      void emit('toast:show', { message: String(error), type: 'error', duration: 4500, key: Date.now() });
    } finally {
      setFlashlightBusy(false);
    }
  }, [flashlight, flashlightBusy]);
  useEffect(() => {
    mounted.current = true;
    let active = true;
    let inFlight = false;
    const cleanups: UnlistenFn[] = [];
    const update = async () => {
      if (inFlight || !active) return;
      inFlight = true;
      try { await refresh(); } catch (e) { if (active) setError(String(e)); }
      finally { inFlight = false; }
    };
    void (async () => {
      for (const name of ['config:saved', 'desktop:shown']) {
        const off = await listen(name, () => void update());
        if (active) cleanups.push(off); else off();
      }
      await update();
    })().catch(e => { if (active) setError(String(e)); });
    return () => { mounted.current = false; active = false; cleanups.forEach(off => off()); };
  }, [refresh]);
  useEffect(() => {
    if (visible && awake) void refresh().catch(e => { if (mounted.current) setError(String(e)); });
  }, [visible, awake, refresh]);
  useEffect(() => { if (form && awake && visible && !isLockScreen) formName.current?.focus(); }, [form?.id, awake, visible, isLockScreen]);

  useEffect(() => { if (!awake || page !== 'lock') collapseNotifications(); }, [awake, page, collapseNotifications]);
  /* 锁屏/熄屏时退出内置 App 页面；Esc 也能返回主屏。 */
  useEffect(() => { if (!awake || isLockScreen) setAppPage(null); }, [awake, isLockScreen]);
  useEffect(() => {
    if (!appPage) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      if (surfaceRef.current?.querySelector('.phone-gallery-viewer,.phone-gallery-action-sheet')) return;
      event.stopPropagation();
      setAppPage(null);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [appPage]);

  const run = async (action: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true); setError('');
    try { await action(); } catch (e) { if (mounted.current) setError(String(e)); }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  };
  const openForm = (item?: Shortcut) => {
    setDeletePending(false);
    setForm(item ? { ...item, group: item.group ?? '' } : { id: crypto.randomUUID(), name: '', kind: 'website', target: '', group: '' });
  };
  const persist = async (next: Shortcut[]) => {
    await invoke('companion_save_shortcuts', { shortcuts: next });
    if (mounted.current) { setShortcuts(next); setForm(null); }
  };
  const app = (id: string, name: string, icon: ReactNode, action: () => void, badge = 0) => <button type="button" className={`phone-app phone-app-${id}`} key={id} data-home-item={id === 'line' || id === 'settings' ? undefined : id} draggable={editing && id !== 'line' && id !== 'settings'} style={{order: orderOf(id)}} onClick={action} aria-label={name}>
    <span className="phone-app-art">{icon}{badge > 0 && <b className="phone-app-badge">{badge > 99 ? '99+' : badge}</b>}</span><span className="phone-app-label">{name}</span>
  </button>;
  const sections = groupShortcuts(shortcuts, query);
  const time = `${String(now.getHours()).padStart(2, '0')}:${String(now.getMinutes()).padStart(2, '0')}`;
  const date = now.toLocaleDateString(i18n.language, { month: 'long', day: 'numeric', weekday: 'long' }).replace('星期', '周');
  const lunar = i18n.language.startsWith('zh') ? (() => {
    const parts = new Intl.DateTimeFormat('zh-CN-u-ca-chinese', { year: 'numeric', month: 'long', day: 'numeric' }).formatToParts(now);
    const part = (type: string) => parts.find(p => p.type === type)?.value ?? '';
    const day = Number(part('day'));
    const digits = ['一','二','三','四','五','六','七','八','九','十'];
    const dayName = day <= 10 ? '初' + digits[day - 1] : day < 20 ? '十' + digits[day - 11] : day === 20 ? '二十' : day < 30 ? '廿' + digits[day - 21] : '三十';
    return `${part('yearName')}年${part('month')}${dayName}`;
  })() : '';
  const latest = notifications[0];
  const focusLabel = quietReason === 'focus' ? label('专注', '集中', 'Focus')
    : quietReason === 'do_not_disturb' ? label('勿扰', '集中', 'Quiet')
    : quietReason === 'quiet_hours' ? label('静默', '静默', 'Silent')
    : label('游戏', 'ゲーム', 'Game');
  const notificationTime = (timestamp: number) => {
    const minutes = Math.max(0, Math.floor((now.getTime() - timestamp) / 60000));
    return minutes < 1 ? label('现在', '今', 'now') : label(`${minutes} 分钟前`, `${minutes} 分前`, `${minutes} min ago`);
  };
  const notificationCard = (item: PhoneNotification, compact = false) => <button type="button" className={`phone-lock-notification phone-liquid-glass${compact ? ' is-compact' : ''}`} key={item.id}
    aria-label={label('打开消息：', 'メッセージを開く：', 'Open notification: ') + item.title}
    onClick={event => { event.stopPropagation(); onNotification(item); }}>
    <span className="phone-lock-app-wrap">
      <NotificationAvatar notification={item} />
      {item.count > 1 && <b className="phone-lock-app-badge">{item.count > 99 ? '99+' : item.count}</b>}
    </span>
    <span className="phone-notification-copy"><span className="phone-lock-notification-heading"><strong>LINE</strong><span>{notificationTime(item.timestamp)}</span></span><b>{item.title}</b><span className="phone-notification-text">{item.content || label('你有新消息', '新しいメッセージ', 'You have new messages')}</span></span>
  </button>;
  return <section ref={surfaceRef} className={`phone-home${awake ? ' is-awake' : ' is-asleep'}${isLockScreen ? ' is-lock-screen' : ' is-home-screen'}${notificationCenter ? ' is-notification-center' : ''}`}
    onClickCapture={event => {
      if (suppressClick.current) { suppressClick.current = false; event.preventDefault(); event.stopPropagation(); return; }
      if (editing && (event.target as HTMLElement).closest('[data-home-item],.phone-home-dock')) {
        if (!(event.target as HTMLElement).closest('.phone-app-shortcut')) { event.preventDefault(); event.stopPropagation(); }
      }
    }}
    onPointerDownCapture={event => {
      suppressClick.current = false;
      if (editing && event.button === 0) {
        const id = (event.target as HTMLElement).closest<HTMLElement>('.phone-home-layout [data-home-item]')?.dataset.homeItem;
        if (id) { dragItem.current = id; pointerDrag.current = { id: event.pointerId, x: event.clientX, y: event.clientY, moved: false, target: null }; }
      }
      if (isLockScreen || editing || appPage || event.button !== 0 || (event.target as HTMLElement).closest('[role="dialog"]')) return;
      cancelHold(); holdPoint.current = { x: event.clientX, y: event.clientY };
      const heldItem = (event.target as HTMLElement).closest<HTMLElement>('[data-home-item]')?.dataset.homeItem;
      const pointerId = event.pointerId;
      hold.current = setTimeout(() => {
        hold.current = null; suppressClick.current = true; setEditing(true);
        if (heldItem) { dragItem.current = heldItem; pointerDrag.current = { id: pointerId, ...holdPoint.current, moved: false, target: null }; }
      }, 600);
    }}
    onPointerMoveCapture={event => {
      if (Math.hypot(event.clientX - holdPoint.current.x, event.clientY - holdPoint.current.y) > 8) cancelHold();
      const drag = pointerDrag.current;
      if (!drag || drag.id !== event.pointerId) return;
      if (Math.hypot(event.clientX - drag.x, event.clientY - drag.y) > 8) {
        drag.moved = true; event.preventDefault(); event.currentTarget.setPointerCapture(event.pointerId);
        drag.target = document.elementFromPoint(event.clientX, event.clientY)?.closest<HTMLElement>('[data-home-item]')?.dataset.homeItem ?? null;
      }
    }}
    onPointerUpCapture={event => {
      const drag = pointerDrag.current;
      if (drag?.moved) { if (drag.target) reorder(drag.target); suppressClick.current = true; if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId); }
      pointerDrag.current = null; dragItem.current = null;
    }}
    onPointerUp={cancelHold} onPointerCancel={() => { cancelHold(); pointerDrag.current = null; dragItem.current = null; }} onPointerLeave={cancelHold}
    hidden={!visible} aria-label={isLockScreen ? label('手机锁屏', 'ロック画面', 'Phone lock screen') : label('手机主屏', 'ホーム画面', 'Phone home')}
    onClick={() => { if (isLockScreen && awake) onUnlock(); }}
    onKeyDown={event => { if (isLockScreen && awake && event.target === event.currentTarget && (event.key === 'Enter' || event.key === ' ')) { event.preventDefault(); onUnlock(); } }}
    onWheel={event => {
      cancelHold();
      if (!isLockScreen || !awake) return;
      event.stopPropagation();
      const list = surfaceRef.current?.querySelector('.phone-notification-scroll');
      // Scroll the message list normally; expand the clock only after reaching its top.
      if (event.deltaY < 0 && list && list.scrollTop > 0) return;
      const delta = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 240 : 1);
      if (!delta) return;
      const next = Math.max(0, Math.min(1, clockCompression.current + delta / 240));
      clockCompression.current = next;
      surfaceRef.current?.style.setProperty('--clock-compression', String(next));
      setNotificationCenter(next > 0);
    }} tabIndex={isLockScreen && awake ? 0 : -1}>
    {isLockScreen ? <>
      <div className="phone-notification-backdrop" aria-hidden="true" />
      <div className="phone-home-clock">
        <div className="phone-home-date">{date}{lunar && ` · ${lunar}`}</div>
        <PhoneLockClock time={time} />
      </div>
      {awake && <>
        <div className="phone-lock-content">
          {notificationCenter ? <div className="phone-notification-scroll">
            {latest && <div className="phone-recent-notification">{notificationCard(latest)}</div>}
            <div className="phone-notification-center-heading"><h2>{label('通知中心', '通知センター', 'Notification Center')}</h2><button type="button" aria-label={label('收起通知中心', '通知を閉じる', 'Collapse notifications')} onClick={event => { event.stopPropagation(); collapseNotifications(); }}><X size={20} /></button></div>
            <div className="phone-notification-list">{notifications.slice(1).map(item => notificationCard(item))}{!notifications.length && <p className="phone-no-notifications">{label('暂无新通知', '新しい通知はありません', 'No new notifications')}</p>}</div>
          </div> : latest && <div className="phone-notification-stack" data-stacked={notifications.length > 1 ? '' : undefined}>
            {notificationCard(latest)}
            {notifications.length > 1 && notificationCard(notifications[1], true)}
            {notifications.length > 2 && <span className="phone-notification-more">{`+${notifications.length - 2}`}</span>}
          </div>}
        </div>
        <div className="phone-lock-actions">
          <button
            type="button"
            className={`phone-lock-action${flashlight ? ' is-on' : ''}`}
            aria-label={label(flashlight ? '关闭闪光灯' : '打开闪光灯', flashlight ? 'フラッシュを消す' : 'フラッシュを点ける', flashlight ? 'Turn off flashlight' : 'Turn on flashlight')}
            aria-pressed={flashlight}
            disabled={flashlightBusy}
            onClick={event => { event.stopPropagation(); void toggleFlashlight(); }}
          ><LockScreenFlashlight /></button>
          {quietReason && (
            <span className={`phone-focus-pill is-${quietReason}`} aria-label={label('当前状态：', '現在の状態：', 'Current status: ') + focusLabel}>
              <span>{focusLabel}</span>
            </span>
          )}
          <button
            type="button"
            className="phone-lock-action"
            aria-label={label('打开相机', 'カメラを開く', 'Open camera')}
            onClick={event => { event.stopPropagation(); onCamera(); }}
          ><LockScreenCamera /></button>
        </div>
      </>}
    </> : <div className="phone-home-unlocked">
      <div className={`phone-home-layout${editing ? ' is-editing' : ''}`}
        onDragStart={event => event.preventDefault()}
        onDragOver={event => { if (editing && dragItem.current) event.preventDefault(); }}
        onDrop={event => { event.preventDefault(); const id = (event.target as HTMLElement).closest<HTMLElement>('[data-home-item]')?.dataset.homeItem; if (id) reorder(id); dragItem.current = null; }}
        onDragEnd={() => { dragItem.current = null; }}>
      <div className="phone-home-widgets">
        <div className="phone-widget-cell" data-home-item="weather" draggable={editing} style={{order: orderOf('weather')}}><button type="button" className="phone-weather-widget" onClick={() => { setAppPage('weather'); void refresh(true).catch(e => { if (mounted.current) setError(String(e)); }); }} aria-label={label('天气', '天気', 'Weather')}>
          <span className="phone-weather-city">{city || label('本地天气', '現在地の天気', 'Local weather')} <span>↗</span></span>
          <strong>{weather ? Math.round(weather.temperature) : '—'}°</strong><CloudSun size={23} fill="white" /><span>{weather?.description || label('暂无天气数据', '天気情報なし', 'Weather unavailable')}</span>
          <small>{weather?.sources?.[0]?.temp_max != null ? label('最高', '最高', 'H:') + Math.round(weather.sources[0].temp_max) + '° ' : ''}{weather?.sources?.[0]?.temp_min != null ? label('最低', '最低', 'L:') + Math.round(weather.sources[0].temp_min) + '°' : ''}</small>
        </button><span>{label('天气', '天気', 'Weather')}</span></div>
        <div className="phone-widget-cell" data-home-item="photo" draggable={editing} style={{order: orderOf('photo')}}><button type="button" className="phone-photo-widget" onClick={() => setAppPage('photos')} aria-label={label('照片', '写真', 'Photos')}>{widgetPhotos.length > 0 && <img key={widgetPhotos[photoIndex % widgetPhotos.length].id} src={widgetPhotos[photoIndex % widgetPhotos.length].src} alt="" draggable={false} />}</button><span>{label('照片', '写真', 'Photos')}</span></div>
      </div>
      <div className="phone-home-apps">
        <div className="phone-app-grid phone-system-apps">
          {!query.trim() && <>{app('clock', label('时钟', '時計', 'Clock'), <ClockAppArtwork />, () => setAppPage('clock'))}
          {app('photos', label('照片', '写真', 'Photos'), <PhotosAppArtwork />, () => setAppPage('photos'))}
          {app('notes', label('随手记', 'メモ', 'Notes'), <NotebookPen size={30} />, onNotes)}
        {app('screen', label('截屏分析', '画面を分析', 'Screen'), <ScanLine size={30} />, onScreen)}
          </>}
          {sections.flatMap(section => section.items).map((item, index) => <button type="button" className={`phone-app phone-app-shortcut${editing ? ' is-editing' : ''}`} key={item.id}
            data-home-item={item.id} draggable={editing} style={{ order: orderOf(item.id), '--app-hue': (index * 53 + 210) % 360 } as React.CSSProperties}
            aria-label={(editing ? label('编辑 ', '編集 ', 'Edit ') : '') + item.name} title={item.target} disabled={busy}
            onContextMenu={e => { e.preventDefault(); if (editing) openForm(item); }}
            onClick={() => editing ? openForm(item) : void run(async () => {
              await invoke('companion_launch_shortcut', { shortcutId: item.id, characterId });
              try { await emit('companion:interaction', { character_id: characterId, content: label('已打开 ' + item.name + '。', item.name + 'を開きました。', 'Opened ' + item.name + '.'), motion: 'remind' }); }
              finally { await invoke('close_chat_window'); }
            })}>
            <span className="phone-app-art"><AppIcon shortcut={item} />{editing && <span className="phone-app-edit-mark">…</span>}</span><span className="phone-app-label">{item.name}</span>
          </button>)}
        </div>
        {query.trim() && !sections.length && <p className="phone-home-empty">{label('没有找到 App', 'アプリが見つかりません', 'No apps found')}</p>}
      </div>
      </div>
      {error && <div className="phone-home-error" role="alert">{error}{!loaded && <button onClick={() => void run(refresh)}>{label('重试', '再試行', 'Retry')}</button>}</div>}
      {editing && <div className="phone-edit-toolbar" role="toolbar" aria-label={label('编辑主屏', 'ホームを編集', 'Edit home screen')}><button onClick={() => openForm()}>{label('添加快捷方式', 'ショートカットを追加', 'Add shortcut')}</button><button onClick={() => void importPhotos(false, true)}>{label('自定义壁纸', '壁紙を変更', 'Wallpaper')}</button><button onClick={() => chooseWallpaper('')}>{label('还原壁纸', '壁紙をリセット', 'Reset wallpaper')}</button><button onClick={() => setEditing(false)}>{label('完成', '完了', 'Done')}</button></div>}
      <div className="phone-home-toolbar">
        <label className="phone-app-search phone-liquid-glass"><Search size={13} /><span className="phone-search-field"><span className="phone-search-measure" aria-hidden="true">{query || label('搜索', '検索', 'Search')}</span><input size={1} value={query} onChange={e => setQuery(e.target.value)} maxLength={120} placeholder={label('搜索', '検索', 'Search')} aria-label={label('搜索 App', 'アプリを検索', 'Search apps')} /></span></label>
      </div>
      <div className="phone-home-dock phone-liquid-glass">
        {app('line', 'LINE', <LineIcon />, onLine, unread)}
        {app('settings', label('设置', '設定', 'Settings'), <Settings size={32} />, onSettings)}
      </div>
    </div>}
    {appPage === 'clock' && !isLockScreen && <ClockPage onBack={() => setAppPage(null)} />}
    {appPage === 'weather' && !isLockScreen && <WeatherPage weather={weather} city={city} now={now} loading={weatherLoading} onBack={() => setAppPage(null)} onSettings={onSettings} />}
    {appPage === 'photos' && !isLockScreen && <GalleryPage images={galleryImages} loading={galleryLoading} onBack={() => setAppPage(null)} onRefresh={() => void loadGallery()} onImport={importPhotos} onWallpaper={chooseWallpaper} onShared={id => { setAppPage(null); onNotification({ id: 'gallery-share', title: '', content: '', timestamp: Date.now(), count: 0, target: { kind: 'private', characterId: id } }); }} />}
    {form && awake && !isLockScreen && <div className="phone-app-modal-backdrop" onClick={e => { if (e.target === e.currentTarget && !busy) setForm(null); }}>
      <form className="phone-app-modal" role="dialog" aria-modal="true" aria-labelledby="phone-app-form-title" onKeyDown={e => {
        if (e.key === 'Escape' && !busy) { e.stopPropagation(); setForm(null); }
        if (e.key === 'Tab') {
          const fields = [...e.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)')];
          const first = fields[0], last = fields[fields.length - 1];
          if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus(); }
          else if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus(); }
        }
      }} onSubmit={e => {
        e.preventDefault();
        if (!loaded || busy || !form.name.trim() || !form.target.trim()) return;
        const target = form.target.trim();
        const item = { ...form, name: form.name.trim(), group: form.group.trim(), target: form.kind === 'website' && !/^[a-z][a-z0-9+.-]*:\/\//i.test(target) ? `https://${target}` : target };
        void run(() => persist(shortcuts.some(s => s.id === item.id) ? shortcuts.map(s => s.id === item.id ? item : s) : [...shortcuts, item]));
      }}>
        <header><h2 id="phone-app-form-title">{shortcuts.some(s => s.id === form.id) ? label('编辑 App', 'アプリを編集', 'Edit app') : label('添加 App', 'アプリを追加', 'Add app')}</h2><button type="button" disabled={busy} aria-label={label('关闭', '閉じる', 'Close')} onClick={() => setForm(null)}><X size={18} /></button></header>
        <div className="phone-app-kind">{(['website', 'application'] as const).map(kind => <button type="button" key={kind} disabled={busy} aria-pressed={form.kind === kind} onClick={() => setForm({ ...form, kind })}>{kind === 'website' ? label('网站', 'サイト', 'Website') : label('应用', 'アプリ', 'Application')}</button>)}</div>
        <label>{label('名称', '名前', 'Name')}<input ref={formName} required maxLength={80} value={form.name} onChange={e => setForm({ ...form, name: e.target.value })} /></label>
        <label>{form.kind === 'website' ? label('网址', 'URL', 'Website URL') : label('应用路径', 'アプリのパス', 'Application path')}<input required value={form.target} placeholder={form.kind === 'website' ? 'https://…' : 'C:\\…\\app.exe'} onChange={e => setForm({ ...form, target: e.target.value })} /></label>
        <label>{label('分组（可选）', 'グループ（任意）', 'Group (optional)')}<input list="phone-app-groups" maxLength={40} value={form.group} onChange={e => setForm({ ...form, group: e.target.value })} /></label>
        <datalist id="phone-app-groups">{shortcutGroupNames(shortcuts).map(group => <option key={group} value={group} />)}</datalist>
        {error && <p className="phone-form-error" role="alert">{error}</p>}
        <footer>{shortcuts.some(s => s.id === form.id) && <button type="button" className="phone-app-delete" disabled={busy} onClick={() => deletePending ? void run(() => persist(shortcuts.filter(s => s.id !== form.id))) : setDeletePending(true)}><Trash2 size={14} />{deletePending ? label('确认删除', '削除する', 'Confirm delete') : label('删除', '削除', 'Delete')}</button>}<button className="phone-app-save" type="submit" disabled={busy || !loaded || !form.name.trim() || !form.target.trim()}>{busy ? label('保存中…', '保存中…', 'Saving…') : label('保存', '保存', 'Save')}</button></footer>
      </form>
    </div>}
  </section>;
}
