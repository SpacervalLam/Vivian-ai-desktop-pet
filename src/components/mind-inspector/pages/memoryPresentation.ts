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
export type MemoryTurn = { speaker: string; audience?: string; text: string };
export type PreparedMemory = {
  item: MemoryRecord;
  body: string;
  turns: MemoryTurn[];
  partner: string;
};
export type RecentThread = {
  id: string;
  title: string;
  time: number;
  turns: MemoryTurn[];
  items: MemoryRecord[];
  searchText: string;
};

const SCAFFOLD_MARKERS = [
  '[Current conversation thread:', '[近期你们的话题]', '[请基于上述记忆自然地回应]',
  '[交接上下文：', '[共同观察]', '[Natural closing]', '[Conversation rhythm]',
];

const seconds = (value: number) => value > 1e12 ? value / 1000 : value;
const dialogueTime = (item: MemoryRecord) => seconds(
  typeof item.metadata?.spoken_at === 'number' ? item.metadata.spoken_at : item.created_at,
);
const speakerName = (value: string, character: Character) => {
  const key = value.trim().toLowerCase();
  if (key === 'i' || key === 'me') return character === 'vivian' ? 'Vivian' : 'Nana';
  if (key === 'user') return '用户';
  if (key === 'vivian') return 'Vivian';
  if (key === 'nana') return 'Nana';
  return value.trim();
};

const cleanBody = (text: string) => {
  let end = text.length;
  for (const marker of SCAFFOLD_MARKERS) {
    const position = text.indexOf(marker);
    if (position >= 0 && position < end) end = position;
  }
  return text.slice(0, end).replace(/\r\n/g, '\n').trim();
};

export const splitSpeechPrefix = (content: string) => {
  const match = content.match(/^\[([^\[\]]+?)\s+says?\s+to\s+([^\[\]]+?)\]\s*/i);
  return match
    ? { body: cleanBody(content.slice(match[0].length)), speaker: match[1].trim(), audience: match[2].trim() }
    : { body: cleanBody(content), speaker: '', audience: '' };
};

export const speechLabel = (speaker: string, audience: string, character: Character) => {
  const name = speakerName(speaker, character);
  const target = audience.toLowerCase() === 'everyone' ? '大家' : audience.toLowerCase() === 'me' ? (character === 'vivian' ? 'Vivian' : 'Nana') : speakerName(audience, character);
  return `${name} → ${target}`;
};

export const isMemorySummary = (item: MemoryRecord): boolean =>
  item.memory_type === 'session_summary'
  || item.tags.some((tag) => ['topic_summary', 'session_summary', 'summary'].includes(tag))
  || ['summary', 'session_summary', 'topic_summary', 'compacted_summary'].includes(String(item.metadata?.content_type ?? ''));

export const isDialogueMemory = (item: MemoryRecord): boolean => {
  if (isMemorySummary(item) || item.metadata?.perspective === 'observer'
    || !['short_term', 'casual_conversation'].includes(item.memory_type)
    || item.tags.some((tag) => ['reflection', 'inner_monologue', 'current_thought'].includes(tag))) return false;
  const speech = splitSpeechPrefix(item.content);
  const speaker = speech.speaker || String(item.metadata?.speaker ?? '');
  const actualSpeech = !!speech.speaker || item.tags.some((tag) => ['dialogue_turn', 'user', 'assistant'].includes(tag));
  return actualSpeech && ['user', 'vivian', 'nana', 'i', 'me'].includes(speaker.trim().toLowerCase());
};

export const prepareMemory = (item: MemoryRecord, character: Character): PreparedMemory | null => {
  const speech = splitSpeechPrefix(item.content);
  const body = speech.body;
  if (item.metadata?.speaker === 'system' || item.metadata?.system_directive === true
    || /^(?:你刚听到用户和[^\n]+的对话[:：]|You just overheard a conversation between the user and|ユーザーと[^\n]+の会話を聞いてしまった[:：])/.test(body)) return null;
  if (!body || /^(?:Treat this as context|Respond to the latest message|JSON output|输出 JSON|tags:|importance:|protected:)/i.test(body)) return null;
  const metaSpeaker = typeof item.metadata?.speaker === 'string' ? item.metadata.speaker : '';
  const metaAudience = typeof item.metadata?.listener === 'string' ? item.metadata.listener : '';
  const speaker = speech.speaker || (item.tags.includes('topic_summary') ? '' : metaSpeaker);
  const turns = [{ speaker: speaker ? speakerName(speaker, character) : '', audience: speech.audience || metaAudience, text: body }];
  const partner = (String(item.metadata?.channel ?? '') === 'cross_character'
    ? speakerName(speakerName(speaker, character).toLowerCase() === character ? (speech.audience || metaAudience) : speaker, character) : '');
  return { item, body, turns: turns.filter((turn) => turn.text), partner };
};

export const buildRecentThreads = (items: MemoryRecord[], character: Character): RecentThread[] => {
  const curated = items.filter(isDialogueMemory).map((item) => prepareMemory(item, character))
    .filter((item): item is PreparedMemory => !!item && item.turns.length > 0);
  curated.sort((a, b) => dialogueTime(a.item) - dialogueTime(b.item));

  const threads: Array<RecentThread & { key: string; lastTime: number }> = [];
  const sessions = new Map<string, typeof threads[number]>();
  for (const entry of curated) {
    const channel = String(entry.item.metadata?.channel ?? 'direct');
    const session = entry.item.metadata?.conversation_id || entry.item.metadata?.session_id;
    const sessionId = typeof session === 'string' ? session.trim() : '';
    // direct/wechat/proactive are entry points into the same user conversation.
    const key = sessionId ? `session:${sessionId}` : entry.partner ? `cross:${entry.partner}` : 'user';
    const time = dialogueTime(entry.item);
    const last = sessionId ? sessions.get(sessionId) : threads[threads.length - 1];
    const canJoin = !!last && last.key === key && (sessionId || time - last.lastTime <= 1800)
      && (sessionId || entry.turns.every((turn) => turn.speaker));
    const title = entry.partner ? `与 ${entry.partner} 的对话` : channel === 'wechat' ? '微信对话' : '对话记录';
    if (canJoin) {
      last.items.push(entry.item);
      last.lastTime = time;
      last.time = Math.max(last.time, time);
      last.turns.push(...entry.turns);
      last.searchText = last.turns.map((turn) => turn.text).join(' ');
    } else {
      threads.push({ id: entry.item.id, title, time, turns: [...entry.turns], items: [entry.item], searchText: entry.turns.map((turn) => turn.text).join(' '), key, lastTime: time });
      if (sessionId) sessions.set(sessionId, threads[threads.length - 1]);
    }
  }
  return threads.sort((a, b) => seconds(b.time) - seconds(a.time));
};
