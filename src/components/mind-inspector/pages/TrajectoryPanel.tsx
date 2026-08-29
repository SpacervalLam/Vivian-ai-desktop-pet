// 轨迹面板：轮次分组的事件台账 + 时间线缩略图（缩放/平移/采样）+ 虚拟滚动 + 搜索索引。
// 范式移植自 deepseek-harness 的 TrajectoryView（timeline / virtual-rows / search-index），
// 数据源为本页会话消息流（coding:* 事件实时更新），不依赖其 snapshot store。

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useVirtualizer } from '@tanstack/react-virtual';
import { GitFork, Search, X } from 'lucide-react';

// ============ 类型 ============

type TrajectoryRole = 'user' | 'assistant' | 'tool_use' | 'tool_result' | 'error';
type TrajectoryKind = 'user' | 'assistant' | 'tool' | 'error';

/** 结构兼容 CodingMessage 的最小消息形状（结构化类型，无需导入页面类型）。 */
interface TrajectoryMessage {
  role: TrajectoryRole;
  content: string;
  tool_name?: string | null;
  tool_arguments?: unknown;
  tool_success?: boolean | null;
  tool_duration_ms?: number | null;
  timestamp: number;
}

interface TrajectoryRecord {
  index: number;
  kind: TrajectoryKind;
  message: TrajectoryMessage;
  turn: number;
}

const KIND_LABEL: Record<TrajectoryKind, string> = {
  user: 'USER',
  assistant: 'ASSISTANT',
  tool: 'TOOL',
  error: 'ERROR',
};

/** 虚拟滚动启用阈值（行数）。 */
const VIRTUALIZATION_THRESHOLD = 100;
/** 单行高度（px，行高 26 + 1 间隙）。 */
const ROW_HEIGHT = 27;
/** 时间线 duration 模式下相邻事件的最小可见宽度（ms）。 */
const TIMELINE_MIN_SPAN_MS = 150;
/** 拖拽判定的最小位移（px）。 */
const MINIMUM_DRAG_PX = 3;

// ============ 摘要提取 ============

/** 从工具参数 JSON 提取单行展示摘要（路径/命令/模式等关键目标）。 */
function toolArgsSummary(args: unknown): string {
  if (!args || typeof args !== 'object') return '';
  const record = args as Record<string, unknown>;
  const parts: string[] = [];
  for (const key of ['path', 'file_path', 'command', 'pattern', 'query', 'dir', 'url']) {
    const v = record[key];
    if (typeof v === 'string' && v) {
      parts.push(key === 'command' ? v.replace(/\s+/g, ' ').slice(0, 80) : v);
      if (parts.length >= 2) break;
    }
  }
  return parts.join(' · ');
}

/** 工具结果正文 → 单行摘要（去 JSON 包裹，截断）。 */
function resultSummary(content: string): string {
  const text = content.trim();
  if (!text) return '';
  try {
    const parsed = JSON.parse(text) as Record<string, unknown>;
    for (const key of ['content', 'output', 'result', 'stdout', 'message', 'error']) {
      const v = parsed[key];
      if (typeof v === 'string' && v.trim()) {
        return v.trim().replace(/\s+/g, ' ').slice(0, 90);
      }
    }
  } catch { /* 非 JSON 原文展示 */ }
  return text.replace(/\s+/g, ' ').slice(0, 90);
}

function formatDuration(ms: number): string {
  const s = ms / 1000;
  if (s < 60) return `${Math.round(s * 10) / 10}s`;
  const whole = Math.round(s);
  return `${Math.floor(whole / 60)}m${whole % 60}s`;
}

function formatClock(ms: number): string {
  const d = new Date(ms);
  const two = (v: number) => String(v).padStart(2, '0');
  return `${two(d.getHours())}:${two(d.getMinutes())}:${two(d.getSeconds())}`;
}

/** messages → 轮次分组的轨迹记录流（每条 user 消息开启一个新轮次）。 */
function deriveTrajectoryRecords(messages: readonly TrajectoryMessage[]): TrajectoryRecord[] {
  const records: TrajectoryRecord[] = [];
  let turn = 0;
  let seenUser = false;
  messages.forEach((message, index) => {
    if (message.role === 'user') {
      if (seenUser) turn += 1;
      seenUser = true;
    }
    let kind: TrajectoryKind;
    if (message.role === 'user') kind = 'user';
    else if (message.role === 'assistant') kind = 'assistant';
    else if (message.role === 'tool_use' || message.role === 'tool_result') kind = 'tool';
    else kind = 'error';
    records.push({ index, kind, message, turn });
  });
  return records;
}

