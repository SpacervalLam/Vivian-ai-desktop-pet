/**
 * Graph 页 — 记忆时间线图谱
 *
 * 数据源（两层）：
 * - 骨架层:  invoke('get_graph_timeline', { characterId })  全部记忆+日记的轻量 {id, ts, kind}
 *            驱动时间比例尺与迷你地图，不含内容
 * - 内容层:  invoke('get_memories_range' / 'get_diary_range', { characterId, after, before })
 *            按可见时间窗口懒加载完整内容，缓存累积
 * - Beliefs:      invoke('list_beliefs', { characterId })
 * - Relationship: invoke('list_relationship_facts', { characterId })
 * - Goals:        invoke('get_mind_state', { characterId }) 取 goals 字段
 *
 * 布局：左侧垂直时间轴（旧→新从下到上），节点按时间比例定位（间隙压缩），左右交替分布
 * 节点类型：User / Agent / Belief / Episode / Goal / Relationship
 * 交互：悬停 tooltip、点击高亮、拖拽微调位置、迷你地图日期跳转
 */

import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { open as openShell } from '@tauri-apps/plugin-shell';
import {
  COLORS,
  TYPO,
  SPACING,
  RADIUS,
  EASE,
  DURATION,
  CHARACTER_ACCENT,
} from '../design-system';
import {
  EmptyState,
} from '../shared-components';
import { useNavigation } from '../NavigationContext';
import { AlertCircle } from 'lucide-react';
import type {
  BeliefView,
  RelationshipFact,
  MemoryItem,
  MindState,
} from '../../../types';
import type {
  CharacterId,
  NodeType,
  GraphNode,
  GraphEdge,
  LayoutNode,
  TimeTick,
} from './graph/types';
import {
  memoryToGraphNodes,
  diaryToGraphNode,
  parseTimestamp,
} from './graph/classifyMemory';
import { renderTextWithActions } from '../../../utils/ActionText';
import { buildTimeScale } from './graph/timeScale';
import type { TimeScale } from './graph/timeScale';
import { niceTicks } from './graph/niceTicks';
import { missingRanges, insertRange } from './graph/intervals';
import type { Interval } from './graph/intervals';
import { handDrawnLoopPath, handDrawnArrowPath } from './graph/handDrawn';
import Minimap from './graph/Minimap';

// ============================================================
// 类型 & 常量
// ============================================================

/** 图谱骨架点 — 仅含 id/时间戳/类型，驱动时间比例尺与迷你地图，不含内容 */
interface SkeletonPoint {
  id: string; // 节点 id（episode:{id} 或 diary:{id}）
  ts: number; // 时间戳（毫秒）
  kind: 'memory' | 'diary';
}

/** 会话分组：对话节点按时间邻近聚合，圈内纳入时间段内的所有内容节点 */
interface SessionGroup {
  id: string; // 稳定种子（由首个对话节点 id 派生）
  dialogueIds: string[];
  memberIds: string[];
  startTs: number;
  endTs: number;
}

/** 回应箭头：from 是回复方节点，to 是被回应的节点 */
interface ReplyArrow {
  id: string;
  fromId: string;
  toId: string;
}

/** 会话切分阈值：相邻对话节点时间间隔超过该值视为新会话 */
const SESSION_GAP_MS = 20 * 60 * 1000;

/** 日记内容缓存条目（图谱节点所需字段） */
interface DiaryEntryLite {
  id: string;
  date: string;
  content: string;
  key_events: string[];
  mood_tag: string;
  created_at: number;
  word_count: number;
  interaction_count: number;
}

const MIN_SIZE = 18;
const MAX_SIZE = 36;

const TIMELINE_X = 400;
const NODE_OFFSET_X = 140;
const SUMMARY_EXTRA_OFFSET = 45;
const CORE_USER_X = 90;
const CORE_AGENT_X = 400;
const CORE_ROOMMATE_X = 710;
const CORE_Y = 55;
const CORE_Y_SECONDARY = 90;
const TOP_PADDING = 150;
const BOTTOM_PADDING = 60;

const SPRING_K = 0.08;
const SPRING_DAMPING = 0.72;
const NEIGHBOR_STRENGTH = 0.04;
const SPRING_THRESHOLD = 0.3;

const NODE_TYPE_KEYS: Record<NodeType, string> = {
  user: 'type_user',
  agent: 'type_agent',
  belief: 'type_belief',
  episode: 'type_episode',
  dialogue: 'type_dialogue',
  wechat: 'type_wechat',
  topic_summary: 'type_topic_summary',
  important_event: 'type_important_event',
  goal: 'type_goal',
  relationship: 'type_relationship',
  inner_thought: 'type_inner_thought',
  diary: 'type_diary',
  reading: 'type_reading',
  session_summary: 'type_session_summary',
};

const nodeSize = (importance: number): number =>
  MIN_SIZE + (MAX_SIZE - MIN_SIZE) * Math.max(0, Math.min(1, importance));

// === 手账视觉常量（与 DiaryPage 保持一致）===
const GPAPER = {
  paper: 'var(--graph-paper)',
  card: 'var(--graph-card)',
  ink: 'var(--graph-ink)',
  inkSoft: 'var(--graph-ink-soft)',
  inkFaint: 'var(--graph-ink-faint)',
  stampRed: 'var(--graph-stamp-red)',
  grid: 'var(--graph-grid)',
  shadowSm: 'var(--graph-shadow-sm)',
  shadowMd: 'var(--graph-shadow-md)',
  shadowLg: 'var(--graph-shadow-lg)',
  border: 'var(--graph-border)',
  line: 'var(--graph-line)',
} as const;

const HAND =
  '"Caveat", "Ma Shan Zheng", "Dancing Script", "Hachi Maru Pop", "Kaiti SC", "KaiTi", "STKaiti", "DFKai-SB", "PingFang SC", "Microsoft YaHei", serif';

// 主题目标色缓存：避免每次 pastel() 调用都触发 getComputedStyle 强制布局重算
// （该函数在拖拽动画期间 60fps 调用，原实现每秒 ~1260 次 getComputedStyle 调用）
let pastelTargetCache: [number, number, number] | null = null;
const readPastelTarget = (): [number, number, number] => {
  if (pastelTargetCache) return pastelTargetCache;
  if (typeof document !== 'undefined') {
    const raw = getComputedStyle(document.documentElement)
      .getPropertyValue('--graph-pastel-target')
      .trim()
      .split(',')
      .map(Number);
    if (raw.length === 3) {
      pastelTargetCache = [raw[0], raw[1], raw[2]];
      return pastelTargetCache;
    }
  }
  pastelTargetCache = [250, 245, 235];
  return pastelTargetCache;
};
// 主题切换时调用以失效缓存（重新读取 CSS 变量）
export const invalidatePastelCache = () => {
  pastelTargetCache = null;
};

/** 把分类色向主题背景调和为淡彩贴纸色 */
const pastel = (hex: string, mix = 0.42): string => {
  const h = hex.replace('#', '');
  const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  const r = parseInt(full.slice(0, 2), 16);
  const g = parseInt(full.slice(2, 4), 16);
  const b = parseInt(full.slice(4, 6), 16);
  const [tr, tg, tb] = readPastelTarget();
  const rr = Math.round(r + (tr - r) * mix);
  const gg = Math.round(g + (tg - g) * mix);
  const bb = Math.round(b + (tb - b) * mix);
  return `rgb(${rr},${gg},${bb})`;
};

/** 由节点 id 生成确定性的轻微旋转角（-2.5° ~ 2.5°），营造随手贴纸感 */
const stickerTilt = (id: string): number => {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) | 0;
  return ((Math.abs(h) % 100) / 100 - 0.5) * 5;
};

/** 角色和纸胶带色（与 DiaryPage 保持一致） */
const CHAR_TAPE: Record<CharacterId, string> = {
  vivian: '#FFE88A',
  nana: '#DDC6FF',
};

// ============================================================
// 工具函数
// ============================================================

const hexToRgba = (hex: string, alpha: number): string => {
  const h = hex.replace('#', '');
  const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  const r = parseInt(full.slice(0, 2), 16);
  const g = parseInt(full.slice(2, 4), 16);
  const b = parseInt(full.slice(4, 6), 16);
  return `rgba(${r},${g},${b},${alpha})`;
};

const formatTimeLabel = (ts: number, now: number): string => {
  const diff = now - ts;
  const minute = 60 * 1000;
  const hour = 60 * minute;
  const day = 24 * hour;
  if (diff < minute) return '刚刚';
  if (diff < hour) return `${Math.floor(diff / minute)}分钟前`;
  if (diff < day) return `${Math.floor(diff / hour)}小时前`;
  if (diff < 7 * day) return `${Math.floor(diff / day)}天前`;
  const d = new Date(ts);
  return `${d.getMonth() + 1}/${d.getDate()}`;
};

/** 估算文本渲染宽度：CJK/全角约 1em，其余约 0.58em */
const estimateTextWidth = (text: string, fontSize: number): number => {
  let w = 0;
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    const wide =
      (code >= 0x1100 && code <= 0x115f) ||
      (code >= 0x2e80 && code <= 0x303e) ||
      (code >= 0x3041 && code <= 0x33ff) ||
      (code >= 0x3400 && code <= 0x4dbf) ||
      (code >= 0x4e00 && code <= 0x9fff) ||
      (code >= 0xac00 && code <= 0xd7af) ||
      (code >= 0xf900 && code <= 0xfaff) ||
      (code >= 0xfe30 && code <= 0xfe4f) ||
      (code >= 0xff00 && code <= 0xff60) ||
      (code >= 0xffe0 && code <= 0xffe6) ||
      (code >= 0x20000 && code <= 0x2fffd) ||
      (code >= 0x30000 && code <= 0x3fffd);
    w += wide ? fontSize : fontSize * 0.58;
  }
  return w;
};

/** 将文本拆分为普通片段和括号内片段 */
interface TextSegment {
  type: 'normal' | 'paren';
  text: string;
}
function splitParenText(text: string): TextSegment[] {
  const segments: TextSegment[] = [];
  let buf = '';
  let inParen = false;
  let parenCount = 0;
  for (const ch of text) {
    if (ch === '(' || ch === '（') {
      if (!inParen) {
        if (buf) segments.push({ type: 'normal', text: buf });
        buf = '';
        inParen = true;
        parenCount = 1;
      } else {
        parenCount++;
        buf += ch;
      }
    } else if (ch === ')' || ch === '）') {
      if (inParen) {
        parenCount--;
        if (parenCount === 0) {
          if (buf) segments.push({ type: 'paren', text: buf });
          buf = '';
          inParen = false;
        } else {
          buf += ch;
        }
      } else {
        buf += ch;
      }
    } else {
      buf += ch;
    }
  }
  if (buf) segments.push({ type: inParen ? 'paren' : 'normal', text: buf });
  return segments;
}

/** 渲染带括号特殊样式的文本（压缩连续空行，避免 pre-wrap 下出现大段空白） */
function renderParenText(text: string, normalStyle?: React.CSSProperties, parenStyle?: React.CSSProperties): React.ReactNode {
  const normalized = text.replace(/\n{2,}/g, '\n').trim();
  const segs = splitParenText(normalized);
  return segs.map((seg, i) => (
    <span key={i} style={{ whiteSpace: 'pre-wrap', ...(seg.type === 'paren' ? parenStyle : normalStyle) }}>
      {seg.text}
    </span>
  ));
}

// ============================================================
// TapeTab — 和纸胶带角色切换
// ============================================================

interface TapeTabProps {
  label: string;
  color: string;
  rot: number;
  active: boolean;
  onClick: () => void;
}

const TapeTab: React.FC<TapeTabProps> = ({ label, color, rot, active, onClick }) => {
  const [hovered, setHovered] = useState(false);
  return (
    <button
      type="button"
      onClick={onClick}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        position: 'relative',
        padding: '7px 24px 6px',
        border: 'none',
        borderRadius: 3,
        cursor: 'pointer',
        background: `repeating-linear-gradient(45deg, rgba(255,255,255,0.3) 0 5px, rgba(255,255,255,0) 5px 10px), ${color}`,
        opacity: active ? 1 : hovered ? 0.85 : 0.55,
        transform: `rotate(${rot}deg) scale(${active ? 1.05 : 1})`,
        boxShadow: active
          ? GPAPER.shadowMd
          : GPAPER.shadowSm,
        transition: `all ${DURATION.normal}s ${EASE.spring}`,
        fontFamily: TYPO.fontFamilyCN,
        fontSize: 17,
        letterSpacing: 3,
        color: '#3B3428',
      }}
    >
      {label}
      {active && (
        <span
          aria-hidden
          style={{
            position: 'absolute',
            top: -4,
            right: -4,
            width: 11,
            height: 11,
            borderRadius: 999,
            background: `radial-gradient(circle at 35% 30%, #E9776F, ${GPAPER.stampRed})`,
            boxShadow: GPAPER.shadowSm,
          }}
        />
      )}
    </button>
  );
};

const MemoTapeTab = React.memo(TapeTab);

// ============================================================
// StickerChip — 贴纸筹码（底部统计）
// ============================================================

interface StickerChipProps {
  color: string;
  rot: number;
  children: React.ReactNode;
}

const StickerChip: React.FC<StickerChipProps> = ({ color, rot, children }) => (
  <span
    style={{
      display: 'inline-flex',
      alignItems: 'center',
      gap: 7,
      padding: '5px 13px',
      borderRadius: 4,
      background: 'var(--graph-card-92)',
      border: `1px dashed ${GPAPER.border}`,
      boxShadow: GPAPER.shadowSm,
      transform: `rotate(${rot}deg)`,
      fontFamily: HAND,
      fontSize: 14.5,
      letterSpacing: 0.5,
      color: GPAPER.ink,
      whiteSpace: 'nowrap',
    }}
  >
    <span
      aria-hidden
      style={{
        width: 9,
        height: 9,
        borderRadius: 999,
        background: pastel(color),
        border: `1px solid ${hexToRgba(color, 0.5)}`,
        flexShrink: 0,
      }}
    />
    {children}
  </span>
);

// ============================================================
// NodeShape
// ============================================================

interface NodeShapeProps {
  type: NodeType;
  color: string;
  size: number;
  x: number;
  y: number;
  isCore?: boolean;
  expanded?: boolean;
}

