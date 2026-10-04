import { splitActionText } from '../../../utils/ActionText';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { BookOpen, Clock3, Heart, MessageCircle, RefreshCw, Search, UserCircle, Sparkles, ArrowUpRight } from 'lucide-react';
import UserProfilePage from './UserProfilePage';
import StickerImage from '../../stickers/StickerImage';
import { conversationThreads, sharedEventTimeline, isMemoryFact, isMemorySummary, type Character, type MemoryRecord, type ConversationRecord } from './memoryPresentation';
import './MemoryPage.css';
import './ClaudeTheme.css';

type Layer = 'facts' | 'episodes' | 'recent' | 'profile';

const SearchHighlightedText: React.FC<{ text: string; query: string }> = ({ text, query }) => {
  const term = query.trim();
  if (!term) return <>{text}</>;
  const escaped = term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const parts = text.split(new RegExp(`(${escaped})`, 'gi'));
  return <>{parts.map((part, index) => index % 2 === 1
    ? <mark className="memory-search-highlight" key={index}>{part}</mark>
    : <React.Fragment key={index}>{part}</React.Fragment>)}</>;
};

const DialogueHighlightedText: React.FC<{ text: string; query: string }> = ({ text, query }) => <>{
  splitActionText(text).map((part, index) => part.action
    ? <em className="memory-action-text" key={index}><SearchHighlightedText text={part.text} query={query} /></em>
    : <SearchHighlightedText key={index} text={part.text} query={query} />)
}</>;

const TABS = [
  { key: 'facts', title: '长期记忆', subtitle: '事实、偏好和约定', icon: Heart },
  { key: 'episodes', title: '共同经历', subtitle: '跨会话的事件与进展', icon: BookOpen },
  { key: 'recent', title: '对话记录', subtitle: '每次交流的原始发言', icon: MessageCircle },
  { key: 'profile', title: '用户画像', subtitle: '关于你的了解', icon: UserCircle },
] as const;

const layerOf = (item: MemoryRecord): Layer | null => {
  if (['system_seed', 'environment_preset'].includes(String(item.metadata?.source ?? ''))) return null;
  if (!item.content.trim() || ['subjective', 'observation', 'internal'].includes(String(item.metadata?.record_kind ?? ''))) return null;
  if (item.consolidated) return null;
  if (isMemoryFact(item)) return 'facts';
  if (item.metadata?.perspective === 'observer') return null;
  return null;
};

const dateText = (value: number) => {
  const millis = value < 1e12 ? value * 1000 : value;
  return Number.isFinite(millis) && millis > 0
    ? new Date(millis).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })
    : '时间未知';
};

const categoryName = (item: MemoryRecord) => {
  if (item.open_hooks?.some((hook) => hook.closed_at == null)) return '约定与待办';
  if (item.tags.includes('preference')) return '偏好';
  if (item.tags.includes('relationship')) return '关系';
  if (item.tags.includes('user_profile')) return '关于你';
  if (item.tags.includes('project_context')) return '共同话题';
  if (isMemorySummary(item)) return '会话摘要';
  return '事实与资料';
};
const topicName = (topic: string) => ({ preference: '偏好', identity: '身份', user_profile: '关于你',
  project_context: '项目', relationship: '关系', health: '健康', reference: '参考', knowledge: '知识' }[topic] ?? topic);

