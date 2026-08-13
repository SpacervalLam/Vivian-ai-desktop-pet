/**
 * World 页 — 世界状态卡片网格 + 用户研究面板
 */

import React, { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { invoke } from '@tauri-apps/api/core';
import {
  AlertCircle,
  Activity,
  ArrowDown,
  ArrowDownUp,
  ArrowUp,
  Brain,
  Calendar,
  ChevronDown,
  Clock,
  Cloud,
  CloudDrizzle,
  CloudFog,
  CloudLightning,
  CloudRain,
  CloudSnow,
  Cpu,
  Disc3,
  FlaskConical,
  HardDrive,
  History,
  Leaf,
  MapPin,
  Monitor,
  Moon,
  Sun,
  Thermometer,
  Volume2,
  Wifi,
  Wind,
} from 'lucide-react';
import FlipCard from './FlipCard';
import { CARD_BG, getTimePeriod, getSeasonKey, getWeatherKey } from './world-card-backgrounds';
import { COLORS, TYPO, SPACING, RADIUS, EASE, DURATION } from '../design-system';
import {
  Card,
  EmptyState,
  SectionTitle,
  StatusDot,
  Tag,
} from '../shared-components';
import type {
  WorldSnapshotResponse,
  WorldSnapshotView,
  ResearchTaskView,
  TaskStatus,
  UserBehaviorEntryView,
  BeliefView,
} from '../../../types';

const toMs = (ts: number): number => (ts < 1e12 ? ts * 1000 : ts);

const formatClock = (ts: number): string => {
  const d = new Date(toMs(ts));
  if (Number.isNaN(d.getTime())) return '—';
  const pad = (x: number) => String(x).padStart(2, '0');
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
};

const formatRelative = (ts: number, t: TFunction): string => {
  if (!ts || ts <= 0) return '—';
  const diff = Math.max(0, Date.now() - toMs(ts));
  const min = 60_000;
  const hour = 3_600_000;
  const day = 86_400_000;
  if (diff < min) return t('mind_inspector.common.just_now');
  if (diff < hour) return t('mind_inspector.common.minutes_ago', { n: Math.floor(diff / min) });
  if (diff < day) return t('mind_inspector.common.hours_ago', { n: Math.floor(diff / hour) });
  if (diff < 7 * day) return t('mind_inspector.common.days_ago', { n: Math.floor(diff / day) });
  return formatClock(ts);
};

const periodOf = (hour: number, t: TFunction): string => {
  if (hour < 5) return t('mind_inspector.world.period_late_night');
  if (hour < 8) return t('mind_inspector.world.period_dawn');
  if (hour < 11) return t('mind_inspector.world.period_morning');
  if (hour < 14) return t('mind_inspector.world.period_noon');
  if (hour < 17) return t('mind_inspector.world.period_afternoon');
  if (hour < 19) return t('mind_inspector.world.period_dusk');
  if (hour < 23) return t('mind_inspector.world.period_evening');
  return t('mind_inspector.world.period_late_night');
};

const WEEKDAY_KEYS = ['weekday_sun', 'weekday_mon', 'weekday_tue', 'weekday_wed', 'weekday_thu', 'weekday_fri', 'weekday_sat'];

const weekdayOf = (ts: number): number => {
  const d = new Date(toMs(ts));
  return d.getDay();
};

const stripWeekday = (s: string): string =>
  s
    .replace(/,?\s*(Sunday|Monday|Tuesday|Wednesday|Thursday|Friday|Saturday)\b/g, '')
    .replace(/,?\s*(周日|周一|周二|周三|周四|周五|周六|星期[日一二三四五六天])\b/g, '')
    .trim();


const formatDurationSecs = (secs: number): string => {
  if (secs < 60) return `${secs}秒`;
  if (secs < 3600) return `${Math.floor(secs / 60)}分${secs % 60}秒`;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return `${h}小时${m}分`;
};

const formatDurationShort = (secs: number): string => {
  if (secs < 60) return `${secs.toFixed(0)}秒`;
  if (secs < 3600) return `${Math.floor(secs / 60)}分`;
  return `${(secs / 3600).toFixed(1)}小时`;
};

const formatBehaviorTime = (ts: number): string => {
  const d = new Date(toMs(ts));
  if (Number.isNaN(d.getTime())) return '—';
  const pad = (x: number) => String(x).padStart(2, '0');
  return `${d.getMonth() + 1}/${d.getDate()} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
};

const BELIEF_CATEGORY_KEY: Record<string, string> = {
  Trait: 'mind_inspector.cognition.category_trait',
  Habit: 'mind_inspector.cognition.category_habit',
  Preference: 'mind_inspector.cognition.category_preference',
  State: 'mind_inspector.cognition.category_state',
  Relationship: 'mind_inspector.cognition.category_relationship',
};

const BELIEF_STATUS_COLOR: Record<string, string> = {
  Stable: COLORS.success,
  Questioning: COLORS.event.mood,
  Superseded: COLORS.textQuaternary,
};

const formatSpeed = (bps: number): string => {
  if (bps >= 1024 * 1024 * 1024) {
    return `${(bps / (1024 * 1024 * 1024)).toFixed(1)} GB/s`;
  }
  if (bps >= 1024 * 1024) {
    return `${(bps / (1024 * 1024)).toFixed(1)} MB/s`;
  }
  if (bps >= 1024) {
    return `${(bps / 1024).toFixed(1)} KB/s`;
  }
  return `${bps.toFixed(0)} B/s`;
};

type WeatherIconProps = { size?: number; color?: string };
const weatherIcon = (code?: number): React.ReactElement<WeatherIconProps> => {
  const iconProps = { size: 24, color: COLORS.textTertiary, strokeWidth: 1.5 };
  if (code == null) return <Cloud {...iconProps} />;
  if (code === 0) return <Sun {...iconProps} />;
  if (code <= 3) return <Cloud {...iconProps} />;
  if (code <= 48) return <CloudFog {...iconProps} />;
  if (code <= 57) return <CloudDrizzle {...iconProps} />;
  if (code <= 67) return <CloudRain {...iconProps} />;
  if (code <= 77) return <CloudSnow {...iconProps} />;
  if (code <= 82) return <CloudRain {...iconProps} />;
  if (code <= 86) return <CloudSnow {...iconProps} />;
  if (code >= 95) return <CloudLightning {...iconProps} />;
  return <Cloud {...iconProps} />;
};

const STATUS_COLOR: Record<TaskStatus, string> = {
  Active: COLORS.success,
  Paused: COLORS.textTertiary,
  Concluded: COLORS.event.observation,
};

const STATUS_LABEL_KEYS: Record<TaskStatus, string> = {
  Active: 'research_status_active',
  Paused: 'research_status_paused',
  Concluded: 'research_status_concluded',
};

interface MarqueeTextProps {
  children: string;
  style?: React.CSSProperties;
}

const MarqueeText: React.FC<MarqueeTextProps> = ({ children, style }) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const trackRef = useRef<HTMLSpanElement>(null);
  const measureRef = useRef<HTMLSpanElement>(null);
  const [duration, setDuration] = useState<number | null>(null);

  useLayoutEffect(() => {
    const measure = () => {
      const c = containerRef.current;
      const m = measureRef.current;
      if (!c || !m) {
        setDuration(null);
        return;
      }
      const gap = parseFloat(getComputedStyle(m).paddingRight) || 0;
      const textWidth = m.scrollWidth - gap;
      const containerWidth = c.clientWidth;
      if (textWidth > containerWidth + 1) {
        setDuration(Math.max(6, Math.min(20, textWidth / 40)));
      } else {
        setDuration(null);
      }
    };
    measure();
    const ro = new ResizeObserver(measure);
    if (containerRef.current) ro.observe(containerRef.current);
    return () => ro.disconnect();
  }, [children]);

  const scrolling = duration !== null;

  return (
    <div
      ref={containerRef}
      style={{
        ...style,
        overflow: 'hidden',
        whiteSpace: 'nowrap',
      }}
    >
      <span
        ref={trackRef}
        className={scrolling ? 'marquee-pause' : undefined}
        style={{
          display: 'inline-block',
          whiteSpace: 'nowrap',
          ...(scrolling
            ? {
                animationName: 'marquee-loop',
                animationDuration: `${duration}s`,
                animationTimingFunction: 'linear',
                animationIterationCount: 'infinite',
              }
            : {}),
        }}
      >
        <span
          ref={measureRef}
          style={{ display: 'inline-block', whiteSpace: 'nowrap', paddingRight: '2em' }}
        >
          {children}
        </span>
        {scrolling && (
          <span
            aria-hidden="true"
            style={{ display: 'inline-block', whiteSpace: 'nowrap', paddingRight: '2em' }}
          >
            {children}
          </span>
        )}
      </span>
    </div>
  );
};

const MemoMarqueeText = React.memo(MarqueeText);

const KEYFRAMES_STYLE_ID = 'world-page-keyframes';
if (typeof document !== 'undefined' && !document.getElementById(KEYFRAMES_STYLE_ID)) {
  const style = document.createElement('style');
  style.id = KEYFRAMES_STYLE_ID;
  style.textContent = `
@keyframes world-rain-drop {
  0% { transform: translateY(-100%); opacity: 0; }
  10% { opacity: 0.6; }
  90% { opacity: 0.6; }
  100% { transform: translateY(200%); opacity: 0; }
}
@keyframes world-sun-glow {
  0%, 100% { opacity: 0.4; transform: scale(1); }
  50% { opacity: 0.6; transform: scale(1.1); }
}
@keyframes world-snowflake {
  0% { transform: translateY(-100%) rotate(0deg); opacity: 0; }
  10% { opacity: 0.5; }
  90% { opacity: 0.5; }
  100% { transform: translateY(200%) rotate(360deg); opacity: 0; }
}
@keyframes world-disc-spin {
  to { transform: rotate(360deg); }
}
@keyframes world-spectrum-bar {
  0%, 100% { height: 20%; }
  50% { height: 100%; }
}
@keyframes world-wind-blow {
  0%, 100% { transform: translateX(0); }
  50% { transform: translateX(4px); }
}
@keyframes world-float {
  0%, 100% { transform: translateY(0); }
  50% { transform: translateY(-3px); }
}
@keyframes world-twinkle {
  0%, 100% { opacity: 0.3; }
  50% { opacity: 0.8; }
}
@keyframes marquee-loop {
  0% { transform: translateX(0); }
  100% { transform: translateX(-50%); }
}
@keyframes world-rise-in {
  from { opacity: 0; transform: translateY(8px); }
  to { opacity: 1; transform: translateY(0); }
}
.world-rise-in {
  animation: world-rise-in 0.5s cubic-bezier(0.16, 1, 0.3, 1) both;
}
@media (prefers-reduced-motion: reduce) {
  .world-rise-in { animation: none; }
}
`;
  document.head.appendChild(style);
}

interface WeatherAnimationProps {
  weatherCode?: number;
}

const WeatherAnimation: React.FC<WeatherAnimationProps> = ({ weatherCode }) => {
  if (weatherCode == null) return null;

  if ([51, 53, 55, 61, 63, 65, 80, 81, 82].includes(weatherCode)) {
    const drops = Array.from({ length: 8 }, (_, i) => (
      <div
        key={i}
        style={{
          position: 'absolute',
          width: 1,
          height: 12,
          background: 'linear-gradient(to bottom, rgba(59,130,246,0.6), rgba(59,130,246,0))',
          left: `${10 + i * 12}%`,
          top: '-20%',
          borderRadius: '0 0 2px 2px',
          animation: `world-rain-drop ${0.6 + Math.random() * 0.4}s linear infinite`,
          animationDelay: `${Math.random() * 2}s`,
        }}
      />
    ));
    return <div style={{ position: 'absolute', inset: 0, overflow: 'hidden', pointerEvents: 'none' }}>{drops}</div>;
  }

  if ([71, 73, 75, 77, 85, 86].includes(weatherCode)) {
    const flakes = Array.from({ length: 6 }, (_, i) => (
      <div
        key={i}
        style={{
          position: 'absolute',
          width: 4,
          height: 4,
          background: 'rgba(255,255,255,0.7)',
          borderRadius: '50%',
          left: `${8 + i * 15}%`,
          top: '-10%',
          boxShadow: '0 0 4px rgba(255,255,255,0.5)',
          animation: `world-snowflake ${2 + Math.random() * 2}s linear infinite`,
          animationDelay: `${Math.random() * 3}s`,
        }}
      />
    ));
    return <div style={{ position: 'absolute', inset: 0, overflow: 'hidden', pointerEvents: 'none' }}>{flakes}</div>;
  }

  if (weatherCode === 0) {
    return (
      <div
        style={{
          position: 'absolute',
          width: 60,
          height: 60,
          background: 'radial-gradient(circle, rgba(251,191,36,0.15) 0%, transparent 70%)',
          borderRadius: '50%',
          top: '-10px',
          right: '-10px',
          animation: 'world-sun-glow 3s ease-in-out infinite',
          pointerEvents: 'none',
        }}
      />
    );
  }

  if ([45, 48].includes(weatherCode)) {
    const fogLayers = Array.from({ length: 3 }, (_, i) => (
      <div
        key={i}
        style={{
          position: 'absolute',
          width: '80%',
          height: 8,
          background: 'rgba(156,163,175,0.15)',
          borderRadius: '10px',
          left: '10%',
          top: `${30 + i * 20}%`,
          animation: `world-wind-blow ${3 + i}s ease-in-out infinite`,
          animationDelay: `${i * 0.5}s`,
          pointerEvents: 'none',
        }}
      />
    ));
    return <div style={{ position: 'absolute', inset: 0, overflow: 'hidden' }}>{fogLayers}</div>;
  }

  return null;
};

interface SeasonEffectProps {
  season: string;
}

const getSeasonTint = (season: string): string => {
  switch (season.toLowerCase()) {
    case 'spring':
      return 'rgba(34,197,94,0.04)';
    case 'summer':
      return 'rgba(59,130,246,0.04)';
    case 'autumn':
      return 'rgba(249,115,22,0.04)';
    case 'winter':
      return 'rgba(148,163,184,0.04)';
    default:
      return 'transparent';
  }
};

const getPeriodTint = (hour: number): string => {
  if (hour < 6) return 'rgba(30,41,59,0.06)';
  if (hour < 12) return 'rgba(253,224,71,0.03)';
  if (hour < 18) return 'rgba(251,146,60,0.03)';
  return 'rgba(49,46,129,0.05)';
};

interface SpectrumVisualizerProps {
  isPlaying: boolean;
}

const SpectrumVisualizer: React.FC<SpectrumVisualizerProps> = ({ isPlaying }) => {
  if (!isPlaying) return null;

  const bars = Array.from({ length: 6 }, (_, i) => (
    <div
      key={i}
      style={{
        width: 3,
        height: '100%',
        background: 'linear-gradient(to top, rgba(34,197,94,0.6), rgba(34,197,94,0.2))',
        borderRadius: 2,
        animation: `world-spectrum-bar ${0.3 + Math.random() * 0.3}s ease-in-out infinite`,
        animationDelay: `${i * 0.08}s`,
      }}
    />
  ));

  return (
    <div
      style={{
        position: 'absolute',
        bottom: 8,
        right: 8,
        width: 24,
        height: 16,
        display: 'flex',
        alignItems: 'flex-end',
        gap: 2,
      }}
    >
      {bars}
    </div>
  );
};

interface DimensionCardProps {
  label: string;
  value: React.ReactNode;
  hint?: React.ReactNode;
  accent?: string;
  icon?: React.ReactNode;
  bgTint?: string;
  bgImage?: string;
  animation?: React.ReactNode;
  children?: React.ReactNode;
}

const DimensionCard: React.FC<DimensionCardProps> = ({
  label,
  value,
  hint,
  accent,
  icon,
  bgTint,
  bgImage,
  animation,
  children,
}) => (
  <Card style={bgTint ? { background: bgTint } : undefined}>
    {bgImage && (
      <img
        src={bgImage}
        alt=""
        aria-hidden
        style={{
          position: 'absolute',
          inset: 0,
          width: '100%',
          height: '100%',
          objectFit: 'cover',
          opacity: 0.35,
          pointerEvents: 'none',
          zIndex: 0,
        }}
      />
    )}
    {animation}
    <div style={{ position: 'relative', zIndex: 1 }}>
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          marginBottom: SPACING.sm,
        }}
      >
        <span style={{ ...TYPO.caption, color: COLORS.textTertiary }}>{label}</span>
        {icon && <span style={{ fontSize: 14, opacity: 0.7 }}>{icon}</span>}
      </div>
      <div
        style={{
          ...TYPO.h2,
          color: accent ?? COLORS.textPrimary,
          marginBottom: hint ? 4 : 0,
        }}
      >
        {typeof value === 'string' ? (
          <MemoMarqueeText>{value || '—'}</MemoMarqueeText>
        ) : (
          <span style={{ whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis', display: 'block' }}>
            {value ?? '—'}
          </span>
        )}
      </div>
      {hint && (
        <div
          style={{
            ...TYPO.body,
            fontSize: 12,
            color: COLORS.textSecondary,
            whiteSpace: 'nowrap',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
          }}
        >
          {hint}
        </div>
      )}
      {children}
    </div>
  </Card>
);

const MemoDimensionCard = React.memo(DimensionCard);

interface ResearchTaskRowProps {
  task: ResearchTaskView;
}

const ResearchTaskRow: React.FC<ResearchTaskRowProps> = ({ task }) => {
  const { t } = useTranslation();
  const color = STATUS_COLOR[task.status];
  const latestSample = task.samples.length > 0 ? task.samples[task.samples.length - 1] : null;
  const [hovered, setHovered] = useState(false);

  return (
    <div
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        display: 'flex',
        alignItems: 'flex-start',
        gap: SPACING.md,
        padding: `${SPACING.sm}px ${SPACING.sm + 2}px`,
        borderRadius: RADIUS.sm,
        background: hovered ? COLORS.bgHover : COLORS.subtleBg,
        border: `1px solid ${hovered ? COLORS.borderHover : COLORS.subtleBorder}`,
        marginBottom: SPACING.xs,
        transition: `background ${DURATION.fast}s ${EASE.swift}, border-color ${DURATION.fast}s ${EASE.swift}`,
      }}
    >
      <div
        style={{
          width: 8,
          height: 8,
          borderRadius: '50%',
          background: color,
          flexShrink: 0,
          marginTop: 4,
        }}
      />
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs }}>
          <span
            style={{
              ...TYPO.body,
              fontSize: 12,
              fontWeight: 500,
              color: COLORS.textPrimary,
            }}
          >
            {task.target}
          </span>
          <Tag color={color}>
            {t(`mind_inspector.world.${STATUS_LABEL_KEYS[task.status]}`)}
          </Tag>
          <span style={{ ...TYPO.caption, fontSize: 10, color: COLORS.textQuaternary }}>
            {task.samples.length} {t('mind_inspector.world.research_samples')}
          </span>
        </div>
        {task.conclusion ? (
          <div
            style={{
              ...TYPO.body,
              fontSize: 11,
              color: COLORS.event.observation,
              marginTop: 2,
            }}
          >
            {task.conclusion.summary}
            {task.conclusion.mean_time && (
              <span style={{ color: COLORS.textTertiary }}> · {task.conclusion.mean_time}</span>
            )}
            <span style={{ color: COLORS.textQuaternary, fontSize: 10 }}>
              {' '}({Math.round(task.conclusion.confidence * 100)}%)
            </span>
          </div>
        ) : latestSample ? (
          <div
            style={{
              ...TYPO.caption,
              color: COLORS.textSecondary,
              fontSize: 11,
              marginTop: 2,
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
            }}
          >
            {latestSample.observation}
          </div>
        ) : null}
      </div>
    </div>
  );
};

const MemoResearchTaskRow = React.memo(ResearchTaskRow);

interface ResearchPanelProps {
  tasks: ResearchTaskView[];
  snapshot: WorldSnapshotView | undefined;
  behaviors: UserBehaviorEntryView[];
  beliefs: BeliefView[];
}

const ResearchPanel: React.FC<ResearchPanelProps> = ({ tasks, snapshot, behaviors, beliefs }) => {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);

  const presence = snapshot?.user_presence;
  const activity = presence?.current_activity ?? null;
  const isPresent = presence?.presence === 'present';
  const hasData = !!(activity || behaviors.length > 0 || beliefs.length > 0);
  const activeTaskCount = tasks.filter((tk) => tk.status === 'Active').length;

  const activityElapsed = activity
    ? formatDurationShort(Math.max(0, Math.floor((Date.now() - toMs(activity.started_at)) / 1000)))
    : '';
  const awaySecs = presence?.away_elapsed_secs ?? 0;

  const statusIcon = activity ? <Activity size={14} /> : <Clock size={14} />;
  const statusColor = activity ? COLORS.accent : (isPresent ? COLORS.success : COLORS.textQuaternary);

  const summaryHint: string[] = [];
  if (behaviors.length > 0) summaryHint.push(`${behaviors.length} ${t('mind_inspector.cognition.timeline_title')}`);
  if (beliefs.length > 0) summaryHint.push(`${beliefs.length} ${t('mind_inspector.cognition.knowledge_title')}`);
  if (activeTaskCount > 0) summaryHint.push(`${activeTaskCount} ${t('mind_inspector.world.research_active')}`);

  return (
    <div className="world-rise-in">
      {/* 摘要卡片 */}
      <Card
        style={{ cursor: hasData ? 'pointer' : 'default' }}
        onClick={() => hasData && setExpanded(!expanded)}
      >
        <img
          src={CARD_BG.observation}
          alt=""
          aria-hidden
          style={{
            position: 'absolute',
            inset: 0,
            width: '100%',
            height: '100%',
            objectFit: 'cover',
            opacity: 0.25,
            pointerEvents: 'none',
            zIndex: 0,
          }}
        />
        <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.md, position: 'relative', zIndex: 1 }}>
          <div
            style={{
              width: 28,
              height: 28,
              borderRadius: RADIUS.sm,
              background: COLORS.bgSurfaceElevated,
              border: `1px solid ${statusColor}`,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
            }}
          >
            {statusIcon}
            {activity && (
              <div
                style={{
                  position: 'absolute',
                  width: 6,
                  height: 6,
                  borderRadius: '50%',
                  background: COLORS.accent,
                  animation: 'mind-inspector-pulse 2s ease-in-out infinite',
                }}
              />
            )}
          </div>

          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.sm }}>
              <span style={{ ...TYPO.caption, color: COLORS.textTertiary }}>
                {t('mind_inspector.world.dim_user_presence')}
              </span>
              <Tag color={statusColor}>{isPresent ? t('mind_inspector.cognition.presence_present') : t('mind_inspector.cognition.presence_away')}</Tag>
              {activeTaskCount > 0 && (
                <Tag color={COLORS.event.observation}>
                  {activeTaskCount} {t('mind_inspector.world.research_active')}
                </Tag>
              )}
            </div>
            <div
              style={{
                ...TYPO.body,
                color: COLORS.textPrimary,
                fontSize: 13,
                marginTop: 2,
                whiteSpace: 'nowrap',
                overflow: 'hidden',
                textOverflow: 'ellipsis',
              }}
            >
              {activity
                ? `${activity.label} · ${activityElapsed}`
                : isPresent
                ? t('mind_inspector.cognition.no_activity_hint')
                : t('mind_inspector.cognition.elapsed_label', { duration: formatDurationShort(awaySecs) })}
            </div>
            {summaryHint.length > 0 && (
              <div
                style={{
                  ...TYPO.caption,
                  color: COLORS.textTertiary,
                  fontSize: 11,
                  whiteSpace: 'nowrap',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                }}
              >
                {summaryHint.join(' · ')}
              </div>
            )}
          </div>

          {hasData && (
            <ChevronDown
              size={16}
              color={COLORS.textTertiary}
              strokeWidth={1.5}
              style={{
                flexShrink: 0,
                transition: 'transform 200ms ease',
                transform: expanded ? 'rotate(180deg)' : 'rotate(0deg)',
              }}
            />
          )}
        </div>
      </Card>

      {/* 展开态：三栏认知内容 */}
      {expanded && hasData && (
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'minmax(160px, 1fr) minmax(0, 2fr) minmax(0, 2fr)',
            gap: SPACING.cardGap,
            marginTop: SPACING.sm,
          }}
        >
          {/* 当前状态 */}
          <Card>
            <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs, marginBottom: SPACING.sm }}>
              <StatusDot color={activity ? COLORS.accent : (isPresent ? COLORS.success : COLORS.textQuaternary)} pulse={!!activity} />
              <span style={{ ...TYPO.caption, color: COLORS.textTertiary }}>
                {t('mind_inspector.cognition.current_state_title')}
              </span>
            </div>
            {activity ? (
              <>
                <div style={{ ...TYPO.h2, color: COLORS.accent, marginBottom: 4 }}>
                  {activity.label}
                </div>
                <div style={{ ...TYPO.body, fontSize: 12, color: COLORS.textSecondary }}>
                  {t('mind_inspector.cognition.since_label', { duration: activityElapsed })}
                </div>
              </>
            ) : (
              <>
                <div style={{ ...TYPO.h2, color: isPresent ? COLORS.success : COLORS.textQuaternary, marginBottom: 4 }}>
                  {isPresent ? t('mind_inspector.cognition.presence_present') : t('mind_inspector.cognition.presence_away')}
                </div>
                <div style={{ ...TYPO.body, fontSize: 12, color: COLORS.textSecondary }}>
                  {!isPresent && awaySecs > 0
                    ? t('mind_inspector.cognition.elapsed_label', { duration: formatDurationShort(awaySecs) })
                    : t('mind_inspector.cognition.no_activity_hint')}
                </div>
              </>
            )}
          </Card>

          {/* 行为时间线 */}
          <Card style={{ padding: 0 }}>
            <div style={{ padding: SPACING.md, paddingBottom: SPACING.xs }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs }}>
                <History size={14} color={COLORS.textTertiary} strokeWidth={1.5} />
                <span style={{ ...TYPO.caption, color: COLORS.textTertiary }}>
                  {t('mind_inspector.cognition.timeline_title')}
                </span>
              </div>
            </div>
            {behaviors.length === 0 ? (
              <div style={{ padding: SPACING.md, paddingTop: 0 }}>
                <EmptyState text={t('mind_inspector.cognition.timeline_empty')} />
              </div>
            ) : (
              <div style={{ maxHeight: 240, overflowY: 'auto', padding: SPACING.sm }}>
                {behaviors.map((entry, idx) => (
                  <div
                    key={entry.id}
                    style={{
                      display: 'flex',
                      alignItems: 'flex-start',
                      gap: SPACING.sm,
                      padding: SPACING.sm,
                      borderRadius: RADIUS.sm,
                      background: COLORS.subtleBg,
                      border: `1px solid ${COLORS.subtleBorder}`,
                      marginBottom: idx < behaviors.length - 1 ? SPACING.xs : 0,
                    }}
                  >
                    <div
                      style={{
                        width: 6,
                        height: 6,
                        borderRadius: '50%',
                        background: COLORS.accent,
                        flexShrink: 0,
                        marginTop: 6,
                        opacity: 0.7,
                      }}
                    />
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs, flexWrap: 'wrap' }}>
                        <span style={{ ...TYPO.body, fontSize: 12, fontWeight: 500, color: COLORS.textPrimary }}>
                          {entry.activity_label}
                        </span>
                        <Tag color={COLORS.event.observation}>
                          {formatDurationShort(entry.duration_secs)}
                        </Tag>
                      </div>
                      <div style={{ ...TYPO.caption, fontSize: 10, color: COLORS.textQuaternary, marginTop: 2 }}>
                        {formatBehaviorTime(entry.started_at)} → {formatBehaviorTime(entry.ended_at)}
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </Card>

          {/* 认知信念 */}
          <Card style={{ padding: 0 }}>
            <div style={{ padding: SPACING.md, paddingBottom: SPACING.xs }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs }}>
                <Brain size={14} color={COLORS.textTertiary} strokeWidth={1.5} />
                <span style={{ ...TYPO.caption, color: COLORS.textTertiary }}>
                  {t('mind_inspector.cognition.knowledge_title')}
                </span>
              </div>
            </div>
            {beliefs.length === 0 ? (
              <div style={{ padding: SPACING.md, paddingTop: 0 }}>
                <EmptyState text={t('mind_inspector.cognition.knowledge_empty')} />
              </div>
            ) : (
              <div style={{ maxHeight: 240, overflowY: 'auto', padding: SPACING.sm }}>
                {beliefs.map((b, idx) => {
                  const status = b.status ?? 'Stable';
                  const statusColor = BELIEF_STATUS_COLOR[status] ?? COLORS.textTertiary;
                  const categoryLabel = BELIEF_CATEGORY_KEY[b.category]
                    ? t(BELIEF_CATEGORY_KEY[b.category])
                    : b.category;
                  return (
                    <div
                      key={b.id}
                      style={{
                        padding: SPACING.sm,
                        borderRadius: RADIUS.sm,
                        background: COLORS.subtleBg,
                        border: `1px solid ${COLORS.subtleBorder}`,
                        marginBottom: idx < beliefs.length - 1 ? SPACING.xs : 0,
                      }}
                    >
                      <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs, flexWrap: 'wrap', marginBottom: 2 }}>
                        <span style={{ ...TYPO.body, fontSize: 12, fontWeight: 500, color: COLORS.textPrimary, flex: 1, minWidth: 0 }}>
                          {b.statement}
                        </span>
                        <Tag color={statusColor}>
                          {Math.round(b.confidence * 100)}%
                        </Tag>
                      </div>
                      <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs, flexWrap: 'wrap' }}>
                        <span style={{ ...TYPO.caption, fontSize: 10, color: COLORS.textQuaternary }}>
                          {categoryLabel}
                        </span>
                        {b.metric && b.value != null && (
                          <span style={{ ...TYPO.caption, fontSize: 10, color: COLORS.event.observation }}>
                            · {b.metric} = {b.value.toFixed(1)}
                          </span>
                        )}
                        {status === 'Questioning' && (
                          <span style={{ ...TYPO.caption, fontSize: 10, color: COLORS.event.mood }}>
                            · {t('mind_inspector.cognition.status_questioning')}
                          </span>
                        )}
                      </div>
                    </div>
                  );
                })}
              </div>
            )}
          </Card>
        </div>
      )}
    </div>
  );
};

const WorldPage: React.FC = () => {
  const { t } = useTranslation();
  const [data, setData] = useState<WorldSnapshotResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [refreshingLocation, setRefreshingLocation] = useState(false);
  const lastLocationClickRef = useRef<number>(0);
  const [scheduledTasks, setScheduledTasks] = useState<Array<{
    id: string;
    message: string;
    scheduled_time: number;
    status: string;
  }>>([]);

  const fetchData = useCallback(async () => {
    try {
      const res = await invoke<WorldSnapshotResponse>('get_world_snapshot');
      setData(res);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
    // 获取待执行日程
    try {
      const schedRes = await invoke<{ tasks: Array<{ id: string; message: string; scheduled_time: number; status: string }> }>('list_scheduled_tasks');
      const pending = (schedRes.tasks || [])
        .filter((tk) => tk.status === 'pending' || tk.status === 'running')
        .sort((a, b) => a.scheduled_time - b.scheduled_time)
        .slice(0, 3);
      setScheduledTasks(pending);
    } catch {
      // 静默失败
    }
  }, []);

  const handleLocationClick = useCallback(async () => {
    const now = Date.now();
    // 防抖：5 秒内不允许重复触发
    if (now - lastLocationClickRef.current < 5000) return;
    lastLocationClickRef.current = now;
    setRefreshingLocation(true);
    try {
      await invoke('auto_detect_location');
      await fetchData();
    } catch {
      // 静默失败，不影响 UI
    } finally {
      setRefreshingLocation(false);
    }
  }, [fetchData]);

  useEffect(() => {
    void fetchData();
    const id = window.setInterval(fetchData, 10_000);
    return () => window.clearInterval(id);
  }, [fetchData]);

  if (loading && !data) {
    return (
      <div style={{ flex: 1, display: 'flex' }}>
        <EmptyState spinner text={t('mind_inspector.world.loading')} />
      </div>
    );
  }

  if (error && !data) {
    return (
      <div style={{ flex: 1, display: 'flex' }}>
        <EmptyState icon={<AlertCircle size={24} color={COLORS.textTertiary} strokeWidth={1.5} />} text={t('mind_inspector.common.load_failed', { error })} />
      </div>
    );
  }

  const snap: WorldSnapshotView | undefined = data?.snapshot;
  const researchTasks: ResearchTaskView[] = data?.research ?? [];
  const behaviors: UserBehaviorEntryView[] = data?.behaviors ?? [];
  const userBeliefs: BeliefView[] = data?.user_beliefs ?? [];

  const cards: React.ReactNode[] = [];
  if (snap) {
    const seasonTint = getSeasonTint(snap.season);
    const periodTint = getPeriodTint(snap.hour);
    const isRainy = snap.weather && [51, 53, 55, 61, 63, 65, 80, 81, 82, 95, 96, 99].includes(snap.weather.weather_code ?? -1);
    const isPlaying = !!(snap.music && snap.music.status === 'Playing');

    cards.push(
      <FlipCard
        key="time-season"
        style={{ height: '100%' }}
        front={
          <MemoDimensionCard
            label={t('mind_inspector.world.dim_time')}
            icon={<Clock size={24} color={COLORS.textTertiary} strokeWidth={1.5} />}
            value={snap.local_time ? stripWeekday(snap.local_time) : '—'}
            hint={`${periodOf(snap.hour, t)} · ${WEEKDAY_KEYS[weekdayOf(snap.timestamp)] ? t(`mind_inspector.world.${WEEKDAY_KEYS[weekdayOf(snap.timestamp)]}`) : ''}${
              snap.is_weekend ? t('mind_inspector.world.weekend_suffix') : ''
            }`}
            bgTint={periodTint}
            bgImage={CARD_BG[`time-${getTimePeriod(snap.hour)}`]}
          />
        }
        back={
          <MemoDimensionCard
            label={t('mind_inspector.world.dim_season')}
            icon={<Leaf size={24} color={COLORS.textTertiary} strokeWidth={1.5} />}
            value={snap.season ? t(`mind_inspector.world.season_${snap.season.toLowerCase()}`) : '—'}
            hint={
              [
                snap.solar_term ? t(`mind_inspector.world.solar_term_${snap.solar_term}`) : null,
                snap.festival ? t(`mind_inspector.world.festival_${snap.festival}`) : null,
              ].filter(Boolean).join(' · ') || '—'
            }
            bgTint={seasonTint}
            bgImage={CARD_BG[`season-${getSeasonKey(snap.season)}`]}
          />
        }
      />,
    );

    cards.push(
      <MemoDimensionCard
        key="weather-sun"
        label={t('mind_inspector.world.dim_weather')}
        icon={weatherIcon(snap.weather?.weather_code)}
        bgTint={isRainy ? 'rgba(59,130,246,0.06)' : seasonTint}
        bgImage={CARD_BG[`weather-${getWeatherKey(snap.weather?.weather_code)}`]}
        animation={<WeatherAnimation weatherCode={snap.weather?.weather_code} />}
        value={
          snap.weather
            ? `${t(`mind_inspector.world.weather_desc.${snap.weather.weather_code ?? 'unknown'}`)}${
                snap.weather.temperature != null ? ` · ${snap.weather.temperature}°C` : ''
              }`
            : t('mind_inspector.world.weather_not_available')
        }
        hint={
          snap.weather
            ? `${t('mind_inspector.world.humidity')} ${Math.round(snap.weather.humidity ?? 0)}% · ${t('mind_inspector.world.wind_speed')} ${(snap.weather.wind_speed ?? 0).toFixed(1)} km/h`
            : t('mind_inspector.world.weather_not_available')
        }
      />,
    );

    cards.push(
      <FlipCard
        key="music-volume"
        style={{ height: '100%' }}
        front={
          <MemoDimensionCard
            label={t('mind_inspector.world.dim_music')}
            icon={
              <Disc3
                size={24}
                color={isPlaying ? COLORS.success : COLORS.textTertiary}
                strokeWidth={1.5}
                style={isPlaying ? { animation: 'world-disc-spin 3s linear infinite' } : undefined}
              />
            }
            accent={isPlaying ? COLORS.success : COLORS.textPrimary}
            bgImage={CARD_BG.media}
            value={snap.music?.title || t('mind_inspector.world.music_not_playing')}
            hint={
              snap.music
                ? [snap.music.artist, snap.music.status].filter(Boolean).join(' · ')
                : t('mind_inspector.world.music_not_playing')
            }
            animation={<SpectrumVisualizer isPlaying={isPlaying} />}
          />
        }
        back={
          <MemoDimensionCard
            label={t('mind_inspector.world.dim_volume')}
            icon={
              <Volume2
                size={24}
                color={snap.volume?.muted ? COLORS.textQuaternary : COLORS.textTertiary}
                strokeWidth={1.5}
              />
            }
            bgImage={CARD_BG.volume}
            value={
              snap.volume
                ? `${snap.volume.level}%${snap.volume.muted ? ` · ${t('mind_inspector.world.muted')}` : ''}`
                : t('mind_inspector.world.volume_unknown')
            }
            hint={snap.volume?.device_name || t('mind_inspector.world.volume_hint')}
          />
        }
      />,
    );

    const memUsedGB = snap.system ? (snap.system.memory_used / (1024 * 1024 * 1024)).toFixed(1) : '—';
    const memTotalGB = snap.system ? (snap.system.memory_total / (1024 * 1024 * 1024)).toFixed(1) : '—';
    cards.push(
      <FlipCard
        key="cpu-memory"
        style={{ height: '100%' }}
        front={
          <MemoDimensionCard
            label={t('mind_inspector.world.dim_cpu')}
            icon={<Cpu size={24} color={COLORS.textTertiary} strokeWidth={1.5} />}
            value={snap.system ? `${snap.system.cpu_usage.toFixed(0)}%` : '—'}
            hint={t('mind_inspector.world.cpu_hint')}
            bgImage={CARD_BG.cpu}
          />
        }
        back={
          <MemoDimensionCard
            label={t('mind_inspector.world.dim_memory')}
            icon={<HardDrive size={24} color={COLORS.textTertiary} strokeWidth={1.5} />}
            value={`${memUsedGB} / ${memTotalGB} GB`}
            hint={snap.system ? `${snap.system.memory_usage_pct.toFixed(0)}%` : '—'}
            bgImage={CARD_BG.memory}
          />
        }
      />,
    );

    const isGame = snap.foreground_window?.process?.toLowerCase().includes('game') ?? false;
    cards.push(
      <MemoDimensionCard
        key="foreground"
        label={t('mind_inspector.world.dim_foreground')}
        icon={<Monitor size={24} color={isGame ? COLORS.event.mood : COLORS.textTertiary} strokeWidth={1.5} />}
        accent={isGame ? COLORS.event.mood : COLORS.textPrimary}
        value={snap.foreground_window?.title || t('mind_inspector.world.foreground_unknown')}
        hint={snap.foreground_window?.process || t('mind_inspector.world.foreground_hint')}
        bgImage={CARD_BG.foreground}
      />,
    );

    cards.push(
      <FlipCard
        key="network-speed"
        style={{ height: '100%' }}
        front={
          <MemoDimensionCard
            label={t('mind_inspector.world.dim_network')}
            icon={
              <Wifi
                size={24}
                color={snap.network_status?.connected ? COLORS.success : COLORS.textQuaternary}
                strokeWidth={1.5}
              />
            }
            bgImage={CARD_BG.network}
            value={
              snap.network_status?.connected
                ? (snap.network_status.name || t('mind_inspector.network.connected'))
                : t('mind_inspector.network.disconnected')
            }
            hint={snap.network_status?.interface_type || t('mind_inspector.network.hint')}
          />
        }
        back={
          <MemoDimensionCard
            label={t('mind_inspector.world.dim_network_speed')}
            icon={
              <Activity
                size={24}
                color={COLORS.textTertiary}
                strokeWidth={1.5}
              />
            }
            bgImage={CARD_BG.speed}
            value={
              snap.system ? (
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 10 }}>
                  <span style={{ color: '#7A9AA8' }}>
                    <ArrowDown size={16} strokeWidth={2} style={{ verticalAlign: '-2px', marginRight: 3 }} />
                    {formatSpeed(snap.system.net_download_bps)}
                  </span>
                  <span style={{ color: COLORS.textQuaternary, fontWeight: 300 }}>·</span>
                  <span style={{ color: '#A67C5B' }}>
                    <ArrowUp size={16} strokeWidth={2} style={{ verticalAlign: '-2px', marginRight: 3 }} />
                    {formatSpeed(snap.system.net_upload_bps)}
                  </span>
                </span>
              ) : '—'
            }
            hint={t('mind_inspector.world.network_hint')}
          />
        }
      />,
    );

    // 去重：城市邦（如 Singapore）city/region/country 可能相同
    const locParts = snap.location
      ? [...new Set([snap.location.city, snap.location.region].filter(Boolean))]
      : [];
    const locValue = locParts.join(' ') || t('mind_inspector.world.location_unknown');
    const locHint = snap.location?.country && snap.location.country !== locValue
      ? snap.location.country
      : t('mind_inspector.world.location_hint');
    cards.push(
      <div
        key="location"
        onClick={handleLocationClick}
        style={{ cursor: 'pointer', opacity: refreshingLocation ? 0.6 : 1, transition: 'opacity 200ms' }}
        title={t('mind_inspector.world.location_click_refresh')}
      >
        <MemoDimensionCard
          label={t('mind_inspector.world.dim_location')}
          icon={<MapPin size={24} color={COLORS.textTertiary} strokeWidth={1.5} />}
          value={refreshingLocation ? t('mind_inspector.world.location_refreshing') : locValue}
          hint={locHint}
          bgImage={CARD_BG.location}
        />
      </div>,
    );

    // 最近日程卡片
    const nextTask = scheduledTasks.length > 0 ? scheduledTasks[0] : null;
    const scheduleValue = nextTask
      ? nextTask.message
      : t('mind_inspector.world.schedule_empty');
    const scheduleHint = nextTask
      ? `${t('mind_inspector.world.schedule_at')} ${formatClock(nextTask.scheduled_time)}${scheduledTasks.length > 1 ? ` · ${t('mind_inspector.world.schedule_more', { n: scheduledTasks.length - 1 })}` : ''}`
      : t('mind_inspector.world.schedule_hint');
    cards.push(
      <MemoDimensionCard
        key="schedule"
        label={t('mind_inspector.world.dim_schedule')}
        icon={<Calendar size={24} color={nextTask ? COLORS.accent : COLORS.textTertiary} strokeWidth={1.5} />}
        accent={nextTask ? COLORS.accent : COLORS.textPrimary}
        value={scheduleValue}
        hint={scheduleHint}
        bgImage={CARD_BG.presence}
      />,
    );
  }

  return (
    <div
      style={{
        flex: 1,
        overflowY: 'auto',
        padding: SPACING.lg,
        display: 'flex',
        flexDirection: 'column',
        gap: SPACING.sectionGap,
      }}
    >
      <div className="world-rise-in">
        <SectionTitle style={{ marginBottom: SPACING.md }}>
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: SPACING.xs }}>
            <StatusDot color={COLORS.accent} pulse />
            {t('mind_inspector.world.state_title')}
          </span>
        </SectionTitle>
        {cards.length === 0 ? (
          <EmptyState text={t('mind_inspector.world.no_snapshot')} />
        ) : (
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'repeat(4, minmax(0, 1fr))',
              gap: SPACING.cardGap,
              width: '100%',
            }}
          >
            {cards}
          </div>
        )}
      </div>

      <ResearchPanel
        tasks={researchTasks}
        snapshot={snap}
        behaviors={behaviors}
        beliefs={userBeliefs}
      />
    </div>
  );
};

export default WorldPage;