const NodeShape: React.FC<NodeShapeProps> = ({ type, color, size, x, y, isCore, expanded }) => {
  const r = size / 2;

  const shapeProps = {
    filter: 'url(#sticker-shadow)',
  };

  switch (type) {
    case 'user':
    case 'agent': {
      return (
        <g {...shapeProps}>
          <circle
            cx={x}
            cy={y}
            r={r}
            fill={pastel(color)}
            stroke={GPAPER.card}
            strokeWidth={2.5}
          />
        </g>
      );
    }
    case 'belief':
      return (
        <g {...shapeProps}>
          <rect x={x - r} y={y - r} width={size} height={size} fill={pastel(color)} rx={3} stroke={GPAPER.card} strokeWidth={2.5} />
          <rect x={x - r + 3} y={y - r + 3} width={size - 6} height={(size - 6) * 0.34} fill={hexToRgba('#ffffff', 0.4)} rx={2} />
        </g>
      );
    case 'diary': {
      const bookW = r * 1.45;
      const bookH = r * 1.1;
      const spineX = x - bookW * 0.15;
      const pageW = bookW * 0.85;
      const pageH = bookH * 0.82;
      const pageX = spineX + bookW * 0.08;
      const pageY = y - bookH * 0.35;
      return (
        <g {...shapeProps}>
          <rect
            x={x - bookW / 2}
            y={y - bookH / 2}
            width={bookW}
            height={bookH}
            fill={pastel(color)}
            rx={2}
            stroke={GPAPER.card}
            strokeWidth={2.5}
          />
          <line
            x1={spineX}
            y1={y - bookH / 2 + 2}
            x2={spineX}
            y2={y + bookH / 2 - 2}
            stroke={'var(--graph-ink-25)'}
            strokeWidth={1}
          />
          <rect
            x={pageX}
            y={pageY}
            width={pageW}
            height={pageH}
            fill={GPAPER.card}
            rx={1}
          />
          <line
            x1={pageX + 3}
            y1={pageY + pageH * 0.25}
            x2={pageX + pageW - 3}
            y2={pageY + pageH * 0.25}
            stroke={hexToRgba(color, 0.45)}
            strokeWidth={0.6}
          />
          <line
            x1={pageX + 3}
            y1={pageY + pageH * 0.45}
            x2={pageX + pageW - 6}
            y2={pageY + pageH * 0.45}
            stroke={hexToRgba(color, 0.35)}
            strokeWidth={0.6}
          />
          <line
            x1={pageX + 3}
            y1={pageY + pageH * 0.65}
            x2={pageX + pageW - 4}
            y2={pageY + pageH * 0.65}
            stroke={hexToRgba(color, 0.3)}
            strokeWidth={0.6}
          />
          <circle
            cx={x + bookW * 0.3}
            cy={y - bookH * 0.2}
            r={r * 0.18}
            fill={'var(--graph-stamp-85)'}
          />
        </g>
      );
    }
    case 'reading': {
      // 阅读/链接分享：圆形背景 + 🔗 图标
      return (
        <g {...shapeProps}>
          <circle cx={x} cy={y} r={r} fill={pastel(color)} stroke={GPAPER.card} strokeWidth={2.5} />
          <text
            x={x}
            y={y}
            textAnchor="middle"
            dominantBaseline="central"
            fontSize={r * 1.05}
            style={{ userSelect: 'none' }}
          >
            🔗
          </text>
        </g>
      );
    }
    case 'episode': {
      const dr = r * 0.85;
      const pts = `${x},${y - dr} ${x + dr},${y} ${x},${y + dr} ${x - dr},${y}`;
      return (
        <g {...shapeProps}>
          <polygon points={pts} fill={pastel(color)} stroke={GPAPER.card} strokeWidth={2.5} strokeLinejoin="round" />
        </g>
      );
    }
    case 'session_summary': {
      // 会话摘要：菱形 + 内部 +/- 指示器，表示可展开/收起
      const dr = r * 0.9;
      const pts = `${x},${y - dr} ${x + dr},${y} ${x},${y + dr} ${x - dr},${y}`;
      const indicatorSize = r * 0.35;
      return (
        <g {...shapeProps}>
          <polygon points={pts} fill={pastel(color)} stroke={color} strokeWidth={2} strokeLinejoin="round" />
          {/* 展开/收起指示器：+ 表示可展开，− 表示可收起 */}
          <line
            x1={x - indicatorSize} y1={y} x2={x + indicatorSize} y2={y}
            stroke="#FFFFFF" strokeWidth={2} strokeLinecap="round"
          />
          {!expanded && (
            <line
              x1={x} y1={y - indicatorSize} x2={x} y2={y + indicatorSize}
              stroke="#FFFFFF" strokeWidth={2} strokeLinecap="round"
            />
          )}
        </g>
      );
    }
    case 'dialogue':
    case 'topic_summary': {
      // 微信风格消息气泡：贝塞尔曲线单一路径，椭圆主体 + 三角尾巴
      const tailDir = type === 'dialogue' ? -1 : 1; // -1=尾巴朝左, 1=尾巴朝右
      const rx = r;
      const ry = r * 0.75;
      const cx = x;
      const cy = y - r * 0.15;

      // 尾巴参数
      const tipX = cx + tailDir * rx * 0.55;
      const tipY = cy + ry + r * 0.5;
      // 缺口两点：尾巴方向的近端和远端
      const notchNearX = cx + tailDir * rx * 0.05;
      const notchFarX = cx + tailDir * rx * 0.35;

      const kappa = 0.5522847498;
      const kx = rx * kappa;
      const ky = ry * kappa;

      // 路径顺序：顶 → 远侧弧 → 缺口远端 → 尾巴尖 → 缺口近端 → 近侧弧 → 顶
      const d = [
        `M ${cx},${cy - ry}`,
        `C ${cx - tailDir * kx},${cy - ry} ${cx - tailDir * rx},${cy - ky} ${cx - tailDir * rx},${cy}`,
        `C ${cx - tailDir * rx},${cy + ky} ${cx - tailDir * kx},${cy + ry} ${notchFarX},${cy + ry}`,
        `L ${tipX},${tipY}`,
        `L ${notchNearX},${cy + ry}`,
        `C ${cx + tailDir * kx},${cy + ry} ${cx + tailDir * rx},${cy + ky} ${cx + tailDir * rx},${cy}`,
        `C ${cx + tailDir * rx},${cy - ky} ${cx + tailDir * kx},${cy - ry} ${cx},${cy - ry}`,
        'Z',
      ].join(' ');

      return (
        <g {...shapeProps}>
          <path d={d} fill={pastel(color)} stroke={GPAPER.card} strokeWidth={2.5} strokeLinejoin="round" />
        </g>
      );
    }
    case 'wechat': {
      // 信封图标：圆角矩形主体 + 折叠盖 V 形
      const ew = r * 1.5;
      const eh = r * 1.0;
      const ex = x - ew / 2;
      const ey = y - eh / 2;
      const er = r * 0.18;
      const foldY = ey + eh * 0.42;
      const c = pastel(color);
      return (
        <g {...shapeProps}>
          <rect x={ex} y={ey} width={ew} height={eh} rx={er} fill={c} stroke={GPAPER.card} strokeWidth={2.5} />
          {/* 信封盖 V 形折线 */}
          <path
            d={`M ${ex} ${foldY} L ${x} ${ey + eh * 0.08} L ${ex + ew} ${foldY}`}
            fill="none"
            stroke={GPAPER.card}
            strokeWidth={1.8}
            strokeLinecap="round"
            strokeLinejoin="round"
          />
          {/* 信封底部倒 V */}
          <path
            d={`M ${ex + ew * 0.18} ${ey + eh} L ${x} ${foldY + eh * 0.32} L ${ex + ew * 0.82} ${ey + eh}`}
            fill="none"
            stroke={'var(--graph-card-50)'}
            strokeWidth={1.2}
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </g>
      );
    }
    case 'important_event': {
      const outer: string[] = [];
      for (let i = 0; i < 10; i++) {
        const a = (-90 + 36 * i) * (Math.PI / 180);
        const rr = i % 2 === 0 ? r : r * 0.45;
        outer.push(`${(x + rr * Math.cos(a)).toFixed(2)},${(y + rr * Math.sin(a)).toFixed(2)}`);
      }
      return (
        <g {...shapeProps}>
          <polygon points={outer.join(' ')} fill={pastel(color)} stroke={GPAPER.card} strokeWidth={2.5} strokeLinejoin="round" />
          <circle cx={x} cy={y} r={r * 0.26} fill={hexToRgba('#ffffff', 0.55)} />
        </g>
      );
    }
    case 'goal': {
      const pts: string[] = [];
      for (let i = 0; i < 6; i++) {
        const a = (-90 + 60 * i) * (Math.PI / 180);
        pts.push(`${(x + r * Math.cos(a)).toFixed(2)},${(y + r * Math.sin(a)).toFixed(2)}`);
      }
      return (
        <g {...shapeProps}>
          <polygon points={pts.join(' ')} fill={pastel(color)} stroke={GPAPER.card} strokeWidth={2.5} strokeLinejoin="round" />
          <circle cx={x} cy={y} r={r * 0.3} fill={hexToRgba('#ffffff', 0.4)} />
        </g>
      );
    }
    case 'relationship':
      return (
        <g {...shapeProps}>
          <circle cx={x} cy={y} r={r} fill={GPAPER.card} stroke={pastel(color)} strokeWidth={3} />
          <circle cx={x} cy={y} r={r * 0.42} fill={pastel(color)} />
        </g>
      );
    case 'inner_thought': {
      const cR = r * 0.88;
      const cX = x;
      const cY = y - r * 0.02;
      const lobes = [
        { cx: cX - cR * 0.5, cy: cY + cR * 0.18, rx: cR * 0.48, ry: cR * 0.4 },
        { cx: cX + cR * 0.48, cy: cY + cR * 0.2, rx: cR * 0.44, ry: cR * 0.37 },
        { cx: cX - cR * 0.12, cy: cY - cR * 0.22, rx: cR * 0.5, ry: cR * 0.46 },
        { cx: cX + cR * 0.22, cy: cY - cR * 0.05, rx: cR * 0.42, ry: cR * 0.38 },
        { cx: cX, cy: cY + cR * 0.08, rx: cR * 0.68, ry: cR * 0.46 },
      ];
      const c = pastel(color);
      return (
        <g {...shapeProps}>
          {lobes.map((l, i) => (
            <ellipse key={`cb-${i}`} cx={l.cx} cy={l.cy} rx={l.rx + 2} ry={l.ry + 2} fill={GPAPER.card} />
          ))}
          {lobes.map((l, i) => (
            <ellipse key={`cf-${i}`} cx={l.cx} cy={l.cy} rx={l.rx} ry={l.ry} fill={c} />
          ))}
        </g>
      );
    }
    default:
      return null;
  }
};

const MemoNodeShape = React.memo(NodeShape);

// ============================================================
// GraphPage
// ============================================================

