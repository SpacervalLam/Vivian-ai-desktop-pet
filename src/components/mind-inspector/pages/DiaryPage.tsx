/**
 * Diary 页 — 日记列表 + 详情（手账风格）
 *
 * 数据源：invoke('get_diary_entries', { characterId })
 * 刷新：监听 'diary:written' 事件
 *
 * 布局：左右两栏（左 40% 便签列表 + 右 60% 信纸详情）
 * 顶部和纸胶带切换角色（vivian / nana）
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { TYPO, SPACING, EASE, DURATION, CHARACTER_ACCENT } from '../design-system';
import { useNavigation } from '../NavigationContext';

// ============================================================
// 类型定义
// ============================================================

interface DiaryEntry {
  id: string;
  date: string;
  start_time: number;
  end_time: number;
  content: string;
  key_events: string[];
  mood_average: unknown;
  word_count: number;
  interaction_count: number;
  trigger_type: string;
  trigger_score: number;
  mood_tag: string;
  created_at: number;
}

type CharacterId = 'vivian' | 'nana';

// ============================================================
// 手账视觉常量
// ============================================================

const JOURNAL = {
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
  lineBlue: 'rgba(122,158,199,0.30)',
  marginPink: 'rgba(214,116,133,0.50)',
  tape: ['#F2CD88', '#F3B8C4', '#C9E4D3', '#C9DDF2', '#DFD3F0'],
} as const;

/** 角色切换标签与卡片胶带用色 */
const CHAR_TAPE: Record<CharacterId, string> = {
  vivian: '#FFE88A',
  nana: '#DDC6FF',
};

const HAND_BODY =
  '"Caveat", "Ma Shan Zheng", "Dancing Script", "Hachi Maru Pop", "Kaiti SC", "KaiTi", "STKaiti", "DFKai-SB", "Noto Serif SC", "PingFang SC", "Microsoft YaHei", serif';

/** 正文段落排版方式：按正文字种自动选择 */
type ParagraphMode = 'indent-jp' | 'indent-cn' | 'blank-line';

/** 含假名判定为日文（首行缩进一字），仅含汉字判定为中文（缩进两字），其余为西文（段间空行、不缩进） */
function detectParagraphMode(text: string): ParagraphMode {
  if (/[\u3040-\u309F\u30A0-\u30FF]/.test(text)) return 'indent-jp';
  if (/[\u4E00-\u9FFF\u3400-\u4DBF]/.test(text)) return 'indent-cn';
  return 'blank-line';
}

const MOOD_COLORS: Record<string, string> = {
  happy: '#D98E2B',
  good: '#6F9A5E',
  neutral: '#8F877A',
  sad: '#5F83A8',
  angry: '#C9403A',
  bored: '#8F877A',
  tired: '#9C7BAB',
};

const MOOD_EMOJI: Record<string, string> = {
  happy: '☀️',
  good: '😊',
  neutral: '😐',
  sad: '😢',
  angry: '😠',
  bored: '😴',
  tired: '😪',
};

const WEEKDAY_KEYS = [
  'weekday_sun',
  'weekday_mon',
  'weekday_tue',
  'weekday_wed',
  'weekday_thu',
  'weekday_fri',
  'weekday_sat',
];

// ============================================================
// 关键帧注入（一次性）
// ============================================================

const KEYFRAMES_ID = 'diary-page-keyframes';
if (typeof document !== 'undefined' && !document.getElementById(KEYFRAMES_ID)) {
  const style = document.createElement('style');
  style.id = KEYFRAMES_ID;
  style.textContent = `
@keyframes diary-slip-in {
  0% { opacity: 0; transform: translateY(18px) rotate(3deg); }
  100% { opacity: 1; transform: translateY(0) rotate(0deg); }
}
@keyframes diary-page-in {
  0% { opacity: 0; transform: translateY(12px) rotate(0.6deg); }
  100% { opacity: 1; transform: translateY(0) rotate(0deg); }
}
@keyframes diary-stamp-pop {
  0% { opacity: 0; transform: scale(1.9) rotate(-20deg); }
  62% { opacity: 1; transform: scale(0.92) rotate(-8deg); }
  100% { opacity: 1; transform: scale(1) rotate(-8deg); }
}
@keyframes diary-float {
  0%, 100% { transform: translateY(0) rotate(-3deg); }
  50% { transform: translateY(-7px) rotate(3deg); }
}
@keyframes diary-pencil {
  0%, 100% { transform: rotate(-10deg) translateY(0); }
  50% { transform: rotate(8deg) translateY(-4px); }
}
@keyframes diary-spin {
  to { transform: rotate(360deg); }
}`;
  document.head.appendChild(style);
}

// ============================================================
// 时间工具
// ============================================================

const parseDate = (s: string): Date | null => {
  if (!s) return null;
  const d = new Date(`${s}T00:00:00`);
  return Number.isNaN(d.getTime()) ? null : d;
};

const formatListDate = (s: string): string => {
  const d = parseDate(s);
  if (!d) return s;
  const month = d.getMonth() + 1;
  const day = d.getDate();
  return `${d.getFullYear()}-${String(month).padStart(2, '0')}-${String(day).padStart(2, '0')}`;
};

const formatDetailTitle = (s: string, t: TFunction): string => {
  const d = parseDate(s);
  if (!d) return s;
  const weekdayKey = WEEKDAY_KEYS[d.getDay()];
  return t('mind_inspector.diary.detail_title', {
    year: d.getFullYear(),
    month: d.getMonth() + 1,
    day: d.getDate(),
    weekday: t(`mind_inspector.diary.${weekdayKey}`),
  });
};

