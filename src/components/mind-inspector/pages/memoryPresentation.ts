import { comicLocale, comicText } from '../../../i18n/comic.js';

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
/** 图片消息的展示信息：相对 `<user_data_dir>` 的路径 + 识图结果文本 */
export type MemoryImage = { path?: string; description?: string };
export type MemoryTurn = { id?: string; speaker: string; audience?: string; text: string; timestamp?: number; sticker?: StickerRef; image?: MemoryImage };
export type ConversationRecord = {
  id: string; source_session_id?: string; title: string; participants: string[]; channels: string[];
  started_at: number; ended_at: number;
  turns: Array<{ id: string; utterance_id?: string; speaker: string; listener: string; text: string; timestamp: number; channel: string; sticker?: StickerRef; image_path?: string; visual_description?: string }>;
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
/** 识图结果是「对这一张图的观察」，不是关于用户的事实：无论落在哪层都不进事实卡。 */
const isImageObservation = (item: MemoryRecord): boolean => item.metadata?.kind === 'image';
export const isVisibleMemoryFact = (item: MemoryRecord): boolean =>
  !['system_seed', 'environment_preset'].includes(String(item.metadata?.source ?? ''))
  && !!item.content.trim() && !item.consolidated && !isImageObservation(item)
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
export function conversationTitle(record: ConversationRecord, character: Character, language = 'zh-CN'): string {
  const others = record.participants.filter(name => name !== character);
  const originalNames = others.map(name => name === 'user' ? '你' : name === 'vivian' ? 'Vivian' : name === 'nana' ? 'Nana' : name);
  // Only the server-generated default is localized; custom titles remain original.
  if (record.title !== `与${originalNames.join('、')}的对话`) return record.title;
  const firstTurn = record.turns.reduce<ConversationRecord['turns'][number] | undefined>((first, turn) =>
    !first || turn.timestamp < first.timestamp ? turn : first, undefined);
  const first = firstTurn?.speaker ?? (record.participants.includes('user') ? 'user' : record.participants[0] ?? character);
  const participants = [...new Set([first, ...record.participants])];
  const name = (person: string) => person === 'user' ? comicText('conversationYou', language) : speakerName(person, character);
  return comicText('conversationParticipants', language).replace('{{first}}', name(first))
    .replace('{{others}}', participants.filter(person => person !== first).map(name).join(comicLocale(language) === 'en' ? ', ' : '、'));
}
/** 图片消息用识图结果当正文，`📷 [图片]` 只是没有描述时的兜底占位。 */
const turnImage = (turn: ConversationRecord['turns'][number]): MemoryImage | undefined => {
  const description = turn.visual_description?.trim();
  if (!description && !turn.image_path) return undefined;
  return { path: turn.image_path, description: description || undefined };
};
const turnText = (turn: ConversationRecord['turns'][number]): string =>
  turn.visual_description?.trim() || turn.text;

export const conversationThreads = (records: ConversationRecord[], character: Character, language = 'zh-CN'): RecentThread[] => records.map((record) => ({
  id: record.id, title: conversationTitle(record, character, language), time: record.ended_at, startedAt: record.started_at, canonical: true,
  turns: record.turns.map((turn) => ({ id: turn.id, speaker: speakerName(turn.speaker, character),
    audience: speakerName(turn.listener === 'all' ? '大家' : turn.listener, character),
    text: turnText(turn), timestamp: turn.timestamp, sticker: turn.sticker, image: turnImage(turn) })),
  items: [], searchText: `${[...new Set([conversationTitle(record, character, language), record.title])].join(' ')} ${record.turns.map(turnText).join(' ')}`,
})).sort((a, b) => b.time - a.time);

export type UnifiedThread = RecentThread & { owner: Character; sourceIds: string[] };
type OwnedConversation = { record: ConversationRecord; owner: Character };
const privatePetConversation = (record: ConversationRecord) => record.participants.length === 2
  && record.participants.includes('vivian') && record.participants.includes('nana');
const sameSpeech = (a: ConversationRecord['turns'][number], b: ConversationRecord['turns'][number]) =>
  a.speaker === b.speaker && a.listener === b.listener && a.text === b.text
  && JSON.stringify(a.sticker) === JSON.stringify(b.sticker);
const mirroredTurn = (a: ConversationRecord['turns'][number], b: ConversationRecord['turns'][number], sameSession: boolean) => {
  if (a.utterance_id && b.utterance_id) return a.utterance_id === b.utterance_id;
  return a.id === b.id || (sameSession && sameSpeech(a, b) && Math.abs(a.timestamp - b.timestamp) <= 30);
};

/** Merge the two histories by speech identity, keeping each original source addressable. */
export function unifiedConversationThreads(vivian: ConversationRecord[], nana: ConversationRecord[], language = 'zh-CN'): UnifiedThread[] {
  const groups: OwnedConversation[][] = [];
  for (const entry of [...vivian.map(record => ({ record, owner: 'vivian' as const })),
    ...nana.map(record => ({ record, owner: 'nana' as const }))]) {
    const matches = privatePetConversation(entry.record) ? groups.filter(group => group.some(peer =>
      peer.owner !== entry.owner && privatePetConversation(peer.record)
      && peer.record.turns.some(a => entry.record.turns.some(b => mirroredTurn(a, b,
        !!entry.record.source_session_id && entry.record.source_session_id === peer.record.source_session_id))))) : [];
    if (!matches.length) groups.push([entry]);
    else {
      matches[0].push(entry);
      for (const group of matches.slice(1)) { matches[0].push(...group); groups.splice(groups.indexOf(group), 1); }
    }
  }
  return groups.map(group => {
    // A mirror can be saved later than the original. Earliest occurrence owns timing.
    const occurrences = group.flatMap(({ record, owner }) => record.turns.map(turn => ({ turn, record, owner })))
      .sort((a, b) => a.turn.timestamp - b.turn.timestamp);
    const kept: typeof occurrences = [];
    const paired = new Set<string>();
    for (const occurrence of occurrences) {
      const mirror = kept.findIndex((peer, index) => {
        if (peer.turn.utterance_id && occurrence.turn.utterance_id) return peer.turn.utterance_id === occurrence.turn.utterance_id;
        // Match legacy mirrors one-for-one; repeated speech in one history is retained.
        return peer.owner !== occurrence.owner && !paired.has(`${index}:${occurrence.owner}`)
          && mirroredTurn(peer.turn, occurrence.turn, !!peer.record.source_session_id
            && peer.record.source_session_id === occurrence.record.source_session_id);
      });
      if (mirror < 0) kept.push(occurrence);
      else paired.add(`${mirror}:${occurrence.owner}`);
    }
    const initiator = kept[0]?.turn.speaker;
    const primary = group.find(entry => entry.owner === initiator) ?? group[0];
    const merged = { ...primary.record, turns: kept.map(entry => entry.turn),
      started_at: Math.min(...group.map(entry => entry.record.started_at)),
      ended_at: Math.max(...group.map(entry => entry.record.ended_at)) };
    return { ...conversationThreads([merged], primary.owner, language)[0], owner: primary.owner,
      sourceIds: group.map(entry => entry.record.id) };
  }).sort((a, b) => b.time - a.time);
}
