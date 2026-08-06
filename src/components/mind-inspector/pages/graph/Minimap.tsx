/**
 * 迷你地图 — 叠在时间轴右侧的日期跳转导航
 *
 * 把完整骨架按时间比例压缩到一条竖向密度条：每个骨架点映射为 mmY = svgY/svgHeight*mmH，
 * 按 ~mmH/3 px 分箱绘制密度条。视口指示矩形来自当前可见 SVG 范围。
 * 点击/拖拽（pointer capture）跳转到对应时间位置，悬停显示该处日期。
 */

import React, { useCallback, useMemo, useRef, useState } from 'react';
import type { TimeScale } from './timeScale';
import { COLORS } from '../../design-system';

interface SkeletonPoint {
  id: string;
  ts: number;
  kind: 'memory' | 'diary';
}

interface MinimapProps {
  skeleton: SkeletonPoint[];
  scale: TimeScale;
  /** 迷你地图高度（= 时间轴容器 clientHeight） */
  height: number;
  /** 当前可见 SVG y 范围 */
  visibleRange: { topY: number; bottomY: number };
  /** 图谱 SVG 总高度 */
  svgHeight: number;
  accent: string;
  /** 请求滚动到指定 SVG y 坐标 */
  onScrollRequest: (svgY: number) => void;
}

const MM_WIDTH = 20;
const HAND =
  '"Caveat", "Ma Shan Zheng", "Dancing Script", "Hachi Maru Pop", "Kaiti SC", "KaiTi", "STKaiti", "DFKai-SB", "PingFang SC", "Microsoft YaHei", serif';

const formatDate = (ts: number): string => {
  const d = new Date(ts);
  const now = new Date();
  const sameYear = d.getFullYear() === now.getFullYear();
  const pad = (n: number) => String(n).padStart(2, '0');
  return sameYear
    ? `${d.getMonth() + 1}月${d.getDate()}日 ${pad(d.getHours())}:${pad(d.getMinutes())}`
    : `${d.getFullYear()}年${d.getMonth() + 1}月${d.getDate()}日`;
};

const Minimap: React.FC<MinimapProps> = ({
  skeleton,
  scale,
  height,
  visibleRange,
  svgHeight,
  accent,
  onScrollRequest,
}) => {
  const trackRef = useRef<HTMLDivElement | null>(null);
  const draggingRef = useRef(false);
  const [hover, setHover] = useState<{ y: number; label: string } | null>(null);

  const mmH = Math.max(1, height);
  const toMm = useCallback(
    (svgY: number) => (svgHeight > 0 ? (svgY / svgHeight) * mmH : 0),
    [svgHeight, mmH],
  );
  const toSvg = useCallback(
    (mmY: number) => (mmH > 0 ? (mmY / mmH) * svgHeight : 0),
    [svgHeight, mmH],
  );

  // 骨架点按 ~mmH/3 px 分箱的密度分布
  const bins = useMemo(() => {
    const binCount = Math.max(1, Math.round(mmH / 3));
    const arr = new Array<number>(binCount).fill(0);
    skeleton.forEach((p, i) => {
      const mmY = toMm(scale.yAtIndex(i));
      const idx = Math.min(binCount - 1, Math.max(0, Math.floor((mmY / mmH) * binCount)));
      arr[idx] += 1;
    });
    const max = arr.reduce((a, b) => Math.max(a, b), 0);
    return { arr, max };
  }, [skeleton, scale, mmH, toMm]);

  const handleAt = useCallback(
    (clientY: number) => {
      const track = trackRef.current;
      if (!track) return;
      const rect = track.getBoundingClientRect();
      const mmY = Math.max(0, Math.min(mmH, clientY - rect.top));
      onScrollRequest(toSvg(mmY));
    },
    [mmH, toSvg, onScrollRequest],
  );

  const handleMove = useCallback(
    (e: React.PointerEvent) => {
      const track = trackRef.current;
      if (!track) return;
      const rect = track.getBoundingClientRect();
      const mmY = Math.max(0, Math.min(mmH, e.clientY - rect.top));
      setHover({ y: mmY, label: formatDate(scale.yToTs(toSvg(mmY))) });
      if (draggingRef.current) onScrollRequest(toSvg(mmY));
    },
    [mmH, toSvg, scale, onScrollRequest],
  );

  const vpTop = Math.max(0, toMm(visibleRange.topY));
  const vpBottom = Math.min(mmH, toMm(visibleRange.bottomY));
  const vpHeight = Math.max(10, vpBottom - vpTop);

  return (
    <div
      style={{
        position: 'relative',
        width: MM_WIDTH + 14,
        flexShrink: 0,
        display: 'flex',
        justifyContent: 'center',
      }}
    >
      <div
        ref={trackRef}
        onPointerDown={(e) => {
          draggingRef.current = true;
          e.currentTarget.setPointerCapture(e.pointerId);
          handleAt(e.clientY);
        }}
        onPointerMove={handleMove}
        onPointerUp={(e) => {
          draggingRef.current = false;
          if (e.currentTarget.hasPointerCapture(e.pointerId)) {
            e.currentTarget.releasePointerCapture(e.pointerId);
          }
        }}
        onPointerLeave={() => setHover(null)}
        style={{
          position: 'relative',
          width: MM_WIDTH,
          height: mmH,
          borderRadius: 6,
          border: '1px solid var(--graph-border)',
          background: 'var(--graph-card)',
          cursor: 'pointer',
          touchAction: 'none',
          overflow: 'hidden',
        }}
      >
        {/* 密度条 */}
        {bins.arr.map((count, i) => {
          if (count === 0) return null;
          const binH = mmH / bins.arr.length;
          const w = 3 + (count / Math.max(1, bins.max)) * (MM_WIDTH - 6);
          return (
            <div
              key={i}
              style={{
                position: 'absolute',
                top: i * binH,
                right: 1,
                width: w,
                height: Math.max(1, binH - 0.5),
                background: accent,
                opacity: 0.28 + 0.5 * (count / Math.max(1, bins.max)),
                borderRadius: 1,
                pointerEvents: 'none',
              }}
            />
          );
        })}

        {/* 视口指示矩形 */}
        <div
          style={{
            position: 'absolute',
            left: 1,
            right: 1,
            top: vpTop,
            height: vpHeight,
            border: `1px solid ${accent}`,
            background: COLORS.subtleBg,
            borderRadius: 3,
            pointerEvents: 'none',
            boxShadow: `0 0 0 1px ${COLORS.subtleBorder}`,
          }}
        />
      </div>

      {/* 悬停日期提示 */}
      {hover && (
        <div
          style={{
            position: 'absolute',
            right: MM_WIDTH + 12,
            top: Math.max(0, Math.min(mmH - 22, hover.y - 11)),
            padding: '2px 8px',
            background: 'var(--graph-card)',
            border: '1px solid var(--graph-border)',
            borderRadius: 4,
            boxShadow: 'var(--graph-shadow-sm)',
            fontFamily: HAND,
            fontSize: 13,
            color: 'var(--graph-ink)',
            whiteSpace: 'nowrap',
            pointerEvents: 'none',
            zIndex: 5,
          }}
        >
          {hover.label}
        </div>
      )}
    </div>
  );
};

export default Minimap;
