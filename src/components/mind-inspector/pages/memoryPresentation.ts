export type MemoryRecord = {
  id: string;
  content: string;
  memory_type: string;
  importance: number;
  created_at: number;
  tags: string[];
  metadata?: Record<string, unknown>;
  consolidated?: boolean;
  open_hooks?: Array<{ type?: string; condition: string; closed_at?: number | null }>;
};

export type Character = 'vivian' | 'nana';
import type { StickerRef } from '../../../types';
export type MemoryTurn = { id?: string; speaker: string; audience?: string; text: string; timestamp?: number; sticker?: StickerRef };
export type ConversationRecord = {
  id: string; source_session_id?: string; title: string; participants: string[]; channels: string[];
  started_at: number; ended_at: number;
  turns: Array<{ id: string; speaker: string; listener: string; text: string; timestamp: number; channel: string; sticker?: StickerRef }>;
};
export type RecentThread = {
  id: string;
  title: string;
  time: number;
  turns: MemoryTurn[];
  items: MemoryRecord[];
  searchText: string;
  canonical?: boolean;
  startedAt?: number;
};

const speakerName = (value: string, character: Character) => {
  const key = value.trim().toLowerCase();
  if (key === 'i' || key === 'me') return character === 'vivian' ? 'Vivian' : 'Nana';
  if (key === 'user') return '用户';
  if (key === 'vivian') return 'Vivian';
  if (key === 'nana') return 'Nana';
  return value.trim();
};

export const isMemoryFact = (item: MemoryRecord): boolean => item.metadata?.record_kind === 'fact';
export const isMemorySummary = (item: MemoryRecord): boolean => item.metadata?.record_kind === 'session_summary';
export const isVisibleMemoryFact = (item: MemoryRecord): boolean =>
  !['system_seed', 'environment_preset'].includes(String(item.metadata?.source ?? ''))
  && !!item.content.trim() && !item.consolidated
  && !['subjective', 'observation', 'internal'].includes(String(item.metadata?.record_kind ?? ''))
  && isMemoryFact(item);

export type EventProgress = {
  id: string; title: string; phase: 'planned' | 'started' | 'progressed' | 'completed' | 'cancelled';
  detail: string; source_message_id: string; source_quote: string; recorded_at: number;
  conversationId: string;
};
export type SharedEventTimeline = { id: string; title: string; progress: EventProgress[]; conversationIds: string[]; time: number; searchText: string };

/** Derived view of evidence-backed event progress; summaries themselves are never event cards. */
export const sharedEventTimeline = (items: MemoryRecord[], records: ConversationRecord[]): SharedEventTimeline[] => {
  const originals = new Map(records.flatMap((record) => record.turns.map((turn) => [turn.id, { turn, conversationId: record.id }] as const)));
  const groups = new Map<string, SharedEventTimeline>();
  for (const item of items) {
    if (!isMemorySummary(item) || item.consolidated || item.metadata?.index_active === false || item.metadata?.event_schema_version !== 1) continue;
    const parts = item.metadata?.summary_parts;
    if (!Array.isArray(parts)) continue;
    for (const part of parts) {
      if (!Array.isArray(part?.events)) continue;
      for (const event of part.events) {
        if (!event || typeof event.id !== 'string' || typeof event.title !== 'string' || typeof event.detail !== 'string'
          || typeof event.source_quote !== 'string' || !event.source_quote.trim()
          || !['planned', 'started', 'progressed', 'completed', 'cancelled'].includes(event.phase)) continue;
        const source = originals.get(event.source_message_id);
        if (!source || source.turn.speaker !== 'user' || !source.turn.text.includes(event.source_quote)) continue;
        let group = groups.get(event.id);
        if (!group) {
          group = { id: event.id, title: event.title, progress: [], conversationIds: [], time: 0, searchText: '' };
          groups.set(event.id, group);
        }
        if (group.progress.some((p) => p.source_message_id === event.source_message_id)) continue;
        group.progress.push({ ...event, recorded_at: source.turn.timestamp, conversationId: source.conversationId });
      }
    }
  }
  for (const group of groups.values()) {
    group.progress.sort((a, b) => a.recorded_at - b.recorded_at || a.source_message_id.localeCompare(b.source_message_id));
    group.conversationIds = [...new Set(group.progress.map((p) => p.conversationId))];
    group.time = group.progress[group.progress.length - 1]?.recorded_at ?? 0;
    group.searchText = `${group.title} ${group.progress.map((p) => `${p.detail} ${p.source_quote}`).join(' ')}`;
  }
  return [...groups.values()].sort((a, b) => b.time - a.time || a.id.localeCompare(b.id));
};

/** Canonical original history has stable message IDs; never deduplicate repeated speech by content. */
export const conversationThreads = (records: ConversationRecord[], character: Character): RecentThread[] => records.map((record) => ({
  id: record.id, title: record.title, time: record.ended_at, startedAt: record.started_at, canonical: true,
  turns: record.turns.map((turn) => ({ id: turn.id, speaker: speakerName(turn.speaker, character),
    audience: speakerName(turn.listener === 'all' ? '大家' : turn.listener, character),
    text: turn.text, timestamp: turn.timestamp, sticker: turn.sticker })),
  items: [], searchText: `${record.title} ${record.turns.map((turn) => turn.text).join(' ')}`,
})).sort((a, b) => b.time - a.time);