const formatCreated = (ts: number, t: TFunction): string => {
  if (!ts || ts <= 0) return '—';
  const ms = ts < 1e12 ? ts * 1000 : ts;
  const diff = Math.max(0, Date.now() - ms);
  const min = 60_000;
  const hour = 3_600_000;
  const day = 86_400_000;
  if (diff < min) return t('mind_inspector.common.just_now');
  if (diff < hour) return t('mind_inspector.common.minutes_ago', { n: Math.floor(diff / min) });
  if (diff < day) return t('mind_inspector.common.hours_ago', { n: Math.floor(diff / hour) });
  if (diff < 7 * day) return t('mind_inspector.common.days_ago', { n: Math.floor(diff / day) });
  const d = new Date(ms);
  const pad = (x: number) => String(x).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
};

// ============================================================
// 颜色与高亮工具
// ============================================================

const hexToRgba = (hex: string, alpha: number): string => {
  const h = hex.replace('#', '');
  const full = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  const r = parseInt(full.slice(0, 2), 16);
  const g = parseInt(full.slice(2, 4), 16);
  const b = parseInt(full.slice(4, 6), 16);
  return `rgba(${r},${g},${b},${alpha})`;
};

/** 用主题色荧光笔高亮命中的关键词（大小写不敏感） */
const highlightText = (text: string, query: string, accent: string): React.ReactNode => {
  const q = query.trim();
  if (!q) return text;
  const lower = text.toLowerCase();
  const needle = q.toLowerCase();
  const parts: React.ReactNode[] = [];
  let cursor = 0;
  let idx = lower.indexOf(needle);
  let key = 0;
  while (idx !== -1) {
    if (idx > cursor) parts.push(text.slice(cursor, idx));
    parts.push(
      <span
        key={`hl-${key++}`}
        style={{
          background: hexToRgba(accent, 0.4),
          borderRadius: 2,
          padding: '0 1px',
          WebkitBoxDecorationBreak: 'clone',
          boxDecorationBreak: 'clone',
        }}
      >
        {text.slice(idx, idx + needle.length)}
      </span>,
    );
    cursor = idx + needle.length;
    idx = lower.indexOf(needle, cursor);
  }
  if (cursor < text.length) parts.push(text.slice(cursor));
  return parts;
};

// ============================================================
// 基础手账元素
// ============================================================

interface TapeStripProps {
  color: string;
  width?: number;
  style?: React.CSSProperties;
}

/** 和纸胶带（斜纹半透明贴纸） */
const TapeStrip: React.FC<TapeStripProps> = ({ color, width = 78, style }) => (
  <div
    aria-hidden
    style={{
      position: 'absolute',
      top: -9,
      left: '50%',
      width,
      height: 20,
      transform: 'translateX(-50%) rotate(-2.5deg)',
      background: `repeating-linear-gradient(45deg, rgba(255,255,255,0.35) 0 5px, rgba(255,255,255,0) 5px 10px), ${color}`,
      opacity: 0.92,
      borderRadius: 2,
      boxShadow: JOURNAL.shadowSm,
      pointerEvents: 'none',
      ...style,
    }}
  />
);

/** 红色小印章（区块标题装饰） */
const SealMark: React.FC<{ char: string; size?: number }> = ({ char, size = 22 }) => (
  <span
    aria-hidden
    style={{
      width: size,
      height: size,
      flexShrink: 0,
      display: 'inline-flex',
      alignItems: 'center',
      justifyContent: 'center',
      background: JOURNAL.stampRed,
      color: '#FFF9EE',
      borderRadius: 4,
      transform: 'rotate(-6deg)',
      boxShadow:
        `inset 0 0 0 1.5px rgba(255,249,238,0.55), ${JOURNAL.shadowSm}`,
      fontFamily: TYPO.fontFamilyCN,
      fontSize: size * 0.62,
      lineHeight: 1,
    }}
  >
    {char}
  </span>
);

/** 区块标题：印章 + 楷体标题 + 虚线延伸线 */
const JournalSectionTitle: React.FC<{
  seal: string;
  title: React.ReactNode;
  style?: React.CSSProperties;
}> = ({ seal, title, style }) => (
  <div style={{ display: 'flex', alignItems: 'center', gap: 9, ...style }}>
    <SealMark char={seal} />
    <span
      style={{
        fontFamily: TYPO.fontFamilyCN,
        fontSize: 17.5,
        color: JOURNAL.ink,
        letterSpacing: 2.5,
        whiteSpace: 'nowrap',
      }}
    >
      {title}
    </span>
    <span
      aria-hidden
      style={{
        flex: 1,
        borderBottom: `1px dashed ${JOURNAL.border}`,
        transform: 'translateY(3px)',
      }}
    />
  </div>
);

/** 便签纸容器（虚线边 + 胶带，用于空状态） */
const PaperNote: React.FC<{
  tapeColor?: string;
  children: React.ReactNode;
  style?: React.CSSProperties;
}> = ({ tapeColor, children, style }) => (
  <div
    style={{
      position: 'relative',
      background: JOURNAL.card,
      border: `1px dashed ${JOURNAL.border}`,
      borderRadius: 5,
      padding: '30px 38px',
      boxShadow: JOURNAL.shadowLg,
      transform: 'rotate(-0.7deg)',
      display: 'flex',
      flexDirection: 'column',
      alignItems: 'center',
      gap: SPACING.sm,
      ...style,
    }}
  >
    <TapeStrip color={tapeColor ?? JOURNAL.tape[0]} width={86} />
    {children}
  </div>
);

