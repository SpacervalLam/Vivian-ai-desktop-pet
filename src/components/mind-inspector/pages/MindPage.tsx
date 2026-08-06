/**
 * Mind 页 — 认知调试器（Cognitive Debugger）
 *
 * 心智观察器最核心的页面。2 个子视图：
 * - Live Mind：实时心智快照（Twin View，Vivian + Nana 并排，5 秒轮询）
 * - Context Pipeline：Prompt 组装分解（八层意识分区）
 *
 * 数据源：
 * - get_mind_state / get_current_mood（Live Mind）
 * - get_last_prompt_breakdown（Context Pipeline）
 */

import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
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
  Copy,
  Check,
  Save,
  RotateCcw,
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
  Plus,
  Trash2,
  Wrench,
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

// ============================================================
// 类型定义
// ============================================================

interface PromptSection {
  name: string;
  preview: string;
  full_content: string;
  char_count: number;
  section_id?: string;
  layer?: string;
  token_estimate?: number;
  optional?: boolean;
  present?: boolean;
}

interface SceneModePreview {
  mode: string;
  description: string;
  instructions: string[];
}

interface ApiParamInfo {
  param_type: string;
  label: string;
  content: string;
  present: boolean;
}

interface PromptBreakdown {
  character_id: string;
  sections: PromptSection[];
  total_chars: number;
  timestamp: number;
  scene_modes?: SceneModePreview[];
  api_params?: ApiParamInfo[];
}

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
type SubView = 'live' | 'pipeline';

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

/** Unix 时间戳可能是秒或毫秒，统一到毫秒 */
const toMs = (ts: number): number => (ts < 1e12 ? ts * 1000 : ts);

/** 将后端返回的英文分区名映射为当前语言的显示名，找不到则原样返回 */
const sectionLabel = (name: string, t: TFunction): string => {
  const key = `mind_inspector.mind.section_names.${name.toLowerCase().replace(/\s+/g, '_')}`;
  const translated = t(key);
  return translated === key ? name : translated;
};

/** 提示词层级配置：颜色 + 排序权重（与后端 SectionLayer 对应） */
const LAYER_CONFIG: Record<string, { color: string; order: number }> = {
  framework: { color: '#8E8E93', order: 0 },       // 灰
  advanced: { color: '#8E8E93', order: 0 },        // 高级配置，与 framework 同组
  character: { color: '#BF5AF2', order: 1 },       // 紫
  mind: { color: '#FF6482', order: 2 },           // 玫红
  world: { color: '#30D158', order: 3 },           // 绿
  relationship: { color: '#FF9F0A', order: 4 },    // 橙
  memory: { color: '#FFD60A', order: 5 },          // 琥珀
  episode: { color: '#FFD60A', order: 5 },         // 记忆相关，与 memory 同组
  user_profile: { color: '#5AC8FA', order: 6 },   // 蓝
  profile: { color: '#5AC8FA', order: 6 },         // 用户画像相关，与 user_profile 同组
  generation: { color: '#5E5CE6', order: 7 },      // 靛蓝
  tail: { color: '#5E5CE6', order: 7 },            // 生成引导相关，与 generation 同组
  postprocess: { color: '#f59e0b', order: 8 },     // 后处理
};

/** 将后端层级名规范化为 snake_case 小写，匹配 LAYER_CONFIG 和 i18n key */
const normalizeLayer = (layer: string | undefined): string =>
  (layer ?? '').toLowerCase().replace(/\s+/g, '_');

/** 层级颜色（未知层级回退为灰色） */
const layerColor = (layer: string | undefined): string =>
  LAYER_CONFIG[normalizeLayer(layer)]?.color ?? '#8E8E93';

/** 将后端层级名映射为当前语言的显示名，找不到则原样返回 */
const layerLabel = (layer: string, t: TFunction): string => {
  const normalized = normalizeLayer(layer);
  const key = `mind_inspector.mind.layer_names.${normalized}`;
  const translated = t(key);
  return translated === key ? layer : translated;
};

/** 模板预览模式下始终显示 section 的原始 full_content，不替换为占位提示文本 */
const templateContent = (
  _name: string,
  fullContent: string,
  _isTemplate: boolean,
  _t: TFunction,
): string => {
  return fullContent;
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
  return new Date(toMs(ts)).toLocaleString();
};

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
// SegmentedControl — 通用分段控件
// ============================================================

interface SegmentedProps<T extends string> {
  options: Array<{ key: T; label: string }>;
  value: T;
  onChange: (v: T) => void;
  accent?: string;
}

function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  accent,
}: SegmentedProps<T>) {
  const activeColor = accent ?? COLORS.accent;
  return (
    <div
      style={{
        display: 'inline-flex',
        padding: 3,
        borderRadius: RADIUS.pill,
        background: COLORS.bgSurface,
        border: `1px solid ${COLORS.border}`,
        gap: 2,
      }}
    >
      {options.map((opt) => {
        const active = opt.key === value;
        return (
          <button
            key={opt.key}
            type="button"
            onClick={() => onChange(opt.key)}
            style={{
              padding: `6px ${SPACING.md}px`,
              borderRadius: RADIUS.pill,
              border: 'none',
              background: active ? activeColor : 'transparent',
              color: active ? COLORS.selectedText : COLORS.textSecondary,
              ...TYPO.caption,
              fontWeight: active ? 600 : 400,
              cursor: 'pointer',
              transition: `all ${DURATION.normal}s ${EASE.ios}`,
              textTransform: 'none',
              letterSpacing: 0,
            }}
          >
            {opt.label}
          </button>
        );
      })}
    </div>
  );
}

const MemoSegmentedControl = React.memo(SegmentedControl) as typeof SegmentedControl;

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
}) => {
  const { t } = useTranslation();
  const nav = useNavigation();
  const accent = CHARACTER_ACCENT[characterId];
  const label = t(`mind_inspector.common.char_${characterId}`);
  const emotionKey = mood?.primary_emotion ?? '';
  const emotionLabel = mood?.mood_label ?? mood?.emotion_label ?? emotionKey;
  const [moodExpanded, setMoodExpanded] = useState(false);
  const [quadrantOpen, setQuadrantOpen] = useState(false);
  const [quadrantClosing, setQuadrantClosing] = useState(false);
  const [ripples, setRipples] = useState<Array<{ id: number; x: number; y: number }>>([]);
  const lastRippleTs = useRef(0);

  const closeQuadrant = () => {
    setQuadrantClosing(true);
  };

  const onQuadrantCloseEnd = () => {
    setQuadrantClosing(false);
    setQuadrantOpen(false);
  };

  const goals = mind?.goals ?? [];
  const activeGoal = goals.find((g) => g.active) ?? goals[0];
  const thought = extractThoughtText(mind?.current_thought ?? '');
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

  // 微透卡片容器（Apple 风格：极淡背景 + 模糊 + 细边框）
  const SectionCard: React.FC<{ children?: React.ReactNode; style?: React.CSSProperties }> = ({
    children,
    style,
  }) => (
    <div
      style={{
        padding: SPACING.md,
        borderRadius: RADIUS.md,
        background: COLORS.subtleBg,
        border: `1px solid ${COLORS.subtleBorder}`,
        backdropFilter: 'blur(8px)',
        WebkitBackdropFilter: 'blur(8px)',
        ...style,
      }}
    >
      {children}
    </div>
  );

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
          <SectionCard>
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
          </SectionCard>

          {/* === Mood（大尺寸象限图 + 细条形情绪） === */}
          <SectionCard>
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
                  <div style={{ ...TYPO.micro, color: COLORS.textTertiary }}>
                    {t('mind_inspector.mind.no_psychology_data')}
                  </div>
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
                    <span style={{ ...TYPO.caption, color: COLORS.textSecondary }}>
                      {t('mind_inspector.mind.emotion_dims')}
                    </span>
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
                    <span style={{ ...TYPO.caption, color: COLORS.textSecondary }}>
                      {t('mind_inspector.mind.need_dims')}
                    </span>
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
                    <span style={{ ...TYPO.caption, color: COLORS.textSecondary }}>
                      {t('mind_inspector.mind.relationship_dims')}
                    </span>
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
          </SectionCard>

         {/* === Goal（Apple Reminder 风格） === */}
          <SectionCard>
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
          </SectionCard>

          {/* === Status（圆环状态指示器） === */}
          <SectionCard>
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
                    strokeDashoffset={62.8 * (1 - (mind?.focus_charge ?? 0))}
                    transform="rotate(-90 12 12)"
                    style={{
                      transition: `stroke-dashoffset ${DURATION.slow}s ${EASE.ios}`,
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
                  {t('mind_inspector.mind.focus_charge')} {Math.round((mind?.focus_charge ?? 0) * 100)}%
                </div>
              </div>
            </div>
          </SectionCard>
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
            background: 'rgba(0,0,0,0.3)',
            backdropFilter: 'blur(12px)',
            WebkitBackdropFilter: 'blur(12px)',
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
  const [presenceStates, setPresenceStates] = useState<Record<string, string>>({});

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
        const states = await invoke<Array<{ character_id: string; state: string }>>('get_all_presence_states');
        if (cancelled) return;
        const map: Record<string, string> = {};
        for (const s of states) map[s.character_id] = s.state;
        setPresenceStates(map);
      } catch { /* ignore */ }
      try {
        unlisten = await listen<{ character_id: string; to: string }>('presence:changed', (e) => {
          if (!e.payload?.character_id) return;
          setPresenceStates((prev) => ({ ...prev, [e.payload.character_id]: e.payload.to }));
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
            presenceState={presenceStates['vivian'] ?? 'online'}
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
            presenceState={presenceStates['nana'] ?? 'online'}
          />
        }
      />
    </div>
  );
};

const MemoLiveMindView = React.memo(LiveMindView);

// ============================================================
// 子视图 3：Context Pipeline（Prompt 组装分解）
// ============================================================

// ============================================================
// IdentityEditor — Character 层 8 段可编辑面板
// ============================================================

// PersonaTextarea — 用 React state 控制聚焦边框，避免命令式 DOM 操作
const PersonaTextarea: React.FC<{
  value: string;
  onChange: (v: string) => void;
  disabled?: boolean;
  accent: string;
}> = ({ value, onChange, disabled, accent }) => {
  const [focused, setFocused] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);

  const autoResize = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = el.scrollHeight + 'px';
  }, []);

  useEffect(() => {
    autoResize();
  }, [value, autoResize]);

  useLayoutEffect(() => {
    autoResize();
  }, [autoResize]);

  return (
    <textarea
      ref={ref}
      value={value}
      onChange={(e) => {
        onChange(e.target.value);
        autoResize();
      }}
      disabled={disabled}
      onFocus={() => setFocused(true)}
      onBlur={() => setFocused(false)}
      style={{
        width: '100%',
        minHeight: 120,
        padding: SPACING.sm,
        borderRadius: RADIUS.sm,
        border: `1px solid ${focused ? accent : COLORS.border}`,
        background: COLORS.bgSurface,
        color: COLORS.textPrimary,
        ...TYPO.micro,
        fontFamily: TYPO.fontMono,
        lineHeight: 1.6,
        resize: 'none',
        overflow: 'hidden',
        outline: 'none',
        boxSizing: 'border-box',
        transition: `border-color ${DURATION.fast}s ${EASE.swift}, background ${DURATION.fast}s ${EASE.swift}`,
        backdropFilter: 'blur(8px)',
        WebkitBackdropFilter: 'blur(8px)',
      }}
    />
  );
};