/** 记录 → 悬浮提示文本。 */
function recordTooltip(record: TrajectoryRecord, toolLabel: (name: string) => string): string {
  const head = KIND_LABEL[record.kind];
  if (record.kind === 'tool') {
    const name = record.message.tool_name ?? '?';
    const parts = [`${head} ${toolLabel(name)}`];
    const args = toolArgsSummary(record.message.tool_arguments);
    if (args) parts.push(args);
    if (record.message.role === 'tool_result') {
      const result = resultSummary(record.message.content);
      if (result) parts.push(`→ ${result}`);
      if (record.message.tool_duration_ms != null) parts.push(formatDuration(record.message.tool_duration_ms));
    }
    return parts.join('\n');
  }
  return `${head}\n${record.message.content.replace(/\s+/g, ' ').slice(0, 120)}`;
}

// ============ 搜索索引（增量，仅重解析变更条目） ============

interface SearchEntry {
  sources: readonly string[];
  text: string;
}

function sameSources(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((v, i) => v === right[i]);
}

function searchableJson(value: unknown): string {
  if (value === undefined || value === null) return '';
  try {
    return JSON.stringify(value) ?? '';
  } catch {
    return '';
  }
}

export class TrajectorySearchIndex {
  private readonly entries = new Map<number, SearchEntry>();

  /** 增量同步当前记录流，返回索引是否发生变化。 */
  update(records: readonly TrajectoryRecord[], toolLabel: (name: string) => string): boolean {
    const seen = new Set<number>();
    let changed = false;
    for (const record of records) {
      seen.add(record.index);
      const sources = [
        KIND_LABEL[record.kind],
        record.message.tool_name ?? '',
        toolArgsSummary(record.message.tool_arguments),
        searchableJson(record.message.tool_arguments),
        record.message.content,
      ];
      const previous = this.entries.get(record.index);
      if (previous !== undefined && sameSources(previous.sources, sources)) continue;
      this.entries.set(record.index, {
        sources,
        text: sources.join('\n').toLocaleLowerCase(),
      });
      changed = true;
    }
    for (const key of this.entries.keys()) {
      if (!seen.has(key)) {
        this.entries.delete(key);
        changed = true;
      }
    }
    return changed;
  }

  /** 空格分隔的多词 AND 匹配；无词返回 null。 */
  search(query: string): ReadonlySet<number> | null {
    const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
    if (terms.length === 0) return null;
    const matches = new Set<number>();
    for (const [index, entry] of this.entries) {
      if (terms.every((term) => entry.text.includes(term))) matches.add(index);
    }
    return matches;
  }
}

// ============ 时间线模型 ============

type TimelineMode = 'sequence' | 'duration';

interface TimeRange {
  start: number;
  end: number;
}

interface TimelineSpan {
  index: number;
  kind: TrajectoryKind;
  isError: boolean;
  /** 0=输入 1=模型 2=工具。 */
  lane: number;
  start: number;
  end: number;
  label: string;
}

interface TimelineModel {
  domainStart: number;
  domainEnd: number;
  spans: readonly TimelineSpan[];
  turnBoundaries: readonly { turn: number; time: number }[];
}

function laneFor(kind: TrajectoryKind): number {
  if (kind === 'tool' || kind === 'error') return 2;
  if (kind === 'assistant') return 1;
  return 0;
}

/** 记录流 → 三泳道时间线模型。sequence 等宽序数域，duration 时间戳域。 */
function deriveTimeline(
  records: readonly TrajectoryRecord[],
  mode: TimelineMode,
  toolLabel: (name: string) => string,
): TimelineModel | null {
  if (records.length === 0) return null;
  const spans: TimelineSpan[] = [];
  const turnBoundaries: { turn: number; time: number }[] = [];
  if (mode === 'sequence') {
    records.forEach((record, i) => {
      if (record.kind === 'user') turnBoundaries.push({ turn: record.turn, time: i });
      spans.push({
        index: record.index,
        kind: record.kind,
        isError: record.kind === 'error' || record.message.tool_success === false,
        lane: laneFor(record.kind),
        start: i,
        end: i + 1,
        label: recordTooltip(record, toolLabel),
      });
    });
    return { domainStart: 0, domainEnd: records.length, spans, turnBoundaries };
  }
  records.forEach((record, i) => {
    const start = record.message.timestamp;
    const next = records[i + 1];
    let end = next ? next.message.timestamp : start + TIMELINE_MIN_SPAN_MS;
    if (end - start < TIMELINE_MIN_SPAN_MS) end = start + TIMELINE_MIN_SPAN_MS;
    if (record.kind === 'user') turnBoundaries.push({ turn: record.turn, time: start });
    spans.push({
      index: record.index,
      kind: record.kind,
      isError: record.kind === 'error' || record.message.tool_success === false,
      lane: laneFor(record.kind),
      start,
      end,
      label: recordTooltip(record, toolLabel),
    });
  });
  const domainStart = spans[0]?.start ?? 0;
  const domainEnd = spans[spans.length - 1]?.end ?? 1;
  return { domainStart, domainEnd, spans, turnBoundaries };
}