const NoteText: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div
    style={{
      fontFamily: HAND_BODY,
      fontSize: 15.5,
      color: JOURNAL.inkSoft,
      letterSpacing: 1,
      textAlign: 'center',
    }}
  >
    {children}
  </div>
);

const Center: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div
    style={{
      flex: 1,
      display: 'flex',
      alignItems: 'center',
      justifyContent: 'center',
      minHeight: 220,
    }}
  >
    {children}
  </div>
);

/** 页面外壳：暖纸底 + 点阵 + 角落光晕 + 漂浮贴纸 */
const PageShell: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <div
    style={{
      flex: 1,
      display: 'flex',
      flexDirection: 'column',
      gap: SPACING.md,
      minHeight: 0,
      position: 'relative',
      background: `radial-gradient(900px 480px at 88% -8%, rgba(243,184,196,0.22), transparent 62%), radial-gradient(820px 560px at -6% 108%, rgba(201,228,211,0.26), transparent 62%), ${JOURNAL.paper}`,
      borderRadius: 10,
      border: `1px solid ${JOURNAL.border}`,
      padding: `${SPACING.md + 2}px ${SPACING.md + 2}px ${SPACING.md}px`,
      overflow: 'hidden',
    }}
  >
    <div
      aria-hidden
      style={{
        position: 'absolute',
        inset: 0,
        background: `radial-gradient(${JOURNAL.inkFaint} 1px, transparent 1.25px)`,
        backgroundSize: '20px 20px',
        pointerEvents: 'none',
      }}
    />
    <span
      aria-hidden
      style={{
        position: 'absolute',
        top: 10,
        right: 20,
        fontSize: 30,
        opacity: 0.2,
        animation: 'diary-float 7s ease-in-out infinite',
        pointerEvents: 'none',
      }}
    >
      🌸
    </span>
    <span
      aria-hidden
      style={{
        position: 'absolute',
        bottom: 14,
        left: 18,
        fontSize: 24,
        opacity: 0.16,
        animation: 'diary-float 9s ease-in-out 1.2s infinite',
        pointerEvents: 'none',
      }}
    >
      🍂
    </span>
    <div
      style={{
        position: 'relative',
        zIndex: 1,
        flex: 1,
        display: 'flex',
        flexDirection: 'column',
        gap: SPACING.md,
        minHeight: 0,
      }}
    >
      {children}
    </div>
  </div>
);

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
          ? JOURNAL.shadowMd
          : JOURNAL.shadowSm,
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
            background: `radial-gradient(circle at 35% 30%, #E9776F, ${JOURNAL.stampRed})`,
            boxShadow: JOURNAL.shadowSm,
          }}
        />
      )}
    </button>
  );
};

interface TopBarProps {
  character: CharacterId;
  setCharacter: (c: CharacterId) => void;
  t: TFunction;
}

const TopBar: React.FC<TopBarProps> = ({ character, setCharacter, t }) => (
  <div
    style={{
      display: 'flex',
      alignItems: 'center',
      gap: SPACING.md,
      flexShrink: 0,
      paddingTop: 6,
      paddingLeft: SPACING.xs,
    }}
  >
    <TapeTab
      label={t('mind_inspector.common.char_vivian')}
      color={CHAR_TAPE.vivian}
      rot={-1.6}
      active={character === 'vivian'}
      onClick={() => setCharacter('vivian')}
    />
    <TapeTab
      label={t('mind_inspector.common.char_nana')}
      color={CHAR_TAPE.nana}
      rot={1.4}
      active={character === 'nana'}
      onClick={() => setCharacter('nana')}
    />
  </div>
);

// ============================================================
// DiaryToolbar — 标题行工具栏（内容搜索 + 日期日历筛选）
// ============================================================

interface DiaryCalendarProps {
  character: CharacterId;
  dateFilter: string | null;
  entryDates: Set<string>;
  onPick: (date: string | null) => void;
  onClose: () => void;
}