type PersonaSubKey = 'identity' | 'personality' | 'background' | 'interests' | 'appearance' | 'speech' | 'relationships';

interface IdentityEditorProps {
  sections: Record<PersonaSubKey, string>;
  drafts: Record<string, string>;
  customized: Record<string, boolean>;
  savingSection: string | null;
  onChange: (key: PersonaSubKey, value: string) => void;
  onSave: (key: PersonaSubKey) => void;
  onReset: (key: PersonaSubKey) => void;
  isDirty: (key: PersonaSubKey, currentContent: string) => boolean;
  t: TFunction;
}

const PERSONA_EDITOR_SUBSECTIONS: PersonaSubKey[] = [
  'identity', 'personality', 'background', 'interests',
  'appearance', 'speech', 'relationships',
];

const SUB_BORDER_COLORS: Record<PersonaSubKey, string> = {
  identity: '#BF5AF2',
  personality: '#5AC8FA',
  background: '#FF9500',
  interests: '#30D158',
  appearance: '#FF375F',
  speech: '#64D2FF',
  relationships: '#FFD60A',
};

const IdentityEditor: React.FC<IdentityEditorProps> = ({
  sections,
  drafts,
  customized,
  savingSection,
  onChange,
  onSave,
  onReset,
  isDirty,
  t,
}) => {
  const subLabelKey: Record<PersonaSubKey, string> = {
    identity: 'mind_inspector.mind.sub_identity',
    personality: 'mind_inspector.mind.sub_personality',
    background: 'mind_inspector.mind.sub_background',
    interests: 'mind_inspector.mind.sub_interests',
    appearance: 'mind_inspector.mind.sub_appearance',
    speech: 'mind_inspector.mind.sub_speech',
    relationships: 'mind_inspector.mind.sub_relationships',
  };

  return (
    <div
      style={{
        marginTop: SPACING.sm,
        display: 'flex',
        flexDirection: 'column',
        gap: SPACING.sm,
      }}
      onClick={(e) => e.stopPropagation()}
    >
      {PERSONA_EDITOR_SUBSECTIONS.map((key) => {
        const currentContent = sections[key];
        const draft = drafts[key] ?? currentContent;
        const dirty = isDirty(key, currentContent);
        const saved = customized[key];
        const canReset = dirty || saved;
        const saving = savingSection === key;
        const accent = SUB_BORDER_COLORS[key];
        return (
          <div
            key={key}
            style={{
              padding: SPACING.md,
              background: COLORS.bgDeep,
              borderRadius: RADIUS.sm,
              border: `1px solid ${dirty ? accent : COLORS.border}`,
              transition: `border-color ${DURATION.fast}s ${EASE.swift}`,
              position: 'relative',
            }}
          >
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                marginBottom: SPACING.xs,
                gap: SPACING.sm,
              }}
            >
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: SPACING.xs }}>
                <span
                  style={{
                    width: 6,
                    height: 6,
                    borderRadius: 3,
                    background: accent,
                    flexShrink: 0,
                  }}
                />
                <span style={{ ...TYPO.caption, fontWeight: 600, color: COLORS.textPrimary }}>
                  {t(subLabelKey[key])}
                </span>
                {saved && !dirty && (
                  <span
                    style={{
                      ...TYPO.micro,
                      color: COLORS.success,
                      padding: '1px 6px',
                      borderRadius: RADIUS.pill,
                      background: `${COLORS.success}15`,
                    }}
                  >
                    {t('mind_inspector.mind.persona_customized')}
                  </span>
                )}
                {dirty && (
                  <span
                    style={{
                      ...TYPO.micro,
                      color: COLORS.warning,
                      padding: '1px 6px',
                      borderRadius: RADIUS.pill,
                      background: `${COLORS.warning}15`,
                    }}
                  >
                    {t('mind_inspector.mind.persona_unsaved')}
                  </span>
                )}
              </span>
              <span style={{ display: 'inline-flex', gap: SPACING.xs }}>
                <button
                  type="button"
                  disabled={saving || !dirty}
                  onClick={() => onSave(key)}
                  style={{
                    display: 'inline-flex',
                    alignItems: 'center',
                    gap: 4,
                    padding: `3px ${SPACING.sm}px`,
                    borderRadius: RADIUS.pill,
                    border: `1px solid ${dirty ? COLORS.accent : COLORS.border}`,
                    background: dirty ? `${COLORS.accent}15` : 'transparent',
                    color: dirty ? COLORS.accent : COLORS.textTertiary,
                    ...TYPO.micro,
                    cursor: saving || !dirty ? 'not-allowed' : 'pointer',
                    opacity: saving || !dirty ? 0.5 : 1,
                    transition: `all ${DURATION.fast}s ${EASE.swift}`,
                  }}
                  title={t('mind_inspector.common.save')}
                >
                  <Save size={11} strokeWidth={2} />
                  {saving ? t('mind_inspector.common.saving') : t('mind_inspector.common.save')}
                </button>
                <button
                  type="button"
                  disabled={saving || !canReset}
                  onClick={() => onReset(key)}
                  style={{
                    display: 'inline-flex',
                    alignItems: 'center',
                    gap: 4,
                    padding: `3px ${SPACING.sm}px`,
                    borderRadius: RADIUS.pill,
                    border: `1px solid ${canReset ? COLORS.danger : COLORS.border}`,
                    background: 'transparent',
                    color: canReset ? COLORS.danger : COLORS.textTertiary,
                    ...TYPO.micro,
                    cursor: saving || !canReset ? 'not-allowed' : 'pointer',
                    opacity: saving || !canReset ? 0.5 : 1,
                    transition: `all ${DURATION.fast}s ${EASE.swift}`,
                  }}
                  title={t('mind_inspector.mind.persona_reset_hint')}
                >
                  <RotateCcw size={11} strokeWidth={1.5} />
                  {t('mind_inspector.mind.persona_reset')}
                </button>
              </span>
            </div>
            <PersonaTextarea
              value={draft}
              onChange={(v) => onChange(key, v)}
              disabled={saving}
              accent={accent}
            />
          </div>
        );
      })}
    </div>
  );
};

const MemoIdentityEditor = React.memo(IdentityEditor);

// ============================================================
// Few-shot Examples 结构化表单编辑器
// ============================================================

type FewShotIntent = 'reply' | 'short_reply' | 'no_reply';

interface FewShotExample {
  scenario: string;
  user_input: string;
  response_text: string;
  intent: FewShotIntent;
  tool?: string | null;
  arguments?: Record<string, unknown> | null;
}

interface FewShotExamplesConfig {
  intro: string;
  examples: FewShotExample[];
}

interface FewShotExamplesResponse {
  data: FewShotExamplesConfig;
  customized: boolean;
}

interface ToolInfo {
  name: string;
  description: string;
  category: string;
  input_schema: {
    properties?: Record<string, { type?: string; description?: string }>;
    required?: string[];
  };
}

interface ToolsListResponse {
  tools: ToolInfo[];
  total: number;
}

const EXAMPLES_ACCENT = '#5E5CE6';

const INTENT_OPTIONS: { value: FewShotIntent; labelKey: string }[] = [
  { value: 'short_reply', labelKey: 'mind_inspector.examples.intent_short_reply' },
  { value: 'reply', labelKey: 'mind_inspector.examples.intent_reply' },
  { value: 'no_reply', labelKey: 'mind_inspector.examples.intent_no_reply' },
];

function createEmptyExample(): FewShotExample {
  return {
    scenario: '',
    user_input: '',
    response_text: '',
    intent: 'short_reply',
    tool: null,
    arguments: null,
  };
}

