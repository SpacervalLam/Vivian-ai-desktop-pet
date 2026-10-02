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
  isDialogueSummary: boolean;
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

const normalized = (text: string) => text.replace(/\s+/g, '').replace(/[，。；、！？,.!?;:：]/g, '').toLowerCase();
const seconds = (value: number) => value > 1e12 ? value / 1000 : value;
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

const parseCrossSummary = (body: string, character: Character): { turns: MemoryTurn[]; partner: string } | null => {
  const current = character === 'vivian' ? 'Vivian' : 'Nana';
  const source = body.match(/^我和\s*(Vivian|Nana)\s*聊了聊[：:]\s*我对她说[：:]\s*([\s\S]*?)[；;]\s*她回复我[：:]\s*([\s\S]*)$/);
  if (source) return { partner: source[1], turns: [
    { speaker: current, text: source[2].trim() },
    { speaker: source[1], text: source[3].trim() },
  ] };
  const target = body.match(/^(Vivian|Nana)\s*和我聊天[：:]\s*她说[：:]\s*([\s\S]*?)[；;]\s*我回复她[：:]\s*([\s\S]*)$/);
  if (target) return { partner: target[1], turns: [
    { speaker: target[1], text: target[2].trim() },
    { speaker: current, text: target[3].trim() },
  ] };
  const singleSource = body.match(/^我对\s*(Vivian|Nana)\s*说了[：:]\s*([\s\S]*?)[；;]\s*她([\s\S]*)$/);
  if (singleSource) return { partner: singleSource[1], turns: [{ speaker: current, text: singleSource[2].trim() }] };
  const singleTarget = body.match(/^(Vivian|Nana)\s*对我说[：:]\s*([\s\S]*?)[；;]\s*我([\s\S]*)$/);
  if (singleTarget) return { partner: singleTarget[1], turns: [{ speaker: singleTarget[1], text: singleTarget[2].trim() }] };
  return null;
};

export const prepareMemory = (item: MemoryRecord, character: Character): PreparedMemory | null => {
  const speech = splitSpeechPrefix(item.content);
  const body = speech.body;
  if (item.metadata?.speaker === 'system' || item.metadata?.system_directive === true
    || /^(?:你刚听到用户和[^\n]+的对话[:：]|You just overheard a conversation between the user and|ユーザーと[^\n]+の会話を聞いてしまった[:：])/.test(body)) return null;
  if (!body || /^(?:Treat this as context|Respond to the latest message|JSON output|输出 JSON|tags:|importance:|protected:)/i.test(body)) return null;
  const summary = item.tags.includes('topic_summary') && item.tags.includes('cross_character')
    ? parseCrossSummary(body, character) : null;
  const metaSpeaker = typeof item.metadata?.speaker === 'string' ? item.metadata.speaker : '';
  const metaAudience = typeof item.metadata?.listener === 'string' ? item.metadata.listener : '';
  const speaker = speech.speaker || (item.tags.includes('topic_summary') ? '' : metaSpeaker);
  const turns = summary?.turns ?? [{ speaker: speaker ? speakerName(speaker, character) : '', audience: speech.audience || metaAudience, text: body }];
  const partner = summary?.partner ?? (String(item.metadata?.channel ?? '') === 'cross_character'
    ? speakerName(metaSpeaker.toLowerCase() === character ? metaAudience : metaSpeaker, character) : '');
  return { item, body, turns: turns.filter((turn) => turn.text), partner, isDialogueSummary: !!summary };
};

const near = (a: MemoryRecord, b: MemoryRecord) => Math.abs(seconds(a.created_at) - seconds(b.created_at)) <= 180;

export const buildRecentThreads = (items: MemoryRecord[], character: Character): RecentThread[] => {
  const prepared = items.map((item) => prepareMemory(item, character)).filter((item): item is PreparedMemory => !!item && item.turns.length > 0);
  const summaries = prepared.filter((entry) => entry.isDialogueSummary);
  const curated = prepared.filter((entry) => {
    if (entry.isDialogueSummary || entry.item.metadata?.channel !== 'cross_character') return true;
    const text = normalized(entry.body);
    return !summaries.some((summary) => near(entry.item, summary.item)
      && summary.turns.some((turn) => normalized(turn.text) === text));
  });
  curated.sort((a, b) => seconds(a.item.created_at) - seconds(b.item.created_at));

  const threads: Array<RecentThread & { key: string; lastTime: number }> = [];
  const sessions = new Map<string, typeof threads[number]>();
  for (const entry of curated) {
    const channel = String(entry.item.metadata?.channel ?? 'direct');
    const session = entry.item.metadata?.conversation_id || entry.item.metadata?.session_id;
    const sessionId = typeof session === 'string' ? session.trim() : '';
    // direct/wechat/proactive are entry points into the same user conversation.
    const key = sessionId ? `session:${sessionId}` : entry.partner ? `cross:${entry.partner}` : 'user';
    const time = seconds(entry.item.created_at);
    const last = sessionId ? sessions.get(sessionId) : threads[threads.length - 1];
    const canJoin = !!last && last.key === key && (sessionId || time - last.lastTime <= 1800)
      && (sessionId || entry.turns.every((turn) => turn.speaker));
    const title = entry.partner ? `与 ${entry.partner} 的对话` : channel === 'wechat' ? '微信对话' : '对话记录';
    if (canJoin) {
      last.items.push(entry.item);
      last.lastTime = time;
      last.time = Math.max(last.time, entry.item.created_at);
      for (const turn of entry.turns) {
        const signature = normalized(turn.text);
        // Only collapse duplicated summary evidence; repeated real utterances are legitimate.
        if (!entry.isDialogueSummary || signature.length < 6 || !last.turns.some((existing) => existing.speaker === turn.speaker && normalized(existing.text) === signature)) last.turns.push(turn);
      }
      last.searchText = last.turns.map((turn) => turn.text).join(' ');
    } else {
      threads.push({ id: entry.item.id, title, time: entry.item.created_at, turns: [...entry.turns], items: [entry.item], searchText: entry.turns.map((turn) => turn.text).join(' '), key, lastTime: time });
      if (sessionId) sessions.set(sessionId, threads[threads.length - 1]);
    }
  }
  return threads.sort((a, b) => seconds(b.time) - seconds(a.time));
};