/** 月历弹层：点选日期筛选日记列表 */
const DiaryCalendar: React.FC<DiaryCalendarProps> = ({
  character,
  dateFilter,
  entryDates,
  onPick,
  onClose,
}) => {
  const { t } = useTranslation();
  const today = new Date();
  const initial = dateFilter ? parseDate(dateFilter) : null;
  const [calYear, setCalYear] = useState(initial?.getFullYear() ?? today.getFullYear());
  const [calMonth, setCalMonth] = useState(initial?.getMonth() ?? today.getMonth());

  const firstWeekday = new Date(calYear, calMonth, 1).getDay();
  const daysInMonth = new Date(calYear, calMonth + 1, 0).getDate();
  const pad = (x: number) => String(x).padStart(2, '0');
  const dateStrOf = (day: number) => `${calYear}-${pad(calMonth + 1)}-${pad(day)}`;

  const prevMonth = () => {
    if (calMonth === 0) {
      setCalMonth(11);
      setCalYear(calYear - 1);
    } else {
      setCalMonth(calMonth - 1);
    }
  };
  const nextMonth = () => {
    if (calMonth === 11) {
      setCalMonth(0);
      setCalYear(calYear + 1);
    } else {
      setCalMonth(calMonth + 1);
    }
  };

  const cells: Array<number | null> = [
    ...Array.from({ length: firstWeekday }, () => null),
    ...Array.from({ length: daysInMonth }, (_, i) => i + 1),
  ];

  const navBtn: React.CSSProperties = {
    width: 26,
    height: 26,
    border: `1px dashed ${JOURNAL.inkFaint}`,
    borderRadius: 999,
    background: 'transparent',
    color: JOURNAL.inkSoft,
    cursor: 'pointer',
    fontFamily: HAND_BODY,
    fontSize: 16,
    lineHeight: 1,
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
  };

  return (
    <>
      {/* 透明遮罩：点击弹层外部关闭 */}
      <div onClick={onClose} style={{ position: 'fixed', inset: 0, zIndex: 40 }} />
      <div
        style={{
          position: 'absolute',
          top: 'calc(100% + 10px)',
          right: 0,
          zIndex: 50,
          transform: 'rotate(-1deg)',
        }}
      >
        <div
          style={{
            position: 'relative',
            width: 264,
            background: JOURNAL.card,
            border: `1px dashed ${JOURNAL.border}`,
            borderRadius: 6,
            boxShadow: JOURNAL.shadowMd,
            padding: '20px 16px 14px',
          }}
        >
          <TapeStrip color={CHAR_TAPE[character]} width={92} />

          {/* 月份导航 */}
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              marginBottom: 10,
            }}
          >
            <button
              type="button"
              onClick={prevMonth}
              title={t('mind_inspector.diary.cal_prev_month')}
              style={navBtn}
            >
              ‹
            </button>
            <span
              style={{
                fontFamily: TYPO.fontFamilyCN,
                fontSize: 15.5,
                color: JOURNAL.ink,
                letterSpacing: 1.5,
              }}
            >
              {t('mind_inspector.diary.cal_month', { year: calYear, month: calMonth + 1 })}
            </span>
            <button
              type="button"
              onClick={nextMonth}
              title={t('mind_inspector.diary.cal_next_month')}
              style={navBtn}
            >
              ›
            </button>
          </div>

          {/* 星期表头 */}
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(7, 1fr)', marginBottom: 4 }}>
            {WEEKDAY_KEYS.map((k) => (
              <span
                key={k}
                style={{
                  textAlign: 'center',
                  fontFamily: HAND_BODY,
                  fontSize: 12,
                  color: JOURNAL.inkSoft,
                }}
              >
                {t(`mind_inspector.diary.${k}`)}
              </span>
            ))}
          </div>

          {/* 日期网格 */}
          <div style={{ display: 'grid', gridTemplateColumns: 'repeat(7, 1fr)', rowGap: 2 }}>
            {cells.map((day, i) => {
              if (day === null) return <span key={`blank-${i}`} />;
              const ds = dateStrOf(day);
              const selected = dateFilter === ds;
              const hasEntry = entryDates.has(ds);
              return (
                <button
                  key={ds}
                  type="button"
                  onClick={() => {
                    onPick(selected ? null : ds);
                    onClose();
                  }}
                  style={{
                    position: 'relative',
                    width: 30,
                    height: 30,
                    margin: '0 auto',
                    border: 'none',
                    borderRadius: 999,
                    cursor: 'pointer',
                    background: selected ? JOURNAL.stampRed : 'transparent',
                    color: selected ? '#FFF9EE' : JOURNAL.ink,
                    fontFamily: HAND_BODY,
                    fontSize: 14,
                    lineHeight: 1,
                    display: 'inline-flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                  }}
                >
                  {day}
                  {hasEntry && !selected && (
                    <span
                      aria-hidden
                      style={{
                        position: 'absolute',
                        bottom: 3,
                        left: '50%',
                        transform: 'translateX(-50%)',
                        width: 4,
                        height: 4,
                        borderRadius: 999,
                        background: JOURNAL.stampRed,
                      }}
                    />
                  )}
                </button>
              );
            })}
          </div>

          {/* 清除日期 */}
          <div style={{ textAlign: 'center', marginTop: 10 }}>
            <button
              type="button"
              onClick={() => {
                onPick(null);
                onClose();
              }}
              style={{
                border: 'none',
                background: 'transparent',
                cursor: 'pointer',
                fontFamily: HAND_BODY,
                fontSize: 13,
                color: JOURNAL.inkSoft,
                textDecoration: 'underline dashed',
                textUnderlineOffset: 3,
              }}
            >
              {t('mind_inspector.diary.cal_clear_date')}
            </button>
          </div>
        </div>
      </div>
    </>
  );
};

interface DiaryToolbarProps {
  character: CharacterId;
  dateFilter: string | null;
  onDateFilter: (d: string | null) => void;
  searchQuery: string;
  onSearchQuery: (q: string) => void;
  entryDates: Set<string>;
}