const ExamplesEditor: React.FC<{ t: TFunction; charId: CharacterId }> = ({ t, charId }) => {
  const [data, setData] = useState<FewShotExamplesConfig>({ intro: '', examples: [] });
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [customized, setCustomized] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [tools, setTools] = useState<ToolInfo[]>([]);
  const [argsDrafts, setArgsDrafts] = useState<Record<number, string>>({});

  const buildSkeletonArgs = useCallback((tool: ToolInfo | undefined): string => {
    const props = tool?.input_schema?.properties;
    if (!props || Object.keys(props).length === 0) return '{}';
    const skeleton: Record<string, unknown> = {};
    for (const [pname, pinfo] of Object.entries(props)) {
      switch (pinfo?.type) {
        case 'string':
          skeleton[pname] = '';
          break;
        case 'number':
        case 'integer':
          skeleton[pname] = 0;
          break;
        case 'boolean':
          skeleton[pname] = false;
          break;
        case 'array':
          skeleton[pname] = [];
          break;
        case 'object':
          skeleton[pname] = {};
          break;
        default:
          skeleton[pname] = '';
      }
    }
    return JSON.stringify(skeleton, null, 2);
  }, []);

  const load = useCallback(async () => {
    setLoaded(false);
    try {
      const [result, toolsResult] = await Promise.all([
        invoke<FewShotExamplesResponse>('get_few_shot_examples', { characterId: charId }),
        invoke<ToolsListResponse>('list_tools'),
      ]);
      setData(result.data);
      setCustomized(result.customized);
      setTools(toolsResult.tools || []);
      setDirty(false);
    } catch { /* ignore */ }
    setLoaded(true);
  }, [charId]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!loaded || tools.length === 0) return;
    setArgsDrafts(prev => {
      const next: Record<number, string> = {};
      data.examples.forEach((ex, idx) => {
        if (idx in prev) {
          next[idx] = prev[idx];
        } else if (ex.tool && ex.arguments) {
          next[idx] = JSON.stringify(ex.arguments, null, 2);
        } else if (ex.tool) {
          const tool = tools.find(t => t.name === ex.tool);
          next[idx] = buildSkeletonArgs(tool);
        } else {
          next[idx] = '';
        }
      });
      return next;
    });
  }, [loaded, tools, data.examples, buildSkeletonArgs]);

  const updateField = (idx: number, field: keyof FewShotExample, value: unknown) => {
    setData(prev => {
      const next = { ...prev, examples: [...prev.examples] };
      next.examples[idx] = { ...next.examples[idx], [field]: value } as FewShotExample;
      return next;
    });
    setDirty(true);
  };

  const addExample = () => {
    const newIdx = data.examples.length;
    setData(prev => ({ ...prev, examples: [...prev.examples, createEmptyExample()] }));
    setArgsDrafts(prev => ({ ...prev, [newIdx]: '' }));
    setDirty(true);
  };

  const removeExample = (idx: number) => {
    setData(prev => ({ ...prev, examples: prev.examples.filter((_, i) => i !== idx) }));
    setArgsDrafts(prev => {
      const next: Record<number, string> = {};
      data.examples.forEach((_, i) => {
        if (i < idx) next[i] = prev[i] ?? '';
        else if (i > idx) next[i - 1] = prev[i] ?? '';
      });
      return next;
    });
    setDirty(true);
  };

  const moveExample = (idx: number, dir: -1 | 1) => {
    const target = idx + dir;
    if (target < 0 || target >= data.examples.length) return;
    setData(prev => {
      const arr = [...prev.examples];
      [arr[idx], arr[target]] = [arr[target], arr[idx]];
      return { ...prev, examples: arr };
    });
    setArgsDrafts(prev => {
      const next = { ...prev };
      const tmp = next[idx];
      next[idx] = next[target] ?? '';
      next[target] = tmp ?? '';
      return next;
    });
    setDirty(true);
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      await invoke('set_few_shot_examples', { characterId: charId, data });
      setCustomized(true);
      setDirty(false);
    } catch { /* ignore */ }
    setSaving(false);
  };

  const handleReset = async () => {
    setSaving(true);
    try {
      await invoke('reset_persona_section', { characterId: charId, section: 'examples' });
      setArgsDrafts({});
      await load();
    } catch { /* ignore */ }
    setSaving(false);
  };

  if (!loaded) {
    return <div style={{ ...TYPO.micro, color: COLORS.textTertiary, padding: SPACING.sm }}>Loading...</div>;
  }

  const inputStyle: React.CSSProperties = {
    width: '100%',
    background: COLORS.bgDeep,
    border: `1px solid ${COLORS.border}`,
    borderRadius: RADIUS.sm,
    padding: `${SPACING.xs}px ${SPACING.sm}px`,
    color: COLORS.textPrimary,
    ...TYPO.caption,
    fontFamily: TYPO.fontMono,
    fontSize: 12,
    outline: 'none',
    transition: `border-color ${DURATION.fast}s ${EASE.swift}`,
    boxSizing: 'border-box',
  };

  const labelStyle: React.CSSProperties = {
    ...TYPO.micro,
    color: COLORS.textTertiary,
    marginBottom: 2,
    display: 'block',
    textTransform: 'uppercase',
    letterSpacing: 0.5,
    fontSize: 10,
  };

  return (
    <div
      style={{
        marginTop: SPACING.sm,
        display: 'flex',
        flexDirection: 'column',
        gap: SPACING.sm,
      }}
      onClick={(e) => e.stopPropagation()}
    >
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: SPACING.sm }}>
        <div style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
          {customized && !dirty && (
            <span style={{
              ...TYPO.micro, color: COLORS.success, padding: '1px 6px',
              borderRadius: RADIUS.pill, background: `${COLORS.success}15`,
            }}>
              {t('mind_inspector.mind.persona_customized')}
            </span>
          )}
          {dirty && (
            <span style={{
              ...TYPO.micro, color: COLORS.warning, padding: '1px 6px',
              borderRadius: RADIUS.pill, background: `${COLORS.warning}15`,
            }}>
              {t('mind_inspector.mind.persona_unsaved')}
            </span>
          )}
        </div>
        <div style={{ display: 'inline-flex', gap: SPACING.xs }}>
          <button
            type="button"
            disabled={saving || !dirty}
            onClick={handleSave}
            style={{
              display: 'inline-flex', alignItems: 'center', gap: 4,
              padding: `3px ${SPACING.sm}px`, borderRadius: RADIUS.pill,
              border: `1px solid ${dirty ? EXAMPLES_ACCENT : COLORS.border}`,
              background: dirty ? `${EXAMPLES_ACCENT}15` : 'transparent',
              color: dirty ? EXAMPLES_ACCENT : COLORS.textTertiary,
              ...TYPO.micro, cursor: saving || !dirty ? 'not-allowed' : 'pointer',
              opacity: saving || !dirty ? 0.5 : 1,
            }}
          >
            <Save size={11} strokeWidth={2} />
            {saving ? t('mind_inspector.common.saving') : t('mind_inspector.common.save')}
          </button>
          <button
            type="button"
            disabled={saving || !(dirty || customized)}
            onClick={handleReset}
            style={{
              display: 'inline-flex', alignItems: 'center', gap: 4,
              padding: `3px ${SPACING.sm}px`, borderRadius: RADIUS.pill,
              border: `1px solid ${(dirty || customized) ? COLORS.danger : COLORS.border}`,
              background: 'transparent',
              color: (dirty || customized) ? COLORS.danger : COLORS.textTertiary,
              ...TYPO.micro, cursor: saving || !(dirty || customized) ? 'not-allowed' : 'pointer',
              opacity: saving || !(dirty || customized) ? 0.5 : 1,
            }}
          >
            <RotateCcw size={11} strokeWidth={1.5} />
            {t('mind_inspector.mind.persona_reset')}
          </button>
        </div>
      </div>

      {data.intro && (
        <div style={{
          padding: `${SPACING.sm}px ${SPACING.md}px`,
          background: `${EXAMPLES_ACCENT}08`,
          borderRadius: RADIUS.sm,
          border: `1px solid ${EXAMPLES_ACCENT}20`,
        }}>
          <div
            style={{
              ...TYPO.micro,
              color: COLORS.textTertiary,
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-word',
              lineHeight: 1.6,
              fontSize: 12,
              opacity: 0.85,
            }}
          >
            {data.intro}
          </div>
        </div>
      )}

      <div style={{ display: 'flex', flexDirection: 'column', gap: SPACING.sm }}>
        {data.examples.map((ex, idx) => (
          // FewShotExample 无稳定 id，用 index 作 key
          <div key={idx} style={{
            padding: SPACING.md, background: COLORS.bgDeep, borderRadius: RADIUS.sm,
            border: `1px solid ${EXAMPLES_ACCENT}30`, position: 'relative',
          }}>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: SPACING.xs }}>
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: SPACING.xs }}>
                <span style={{
                  width: 6, height: 6, borderRadius: 3, background: EXAMPLES_ACCENT, flexShrink: 0,
                }} />
                <span style={{ ...TYPO.caption, fontWeight: 600, color: COLORS.textPrimary }}>
                  {t('mind_inspector.examples.example_num', { num: idx + 1 })}
                </span>
              </span>
              <span style={{ display: 'inline-flex', gap: 2 }}>
                <button
                  type="button"
                  onClick={() => moveExample(idx, -1)}
                  disabled={idx === 0}
                  style={{
                    width: 22, height: 22, display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
                    border: 'none', background: 'transparent', color: COLORS.textTertiary, cursor: idx === 0 ? 'not-allowed' : 'pointer',
                    opacity: idx === 0 ? 0.3 : 1, borderRadius: RADIUS.xs,
                  }}
                  title={t('mind_inspector.examples.move_up')}
                >
                  <ChevronUp size={13} strokeWidth={1.5} />
                </button>
                <button
                  type="button"
                  onClick={() => moveExample(idx, 1)}
                  disabled={idx === data.examples.length - 1}
                  style={{
                    width: 22, height: 22, display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
                    border: 'none', background: 'transparent', color: COLORS.textTertiary, cursor: idx === data.examples.length - 1 ? 'not-allowed' : 'pointer',
                    opacity: idx === data.examples.length - 1 ? 0.3 : 1, borderRadius: RADIUS.xs,
                  }}
                  title={t('mind_inspector.examples.move_down')}
                >
                  <ChevronDown size={13} strokeWidth={1.5} />
                </button>
                <button
                  type="button"
                  onClick={() => removeExample(idx)}
                  style={{
                    width: 22, height: 22, display: 'inline-flex', alignItems: 'center', justifyContent: 'center',
                    border: 'none', background: 'transparent', color: COLORS.danger, cursor: 'pointer', borderRadius: RADIUS.xs,
                  }}
                  title={t('mind_inspector.examples.remove')}
                >
                  <Trash2 size={12} strokeWidth={1.5} />
                </button>
              </span>
            </div>

            <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: SPACING.sm }}>
              <div>
                <label style={labelStyle}>{t('mind_inspector.examples.scenario')}</label>
                <input
                  type="text"
                  value={ex.scenario}
                  onChange={(e) => updateField(idx, 'scenario', e.target.value)}
                  style={inputStyle}
                  placeholder={t('mind_inspector.examples.scenario_placeholder')}
                />
              </div>
              <div>
                <label style={labelStyle}>{t('mind_inspector.examples.intent')}</label>
                <select
                  value={ex.intent}
                  onChange={(e) => updateField(idx, 'intent', e.target.value as FewShotIntent)}
                  style={{ ...inputStyle, cursor: 'pointer' }}
                >
                  {INTENT_OPTIONS.map(opt => (
                    <option key={opt.value} value={opt.value}>{t(opt.labelKey)}</option>
                  ))}
                </select>
              </div>
            </div>

            <div style={{ marginTop: SPACING.xs }}>
              <label style={labelStyle}>{t('mind_inspector.examples.user_input')}</label>
              <input
                type="text"
                value={ex.user_input}
                onChange={(e) => updateField(idx, 'user_input', e.target.value)}
                style={inputStyle}
                placeholder={t('mind_inspector.examples.user_input_placeholder')}
              />
            </div>

            <div style={{ marginTop: SPACING.xs }}>
              <label style={labelStyle}>{t('mind_inspector.examples.response_text')}</label>
              <input
                type="text"
                value={ex.response_text}
                onChange={(e) => updateField(idx, 'response_text', e.target.value)}
                style={inputStyle}
                placeholder={t('mind_inspector.examples.response_text_placeholder')}
              />
            </div>

            {(ex.tool !== null && ex.tool !== undefined) && (
              <div style={{ marginTop: SPACING.xs }}>
                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 2 }}>
                  <label style={{ ...labelStyle, marginBottom: 0 }}>{t('mind_inspector.examples.tool_section')}</label>
                  <button
                    type="button"
                    onClick={() => {
                      updateField(idx, 'tool', null);
                      updateField(idx, 'arguments', null);
                      setArgsDrafts(prev => ({ ...prev, [idx]: '' }));
                    }}
                    style={{
                      ...TYPO.micro, color: COLORS.danger, background: 'transparent',
                      border: 'none', cursor: 'pointer', padding: 0,
                    }}
                  >
                    ✕
                  </button>
                </div>
                <div>
                  <label style={labelStyle}>{t('mind_inspector.examples.tool')}</label>
                  <select
                    value={ex.tool || ''}
                    onChange={(e) => {
                      const val = e.target.value;
                      updateField(idx, 'tool', val || null);
                      if (!val) {
                        updateField(idx, 'arguments', null);
                        setArgsDrafts(prev => ({ ...prev, [idx]: '' }));
                      } else {
                        const tool = tools.find(t => t.name === val);
                        const skeleton = buildSkeletonArgs(tool);
                        setArgsDrafts(prev => ({ ...prev, [idx]: skeleton }));
                        try {
                          updateField(idx, 'arguments', JSON.parse(skeleton));
                        } catch { /* ignore */ }
                      }
                    }}
                    style={{ ...inputStyle, cursor: 'pointer' }}
                  >
                    <option value="">—</option>
                    {tools.map(t => (
                      <option key={t.name} value={t.name}>{t.name}</option>
                    ))}
                  </select>
                </div>
                {ex.tool && (() => {
                  const selectedTool = tools.find(t => t.name === ex.tool);
                  const props = selectedTool?.input_schema?.properties;
                  const required = selectedTool?.input_schema?.required || [];
                  const draftText = argsDrafts[idx] ?? '';
                  const hasJsonError = draftText.trim().length > 0 && (() => {
                    try { JSON.parse(draftText); return false; } catch { return true; }
                  })();
                  return (
                    <div style={{ marginTop: SPACING.xs }}>
                      <label style={labelStyle}>{t('mind_inspector.examples.arguments')}</label>
                      <textarea
                        value={draftText}
                        onChange={(e) => {
                          const v = e.target.value;
                          setArgsDrafts(prev => ({ ...prev, [idx]: v }));
                          const trimmed = v.trim();
                          if (!trimmed) {
                            updateField(idx, 'arguments', null);
                          } else {
                            try {
                              const parsed = JSON.parse(trimmed);
                              updateField(idx, 'arguments', parsed);
                            } catch {
                              /* JSON无效时不更新data，保持用户编辑状态 */
                            }
                          }
                        }}
                        style={{
                          ...inputStyle,
                          minHeight: 80,
                          fontFamily: TYPO.fontMono,
                          fontSize: 11,
                          resize: 'vertical',
                          borderColor: hasJsonError ? COLORS.danger : undefined,
                        }}
                        placeholder='{}'
                        rows={Math.max(3, draftText.split('\n').length)}
                      />
                      {hasJsonError && (
                        <div style={{ ...TYPO.micro, color: COLORS.danger, marginTop: 2 }}>{t('mind_inspector.mind.json_format_error')}</div>
                      )}
                      {props && Object.keys(props).length > 0 && (
                        <div style={{ marginTop: 4, padding: `${SPACING.xs}px ${SPACING.sm}px`, background: `${COLORS.bgDeep}80`, borderRadius: RADIUS.xs }}>
                          <div style={{ ...TYPO.micro, color: COLORS.textTertiary, marginBottom: 2 }}>{t('mind_inspector.mind.param_hint')}</div>
                          {Object.entries(props).map(([pname, pinfo]) => (
                            <div key={pname} style={{ ...TYPO.micro, color: COLORS.textSecondary, lineHeight: 1.5 }}>
                              <code style={{ color: EXAMPLES_ACCENT }}>{pname}</code>
                              {required.includes(pname) && <span style={{ color: COLORS.danger }}>*</span>}
                              {pinfo?.type && <span style={{ color: COLORS.textTertiary }}> ({pinfo.type})</span>}
                              {pinfo?.description && <span> — {pinfo.description}</span>}
                            </div>
                          ))}
                        </div>
                      )}
                    </div>
                  );
                })()}
              </div>
            )}

            {(ex.tool === null || ex.tool === undefined) && (
              <div style={{ marginTop: SPACING.xs }}>
                <button
                  type="button"
                  onClick={() => {
                    updateField(idx, 'tool', '');
                    setArgsDrafts(prev => ({ ...prev, [idx]: '' }));
                  }}
                  style={{
                    ...TYPO.micro, color: COLORS.textTertiary, background: 'transparent',
                    border: `1px dashed ${COLORS.border}`, borderRadius: RADIUS.xs,
                    padding: `2px ${SPACING.sm}px`, cursor: 'pointer',
                  }}
                >
                  + {t('mind_inspector.examples.add_tool')}
                </button>
              </div>
            )}
          </div>
        ))}
      </div>

      <button
        type="button"
        onClick={addExample}
        style={{
          display: 'inline-flex', alignItems: 'center', gap: SPACING.xs,
          padding: `${SPACING.sm}px ${SPACING.md}px`, borderRadius: RADIUS.sm,
          border: `1px dashed ${EXAMPLES_ACCENT}60`, background: `${EXAMPLES_ACCENT}08`,
          color: EXAMPLES_ACCENT, ...TYPO.caption, cursor: 'pointer',
          alignSelf: 'flex-start',
        }}
      >
        <Plus size={14} strokeWidth={1.5} />
        {t('mind_inspector.examples.add_example')}
      </button>
    </div>
  );
};