const GraphPage: React.FC = () => {
  const { t } = useTranslation();
  const nav = useNavigation();
  const [character, setCharacter] = useState<CharacterId>('vivian');
  const [beliefs, setBeliefs] = useState<BeliefView[]>([]);
  const [relationships, setRelationships] = useState<RelationshipFact[]>([]);
  const [memories, setMemories] = useState<MemoryItem[]>([]);
  const [diaries, setDiaries] = useState<DiaryEntryLite[]>([]);
  const [skeleton, setSkeleton] = useState<SkeletonPoint[]>([]);
  const [cacheVersion, setCacheVersion] = useState(0);
  const [mind, setMind] = useState<MindState | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedNode, setSelectedNode] = useState<string | null>(null);
  // session_summary 节点的展开状态：已展开的 session_summary ID 集合
  const [expandedSummaryIds, setExpandedSummaryIds] = useState<Set<string>>(new Set());
  const [hoveredNode, setHoveredNode] = useState<string | null>(null);
  const [version, setVersion] = useState(0);
  const [modalNode, setModalNode] = useState<GraphNode | null>(null);

  const svgRef = useRef<SVGSVGElement | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const layoutRef = useRef<Map<string, LayoutNode>>(new Map());
  const draggingRef = useRef<{ id: string; offsetX: number; offsetY: number; lastX: number; lastY: number; lastTime: number } | null>(null);
  const dragMovedRef = useRef(false);
  const animatingRef = useRef(false);
  const rafRef = useRef<number | null>(null);
  const neighborMapRef = useRef<Map<string, string[]>>(new Map());
  const sessionPeerRef = useRef<Map<string, string[]>>(new Map());
  const pendingFocusRef = useRef<{ targetChar: CharacterId; timestamp: number; preview: string } | null>(null);
  const [focusPulse, setFocusPulse] = useState<string | null>(null);

  // ── 两层数据懒加载 refs ──
  const charRef = useRef<CharacterId>(character);
  const lastCharRef = useRef<CharacterId | null>(null);
  const loadedMemRangesRef = useRef<Interval[]>([]);
  const loadedDiaryRangesRef = useRef<Interval[]>([]);
  const inFlightMemRef = useRef<Set<string>>(new Set());
  const inFlightDiaryRef = useRef<Set<string>>(new Set());
  const pendingAnchorRef = useRef<number | null>(null);
  const visibleRangeRef = useRef<{ topY: number; bottomY: number }>({ topY: 0, bottomY: 0 });

  const forceUpdate = useCallback(() => setVersion((v) => v + 1), []);

  // ── 视口追踪（渲染虚拟化）──
  // viewVersion 在滚动/尺寸变化时节流自增，触发可见 SVG 范围重算
  const [viewVersion, setViewVersion] = useState(0);
  const [containerHeight, setContainerHeight] = useState(0);
  const scrollRafRef = useRef<number | null>(null);

  useEffect(() => {
    if (containerRef.current) setContainerHeight(containerRef.current.clientHeight);
  }, [viewVersion]);

  const handleScroll = useCallback(() => {
    if (scrollRafRef.current) return;
    scrollRafRef.current = requestAnimationFrame(() => {
      scrollRafRef.current = null;
      setViewVersion((v) => v + 1);
    });
  }, []);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const ro = new ResizeObserver(() => setViewVersion((v) => v + 1));
    ro.observe(container);
    return () => {
      ro.disconnect();
      if (scrollRafRef.current) cancelAnimationFrame(scrollRafRef.current);
    };
  }, []);

  // 数据加载：拉取骨架（驱动时间比例尺）+ 信念/关系/目标；完整内容按视口懒加载
  const fetchData = useCallback(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    setSelectedNode(null);
    charRef.current = character;

    // 刷新（同角色）前记录可见中心时间戳，骨架更新后据此重锚滚动位置
    if (lastCharRef.current === character && containerRef.current) {
      const vr = visibleRangeRef.current;
      const midY = (vr.topY + vr.bottomY) / 2;
      pendingAnchorRef.current = scaleRef.current ? scaleRef.current.yToTs(midY) : null;
    }

    // 切换角色：立即重置滚动位置和视口状态，避免旧角色的图谱高度/滚动位置影响新角色视图
    if (lastCharRef.current !== null && lastCharRef.current !== character) {
      if (containerRef.current) {
        containerRef.current.scrollTop = 0;
      }
      visibleRangeRef.current = { topY: 0, bottomY: 0 };
      pendingAnchorRef.current = null;
      setHoveredNode(null);
    }

    Promise.all([
      invoke<BeliefView[]>('list_beliefs', { characterId: character }).catch(() => [] as BeliefView[]),
      invoke<RelationshipFact[]>('list_relationship_facts', { characterId: character }).catch(
        () => [] as RelationshipFact[],
      ),
      invoke<Array<{ id: string; ts: number; kind: string }>>('get_graph_timeline', { characterId: character }).catch(
        () => [] as Array<{ id: string; ts: number; kind: string }>,
      ),
      invoke<MindState>('get_mind_state', { characterId: character }).catch(() => null),
    ])
      .then(([b, r, timeline, mindState]) => {
        if (cancelled) return;

        // 切换角色：清空内容缓存与加载区间；同角色刷新：保留缓存仅更新骨架
        if (lastCharRef.current !== character) {
          setMemories([]);
          setDiaries([]);
          loadedMemRangesRef.current = [];
          loadedDiaryRangesRef.current = [];
          inFlightMemRef.current.clear();
          inFlightDiaryRef.current.clear();
          setCacheVersion((v) => v + 1);
        }
        lastCharRef.current = character;

        setBeliefs(b ?? []);
        setRelationships((r ?? []).filter((rel) => rel.owner_agent === character));
        setMind(mindState);
        setSkeleton(
          (timeline ?? []).map((p) => ({
            id: p.kind === 'diary' ? `diary:${p.id}` : `episode:${p.id}`,
            ts: p.ts < 1e12 ? p.ts * 1000 : p.ts,
            kind: p.kind === 'diary' ? 'diary' : 'memory',
          })),
        );
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

  // ESC 键关闭弹窗
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && modalNode) {
        setModalNode(null);
        setSelectedNode(null);
      }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [modalNode]);

  // ── 时间比例尺：由完整骨架驱动，内容懒加载不改变布局 ──
  const scale = useMemo<TimeScale>(
    () => buildTimeScale(skeleton.map((p) => p.ts), { topPadding: TOP_PADDING, bottomPadding: BOTTOM_PADDING }),
    [skeleton],
  );
  const scaleRef = useRef<TimeScale>(scale);
  useEffect(() => {
    scaleRef.current = scale;
  }, [scale]);

  // 聚焦到最接近指定时间戳的记忆骨架点（旁观记录跨角色跳转 / 刷新定位用）
  const focusNearestMemory = useCallback(
    (timestamp: number, preview: string) => {
      let bestId: string | null = null;
      let bestIdx = -1;
      let bestScore = Infinity;
      skeleton.forEach((p, i) => {
        if (p.kind !== 'memory') return;
        const dt = Math.abs(p.ts - timestamp);
        let bonus = 0;
        if (preview) {
          const mem = memories.find((m) => `episode:${m.id}` === p.id);
          if (mem && mem.content.includes(preview.slice(0, 8))) bonus = 60000;
        }
        const score = dt - bonus;
        if (score < bestScore) {
          bestScore = score;
          bestId = p.id;
          bestIdx = i;
        }
      });
      if (!bestId) return;
      const targetId = bestId;
      const fallbackY = bestIdx >= 0 ? scale.yAtIndex(bestIdx) : null;
      setSelectedNode(targetId);
      setFocusPulse(targetId);
      setTimeout(() => setFocusPulse(null), 2000);
      requestAnimationFrame(() => {
        const pos = layoutRef.current.get(targetId);
        const container = containerRef.current;
        const y = pos ? pos.y : fallbackY;
        if (y != null && container) {
          const targetY = y - container.clientHeight / 2;
          container.scrollTo({ top: Math.max(0, targetY), behavior: 'smooth' });
        }
      });
    },
    [skeleton, memories, scale],
  );

  // 记忆内容加载完成后聚焦到最近记忆（由外部跳转触发）
  useEffect(() => {
    const pf = pendingFocusRef.current;
    if (!pf || pf.targetChar !== character || skeleton.length === 0 || memories.length === 0) return;
    pendingFocusRef.current = null;
    const { timestamp, preview } = pf;
    requestAnimationFrame(() => focusNearestMemory(timestamp, preview));
  }, [character, skeleton, memories, focusNearestMemory]);

  // 骨架索引：节点 id → 骨架数组下标（升序），用于把已加载内容定位到精确骨架位置
  const skIndex = useMemo(() => {
    const m = new Map<string, number>();
    skeleton.forEach((p, i) => m.set(p.id, i));
    return m;
  }, [skeleton]);

  // 求滚动容器视口对应的 SVG y 范围（经 CTM 换算，兼容 viewBox 缩放）
  const visibleSvgRange = useCallback((): { topY: number; bottomY: number } => {
    const svg = svgRef.current;
    const container = containerRef.current;
    if (!svg || !container) return { topY: 0, bottomY: scale.svgHeight };
    const ctm = svg.getScreenCTM();
    if (!ctm) return { topY: 0, bottomY: scale.svgHeight };
    const rect = container.getBoundingClientRect();
    const inv = ctm.inverse();
    const ptTop = svg.createSVGPoint();
    ptTop.x = rect.left;
    ptTop.y = rect.top;
    const ptBot = svg.createSVGPoint();
    ptBot.x = rect.left;
    ptBot.y = rect.bottom;
    const top = ptTop.matrixTransform(inv);
    const bot = ptBot.matrixTransform(inv);
    return { topY: Math.min(top.y, bot.y), bottomY: Math.max(top.y, bot.y) };
  }, [scale.svgHeight]);

  // 把 SVG y 坐标换算为容器 scrollTop（迷你地图跳转 / 刷新重锚用）
  const svgYToScrollTop = useCallback((svgY: number): number => {
    const svg = svgRef.current;
    const container = containerRef.current;
    if (!svg || !container) return 0;
    const ctm = svg.getScreenCTM();
    if (!ctm) return 0;
    const pt = svg.createSVGPoint();
    pt.x = 0;
    pt.y = svgY;
    const screen = pt.matrixTransform(ctm);
    const rect = container.getBoundingClientRect();
    return container.scrollTop + (screen.y - rect.top);
  }, []);

  // 当前可见 SVG y 范围（含 overscan），随 viewVersion 重算
  const OVERSCAN_Y = 120;
  const visibleRange = useMemo(() => {
    const r = visibleSvgRange();
    return { topY: r.topY - OVERSCAN_Y, bottomY: r.bottomY + OVERSCAN_Y };
  }, [viewVersion, scale.svgHeight, visibleSvgRange]);

  useEffect(() => {
    visibleRangeRef.current = visibleRange;
  }, [visibleRange]);

  // 构建图数据 + 时间轴布局
  const graphData = useMemo(() => {
    const now = Date.now();
    const nodes: GraphNode[] = [];
    const edges: GraphEdge[] = [];
    const nodeIds = new Set<string>();

    const addNode = (n: GraphNode) => {
      if (!nodeIds.has(n.id)) {
        nodeIds.add(n.id);
        nodes.push(n);
      }
    };

    const agentId = `agent:${character}`;
    const agentLabel = character === 'vivian' ? t('mind_inspector.common.char_vivian') : t('mind_inspector.common.char_nana');
    const roommateChar: CharacterId = character === 'vivian' ? 'nana' : 'vivian';
    const roommateId = `agent:${roommateChar}`;
    const roommateLabel = roommateChar === 'vivian' ? t('mind_inspector.common.char_vivian') : t('mind_inspector.common.char_nana');

    addNode({
      id: 'user',
      type: 'user',
      label: t('mind_inspector.graph.type_user'),
      color: COLORS.event.presence,
      importance: 0.85,
      preview: t('mind_inspector.graph.node_user'),
      timestamp: now,
      side: 'left',
    });

    addNode({
      id: agentId,
      type: 'agent',
      label: agentLabel,
      color: CHARACTER_ACCENT[character],
      importance: 0.95,
      preview: t('mind_inspector.graph.node_agent', { name: agentLabel }),
      timestamp: now,
      side: 'right',
    });

    addNode({
      id: roommateId,
      type: 'agent',
      label: roommateLabel,
      color: CHARACTER_ACCENT[roommateChar],
      importance: 0.8,
      preview: t('mind_inspector.graph.node_agent', { name: roommateLabel }),
      timestamp: now,
      side: 'right',
    });

    edges.push({ source: 'user', target: agentId, kind: 'relation' });
    edges.push({ source: 'user', target: roommateId, kind: 'relation' });
    edges.push({ source: agentId, target: roommateId, kind: 'relation' });

    const timedNodes: Array<{ node: GraphNode }> = [];

    beliefs.forEach((b) => {
      const id = `belief:${b.id}`;
      const ts = b.created_at ? b.created_at * 1000 : now;
      const n: GraphNode = {
        id,
        type: 'belief',
        label: b.statement.length > 16 ? b.statement.slice(0, 16) + '…' : b.statement,
        color: COLORS.event.belief,
        importance: b.confidence,
        preview: b.statement,
        timestamp: ts,
        side: 'left',
      };
      addNode(n);
      timedNodes.push({ node: n });
      edges.push({ source: agentId, target: id, kind: 'relation' });
    });

    const roleToId = (role: 'user' | 'agent' | 'roommate'): string =>
      role === 'user' ? 'user' : role === 'agent' ? agentId : roommateId;

    // 广播群发时，同一用户消息会同时产生：直接对话节点（speaker=user,非旁观）与旁观节点
    // （speaker=user,perspective=observer）。二者正文相同，只保留直接对话节点，跳过旁观节点。
    const memoryResults = memories
      .map((m) => memoryToGraphNodes(m, { character, roommateChar, now }))
      .filter((res) => res && res.length > 0);
    const directUserBodies = new Set<string>();
    memoryResults.forEach((res) => {
      res.forEach((r) => {
        const n = r.node;
        if (n.speaker === 'user' && n.type === 'dialogue' && !n.bystander && n.preview) {
          directUserBodies.add(n.preview.trim());
        }
      });
    });

    memoryResults.forEach((res) => {
      res.forEach((r) => {
        const n = r.node;
        // 跳过与直接对话用户节点正文相同的旁观用户节点（广播去重）
        if (n.speaker === 'user' && n.bystander && n.preview && directUserBodies.has(n.preview.trim())) {
          return;
        }
        addNode(n);
        timedNodes.push({ node: n });
        r.edgeRoles.forEach((role) => {
          edges.push({ source: roleToId(role), target: n.id, kind: 'relation' });
        });
      });
    });

    // 建立摘要节点 ↔ summarized 原始对话的父子关系
    // session_summary 通过 metadata.promoted_from = [源 ShortTerm IDs] 反查子节点
    const summaryBySourceId = new Map<string, string>(); // sourceMemoryId → summaryNodeId
    nodes.forEach((node) => {
      if (node.type === 'session_summary') {
        const sources = node.metadata?.['promoted_from'];
        if (Array.isArray(sources)) {
          sources.forEach((srcId) => {
            if (typeof srcId === 'string') summaryBySourceId.set(srcId, node.id);
          });
        }
      }
    });
    // 为 summarized 节点设置 parentSummaryId，为 session_summary 节点设置 childIds
    if (summaryBySourceId.size > 0) {
      const childMap = new Map<string, string[]>(); // summaryId → childNodeIds
      nodes.forEach((node) => {
        if (node.summarized && node.memoryId) {
          const parentId = summaryBySourceId.get(node.memoryId);
          if (parentId) {
            node.parentSummaryId = parentId;
            const arr = childMap.get(parentId) || [];
            arr.push(node.id);
            childMap.set(parentId, arr);
          }
        }
      });
      childMap.forEach((childIds, parentId) => {
        const parent = nodes.find((n) => n.id === parentId);
        if (parent) {
          parent.childIds = childIds;
          // 将 session_summary 的时间戳对齐到最后一条子节点的时间，
          // 使其在时间轴上与被摘要的对话处于同一时间段
          let maxChildTs = 0;
          childIds.forEach((cid) => {
            const child = nodes.find((n) => n.id === cid);
            if (child && child.timestamp > maxChildTs) maxChildTs = child.timestamp;
          });
          if (maxChildTs > 0) parent.timestamp = maxChildTs;
          // 添加 session_summary → 子节点的连线
          childIds.forEach((cid) => {
            edges.push({ source: parentId, target: cid, kind: 'summary_child' });
          });
        }
      });
    }

    (mind?.goals ?? []).forEach((g) => {
      const id = `goal:${g.id}`;
      const n: GraphNode = {
        id,
        type: 'goal',
        label: g.description.length > 16 ? g.description.slice(0, 16) + '…' : g.description,
        color: COLORS.event.goal,
        importance: g.priority,
        preview: g.description,
        timestamp: now - 365 * 24 * 60 * 60 * 1000,
        side: 'right',
      };
      addNode(n);
      timedNodes.push({ node: n });
      edges.push({ source: agentId, target: id, kind: 'relation' });
    });

    relationships.forEach((r) => {
      const id = `rel:${r.id}`;
      const ts = r.created_at ? r.created_at * 1000 : now;
      const targetIsUser = r.target_agent === 'user' || r.target_agent === 'player';
      const targetNodeId = targetIsUser ? 'user' : `agent:${r.target_agent}`;
      const ownerNodeId = `agent:${r.owner_agent}`;
      const n: GraphNode = {
        id,
        type: 'relationship',
        label: r.fact_text.length > 16 ? r.fact_text.slice(0, 16) + '…' : r.fact_text,
        color: COLORS.event.relationship,
        importance: 0.5,
        preview: r.fact_text,
        timestamp: ts,
        side: 'left',
      };
      addNode(n);
      timedNodes.push({ node: n });
      if (nodeIds.has(ownerNodeId)) {
        edges.push({ source: ownerNodeId, target: id, kind: 'relation' });
      }
      if (nodeIds.has(targetNodeId) && targetNodeId !== ownerNodeId) {
        edges.push({ source: id, target: targetNodeId, kind: 'relation' });
      }
    });

    diaries.forEach((d) => {
      const n = diaryToGraphNode(d, { character, roommateChar, now });
      addNode(n);
      timedNodes.push({ node: n });
      edges.push({ source: agentId, target: n.id, kind: 'relation' });
    });

    beliefs.forEach((b) => {
      b.source_memory_ids.forEach((mid) => {
        const epId = `episode:${mid}`;
        if (nodeIds.has(epId)) {
          edges.push({ source: `belief:${b.id}`, target: epId, kind: 'relation' });
        }
      });
    });

    // ── 时间比例布局（骨架驱动比例尺，内容按骨架索引精确定位）──
    const contentItems = timedNodes.filter(
      (it) => it.node.type !== 'belief' && it.node.type !== 'relationship' && it.node.type !== 'goal',
    );
    const auxItems = timedNodes.filter((it) => it.node.type === 'belief' || it.node.type === 'relationship');
    const goalItems = timedNodes.filter((it) => it.node.type === 'goal');

    const svgW = 800;
    const svgH = scale.svgHeight;

    const layout = new Map<string, LayoutNode>();
    const prevLayout = layoutRef.current;
    const carryOffset = (id: string): { offsetX: number; offsetY: number } => {
      const p = prevLayout.get(id);
      return p ? { offsetX: p.offsetX, offsetY: p.offsetY } : { offsetX: 0, offsetY: 0 };
    };

    layout.set('user', { id: 'user', x: CORE_USER_X, y: CORE_Y, fixed: false, ...carryOffset('user'), vx: 0, vy: 0 });
    layout.set(agentId, { id: agentId, x: CORE_AGENT_X, y: CORE_Y, fixed: false, ...carryOffset(agentId), vx: 0, vy: 0 });
    layout.set(roommateId, { id: roommateId, x: CORE_ROOMMATE_X, y: CORE_Y_SECONDARY, fixed: false, ...carryOffset(roommateId), vx: 0, vy: 0 });

    // 已加载内容节点：按骨架索引定位（最新在上），与骨架占位点精确对齐
    // 没有骨架索引的节点（如被摘要的原始对话）按时间戳插值定位
    // session_summary 节点：时间戳已对齐到子节点，强制使用时间戳定位以与子节点处于同一时间段
    contentItems.forEach((item) => {
      item.node.side = (item.node.type === 'dialogue' || item.node.type === 'wechat') ? 'right' : 'left';
      const useTsPosition = item.node.type === 'session_summary' && item.node.childIds && item.node.childIds.length > 0;
      const idx = useTsPosition ? undefined : skIndex.get(item.node.id);
      const y = idx !== undefined ? scale.yAtIndex(idx) : scale.tsToY(item.node.timestamp);
      const x = item.node.side === 'left'
        ? TIMELINE_X - NODE_OFFSET_X - (item.node.type === 'session_summary' ? SUMMARY_EXTRA_OFFSET : 0)
        : TIMELINE_X + NODE_OFFSET_X;
      layout.set(item.node.id, { id: item.node.id, x, y, fixed: false, ...carryOffset(item.node.id), vx: 0, vy: 0 });
      edges.push({ source: 'timeline', target: item.node.id, kind: 'timeline' });
    });

    // 辅助节点（信念/关系）：不在骨架中，按时间戳插值定位
    auxItems.forEach((item) => {
      const y = scale.tsToY(item.node.timestamp);
      const x = item.node.side === 'left' ? TIMELINE_X - NODE_OFFSET_X : TIMELINE_X + NODE_OFFSET_X;
      layout.set(item.node.id, { id: item.node.id, x, y, fixed: false, ...carryOffset(item.node.id), vx: 0, vy: 0 });
      edges.push({ source: 'timeline', target: item.node.id, kind: 'timeline' });
    });

    // 目标节点：钉在核心节点下方水平带状区
    goalItems.forEach((item, i) => {
      const x = 180 + (i % 3) * 220;
      const y = 118 + Math.floor(i / 3) * 40;
      item.node.side = 'right';
      layout.set(item.node.id, { id: item.node.id, x, y, fixed: false, ...carryOffset(item.node.id), vx: 0, vy: 0 });
    });

    // 碰撞避免：同侧时间轴节点按 y 排序，自顶向下推开重叠节点
    {
      const sizeById = new Map<string, number>();
      timedNodes.forEach(({ node }) => sizeById.set(node.id, nodeSize(node.importance)));
      const sideIds: { left: string[]; right: string[] } = { left: [], right: [] };
      [...contentItems, ...auxItems].forEach((item) => {
        if (layout.has(item.node.id)) sideIds[item.node.side].push(item.node.id);
      });
      const COLLISION_GAP = 4;
      (['left', 'right'] as const).forEach((side) => {
        const ids = sideIds[side];
        if (ids.length < 2) return;
        ids.sort((a, b) => layout.get(a)!.y - layout.get(b)!.y);
        for (let i = 1; i < ids.length; i++) {
          const prev = layout.get(ids[i - 1])!;
          const cur = layout.get(ids[i])!;
          const minGap = (sizeById.get(prev.id) ?? MIN_SIZE) / 2 + (sizeById.get(cur.id) ?? MIN_SIZE) / 2 + COLLISION_GAP;
          if (cur.y - prev.y < minGap) {
            cur.y = prev.y + minGap;
          }
        }
      });
    }

    // summarized 子节点：始终按自身时间戳在时间轴上定位（参与正常防碰撞布局）
    // 收起时由渲染层隐藏，展开时直接显示在正确位置
    {
      const childrenOfSummary = new Map<string, string[]>();
      nodes.forEach((node) => {
        if (node.summarized && node.parentSummaryId) {
          const arr = childrenOfSummary.get(node.parentSummaryId) || [];
          arr.push(node.id);
          childrenOfSummary.set(node.parentSummaryId, arr);
        }
      });
      // 不再强制聚集到父节点周围，子节点保持时间轴自然位置
    }

    // 骨架占位点：尚未加载内容的骨架点在时间轴上以淡点呈现（居中于时间轴）
    const placeholders: Array<{ id: string; x: number; y: number; color: string }> = [];
    skeleton.forEach((p, i) => {
      if (nodeIds.has(p.id)) return;
      const y = scale.yAtIndex(i);
      if (y < visibleRange.topY || y > visibleRange.bottomY) return;
      placeholders.push({ id: p.id, x: TIMELINE_X, y, color: p.kind === 'diary' ? '#8B4513' : COLORS.event.observation });
    });

    // 时间刻度：在整个时间范围生成整齐刻度
    const targetTickCount = Math.max(4, Math.min(30, Math.floor(scale.totalContent / 90)));
    const ticks: TimeTick[] = scale.breakpoints.length > 0
      ? niceTicks(scale.minTs, scale.maxTs, targetTickCount).map((tk) => ({
          y: scale.tsToY(tk.ts),
          label: tk.label,
          timestamp: tk.ts,
        }))
      : [];

    layoutRef.current = layout;

    const neighborMap = new Map<string, string[]>();
    edges.forEach((edge) => {
      if (edge.kind !== 'relation') return;
      if (!neighborMap.has(edge.source)) neighborMap.set(edge.source, []);
      if (!neighborMap.has(edge.target)) neighborMap.set(edge.target, []);
      neighborMap.get(edge.source)!.push(edge.target);
      neighborMap.get(edge.target)!.push(edge.source);
    });
    neighborMapRef.current = neighborMap;

    // ── 在场状态分段：从 presence_log 记忆提取历史状态，驱动时间轴分段着色 ──
    const presenceSegments: Array<{ y1: number; y2: number; state: string }> = [];
    const presenceEvents = memories
      .filter((m) => m.tags?.includes('presence_log') || (m.metadata as Record<string, unknown> | undefined)?.kind === 'presence_log')
      .map((m) => ({
        ts: parseTimestamp(m.timestamp ?? m.created_at ?? Date.now()),
        from: String((m.metadata as Record<string, unknown> | undefined)?.from || ''),
        to: String((m.metadata as Record<string, unknown> | undefined)?.to || ''),
      }))
      .filter((e) => e.to)
      .sort((a, b) => a.ts - b.ts);

    if (presenceEvents.length > 0) {
      // 最早状态段：从 scale.minTs 到第一个事件
      const firstFrom = presenceEvents[0].from.toLowerCase();
      if (firstFrom) {
        presenceSegments.push({
          y1: scale.tsToY(presenceEvents[0].ts),
          y2: scale.tsToY(scale.minTs),
          state: firstFrom,
        });
      }
      // 每个事件对应的状态持续段
      for (let i = 0; i < presenceEvents.length; i++) {
        const startTs = presenceEvents[i].ts;
        const endTs = i + 1 < presenceEvents.length ? presenceEvents[i + 1].ts : Date.now();
        presenceSegments.push({
          y1: scale.tsToY(endTs),
          y2: scale.tsToY(startTs),
          state: presenceEvents[i].to.toLowerCase(),
        });
      }
    }

    return { graphNodes: nodes, graphEdges: edges, svgWidth: svgW, svgHeight: svgH, timeTicks: ticks, placeholders, presenceSegments };
  }, [beliefs, relationships, memories, diaries, mind, character, t, skeleton, skIndex, scale, visibleRange, cacheVersion]);

  const { graphNodes, graphEdges, svgWidth, svgHeight, timeTicks, placeholders, presenceSegments } = graphData;

  const viewBox = useMemo(() => {
    return `0 0 ${svgWidth} ${svgHeight}`;
  }, [svgWidth, svgHeight]);

  const startAnim = useCallback(() => {
    if (animatingRef.current) return;
    animatingRef.current = true;
    const tick = () => {
      const layout = layoutRef.current;
      const neighbors = neighborMapRef.current;
      let moving = false;

      const forces = new Map<string, { fx: number; fy: number }>();
      layout.forEach((n) => {
        if (n.fixed) return;
        let fx = -SPRING_K * n.offsetX;
        let fy = -SPRING_K * n.offsetY;
        forces.set(n.id, { fx, fy });
      });

      forces.forEach((force, id) => {
        const nbrs = neighbors.get(id);
        if (!nbrs) return;
        let totalPullX = 0;
        let totalPullY = 0;
        let activeNbrs = 0;
        nbrs.forEach((nid) => {
          const nb = layout.get(nid);
          if (!nb) return;
          if (Math.abs(nb.offsetX) > 0.5 || Math.abs(nb.offsetY) > 0.5) {
            totalPullX += nb.offsetX;
            totalPullY += nb.offsetY;
            activeNbrs++;
          }
        });
        if (activeNbrs > 0) {
          force.fx += NEIGHBOR_STRENGTH * (totalPullX / activeNbrs);
          force.fy += NEIGHBOR_STRENGTH * (totalPullY / activeNbrs);
        }
      });

      layout.forEach((n) => {
        if (n.fixed) return;
        const f = forces.get(n.id);
        if (!f) return;
        n.vx = (n.vx + f.fx) * SPRING_DAMPING;
        n.vy = (n.vy + f.fy) * SPRING_DAMPING;
        n.offsetX += n.vx;
        n.offsetY += n.vy;
        if (Math.abs(n.vx) > SPRING_THRESHOLD || Math.abs(n.vy) > SPRING_THRESHOLD ||
            Math.abs(n.offsetX) > SPRING_THRESHOLD || Math.abs(n.offsetY) > SPRING_THRESHOLD) {
          moving = true;
        } else {
          n.vx = 0;
          n.vy = 0;
          n.offsetX = 0;
          n.offsetY = 0;
        }
      });

      forceUpdate();

      if (moving) {
        rafRef.current = requestAnimationFrame(tick);
      } else {
        animatingRef.current = false;
        rafRef.current = null;
      }
    };
    rafRef.current = requestAnimationFrame(tick);
  }, [forceUpdate]);

  useEffect(() => {
    return () => {
      if (rafRef.current) cancelAnimationFrame(rafRef.current);
    };
  }, []);

  // 获取节点实际位置（含拖拽偏移）
  const getNodePos = useCallback((id: string): { x: number; y: number } | null => {
    const n = layoutRef.current.get(id);
    if (!n) return null;
    return { x: n.x + n.offsetX, y: n.y + n.offsetY };
  }, []);

  // 坐标转换
  const getSvgPoint = useCallback((clientX: number, clientY: number): { x: number; y: number } => {
    const svg = svgRef.current;
    if (!svg) return { x: 0, y: 0 };
    const pt = svg.createSVGPoint();
    pt.x = clientX;
    pt.y = clientY;
    const ctm = svg.getScreenCTM();
    if (!ctm) return { x: 0, y: 0 };
    return pt.matrixTransform(ctm.inverse());
  }, []);

  // 骨架刷新后按中心时间戳重锚滚动，避免高度变化导致视口跳变
  useLayoutEffect(() => {
    const anchorTs = pendingAnchorRef.current;
    if (anchorTs == null) return;
    pendingAnchorRef.current = null;
    const container = containerRef.current;
    if (!container) return;
    container.scrollTop = Math.max(0, svgYToScrollTop(scale.tsToY(anchorTs)));
  }, [scale, svgYToScrollTop]);

  // SVG 高度变化后强制重算可见范围：visibleRange 在渲染期经 getScreenCTM 计算，
  // 此时 DOM 尚未提交新高度，CTM 仍是旧值；高度提交后须刷新一次以得到正确视口范围
  useLayoutEffect(() => {
    setViewVersion((v) => v + 1);
  }, [svgHeight]);

  // ── 内容懒加载：按可见骨架索引带拉取缺失的记忆/日记内容 ──
  useEffect(() => {
    if (skeleton.length === 0 || scale.breakpoints.length === 0) return;
    const char = character;
    let cancelled = false;

    const timer = setTimeout(() => {
      const bps = scale.breakpoints;
      const n = bps.length;
      const topTs = scale.yToTs(visibleRange.topY);
      const botTs = scale.yToTs(visibleRange.bottomY);
      const tsLo = Math.min(topTs, botTs);
      const tsHi = Math.max(topTs, botTs);

      // 二分定位可见骨架索引带
      let lo = 0;
      let hi = n - 1;
      let first = n;
      while (lo <= hi) {
        const mid = (lo + hi) >> 1;
        if (bps[mid].ts >= tsLo) { first = mid; hi = mid - 1; } else { lo = mid + 1; }
      }
      lo = 0; hi = n - 1;
      let last = -1;
      while (lo <= hi) {
        const mid = (lo + hi) >> 1;
        if (bps[mid].ts <= tsHi) { last = mid; lo = mid + 1; } else { hi = mid - 1; }
      }
      // 视口可能落在骨架时间范围之外（顶部/底部留白）或两断点之间，
      // 钳制到有效索引并用 min/max 取最近的有效索引带
      const firstIdx = Math.max(0, Math.min(n - 1, first));
      const lastIdx = Math.max(0, Math.min(n - 1, last));

      const OVERSCAN_PTS = 30;
      const i0 = Math.max(0, Math.min(firstIdx, lastIdx) - OVERSCAN_PTS);
      const i1 = Math.min(n - 1, Math.max(firstIdx, lastIdx) + OVERSCAN_PTS);
      const afterMs = bps[i0].ts;
      const beforeMs = bps[i1].ts + 1;

      const memMissing = missingRanges(afterMs, beforeMs, loadedMemRangesRef.current)
        .filter((iv) => !inFlightMemRef.current.has(`${iv.start}:${iv.end}`));
      const diaryMissing = missingRanges(afterMs, beforeMs, loadedDiaryRangesRef.current)
        .filter((iv) => !inFlightDiaryRef.current.has(`${iv.start}:${iv.end}`));
      if (memMissing.length === 0 && diaryMissing.length === 0) return;

      memMissing.forEach((iv) => inFlightMemRef.current.add(`${iv.start}:${iv.end}`));
      diaryMissing.forEach((iv) => inFlightDiaryRef.current.add(`${iv.start}:${iv.end}`));

      const memPromises = memMissing.map((iv) =>
        invoke<MemoryItem[]>('get_memories_range', {
          characterId: char,
          after: iv.start / 1000,
          before: iv.end / 1000,
        }).catch(() => [] as MemoryItem[]),
      );
      const diaryPromises = diaryMissing.map((iv) =>
        invoke<DiaryEntryLite[]>('get_diary_range', {
          characterId: char,
          after: Math.floor(iv.start / 1000),
          before: Math.ceil(iv.end / 1000),
        }).catch(() => [] as DiaryEntryLite[]),
      );

      Promise.all([Promise.all(memPromises), Promise.all(diaryPromises)])
        .then(([memResults, diaryResults]) => {
          if (cancelled || charRef.current !== char) return;

          const newMems = memResults.flat();
          if (newMems.length > 0) {
            setMemories((prev) => {
              const seen = new Set(prev.map((m) => m.id));
              const add = newMems.filter((m) => !seen.has(m.id));
              return add.length > 0 ? [...prev, ...add] : prev;
            });
          }
          const newDiaries = diaryResults.flat();
          if (newDiaries.length > 0) {
            setDiaries((prev) => {
              const seen = new Set(prev.map((d) => d.id));
              const add = newDiaries.filter((d) => !seen.has(d.id));
              return add.length > 0 ? [...prev, ...add] : prev;
            });
          }
          if (newMems.length > 0 || newDiaries.length > 0) {
            setCacheVersion((v) => v + 1);
          }
          requestAnimationFrame(() => {
            const pf = pendingFocusRef.current;
            if (pf && pf.targetChar === char) {
              focusNearestMemory(pf.timestamp, pf.preview);
            }
          });
        })
        .finally(() => {
          memMissing.forEach((iv) => {
            inFlightMemRef.current.delete(`${iv.start}:${iv.end}`);
            loadedMemRangesRef.current = insertRange(iv.start, iv.end, loadedMemRangesRef.current);
          });
          diaryMissing.forEach((iv) => {
            inFlightDiaryRef.current.delete(`${iv.start}:${iv.end}`);
            loadedDiaryRangesRef.current = insertRange(iv.start, iv.end, loadedDiaryRangesRef.current);
          });
        });
    }, 120);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [visibleRange, skeleton, scale, character]);

  const handleNodeMouseDown = (e: React.MouseEvent, nodeId: string) => {
    e.preventDefault();
    e.stopPropagation();
    const pt = getSvgPoint(e.clientX, e.clientY);
    const node = layoutRef.current.get(nodeId);
    if (!node) return;
    if (rafRef.current) {
      cancelAnimationFrame(rafRef.current);
      animatingRef.current = false;
      rafRef.current = null;
    }
    node.vx = 0;
    node.vy = 0;
    draggingRef.current = {
      id: nodeId,
      offsetX: pt.x - node.x - node.offsetX,
      offsetY: pt.y - node.y - node.offsetY,
      lastX: pt.x,
      lastY: pt.y,
      lastTime: performance.now(),
    };
    dragMovedRef.current = false;
    node.fixed = true;
  };

  const handleSvgMouseMove = (e: React.MouseEvent) => {
    if (!draggingRef.current) return;
    const pt = getSvgPoint(e.clientX, e.clientY);
    const node = layoutRef.current.get(draggingRef.current.id);
    if (node) {
      const now = performance.now();
      const dt = Math.max(1, now - draggingRef.current.lastTime);
      const newOX = pt.x - node.x - draggingRef.current.offsetX;
      const newOY = pt.y - node.y - draggingRef.current.offsetY;
      const deltaX = newOX - node.offsetX;
      const deltaY = newOY - node.offsetY;
      node.vx = deltaX / dt * 16;
      node.vy = deltaY / dt * 16;
      node.offsetX = newOX;
      node.offsetY = newOY;
      // 会话圈拖拽耦合：同圈节点被牵扯（衰减系数 0.3）
      const peers = sessionPeerRef.current.get(draggingRef.current.id);
      if (peers) {
        const coupling = 0.3;
        for (const pid of peers) {
          const peer = layoutRef.current.get(pid);
          if (peer && !peer.fixed) {
            peer.offsetX += deltaX * coupling;
            peer.offsetY += deltaY * coupling;
          }
        }
      }
      draggingRef.current.lastX = pt.x;
      draggingRef.current.lastY = pt.y;
      draggingRef.current.lastTime = now;
      dragMovedRef.current = true;
      forceUpdate();
    }
  };

  const handleSvgMouseUp = () => {
    if (draggingRef.current) {
      const node = layoutRef.current.get(draggingRef.current.id);
      if (node) {
        node.fixed = false;
        const speed = Math.sqrt(node.vx * node.vx + node.vy * node.vy);
        if (speed > 40) {
          const scale = Math.min(12, 40 / speed);
          node.vx *= scale;
          node.vy *= scale;
        }
      }
      draggingRef.current = null;
      startAnim();
    }
  };

  const handleNodeClick = (e: React.MouseEvent, nodeId: string) => {
    e.stopPropagation();
    if (dragMovedRef.current) return;

    const node = nodeMap.get(nodeId);

    // session_summary 节点：点击切换展开/收起，展开后显示被摘要的原始对话节点
    if (node?.type === 'session_summary' && node.childIds && node.childIds.length > 0) {
      setExpandedSummaryIds((prev) => {
        const next = new Set(prev);
        if (next.has(nodeId)) {
          next.delete(nodeId);
        } else {
          next.add(nodeId);
        }
        return next;
      });
      return;
    }

    if (node?.type === 'diary') {
      const diaryId = nodeId.replace(/^diary:/, '');
      nav?.navigateTo('diary', {
        diaryId,
        diaryCharacter: character,
      });
      return;
    }

    // 核心节点（user/agent）不弹窗
    if (node?.type === 'user' || node?.type === 'agent') {
      setSelectedNode((prev) => (prev === nodeId ? null : nodeId));
      return;
    }

    // 其他节点：点击打开弹窗显示完整内容
    if (node) {
      setModalNode(node);
      setSelectedNode(nodeId);
    }
  };

  const handleSvgClick = () => {
    if (!dragMovedRef.current) {
      setSelectedNode(null);
      setModalNode(null);
    }
  };

  // 高亮计算
  const connectedSet = useMemo(() => {
    if (!selectedNode) return null;
    const set = new Set<string>([selectedNode]);
    for (const edge of graphEdges) {
      if (edge.source === selectedNode) set.add(edge.target);
      if (edge.target === selectedNode) set.add(edge.source);
    }
    return set;
  }, [selectedNode, graphEdges]);

  const isNodeActive = (id: string): boolean => {
    if (connectedSet) return connectedSet.has(id);
    if (hoveredNode) return hoveredNode === id;
    return true;
  };

  const nodeMap = useMemo(() => {
    const m = new Map<string, GraphNode>();
    graphNodes.forEach((n) => m.set(n.id, n));
    return m;
  }, [graphNodes]);

  // 会话分组 + 回应箭头推导（时间邻近 + channel 类别 + speaker→listener 语义匹配）
  // 折叠的 summarized 子节点不参与会话圈和箭头，避免幽灵元素
  const { sessionGroups, replyArrows } = useMemo(() => {
    const groups: SessionGroup[] = [];
    const arrows: ReplyArrow[] = [];

    const channelClass = (n: GraphNode): string =>
      n.metadata?.channel === 'cross_character' ? 'cross' : 'direct';

    // 隐藏节点：被折叠的 summarized 子节点
    const isHidden = (n: GraphNode): boolean =>
      !!n.summarized && !!n.parentSummaryId && !expandedSummaryIds.has(n.parentSummaryId);

    const dialogues = graphNodes
      .filter((n) => (n.type === 'dialogue' || n.type === 'wechat') && !isHidden(n))
      .sort((a, b) => a.timestamp - b.timestamp);

    let current: GraphNode[] = [];
    const flush = () => {
      if (current.length >= 2) {
        const startTs = current[0].timestamp;
        const endTs = current[current.length - 1].timestamp;
        const dialogueIds = current.map((d) => d.id);
        const memberIds = [...dialogueIds];
        groups.push({
          id: `sess:${dialogueIds[0]}`,
          dialogueIds,
          memberIds,
          startTs,
          endTs,
        });
        // 箭头推导：基于 speaker→listener 语义关系，而非单纯时间顺序
        // 当前节点 cur 的回应箭头应指向：最近的一条以 cur.listener 作为 speaker 的消息
        // 这样才能保证箭头从说话者指向被说话者
        for (let i = 1; i < current.length; i++) {
          const cur = current[i];
          const curSpeaker = cur.speaker;
          const curListener = cur.metadata?.listener as string | undefined;
          if (!curSpeaker) continue;
          // 寻找最近的一条消息：其 speaker 等于当前消息的 listener
          // 若当前消息没有显式 listener，回退到 speaker 翻转逻辑
          let targetIdx = -1;
          if (curListener) {
            for (let j = i - 1; j >= 0; j--) {
              const prev = current[j];
              if (prev.speaker === curListener) {
                targetIdx = j;
                break;
              }
            }
          }
          // 回退策略：仅在没有显式 listener 信息时（旧数据兼容），
          // 才用"找 speaker 不同的最近消息"来猜测对话关系。
          // 有明确 listener 但语义匹配失败时，说明听话人尚未在本会话中发言，不画箭头。
          if (targetIdx === -1 && !curListener) {
            for (let j = i - 1; j >= 0; j--) {
              const prev = current[j];
              if (prev.speaker && prev.speaker !== curSpeaker) {
                targetIdx = j;
                break;
              }
            }
          }
          if (targetIdx !== -1) {
            const prev = current[targetIdx];
            arrows.push({
              id: `reply:${cur.id}>${prev.id}`,
              fromId: cur.id,
              toId: prev.id,
            });
          }
        }
      }
      current = [];
    };

    for (const d of dialogues) {
      const last = current[current.length - 1];
      if (last) {
        const dHasSession = !!d.sessionId;
        const lastHasSession = !!last.sessionId;
        let shouldSplit = false;
        if (dHasSession && lastHasSession) {
          shouldSplit = d.sessionId !== last.sessionId;
        } else if (dHasSession && !lastHasSession) {
          shouldSplit = d.timestamp - last.timestamp > SESSION_GAP_MS;
        } else if (!dHasSession && lastHasSession) {
          shouldSplit = true;
        } else {
          shouldSplit = d.timestamp - last.timestamp > SESSION_GAP_MS ||
                        channelClass(d) !== channelClass(last);
        }
        if (shouldSplit) {
          flush();
        }
      }
      current.push(d);
    }
    flush();

    // 更新会话组同伴映射（拖拽耦合用）
    const peerMap = new Map<string, string[]>();
    for (const g of groups) {
      for (const id of g.memberIds) {
        peerMap.set(id, g.memberIds.filter((m) => m !== id));
      }
    }
    sessionPeerRef.current = peerMap;

    return { sessionGroups: groups, replyArrows: arrows };
  }, [graphNodes, skIndex, expandedSummaryIds]);

  // 扇形展开：共享同一目标节点的箭头组，每条箭头获得不同的垂直偏移以避免路径重叠
  const fanOffsetMap = useMemo(() => {
    const map = new Map<string, number>();
    const byTarget = new Map<string, string[]>();
    for (const ra of replyArrows) {
      const list = byTarget.get(ra.toId);
      if (list) list.push(ra.id);
      else byTarget.set(ra.toId, [ra.id]);
    }
    for (const ids of byTarget.values()) {
      const n = ids.length;
      if (n < 2) continue;
      ids.forEach((id, i) => {
        map.set(id, i - (n - 1) / 2);
      });
    }
    return map;
  }, [replyArrows]);

  if (loading && skeleton.length === 0) {
    return (
      <div style={{ flex: 1, display: 'flex' }}>
        <EmptyState spinner text={t('mind_inspector.graph.loading')} />
      </div>
    );
  }

  if (error && skeleton.length === 0) {
    return (
      <div style={{ flex: 1, display: 'flex' }}>
        <EmptyState icon={<AlertCircle size={24} color={COLORS.textTertiary} strokeWidth={1.5} />} text={t('mind_inspector.common.load_failed', { error })} />
      </div>
    );
  }

  const hoveredPos = hoveredNode ? getNodePos(hoveredNode) : null;
  const hoveredGraphNode = hoveredNode ? nodeMap.get(hoveredNode) : null;

  // 便签定位：侧向弹出（远离时间轴中心），垂直居中对齐节点
  const tooltipAnchor = (() => {
    if (!hoveredPos) return null;
    const pctX = (hoveredPos.x / svgWidth) * 100;
    const pctY = Math.min(Math.max((hoveredPos.y / svgHeight) * 100, 8), 92);
    const nodeScreenR = 20; // 节点屏幕半径近似
    const gap = 12;
    // 节点在 SVG 左半 → 便签偏右；右半 → 便签偏左
    const placeRight = pctX < 50;
    const left = placeRight
      ? `${Math.min(pctX + (nodeScreenR / svgWidth) * 100 + (gap / svgWidth) * 100, 72)}%`
      : `${Math.max(pctX - (nodeScreenR / svgWidth) * 100 - (gap / svgWidth) * 100, 28)}%`;
    const transform = placeRight
      ? 'translate(0, -50%)'
      : 'translate(-100%, -50%)';
    return { left, top: `${pctY}%`, transform };
  })();

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
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: SPACING.md,
          flexWrap: 'wrap',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.md }}>
          <MemoTapeTab
            label={t('mind_inspector.common.char_vivian')}
            color={CHAR_TAPE.vivian}
            rot={-1.6}
            active={character === 'vivian'}
            onClick={() => setCharacter('vivian')}
          />
          <MemoTapeTab
            label={t('mind_inspector.common.char_nana')}
            color={CHAR_TAPE.nana}
            rot={1.4}
            active={character === 'nana'}
            onClick={() => setCharacter('nana')}
          />
        </div>
      </div>

      <div style={{ display: 'flex', alignItems: 'center', gap: 9 }}>
        <span
          aria-hidden
          style={{
            width: 22,
            height: 22,
            flexShrink: 0,
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            background: GPAPER.stampRed,
            color: '#FFF9EE',
            borderRadius: 4,
            transform: 'rotate(-6deg)',
            boxShadow: `inset 0 0 0 1.5px rgba(255,249,238,0.55), ${GPAPER.shadowSm}`,
            fontFamily: TYPO.fontFamilyCN,
            fontSize: 13.5,
            lineHeight: 1,
          }}
        >
          忆
        </span>
        <span
          style={{
            fontFamily: TYPO.fontFamilyCN,
            fontSize: 17.5,
            color: GPAPER.ink,
            letterSpacing: 2.5,
            whiteSpace: 'nowrap',
          }}
        >
          {t('mind_inspector.graph.title', { character, nodes: graphNodes.length, edges: graphEdges.length })}
        </span>
        <span
          aria-hidden
          style={{
            flex: 1,
            borderBottom: `1px dashed ${GPAPER.border}`,
            transform: 'translateY(3px)',
          }}
        />
      </div>

      {/* 时间线图谱 + 迷你地图 */}
      <div style={{ flex: 1, display: 'flex', gap: SPACING.sm, minHeight: 520 }}>
      <div
          ref={containerRef}
          onScroll={handleScroll}
          style={{
            position: 'relative',
            flex: '1 1 0',
            minWidth: 0,
            minHeight: 520,
            borderRadius: RADIUS.xl,
            overflowY: 'auto',
            overflowX: 'hidden',
            border: `1.5px solid ${GPAPER.border}`,
            backgroundColor: GPAPER.paper,
            backgroundImage: `linear-gradient(${GPAPER.grid} 1px, transparent 1px), linear-gradient(90deg, ${GPAPER.grid} 1px, transparent 1px)`,
            backgroundSize: '24px 24px',
            backgroundAttachment: 'local',
            boxShadow: GPAPER.shadowLg,
          }}
        >
        <div
          style={{
            position: 'sticky',
            top: 0,
            left: 0,
            right: 0,
            height: 2,
            background: `linear-gradient(90deg, transparent 0%, ${hexToRgba(CHARACTER_ACCENT[character], 0.5)} 30%, ${hexToRgba(CHARACTER_ACCENT[character], 0.7)} 50%, ${hexToRgba(CHARACTER_ACCENT[character], 0.5)} 70%, transparent 100%)`,
            zIndex: 2,
          }}
        />
        <svg
          ref={svgRef}
          viewBox={viewBox}
          preserveAspectRatio="xMidYMin meet"
          style={{
            width: '100%',
            display: 'block',
            minHeight: 520,
            height: svgHeight,
            cursor: draggingRef.current ? 'grabbing' : 'default',
          }}
          onMouseMove={handleSvgMouseMove}
          onMouseUp={handleSvgMouseUp}
          onMouseLeave={handleSvgMouseUp}
          onClick={handleSvgClick}
        >
          <defs>
            {/* 贴纸纸张投影 */}
            <filter id="sticker-shadow" x="-40%" y="-40%" width="180%" height="180%">
              <feDropShadow dx="0" dy="1.5" stdDeviation="2" floodColor="#3B3428" floodOpacity="0.22" />
            </filter>
          </defs>

          {/* 时间轴：按在场状态历史分段着色 */}
          {presenceSegments.length > 0
            ? presenceSegments.map((seg, i) => {
                const color =
                  seg.state === 'busy'
                    ? COLORS.danger
                    : seg.state === 'rest' || seg.state === 'offline'
                      ? COLORS.textTertiary
                      : COLORS.success;
                return (
                  <line
                    key={`tl-seg-${i}`}
                    x1={TIMELINE_X}
                    y1={Math.max(seg.y1, CORE_Y + 34)}
                    x2={TIMELINE_X}
                    y2={Math.min(seg.y2, svgHeight - BOTTOM_PADDING + 10)}
                    stroke={color}
                    strokeWidth={2}
                    opacity={0.75}
                  />
                );
              })
            : (
              <line
                x1={TIMELINE_X}
                y1={CORE_Y + 34}
                x2={TIMELINE_X}
                y2={svgHeight - BOTTOM_PADDING + 10}
                stroke={COLORS.success}
                strokeWidth={2}
                opacity={0.75}
              />
            )}

          {/* 时间刻度：墨点 + 手写时间 */}
          {timeTicks.filter((tick) => tick.y >= visibleRange.topY && tick.y <= visibleRange.bottomY).map((tick, i) => (
            <g key={`tick-${i}`}>
              <circle
                cx={TIMELINE_X}
                cy={tick.y}
                r={3}
                fill={GPAPER.ink}
                stroke={GPAPER.card}
                strokeWidth={1.2}
                opacity={0.55}
              />
              <text
                x={TIMELINE_X - 18}
                y={tick.y + 4}
                textAnchor="end"
                fill={GPAPER.inkSoft}
                fontSize={12.5}
                fontFamily={HAND}
                style={{ pointerEvents: 'none', userSelect: 'none' }}
              >
                {tick.label}
              </text>
            </g>
          ))}

          {/* 「现在」：印章红圆点 + 手写字 */}
          {CORE_Y + 40 >= visibleRange.topY && CORE_Y + 40 <= visibleRange.bottomY && (
            <g>
              <circle
                cx={TIMELINE_X}
                cy={CORE_Y + 40}
                r={4.5}
                fill={GPAPER.stampRed}
                stroke={GPAPER.card}
                strokeWidth={1.5}
              />
              <text
                x={TIMELINE_X - 18}
                y={CORE_Y + 44}
                textAnchor="end"
                fill={GPAPER.stampRed}
                fontSize={13}
                fontWeight={600}
                fontFamily={HAND}
                style={{ pointerEvents: 'none', userSelect: 'none' }}
              >
                现在
              </text>
            </g>
          )}

          {/* 关系边：悬浮或选中节点时显示与其相连的边 */}
          {(hoveredNode || selectedNode) && graphEdges
            .filter(e => e.kind === 'relation' && (e.source === (hoveredNode || selectedNode) || e.target === (hoveredNode || selectedNode)))
            .map((edge, i) => {
            const sp = getNodePos(edge.source);
            const tp = getNodePos(edge.target);
            if (!sp || !tp) return null;
            const spVisible = sp.y >= visibleRange.topY && sp.y <= visibleRange.bottomY;
            const tpVisible = tp.y >= visibleRange.topY && tp.y <= visibleRange.bottomY;
            if (!spVisible && !tpVisible) return null;
            const sourceNode = nodeMap.get(edge.source);
            const edgeColor = sourceNode ? sourceNode.color : COLORS.accent;
            const dx = tp.x - sp.x;
            const dy = tp.y - sp.y;
            const cx1 = sp.x + dx * 0.35;
            const cy1 = sp.y + dy * 0.2;
            const cx2 = sp.x + dx * 0.65;
            const cy2 = tp.y - dy * 0.2;
            return (
              <g key={`rel-${i}`}>
                <path
                  d={`M ${sp.x} ${sp.y} C ${cx1} ${cy1}, ${cx2} ${cy2}, ${tp.x} ${tp.y}`}
                  fill="none"
                  stroke={pastel(edgeColor)}
                  strokeWidth={1.5}
                  strokeLinecap="round"
                  strokeDasharray={'4 3'}
                />
              </g>
            );
          })}

          {/* 时间轴水平连线 */}
          {graphEdges.filter(e => e.kind === 'timeline').map((edge, i) => {
            const tp = getNodePos(edge.target);
            if (!tp) return null;
            const node = nodeMap.get(edge.target);
            if (!node) return null;
            const layoutNode = layoutRef.current.get(edge.target);
            const originalY = layoutNode ? layoutNode.y : tp.y;
            if (originalY < visibleRange.topY || originalY > visibleRange.bottomY) return null;
            const active = isNodeActive(edge.target);
            const x2 = tp.x;
            const dir = node.side === 'left' ? -1 : 1;
            const x1 = TIMELINE_X;
            return (
              <g key={`tl-${i}`}>
                <line
                  x1={x1}
                  y1={originalY}
                  x2={x2 + dir * (MIN_SIZE / 2 + 2)}
                  y2={tp.y}
                  stroke={active ? hexToRgba(node.color, 0.2) : hexToRgba(node.color, 0.05)}
                  strokeWidth={active ? 1 : 0.5}
                  strokeDasharray={active ? 'none' : '3 3'}
                  strokeLinecap="round"
                  opacity={active ? 0.5 : 0.25}
                />
                {/* 节点在时间轴上的小圆点 - 固定在原始位置 */}
                <circle
                  cx={TIMELINE_X}
                  cy={originalY}
                  r={active ? 3 : 2}
                  fill={active ? pastel(node.color) : hexToRgba(node.color, 0.2)}
                  stroke={active ? GPAPER.card : 'none'}
                  strokeWidth={active ? 1 : 0}
                />
              </g>
            );
          })}

          {/* session_summary → 子节点的连线（展开时显示） */}
          {graphEdges.filter(e => e.kind === 'summary_child').map((edge, i) => {
            if (!expandedSummaryIds.has(edge.source)) return null;
            const sp = getNodePos(edge.source);
            const tp = getNodePos(edge.target);
            if (!sp || !tp) return null;
            const sourceNode = nodeMap.get(edge.source);
            const edgeColor = sourceNode ? sourceNode.color : '#7C3AED';
            return (
              <line
                key={`sc-${i}`}
                x1={sp.x}
                y1={sp.y}
                x2={tp.x}
                y2={tp.y}
                stroke={hexToRgba(edgeColor, 0.35)}
                strokeWidth={1}
                strokeDasharray={'2 3'}
                strokeLinecap="round"
              />
            );
          })}

          {/* 骨架占位点：尚未加载内容的记忆/日记以淡点呈现 */}
          {placeholders.map((p) => (
            <circle
              key={`ph-${p.id}`}
              cx={p.x}
              cy={p.y}
              r={2.5}
              fill={hexToRgba(p.color, 0.3)}
              style={{ pointerEvents: 'none' }}
            />
          ))}

          {/* 会话圈：同一次会话的节点用手绘圈圈起；任一成员被拖拽时隐藏 */}
          {sessionGroups.map((sg) => {
            // 任一成员有位移时隐藏圈
            for (const id of sg.memberIds) {
              const ln = layoutRef.current.get(id);
              if (ln && (Math.abs(ln.offsetX) > 2 || Math.abs(ln.offsetY) > 2)) return null;
            }
            let minX = Infinity;
            let maxX = -Infinity;
            let minY = Infinity;
            let maxY = -Infinity;
            let any = false;
            for (const id of sg.memberIds) {
              const p = getNodePos(id);
              if (!p) continue;
              const node = nodeMap.get(id);
              const r = (node ? nodeSize(node.importance) : 24) / 2;
              minX = Math.min(minX, p.x - r);
              maxX = Math.max(maxX, p.x + r);
              minY = Math.min(minY, p.y - r);
              maxY = Math.max(maxY, p.y + r);
              any = true;
            }
            if (!any) return null;
            if (maxY < visibleRange.topY || minY > visibleRange.bottomY) return null;
            const pad = 16;
            const cx = (minX + maxX) / 2;
            const cy = (minY + maxY) / 2;
            const rx = Math.max(30, (maxX - minX) / 2 + pad);
            const ry = Math.max(24, (maxY - minY) / 2 + pad);
            return (
              <path
                key={sg.id}
                d={handDrawnLoopPath(cx, cy, rx, ry, sg.id)}
                fill="none"
                stroke={pastel(CHARACTER_ACCENT[character])}
                strokeWidth={1.6}
                opacity={0.5}
                strokeLinecap="round"
                style={{ pointerEvents: 'none' }}
              />
            );
          })}

          {/* 节点 */}
          {graphNodes.map((node) => {
            const pos = getNodePos(node.id);
            if (!pos) return null;
            const isCore = node.type === 'user' || node.type === 'agent';
            if (!isCore && (pos.y < visibleRange.topY || pos.y > visibleRange.bottomY)) return null;
            // summarized 节点：默认隐藏，仅当父 session_summary 展开时显示
            if (node.summarized && node.parentSummaryId) {
              if (!expandedSummaryIds.has(node.parentSummaryId)) return null;
            }
            // session_summary 节点：派生展开状态（不 mutate node 对象）
            const isSummaryExpanded = node.type === 'session_summary' && expandedSummaryIds.has(node.id);
            const size = nodeSize(node.importance);
            const active = isNodeActive(node.id);
            const isHighlighted = selectedNode === node.id || hoveredNode === node.id;
            const tilt = stickerTilt(node.id);
            const labelY = isCore
              ? pos.y - size / 2 + 1
              : (node.side === 'left' ? pos.y - size / 2 - 10 : pos.y + size / 2 + 16);
            const labelText = node.label;
            const labelFontSize = isCore ? 13.5 : 12.5;
            const labelWidth = estimateTextWidth(labelText, labelFontSize) + 22;
            const labelAnchor = node.side === 'left'
              ? (isCore ? 'middle' : 'end')
              : (isCore ? 'middle' : 'start');
            const labelX = node.side === 'left'
              ? (isCore ? pos.x : pos.x - size / 2 - 6)
              : (isCore ? pos.x : pos.x + size / 2 + 6);
            const rectX = labelAnchor === 'middle'
              ? labelX - labelWidth / 2
              : (labelAnchor === 'end' ? labelX - labelWidth : labelX);
            return (
              <g
                key={node.id}
                onMouseDown={(e) => handleNodeMouseDown(e, node.id)}
                onClick={(e) => handleNodeClick(e, node.id)}
                onMouseEnter={() => setHoveredNode(node.id)}
                onMouseLeave={() => setHoveredNode(null)}
                style={{
                  cursor: 'grab',
                  // 旁观对话节点用半透明渲染（表现旁观而非参与）
                  opacity: active ? (node.bystander ? 0.5 : 1) : 0.2,
                  transition: `opacity ${DURATION.fast}s ${EASE.swift}, transform ${DURATION.fast}s ${EASE.spring}`,
                  transform: isHighlighted ? `rotate(${tilt}deg) scale(1.1)` : `rotate(${tilt}deg) scale(1)`,
                  transformOrigin: `${pos.x}px ${pos.y}px`,
                }}
              >
                {selectedNode === node.id && (
                  <circle
                    cx={pos.x}
                    cy={pos.y}
                    r={size / 2 + 6}
                    fill="none"
                    stroke={pastel(node.color)}
                    strokeWidth={2}
                    strokeDasharray="5 4"
                  />
                )}
                {hoveredNode === node.id && selectedNode !== node.id && (
                  <circle
                    cx={pos.x}
                    cy={pos.y}
                    r={size / 2 + 5}
                    fill="none"
                    stroke={pastel(node.color)}
                    strokeWidth={1.5}
                    strokeDasharray="3 3"
                  />
                )}
                <MemoNodeShape
                  type={node.type}
                  color={node.color}
                  size={size}
                  x={pos.x}
                  y={pos.y}
                  isCore={isCore}
                  expanded={isSummaryExpanded}
                />
                {/* 标签背景（session_summary 展开后隐藏，避免和子节点重叠） */}
                {(active || isCore) && !(node.type === 'session_summary' && isSummaryExpanded) && (
                  <rect
                    x={rectX}
                    y={labelY - 10}
                    width={labelWidth}
                    height={17}
                    rx={3}
                    fill={'var(--graph-card-92)'}
                    stroke={GPAPER.inkFaint}
                    strokeWidth={0.8}
                    style={{ pointerEvents: 'none' }}
                  />
                )}
                {!(node.type === 'session_summary' && isSummaryExpanded) && (
                  <text
                    x={rectX + labelWidth / 2}
                    y={labelY + 2.5}
                    textAnchor="middle"
                    fill={active ? GPAPER.ink : GPAPER.inkSoft}
                    fontSize={labelFontSize}
                    fontWeight={isHighlighted || isCore ? 600 : 500}
                    fontFamily={HAND}
                    style={{ pointerEvents: 'none', userSelect: 'none' }}
                  >
                    {node.label}
                  </text>
                )}
              </g>
            );
          })}

          {/* 回应箭头：手绘墨线，从回复指向被回应的那句话（压在节点上层） */}
          {replyArrows.map((ra) => {
            const fp = getNodePos(ra.fromId);
            const tp = getNodePos(ra.toId);
            if (!fp || !tp) return null;
            if (fp.y < visibleRange.topY || fp.y > visibleRange.bottomY) return null;
            if (tp.y < visibleRange.topY || tp.y > visibleRange.bottomY) return null;
            const fromNode = nodeMap.get(ra.fromId);
            const toNode = nodeMap.get(ra.toId);
            const trimA = (fromNode ? nodeSize(fromNode.importance) : 24) / 2 + 4;
            const trimB = (toNode ? nodeSize(toNode.importance) : 24) / 2 + 4;
            const dist = Math.hypot(tp.x - fp.x, tp.y - fp.y);
            const bow = Math.min(64, Math.max(24, dist * 0.3));
            const fanOffset = fanOffsetMap.get(ra.id) ?? 0;
            const dimmed = !!connectedSet && (!connectedSet.has(ra.fromId) || !connectedSet.has(ra.toId));
            return (
              <path
                key={ra.id}
                d={handDrawnArrowPath(fp.x, fp.y, tp.x, tp.y, ra.id, bow, trimA, trimB, fanOffset)}
                fill="none"
                stroke={hexToRgba(GPAPER.ink, 0.6)}
                strokeWidth={1.3}
                strokeLinecap="round"
                strokeLinejoin="round"
                opacity={dimmed ? 0.3 : 1}
                style={{ pointerEvents: 'none' }}
              />
            );
          })}
        </svg>

        {/* Tooltip - 按节点类型使用不同形状的便签（角色节点 user/agent 不显示，episode/important_event/reading 长内容类型也不显示，改用点击弹窗） */}
        {tooltipAnchor && hoveredGraphNode
          && hoveredGraphNode.type !== 'user'
          && hoveredGraphNode.type !== 'agent'
          && hoveredGraphNode.type !== 'episode'
          && hoveredGraphNode.type !== 'important_event'
          && hoveredGraphNode.type !== 'reading'
          && (() => {
          const ntype = hoveredGraphNode.type;
          const ncolor = pastel(hoveredGraphNode.color);
          const typeLabel = t(`mind_inspector.graph.${NODE_TYPE_KEYS[ntype]}`);
          const timeLabel = formatTimeLabel(hoveredGraphNode.timestamp, Date.now());

          // 各形状共用的头部（类型标签 + 时间）
          const header = (
            <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs, marginBottom: SPACING.sm, fontFamily: 'sans-serif' }}>
              <span style={{
                display: 'inline-flex', alignItems: 'center', gap: 4,
                padding: '2px 8px', fontSize: 10, fontWeight: 600,
                color: GPAPER.ink, background: ncolor,
                letterSpacing: 0.3, borderRadius: 0, transform: 'rotate(-1deg)',
              }}>
                {typeLabel}
              </span>
              <span style={{ color: 'var(--panel-text-tertiary)', fontSize: 10, fontFamily: 'sans-serif' }}>{timeLabel}</span>
            </div>
          );

          // 内心OS → 云朵形状（白底 + 浅蓝内轮廓边框）
          if (ntype === 'inner_thought') {
            return (
              <div style={{
                position: 'absolute',
                left: tooltipAnchor.left,
                top: tooltipAnchor.top,
                transform: tooltipAnchor.transform,
                maxWidth: 280, pointerEvents: 'none', zIndex: 10,
              }}>
                {/* 云朵凸起 */}
                <span aria-hidden style={{ position: 'absolute', top: -12, left: 22, width: 44, height: 44, borderRadius: '50%', background: 'var(--panel-surface)', boxShadow: '0 2px 8px rgba(135,180,220,0.15)' }} />
                <span aria-hidden style={{ position: 'absolute', top: -18, left: 58, width: 56, height: 56, borderRadius: '50%', background: 'var(--panel-surface)', boxShadow: '0 2px 8px rgba(135,180,220,0.15)' }} />
                <span aria-hidden style={{ position: 'absolute', top: -10, right: 30, width: 40, height: 40, borderRadius: '50%', background: 'var(--panel-surface)', boxShadow: '0 2px 8px rgba(135,180,220,0.15)' }} />
                {/* 云朵主体 */}
                <div style={{
                  position: 'relative', background: 'var(--panel-surface)',
                  borderRadius: '28px 32px 30px 26px / 30px 26px 32px 28px',
                  padding: '22px 20px 16px',
                  boxShadow: '0 4px 16px rgba(135,180,220,0.20), var(--graph-shadow-sm)',
                  fontFamily: '"Caveat", "Ma Shan Zheng", "Dancing Script", cursive',
                  fontSize: 15, lineHeight: 1.55, wordBreak: 'break-word',
                }}>
                  {/* 内轮廓边框（比外缘小一圈） */}
                  <span aria-hidden style={{
                    position: 'absolute', inset: 5,
                    border: '1.5px solid rgba(135,180,220,0.55)',
                    borderRadius: '24px 28px 26px 22px / 26px 22px 28px 24px',
                    pointerEvents: 'none',
                  }} />
                  <div style={{ position: 'relative' }}>
                    {header}
                    <div style={{ color: GPAPER.ink }} dangerouslySetInnerHTML={{ __html: renderTextWithActions(hoveredGraphNode.preview) }} />
                  </div>
                </div>
              </div>
            );
          }

          // 对话 / 微信 → 气泡形状（带小尾巴）
          if (ntype === 'dialogue' || ntype === 'wechat' || ntype === 'topic_summary') {
            return (
              <div style={{
                position: 'absolute',
                left: tooltipAnchor.left,
                top: tooltipAnchor.top,
                transform: tooltipAnchor.transform,
                maxWidth: 280, pointerEvents: 'none', zIndex: 10,
              }}>
                <div style={{
                  position: 'relative', background: 'var(--panel-surface)',
                  borderRadius: '16px 16px 16px 4px',
                  border: '1px solid rgba(180,190,205,0.45)',
                  padding: '14px 16px 12px',
                  boxShadow: 'var(--graph-shadow-md)',
                  fontFamily: '"Caveat", "Ma Shan Zheng", "Dancing Script", cursive',
                  fontSize: 15, lineHeight: 1.55, wordBreak: 'break-word',
                }}>
                  {header}
                  <div style={{ color: GPAPER.ink }}>
                    {renderParenText(
                      hoveredGraphNode.preview || '',
                      undefined,
                      { color: '#8B5CF6', fontStyle: 'italic', fontFamily: '"Ma Shan Zheng", cursive' }
                    )}
                  </div>
                </div>
                {/* 气泡尾巴 */}
                <span aria-hidden style={{
                  position: 'absolute', bottom: -7, left: 24,
                  width: 14, height: 14, background: 'var(--panel-surface)',
                  borderRight: '1px solid rgba(180,190,205,0.45)',
                  borderBottom: '1px solid rgba(180,190,205,0.45)',
                  transform: 'rotate(45deg)',
                }} />
              </div>
            );
          }

          // 信念 → 琥珀色内边框圆角卡片
          if (ntype === 'belief') {
            return (
              <div style={{
                position: 'absolute',
                left: tooltipAnchor.left,
                top: tooltipAnchor.top,
                transform: `${tooltipAnchor.transform} rotate(-1deg)`,
                maxWidth: 280, pointerEvents: 'none', zIndex: 10,
              }}>
                <div style={{
                  position: 'relative', background: GPAPER.card,
                  borderRadius: 14, padding: '14px 16px 12px',
                  boxShadow: `2px 3px 10px ${GPAPER.shadowSm}`,
                  fontFamily: '"Caveat", "Ma Shan Zheng", "Dancing Script", cursive',
                  fontSize: 15, lineHeight: 1.55, wordBreak: 'break-word',
                }}>
                  <span aria-hidden style={{ position: 'absolute', inset: 4, border: '1.5px solid rgba(212,175,105,0.5)', borderRadius: 10, pointerEvents: 'none' }} />
                  <div style={{ position: 'relative' }}>
                    {header}
                    <div style={{ color: GPAPER.ink }} dangerouslySetInnerHTML={{ __html: renderTextWithActions(hoveredGraphNode.preview) }} />
                  </div>
                </div>
              </div>
            );
          }

          // 会话摘要 → 票根形状 + 子节点计数提示
          if (ntype === 'session_summary') {
            const childCount = hoveredGraphNode.childIds?.length ?? 0;
            return (
              <div style={{
                position: 'absolute',
                left: tooltipAnchor.left,
                top: tooltipAnchor.top,
                transform: `${tooltipAnchor.transform} rotate(0.5deg)`,
                maxWidth: 280, pointerEvents: 'none', zIndex: 10,
              }}>
                <div style={{
                  position: 'relative', background: GPAPER.card,
                  borderRadius: '4px 12px 12px 4px',
                  border: `1px solid ${hexToRgba('#7C3AED', 0.4)}`,
                  borderLeft: `3px solid #7C3AED`,
                  padding: '12px 16px 12px 14px',
                  boxShadow: `2px 3px 8px ${GPAPER.shadowSm}`,
                  fontFamily: '"Caveat", "Ma Shan Zheng", "Dancing Script", cursive',
                  fontSize: 15, lineHeight: 1.55, wordBreak: 'break-word',
                }}>
                  {header}
                  {!expandedSummaryIds.has(hoveredGraphNode.id) && (
                    <div style={{ color: GPAPER.ink }} dangerouslySetInnerHTML={{ __html: renderTextWithActions(hoveredGraphNode.preview) }} />
                  )}
                  {childCount > 0 && (
                    <div style={{ marginTop: 6, fontSize: 12, color: '#7C3AED', fontFamily: 'system-ui, sans-serif' }}>
                      {expandedSummaryIds.has(hoveredGraphNode.id) ? '点击收起' : '点击展开'} {childCount} 条原始对话
                    </div>
                  )}
                </div>
              </div>
            );
          }

          // 目标 → 旗帜形状（右侧尖角）
          if (ntype === 'goal') {
            return (
              <div style={{
                position: 'absolute',
                left: tooltipAnchor.left,
                top: tooltipAnchor.top,
                transform: tooltipAnchor.transform,
                maxWidth: 280, pointerEvents: 'none', zIndex: 10,
              }}>
                <div style={{
                  position: 'relative', background: GPAPER.card,
                  borderRadius: '10px 2px 2px 10px',
                  border: `1px solid ${'var(--panel-border)'}`,
                  borderRight: `3px solid ${ncolor}`,
                  padding: '12px 18px 12px 14px',
                  boxShadow: `2px 3px 8px ${GPAPER.shadowSm}`,
                  fontFamily: '"Caveat", "Ma Shan Zheng", "Dancing Script", cursive',
                  fontSize: 15, lineHeight: 1.55, wordBreak: 'break-word',
                }}>
                  {header}
                  <div style={{ color: GPAPER.ink }} dangerouslySetInnerHTML={{ __html: renderTextWithActions(hoveredGraphNode.preview) }} />
                </div>
              </div>
            );
          }

          // 关系 → 粉色圆角卡片
          if (ntype === 'relationship') {
            return (
              <div style={{
                position: 'absolute',
                left: tooltipAnchor.left,
                top: tooltipAnchor.top,
                transform: `${tooltipAnchor.transform} rotate(-0.5deg)`,
                maxWidth: 280, pointerEvents: 'none', zIndex: 10,
              }}>
                <div style={{
                  position: 'relative', background: 'var(--panel-surface)',
                  borderRadius: 18, padding: '14px 16px 12px',
                  boxShadow: '0 3px 12px rgba(220,160,175,0.18)',
                  fontFamily: '"Caveat", "Ma Shan Zheng", "Dancing Script", cursive',
                  fontSize: 15, lineHeight: 1.55, wordBreak: 'break-word',
                }}>
                  <span aria-hidden style={{ position: 'absolute', inset: 4, border: '1.5px solid rgba(235,170,185,0.5)', borderRadius: 14, pointerEvents: 'none' }} />
                  <div style={{ position: 'relative' }}>
                    {header}
                    <div style={{ color: GPAPER.ink }} dangerouslySetInnerHTML={{ __html: renderTextWithActions(hoveredGraphNode.preview) }} />
                  </div>
                </div>
              </div>
            );
          }

          // 日记 → 横线笔记页
          if (ntype === 'diary') {
            return (
              <div style={{
                position: 'absolute',
                left: tooltipAnchor.left,
                top: tooltipAnchor.top,
                transform: `${tooltipAnchor.transform} rotate(0.5deg)`,
                maxWidth: 280, pointerEvents: 'none', zIndex: 10,
              }}>
                <div style={{
                  position: 'relative', background: GPAPER.card,
                  borderRadius: 6, padding: '12px 16px 12px 22px',
                  border: `1px solid ${'var(--panel-border)'}`,
                  backgroundImage: `repeating-linear-gradient(transparent, transparent 21px, ${GPAPER.line} 21px, ${GPAPER.line} 22px)`,
                  boxShadow: `2px 3px 8px ${GPAPER.shadowSm}`,
                  fontFamily: '"Caveat", "Ma Shan Zheng", "Dancing Script", cursive',
                  fontSize: 15, lineHeight: '22px', wordBreak: 'break-word',
                }}>
                  {/* 左侧红色页边线 */}
                  <span aria-hidden style={{ position: 'absolute', top: 0, bottom: 0, left: 14, width: 1, background: 'rgba(201,64,58,0.35)', pointerEvents: 'none' }} />
                  {header}
                  <div style={{ color: GPAPER.ink }} dangerouslySetInnerHTML={{ __html: renderTextWithActions(hoveredGraphNode.preview) }} />
                </div>
              </div>
            );
          }

          // 默认（user / agent 等）→ 便签风格
          return (
            <div style={{
              position: 'absolute',
              left: tooltipAnchor.left,
              top: tooltipAnchor.top,
              transform: `${tooltipAnchor.transform} rotate(-1deg)`,
              maxWidth: 280, padding: `${SPACING.md + 4}px`,
              borderRadius: 2, background: GPAPER.card,
              borderLeft: `4px solid ${ncolor}`,
              borderRight: `1px solid ${'var(--panel-border)'}`,
              borderBottom: `1px solid ${'var(--panel-border)'}`,
              borderTop: `1px solid ${'var(--panel-border)'}`,
              fontFamily: '"Caveat", "Ma Shan Zheng", "Dancing Script", cursive',
              fontSize: 15, color: GPAPER.ink, pointerEvents: 'none', zIndex: 10,
              boxShadow: `2px 3px 8px ${GPAPER.shadowSm}, -1px -1px 4px ${GPAPER.shadowSm}`,
              wordBreak: 'break-word', lineHeight: 1.55,
            }}>
              <span aria-hidden style={{
                position: 'absolute', top: -8, left: '50%',
                width: 56, height: 14, transform: 'translateX(-50%) rotate(-2deg)',
                background: `repeating-linear-gradient(45deg, rgba(255,255,255,0.35) 0 4px, rgba(255,255,255,0) 4px 8px), ${hexToRgba(CHARACTER_ACCENT[character], 0.55)}`,
                borderRadius: 1, boxShadow: GPAPER.shadowSm, pointerEvents: 'none',
              }} />
              {header}
              <div style={{ color: GPAPER.ink, fontStyle: 'normal' }} dangerouslySetInnerHTML={{ __html: renderTextWithActions(hoveredGraphNode.preview) }} />
            </div>
          );
        })()}

        {/* 时间轴方向提示 */}
        <div
          style={{
            position: 'absolute',
            left: SPACING.sm,
            top: '50%',
            transform: 'rotate(-90deg) translateX(-50%)',
            transformOrigin: 'left center',
            fontFamily: HAND,
            fontSize: 14,
            color: GPAPER.inkSoft,
            letterSpacing: 2,
            pointerEvents: 'none',
            opacity: 0.6,
          }}
        >
          时间线 →
        </div>
      </div>

        {/* 迷你地图：日期跳转导航 */}
        <Minimap
          skeleton={skeleton}
          scale={scale}
          height={containerHeight}
          visibleRange={visibleRange}
          svgHeight={svgHeight}
          accent={CHARACTER_ACCENT[character]}
          onScrollRequest={(svgY) => {
            const container = containerRef.current;
            if (container) container.scrollTo({ top: Math.max(0, svgYToScrollTop(svgY)), behavior: 'smooth' });
          }}
        />
      </div>

      {/* 底部统计 */}
      <div style={{ display: 'flex', gap: SPACING.cardGap, flexWrap: 'wrap' }}>
        <StickerChip color={COLORS.event.belief} rot={-1.2}>
          {t('mind_inspector.graph.type_belief')} · {beliefs.length}
        </StickerChip>
        <StickerChip color={COLORS.event.reading} rot={0.5}>
          {t('mind_inspector.graph.type_reading')} · {graphNodes.filter((n) => n.type === 'reading').length}
        </StickerChip>
        <StickerChip color={COLORS.event.observation} rot={0.9}>
          {t('mind_inspector.graph.type_episode')} · {skeleton.filter((p) => p.kind === 'memory').length - graphNodes.filter((n) => n.type === 'reading').length}
        </StickerChip>
        <StickerChip color={COLORS.event.goal} rot={-0.7}>
          {t('mind_inspector.graph.type_goal')} · {(mind?.goals ?? []).length}
        </StickerChip>
        <StickerChip color={COLORS.event.relationship} rot={1.3}>
          {t('mind_inspector.graph.type_relationship')} · {relationships.length}
        </StickerChip>
      </div>

      {/* 内容弹窗：点击节点时显示完整内容 */}
      {modalNode && (() => {
        const ntype = modalNode.type;
        const ncolor = pastel(modalNode.color);
        const typeLabel = t(`mind_inspector.graph.${NODE_TYPE_KEYS[ntype]}`);
        const timeLabel = formatTimeLabel(modalNode.timestamp, Date.now());
        return (
          <div
            onClick={(e) => {
              e.stopPropagation();
              setModalNode(null);
              setSelectedNode(null);
            }}
            style={{
              position: 'fixed',
              inset: 0,
              zIndex: 1000,
              background: 'rgba(0,0,0,0.45)',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              padding: '20px',
            }}
          >
            <div
              onClick={(e) => e.stopPropagation()}
              style={{
                background: GPAPER.paper,
                borderRadius: RADIUS.xl,
                maxWidth: 'min(680px, 90vw)',
                width: '100%',
                maxHeight: '80vh',
                display: 'flex',
                flexDirection: 'column',
                boxShadow: '0 20px 60px rgba(0,0,0,0.3)',
                border: `1.5px solid ${GPAPER.border}`,
              }}
            >
              {/* 弹窗头部 */}
              <div style={{
                padding: '18px 22px 14px',
                borderBottom: `1px dashed ${GPAPER.border}`,
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                gap: 12,
              }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                  <span style={{
                    display: 'inline-flex',
                    alignItems: 'center',
                    padding: '3px 10px',
                    fontSize: 12,
                    fontWeight: 600,
                    color: GPAPER.ink,
                    background: ncolor,
                    borderRadius: 3,
                    transform: 'rotate(-1deg)',
                  }}>
                    {typeLabel}
                  </span>
                  <span style={{ color: 'var(--panel-text-tertiary)', fontSize: 12, fontFamily: 'system-ui, sans-serif' }}>
                    {timeLabel}
                  </span>
                </div>
                <button
                  onClick={() => {
                    setModalNode(null);
                    setSelectedNode(null);
                  }}
                  style={{
                    width: 28,
                    height: 28,
                    borderRadius: 6,
                    border: 'none',
                    background: 'transparent',
                    cursor: 'pointer',
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    color: GPAPER.inkSoft,
                    fontSize: 18,
                    lineHeight: 1,
                  }}
                  onMouseEnter={(e) => {
                    (e.currentTarget as HTMLButtonElement).style.background = 'var(--panel-surface-hover)';
                  }}
                  onMouseLeave={(e) => {
                    (e.currentTarget as HTMLButtonElement).style.background = 'transparent';
                  }}
                >
                  ×
                </button>
              </div>
              {/* 弹窗内容区 */}
              <div style={{
                flex: 1,
                overflowY: 'auto',
                padding: '20px 22px',
                fontSize: 15,
                lineHeight: 1.7,
                color: GPAPER.ink,
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-word',
                fontFamily: '"Caveat", "Ma Shan Zheng", "Dancing Script", "Hachi Maru Pop", "Kaiti SC", "KaiTi", "STKaiti", "DFKai-SB", "PingFang SC", "Microsoft YaHei", serif',
              }}>
                {(() => {
                  // 微信分享链接卡片：结构化展示（标题/描述/来源/可点击 URL）
                  const lc = modalNode.metadata?.link_card as
                    | { url?: string; title?: string; description?: string; source?: string }
                    | undefined;
                  if (modalNode.type === 'wechat' && lc && lc.url && lc.title) {
                    const sourceLabel = lc.source || (() => {
                      try { return new URL(lc.url!).hostname; } catch { return lc.url; }
                    })();
                    return (
                      <div>
                        <div
                          onClick={() => { const u = lc.url; if (u) { void openShell(u).catch(() => window.open(u, '_blank', 'noopener,noreferrer')); } }}
                          onMouseEnter={(e) => { e.currentTarget.style.boxShadow = '0 2px 12px rgba(0,0,0,0.15)'; }}
                          onMouseLeave={(e) => { e.currentTarget.style.boxShadow = '0 1px 3px rgba(0,0,0,0.1)'; }}
                          style={{
                            width: '100%',
                            maxWidth: 360,
                            borderRadius: 8,
                            background: GPAPER.card,
                            border: `1px solid ${GPAPER.border}`,
                            boxShadow: '0 1px 3px rgba(0,0,0,0.1)',
                            cursor: 'pointer',
                            overflow: 'hidden',
                            margin: '0 auto 16px',
                          }}
                        >
                          <div style={{
                            padding: '12px 14px 10px',
                            borderBottom: `1px dashed ${GPAPER.border}`,
                            display: 'flex',
                            alignItems: 'flex-start',
                            gap: 10,
                          }}>
                            <div style={{
                              width: 52,
                              height: 52,
                              borderRadius: 6,
                              background: 'linear-gradient(135deg, #f0f7ff 0%, #e6f0fa 100%)',
                              display: 'flex',
                              alignItems: 'center',
                              justifyContent: 'center',
                              flexShrink: 0,
                            }}>
                              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#5b8def" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
                                <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" />
                                <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" />
                              </svg>
                            </div>
                            <div style={{ flex: 1, minWidth: 0 }}>
                              <div style={{
                                fontSize: 14,
                                fontWeight: 500,
                                color: GPAPER.ink,
                                lineHeight: 1.4,
                                display: '-webkit-box',
                                WebkitLineClamp: 2,
                                WebkitBoxOrient: 'vertical',
                                overflow: 'hidden',
                                textOverflow: 'ellipsis',
                                wordBreak: 'break-all',
                                marginBottom: lc.description ? 4 : 0,
                              }}>
                                {lc.title}
                              </div>
                              {lc.description && (
                                <div style={{
                                  fontSize: 12,
                                  color: GPAPER.inkSoft,
                                  lineHeight: 1.4,
                                  display: '-webkit-box',
                                  WebkitLineClamp: 2,
                                  WebkitBoxOrient: 'vertical',
                                  overflow: 'hidden',
                                  textOverflow: 'ellipsis',
                                  wordBreak: 'break-all',
                                }}>
                                  {lc.description}
                                </div>
                              )}
                            </div>
                          </div>
                          <div style={{
                            padding: '6px 14px',
                            display: 'flex',
                            alignItems: 'center',
                            gap: 4,
                            fontSize: 11,
                            color: GPAPER.inkSoft,
                          }}>
                            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                              <circle cx="12" cy="12" r="10" />
                              <line x1="2" y1="12" x2="22" y2="12" />
                              <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
                            </svg>
                            <span style={{
                              overflow: 'hidden',
                              textOverflow: 'ellipsis',
                              whiteSpace: 'nowrap',
                              flex: 1,
                            }}>
                              {sourceLabel}
                            </span>
                          </div>
                        </div>
                        {/* 跟进评论（preview 中 URL 之后的文本） */}
                        {(() => {
                          const urlIdx = modalNode.preview.indexOf(lc.url!);
                          const after = urlIdx >= 0 ? modalNode.preview.slice(urlIdx + lc.url!.length).trim() : '';
                          return after ? <div style={{ marginTop: 8 }}>{after}</div> : null;
                        })()}
                      </div>
                    );
                  }
                  return modalNode.preview;
                })()}
              </div>
            </div>
          </div>
        );
      })()}
    </div>
  );
};

export default GraphPage;
