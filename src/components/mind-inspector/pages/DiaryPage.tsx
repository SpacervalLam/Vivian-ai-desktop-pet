/**
 * Diary 页 — 日记阅读视图
 *
 * 数据源：并行读取 Vivian/Nana 的 get_diary_entries，合并为作者可辨的时间线。
 * 刷新：监听 'diary:written' 事件
 *
 * 布局（阅读型）：左索引栏按月分组列出日期，右阅读面展示正文。
 * 日期做页级标题、心情做成一个安静的读数，正文用衬线 + 1.9 行高。
 * 排版刻度与共用控件走 RecordTheme.css，本文件只管这页的形状。
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import {
  CalendarDays,
  Check,
  ChevronLeft,
  ChevronRight,
  Search,
  X,
} from 'lucide-react';
import { useNavigation } from '../NavigationContext';
import './RecordTheme.css';
import { ComicChapterHeading, ComicChapterInset } from './ComicChapter';

// ============================================================
// 类型定义
// ============================================================

interface DiaryEntry {
  character: CharacterId;
  sourceId: string;
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

const WEEKDAY_KEYS = [
  'weekday_sun',
  'weekday_mon',
  'weekday_tue',
  'weekday_wed',
  'weekday_thu',
  'weekday_fri',
  'weekday_sat',
];

const MOOD_COLORS: Record<string, string> = {
  happy: '#C08A3E',
  good: '#788C5D',
  neutral: '#8A8780',
  sad: '#5C7C93',
  angry: '#B4553F',
  bored: '#8A8780',
  tired: '#8A7A94',
};

const CHARACTERS: CharacterId[] = ['vivian', 'nana'];

// ============================================================
// 时间工具
// ============================================================

const parseDate = (s: string): Date | null => {
  if (!s) return null;
  const d = new Date(`${s}T00:00:00`);
  return Number.isNaN(d.getTime()) ? null : d;
};

/** 索引栏用的短日期：2026-10-03 → 10-03 */
const formatLinkDate = (s: string): string => {
  const d = parseDate(s);
  if (!d) return s;
  return `${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
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
// 正文排版：按正文字种选择段间距策略
// ============================================================

/** 含假名判定为日文（首行缩进一字），仅含汉字判定为中文（缩进两字），其余为西文（段间空行、不缩进） */
function detectParagraphMode(text: string): 'indent-jp' | 'indent-cn' | 'blank-line' {
  if (/[\u3040-\u309F\u30A0-\u30FF]/.test(text)) return 'indent-jp';
  if (/[\u4E00-\u9FFF\u3400-\u4DBF]/.test(text)) return 'indent-cn';
  return 'blank-line';
}

/** 用陶土色荧光笔高亮命中的关键词（大小写不敏感） */
const highlightText = (text: string, query: string): React.ReactNode => {
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
      <span key={`hl-${key++}`} className="rec-mark">
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
// 共用片段
// ============================================================

// ============================================================
// 角色切换
// ============================================================


// ============================================================
// 月历弹层
// ============================================================

interface DiaryCalendarProps {
  dateFilter: string | null;
  entryDates: Set<string>;
  onPick: (date: string | null) => void;
  onClose: () => void;
}

/** 月历弹层：点选日期筛选日记。带日记的日期在下方标一个点。 */
const DiaryCalendar: React.FC<DiaryCalendarProps> = ({ dateFilter, entryDates, onPick, onClose }) => {
  const { t } = useTranslation();
  const today = new Date();
  const initial = dateFilter ? parseDate(dateFilter) : null;
  const [calYear, setCalYear] = useState(initial?.getFullYear() ?? today.getFullYear());
  const [calMonth, setCalMonth] = useState(initial?.getMonth() ?? today.getMonth());

  const firstWeekday = new Date(calYear, calMonth, 1).getDay();
  const daysInMonth = new Date(calYear, calMonth + 1, 0).getDate();
  const pad = (x: number) => String(x).padStart(2, '0');
  const dateStrOf = (day: number) => `${calYear}-${pad(calMonth + 1)}-${pad(day)}`;

  const shiftMonth = (delta: number) => {
    const next = new Date(calYear, calMonth + delta, 1);
    setCalYear(next.getFullYear());
    setCalMonth(next.getMonth());
  };

  const cells: Array<number | null> = [
    ...Array.from({ length: firstWeekday }, () => null),
    ...Array.from({ length: daysInMonth }, (_, i) => i + 1),
  ];

  return (
    <>
      <div onClick={onClose} style={{ position: 'fixed', inset: 0, zIndex: 40 }} />
      <div className="rec-pop" style={{ top: 'calc(100% + 8px)', right: 0, width: 252, padding: '14px 14px 10px' }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 10 }}>
          <button
            type="button"
            onClick={() => shiftMonth(-1)}
            title={t('mind_inspector.diary.cal_prev_month')}
            className="rec-icon-btn"
          >
            <ChevronLeft size={15} />
          </button>
          <span style={{ color: 'var(--rec-ink)', fontSize: 'var(--rec-fs-body)', fontWeight: 500 }}>
            {t('mind_inspector.diary.cal_month', { year: calYear, month: calMonth + 1 })}
          </span>
          <button
            type="button"
            onClick={() => shiftMonth(1)}
            title={t('mind_inspector.diary.cal_next_month')}
            className="rec-icon-btn"
          >
            <ChevronRight size={15} />
          </button>
        </div>

        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(7, 1fr)', marginBottom: 4 }}>
          {WEEKDAY_KEYS.map((k) => (
            <span
              key={k}
              style={{ textAlign: 'center', color: 'var(--rec-faint)', fontSize: 'var(--rec-fs-micro)' }}
            >
              {t(`mind_inspector.diary.${k}`)}
            </span>
          ))}
        </div>

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
                className="rec-icon-btn"
                style={{
                  position: 'relative',
                  width: 30,
                  height: 30,
                  margin: '0 auto',
                  borderRadius: 999,
                  background: selected ? 'var(--rec-accent)' : 'transparent',
                  color: selected ? '#faf9f5' : 'var(--rec-ink)',
                  fontSize: 'var(--rec-fs-body)',
                  fontVariantNumeric: 'tabular-nums',
                }}
              >
                {day}
                {hasEntry && !selected && (
                  <span
                    aria-hidden
                    style={{
                      position: 'absolute',
                      bottom: 4,
                      left: '50%',
                      transform: 'translateX(-50%)',
                      width: 3,
                      height: 3,
                      borderRadius: 999,
                      background: 'var(--rec-accent)',
                    }}
                  />
                )}
              </button>
            );
          })}
        </div>

        <div style={{ textAlign: 'center', marginTop: 8 }}>
          <button
            type="button"
            onClick={() => {
              onPick(null);
              onClose();
            }}
            className="rec-btn is-ghost"
          >
            {t('mind_inspector.diary.cal_clear_date')}
          </button>
        </div>
      </div>
    </>
  );
};

// ============================================================
// 顶部工具条
// ============================================================

const DiaryToolbar: React.FC<{
  dateFilter: string | null;
  onDateFilter: (d: string | null) => void;
  searchQuery: string;
  onSearchQuery: (q: string) => void;
  entryDates: Set<string>;
  shownCount: number;
  totalCount: number;
}> = ({ dateFilter, onDateFilter, searchQuery, onSearchQuery, entryDates, shownCount, totalCount }) => {
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
    <>
      <div className="rec-search" style={{ width: 216 }}>
        <Search size={14} />
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
        />
        {searchQuery && (
          <button
            type="button"
            onClick={() => {
              onSearchQuery('');
              if (inputRef.current) inputRef.current.value = '';
            }}
            aria-label={t('comic.clearSearch')}
            className="rec-icon-btn"
            style={{ width: 20, height: 20 }}
          >
            <X size={13} />
          </button>
        )}
      </div>

      <div className="record-diary-cal-wrap">
        <button
          type="button"
          onClick={() => setCalOpen((v) => !v)}
          className={`rec-btn${dateFilter ? ' is-primary' : ''}`}
        >
          <CalendarDays size={14} />
          {dateFilter ?? t('mind_inspector.diary.cal_all_dates')}
        </button>
        {calOpen && (
          <DiaryCalendar
            dateFilter={dateFilter}
            entryDates={entryDates}
            onPick={onDateFilter}
            onClose={() => setCalOpen(false)}
          />
        )}
      </div>

      <span className="record-diary-bar-spacer" />
      <span className="record-plan-count">
        {t('mind_inspector.diary.list_title', { shown: shownCount, total: totalCount })}
      </span>
    </>
  );
};

// ============================================================
// 索引栏：按月分组
// ============================================================

interface MonthGroup {
  key: string;
  label: string;
  entries: DiaryEntry[];
}

/** 把倒序的日记列表按「年-月」切成连续分组 */
function groupByMonth(entries: DiaryEntry[], t: TFunction): MonthGroup[] {
  const groups: MonthGroup[] = [];
  for (const entry of entries) {
    const d = parseDate(entry.date);
    const key = d ? `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}` : entry.date;
    const last = groups[groups.length - 1];
    if (last && last.key === key) {
      last.entries.push(entry);
    } else {
      groups.push({
        key,
        label: d
          ? t('mind_inspector.diary.cal_month', { year: d.getFullYear(), month: d.getMonth() + 1 })
          : key,
        entries: [entry],
      });
    }
  }
  return groups;
}

const DiaryIndex: React.FC<{
  entries: DiaryEntry[];
  selectedId: string | null;
  onSelect: (id: string) => void;
  hasFilter: boolean;
  t: TFunction;
}> = ({ entries, selectedId, onSelect, hasFilter, t }) => {
  const groups = useMemo(() => groupByMonth(entries, t), [entries, t]);

  return (
    <div className="record-diary-index">
      <div className="record-diary-index-scroll">
        {entries.length === 0 && hasFilter && (
          <span className="record-diary-month-empty" role="status">{t('mind_inspector.diary.no_match')}</span>
        )}
        {groups.map((group) => (
          <div key={group.key} className="record-diary-month">
            <div className="record-diary-month-label">{group.label}</div>
            {group.entries.map((entry) => {
              const d = parseDate(entry.date);
              const moodColor = MOOD_COLORS[entry.mood_tag] ?? 'var(--rec-faint)';
              return (
                <button
                  key={entry.id}
                  type="button"
                  onClick={() => onSelect(entry.id)}
                  className={`record-diary-link${entry.id === selectedId ? ' is-active' : ''}`}
                >
                  <span className="record-diary-link-date">{formatLinkDate(entry.date)}</span>
                  <span className={`record-diary-author is-${entry.character}`}>{t(`mind_inspector.common.char_${entry.character}`)}</span>
                  <span className="record-diary-link-weekday">
                    {d ? t(`mind_inspector.diary.${WEEKDAY_KEYS[d.getDay()]}`) : ''}
                    {' · '}
                    {t('mind_inspector.diary.word_count_suffix', { n: entry.word_count })}
                  </span>
                  <span
                    className="record-diary-link-dot"
                    style={{ background: moodColor }}
                    aria-hidden
                  />
                </button>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
};

// ============================================================
// 阅读面
// ============================================================

const DiaryReader: React.FC<{ entry: DiaryEntry; query: string; character: CharacterId; embedded?: boolean }> = ({ entry, query, character, embedded }) => {
  const { t } = useTranslation();
  const d = parseDate(entry.date);
  const moodColor = MOOD_COLORS[entry.mood_tag] ?? 'var(--rec-muted)';
  const moodLabel = t(`mind_inspector.diary.mood_${entry.mood_tag}`, { defaultValue: entry.mood_tag });

  const paragraphs = entry.content
    .split(/\n\s*\n/)
    .filter((para) => para.trim().length > 0);
  const paragraphMode = detectParagraphMode(entry.content);
  const proseClass =
    paragraphMode === 'blank-line' ? 'is-spaced' : paragraphMode === 'indent-jp' ? 'is-indented' : 'is-indented';

  return (
    <div className="record-diary-reader">
      <article key={entry.id} className="record-diary-article">
        <header className="record-diary-head">
          <div style={{ minWidth: 0 }}>
            <div className="comic-record-stamp">{t('comic.historicalRecord')}</div>
            <h2 className="record-diary-date">
              {d ? (
                <span className="record-diary-date-ymd">
                  <span className="record-diary-date-year">{d.getFullYear()}</span>
                  <span className="record-diary-date-md">
                    {String(d.getMonth() + 1).padStart(2, '0')}.{String(d.getDate()).padStart(2, '0')}
                  </span>
                </span>
              ) : (
                entry.date
              )}
            </h2>
            {/* 互动次数原先挂在「今日要事」标题的右端。标题拿掉后它没了落脚点，
                而它本来就和字数、写作时间一样是「这一篇」的统计，所以并进这行。 */}
            <div className="record-diary-date-weekday">
              {d ? t(`mind_inspector.diary.${WEEKDAY_KEYS[d.getDay()]}`) : ''}
              {' · '}
              {t('mind_inspector.diary.created_at', { time: formatCreated(entry.created_at, t) })}
              {' · '}
              {t('mind_inspector.diary.word_count_suffix', { n: entry.word_count })}
              {' · '}
              {t('mind_inspector.diary.metric_interaction_label')}
              {' '}
              {t('mind_inspector.diary.metric_interaction_value', { n: entry.interaction_count })}
            </div>
          </div>
          <div className="record-diary-mood" style={{ color: moodColor }}>
            <span className="rec-dot" style={{ color: moodColor }} />
            <span className="record-diary-mood-label">{moodLabel}</span>
          </div>
        </header>

        <div className="comic-diary-montage">
          {!embedded && <ComicChapterInset image={character === 'nana' ? 'nana-diary-writing-v31' : 'vivian-thought-v01'}
            caption={character === 'nana' ? '慢慢写下，今天的事。' : '这一天，值得记下来。'} position={character === 'nana' ? '30% 50%' : '50% 35%'} />}
        <section className="record-diary-section">
          {entry.key_events.length === 0 ? (
            <p className="record-diary-muted" style={{ marginTop: 'var(--rec-gap-3)' }}>
              {t('mind_inspector.diary.detail_no_key_events')}
            </p>
          ) : (
            <div className="record-diary-events">
              {entry.key_events.map((ev, i) => (
                <div key={`event-${i}`} className="record-diary-event">
                  <span className="record-diary-event-mark">
                    <Check size={13} strokeWidth={2.2} />
                  </span>
                  <span className="record-diary-event-text">{highlightText(ev, query)}</span>
                </div>
              ))}
            </div>
          )}
        </section>
        </div>

        {/* 两个区块不再各有标题，靠一条手绘接缝把「要事清单」和「正文」分开。
            两边都有内容时才画，免得空段落之间浮一条莫名其妙的线。 */}
        {entry.key_events.length > 0 && entry.content.trim().length > 0 && (
          <div className="record-diary-seam" aria-hidden="true" />
        )}

        <section className="record-diary-section">
          {entry.content.trim().length === 0 ? (
            <p className="record-diary-muted" style={{ marginTop: 'var(--rec-gap-3)' }}>
              {t('mind_inspector.diary.detail_no_content')}
            </p>
          ) : (
            <div className={`record-diary-prose ${proseClass}`}>
              {paragraphs.map((para, i) => (
                <p
                  key={i}
                  className={paragraphMode === 'indent-jp' ? 'is-indented-jp' : undefined}
                >
                  {highlightText(para.trim(), query)}
                </p>
              ))}
            </div>
          )}
        </section>
      </article>
    </div>
  );
};

// ============================================================
// DiaryPage
// ============================================================

const DiaryPage: React.FC<{ embedded?: boolean; expanded?: boolean }> = ({ embedded = false, expanded = false }) => {
  const [emptyOwner, setEmptyOwner] = useState<CharacterId>(() => Math.random() < .5 ? 'vivian' : 'nana');
  useEffect(() => {
    if (expanded) setEmptyOwner(Math.random() < .5 ? 'vivian' : 'nana');
  }, [expanded]);
  const { t } = useTranslation();
  const nav = useNavigation();
  const [entries, setEntries] = useState<DiaryEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [dateFilter, setDateFilter] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');

  // 请求序号：仅最新一次请求的响应允许写入状态
  const requestSeq = useRef(0);

  // === 数据加载 ===
  const loadEntries = useCallback(() => {
    const seq = ++requestSeq.current;
    setLoading(true);
    setError(null);

    Promise.all(CHARACTERS.map(async character => {
      const res = await invoke<DiaryEntry[]>('get_diary_entries', { characterId: character, dateFilter: null });
      return (res ?? []).map(entry => ({ ...entry, character, sourceId: entry.id, id: `${character}:${entry.id}` }));
    }))
      .then((groups) => {
        if (seq !== requestSeq.current) return;
        const seen = new Set<string>();
        const deduped: DiaryEntry[] = [];
        for (const e of groups.flat()) {
          if (!seen.has(e.id)) {
            seen.add(e.id);
            deduped.push(e);
          }
        }
        const list = deduped.sort((a, b) => b.date.localeCompare(a.date) || b.created_at - a.created_at || a.id.localeCompare(b.id));
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

  // === 首次加载两位作者 ===
  useEffect(() => {
    setEntries([]);
    setSelectedId(null);
    loadEntries();
  }, [loadEntries]);

  // === 日记写入事件刷新 ===
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      try {
        unlisten = await listen<{ character_id?: string }>('diary:written', (event) => {
          if (!event.payload?.character_id || CHARACTERS.includes(event.payload.character_id as CharacterId)) {
            loadEntries();
          }
        });
        if (cancelled) unlisten();
      } catch {
        /* ignore */
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [loadEntries]);

  // === 响应导航参数：按原始 ID 和作者定位日记 ===
  useEffect(() => {
    if (!nav?.pageParams?.diaryId) return;
    const { diaryId, diaryCharacter } = nav.pageParams;

    const matched = entries.find((e) => (e.sourceId === diaryId || e.id === diaryId) && (!diaryCharacter || e.character === diaryCharacter));
    if (matched) {
      setDateFilter(null);
      setSearchQuery('');
      setSelectedId(matched.id);
      nav.clearPageParams();
    } else if (!loading) {
      nav.clearPageParams();
    }
  }, [nav?.pageParams?.diaryId, nav?.pageParams?.diaryCharacter, entries, loading, nav]);

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
    if (filteredEntries.length === 0) { setSelectedId(null); return; }
    if (!filteredEntries.some((e) => e.id === selectedId)) {
      setSelectedId(filteredEntries[0].id);
    }
  }, [filteredEntries, selectedId]);

  const selectedEntry = useMemo(() => {
    if (!selectedId) return null;
    return filteredEntries.find((e) => e.id === selectedId) ?? null;
  }, [selectedId, filteredEntries]);
  const backdropOwner = selectedEntry?.character ?? (!loading && entries.length === 0 && expanded ? emptyOwner : undefined);

  // 无日记时也保留工具条、日期索引和阅读面。
  return (
    <div className="record-diary comic-chapter comic-diary" data-owner={backdropOwner}>
      {!embedded && <ComicChapterHeading number="02" title="把今天留下来" caption="翻到那一天，就又想起了你。" image="train-tickets-v20">
      <div className="record-diary-bar">
        <DiaryToolbar
          dateFilter={dateFilter}
          onDateFilter={setDateFilter}
          searchQuery={searchQuery}
          onSearchQuery={setSearchQuery}
          entryDates={entryDates}
          shownCount={filteredEntries.length}
          totalCount={entries.length}
        />
      </div>
      </ComicChapterHeading>}
      {embedded && <div className="record-diary-bar">
        <DiaryToolbar dateFilter={dateFilter} onDateFilter={setDateFilter} searchQuery={searchQuery} onSearchQuery={setSearchQuery}
          entryDates={entryDates} shownCount={filteredEntries.length} totalCount={entries.length} />
      </div>}

      <div className="record-diary-body">
        {/* 底纹跟随正文作者；列表和正文的名字标签明确区分归属。 */}
        {backdropOwner && <img
          className="record-diary-backdrop"
          src={`${import.meta.env.BASE_URL}mind-comic/${backdropOwner === 'nana' ? 'nana-train-diary-v32' : 'vivian-message-read-v32'}.webp`}
          alt=""
          aria-hidden="true"
        />}
        <DiaryIndex
          entries={filteredEntries}
          selectedId={selectedId}
          onSelect={setSelectedId}
          hasFilter={!!dateFilter || !!searchQuery.trim()}
          t={t}
        />
        {selectedEntry ? (
          <DiaryReader entry={selectedEntry} query={searchQuery} character={selectedEntry.character} embedded={embedded} />
        ) : (
          <div className="record-diary-reader">
            {loading && <span role="status">{t('mind_inspector.diary.loading')}</span>}
            {!loading && error && <span role="alert">{t('mind_inspector.common.load_failed', { error })}</span>}
          </div>
        )}
      </div>
    </div>
  );
};

export default DiaryPage;