/** 选中区间内的记录索引集合（区间与 span 相交即命中）。 */
function timelineFocusIndexes(model: TimelineModel, range: TimeRange): ReadonlySet<number> {
  return new Set(
    model.spans
      .filter((span) => span.start <= range.end && span.end >= range.start)
      .map((span) => span.index),
  );
}

/** 距某域坐标最近的 span（空白点击聚焦用）。 */
function nearestSpan(model: TimelineModel, time: number): TimelineSpan | undefined {
  return model.spans.reduce((candidate, span) => {
    const candidateDistance = time < candidate.start
      ? candidate.start - time
      : time > candidate.end ? time - candidate.end : 0;
    const spanDistance = time < span.start
      ? span.start - time
      : time > span.end ? time - span.end : 0;
    return spanDistance < candidateDistance ? span : candidate;
  });
}

function orderedRange(left: number, right: number): TimeRange {
  return left <= right ? { start: left, end: right } : { start: right, end: left };
}

function clampFraction(value: number): number {
  return Math.min(1, Math.max(0, value));
}

// ============ 时间线组件 ============

interface TrajectoryTimelineProps {
  model: TimelineModel | null;
  mode: TimelineMode;
  range: TimeRange | null;
  selectedIndex: number | null;
  searchMatchIndexes: ReadonlySet<number> | null;
  onRangeChange: (range: TimeRange | null) => void;
  onRecordSelect: (index: number) => void;
}

/** 时间线最多渲染的色块数（超出按步长采样，缩放后自然恢复细节）。 */
const MAX_TIMELINE_SPANS = 240;
/** 时间线最多渲染的轮次分隔线数。 */
const MAX_TIMELINE_TURNS = 200;
/** 拖选时靠近边缘触发的平移步长（域时长比例）。 */
const EDGE_PAN_STEP_FRACTION = 0.025;

const clamp = (value: number, min: number, max: number): number =>
  Math.min(Math.max(value, min), Math.max(min, max));

