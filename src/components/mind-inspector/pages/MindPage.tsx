/**
 * Mind 页 — 认知调试器（Cognitive Debugger）
 *
 * 心智观察器最核心的页面：Live Mind 实时心智快照
 * （Twin View，Vivian + Nana 并排，5 秒轮询）
 *
 * 数据源：
 * - get_mind_state / get_current_mood
 */

import React, { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { LucideIcon } from 'lucide-react';
import {
  Smile,
  Frown,
  Angry,
  ShieldAlert,
  Heart,
  CloudRain,
  Sparkles,
  Meh,
  ChevronDown,
  ChevronUp,
  Target,
  Home,
  Unlock,
  Shield,
  Star,
  MessageCircle,
  Crown,
  Users,
  AlertCircle,
  ShieldCheck,
} from 'lucide-react';
import {
  COLORS,
  TYPO,
  SPACING,
  RADIUS,
  EASE,
  DURATION,
  CHARACTER_ACCENT,
} from '../design-system';
import { useNavigation } from '../NavigationContext';
import {
  Card,
  MetricBar,
  EmptyState,
  SectionTitle,
  TwinView,
} from '../shared-components';
import type { MindState } from '../../../types';

// === 区块渐次入场动画（尊重系统减弱动效设置，配合 .mi-rise 类使用） ===
const RISE_KEYFRAMES_ID = 'mind-inspector-rise';
if (typeof document !== 'undefined' && !document.getElementById(RISE_KEYFRAMES_ID)) {
  const style = document.createElement('style');
  style.id = RISE_KEYFRAMES_ID;
  style.textContent = `
@keyframes mind-inspector-rise {
  from { opacity: 0; transform: translateY(10px); }
  to { opacity: 1; transform: translateY(0); }
}
.mi-rise {
  animation: mind-inspector-rise 0.5s cubic-bezier(0.1, 0.9, 0.2, 1) both;
}
@media (prefers-reduced-motion: reduce) {
  .mi-rise { animation: none !important; }
}`;
  document.head.appendChild(style);
}

// ============================================================
// 类型定义
// ============================================================

/** 心情数据（get_current_mood 返回的扁平字段子集） */
interface MoodData {
  valence: number;
  arousal: number;
  primary_emotion: string;
  mood_label?: string;
  emotion_label?: string;
  primary_intensity?: number;
  // 扩展字段（后端注入的兼容字段）
  energy?: number;
  focus?: number;
  intimacy?: number;
  trust?: number;
  positive_affect?: number;
  negative_affect?: number;
  mood_score?: number;
  mood_emotion?: string;
  mood_secondary?: string;
  emotion_key?: string;
  fatigue?: number;
  stress?: number;
  relationship_score?: number;
}

/** 心理学全维度快照（get_psychology_state 返回） */
interface PsychologySnapshot {
  emotion: {
    joy: number;
    sadness: number;
    anger: number;
    fear: number;
    closeness: number;
    loneliness: number;
    curiosity: number;
  };
  needs: {
    belonging: number;
    autonomy: number;
    security: number;
    novelty: number;
    expression: number;
  };
  relationship: {
    trust: number;
    intimacy: number;
    respect: number;
    dependency: number;
    familiarity: number;
  };
}

// ============================================================
// 常量
// ============================================================

type CharacterId = 'vivian' | 'nana';

/** 心理指标 → Lucide 图标组件映射（替代旧版 emoji） */
const METRIC_ICON: Record<string, LucideIcon> = {
  // 情绪
  joy: Smile,
  happiness: Smile,
  sadness: Frown,
  anger: Angry,
  fear: ShieldAlert,
  anxiety: ShieldAlert,
  affection: Heart,
  love: Heart,
  closeness: Heart,
  loneliness: CloudRain,
  neutral: Meh,
  curiosity: Sparkles,
  surprise: Sparkles,
  // 需求
  belonging: Home,
  autonomy: Unlock,
  security: Shield,
  novelty: Star,
  expression: MessageCircle,
  // 关系
  trust: ShieldCheck,
  intimacy: Heart,
  respect: Crown,
  dependency: Heart,
  familiarity: Users,
};

const metricIcon = (key: string, size = 14, color?: string): React.ReactElement => {
  const Icon = METRIC_ICON[(key || '').toLowerCase()] ?? Sparkles;
  return <Icon size={size} color={color ?? COLORS.textTertiary} strokeWidth={1.5} />;
};

// ============================================================
// 工具函数
// ============================================================

const THOUGHT_TEXT_KEYS = ['content', 'thinking', 'thought', 'text', 'monologue', 'reply', 'answer'];

const extractThoughtText = (raw: string): string => {
  if (!raw) return '';
  const trimmed = raw.trim();
  if (!trimmed.startsWith('{')) return trimmed;
  try {
    const obj = JSON.parse(trimmed);
    for (const key of THOUGHT_TEXT_KEYS) {
      const val = obj[key];
      if (typeof val === 'string' && val.trim()) {
        return val.trim();
      }
    }
  } catch {
    // 不是合法 JSON，原样返回
  }
  return trimmed;
};

// ============================================================
// 子视图 1：Live Mind（实时心智快照，Twin View，5 秒轮询）
// ============================================================

/** 根据 valence/arousal 返回当前所处象限的标签 */
const quadrantLabel = (valence: number, arousal: number, t: TFunction): string => {
  if (valence >= 0 && arousal >= 0) return t('mind_inspector.mind.quad_excited_short');
  if (valence < 0 && arousal >= 0) return t('mind_inspector.mind.quad_tense_short');
  if (valence >= 0 && arousal < 0) return t('mind_inspector.mind.quad_calm_short');
  return t('mind_inspector.mind.quad_depressed_short');
};

interface CharacterMindPanelProps {
  characterId: CharacterId;
  mind: MindState | null;
  mood: MoodData | null;
  psychState: PsychologySnapshot | null;
  loading: boolean;
  error: string | null;
  presenceState: string;
  /** 进入当前在场状态的时间戳（Unix 秒） */
  presenceSince: number;
}

// 大尺寸象限图（模块级 + memo，防止父组件重渲染时卸载重建导致坐标点闪烁）
const LargeMoodQuadrant = React.memo<{ valence: number; arousal: number; accent: string; t: TFunction }>(
  ({ valence, arousal, accent, t }) => {
    const x = ((valence + 1) / 2) * 100;
    const y = (1 - arousal) * 100;

    return (
      <div style={{ position: 'relative', width: '100%', height: '100%' }}>
        {/* 微妙网格 */}
        <div
          style={{
            position: 'absolute',
            inset: 0,
            backgroundImage: `
              linear-gradient(${COLORS.gridLine} 1px, transparent 1px),
              linear-gradient(90deg, ${COLORS.gridLine} 1px, transparent 1px)
            `,
            backgroundSize: '32px 32px',
          }}
        />

        {/* 十字线 */}
        <div style={{ position: 'absolute', top: '50%', left: '10%', right: '10%', height: 1, background: COLORS.axisLine }} />
        <div style={{ position: 'absolute', left: '50%', top: '10%', bottom: '10%', width: 1, background: COLORS.axisLine }} />

        {/* 四象限标签 */}
        <span style={{ position: 'absolute', top: 12, left: 12, ...TYPO.micro, color: COLORS.textQuaternary }}>{t('mind_inspector.mind.quad_tense_short')}</span>
        <span style={{ position: 'absolute', top: 12, right: 12, ...TYPO.micro, color: COLORS.textQuaternary }}>{t('mind_inspector.mind.quad_excited_short')}</span>
        <span style={{ position: 'absolute', bottom: 12, left: 12, ...TYPO.micro, color: COLORS.textQuaternary }}>{t('mind_inspector.mind.quad_depressed_short')}</span>
        <span style={{ position: 'absolute', bottom: 12, right: 12, ...TYPO.micro, color: COLORS.textQuaternary }}>{t('mind_inspector.mind.quad_calm_short')}</span>

        {/* 位置点 */}
        <div
          style={{
            position: 'absolute',
            left: `${x}%`,
            top: `${y}%`,
            transform: 'translate(-50%, -50%)',
            width: 11,
            height: 11,
            borderRadius: RADIUS.pill,
            background: accent,
            boxShadow: `0 0 8px ${accent}80, 0 2px 6px rgba(0,0,0,0.15)`,
            zIndex: 2,
          }}
        >
          <div
            style={{
              position: 'absolute',
              inset: -6,
              borderRadius: RADIUS.pill,
              background: accent,
              opacity: 0.25,
              animation: `mind-inspector-pulse ${DURATION.slow * 3}s ${EASE.swift} infinite`,
            }}
          />
          <div style={{ position: 'absolute', inset: 2, borderRadius: RADIUS.pill, background: '#fff' }} />
        </div>
      </div>
    );
  },
);
LargeMoodQuadrant.displayName = 'LargeMoodQuadrant';

// 涟漪层（独立组件，setRipples 只触发本组件重渲染，不影响象限图）
const QuadrantRippleLayer = React.memo<{
  ripples: Array<{ id: number; x: number; y: number }>;
  accent: string;
}>(({ ripples, accent }) => (
  <>
    {ripples.map((r) => (
      <div
        key={r.id}
        style={{
          position: 'absolute',
          left: r.x,
          top: r.y,
          width: 220,
          height: 220,
          borderRadius: '50%',
          background: `radial-gradient(circle, ${accent}25 0%, ${accent}12 20%, ${accent}06 45%, transparent 70%)`,
          pointerEvents: 'none',
          animation: 'mind-inspector-ripple 1.1s cubic-bezier(0.2, 0.8, 0.2, 1) forwards',
          zIndex: 10,
        }}
      />
    ))}
  </>
));
QuadrantRippleLayer.displayName = 'QuadrantRippleLayer';

const CharacterMindPanel: React.FC<CharacterMindPanelProps> = ({
  characterId,
  mind,
  mood,
  psychState,
  loading,
  error,
  presenceState,
  presenceSince,
}) => {
  const { t } = useTranslation();
  const nav = useNavigation();
  const accent = CHARACTER_ACCENT[characterId];
  const label = t(`mind_inspector.common.char_${characterId}`);
  const emotionKey = mood?.primary_emotion ?? '';
  const emotionLabel = t(`mind_inspector.mind.em_${emotionKey}`, {
    defaultValue: mood?.mood_label ?? mood?.emotion_label ?? emotionKey,
  });
  const [moodExpanded, setMoodExpanded] = useState(false);
  const [quadrantOpen, setQuadrantOpen] = useState(false);
  const [quadrantClosing, setQuadrantClosing] = useState(false);
  const [ripples, setRipples] = useState<Array<{ id: number; x: number; y: number }>>([]);
  const lastRippleTs = useRef(0);

  // 每秒滴答，用于刷新在场状态持续时长显示
  const [, setNowTick] = useState(0);
  useEffect(() => {
    const id = window.setInterval(() => setNowTick((n) => n + 1), 1000);
    return () => window.clearInterval(id);
  }, []);

  const closeQuadrant = () => {
    setQuadrantClosing(true);
  };

  const onQuadrantCloseEnd = () => {
    setQuadrantClosing(false);
    setQuadrantOpen(false);
  };

  const goals = mind?.goals ?? [];
  const activeGoal = goals.find((g) => g.active) ?? goals[0];
  // 内心独白/当前想法开关关闭时不显示想法内容（由后端清空缓存，此处防御性兜底）
  const thoughtEnabled = mind?.inner_monologue_enabled !== false;
  const thought = thoughtEnabled ? extractThoughtText(mind?.current_thought ?? '') : '';
  const valence = mood?.valence ?? 0;
  const arousal = mood?.arousal ?? 0;

  // 在场状态 → 显示文本与颜色
  const presenceDisplay = (() => {
    const normalized = (presenceState || 'online').toLowerCase();
    if (normalized === 'busy') return { labelKey: 'chat.status_busy', color: COLORS.danger, pulse: false };
    if (normalized === 'rest') return { labelKey: 'chat.status_rest', color: COLORS.warning, pulse: false };
    if (normalized === 'offline') return { labelKey: 'chat.status_offline', color: COLORS.textTertiary, pulse: false };
    return { labelKey: 'chat.status_online', color: COLORS.success, pulse: true };
  })();

  // 当前状态已持续时长（秒）
  const presenceElapsedSecs = Math.max(0, Math.floor(Date.now() / 1000 - (presenceSince || Date.now() / 1000)));
  const presenceDurationLabel = (() => {
    const secs = presenceElapsedSecs;
    if (secs < 60) return t('mind_inspector.mind.dur_sec', { s: secs });
    if (secs < 3600) return t('mind_inspector.mind.dur_min', { m: Math.floor(secs / 60), s: secs % 60 });
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    return t('mind_inspector.mind.dur_hr', { h, m });
  })();

  // 心理学全维度条形图行
  const MetricRow: React.FC<{ label: string; value: number; color?: string }> = ({ label: lbl, value, color: c }) => (
    <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.sm }}>
      <span style={{ ...TYPO.micro, color: COLORS.textTertiary, minWidth: 72 }}>{lbl}</span>
      <MetricBar value={value} color={c ?? accent} style={{ flex: 1 }} />
      <span
        style={{
          ...TYPO.micro,
          color: COLORS.textSecondary,
          minWidth: 36,
          textAlign: 'right',
          fontVariantNumeric: 'tabular-nums',
        }}
      >
        {value.toFixed(2)}
      </span>
    </div>
  );

  // 心情象限图（Apple 风格：渐变暗底 + 网格 + screen 混合光晕）—— 放大到 120px
  const MoodQuadrant: React.FC<{ valence: number; arousal: number; accent: string }> = ({
    valence,
    arousal,
    accent,
  }) => {
    const x = ((valence + 1) / 2) * 100;
    const y = (1 - arousal) * 100;

    return (
      <div
        style={{
          position: 'relative',
          borderRadius: RADIUS.md,
          background:
            `radial-gradient(circle at 50% 50%, ${COLORS.bgSurface} 0%, ${COLORS.bgSurfaceElevated} 100%)`,
          aspectRatio: '1',
          overflow: 'hidden',
          border: `1px solid ${COLORS.subtleBorder}`,
        }}
      >
        {/* 微妙网格 */}
        <div
          style={{
            position: 'absolute',
            inset: 0,
            backgroundImage: `
              linear-gradient(${COLORS.gridLine} 1px, transparent 1px),
              linear-gradient(90deg, ${COLORS.gridLine} 1px, transparent 1px)
            `,
            backgroundSize: '20px 20px',
          }}
        />

        {/* 十字线 */}
        <div
          style={{
            position: 'absolute',
            top: '50%',
            left: '12%',
            right: '12%',
            height: 1,
            background: COLORS.axisLine,
            transform: 'translateY(-50%)',
          }}
        />
        <div
          style={{
            position: 'absolute',
            left: '50%',
            top: '12%',
            bottom: '12%',
            width: 1,
            background: COLORS.axisLine,
            transform: 'translateX(-50%)',
          }}
        />

        {/* 位置点（带呼吸光晕） */}
        <div
          style={{
            position: 'absolute',
            left: `${x}%`,
            top: `${y}%`,
            transform: 'translate(-50%, -50%)',
            width: 10,
            height: 10,
            borderRadius: RADIUS.pill,
            background: accent,
            boxShadow: `0 0 6px ${accent}80, 0 2px 4px rgba(0,0,0,0.12)`,
            transition: `all ${DURATION.slow}s ${EASE.ios}`,
            zIndex: 2,
          }}
        >
          <div
            style={{
              position: 'absolute',
              inset: -5,
              borderRadius: RADIUS.pill,
              background: accent,
              opacity: 0.3,
              animation: `mind-inspector-pulse ${DURATION.slow * 3}s ${EASE.swift} infinite`,
            }}
          />
          <div
            style={{
              position: 'absolute',
              inset: 2,
              borderRadius: RADIUS.pill,
              background: '#fff',
            }}
          />
        </div>
      </div>
    );
  };

  // 情绪指标（渐变条 + 发光端点 + 胶囊百分比）
  const MoodMetric: React.FC<{ label: string; value: number; iconKey: string }> = ({
    label,
    value,
    iconKey,
  }) => {
    const pct = Math.round(value * 100);
    return (
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '3px 0' }}>
        {/* 图标 + 柔和背景圆 */}
        <span
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            width: 18,
            height: 18,
            borderRadius: RADIUS.pill,
            background: `${accent}12`,
            flexShrink: 0,
          }}
        >
          {metricIcon(iconKey, 10, accent)}
        </span>
        <span
          style={{
            ...TYPO.caption,
            color: COLORS.textSecondary,
            minWidth: 40,
            flexShrink: 0,
            whiteSpace: 'nowrap',
          }}
        >
          {label}
        </span>
        <div
          style={{
            flex: 1,
            height: 4,
            borderRadius: RADIUS.pill,
            background: COLORS.bgHover,
            overflow: 'visible',
            position: 'relative',
          }}
        >
          <div
            style={{
              height: '100%',
              width: `${pct}%`,
              borderRadius: RADIUS.pill,
              background: `linear-gradient(90deg, ${accent}90, ${accent})`,
              boxShadow: pct > 5 ? `0 0 6px ${accent}40` : 'none',
              transition: `width ${DURATION.slow}s ${EASE.ios}`,
              position: 'relative',
            }}
          >
            {/* 前端发光点 */}
            {pct > 3 && (
              <div
                style={{
                  position: 'absolute',
                  right: -2,
                  top: '50%',
                  transform: 'translateY(-50%)',
                  width: 7,
                  height: 7,
                  borderRadius: RADIUS.pill,
                  background: '#fff',
                  boxShadow: `0 0 5px ${accent}, 0 0 10px ${accent}60`,
                }}
              />
            )}
          </div>
        </div>
        {/* 胶囊百分比 */}
        <span
          style={{
            ...TYPO.micro,
            color: COLORS.textTertiary,
            background: COLORS.bgSurface,
            borderRadius: RADIUS.xs,
            padding: '1px 5px',
            fontVariantNumeric: 'tabular-nums',
            minWidth: 30,
            textAlign: 'center',
            flexShrink: 0,
          }}
        >
          {pct}%
        </span>
      </div>
    );
  };

  // 排序动画列表（FLIP 技术：值变化时平滑交换位置）
  const AnimatedMetricList: React.FC<{
    items: Array<{ label: string; value: number; iconKey: string }>;
  }> = ({ items }) => {
    const containerRef = useRef<HTMLDivElement>(null);
    const [order, setOrder] = useState<string[]>(() =>
      [...items].sort((a, b) => a.value - b.value).map((i) => i.label),
    );

    // 上一轮 FLIP 的位置快照（由 useLayoutEffect 写入，下一次 useLayoutEffect cleanup 读取）
    const flipLastPosRef = useRef<Record<string, number>>({});
    const flipLastOrderRef = useRef<string[]>([]);
    const flipElMapRef = useRef<Map<string, HTMLElement>>(new Map());
    const flipRafRef = useRef<number>(0);

    // 数据变化时重新排序
    useEffect(() => {
      const newOrder = [...items].sort((a, b) => a.value - b.value).map((i) => i.label);
      setOrder((prev) => {
        if (prev.length !== newOrder.length) return newOrder;
        for (let k = 0; k < prev.length; k++) {
          if (prev[k] !== newOrder[k]) return newOrder;
        }
        return prev;
      });
    }, [items]);

    // FLIP 第一阶段：布局后清除旧 transform → 记录自然位置
    useLayoutEffect(() => {
      const elMap = flipElMapRef.current;
      const prevOrder = flipLastOrderRef.current;

      // cleanup：用上一轮 FLIP 的顺序记录当前位置（可能仍含旧 FLIP transform）
      if (prevOrder.length) {
        const pos: Record<string, number> = {};
        for (const key of prevOrder) {
          const el = elMap.get(key);
          if (el) pos[key] = el.getBoundingClientRect().top;
        }
        flipLastPosRef.current = pos;
      }

      // 清除所有 transform（布局已强制计算）
      for (const el of elMap.values()) {
        el.style.transition = 'none';
        el.style.transform = '';
      }

      // 记录新的自然位置
      const newPos: Record<string, number> = {};
      for (const key of order) {
        const el = elMap.get(key);
        if (el) newPos[key] = el.getBoundingClientRect().top;
      }
      flipLastPosRef.current = newPos;
      flipLastOrderRef.current = [...order];

      return () => {
        cancelAnimationFrame(flipRafRef.current);
      };
    });

    // FLIP 第二阶段：paint 后施加反向 transform → 下一帧过渡到 0
    useEffect(() => {
      const lastPos = flipLastPosRef.current;
      const elMap = flipElMapRef.current;

      for (const key of order) {
        const el = elMap.get(key);
        if (!el) continue;
        const oldY = lastPos[key];
        if (oldY === undefined) continue;
        const newY = el.getBoundingClientRect().top;
        const delta = oldY - newY;
        if (Math.abs(delta) < 1) continue;
        el.style.transform = `translateY(${delta}px)`;
      }

      flipRafRef.current = requestAnimationFrame(() => {
        for (const key of order) {
          const el = elMap.get(key);
          if (!el) continue;
          el.style.transition = `transform ${DURATION.slow}s ${EASE.ios}`;
          el.style.transform = '';
        }
      });
    }, [order]);

    const refCb = (label: string) => (el: HTMLDivElement | null) => {
      if (el) flipElMapRef.current.set(label, el);
      else flipElMapRef.current.delete(label);
    };

    return (
      <div ref={containerRef} style={{ display: 'flex', flexDirection: 'column' }}>
        {order.map((label) => {
          const item = items.find((i) => i.label === label);
          if (!item) return null;
          return (
            <div key={label} ref={refCb(label)}>
              <MoodMetric label={item.label} value={item.value} iconKey={item.iconKey} />
            </div>
          );
        })}
      </div>
    );
  };

  // 心情默认展示 3 个核心情绪，展开后显示全部
  const topEmotions = psychState
    ? [
        { label: t('mind_inspector.mind.em_joy'), value: psychState.emotion.joy, iconKey: 'joy' },
        { label: t('mind_inspector.mind.em_curiosity'), value: psychState.emotion.curiosity, iconKey: 'curiosity' },
        { label: t('mind_inspector.mind.em_closeness'), value: psychState.emotion.closeness, iconKey: 'closeness' },
      ]
    : [];

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: SPACING.xl }}>
      {/* === Header：角色名 + Avatar + Thinking 状态 === */}
      <div style={{ paddingTop: SPACING.sm }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.sm }}>
          {/* Avatar */}
          <div
            style={{
              width: 44,
              height: 44,
              borderRadius: RADIUS.md,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              overflow: 'hidden',
              background: `${accent}12`,
              border: `1px solid ${accent}25`,
              boxShadow: `0 0 12px ${accent}20`,
            }}
          >
            <img
              src={characterId === 'vivian' ? '/expression/gigi.webp' : '/expression/happy.webp'}
              alt={label}
              style={{ width: '100%', height: '100%', objectFit: 'cover' }}
            />
          </div>
          <div>
            <div style={{ display: 'flex', alignItems: 'baseline', gap: SPACING.sm }}>
              <span style={{ ...TYPO.h2, color: COLORS.textPrimary, fontSize: 17 }}>{label}</span>
            </div>
            {emotionLabel && (
              <div style={{ ...TYPO.micro, color: COLORS.textTertiary, marginTop: 2 }}>
                {emotionLabel}
              </div>
            )}
          </div>
        </div>
      </div>

      {loading && !mind ? (
        <EmptyState spinner text={t('mind_inspector.mind.loading_state')} />
      ) : error && !mind ? (
        <EmptyState icon={<AlertCircle size={24} color={COLORS.textTertiary} strokeWidth={1.5} />} text={t('mind_inspector.common.load_failed', { error })} />
      ) : (
        <>
          {/* === Thought（视觉中心，引述风格） === */}
          <div className="mi-rise">
            <Card hover>
            <div style={{ ...TYPO.caption, color: COLORS.textTertiary, marginBottom: SPACING.sm }}>
              {t('mind_inspector.mind.current_thought')}
            </div>
            <div
              style={{
                position: 'relative',
                paddingLeft: SPACING.md,
                borderLeft: `2px solid ${accent}50`,
              }}
            >
              <div
                style={{
                  fontSize: 14,
                  lineHeight: 1.6,
                  color: thought ? COLORS.textPrimary : COLORS.textTertiary,
                  fontWeight: 500,
                  fontStyle: thought ? 'normal' : 'italic',
                }}
              >
                {thought || t('mind_inspector.mind.thought_empty')}
              </div>
            </div>
          </Card>
          </div>

          {/* === Mood（大尺寸象限图 + 细条形情绪） === */}
          <div className="mi-rise" style={{ animationDelay: '60ms' }}>
            <Card hover>
            <div
              role="button"
              onClick={() => setMoodExpanded((v) => !v)}
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                marginBottom: SPACING.md,
                cursor: 'pointer',
              }}
            >
              <span style={{ ...TYPO.caption, color: COLORS.textTertiary }}>
                {t('mind_inspector.mind.mood_metrics')}
              </span>
              {moodExpanded ? (
                <ChevronUp size={14} color={COLORS.textTertiary} />
              ) : (
                <ChevronDown size={14} color={COLORS.textTertiary} />
              )}
            </div>

            <div
              role="button"
              onClick={() => setMoodExpanded((v) => !v)}
              style={{ display: 'flex', gap: SPACING.xl, alignItems: 'flex-start', cursor: 'pointer' }}
            >
              {/* 象限图（点击呼出大图，阻止冒泡以避免触发展开/收起） */}
              <div
                onClick={(e) => {
                  e.stopPropagation();
                  setQuadrantOpen(true);
                }}
                style={{ width: 92, height: 92, flexShrink: 0, cursor: 'pointer', position: 'relative' }}
                title={t('mind_inspector.mind.click_to_enlarge')}
              >
                <MoodQuadrant valence={valence} arousal={arousal} accent={accent} />
              </div>

              {/* 细条形情绪指标 */}
              <div style={{ flex: 1, minWidth: 0 }}>
                {topEmotions.length > 0 ? (
                  topEmotions.map((em) => (
                    <MoodMetric key={em.label} label={em.label} value={em.value} iconKey={em.iconKey} />
                  ))
                ) : (
                  <EmptyState
                    text={t('mind_inspector.mind.no_psychology_data')}
                    style={{ padding: `${SPACING.sm}px`, ...TYPO.micro } as React.CSSProperties}
                  />
                )}
              </div>
            </div>

            {/* 展开后显示全维度 */}
            {moodExpanded && psychState && (
              <div style={{ marginTop: SPACING.xl, display: 'flex', flexDirection: 'column', gap: SPACING.md }}>
                {/* 情绪维度 */}
                <div
                  style={{
                    padding: '10px 12px',
                    borderRadius: RADIUS.md,
                    background: COLORS.subtleBg,
                    border: `1px solid ${COLORS.subtleBorder}`,
                  }}
                >
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 6,
                      marginBottom: SPACING.sm,
                    }}
                  >
                    <div style={{ width: 2, height: 11, borderRadius: RADIUS.xs, background: accent }} />
                    <Sparkles size={11} color={accent} strokeWidth={1.8} />
                    <SectionTitle style={{ color: accent }}>
                      {t('mind_inspector.mind.emotion_dims')}
                    </SectionTitle>
                  </div>
                  <AnimatedMetricList
                    items={[
                      { label: t('mind_inspector.mind.em_joy'), value: psychState.emotion.joy, iconKey: 'joy' },
                      { label: t('mind_inspector.mind.em_sadness'), value: psychState.emotion.sadness, iconKey: 'sadness' },
                      { label: t('mind_inspector.mind.em_anger'), value: psychState.emotion.anger, iconKey: 'anger' },
                      { label: t('mind_inspector.mind.em_fear'), value: psychState.emotion.fear, iconKey: 'fear' },
                      { label: t('mind_inspector.mind.em_closeness'), value: psychState.emotion.closeness, iconKey: 'closeness' },
                      { label: t('mind_inspector.mind.em_loneliness'), value: psychState.emotion.loneliness, iconKey: 'loneliness' },
                      { label: t('mind_inspector.mind.em_curiosity'), value: psychState.emotion.curiosity, iconKey: 'curiosity' },
                    ]}
                  />
                </div>

                {/* 需求维度 */}
                <div
                  style={{
                    padding: '10px 12px',
                    borderRadius: RADIUS.md,
                    background: COLORS.subtleBg,
                    border: `1px solid ${COLORS.subtleBorder}`,
                  }}
                >
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 6,
                      marginBottom: SPACING.sm,
                    }}
                  >
                    <div style={{ width: 2, height: 11, borderRadius: RADIUS.xs, background: COLORS.info }} />
                    <Target size={11} color={COLORS.info} strokeWidth={1.8} />
                    <SectionTitle style={{ color: COLORS.info }}>
                      {t('mind_inspector.mind.need_dims')}
                    </SectionTitle>
                  </div>
                  <AnimatedMetricList
                    items={[
                      { label: t('mind_inspector.mind.need_belonging'), value: psychState.needs.belonging, iconKey: 'belonging' },
                      { label: t('mind_inspector.mind.need_autonomy'), value: psychState.needs.autonomy, iconKey: 'autonomy' },
                      { label: t('mind_inspector.mind.need_security'), value: psychState.needs.security, iconKey: 'security' },
                      { label: t('mind_inspector.mind.need_novelty'), value: psychState.needs.novelty, iconKey: 'novelty' },
                      { label: t('mind_inspector.mind.need_expression'), value: psychState.needs.expression, iconKey: 'expression' },
                    ]}
                  />
                </div>

                {/* 关系维度 */}
                <div
                  style={{
                    padding: '10px 12px',
                    borderRadius: RADIUS.md,
                    background: COLORS.subtleBg,
                    border: `1px solid ${COLORS.subtleBorder}`,
                  }}
                >
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 6,
                      marginBottom: SPACING.sm,
                    }}
                  >
                    <div style={{ width: 2, height: 11, borderRadius: RADIUS.xs, background: COLORS.success }} />
                    <Users size={11} color={COLORS.success} strokeWidth={1.8} />
                    <SectionTitle style={{ color: COLORS.success }}>
                      {t('mind_inspector.mind.relationship_dims')}
                    </SectionTitle>
                  </div>
                  <AnimatedMetricList
                    items={[
                      { label: t('mind_inspector.mind.rel_trust'), value: psychState.relationship.trust, iconKey: 'trust' },
                      { label: t('mind_inspector.mind.rel_intimacy'), value: psychState.relationship.intimacy, iconKey: 'intimacy' },
                      { label: t('mind_inspector.mind.rel_respect'), value: psychState.relationship.respect, iconKey: 'respect' },
                      { label: t('mind_inspector.mind.rel_dependency'), value: psychState.relationship.dependency, iconKey: 'dependency' },
                      { label: t('mind_inspector.mind.rel_familiarity'), value: psychState.relationship.familiarity, iconKey: 'familiarity' },
                    ]}
                  />
                </div>
              </div>
            )}
            </Card>
          </div>

         {/* === Goal（Apple Reminder 风格） === */}
          <div className="mi-rise" style={{ animationDelay: '120ms' }}>
            <Card hover>
            <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.sm, marginBottom: SPACING.sm }}>
              <Target size={13} color={COLORS.textTertiary} strokeWidth={1.5} />
              <span style={{ ...TYPO.caption, color: COLORS.textTertiary }}>
                {t('mind_inspector.mind.current_goal')}
              </span>
            </div>
            <div
              style={{
                fontSize: 14,
                lineHeight: 1.55,
                color: activeGoal ? COLORS.textPrimary : COLORS.textTertiary,
                fontStyle: activeGoal ? 'normal' : 'italic',
              }}
            >
              {activeGoal?.description || t('mind_inspector.mind.no_active_goal')}
            </div>
            </Card>
          </div>

          {/* === Status（圆环状态指示器） === */}
          <div className="mi-rise" style={{ animationDelay: '180ms' }}>
            <Card hover>
            <div style={{ ...TYPO.caption, color: COLORS.textTertiary, marginBottom: SPACING.sm }}>
              {t('mind_inspector.mind.online_status')}
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.sm }}>
              {/* 圆环状态 */}
              <div style={{ position: 'relative', width: 20, height: 20, flexShrink: 0 }}>
                <svg width={20} height={20} viewBox="0 0 24 24">
                  <circle
                    cx={12}
                    cy={12}
                    r={10}
                    fill="none"
                    stroke={COLORS.border}
                    strokeWidth={2}
                  />
                  <circle
                    cx={12}
                    cy={12}
                    r={10}
                    fill="none"
                    stroke={presenceDisplay.color}
                    strokeWidth={2}
                    strokeLinecap="round"
                    strokeDasharray={62.8}
                    strokeDashoffset={0}
                    transform="rotate(-90 12 12)"
                    style={{
                      transition: `stroke ${DURATION.slow}s ${EASE.ios}`,
                    }}
                  />
                  <circle
                    cx={12}
                    cy={12}
                    r={3}
                    fill={presenceDisplay.color}
                  />
                </svg>
              </div>
              <div>
                <div style={{ ...TYPO.caption, color: COLORS.textSecondary }}>
                  {t(presenceDisplay.labelKey)}
                </div>
                <div
                  style={{
                    ...TYPO.micro,
                    color: COLORS.textQuaternary,
                    fontVariantNumeric: 'tabular-nums',
                  }}
                >
                  {t('mind_inspector.cognition.since_label', { duration: presenceDurationLabel })}
                </div>
              </div>
            </div>
            </Card>
          </div>
        </>
      )}

      {/* === Mood Quadrant 大图 Modal（点击呼出，外部点击关闭） === */}
      {(quadrantOpen || quadrantClosing) && (
        <div
          onClick={closeQuadrant}
          onAnimationEnd={(e) => {
            if (e.animationName === 'mind-inspector-fade-out') {
              onQuadrantCloseEnd();
            }
          }}
          style={{
            position: 'fixed',
            inset: 0,
            zIndex: 9999,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            background: 'var(--panel-overlay)',
            animation: quadrantClosing ? 'mind-inspector-fade-out 0.2s ease-in' : 'mind-inspector-fade-in 0.25s ease-out',
            opacity: quadrantClosing ? 0 : 1,
          }}
        >
          <div
            onClick={(e) => e.stopPropagation()}
            onAnimationEnd={(e) => e.stopPropagation()}
            onMouseMove={(e) => {
              const now = performance.now();
              if (now - lastRippleTs.current < 80) return;
              lastRippleTs.current = now;
              const rect = e.currentTarget.getBoundingClientRect();
              const x = e.clientX - rect.left;
              const y = e.clientY - rect.top;
              const id = Date.now() + Math.random();
              setRipples((prev) => [...prev, { id, x, y }]);
              setTimeout(() => {
                setRipples((prev) => prev.filter((r) => r.id !== id));
              }, 900);
            }}
            style={{
              width: 260,
              height: 260,
              position: 'relative',
              overflow: 'hidden',
              borderRadius: RADIUS.lg,
              background: 'var(--panel-surface)',
              border: '1.5px solid var(--panel-border)',
              boxShadow: 'var(--panel-shadow-elevated)',
              animation: quadrantClosing
                ? 'mind-inspector-scale-out 0.2s ease-in forwards'
                : 'mind-inspector-scale-in 0.3s cubic-bezier(0.34, 1.56, 0.64, 1)',
            }}
          >
            {/* 渐变背景（中心亮，向外逐渐透明） */}
            <div
              style={{
                position: 'absolute',
                inset: -60,
                background: `radial-gradient(circle at 50% 50%, ${accent}15 0%, ${accent}08 30%, transparent 70%)`,
                borderRadius: RADIUS.pill,
                pointerEvents: 'none',
              }}
            />

            {/* 象限图本体 */}
            <div style={{ width: '100%', height: '100%', position: 'relative' }}>
              {/* 四象限背景色块 */}
              <div style={{ position: 'absolute', inset: 0, display: 'grid', gridTemplateColumns: '1fr 1fr', gridTemplateRows: '1fr 1fr', borderRadius: RADIUS.lg, overflow: 'hidden' }}>
                <div style={{ background: 'rgba(255,100,100,0.06)' }} />
                <div style={{ background: 'rgba(255,200,100,0.06)' }} />
                <div style={{ background: 'rgba(100,100,255,0.06)' }} />
                <div style={{ background: 'rgba(100,255,150,0.06)' }} />
              </div>

              {/* 象限图内容（放大版） */}
              <div style={{ position: 'absolute', inset: 0 }}>
                <LargeMoodQuadrant valence={valence} arousal={arousal} accent={accent} t={t} />
              </div>
            </div>

            {/* 水波纹层（独立组件，不触发象限图重渲染） */}
            <QuadrantRippleLayer ripples={ripples} accent={accent} />
          </div>
        </div>
      )}
    </div>
  );
};