/** 标题行工具栏：内容搜索输入框 + 可展开日历的日期筛选按钮 */
const DiaryToolbar: React.FC<DiaryToolbarProps> = ({
  character,
  dateFilter,
  onDateFilter,
  searchQuery,
  onSearchQuery,
  entryDates,
}) => {
  const { t } = useTranslation();
  const [calOpen, setCalOpen] = useState(false);
  // 使用 uncontrolled input + ref 确保 IME 输入完全由浏览器原生处理
  const inputRef = useRef<HTMLInputElement>(null);
  const composingRef = useRef(false);

  // 外部（如清除按钮）更新 searchQuery 时同步到非受控 input
  useEffect(() => {
    if (inputRef.current && inputRef.current.value !== searchQuery) {
      inputRef.current.value = searchQuery;
    }
  }, [searchQuery]);

  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.sm }}>
      {/* 内容搜索框（纸片风格） */}
      <div style={{ position: 'relative', display: 'flex', alignItems: 'center' }}>
        <span
          aria-hidden
          style={{
            position: 'absolute',
            left: 10,
            fontSize: 14,
            color: JOURNAL.inkSoft,
            pointerEvents: 'none',
          }}
        >
          ✎
        </span>
        <input
          ref={inputRef}
          type="text"
          defaultValue={searchQuery}
          onCompositionStart={() => {
            composingRef.current = true;
          }}
          onCompositionEnd={() => {
            composingRef.current = false;
            if (inputRef.current) onSearchQuery(inputRef.current.value);
          }}
          onInput={() => {
            if (!composingRef.current && inputRef.current) {
              onSearchQuery(inputRef.current.value);
            }
          }}
          placeholder={t('mind_inspector.diary.search_placeholder')}
          style={{
            width: 200,
            padding: '7px 28px 6px 30px',
            background: JOURNAL.card,
            border: `1px dashed ${JOURNAL.border}`,
            borderRadius: 5,
            fontFamily: HAND_BODY,
            fontSize: 14.5,
            color: JOURNAL.ink,
            outline: 'none',
            boxShadow: JOURNAL.shadowSm,
            transform: 'rotate(-0.6deg)',
          }}
        />
        {searchQuery && (
          <button
            type="button"
            onClick={() => {
              onSearchQuery('');
              if (inputRef.current) inputRef.current.value = '';
            }}
            aria-label="clear search"
            style={{
              position: 'absolute',
              right: 8,
              border: 'none',
              background: 'transparent',
              cursor: 'pointer',
              color: JOURNAL.inkSoft,
              fontSize: 15,
              lineHeight: 1,
              padding: 2,
            }}
          >
            ×
          </button>
        )}
      </div>

      {/* 日期筛选按钮 + 日历弹层 */}
      <div style={{ position: 'relative' }}>
        <button
          type="button"
          onClick={() => setCalOpen((v) => !v)}
          style={{
            padding: '7px 16px 6px',
            background: JOURNAL.card,
            border: dateFilter
              ? `1.5px dashed ${JOURNAL.stampRed}`
              : '1px dashed ${JOURNAL.border}',
            borderRadius: 5,
            cursor: 'pointer',
            fontFamily: HAND_BODY,
            fontSize: 14.5,
            color: dateFilter ? JOURNAL.stampRed : JOURNAL.ink,
            boxShadow: JOURNAL.shadowSm,
            transform: 'rotate(0.8deg)',
            display: 'inline-flex',
            alignItems: 'center',
            gap: 7,
            whiteSpace: 'nowrap',
          }}
        >
          <span aria-hidden>📅</span>
          {dateFilter ?? t('mind_inspector.diary.cal_all_dates')}
        </button>
        {calOpen && (
          <DiaryCalendar
            character={character}
            dateFilter={dateFilter}
            entryDates={entryDates}
            onPick={onDateFilter}
            onClose={() => setCalOpen(false)}
          />
        )}
      </div>
    </div>
  );
};

// ============================================================
// DiaryCard — 便签卡片
// ============================================================

interface DiaryCardProps {
  entry: DiaryEntry;
  index: number;
  character: CharacterId;
  selected: boolean;
  onClick: () => void;
}

const DiaryCard: React.FC<DiaryCardProps> = ({ entry, index, character, selected, onClick }) => {
  const { t } = useTranslation();
  const [hovered, setHovered] = useState(false);
  const moodColor = MOOD_COLORS[entry.mood_tag] ?? JOURNAL.inkSoft;
  const moodLabel = t(`mind_inspector.diary.mood_${entry.mood_tag}`, {
    defaultValue: entry.mood_tag,
  });
  const rot = (index % 2 === 0 ? 1 : -1) * (0.45 + (index % 3) * 0.3);
  const tapeColor = CHAR_TAPE[character];
  return (
    <div
      style={{
        position: 'relative',
        transform: `rotate(${rot}deg)`,
        animation: `diary-slip-in ${DURATION.slow}s ${EASE.spring} both`,
        animationDelay: `${Math.min(index * 45, 420)}ms`,
      }}
    >
      <div
        role="button"
        onClick={onClick}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        style={{
          position: 'relative',
          marginTop: 10,
          background: JOURNAL.card,
          border: selected
            ? `1.5px dashed ${JOURNAL.stampRed}`
            : `1px solid ${JOURNAL.inkFaint}`,
          borderRadius: 5,
          padding: '13px 15px 11px',
          cursor: 'pointer',
          boxShadow: selected
            ? '0 6px 16px rgba(201,64,58,0.16), 0 2px 5px rgba(0,0,0,0.3)'
            : hovered
              ? JOURNAL.shadowMd
              : JOURNAL.shadowSm,
          transform: hovered ? 'translateY(-3px)' : 'translateY(0)',
          transition: `transform ${DURATION.normal}s ${EASE.spring}, box-shadow ${DURATION.normal}s ${EASE.ios}, border-color ${DURATION.fast}s ${EASE.swift}`,
        }}
      >
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: SPACING.sm,
            marginBottom: SPACING.xs + 2,
          }}
        >
          <span
            style={{
              width: 30,
              height: 30,
              flexShrink: 0,
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              fontSize: 16,
              background: `${moodColor}14`,
              border: `1.5px dashed ${moodColor}66`,
              borderRadius: 999,
              transform: 'rotate(-6deg)',
            }}
          >
            {MOOD_EMOJI[entry.mood_tag] ?? '📝'}
          </span>
          <span
            style={{
              fontFamily: TYPO.fontFamilyCN,
              fontSize: 16.5,
              color: JOURNAL.ink,
              letterSpacing: 0.5,
              flex: 1,
              minWidth: 0,
              fontVariantNumeric: 'tabular-nums',
            }}
          >
            {formatListDate(entry.date)}
          </span>
        </div>

        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: SPACING.sm,
            flexWrap: 'wrap',
          }}
        >
          <span
            style={{
              fontFamily: HAND_BODY,
              fontSize: 12.5,
              color: moodColor,
              background: `${moodColor}12`,
              border: `1px dashed ${moodColor}55`,
              borderRadius: 999,
              padding: '1.5px 9px',
              whiteSpace: 'nowrap',
            }}
          >
            {moodLabel}
          </span>
          <span style={{ fontFamily: HAND_BODY, fontSize: 12.5, color: JOURNAL.inkSoft }}>
            {t('mind_inspector.diary.word_count_suffix', { n: entry.word_count })}
          </span>
          <span
            style={{
              fontFamily: HAND_BODY,
              fontSize: 12.5,
              color: JOURNAL.inkSoft,
              marginLeft: 'auto',
            }}
          >
            {formatCreated(entry.created_at, t)}
          </span>
        </div>

        <TapeStrip color={tapeColor} />

        {selected && (
          <span
            aria-hidden
            style={{
              position: 'absolute',
              top: 2,
              right: -6,
              width: 24,
              height: 24,
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              borderRadius: 999,
              background: `radial-gradient(circle at 35% 30%, #E9776F, ${JOURNAL.stampRed})`,
              color: '#FFF9EE',
              fontSize: 13,
              fontWeight: 700,
              boxShadow: JOURNAL.shadowSm,
              animation: `diary-stamp-pop 0.34s ${EASE.spring} both`,
            }}
          >
            ✓
          </span>
        )}
      </div>
    </div>
  );
};

