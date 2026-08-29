/**
 * 视频动画演出层 — 全窗口视频动画播放
 *
 * - 监听 `video:animation` / `video:animation:stop` 事件（按角色过滤）
 * - 双缓冲交叉淡入播放（双 <video> 层叠，切换无空白帧）
 * - 素材缺失/加载失败时优雅跳过（onerror → 淡出），不影响既有 Live2D 渲染
 * - 素材路径：`public/video-animations/<file>`（dev 由 vite 提供，prod 由 asset 协议提供）
 * - 通过 `onActiveChange` 通知父组件隐藏/恢复 Live2D 本体
 */

import { useEffect, useRef, useState } from 'react';

interface VideoAnimationLayerProps {
  /** 当前窗口角色 id（事件按此过滤） */
  characterId?: string;
  /** 播放状态变化回调（用于父组件切换 Live2D 可见性） */
  onActiveChange?: (active: boolean) => void;
}

interface AnimPayload {
  character_id?: string;
  name?: string;
  file?: string;
}

export function VideoAnimationLayer({
  characterId,
  onActiveChange,
}: VideoAnimationLayerProps) {
  const [active, setActive] = useState(false);
  const videoARef = useRef<HTMLVideoElement | null>(null);
  const videoBRef = useRef<HTMLVideoElement | null>(null);
  /** 当前显示的是 A(0) 还是 B(1) */
  const frontRef = useRef(0);
  /** 切换代数：防止旧回调覆盖新播放 */
  const genRef = useRef(0);

  useEffect(() => {
    onActiveChange?.(active);
  }, [active, onActiveChange]);

  useEffect(() => {
    let cancelled = false;

    const finish = (gen: number) => {
      if (cancelled || genRef.current !== gen) return;
      const el = frontRef.current === 0 ? videoBRef.current : videoARef.current;
      if (el) {
        el.classList.remove('is-front');
        el.pause();
      }
      setActive(false);
    };

    const play = (payload: AnimPayload) => {
      if (characterId && payload.character_id && payload.character_id !== characterId) return;
      const file = payload.file;
      if (!file) return;
      const gen = ++genRef.current;
      const target = frontRef.current === 0 ? videoBRef.current : videoARef.current;
      if (!target) return;

      target.src = `/video-animations/${encodeURIComponent(file)}`;
      target.muted = true;
      target.loop = false;
      target.playsInline = true;
      target.autoplay = true;
      target.onended = () => finish(gen);
      target.onerror = () => {
        console.warn('[VideoAnimationLayer] 素材缺失或无法播放:', file);
        finish(gen);
      };
      target.load();

      const onReady = () => {
        target.removeEventListener('loadeddata', onReady);
        if (cancelled || genRef.current !== gen) return;
        const old = frontRef.current === 0 ? videoARef.current : videoBRef.current;
        target.classList.add('is-front');
        if (old && old !== target) old.classList.remove('is-front');
        frontRef.current = frontRef.current === 0 ? 1 : 0;
        setActive(true);
        void target.play().catch(() => finish(gen));
      };
      target.addEventListener('loadeddata', onReady);
      if (target.readyState >= 2) onReady();
    };

    const stop = (payload?: { character_id?: string }) => {
      if (characterId && payload?.character_id && payload.character_id !== characterId) return;
      finish(++genRef.current);
    };

    void import('@tauri-apps/api/event').then(({ listen }) => {
      if (cancelled) return;
      void listen<AnimPayload>('video:animation', (e) => play(e.payload));
      void listen<{ character_id?: string }>('video:animation:stop', (e) => stop(e.payload));
    });

    return () => {
      cancelled = true;
      genRef.current++;
      // 卸载时暂停视频释放资源
      videoARef.current?.pause();
      videoBRef.current?.pause();
    };
  }, [characterId]);

  return (
    <div
      style={{
        position: 'absolute',
        inset: 0,
        zIndex: 10,
        pointerEvents: 'none',
        overflow: 'hidden',
      }}
    >
      <style>{`
        .va-video{position:absolute;inset:0;width:100%;height:100%;object-fit:contain;opacity:0;transition:opacity .25s ease;pointer-events:none}
        .va-video.is-front{opacity:1}
      `}</style>
      <video ref={videoARef} className="va-video" muted playsInline />
      <video ref={videoBRef} className="va-video" muted playsInline />
    </div>
  );
}

export default VideoAnimationLayer;
