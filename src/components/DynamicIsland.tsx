import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import './DynamicIsland.css';

/** 与 Rust `world::music::MusicSnapshot` 的序列化结果对齐。 */
interface Music {
  title: string;
  artist: string;
  album: string;
  status: 'playing' | 'paused' | 'stopped' | 'changing' | 'closed';
  source_app: string;
  artwork_data_url?: string | null;
}

/** AUMID → 用户认得的名字；匹配不到就回落到去掉 `.exe` 的主干。 */
const SOURCE_NAMES: Array<[RegExp, string]> = [
  [/spotify/i, 'Spotify'],
  [/cloudmusic|netease/i, '网易云音乐'],
  [/qqmusic/i, 'QQ 音乐'],
  [/zunemusic|mediaplayer|music\.ui/i, '媒体播放器'],
  [/msedge|edge/i, 'Edge'],
  [/chrome/i, 'Chrome'],
  [/firefox/i, 'Firefox'],
  [/potplayer/i, 'PotPlayer'],
  [/vlc/i, 'VLC'],
];

function sourceName(sourceApp: string): string {
  const raw = sourceApp.trim();
  if (!raw) return '';
  for (const [pattern, name] of SOURCE_NAMES) if (pattern.test(raw)) return name;
  const entry = raw.split('!').pop() ?? raw;
  const stem = entry.split(/[\\/]/).pop() ?? entry;
  return stem.replace(/\.exe$/i, '');
}

/**
 * ChatWindow 顶部的灵动岛。
 *
 * 折叠态与原来的静态黑胶囊完全一致（126×36 / 圆角 20 / 恒定黑色），
 * 媒体默认显示紧凑左右区域；悬停或键盘聚焦时展开详情，点击打开来源。
 * 无播放时保持折叠并让出指针事件，不影响状态栏的拖拽区域。
 */
export default function DynamicIsland() {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh');
  const ja = i18n.language.startsWith('ja');
  const [music, setMusic] = useState<Music | null>(null);
  const [busy, setBusy] = useState(false);
  const [failedArtwork, setFailedArtwork] = useState<string | null>(null);
  const busyRef = useRef(false);

  useEffect(() => {
    let active = true;
    let inFlight = false;
    const read = async () => {
      if (!active || inFlight) return;
      inFlight = true;
      try {
        const next = await invoke<{ music: Music | null }>('companion_now_playing');
        if (active) setMusic(next?.music ?? null);
      } catch {
        // 装饰件：读不到就保持上一帧，不打扰用户
      } finally {
        inFlight = false;
      }
    };
    void read();
    const timer = window.setInterval(read, 2000);
    const onWake = () => { if (!document.hidden) void read(); };
    document.addEventListener('visibilitychange', onWake);
    window.addEventListener('focus', onWake);
    return () => {
      active = false;
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', onWake);
      window.removeEventListener('focus', onWake);
    };
  }, []);

  const open = useCallback(async () => {
    if (!music || busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    try {
      const message = await invoke<string>('companion_focus_media_source', { sourceApp: music.source_app });
      void emit('toast:show', { message, type: 'success', duration: 2000, key: Date.now() });
    } catch (error) {
      void emit('toast:show', { message: String(error), type: 'error', duration: 4000, key: Date.now() });
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }, [music]);

  const title = music?.title?.trim() ?? '';
  const live = !!title;
  const playing = music?.status === 'playing';
  const artist = music?.artist?.trim() ?? '';
  const artwork = music?.artwork_data_url;
  const artworkIcon = artwork && artwork !== failedArtwork
    ? <img src={artwork} alt="" onError={() => setFailedArtwork(artwork)} />
    : <svg width="18" height="18" viewBox="0 0 24 24" fill="currentColor"><path d="M9 4v12.2a3.5 3.5 0 1 0 2 3.1V8l8-2v8.2a3.5 3.5 0 1 0 2 3.1V2L9 4Z" /></svg>;
  const source = live ? sourceName(music!.source_app) : '';
  const target = source || (zh ? '播放器' : ja ? 'アプリ' : 'the player');
  const label = live
    ? zh ? `正在播放：${title}${artist ? ` · ${artist}` : ''}，点击打开 ${target}`
      : ja ? `再生中：${title}、クリックで ${target} を開く`
      : `Now playing: ${title}${artist ? ` · ${artist}` : ''}. Click to open ${target}`
    : undefined;

  return (
    <div
      className={'dynamic-island' + (live ? ' is-live' : '') + (playing ? ' is-playing' : '') + (busy ? ' is-busy' : '')}
      role={live ? 'button' : undefined}
      tabIndex={live ? 0 : undefined}
      aria-label={label}
      aria-hidden={live ? undefined : true}
      onClick={live ? () => void open() : undefined}
      onKeyDown={live ? (event) => {
        if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); void open(); }
      } : undefined}
    >
      <div className="dynamic-island-compact" aria-hidden>
        <span className="dynamic-island-art">
          {artworkIcon}
        </span>
        <span className="dynamic-island-eq"><i /><i /><i /><i /></span>
      </div>
      <div className="dynamic-island-content">
        <span className="dynamic-island-art" aria-hidden>
          {artworkIcon}
        </span>
        <span className="dynamic-island-meta">
          <span className="dynamic-island-title">{title}</span>
          <span className="dynamic-island-sub">{artist || source}</span>
        </span>
        <span className="dynamic-island-eq" aria-hidden><i /><i /><i /><i /></span>
      </div>
    </div>
  );
}