const TrajectoryTimeline: React.FC<TrajectoryTimelineProps> = ({
  model,
  mode,
  range,
  selectedIndex,
  searchMatchIndexes,
  onRangeChange,
  onRecordSelect,
}) => {
  const { t } = useTranslation();
  const trackRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef<{
    pointerId: number;
    anchorTime: number;
    anchorClientX: number;
    recordIndex: number | null;
  } | null>(null);
  const panRef = useRef<{
    pointerId: number;
    anchorClientX: number;
    anchorStart: number;
    moved: boolean;
    pannable: boolean;
  } | null>(null);
  const [draft, setDraft] = useState<TimeRange | null>(null);
  const [viewport, setViewport] = useState<TimeRange | null>(null);
  const [animateViewport, setAnimateViewport] = useState(false);
  const [panning, setPanning] = useState(false);

  const fullDuration = model ? Math.max(1, model.domainEnd - model.domainStart) : 1;
  const minZoom = model === null ? 1 : Math.min(mode === 'sequence' ? 4 : 1000, fullDuration);
  const viewportDuration = viewport === null
    ? fullDuration
    : Math.min(fullDuration, Math.max(minZoom, viewport.end - viewport.start));
  const domainStart = model === null
    ? 0
    : viewport === null
      ? model.domainStart
      : clamp(viewport.start, model.domainStart, model.domainEnd - viewportDuration);
  const domainDuration = viewport === null ? fullDuration : viewportDuration;

  // 选中记录滚入视口（缩放状态下带动画平移）
  useEffect(() => {
    if (model === null || selectedIndex === null) return;
    const span = model.spans.find((s) => s.index === selectedIndex);
    if (span === undefined) return;
    setAnimateViewport(true);
    setViewport((current) => {
      if (current === null) return current;
      if (span.end > current.start && span.start < current.end) return current;
      const duration = Math.max(1, current.end - current.start);
      const desiredStart = span.end <= current.start ? span.start : span.end - duration;
      const nextStart = clamp(desiredStart, model.domainStart, model.domainEnd - duration);
      return nextStart === current.start ? current : { start: nextStart, end: nextStart + duration };
    });
  }, [model, selectedIndex]);

  // 模式切换重置视口；选中区间脱离数据域时清除
  useEffect(() => { setViewport(null); }, [mode]);
  useEffect(() => {
    if (
      model !== null && range !== null
      && (range.end < model.domainStart || range.start > model.domainEnd)
    ) {
      onRangeChange(null);
    }
  }, [model, range, onRangeChange]);
  // 数据域更新（流式追加/重算）：关闭动画标记，越界视口回退全量
  useEffect(() => {
    setAnimateViewport(false);
    setViewport((current) => {
      if (model === null || current === null) return current;
      return current.end < model.domainStart || current.start > model.domainEnd ? null : current;
    });
  }, [model]);

  // 滚轮缩放：以光标为锚点指数缩放，放大到全量时自动复位
  useEffect(() => {
    const track = trackRef.current;
    if (track === null || model === null) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      setAnimateViewport(false);
      const rect = track.getBoundingClientRect();
      const anchorFraction = clampFraction((event.clientX - rect.left) / Math.max(1, rect.width));
      const nextDuration = Math.min(
        fullDuration,
        Math.max(minZoom, domainDuration * Math.exp(event.deltaY * 0.0015)),
      );
      if (nextDuration >= fullDuration * 0.999) {
        setViewport(null);
        return;
      }
      const anchorTime = domainStart + anchorFraction * domainDuration;
      const nextStart = clamp(
        anchorTime - anchorFraction * nextDuration,
        model.domainStart,
        model.domainEnd - nextDuration,
      );
      setViewport({ start: nextStart, end: nextStart + nextDuration });
    };
    track.addEventListener('wheel', onWheel, { passive: false });
    return () => { track.removeEventListener('wheel', onWheel); };
  }, [model, domainStart, domainDuration, fullDuration, minZoom]);

  const fractionAt = (clientX: number): number => {
    const track = trackRef.current;
    if (!track) return 0;
    const rect = track.getBoundingClientRect();
    return clampFraction((clientX - rect.left) / Math.max(1, rect.width));
  };

  const timeAt = (clientX: number): number =>
    domainStart + fractionAt(clientX) * domainDuration;

  const recordIndexAt = (target: EventTarget | null): number | null => {
    if (!(target instanceof HTMLElement)) return null;
    const value = target.closest<HTMLElement>('[data-record-index]')?.dataset.recordIndex;
    if (value === undefined) return null;
    const index = Number(value);
    return Number.isFinite(index) ? index : null;
  };

  if (model === null) return null;

  const leftOf = (time: number): string =>
    `${((time - domainStart) / domainDuration) * 100}%`;

  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if (event.button === 2) {
      // 右键拖拽：平移视口（无缩放时禁用）；原地点击清除选中
      panRef.current = {
        pointerId: event.pointerId,
        anchorClientX: event.clientX,
        anchorStart: domainStart,
        moved: false,
        pannable: viewport !== null,
      };
      setAnimateViewport(false);
      setPanning(true);
      if (typeof event.currentTarget.setPointerCapture === 'function') {
        event.currentTarget.setPointerCapture(event.pointerId);
      }
      return;
    }
    if (event.button !== 0) return;
    const recordIndex = recordIndexAt(event.target);
    dragRef.current = {
      pointerId: event.pointerId,
      anchorTime: timeAt(event.clientX),
      anchorClientX: event.clientX,
      recordIndex,
    };
    if (typeof event.currentTarget.setPointerCapture === 'function') {
      event.currentTarget.setPointerCapture(event.pointerId);
    }
    setDraft({ start: dragRef.current.anchorTime, end: dragRef.current.anchorTime });
  };

  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    const rect = event.currentTarget.getBoundingClientRect();
    // 右键平移
    const pan = panRef.current;
    if (pan !== null && pan.pointerId === event.pointerId) {
      if (Math.abs(event.clientX - pan.anchorClientX) >= MINIMUM_DRAG_PX) pan.moved = true;
      if (!pan.pannable || !pan.moved) return;
      const delta = (event.clientX - pan.anchorClientX) / Math.max(1, rect.width);
      const nextStart = clamp(
        pan.anchorStart - delta * domainDuration,
        model.domainStart,
        model.domainEnd - domainDuration,
      );
      setViewport({ start: nextStart, end: nextStart + domainDuration });
      return;
    }
    const drag = dragRef.current;
    if (drag === null || drag.pointerId !== event.pointerId) return;
    // 拖选 + 缩放状态下靠近边缘：平移视口跟随
    let nextDomainStart = domainStart;
    if (viewport !== null) {
      const localX = event.clientX - rect.left;
      const edgeWidth = Math.min(32, Math.max(1, rect.width * 0.08));
      const direction = localX < edgeWidth ? -1 : localX > rect.width - edgeWidth ? 1 : 0;
      if (direction !== 0) {
        const edgeDistance = direction < 0 ? edgeWidth - localX : localX - (rect.width - edgeWidth);
        const strength = clampFraction(edgeDistance / edgeWidth);
        const desiredStart = domainStart
          + direction * domainDuration * EDGE_PAN_STEP_FRACTION * Math.max(0.2, strength);
        nextDomainStart = clamp(desiredStart, model.domainStart, model.domainEnd - domainDuration);
        if (nextDomainStart !== domainStart) {
          setAnimateViewport(false);
          setViewport({ start: nextDomainStart, end: nextDomainStart + domainDuration });
        }
      }
    }
    const pointTime = nextDomainStart + fractionAt(event.clientX) * domainDuration;
    setDraft(orderedRange(drag.anchorTime, pointTime));
  };

  const onPointerEnd = (event: React.PointerEvent<HTMLDivElement>) => {
    const pan = panRef.current;
    if (pan !== null && pan.pointerId === event.pointerId) {
      const moved = pan.moved || Math.abs(event.clientX - pan.anchorClientX) >= MINIMUM_DRAG_PX;
      panRef.current = null;
      setPanning(false);
      if (!moved) onRangeChange(null);
      return;
    }
    const drag = dragRef.current;
    if (drag === null || drag.pointerId !== event.pointerId) return;
    dragRef.current = null;
    setDraft(null);
    const click = Math.abs(event.clientX - drag.anchorClientX) < MINIMUM_DRAG_PX;
    if (click && drag.recordIndex !== null) {
      onRangeChange(null);
      onRecordSelect(drag.recordIndex);
      return;
    }
    const selected = orderedRange(drag.anchorTime, timeAt(event.clientX));
    if (click) {
      // 空白点击：聚焦最近的记录
      const nearest = nearestSpan(model, selected.start);
      onRangeChange(null);
      if (nearest !== undefined) onRecordSelect(nearest.index);
      return;
    }
    onRangeChange(selected);
  };

  const onPointerCancel = () => {
    dragRef.current = null;
    panRef.current = null;
    setDraft(null);
    setPanning(false);
  };

  const visibleRange = draft ?? range;

  // 视口内色块（采样上限保护超长会话 DOM）
  const visibleSpans = model.spans.filter((span) =>
    span.index === selectedIndex
    || (span.end >= domainStart && span.start <= domainStart + domainDuration));
  const spanStride = Math.max(1, Math.ceil(visibleSpans.length / MAX_TIMELINE_SPANS));
  let renderedSpans = spanStride > 1
    ? visibleSpans.filter((_, i) => i % spanStride === 0)
    : visibleSpans;
  if (spanStride > 1) {
    const sel = visibleSpans.find((s) => s.index === selectedIndex);
    if (sel !== undefined && !renderedSpans.includes(sel)) renderedSpans = [...renderedSpans, sel];
  }
  // 采样时每块至少占等分宽度，保证轨道视觉连续
  const shareWidth = renderedSpans.length > 0
    ? (domainDuration / fullDuration) * (100 / renderedSpans.length) * 0.92
    : 0;

  const visibleTurns = model.turnBoundaries.filter((b) =>
    b.time > model.domainStart && b.time >= domainStart && b.time <= domainStart + domainDuration);
  const turnStride = Math.max(1, Math.ceil(visibleTurns.length / MAX_TIMELINE_TURNS));
  const renderedTurns = turnStride > 1
    ? visibleTurns.filter((_, i) => i % turnStride === 0)
    : visibleTurns;

  return (
    <div className="codex-timeline">
      <div className="codex-timeline-labels" aria-hidden="true">
        <span>{t('mind_inspector.code_traj_input')}</span>
        <span>{t('mind_inspector.code_traj_model')}</span>
        <span>{t('mind_inspector.code_traj_tool')}</span>
      </div>
      <div
        ref={trackRef}
        className="codex-timeline-track"
        data-panning={panning || undefined}
        tabIndex={0}
        role="slider"
        aria-label={t('mind_inspector.code_traj_aria')}
        aria-valuenow={selectedIndex ?? undefined}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerEnd}
        onPointerCancel={onPointerCancel}
        onDoubleClick={() => onRangeChange(null)}
        onContextMenu={(event) => { event.preventDefault(); }}
        onKeyDown={(event) => {
          if (event.key === 'Escape' && range !== null) {
            event.preventDefault();
            onRangeChange(null);
          }
        }}
      >
        <div
          className="codex-timeline-domain"
          data-animate={animateViewport || undefined}
          style={{
            left: `${(-(domainStart - model.domainStart) / domainDuration) * 100}%`,
            width: `${(fullDuration / domainDuration) * 100}%`,
          }}
        >
          {renderedTurns.map((boundary) => (
            <span
              key={boundary.turn}
              className="codex-timeline-turn"
              style={{ left: `${((boundary.time - model.domainStart) / fullDuration) * 100}%` }}
              title={t('mind_inspector.code_traj_turn', { n: boundary.turn + 1 })}
            />
          ))}
          {renderedSpans.map((span) => (
            <span
              key={span.index}
              className="codex-timeline-span"
              data-kind={span.kind}
              data-error={span.isError || undefined}
              data-current={span.index === selectedIndex || undefined}
              data-search-match={searchMatchIndexes === null
                ? undefined
                : searchMatchIndexes.has(span.index) ? 'true' : 'false'}
              data-selected={range === null
                ? undefined
                : span.start <= range.end && span.end >= range.start ? 'true' : 'false'}
              data-record-index={span.index}
              title={span.label}
              style={{
                left: `${((span.start - model.domainStart) / fullDuration) * 100}%`,
                width: `${Math.max((span.end - span.start) / fullDuration * 100, shareWidth)}%`,
                ['--lane' as string]: span.lane,
              }}
            />
          ))}
        </div>
        {visibleRange !== null && (
          <div
            className="codex-timeline-selection"
            data-drafting={draft === null ? undefined : 'true'}
            aria-hidden="true"
            style={{
              left: leftOf(visibleRange.start),
              width: `${Math.max(0.5, ((visibleRange.end - visibleRange.start) / domainDuration) * 100)}%`,
            }}
          />
        )}
      </div>
      <div className="codex-timeline-side">
        {viewport !== null && (
          <button
            type="button"
            className="codex-timeline-reset"
            onClick={() => setViewport(null)}
            title={t('mind_inspector.code_traj_reset')}
          >
            {t('mind_inspector.code_traj_reset_btn')}
          </button>
        )}
        <span className="codex-timeline-mode">
          {mode === 'sequence' ? t('mind_inspector.code_traj_sequence') : `${formatClock(domainStart)} → ${formatClock(domainStart + domainDuration)}`}
          {spanStride > 1 ? ` · ${t('mind_inspector.code_traj_sampled')}` : ''}
        </span>
      </div>
    </div>
  );
};