const MemoryPage: React.FC<{ initialLayer?: Layer }> = ({ initialLayer = 'facts' }) => {
  const [character, setCharacter] = useState<Character>('vivian');
  const [layer, setLayer] = useState<Layer>(initialLayer);
  useEffect(() => { setLayer(initialLayer); }, [initialLayer]);
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<MemoryRecord[]>([]);
  const [conversations, setConversations] = useState<ConversationRecord[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [focused, setFocused] = useState('');
  const [organizing, setOrganizing] = useState('');
  const [notice, setNotice] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [cardViews, setCardViews] = useState<Set<string>>(new Set());
  const toggleCardView = (id: string) => setCardViews((value) => {
    const next = new Set(value);
    if (next.has(id)) next.delete(id); else next.add(id);
    return next;
  });
  const requestId = useRef(0);
  const ownerRef = useRef(character);
  ownerRef.current = character;

  const load = useCallback(async () => {
    if (ownerRef.current !== character) return;
    const request = ++requestId.current;
    setLoading(true);
    setError('');
    try {
      const [result, records] = await Promise.all([
        invoke<MemoryRecord[]>('get_memories', { characterId: character }),
        invoke<ConversationRecord[]>('get_memory_conversations', { characterId: character }),
      ]);
      if (request === requestId.current) { setItems(result ?? []); setConversations(records ?? []); }
    } catch (e) {
      if (request === requestId.current) setError(String(e));
    } finally {
      if (request === requestId.current) setLoading(false);
    }
  }, [character]);

  useEffect(() => { void load(); }, [load]);
  useEffect(() => {
    let cancelled = false;
    const unlisten: Array<() => void> = [];
    let timer: ReturnType<typeof setTimeout> | undefined;
    for (const event of ['memory:updated', 'dialogue:changed', 'chat:history-cleared']) {
      void listen<{ character_id?: string | null }>(event, ({ payload }) => {
        if (!payload?.character_id || payload.character_id === character) {
          clearTimeout(timer); timer = setTimeout(() => void load(), 350);
        }
      }).then((cleanup) => { if (cancelled) cleanup(); else unlisten.push(cleanup); }).catch(() => {});
    }
    return () => { cancelled = true; clearTimeout(timer); unlisten.forEach((cleanup) => cleanup()); };
  }, [character, load]);

  const recentThreads = useMemo(() => conversationThreads(conversations, character), [conversations, character]);
  const events = useMemo(() => sharedEventTimeline(items, conversations), [items, conversations]);
  const summaries = useMemo(() => new Map(items.filter((item) => isMemorySummary(item) && !item.consolidated && item.metadata?.index_active !== false).map((item) => [String(item.metadata?.conversation_id ?? ''), item])), [items]);
  const showOriginal = (id: string) => {
    setExpanded((value) => new Set(value).add(id)); setFocused(id); setQuery(''); setLayer('recent');
  };
  useEffect(() => {
    if (layer !== 'recent' || !focused || loading) return;
    const frame = requestAnimationFrame(() => document.getElementById(`memory-conversation-${encodeURIComponent(focused)}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' }));
    return () => cancelAnimationFrame(frame);
  }, [focused, layer, loading]);
  const organize = async (id: string) => {
    const owner = character;
    setOrganizing(`${owner}:${id}`); setNotice(''); setError('');
    try {
      const summary = await invoke<MemoryRecord>('summarize_memory_conversation', { characterId: owner, conversationId: id });
      if (ownerRef.current !== owner) return;
      setNotice(summary.metadata?.summary_status === 'no_content' ? '这次交流无需额外摘要，原始消息已保留。'
        : summary.metadata?.summary_status === 'partial' ? '已保存阶段摘要，剩余内容可继续整理。' : '会话摘要已更新，有依据的事件进展会进入共同经历。');
      await load();
    } catch (e) { if (ownerRef.current === owner) setError(String(e)); }
    finally { setOrganizing(''); }
  };
  const counts = useMemo(() => items.reduce((acc, item) => {
    const key = layerOf(item);
    if (key && key !== 'recent' && item.content.trim()) acc[key]++;
    return acc;
  }, { facts: 0, episodes: events.length, recent: recentThreads.length, profile: 0 }), [items, character, recentThreads, events]);

  const visible = useMemo(() => items
    .filter((item) => layerOf(item) === layer && [item.content,
      item.metadata?.title ?? '', item.metadata?.source_quote ?? '',
      ...(Array.isArray(item.metadata?.topics) ? item.metadata.topics : []),
      ...(item.open_hooks ?? []).map((hook) => hook.condition)].join(' ').toLowerCase().includes(query.trim().toLowerCase()))
    .sort((a, b) => Number(b.metadata?.ended_at ?? b.created_at) - Number(a.metadata?.ended_at ?? a.created_at)), [items, layer, query, character]);
  const visibleThreads = useMemo(() => recentThreads.filter((thread) => thread.searchText.toLowerCase().includes(query.trim().toLowerCase())), [recentThreads, query]);
  const visibleEvents = useMemo(() => events.filter((event) => event.searchText.toLowerCase().includes(query.trim().toLowerCase())), [events, query]);

  const activeTab = TABS.find((tab) => tab.key === layer)!;

  return <section className="memory-page claude-theme claude-page-memory">
    <header className="memory-page-header">
      <div>
        <p className="memory-eyebrow"><span className="claude-spark" aria-hidden="true">✻</span>Memory · {character === 'vivian' ? 'Vivian' : 'Nana'}</p>
        <h2>记得的事</h2>
        <p className="memory-page-intro">原始交流、会话脉络，以及有依据的事实与约定。</p>
      </div>
      <div className="memory-page-actions">
        <div className="memory-character-switch" aria-label="选择角色">
          {(['vivian', 'nana'] as const).map((id) => <button key={id} type="button" aria-pressed={character === id} className={character === id ? 'active' : ''} onClick={() => setCharacter(id)}>{id === 'vivian' ? 'Vivian' : 'Nana'}</button>)}
        </div>
        <button className="memory-refresh" type="button" onClick={() => void load()} title="刷新记忆" aria-label="刷新记忆"><RefreshCw size={17} className={loading ? 'spinning' : ''} /></button>
      </div>
    </header>

    <nav className="memory-layer-grid" aria-label="记忆分类">
      {TABS.map((tab) => <button key={tab.key} type="button" className={`memory-layer-card ${layer === tab.key ? 'active' : ''}`} aria-pressed={layer === tab.key} onClick={() => setLayer(tab.key)}>
        <span className="memory-layer-icon"><tab.icon size={19} strokeWidth={1.7} /></span>
        <span className="memory-layer-copy"><strong>{tab.title}</strong><small>{tab.subtitle}</small></span>
        <span className="memory-layer-count">{tab.key === 'profile' ? '↗' : counts[tab.key]}</span>
      </button>)}
    </nav>

    {layer === 'profile' ? <div className="memory-profile-panel"><UserProfilePage characterId={character} embedded /></div> : <><div className="memory-list-toolbar">
      <div className="memory-list-heading"><activeTab.icon size={17} /><strong>{activeTab.title}</strong><span>{counts[layer]} {layer === 'recent' ? '组' : '条'}</span></div>
      <label className="memory-search"><Search size={16} /><input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索这里的记忆" aria-label="搜索记忆" /></label>
    </div>
    {layer === 'recent' && <p className="memory-list-note">每张卡片保留一次交流的原始消息和会话摘要；具体事件的进展单独汇入共同经历。</p>}
    {layer === 'episodes' && <p className="memory-list-note">同一事件在不同会话中的计划、进展与结果串成一条时间线。时间表示原话记录时间，计划不代表已经发生。</p>}
    {error && <p className="memory-error" role="alert">{error}</p>}
    {notice && <p className="memory-list-note" role="status">{notice}</p>}
    {loading ? <div className="memory-empty">正在读取记忆…</div> : (layer === 'recent' ? visibleThreads.length === 0 : layer === 'episodes' ? visibleEvents.length === 0 : visible.length === 0) ? <div className="memory-empty"><Clock3 size={24} /><strong>{query ? '没有找到匹配的记忆' : `还没有${activeTab.title}的记录`}</strong><span>{query ? '试试其他关键词' : layer === 'episodes' ? '整理对话后，有原话依据的事件会出现在这里；普通聊天不会成为事件。' : '有新的交流时，这里会慢慢丰富起来'}</span></div> : layer === 'episodes' ? <div className="memory-event-list">
      {visibleEvents.map((event) => <article key={event.id} className="memory-event-card">
        <div className="memory-thread-header"><span className="memory-thread-title"><BookOpen size={15} /><SearchHighlightedText text={event.title} query={query} /></span><span className="memory-thread-count">{event.conversationIds.length} 次会话 · {event.progress.length} 条进展</span></div>
        <ol className="memory-event-progress">{event.progress.map((progress) => <li key={progress.source_message_id}>
          <div className="memory-entry-top"><span className={`memory-event-phase phase-${progress.phase}`}>{({ planned: '计划与约定', started: '开始', progressed: '进展', completed: '已完成', cancelled: '已取消' })[progress.phase]}</span><time>记录于 {dateText(progress.recorded_at)}</time></div>
          <p className="memory-entry-content"><SearchHighlightedText text={progress.detail} query={query} /></p>
          <details className="memory-entry-evidence"><summary>原话依据</summary><p>「<SearchHighlightedText text={progress.source_quote} query={query} />」</p><button className="memory-evidence-link" type="button" onClick={() => showOriginal(progress.conversationId)}>查看来源会话<ArrowUpRight size={13} /></button></details>
        </li>)}</ol>
      </article>)}
    </div> : layer === 'recent' ? <div className="memory-thread-list">
      {visibleThreads.map((thread) => <article className={`memory-thread${focused === thread.id ? ' memory-thread-focused' : ''}`} id={`memory-conversation-${encodeURIComponent(thread.id)}`} key={thread.id}>
        <div className="memory-thread-header"><span className="memory-thread-title"><MessageCircle size={15} />{thread.title}</span><span className="memory-thread-count">{thread.turns.length} 条消息</span><time>{dateText(thread.startedAt ?? thread.time)}{thread.startedAt !== thread.time && thread.startedAt ? ` — ${dateText(thread.time)}` : ''}</time></div>
        {cardViews.has(`${character}:thread:${thread.id}`) && summaries.get(thread.id)?.content ? <p className="memory-entry-content"><SearchHighlightedText text={summaries.get(thread.id)!.content} query={query} /></p> : <div className="memory-thread-turns">{(expanded.has(thread.id) || query.trim() ? thread.turns : thread.turns.slice(0, 6)).map((turn, index) => <div className={`memory-thread-turn${turn.speaker ? '' : ' memory-thread-turn-plain'}`} key={turn.id ?? `${thread.id}-${index}`}>
          {turn.speaker && <span className="memory-thread-speaker" title={turn.audience ? `对 ${turn.audience} 说` : undefined}>{turn.speaker}</span>}
          <div><p><DialogueHighlightedText text={turn.text} query={query} /></p>{turn.sticker && <StickerImage sticker={turn.sticker} size={96} />}</div>
        </div>)}</div>}
        <div className="memory-session-actions">
          {!cardViews.has(`${character}:thread:${thread.id}`) && thread.turns.length > 6 && !query.trim() && <button type="button" onClick={() => setExpanded((value) => {
            const next = new Set(value); if (next.has(thread.id)) next.delete(thread.id); else next.add(thread.id); return next;
          })}>{expanded.has(thread.id) ? '收起' : `展开全部 ${thread.turns.length} 条消息`}</button>}
          {thread.canonical && <button type="button" disabled={!!organizing} onClick={() => void organize(thread.id)}><Sparkles size={14} />{organizing === `${character}:${thread.id}` ? '正在整理…' : summaries.get(thread.id)?.metadata?.summary_status === 'partial' ? '继续整理' : summaries.has(thread.id) ? '更新摘要' : '整理这次对话'}</button>}
          {summaries.get(thread.id)?.content && <button type="button" onClick={() => toggleCardView(`${character}:thread:${thread.id}`)}>{cardViews.has(`${character}:thread:${thread.id}`) ? '查看原始会话' : '查看会话摘要'}<ArrowUpRight size={13} /></button>}
        </div>
      </article>)}
    </div> : <div className="memory-entry-grid">
      {visible.map((item) => {
        const rawQuote = typeof item.metadata?.source_quote === 'string' ? item.metadata.source_quote : '';
        const quote = rawQuote;
        const hooks = (item.open_hooks ?? []).filter((hook) => hook.closed_at == null);
        const body = item.content;
        const conversationId = String(item.metadata?.conversation_id ?? '');
        const hasOriginal = recentThreads.some((thread) => thread.id === conversationId && thread.canonical);
        const topics = Array.isArray(item.metadata?.topics) ? item.metadata.topics.filter((topic): topic is string => typeof topic === 'string').slice(0, 5) : [];
        const evidence = Array.isArray(item.metadata?.evidence_sources) ? item.metadata.evidence_sources.filter((source): source is { quote: string; conversation_id?: string } =>
          typeof source === 'object' && source !== null && typeof source.quote === 'string' && source.quote !== rawQuote) : [];
        return <article key={item.id} className={`memory-entry memory-entry-${layer}`}>
          <div className="memory-entry-top"><span className="memory-entry-kind">{categoryName(item)}</span><time>{dateText(typeof item.metadata?.started_at === 'number' ? item.metadata.started_at : item.created_at)}</time></div>
          {typeof item.metadata?.title === 'string' && <h3 className="memory-session-title">{item.metadata.title}</h3>}
          <p className="memory-entry-content"><SearchHighlightedText text={body} query={query} /></p>
          {topics.length > 0 && <div className="memory-topic-tags">{topics.map((topic) => <span key={topic}>{topicName(topic)}</span>)}</div>}
          {quote && quote !== body && <div className="memory-entry-evidence"><span>来自原话</span>「<SearchHighlightedText text={quote} query={query} />」</div>}
          {hooks.map((hook, index) => <div className="memory-entry-hook" key={index}><span>待跟进</span><SearchHighlightedText text={hook.condition} query={query} /></div>)}
          {evidence.length > 0 && <details className="memory-entry-evidence"><summary>更多原话证据 · {evidence.length} 条</summary>{evidence.map((source, index) => <p key={index}>「<SearchHighlightedText text={source.quote} query={query} />」{recentThreads.some((thread) => thread.id === source.conversation_id && thread.canonical) && <button className="memory-evidence-link" type="button" onClick={() => showOriginal(source.conversation_id!)}>查看会话</button>}</p>)}</details>}
          <div className="memory-session-actions">
            {hasOriginal ? <button type="button" onClick={() => showOriginal(conversationId)}>查看原始会话<ArrowUpRight size={13} /></button>
              : <span>{quote ? '原话提取' : '未标注证据'}</span>}
            {item.metadata?.summary_status === 'partial' && <span>阶段摘要 · {String(item.metadata.completed_parts)}/{String(item.metadata.total_parts)} 段</span>}
            {isMemorySummary(item) && Array.isArray(item.metadata?.source_message_ids) && <span>覆盖 {item.metadata.source_message_ids.length} 条原消息</span>}
            {item.metadata?.important_event === true && <span>重要事件</span>}
          </div>
        </article>;
      })}
    </div>}</>}
  </section>;
};

export default MemoryPage;