const MemoExamplesEditor = React.memo(ExamplesEditor);

const ContextPipelineView: React.FC = () => {
  const { t } = useTranslation();
  const [character, setCharacter] = useState<CharacterId>('vivian');
  const [realBreakdown, setRealBreakdown] = useState<PromptBreakdown | null>(null);
  const [templateBreakdown, setTemplateBreakdown] = useState<PromptBreakdown | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Record<number, boolean>>({});
  const [copiedIdx, setCopiedIdx] = useState<number | null>(null);
  const [hoveredSectionIdx, setHoveredSectionIdx] = useState<number | null>(null);
  const [copiedAll, setCopiedAll] = useState(false);
  const [apiParamsExpanded, setApiParamsExpanded] = useState(true);
  const [expandedApiParam, setExpandedApiParam] = useState<Record<number, boolean>>({});
  type ViewMode = 'last_request' | 'template';
  const [viewMode, setViewMode] = useState<ViewMode>('last_request');
  const [activeMode, setActiveMode] = useState<string>('daily_chat');
  const sectionRefs = useRef<(HTMLDivElement | null)[]>([]);
  const pendingScrollIdx = useRef<number | null>(null);

  const toggleSection = useCallback((idx: number, scroll: boolean = false) => {
    setExpanded((prev) => {
      const willOpen = !prev[idx];
      if (scroll && willOpen) {
        pendingScrollIdx.current = idx;
      }
      return { ...prev, [idx]: willOpen };
    });
  }, []);

  useEffect(() => {
    const idx = pendingScrollIdx.current;
    if (idx !== null) {
      pendingScrollIdx.current = null;
      requestAnimationFrame(() => {
        const el = sectionRefs.current[idx];
        if (el) {
          el.scrollIntoView({ behavior: 'smooth', block: 'start' });
        }
      });
    }
  }, [expanded]);

  // 人设段落编辑状态（仅模板模式有效）
  const [personaDrafts, setPersonaDrafts] = useState<Record<string, string>>({});
  const [savingSection, setSavingSection] = useState<string | null>(null);
  const [personaCustomized, setPersonaCustomized] = useState<Record<string, boolean>>({});
  const [personaSections, setPersonaSections] = useState<Record<PersonaSubKey, string>>({
    identity: '', personality: '', background: '', interests: '',
    appearance: '', speech: '', relationships: '',
  });

  const CHARACTER_SECTION_KEY = 'Character';

  interface ToolParamInfo {
    type?: string;
    description?: string;
    enum?: string[];
    minimum?: number;
    maximum?: number;
    default?: unknown;
  }

  interface ToolPreviewInfo {
    name: string;
    description: string;
    category: string;
    is_read_only?: boolean;
    input_schema: {
      properties?: Record<string, ToolParamInfo>;
      required?: string[];
    };
  }

  const [previewTools, setPreviewTools] = useState<ToolPreviewInfo[]>([]);
  const [expandedToolCats, setExpandedToolCats] = useState<Record<string, boolean>>({});

  useEffect(() => {
    invoke<{ tools: ToolPreviewInfo[]; total: number }>('list_tools')
      .then(res => setPreviewTools(res.tools || []))
      .catch(() => {});
  }, []);

  const categoryLabels: Record<string, string> = {
    system: 'System Control',
    file: 'File & Computer Interaction',
    memory: 'Memory',
    web: 'Web & Info',
    media: 'Media',
    pet: 'Pet Control',
    mcp: 'MCP Tools',
  };

  // 从后端加载 Character 层 7 段文本内容和自定义状态（examples 单独用结构化表单编辑）
  const loadPersonaSections = useCallback(async (charId: CharacterId) => {
    try {
      const sections = await invoke<Record<PersonaSubKey, { content: string; customized: boolean }>>(
        'get_persona_sections',
        { characterId: charId },
      );
      const contents: Record<string, string> = {};
      const customizedMap: Record<string, boolean> = {};
      for (const key of PERSONA_EDITOR_SUBSECTIONS) {
        contents[key] = sections[key]?.content ?? '';
        customizedMap[key] = sections[key]?.customized ?? false;
      }
      setPersonaSections(contents as Record<PersonaSubKey, string>);
      setPersonaCustomized(customizedMap);
      setPersonaDrafts((prev) => {
        const next = { ...prev };
        for (const key of PERSONA_EDITOR_SUBSECTIONS) {
          if (!(key in prev) || prev[key] === '') {
            next[key] = contents[key];
          }
        }
        return next;
      });
    } catch { /* ignore */ }
  }, []);

  useEffect(() => {
    void loadPersonaSections(character);
  }, [character, loadPersonaSections]);

  const handlePersonaDraftChange = (key: PersonaSubKey, value: string) => {
    setPersonaDrafts((prev) => ({ ...prev, [key]: value }));
  };

  const isDraftDirty = (key: PersonaSubKey, currentContent: string): boolean => {
    return (personaDrafts[key] ?? currentContent) !== currentContent;
  };

  const hasRealData = !!realBreakdown;

  // 当没有真实数据时，自动切到模板模式；有真实数据时默认显示最近请求
  useEffect(() => {
    if (!hasRealData) {
      setViewMode('template');
    }
  }, [hasRealData]);

  const fetchData = useCallback(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);

    // 同时拉取真实请求数据和模板数据
    const realPromise = invoke<PromptBreakdown | null>('get_last_prompt_breakdown', {
      characterId: character,
    });
    const templatePromise = invoke<PromptBreakdown | null>('get_prompt_template_preview', {
      characterId: character,
    });

    Promise.all([realPromise, templatePromise])
      .then(([real, template]) => {
        if (cancelled) return;
        setRealBreakdown(real);
        setTemplateBreakdown(template);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [character]);

  const handleSavePersona = async (key: PersonaSubKey) => {
    setSavingSection(key);
    try {
      await invoke('set_persona_section', {
        characterId: character,
        section: key,
        content: personaDrafts[key] ?? '',
      });
      setPersonaCustomized((prev) => ({ ...prev, [key]: (personaDrafts[key] ?? '').trim().length > 0 }));
      setPersonaSections((prev) => ({ ...prev, [key]: personaDrafts[key] ?? '' }));
      await fetchData();
    } catch (e) {
      setError(String(e));
    } finally {
      setSavingSection(null);
    }
  };

  const handleResetPersona = async (key: PersonaSubKey) => {
    setSavingSection(key);
    try {
      const defaultContent = await invoke<string>('reset_persona_section', {
        characterId: character,
        section: key,
      });
      setPersonaDrafts((prev) => ({ ...prev, [key]: defaultContent }));
      setPersonaSections((prev) => ({ ...prev, [key]: defaultContent }));
      setPersonaCustomized((prev) => ({ ...prev, [key]: false }));
      await fetchData();
    } catch (e) {
      setError(String(e));
    } finally {
      setSavingSection(null);
    }
  };

  useEffect(() => {
    const cleanup = fetchData();
    let unlisten: (() => void) | null = null;
    listen('memory:updated', () => {
      fetchData();
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      cleanup();
      if (unlisten) unlisten();
    };
  }, [fetchData]);

  const accent = CHARACTER_ACCENT[character];
  const currentBreakdown = viewMode === 'template' ? templateBreakdown : realBreakdown;
  const isTemplate = viewMode === 'template';
  const sections = currentBreakdown?.sections ?? [];
  const totalChars = currentBreakdown?.total_chars ?? 0;
  const charName = t(`mind_inspector.common.char_${character}`);

  // 切换视图时重置展开状态
  const switchViewMode = (mode: ViewMode) => {
    setViewMode(mode);
    setExpanded({});
    setExpandedLayers({});
    setCopiedIdx(null);
    setCopiedAll(false);
    setActiveMode('daily_chat');
  };

  // 切换角色时重置activeMode
  useEffect(() => {
    setActiveMode('daily_chat');
  }, [character]);

  const copySection = async (idx: number, text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopiedIdx(idx);
      setTimeout(() => setCopiedIdx(null), 1500);
    } catch { /* ignore */ }
  };

  const copyFullPrompt = async () => {
    if (!currentBreakdown) return;
    const full = sections.map((s) => `===== ${sectionLabel(s.name, t)} =====\n${templateContent(s.name, s.full_content, isTemplate, t)}`).join('\n\n');
    try {
      await navigator.clipboard.writeText(full);
      setCopiedAll(true);
      setTimeout(() => setCopiedAll(false), 1500);
    } catch { /* ignore */ }
  };

  // 层级分组展开状态（默认全部收起）
  const [expandedLayers, setExpandedLayers] = useState<Record<string, boolean>>({});
  const toggleLayer = useCallback((layer: string) => {
    setExpandedLayers((prev) => ({ ...prev, [layer]: !prev[layer] }));
  }, []);

  // 自动隐藏 0 字符的空 section（保留原始索引以维持展开/复制/悬停状态稳定）
  const visibleSections = useMemo(
    () =>
      sections
        .map((section, origIdx) => ({ section, origIdx }))
        .filter(({ section }) => section.char_count > 0),
    [sections],
  );
  const hiddenCount = sections.filter(
    (s) => s.char_count === 0 && normalizeLayer(s.layer) !== 'postprocess',
  ).length;

  // 按层级分组并按 LAYER_CONFIG 顺序排序
  const layerGroups = useMemo(() => {
    const groups: Record<string, { layer: string; items: { section: PromptSection; origIdx: number }[] }> = {};
    for (const item of visibleSections) {
      const layer = normalizeLayer(item.section.layer) || 'user_profile';
      if (!groups[layer]) {
        groups[layer] = { layer, items: [] };
      }
      groups[layer].items.push(item);
    }
    return Object.values(groups).sort((a, b) => 
      (LAYER_CONFIG[a.layer]?.order ?? 99) - (LAYER_CONFIG[b.layer]?.order ?? 99)
    );
  }, [visibleSections]);

  // “全部展开/收起”操控所有层级分组抽屉（默认全部收起）
  const allExpanded = layerGroups.length > 0 && layerGroups.every((g) => expandedLayers[g.layer]);

  const toggleAll = () => {
    if (allExpanded) {
      setExpandedLayers({});
    } else {
      const next: Record<string, boolean> = {};
      layerGroups.forEach((g) => { next[g.layer] = true; });
      setExpandedLayers(next);
    }
  };

  // 构建Style section动态内容（根据选中的模式）
  const buildStyleContent = useCallback((section: PromptSection) => {
    const sceneModes = currentBreakdown?.scene_modes ?? [];
    const noGoMatch = section.full_content.match(/### NO-GO[\s\S]*$/);
    const noGoPart = noGoMatch ? noGoMatch[0] : '';

    const currentModeData = sceneModes.find(m => m.mode === activeMode);
    if (!currentModeData) {
      return section.full_content;
    }

    const blocks: string[] = [];
    blocks.push(`#### Mode: \`${currentModeData.mode}\``);
    if (currentModeData.description) {
      blocks.push(currentModeData.description);
      blocks.push('');
    }
    if (currentModeData.instructions.length > 0) {
      blocks.push('**Mode-Specific Instructions:**');
      for (const inst of currentModeData.instructions) {
        blocks.push(`- ${inst}`);
      }
      blocks.push('');
    }
    if (noGoPart) {
      blocks.push(noGoPart);
    }
    return blocks.join('\n');
  }, [activeMode, currentBreakdown]);

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        gap: SPACING.lg,
        width: '100%',
        paddingBottom: SPACING.xl,
        boxSizing: 'border-box',
      }}
    >
      {/* 工具栏：左视图切换组 + 中状态信息 + 右操作组 */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: SPACING.md,
          flexWrap: 'wrap',
          padding: `${SPACING.md}px ${SPACING.lg}px`,
          borderRadius: RADIUS.xl,
          background: COLORS.subtleBg,
          border: `1px solid ${COLORS.subtleBorder}`,
          backdropFilter: 'blur(20px) saturate(180%)',
          WebkitBackdropFilter: 'blur(20px) saturate(180%)',
          position: 'sticky',
          top: 0,
          zIndex: 10,
        }}
      >
        {/* 左侧：视图选择组（角色 + 模式紧贴成组） */}
        <div style={{ display: 'inline-flex', gap: SPACING.xs, alignItems: 'center' }}>
          <MemoSegmentedControl<CharacterId>
            options={[
              { key: 'vivian', label: t('mind_inspector.common.char_vivian') },
              { key: 'nana', label: t('mind_inspector.common.char_nana') },
            ]}
            value={character}
            onChange={setCharacter}
          />
          {hasRealData && (
            <>
              <span
                aria-hidden
                style={{
                  width: 1,
                  height: 14,
                  background: COLORS.border,
                  margin: `0 ${SPACING.xs}px`,
                }}
              />
              <MemoSegmentedControl<ViewMode>
                options={[
                  { key: 'last_request', label: t('mind_inspector.mind.last_request') },
                  { key: 'template', label: t('mind_inspector.mind.template_preview') },
                ]}
                value={viewMode}
                onChange={switchViewMode}
              />
            </>
          )}
        </div>

        {/* 中间：状态信息（柔和标签风格，前置色点指示当前模式） */}
        <span
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: 6,
            ...TYPO.caption,
            color: COLORS.textTertiary,
            textTransform: 'none',
            letterSpacing: 0.1,
            fontWeight: 500,
            padding: `3px ${SPACING.sm}px`,
            borderRadius: RADIUS.pill,
            background: isTemplate ? `${COLORS.info}10` : `${accent}10`,
            border: `1px solid ${isTemplate ? `${COLORS.info}25` : `${accent}25`}`,
          }}
        >
          <span
            aria-hidden
            style={{
              width: 5,
              height: 5,
              borderRadius: RADIUS.pill,
              background: isTemplate ? COLORS.info : accent,
              boxShadow: `0 0 6px ${isTemplate ? COLORS.info : accent}80`,
            }}
          />
          {currentBreakdown
            ? isTemplate
              ? (hasRealData
                  ? t('mind_inspector.mind.template_hint_compare')
                  : t('mind_inspector.mind.template_hint'))
              : t('mind_inspector.mind.assembled_at', { time: formatRelative(currentBreakdown.timestamp, t) })
            : ''}
        </span>

        <div style={{ flex: 1 }} />

        {/* 右侧：操作组（按钮紧贴成组） */}
        {sections.length > 0 && (
          <div style={{ display: 'inline-flex', gap: SPACING.xs, alignItems: 'center' }}>
            <button
              type="button"
              onClick={toggleAll}
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 4,
                padding: `5px ${SPACING.md}px`,
                borderRadius: RADIUS.pill,
                border: `1px solid ${COLORS.border}`,
                background: COLORS.bgSurface,
                color: COLORS.textSecondary,
                ...TYPO.caption,
                textTransform: 'none',
                letterSpacing: 0.1,
                cursor: 'pointer',
                transition: `all ${DURATION.fast}s ${EASE.swift}`,
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.borderColor = COLORS.borderHover;
                e.currentTarget.style.color = COLORS.textPrimary;
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.borderColor = COLORS.border;
                e.currentTarget.style.color = COLORS.textSecondary;
              }}
            >
              {allExpanded ? <ChevronUp size={12} strokeWidth={1.5} /> : <ChevronDown size={12} strokeWidth={1.5} />}
              {allExpanded ? t('mind_inspector.mind.collapse_all') : t('mind_inspector.mind.expand_all')}
            </button>
            <button
              type="button"
              onClick={copyFullPrompt}
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 5,
                padding: `5px ${SPACING.md}px`,
                borderRadius: RADIUS.pill,
                border: `1px solid ${copiedAll ? COLORS.success : COLORS.border}`,
                background: copiedAll ? `${COLORS.success}15` : COLORS.bgSurface,
                color: copiedAll ? COLORS.success : COLORS.textSecondary,
                ...TYPO.caption,
                textTransform: 'none',
                letterSpacing: 0.1,
                cursor: 'pointer',
                transition: `all ${DURATION.fast}s ${EASE.swift}`,
              }}
              onMouseEnter={(e) => {
                if (!copiedAll) {
                  e.currentTarget.style.borderColor = COLORS.borderHover;
                  e.currentTarget.style.color = COLORS.textPrimary;
                }
              }}
              onMouseLeave={(e) => {
                if (!copiedAll) {
                  e.currentTarget.style.borderColor = COLORS.border;
                  e.currentTarget.style.color = COLORS.textSecondary;
                }
              }}
            >
              {copiedAll ? <Check size={12} strokeWidth={2} /> : <Copy size={12} strokeWidth={1.5} />}
              {copiedAll ? t('mind_inspector.common.copied') : t('mind_inspector.mind.copy_all')}
            </button>
          </div>
        )}
      </div>

      <SectionTitle style={{ marginTop: SPACING.sm }}>{t('mind_inspector.mind.pipeline_title', { name: charName })}</SectionTitle>

      {loading && !realBreakdown && !templateBreakdown ? (
        <EmptyState spinner text={t('mind_inspector.mind.loading_pipeline')} />
      ) : error ? (
        <EmptyState icon={<AlertCircle size={24} color={COLORS.textTertiary} strokeWidth={1.5} />} text={t('mind_inspector.common.load_failed', { error })} />
      ) : !currentBreakdown || sections.length === 0 ? (
        <EmptyState text={t('mind_inspector.mind.no_pipeline_data')} />
      ) : (
        <>
          {/* API Parameters — 非 messages 数组的内容（native FC tools、response_format、instructions） */}
          {currentBreakdown.api_params && currentBreakdown.api_params.length > 0 && (
            <div style={{ marginTop: SPACING.sm }}>
              <div
                onClick={() => setApiParamsExpanded((prev) => !prev)}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: SPACING.sm,
                  margin: `${SPACING.sm}px ${SPACING.xs}px ${SPACING.sm}px`,
                  padding: `6px ${SPACING.sm}px`,
                  borderRadius: RADIUS.md,
                  cursor: 'pointer',
                  userSelect: 'none',
                  transition: `background ${DURATION.fast}s ${EASE.swift}`,
                }}
                onMouseEnter={(e) => { e.currentTarget.style.background = COLORS.bgHover; }}
                onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
              >
                <Wrench size={14} strokeWidth={2} color={COLORS.info} />
                <span style={{ ...TYPO.caption, color: COLORS.info, fontWeight: 700, letterSpacing: 0.3, fontSize: 11 }}>
                  API Parameters
                </span>
                <span style={{ ...TYPO.micro, color: COLORS.textTertiary, fontVariantNumeric: 'tabular-nums' }}>
                  {currentBreakdown.api_params.length} params (non-message)
                </span>
                <span
                  style={{
                    marginLeft: 'auto',
                    display: 'inline-flex',
                    alignItems: 'center',
                    color: COLORS.textTertiary,
                    transition: `transform ${DURATION.normal}s ${EASE.ios}`,
                    transform: apiParamsExpanded ? 'rotate(0deg)' : 'rotate(-90deg)',
                  }}
                >
                  <ChevronDown size={14} strokeWidth={2} />
                </span>
              </div>
              {apiParamsExpanded && currentBreakdown.api_params.map((param, i) => {
                const isOpen = !!expandedApiParam[i];
                return (
                  <div
                    key={i}
                    style={{
                      marginBottom: SPACING.sm,
                      borderRadius: RADIUS.lg,
                      border: `1px solid ${COLORS.info}30`,
                      borderStyle: 'dashed',
                      background: isOpen ? `${COLORS.info}08` : 'transparent',
                      overflow: 'hidden',
                      transition: `all ${DURATION.fast}s ${EASE.swift}`,
                    }}
                  >
                    <div
                      onClick={() => setExpandedApiParam((prev) => ({ ...prev, [i]: !prev[i] }))}
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        gap: SPACING.sm,
                        padding: `${SPACING.sm}px ${SPACING.md}px`,
                        cursor: 'pointer',
                        userSelect: 'none',
                      }}
                    >
                      <span
                        style={{
                          width: 6,
                          height: 6,
                          borderRadius: 3,
                          background: param.present ? COLORS.success : COLORS.textTertiary,
                          flexShrink: 0,
                        }}
                      />
                      <span style={{ ...TYPO.caption, color: COLORS.textPrimary, fontWeight: 600 }}>
                        {param.label}
                      </span>
                      <span
                        style={{
                          ...TYPO.micro,
                          color: param.present ? COLORS.success : COLORS.textTertiary,
                          marginLeft: 'auto',
                          padding: `2px ${SPACING.xs}px`,
                          borderRadius: RADIUS.pill,
                          background: param.present ? `${COLORS.success}15` : `${COLORS.textTertiary}15`,
                        }}
                      >
                        {param.present ? 'sent' : 'not sent'}
                      </span>
                      <ChevronDown
                        size={12}
                        strokeWidth={2}
                        color={COLORS.textTertiary}
                        style={{
                          transition: `transform ${DURATION.fast}s ${EASE.swift}`,
                          transform: isOpen ? 'rotate(180deg)' : 'rotate(0deg)',
                        }}
                      />
                    </div>
                    {isOpen && (
                      <pre
                        style={{
                          margin: 0,
                          padding: SPACING.md,
                          background: COLORS.subtleBg,
                          borderTop: `1px solid ${COLORS.info}15`,
                          fontSize: 11,
                          lineHeight: 1.5,
                          fontFamily: '"SF Mono", "Fira Code", "Cascadia Code", monospace',
                          color: COLORS.textSecondary,
                          overflowX: 'auto',
                          whiteSpace: 'pre-wrap',
                          wordBreak: 'break-word',
                          maxHeight: 400,
                          overflowY: 'auto',
                        }}
                      >
                        {param.content}
                      </pre>
                    )}
                  </div>
                );
              })}
            </div>
          )}

          {/* 分区列表 — 按层级分组 */}
          {layerGroups.map((group) => {
            const groupColor = layerColor(group.layer);
            const isLayerCollapsed = !expandedLayers[group.layer];
            const groupChars = group.items.reduce((sum, it) => sum + it.section.char_count, 0);
            const groupPct = totalChars > 0 ? (groupChars / totalChars) * 100 : 0;
            return (
              <div key={group.layer}>
                {/* 层级分组头 — 可折叠 + 聚合统计 */}
                <div
                  onClick={() => toggleLayer(group.layer)}
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: SPACING.sm,
                    margin: `${SPACING.lg}px ${SPACING.xs}px ${SPACING.sm}px`,
                    padding: `6px ${SPACING.sm}px`,
                    borderRadius: RADIUS.md,
                    cursor: 'pointer',
                    userSelect: 'none',
                    transition: `background ${DURATION.fast}s ${EASE.swift}`,
                  }}
                  onMouseEnter={(e) => { e.currentTarget.style.background = COLORS.bgHover; }}
                  onMouseLeave={(e) => { e.currentTarget.style.background = 'transparent'; }}
                >
                  <span
                    aria-hidden
                    style={{
                      width: 8,
                      height: 8,
                      borderRadius: 4,
                      background: groupColor,
                      boxShadow: `0 0 6px ${groupColor}60`,
                      flexShrink: 0,
                    }}
                  />
                  <span style={{ ...TYPO.caption, color: COLORS.textPrimary, fontWeight: 700, letterSpacing: 0.3, fontSize: 11 }}>
                    {layerLabel(group.layer, t)}
                  </span>
                  <span style={{ ...TYPO.micro, color: COLORS.textTertiary, fontVariantNumeric: 'tabular-nums' }}>
                    {group.items.length} {t('mind_inspector.mind.section_count', { n: '' }).trim()} · {groupChars.toLocaleString()} {t('mind_inspector.common.char_count', { n: '' }).trim()} · {groupPct.toFixed(1)}%
                  </span>
                  <span
                    style={{
                      marginLeft: 'auto',
                      display: 'inline-flex',
                      alignItems: 'center',
                      color: COLORS.textTertiary,
                      transition: `transform ${DURATION.normal}s ${EASE.ios}`,
                      transform: isLayerCollapsed ? 'rotate(-90deg)' : 'rotate(0deg)',
                    }}
                  >
                    <ChevronDown size={14} strokeWidth={2} />
                  </span>
                </div>
                {!isLayerCollapsed && group.items.map(({ section: s, origIdx }) => {
                  const isOpen = !!expanded[origIdx];
                  const pct = totalChars > 0 ? (s.char_count / totalChars) * 100 : 0;
                  const isPostprocess = normalizeLayer(s.layer) === 'postprocess';
                  const color = layerColor(s.layer);
                  const isCopied = copiedIdx === origIdx;
                  const isHot = hoveredSectionIdx === origIdx;
                  const sectionNumber = isPostprocess ? '†' : String(origIdx + 1).padStart(2, '0');
                  return (
                    <div key={s.name} ref={(el) => { sectionRefs.current[origIdx] = el; }}>
                    <Card
                hover
                onClick={() => toggleSection(origIdx, false)}
                style={{
                  position: 'relative',
                  overflow: 'hidden',
                  padding: 0,
                  borderColor: isOpen ? `${color}40` : isPostprocess ? `${color}30` : undefined,
                  borderStyle: isPostprocess ? 'dashed' : undefined,
                  background: isOpen
                    ? (isPostprocess ? `${color}08` : COLORS.bgHover)
                    : isPostprocess ? `${color}04` : undefined,
                  opacity: isPostprocess ? 0.85 : 1,
                }}
                onMouseEnterExternal={() => setHoveredSectionIdx(origIdx)}
                onMouseLeaveExternal={() => setHoveredSectionIdx(null)}
              >
                {/* 顶部进度条（按字符占比填充颜色） */}
                <div
                  aria-hidden
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    right: 0,
                    height: 2,
                    background: COLORS.bgHover,
                  }}
                >
                  <div
                    style={{
                      height: '100%',
                      width: isPostprocess ? '100%' : `${pct}%`,
                      background: isHot
                        ? `linear-gradient(90deg, ${color}, ${color}cc)`
                        : color,
                      borderRadius: `0 ${RADIUS.pill}px ${RADIUS.pill}px 0`,
                      transition: `width ${DURATION.normal}s ${EASE.swift}, background ${DURATION.fast}s ${EASE.swift}`,
                      boxShadow: isHot ? `0 0 8px ${color}80` : 'none',
                    }}
                  />
                </div>

                {/* 卡片主体区域（带内边距） */}
                <div style={{ padding: SPACING.lg }}>
                {/* 编号水印（柔和装饰，悬停时浮现） */}
                <span
                  aria-hidden
                  style={{
                    position: 'absolute',
                    top: -10,
                    right: SPACING.md,
                    fontSize: 80,
                    fontWeight: 900,
                    color: `${color}08`,
                    fontVariantNumeric: 'tabular-nums',
                    lineHeight: 1,
                    pointerEvents: 'none',
                    letterSpacing: '-0.06em',
                    opacity: isHot ? 1 : 0.4,
                    transition: `opacity ${DURATION.normal}s ${EASE.swift}, transform ${DURATION.normal}s ${EASE.swift}`,
                    transform: isHot ? 'scale(1.1)' : 'scale(1)',
                    zIndex: 0,
                  }}
                >
                  {sectionNumber}
                </span>

                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'space-between',
                    gap: SPACING.sm,
                    position: 'relative',
                    zIndex: 1,
                  }}
                >
                  <span style={{ display: 'inline-flex', alignItems: 'center', gap: SPACING.md, minWidth: 0 }}>
                    {/* 编号徽章（更大更突出） */}
                    <span
                      style={{
                        display: 'inline-flex',
                        alignItems: 'center',
                        justifyContent: 'center',
                        width: 32,
                        height: 32,
                        borderRadius: RADIUS.md,
                        background: isOpen
                          ? `linear-gradient(135deg, ${color}30, ${color}15)`
                          : `${color}12`,
                        border: `1.5px solid ${isOpen ? `${color}60` : `${color}25`}`,
                        color: color,
                        fontSize: 12,
                        fontWeight: 800,
                        fontVariantNumeric: 'tabular-nums',
                        flexShrink: 0,
                        letterSpacing: 0,
                        boxShadow: isHot ? `0 0 0 4px ${color}10, 0 2px 8px ${color}20` : 'none',
                        transition: `all ${DURATION.fast}s ${EASE.swift}`,
                      }}
                    >
                      {sectionNumber}
                    </span>
                    <div style={{ display: 'flex', flexDirection: 'column', gap: 2, minWidth: 0 }}>
                      <span
                        style={{
                          ...TYPO.h3,
                          color: isOpen ? COLORS.textPrimary : COLORS.textPrimary,
                          overflow: 'hidden',
                          textOverflow: 'ellipsis',
                          whiteSpace: 'nowrap',
                          fontSize: 15,
                          fontWeight: 700,
                        }}
                      >
                        {sectionLabel(s.name, t)}
                      </span>
                      <span
                        style={{
                          ...TYPO.micro,
                          color: COLORS.textQuaternary,
                          fontVariantNumeric: 'tabular-nums',
                        }}
                      >
                        {s.char_count.toLocaleString()} {t('mind_inspector.common.char_count', { n: '' }).trim()}
                      </span>
                    </div>
                  </span>
                  <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.sm, flexShrink: 0 }}>
                    {/* 占比胶囊（postprocess 不显示百分比） */}
                    {!isPostprocess && (
                    <span
                      style={{
                        ...TYPO.caption,
                        color: color,
                        fontWeight: 700,
                        fontVariantNumeric: 'tabular-nums',
                        padding: `4px 10px`,
                        borderRadius: RADIUS.pill,
                        background: isHot ? `${color}20` : `${color}10`,
                        border: `1px solid ${color}25`,
                        fontSize: 12,
                        transition: `all ${DURATION.fast}s ${EASE.swift}`,
                      }}
                    >
                      {pct.toFixed(1)}%
                    </span>
                    )}
                    {isOpen && (
                      <button
                        type="button"
                        onClick={(e) => {
                          e.stopPropagation();
                          copySection(origIdx, templateContent(s.name, s.full_content, isTemplate, t));
                        }}
                        style={{
                          display: 'inline-flex',
                          alignItems: 'center',
                          justifyContent: 'center',
                          width: 28,
                          height: 28,
                          borderRadius: RADIUS.sm,
                          border: `1px solid ${isCopied ? COLORS.success : COLORS.border}`,
                          background: isCopied ? `${COLORS.success}15` : COLORS.subtleBg,
                          color: isCopied ? COLORS.success : COLORS.textTertiary,
                          cursor: 'pointer',
                          transition: `all ${DURATION.fast}s ${EASE.swift}`,
                        }}
                        title={t('mind_inspector.common.copy')}
                      >
                        {isCopied ? <Check size={13} strokeWidth={2} /> : <Copy size={13} strokeWidth={1.5} />}
                      </button>
                    )}
                    <span
                      style={{
                        display: 'inline-flex',
                        alignItems: 'center',
                        justifyContent: 'center',
                        width: 28,
                        height: 28,
                        borderRadius: RADIUS.sm,
                        color: isOpen ? color : COLORS.textTertiary,
                        background: isOpen ? `${color}10` : 'transparent',
                        transition: `all ${DURATION.normal}s ${EASE.ios}`,
                        transform: isOpen ? 'rotate(180deg)' : 'rotate(0deg)',
                      }}
                    >
                      <ChevronDown size={16} strokeWidth={2} />
                    </span>
                  </div>
                </div>
                {/* 预览：等宽字体 + 渐变遮罩 */}
                {!isOpen && s.preview && (
                  <div
                    style={{
                      marginTop: SPACING.md,
                      marginLeft: 32 + SPACING.md,
                      paddingLeft: SPACING.md,
                      borderLeft: `2px solid ${isHot ? `${color}40` : COLORS.border}`,
                      display: 'flex',
                      flexDirection: 'column',
                      gap: 4,
                      transition: `border-color ${DURATION.fast}s ${EASE.swift}`,
                    }}
                  >
                    <div
                      style={{
                        color: COLORS.textSecondary,
                        whiteSpace: 'nowrap',
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        lineHeight: 1.6,
                        fontFamily: TYPO.fontMono,
                        fontSize: 12,
                        opacity: 0.8,
                      }}
                    >
                      {s.preview}
                    </div>
                    <span
                      style={{
                        ...TYPO.micro,
                        color: color,
                        fontSize: 10,
                        letterSpacing: 0.5,
                        textTransform: 'uppercase',
                        fontWeight: 600,
                        opacity: isHot ? 1 : 0.5,
                        transition: `opacity ${DURATION.fast}s ${EASE.swift}`,
                      }}
                    >
                      {t('mind_inspector.mind.click_to_expand')}
                    </span>
                  </div>
                )}
                {/* 展开内容 */}
                {isOpen && isTemplate && s.name === CHARACTER_SECTION_KEY ? (
                  <div style={{ marginTop: SPACING.lg }} onClick={(e) => e.stopPropagation()}>
                    <IdentityEditor
                      sections={personaSections}
                      drafts={personaDrafts}
                      customized={personaCustomized}
                      savingSection={savingSection}
                      onChange={handlePersonaDraftChange}
                      onSave={handleSavePersona}
                      onReset={handleResetPersona}
                      isDirty={isDraftDirty}
                      t={t}
                    />
                  </div>
                ) : isOpen && isTemplate && s.name === 'Examples' ? (
                  <div style={{ marginTop: SPACING.lg }} onClick={(e) => e.stopPropagation()}>
                    <ExamplesEditor t={t} charId={character} />
                  </div>
                ) : isOpen && isTemplate && s.name === 'Style' ? (
                  <div
                    style={{
                      marginTop: SPACING.lg,
                      marginLeft: 32 + SPACING.md,
                      paddingLeft: SPACING.md,
                      borderLeft: `2px solid ${color}40`,
                    }}
                    onClick={(e) => e.stopPropagation()}
                  >
                    {/* 模式切换器 */}
                    {currentBreakdown?.scene_modes && currentBreakdown.scene_modes.length > 0 && (
                      <div style={{ marginBottom: SPACING.md }}>
                        <div
                          style={{
                            ...TYPO.micro,
                            color: COLORS.textTertiary,
                            marginBottom: SPACING.sm,
                            textTransform: 'uppercase',
                            letterSpacing: 0.5,
                            fontWeight: 600,
                            fontSize: 10,
                          }}
                        >
                          Scene Modes
                        </div>
                        <div style={{ display: 'flex', flexWrap: 'wrap', gap: SPACING.xs }}>
                          {currentBreakdown.scene_modes.map((mode) => {
                            const isActive = mode.mode === activeMode;
                            return (
                              <button
                                key={mode.mode}
                                type="button"
                                onClick={() => setActiveMode(mode.mode)}
                                style={{
                                  padding: `4px ${SPACING.sm}px`,
                                  borderRadius: RADIUS.pill,
                                  border: `1px solid ${isActive ? color : COLORS.border}`,
                                  background: isActive ? `${color}20` : 'transparent',
                                  color: isActive ? color : COLORS.textSecondary,
                                  ...TYPO.micro,
                                  cursor: 'pointer',
                                  transition: `all ${DURATION.fast}s ${EASE.swift}`,
                                  textTransform: 'capitalize',
                                  fontSize: 11,
                                  fontWeight: isActive ? 600 : 400,
                                }}
                                onMouseEnter={(e) => {
                                  if (!isActive) {
                                    e.currentTarget.style.borderColor = COLORS.borderHover;
                                    e.currentTarget.style.color = COLORS.textPrimary;
                                  }
                                }}
                                onMouseLeave={(e) => {
                                  if (!isActive) {
                                    e.currentTarget.style.borderColor = COLORS.border;
                                    e.currentTarget.style.color = COLORS.textSecondary;
                                  }
                                }}
                              >
                                {mode.mode}
                              </button>
                            );
                          })}
                        </div>
                      </div>
                    )}
                    <pre
                      style={{
                        margin: 0,
                        paddingBottom: SPACING.xs,
                        ...TYPO.micro,
                        fontFamily: TYPO.fontMono,
                        color: COLORS.textSecondary,
                        whiteSpace: 'pre-wrap',
                        wordBreak: 'break-word',
                        lineHeight: 1.7,
                        maxHeight: 600,
                        overflowY: 'auto',
                        fontSize: 12,
                      }}
                    >
                      {templateContent(s.name, buildStyleContent(s), isTemplate, t)}
                    </pre>
                  </div>
                ) : isOpen && isTemplate && s.name === 'Tools' ? (
                  <div
                    style={{
                      marginTop: SPACING.lg,
                      marginLeft: 32 + SPACING.md,
                      paddingLeft: SPACING.md,
                      borderLeft: `2px solid ${color}40`,
                    }}
                    onClick={(e) => e.stopPropagation()}
                  >
                    <div style={{ marginBottom: SPACING.sm, ...TYPO.micro, color: COLORS.textTertiary, fontSize: 11 }}>
                      Runtime note: tools are contextually filtered per turn. This list shows all registered tools.
                    </div>
                    {(() => {
                      const toolsByCat: Record<string, ToolPreviewInfo[]> = {};
                      for (const tool of previewTools) {
                        const cat = tool.category || 'other';
                        if (!toolsByCat[cat]) toolsByCat[cat] = [];
                        toolsByCat[cat].push(tool);
                      }
                      const catOrder = ['system', 'file', 'memory', 'web', 'media', 'pet', 'mcp'];
                      const cats = catOrder.filter(c => toolsByCat[c]);
                      const otherCats = Object.keys(toolsByCat).filter(c => !catOrder.includes(c));
                      const allCats = [...cats, ...otherCats];
                      return (
                        <div style={{ display: 'flex', flexDirection: 'column', gap: SPACING.sm, width: '100%', maxHeight: 600, overflowY: 'auto', paddingRight: SPACING.sm }}>
                          {allCats.map((cat, catIdx) => {
                            const isCatOpen = expandedToolCats[cat] ?? catIdx === 0;
                            return (
                              <div key={cat} style={{
                                width: '100%',
                                flexShrink: 0,
                                borderRadius: RADIUS.sm,
                                border: `1px solid ${isCatOpen ? `${color}30` : COLORS.border}`,
                                overflow: 'hidden',
                                transition: `border-color ${DURATION.fast}s ${EASE.swift}`,
                              }}>
                                <div
                                  onClick={() => setExpandedToolCats(prev => ({ ...prev, [cat]: !isCatOpen }))}
                                  style={{
                                    display: 'flex',
                                    alignItems: 'center',
                                    gap: SPACING.sm,
                                    padding: `${SPACING.sm}px ${SPACING.md}px`,
                                    cursor: 'pointer',
                                    userSelect: 'none',
                                    background: isCatOpen ? `${color}08` : 'transparent',
                                    transition: `background ${DURATION.fast}s ${EASE.swift}`,
                                  }}
                                >
                                  <span style={{
                                    display: 'flex',
                                    color: color,
                                    transform: isCatOpen ? 'rotate(0deg)' : 'rotate(-90deg)',
                                    transition: `transform ${DURATION.normal}s ${EASE.ios}`,
                                  }}>
                                    <ChevronDown size={14} strokeWidth={2} />
                                  </span>
                                  <span style={{
                                    ...TYPO.micro,
                                    fontWeight: 600,
                                    fontSize: 11,
                                    textTransform: 'uppercase',
                                    letterSpacing: 0.5,
                                    color: color,
                                    flex: 1,
                                  }}>
                                    {categoryLabels[cat] || cat}
                                  </span>
                                  <span style={{ ...TYPO.micro, fontSize: 10, color: COLORS.textTertiary }}>
                                    {toolsByCat[cat].length}
                                  </span>
                                </div>
                                {isCatOpen && (
                                  <div style={{ display: 'flex', flexDirection: 'column', gap: 4, width: '100%', padding: `${SPACING.xs}px ${SPACING.sm}px ${SPACING.sm}px` }}>
                                    {toolsByCat[cat].map(tool => {
                                      const props = tool.input_schema?.properties || {};
                                      const required = tool.input_schema?.required || [];
                                      const paramEntries = Object.entries(props);
                                      return (
                                        <div key={tool.name} style={{
                                          width: '100%',
                                          flexShrink: 0,
                                          padding: `6px ${SPACING.sm}px`,
                                          borderRadius: RADIUS.sm,
                                          background: `${COLORS.bgSurface}80`,
                                          border: `1px solid ${COLORS.border}`,
                                        }}>
                                          <div style={{ display: 'flex', alignItems: 'baseline', gap: SPACING.sm, flexWrap: 'wrap', marginBottom: paramEntries.length > 0 ? 4 : 0 }}>
                                            <code style={{
                                              ...TYPO.micro,
                                              fontFamily: TYPO.fontMono,
                                              fontWeight: 600,
                                              color: COLORS.textPrimary,
                                              fontSize: 12,
                                              background: `${color}15`,
                                              padding: '1px 5px',
                                              borderRadius: 3,
                                            }}>{tool.name}</code>
                                            {tool.is_read_only && (
                                              <span style={{ ...TYPO.micro, fontSize: 9, color: COLORS.textTertiary, background: COLORS.bgSurface, padding: '0 4px', borderRadius: 2, border: `1px solid ${COLORS.border}` }}>read-only</span>
                                            )}
                                            <span style={{ ...TYPO.micro, color: COLORS.textSecondary, fontSize: 13, lineHeight: 1.5 }}>{tool.description}</span>
                                          </div>
                                          {paramEntries.length > 0 && (
                                            <div style={{ paddingLeft: SPACING.sm, display: 'flex', flexDirection: 'column', gap: 2 }}>
                                              {paramEntries.map(([pname, pinfo]) => {
                                                const isReq = required.includes(pname);
                                                const parts: string[] = [];
                                                if (pinfo.type) parts.push(pinfo.type);
                                                if (pinfo.enum?.length) parts.push(`enum: ${pinfo.enum.join('|')}`);
                                                if (pinfo.minimum !== undefined) parts.push(`min: ${pinfo.minimum}`);
                                                if (pinfo.maximum !== undefined) parts.push(`max: ${pinfo.maximum}`);
                                                if (pinfo.default !== undefined) parts.push(`default: ${String(pinfo.default)}`);
                                                return (
                                                  <div key={pname} style={{ display: 'flex', alignItems: 'baseline', gap: SPACING.xs, ...TYPO.micro, fontSize: 12 }}>
                                                    <code style={{
                                                      fontFamily: TYPO.fontMono,
                                                      color: isReq ? COLORS.textPrimary : COLORS.textSecondary,
                                                      fontWeight: isReq ? 600 : 400,
                                                      fontSize: 12,
                                                    }}>{pname}</code>
                                                    {isReq && <span style={{ color: '#FF3B30', fontSize: 9, fontWeight: 600 }}>*</span>}
                                                    {parts.length > 0 && (
                                                      <span style={{ color: COLORS.textTertiary, fontSize: 10, fontStyle: 'italic' }}>({parts.join(', ')})</span>
                                                    )}
                                                    <span style={{ color: COLORS.textSecondary, fontSize: 12 }}>— {pinfo.description || ''}</span>
                                                  </div>
                                                );
                                              })}
                                            </div>
                                          )}
                                        </div>
                                      );
                                    })}
                                  </div>
                                )}
                              </div>
                            );
                          })}
                        </div>
                      );
                    })()}
                  </div>
                ) : isOpen ? (
                  <div
                    style={{
                      marginTop: SPACING.lg,
                      marginLeft: 32 + SPACING.md,
                      paddingLeft: SPACING.md,
                      borderLeft: `2px solid ${color}40`,
                    }}
                    onClick={(e) => e.stopPropagation()}
                  >
                    <pre
                      style={{
                        margin: 0,
                        paddingBottom: SPACING.xs,
                        ...TYPO.micro,
                        fontFamily: TYPO.fontMono,
                        color: COLORS.textSecondary,
                        whiteSpace: 'pre-wrap',
                        wordBreak: 'break-word',
                        lineHeight: 1.7,
                        maxHeight: 600,
                        overflowY: 'auto',
                        fontSize: 12,
                      }}
                    >
                      {templateContent(s.name, s.full_content, isTemplate, t)}
                    </pre>
                  </div>
                ) : null}
                </div>{/* end padding wrapper */}
              </Card>
              </div>
                  );
                })}
              </div>
            );
          })}
          {hiddenCount > 0 && (
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                gap: SPACING.sm,
                margin: `${SPACING.md}px ${SPACING.xs}px`,
                padding: `${SPACING.sm}px ${SPACING.md}px`,
                borderRadius: RADIUS.md,
                border: `1px dashed ${COLORS.border}`,
                color: COLORS.textTertiary,
                ...TYPO.micro,
              }}
            >
              {t('mind_inspector.mind.hidden_empty_sections', { n: hiddenCount })}
            </div>
          )}
        </>
      )}
    </div>
  );
};

const MemoContextPipelineView = React.memo(ContextPipelineView);
// ============================================================
// MindPage — 主页面
// ============================================================

const MindPage: React.FC = () => {
  const { t } = useTranslation();
  const [view, setView] = useState<SubView>('live');
  const SUB_VIEWS: Array<{ key: SubView; label: string }> = [
    { key: 'live', label: t('mind_inspector.mind.sub_live') },
    { key: 'pipeline', label: t('mind_inspector.mind.sub_pipeline') },
  ];

  return (
    <div
      style={{
        flex: 1,
        overflowY: 'auto',
        padding: SPACING.lg,
        display: 'flex',
        flexDirection: 'column',
        gap: SPACING.md,
      }}
    >
      {/* 顶部：子视图切换（去掉冗余副标题，Large Title "心智" 已在 MindInspector 中显示） */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'flex-end',
          gap: SPACING.md,
          flexWrap: 'wrap',
        }}
      >
        <MemoSegmentedControl<SubView> options={SUB_VIEWS} value={view} onChange={setView} />
      </div>

      {/* 子视图内容 */}
      {view === 'live' && <MemoLiveMindView />}
      {view === 'pipeline' && <MemoContextPipelineView />}
    </div>
  );
};

export default MindPage;