const MemoCharacterMindPanel = React.memo(CharacterMindPanel);

const LiveMindView: React.FC = () => {
  const { t } = useTranslation();
  const [vivianMind, setVivianMind] = useState<MindState | null>(null);
  const [nanaMind, setNanaMind] = useState<MindState | null>(null);
  const [vivianMood, setVivianMood] = useState<MoodData | null>(null);
  const [nanaMood, setNanaMood] = useState<MoodData | null>(null);
  const [vivianPsych, setVivianPsych] = useState<PsychologySnapshot | null>(null);
  const [nanaPsych, setNanaPsych] = useState<PsychologySnapshot | null>(null);
  const [loadingV, setLoadingV] = useState(true);
  const [loadingN, setLoadingN] = useState(true);
  const [errorV, setErrorV] = useState<string | null>(null);
  const [errorN, setErrorN] = useState<string | null>(null);
  // 各角色在场状态（presence）：online / busy / rest / offline，与 ChatWindow 数据源保持一致
  // 同时记录进入当前状态的 Unix 秒时间戳，用于心智页持续时长显示
  const [presenceStates, setPresenceStates] = useState<Record<string, { state: string; since: number }>>({});

  useEffect(() => {
    let cancelled = false;
    const fetchVivian = async () => {
      try {
        const [mind, mood, psych] = await Promise.all([
          invoke<MindState>('get_mind_state', { characterId: 'vivian' }),
          invoke<MoodData>('get_current_mood', { characterId: 'vivian' }),
          invoke<PsychologySnapshot>('get_psychology_state', { characterId: 'vivian' }),
        ]);
        if (!cancelled) {
          setVivianMind(mind);
          setVivianMood(mood);
          setVivianPsych(psych);
          setErrorV(null);
        }
      } catch (e) {
        if (!cancelled) setErrorV(String(e));
      } finally {
        if (!cancelled) setLoadingV(false);
      }
    };
    const fetchNana = async () => {
      try {
        const [mind, mood, psych] = await Promise.all([
          invoke<MindState>('get_mind_state', { characterId: 'nana' }),
          invoke<MoodData>('get_current_mood', { characterId: 'nana' }),
          invoke<PsychologySnapshot>('get_psychology_state', { characterId: 'nana' }),
        ]);
        if (!cancelled) {
          setNanaMind(mind);
          setNanaMood(mood);
          setNanaPsych(psych);
          setErrorN(null);
        }
      } catch (e) {
        if (!cancelled) setErrorN(String(e));
      } finally {
        if (!cancelled) setLoadingN(false);
      }
    };
    void fetchVivian();
    void fetchNana();
    const id = window.setInterval(() => {
      void fetchVivian();
      void fetchNana();
    }, 5_000);
    return () => {
      cancelled = true;
      window.clearInterval(id);
    };
  }, []);

  // 拉取在场状态 + 监听 presence:changed 事件，保持与微信面板显示一致
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void (async () => {
      try {
        const states = await invoke<Array<{ character_id: string; state: string; since: number }>>('get_all_presence_states');
        if (cancelled) return;
        const map: Record<string, { state: string; since: number }> = {};
        for (const s of states) map[s.character_id] = { state: s.state, since: s.since ?? Date.now() / 1000 };
        setPresenceStates(map);
      } catch { /* ignore */ }
      try {
        unlisten = await listen<{ character_id: string; to: string }>('presence:changed', (e) => {
          if (!e.payload?.character_id) return;
          setPresenceStates((prev) => ({
            ...prev,
            [e.payload.character_id]: { state: e.payload.to, since: Date.now() / 1000 },
          }));
        });
      } catch { /* ignore */ }
    })();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: SPACING.xl }}>
      <TwinView
        left={
          <MemoCharacterMindPanel
            characterId="vivian"
            mind={vivianMind}
            mood={vivianMood}
            psychState={vivianPsych}
            loading={loadingV}
            error={errorV}
            presenceState={presenceStates['vivian']?.state ?? 'online'}
            presenceSince={presenceStates['vivian']?.since ?? Date.now() / 1000}
          />
        }
        right={
          <MemoCharacterMindPanel
            characterId="nana"
            mind={nanaMind}
            mood={nanaMood}
            psychState={nanaPsych}
            loading={loadingN}
            error={errorN}
            presenceState={presenceStates['nana']?.state ?? 'online'}
            presenceSince={presenceStates['nana']?.since ?? Date.now() / 1000}
          />
        }
      />
    </div>
  );
};

const MemoLiveMindView = React.memo(LiveMindView);

// ============================================================
// MindPage — 主页面
// ============================================================

const MindPage: React.FC = () => {
  return (
    <div
      style={{
        flex: 1,
        overflowY: 'auto',
        display: 'flex',
        flexDirection: 'column',
        gap: SPACING.md,
      }}
    >
      <MemoLiveMindView />
    </div>
  );
};

export default MindPage;