const MemoDiaryCard = React.memo(DiaryCard);

// ============================================================
// DiaryPage
// ============================================================

const DiaryPage: React.FC = () => {
  const { t } = useTranslation();
  const nav = useNavigation();
  const [character, setCharacter] = useState<CharacterId>('vivian');
  const [entries, setEntries] = useState<DiaryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [dateFilter, setDateFilter] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');

  // 请求序号：仅最新一次请求的响应允许写入状态
  const requestSeq = useRef(0);

  // === 数据加载 ===
  const loadEntries = useCallback((charId: CharacterId) => {
    const seq = ++requestSeq.current;
    setLoading(true);
    setError(null);

    invoke<DiaryEntry[]>('get_diary_entries', { characterId: charId, dateFilter: null })
      .then((res) => {
        if (seq !== requestSeq.current) return;
        const seen = new Set<string>();
        const deduped: DiaryEntry[] = [];
        for (const e of res ?? []) {
          if (!seen.has(e.id)) {
            seen.add(e.id);
            deduped.push(e);
          }
        }
        const list = deduped.sort((a, b) => b.date.localeCompare(a.date));
        setEntries(list);
        setSelectedId((prev) =>
          prev && list.some((e) => e.id === prev) ? prev : list[0]?.id ?? null,
        );
      })
      .catch((e) => {
        if (seq === requestSeq.current) setError(String(e));
      })
      .finally(() => {
        if (seq === requestSeq.current) setLoading(false);
      });
  }, []);

  // === 切换角色：清空旧列表后重新加载 ===
  useEffect(() => {
    setEntries([]);
    setSelectedId(null);
    loadEntries(character);
  }, [character, loadEntries]);

  // === 日记写入事件刷新 ===
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ character_id?: string }>('diary:written', (event) => {
          if (!event.payload?.character_id || event.payload.character_id === character) {
            loadEntries(character);
          }
        });
        if (cancelled) unlisten();
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [character, loadEntries]);

  // === 响应导航参数：自动切换角色并选中日记 ===
  useEffect(() => {
    if (!nav?.pageParams?.diaryId) return;
    const { diaryId, diaryCharacter } = nav.pageParams;

    if (diaryCharacter && diaryCharacter !== character) {
      setCharacter(diaryCharacter);
    }

    const matched = entries.find((e) => e.id === diaryId);
    if (matched) {
      setSelectedId(diaryId);
      nav.clearPageParams();
    } else if (!loading) {
      nav.clearPageParams();
    }
  }, [nav?.pageParams?.diaryId, nav?.pageParams?.diaryCharacter, character, entries, loading, nav]);

  // === 日期 + 内容关键词筛选 ===
  const filteredEntries = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    return entries.filter((e) => {
      if (dateFilter && e.date !== dateFilter) return false;
      if (q && !e.content.toLowerCase().includes(q)) return false;
      return true;
    });
  }, [entries, dateFilter, searchQuery]);

  // 有日记的日期集合（日历红点标记）
  const entryDates = useMemo(() => new Set(entries.map((e) => e.date)), [entries]);

  // 当前选中项被筛掉时，自动选中第一个匹配项
  useEffect(() => {
    if (filteredEntries.length === 0) return;
    if (!filteredEntries.some((e) => e.id === selectedId)) {
      setSelectedId(filteredEntries[0].id);
    }
  }, [filteredEntries, selectedId]);

  // === 标题行工具栏（注入共享 header 右侧） ===
  const setHeaderExtra = nav?.setHeaderExtra;
  const toolbar = useMemo(
    () => (
      <DiaryToolbar
        character={character}
        dateFilter={dateFilter}
        onDateFilter={setDateFilter}
        searchQuery={searchQuery}
        onSearchQuery={setSearchQuery}
        entryDates={entryDates}
      />
    ),
    [character, dateFilter, searchQuery, entryDates],
  );
  useEffect(() => {
    if (!setHeaderExtra) return;
    setHeaderExtra(toolbar);
    return () => setHeaderExtra(null);
  }, [toolbar, setHeaderExtra]);

  // === 选中日记 ===
  const selectedEntry = useMemo(() => {
    if (!selectedId) return null;
    return entries.find((e) => e.id === selectedId) ?? null;
  }, [selectedId, entries]);

  // === 渲染：加载中 ===
  if (loading && entries.length === 0) {
    return (
      <PageShell>
        <TopBar character={character} setCharacter={setCharacter} t={t} />
        <Center>
          <PaperNote tapeColor={JOURNAL.tape[3]}>
            <span
              style={{
                fontSize: 30,
                lineHeight: 1,
                animation: `diary-pencil 1.6s ${EASE.swift} infinite`,
              }}
            >
              ✏️
            </span>
            <span
              style={{
                width: 18,
                height: 18,
                border: `2px solid ${JOURNAL.border}`,
                borderTopColor: JOURNAL.stampRed,
                borderRadius: 999,
                animation: 'diary-spin 0.8s linear infinite',
              }}
            />
            <NoteText>{t('mind_inspector.diary.loading')}</NoteText>
          </PaperNote>
        </Center>
      </PageShell>
    );
  }

  // === 渲染：加载失败 ===
  if (error && entries.length === 0) {
    return (
      <PageShell>
        <TopBar character={character} setCharacter={setCharacter} t={t} />
        <Center>
          <PaperNote tapeColor={JOURNAL.tape[1]}>
            <span
              style={{
                width: 40,
                height: 40,
                display: 'inline-flex',
                alignItems: 'center',
                justifyContent: 'center',
                border: `2px dashed ${JOURNAL.stampRed}`,
                borderRadius: 999,
                color: JOURNAL.stampRed,
                fontSize: 20,
                fontWeight: 700,
                transform: 'rotate(-8deg)',
              }}
            >
              !
            </span>
            <NoteText>{t('mind_inspector.common.load_failed', { error })}</NoteText>
          </PaperNote>
        </Center>
      </PageShell>
    );
  }

  // === 渲染：空数据 ===
  if (entries.length === 0) {
    return (
      <PageShell>
        <TopBar character={character} setCharacter={setCharacter} t={t} />
        <Center>
          <PaperNote tapeColor={JOURNAL.tape[2]}>
            <span style={{ fontSize: 34, lineHeight: 1, transform: 'rotate(-4deg)' }}>📔</span>
            <NoteText>{t(`mind_inspector.diary.no_diary_${character}`)}</NoteText>
          </PaperNote>
        </Center>
      </PageShell>
    );
  }

  return (
    <PageShell>
      <TopBar character={character} setCharacter={setCharacter} t={t} />

      <div style={{ flex: 1, display: 'flex', gap: SPACING.lg, minHeight: 0 }}>
        {/* ===== 左侧：便签列表（40%） ===== */}
        <div
          style={{
            width: '40%',
            minWidth: 280,
            display: 'flex',
            flexDirection: 'column',
            gap: SPACING.sm + 2,
            overflowY: 'auto',
            paddingRight: SPACING.xs,
          }}
        >
          <JournalSectionTitle
            seal="目"
            title={t('mind_inspector.diary.list_title', {
              shown: filteredEntries.length,
              total: entries.length,
            })}
            style={{ marginTop: 2, marginBottom: 4, flexShrink: 0 }}
          />
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              gap: SPACING.sm + 2,
              paddingBottom: SPACING.md,
            }}
          >
            {filteredEntries.length === 0 ? (
              <PaperNote tapeColor={JOURNAL.tape[3]} style={{ padding: '24px 30px' }}>
                <span style={{ fontSize: 26, lineHeight: 1, transform: 'rotate(-4deg)' }}>🔍</span>
                <NoteText>{t('mind_inspector.diary.no_match')}</NoteText>
              </PaperNote>
            ) : (
              filteredEntries.map((e, i) => (
                <MemoDiaryCard
                  key={e.id}
                  entry={e}
                  index={i}
                  character={character}
                  selected={e.id === selectedId}
                  onClick={() => setSelectedId(e.id)}
                />
              ))
            )}
          </div>
        </div>

        {/* ===== 右侧：信纸详情（60%） ===== */}
        <div
          style={{
            width: '60%',
            minWidth: 320,
            display: 'flex',
            flexDirection: 'column',
            overflowY: 'auto',
            paddingRight: SPACING.xs,
            paddingBottom: SPACING.md,
          }}
        >
          {selectedEntry ? (
            <DiaryDetail
              key={selectedEntry.id}
              entry={selectedEntry}
              character={character}
              query={searchQuery}
            />
          ) : (
            <Center>
              <PaperNote tapeColor={JOURNAL.tape[1]}>
                <span style={{ fontSize: 30, lineHeight: 1 }}>👈</span>
                <NoteText>{t('mind_inspector.diary.select_hint')}</NoteText>
              </PaperNote>
            </Center>
          )}
        </div>
      </div>
    </PageShell>
  );
};