// ============ 主面板 ============

type RowDesc =
  | { key: string; type: 'record'; record: TrajectoryRecord; turnLead: boolean; focus: boolean }
  | { key: string; type: 'collapsed'; turn: number; toolCount: number };

export interface TrajectoryPanelProps {
  messages: readonly TrajectoryMessage[];
  running: boolean;
  toolLabel: (name: string) => string;
}

const EMPTY_SET: ReadonlySet<number> = new Set();

const TrajectoryPanel: React.FC<TrajectoryPanelProps> = ({ messages, running, toolLabel }) => {
  const { t } = useTranslation();
  const records = useMemo(() => deriveTrajectoryRecords(messages), [messages]);
  const [selected, setSelected] = useState<number | null>(null);
  const [collapsedTurns, setCollapsedTurns] = useState<ReadonlySet<number>>(EMPTY_SET);
  const [timelineMode, setTimelineMode] = useState<TimelineMode>('sequence');
  const [timelineRange, setTimelineRange] = useState<TimeRange | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [searchIndex] = useState(() => new TrajectorySearchIndex());
  const tableRef = useRef<HTMLDivElement>(null);
  const atBottomRef = useRef(true);
  const pendingScrollIndexRef = useRef<number | null>(null);

  // 增量索引：内容未变的条目复用旧解析结果
  useMemo(() => { searchIndex.update(records, toolLabel); }, [searchIndex, records, toolLabel]);

  const searchMatches = useMemo(
    () => searchIndex.search(searchQuery),
    [searchIndex, searchQuery, records],
  );

  const timelineModel = useMemo(
    () => deriveTimeline(records, timelineMode, toolLabel),
    [records, timelineMode, toolLabel],
  );

  const focusIndexes = useMemo(
    () => (timelineModel !== null && timelineRange !== null
      ? timelineFocusIndexes(timelineModel, timelineRange)
      : null),
    [timelineModel, timelineRange],
  );

  /** 搜索过滤 + 轮次折叠 → 可渲染行序列。 */
  const rows = useMemo<RowDesc[]>(() => {
    const filtered = searchMatches !== null
      ? records.filter((r) => searchMatches.has(r.index))
      : records;
    const firstNonUserOfTurn = new Map<number, number>();
    for (const r of filtered) {
      if (r.kind !== 'user' && !firstNonUserOfTurn.has(r.turn)) {
        firstNonUserOfTurn.set(r.turn, r.index);
      }
    }
    const emittedCollapsed = new Set<number>();
    const out: RowDesc[] = [];
    for (const record of filtered) {
      const turnLead = record.kind === 'user'
        || firstNonUserOfTurn.get(record.turn) === record.index;
      if (collapsedTurns.has(record.turn) && record.kind !== 'user' && !turnLead) {
        if (!emittedCollapsed.has(record.turn)) {
          emittedCollapsed.add(record.turn);
          const toolCount = records.filter((r) => r.turn === record.turn && r.kind === 'tool').length;
          out.push({ key: `collapsed-${record.turn}`, type: 'collapsed', turn: record.turn, toolCount });
        }
        continue;
      }
      out.push({
        key: `r-${record.index}`,
        type: 'record',
        record,
        turnLead,
        focus: focusIndexes === null || focusIndexes.has(record.index),
      });
    }
    return out;
  }, [records, searchMatches, collapsedTurns, focusIndexes]);

  const virtualizationEnabled = rows.length > VIRTUALIZATION_THRESHOLD;
  const virtualizer = useVirtualizer({
    count: virtualizationEnabled ? rows.length : 0,
    getScrollElement: () => tableRef.current,
    estimateSize: () => ROW_HEIGHT,
    overscan: 12,
    getItemKey: (i) => rows[i]?.key ?? i,
  });

  /** 可折叠轮次（非 user 记录数 > 1）。 */
  const collapsibleTurns = useMemo(() => {
    const counts = new Map<number, number>();
    for (const r of records) {
      if (r.kind !== 'user') counts.set(r.turn, (counts.get(r.turn) ?? 0) + 1);
    }
    return [...counts.entries()].filter(([, n]) => n > 1).map(([turn]) => turn);
  }, [records]);

  const allCollapsed = collapsibleTurns.length > 0
    && collapsibleTurns.every((t) => collapsedTurns.has(t));

  const toggleTurn = useCallback((turn: number) => {
    setCollapsedTurns((prev) => {
      const next = new Set(prev);
      if (next.has(turn)) next.delete(turn); else next.add(turn);
      return next;
    });
  }, []);

  /** 时间线选中：展开所属轮次 + 滚动到对应行。 */
  const selectFromTimeline = useCallback((index: number) => {
    setCollapsedTurns((prev) => {
      if (prev.size === 0) return prev;
      const turn = records.find((r) => r.index === index)?.turn;
      if (turn === undefined || !prev.has(turn)) return prev;
      const next = new Set(prev);
      next.delete(turn);
      return next;
    });
    setSelected(index);
    pendingScrollIndexRef.current = index;
  }, [records]);

  // 待滚动行就绪后居中滚动（折叠展开/过滤重算后再定位）
  useEffect(() => {
    const index = pendingScrollIndexRef.current;
    if (index === null) return;
    const pos = rows.findIndex((row) => row.type === 'record' && row.record.index === index);
    if (pos === -1) return;
    pendingScrollIndexRef.current = null;
    if (virtualizationEnabled) {
      virtualizer.scrollToIndex(pos, { align: 'center' });
    } else {
      tableRef.current
        ?.querySelector(`[data-row-key="r-${index}"]`)
        ?.scrollIntoView({ behavior: 'smooth', block: 'center' });
    }
  }, [rows, virtualizationEnabled, virtualizer]);

  const onTableScroll = useCallback(() => {
    const el = tableRef.current;
    if (!el) return;
    atBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight <= 4;
  }, []);

  // 运行中跟随底部（用户上滚查看历史时停止跟随）
  useEffect(() => {
    const el = tableRef.current;
    if (!el || !atBottomRef.current) return;
    if (virtualizationEnabled) {
      virtualizer.scrollToIndex(rows.length - 1, { align: 'end' });
    } else {
      el.scrollTop = el.scrollHeight;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rows.length]);

  const selectedRecord = selected !== null
    ? records.find((r) => r.index === selected)
    : undefined;

  const renderRow = (row: RowDesc) => {
    if (row.type === 'collapsed') {
      return (
        <button
          key={row.key}
          type="button"
          data-row-key={row.key}
          className="codex-trajectory-row codex-trajectory-collapsed"
          onClick={() => toggleTurn(row.turn)}
        >
          <span className="codex-trajectory-kind tool">…</span>
          <span className="codex-trajectory-text">
            {t('mind_inspector.code_traj_collapsed_tools', { count: row.toolCount })}
          </span>
        </button>
      );
    }
    const { record } = row;
    const { message } = record;
    let line: React.ReactNode;
    if (record.kind === 'tool') {
      const args = toolArgsSummary(message.tool_arguments);
      const isResult = message.role === 'tool_result';
      const result = isResult ? resultSummary(message.content) : '';
      const failed = isResult && message.tool_success === false;
      line = (
        <>
          <span className="codex-trajectory-tool-name">{toolLabel(message.tool_name ?? '?')}</span>
          {args && <span className="codex-trajectory-tool-args">{args}</span>}
          {result && (
            <span className={`codex-trajectory-result ${failed ? 'failed' : ''}`}>
              <span className="codex-trajectory-arrow">→</span>
              {result}
            </span>
          )}
          {message.tool_duration_ms != null && (
            <span className="codex-trajectory-duration">
              {formatDuration(message.tool_duration_ms)}
            </span>
          )}
        </>
      );
    } else if (record.kind === 'error') {
      line = <span className="codex-trajectory-text error">{message.content.slice(0, 120)}</span>;
    } else {
      line = <span className="codex-trajectory-text">{message.content.replace(/\s+/g, ' ').slice(0, 110)}</span>;
    }
    return (
      <button
        key={row.key}
        type="button"
        data-row-key={row.key}
        data-kind={record.kind}
        data-turn-start={row.turnLead || undefined}
        data-focus={focusIndexes === null ? undefined : row.focus ? 'inside' : 'outside'}
        className={`codex-trajectory-row ${selected === record.index ? 'selected' : ''}`}
        onClick={() => setSelected(selected === record.index ? null : record.index)}
        onDoubleClick={() => {
          if (collapsibleTurns.includes(record.turn)) toggleTurn(record.turn);
        }}
      >
        {record.kind === 'user' && (
          <span className="codex-trajectory-turn">#{record.turn + 1}</span>
        )}
        <span className={`codex-trajectory-kind ${record.kind}`}>
          {KIND_LABEL[record.kind]}
        </span>
        {line}
      </button>
    );
  };

  if (records.length === 0) {
    return (
      <div className="codex-info-card">
        <div className="codex-info-title"><GitFork size={13} /> {t('mind_inspector.code_inspector_trajectory')}</div>
        <div className="codex-empty-note">
          {running ? t('mind_inspector.code_traj_running') : t('mind_inspector.code_traj_empty')}
        </div>
      </div>
    );
  }

  const turnCount = records[records.length - 1]?.turn ?? 0;
  const toolCount = records.filter((r) => r.kind === 'tool').length;

  return (
    <div className="codex-trajectory">
      <div className="codex-trajectory-head">
        <span>
          {t('mind_inspector.code_traj_stats', { turns: turnCount + 1, tools: toolCount, events: records.length })}
          {searchMatches !== null && ` · ${t('mind_inspector.code_traj_match', { n: searchMatches.size })}`}
        </span>
        <div className="codex-trajectory-tools">
          <label className="codex-trajectory-search">
            <Search size={11} />
            <input
              type="text"
              value={searchQuery}
              placeholder={t('mind_inspector.code_traj_search')}
              spellCheck={false}
              onChange={(e) => setSearchQuery(e.target.value)}
            />
            {searchQuery !== '' && (
              <button type="button" aria-label={t('mind_inspector.code_traj_clear_search')} onClick={() => setSearchQuery('')}>
                <X size={10} />
              </button>
            )}
          </label>
          <button
            type="button"
            className="codex-trajectory-fold"
            onClick={() => setTimelineMode(timelineMode === 'sequence' ? 'duration' : 'sequence')}
            title={timelineMode === 'sequence' ? t('mind_inspector.code_traj_switch_duration') : t('mind_inspector.code_traj_switch_sequence')}
          >
            {timelineMode === 'sequence' ? t('mind_inspector.code_traj_sequence') : t('mind_inspector.code_traj_duration')}
          </button>
          {collapsibleTurns.length > 0 && (
            <button
              type="button"
              className="codex-trajectory-fold"
              onClick={() => setCollapsedTurns(allCollapsed ? EMPTY_SET : new Set(collapsibleTurns))}
            >
              {allCollapsed ? t('mind_inspector.code_traj_expand_all') : t('mind_inspector.code_traj_collapse_all')}
            </button>
          )}
        </div>
      </div>

      <TrajectoryTimeline
        model={timelineModel}
        mode={timelineMode}
        range={timelineRange}
        selectedIndex={selected}
        searchMatchIndexes={searchMatches}
        onRangeChange={setTimelineRange}
        onRecordSelect={selectFromTimeline}
      />

      <div
        ref={tableRef}
        className={`codex-trajectory-table ${virtualizationEnabled ? 'virtual' : ''}`}
        onScroll={onTableScroll}
      >
        {virtualizationEnabled ? (
          <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
            {virtualizer.getVirtualItems().map((item) => {
              const row = rows[item.index];
              if (row === undefined) return null;
              return (
                <div
                  key={row.key}
                  className="codex-trajectory-vrow"
                  style={{
                    height: ROW_HEIGHT - 1,
                    transform: `translateY(${item.start}px)`,
                  }}
                >
                  {renderRow(row)}
                </div>
              );
            })}
          </div>
        ) : (
          rows.map((row) => renderRow(row))
        )}
      </div>

      {selectedRecord && (
        <div className="codex-trajectory-detail">
          <div className="codex-trajectory-detail-head">
            <span className={`codex-trajectory-kind ${selectedRecord.kind}`}>
              {KIND_LABEL[selectedRecord.kind]}
            </span>
            <span className="codex-trajectory-detail-loc">
              {t('mind_inspector.code_traj_detail_loc', { turn: selectedRecord.turn + 1, index: selectedRecord.index + 1 })}
            </span>
            <button type="button" className="codex-trajectory-close" onClick={() => setSelected(null)}>×</button>
          </div>
          {selectedRecord.message.tool_arguments != null && (
            <div className="codex-trajectory-section">
              <div className="codex-trajectory-section-title">{t('mind_inspector.code_traj_args')}</div>
              <pre>{JSON.stringify(selectedRecord.message.tool_arguments, null, 2)}</pre>
            </div>
          )}
          {selectedRecord.message.content && (
            <div className="codex-trajectory-section">
              <div className="codex-trajectory-section-title">
                {selectedRecord.kind === 'tool' && selectedRecord.message.role === 'tool_result' ? t('mind_inspector.code_traj_tool_result') : t('mind_inspector.code_traj_content')}
              </div>
              <pre>{selectedRecord.message.content}</pre>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

export default TrajectoryPanel;