// ============================================================
// DiaryDetail — 信纸详情
// ============================================================

const DiaryDetail: React.FC<{ entry: DiaryEntry; character: CharacterId; query: string }> = ({
  entry,
  character,
  query,
}) => {
  const { t } = useTranslation();
  const moodColor = MOOD_COLORS[entry.mood_tag] ?? JOURNAL.inkSoft;
  const moodLabel = t(`mind_inspector.diary.mood_${entry.mood_tag}`, {
    defaultValue: entry.mood_tag,
  });
  const accent = CHARACTER_ACCENT[character];

  const paragraphs = entry.content
    .split(/\n\s*\n/)
    .filter((para) => para.trim().length > 0);
  const paragraphMode = detectParagraphMode(entry.content);
  const paragraphIndent =
    paragraphMode === 'indent-jp' ? '1em' : paragraphMode === 'indent-cn' ? '2em' : undefined;
  const blankLineBetween = paragraphMode === 'blank-line';

  return (
    <div
      style={{
        position: 'relative',
        animation: `diary-page-in ${DURATION.slow}s ${EASE.ios} both`,
      }}
    >
      <div
        style={{
          position: 'relative',
          background: JOURNAL.card,
          border: `1px solid ${JOURNAL.border}`,
          borderRadius: 8,
          boxShadow: JOURNAL.shadowLg,
          padding: '24px 26px 28px 62px',
          overflow: 'hidden',
        }}
      >
        {/* 粉色页边线 */}
        <div
          aria-hidden
          style={{
            position: 'absolute',
            left: 44,
            top: 0,
            bottom: 0,
            width: 1.5,
            background: JOURNAL.marginPink,
          }}
        />
        {/* 装订孔 */}
        {['18%', '50%', '82%'].map((top) => (
          <div
            key={top}
            aria-hidden
            style={{
              position: 'absolute',
              left: 14,
              top,
              width: 11,
              height: 11,
              borderRadius: 999,
              background: JOURNAL.paper,
              border: `1px solid ${JOURNAL.border}`,
              boxShadow: JOURNAL.shadowSm,
              transform: 'translateY(-50%)',
            }}
          />
        ))}
        <TapeStrip
          color={JOURNAL.tape[2]}
          width={92}
          style={{ top: -9, left: 96, transform: 'rotate(-4deg)' }}
        />

        {/* 日期标题 + 心情邮戳 */}
        <div
          style={{
            display: 'flex',
            alignItems: 'flex-start',
            justifyContent: 'space-between',
            gap: SPACING.md,
          }}
        >
          <div style={{ minWidth: 0 }}>
            <div
              style={{
                fontFamily: TYPO.fontFamilyCN,
                fontSize: 26,
                color: JOURNAL.ink,
                letterSpacing: 1.5,
                lineHeight: 1.3,
              }}
            >
              {formatDetailTitle(entry.date, t)}
            </div>
            <div
              style={{
                fontFamily: HAND_BODY,
                fontSize: 13,
                color: JOURNAL.inkSoft,
                marginTop: 6,
                letterSpacing: 1,
              }}
            >
              ✎ {t('mind_inspector.diary.created_at', { time: formatCreated(entry.created_at, t) })}
            </div>
          </div>
          <div
            style={{
              flexShrink: 0,
              width: 78,
              height: 78,
              display: 'flex',
              flexDirection: 'column',
              alignItems: 'center',
              justifyContent: 'center',
              gap: 2,
              border: `1.5px dashed ${moodColor}`,
              borderRadius: 999,
              color: moodColor,
              animation: `diary-stamp-pop 0.4s ${EASE.spring} both`,
            }}
          >
            <span style={{ fontSize: 24, lineHeight: 1 }}>
              {MOOD_EMOJI[entry.mood_tag] ?? '📝'}
            </span>
            <span style={{ fontFamily: HAND_BODY, fontSize: 11.5, letterSpacing: 1 }}>
              {moodLabel}
            </span>
          </div>
        </div>

        {/* 今日要事 */}
        <JournalSectionTitle
          seal="事"
          title={t('mind_inspector.diary.detail_key_events')}
          style={{ margin: '24px 0 12px' }}
        />
        {entry.key_events.length === 0 ? (
          <div style={{ fontFamily: HAND_BODY, fontSize: 15, color: JOURNAL.inkSoft }}>
            {t('mind_inspector.diary.detail_no_key_events')}
          </div>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 9 }}>
            {entry.key_events.map((ev, i) => (
              <div key={`event-${i}`} style={{ display: 'flex', alignItems: 'flex-start', gap: 9 }}>
                <span
                  aria-hidden
                  style={{
                    marginTop: 3,
                    width: 17,
                    height: 17,
                    flexShrink: 0,
                    display: 'inline-flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    border: `1.5px dashed ${JOURNAL.stampRed}`,
                    borderRadius: 999,
                    color: JOURNAL.stampRed,
                    fontSize: 10.5,
                    fontWeight: 700,
                  }}
                >
                  ✓
                </span>
                <span
                  style={{
                    fontFamily: HAND_BODY,
                    fontSize: 15.5,
                    color: JOURNAL.ink,
                    lineHeight: 1.6,
                  }}
                >
                  {ev}
                </span>
              </div>
            ))}
          </div>
        )}

        {/* 日记正文（横线信纸） */}
        <JournalSectionTitle
          seal="记"
          title={t('mind_inspector.diary.detail_content')}
          style={{ margin: '24px 0 12px' }}
        />
        {entry.content.trim().length === 0 ? (
          <div style={{ fontFamily: HAND_BODY, fontSize: 15, color: JOURNAL.inkSoft }}>
            {t('mind_inspector.diary.detail_no_content')}
          </div>
        ) : (
          <div
            style={{
              background: `repeating-linear-gradient(180deg, transparent 0px, transparent 29px, ${JOURNAL.lineBlue} 29px, ${JOURNAL.lineBlue} 30px), ${JOURNAL.card}`,
              borderRadius: 4,
              padding: '8px 16px 12px',
              fontFamily: HAND_BODY,
              fontSize: 16.5,
              lineHeight: '30px',
              color: JOURNAL.ink,
              whiteSpace: 'pre-wrap',
              wordBreak: 'break-word',
            }}
          >
            {paragraphs.map((para, i) => (
              <p
                key={i}
                style={{
                  margin: 0,
                  marginBottom: blankLineBetween && i < paragraphs.length - 1 ? 30 : 0,
                  textIndent: paragraphIndent,
                }}
              >
                {highlightText(para.trim(), query, accent)}
              </p>
            ))}
          </div>
        )}
      </div>
    </div>
  );
};

export default DiaryPage;
