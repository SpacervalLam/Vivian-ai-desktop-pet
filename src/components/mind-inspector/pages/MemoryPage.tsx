import { splitActionText } from '../../../utils/ActionText';
import { useTranslation } from 'react-i18next';
import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { BookOpen, Clock3, Heart, MessageCircle, RefreshCw, Search, UserCircle, Sparkles, ArrowUpRight } from 'lucide-react';
import UserProfilePage from './UserProfilePage';
import StickerImage from '../../stickers/StickerImage';
import { conversationThreads, sharedEventTimeline, isVisibleMemoryFact, isMemorySummary, type Character, type MemoryRecord, type ConversationRecord } from './memoryPresentation';
import './MemoryPage.css';
import './ClaudeTheme.css';
import { ComicChapterHeading, ComicChapterInset } from './ComicChapter';
import ComicConversation from './ComicConversation';
import { comicKeyForText } from '../../../i18n/comic';

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
  return isVisibleMemoryFact(item) ? 'facts' : null;
};

const memoryDateText = (value: number, language: string) => {
  const millis = value < 1e12 ? value * 1000 : value;
  return Number.isFinite(millis) && millis > 0
    ? new Date(millis).toLocaleString(language, { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })
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

const MemoryPage: React.FC<{ initialLayer?: Layer; embedded?: boolean; latestOnly?: boolean; active?: boolean; refreshToken?: number; onLoadingChange?: (loading: boolean) => void }> = ({ initialLayer = 'facts', embedded = false, latestOnly = false, active = true, refreshToken = 0, onLoadingChange }) => {
  const { t, i18n } = useTranslation();
  const localize = (value: string) => { const key = comicKeyForText(value); return key ? t(`comic.${key}`) : value; };
  const displaySpeaker = (value: string) => value === '用户' ? t('comic.user') : value === '大家' ? t('comic.everyone') : value;
  const dateText = (value: number) => localize(memoryDateText(value, i18n.resolvedLanguage ?? i18n.language));
  const character: Character = 'vivian';
  const [layer, setLayer] = useState<Layer>(initialLayer);
  useEffect(() => { setLayer(initialLayer); }, [initialLayer]);
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<MemoryRecord[]>([]);
  const [conversations, setConversations] = useState<ConversationRecord[]>([]);
  const [otherConversations, setOtherConversations] = useState<ConversationRecord[]>([]);
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

  const load = useCallback(async () => {
    const request = ++requestId.current;
    setLoading(true);
    setError('');
    try {
      const results = await Promise.all((['vivian', 'nana'] as const).map(async (owner) => {
        const [memories, records] = await Promise.all([
          invoke<MemoryRecord[]>('get_memories', { characterId: owner }),
          invoke<ConversationRecord[]>('get_memory_conversations', { characterId: owner }),
        ]);
        const conversationKey = (id: unknown) => id ? `${owner}:${id}` : '';
        return {
          items: (memories ?? []).map(item => ({ ...item, id: `${owner}:${item.id}`, metadata: {
            ...item.metadata, conversation_id: conversationKey(item.metadata?.conversation_id),
            evidence_sources: Array.isArray(item.metadata?.evidence_sources)
              ? item.metadata.evidence_sources.map(source => ({ ...source, conversation_id: conversationKey(source.conversation_id) })) : [],
          } })),
          records: (records ?? []).map(record => ({ ...record, id: conversationKey(record.id) })),
        };
      }));
      if (request === requestId.current) {
        setItems(results.flatMap(result => result.items));
        setConversations(results[0].records);
        setOtherConversations(results[1].records);
      }
    } catch (e) {
      if (request === requestId.current) setError(String(e));
    } finally {
      if (request === requestId.current) setLoading(false);
    }
  }, []);

  useEffect(() => { void load(); }, [load, refreshToken]);
  useEffect(() => { onLoadingChange?.(loading); }, [loading, onLoadingChange]);
  useEffect(() => {
    let cancelled = false;
    const unlisten: Array<() => void> = [];
    let timer: ReturnType<typeof setTimeout> | undefined;
    for (const event of ['memory:updated', 'dialogue:changed', 'chat:history-cleared']) {
      void listen<{ character_id?: string | null }>(event, ({ payload }) => {
        if (!payload?.character_id || ['vivian', 'nana'].includes(payload.character_id)) {
          clearTimeout(timer); timer = setTimeout(() => void load(), 350);
        }
      }).then((cleanup) => { if (cancelled) cleanup(); else unlisten.push(cleanup); }).catch(() => {});
    }
    return () => { cancelled = true; clearTimeout(timer); unlisten.forEach((cleanup) => cleanup()); };
  }, [load]);

  const recentThreads = useMemo(() => [
    ...conversationThreads(conversations, 'vivian', i18n.resolvedLanguage ?? i18n.language).map(thread => ({ ...thread, owner: 'vivian' as const })),
    ...conversationThreads(otherConversations, 'nana', i18n.resolvedLanguage ?? i18n.language).map(thread => ({ ...thread, owner: 'nana' as const })),
  ].sort((a, b) => b.time - a.time), [conversations, otherConversations, i18n.resolvedLanguage]);
  const latestThread = recentThreads[0];
  const events = useMemo(() => sharedEventTimeline(items, [...conversations, ...otherConversations]), [items, conversations, otherConversations]);
  const summaries = useMemo(() => new Map(items.filter((item) => isMemorySummary(item) && !item.consolidated && item.metadata?.index_active !== false).map((item) => [String(item.metadata?.conversation_id ?? ''), item])), [items]);
  const showOriginal = (id: string) => {
    setExpanded((value) => new Set(value).add(id)); setFocused(id); setQuery(''); setLayer('recent');
  };
  useEffect(() => {
    if (layer !== 'recent' || !focused || loading) return;
    const frame = requestAnimationFrame(() => document.getElementById(`memory-conversation-${encodeURIComponent(focused)}`)?.scrollIntoView({ behavior: 'smooth', block: 'center' }));
    return () => cancelAnimationFrame(frame);
  }, [focused, layer, loading]);
  const organize = async (id: string, owner: Character) => {
    setOrganizing(`${owner}:${id}`); setNotice(''); setError('');
    try {
      const summary = await invoke<MemoryRecord>('summarize_memory_conversation', { characterId: owner, conversationId: id.startsWith(`${owner}:`) ? id.slice(owner.length + 1) : id });
      setNotice(summary.metadata?.summary_status === 'no_content' ? t('comic.memoryNoSummary')
        : summary.metadata?.summary_status === 'partial' ? t('comic.memoryPartial') : t('comic.memoryUpdated'));
      await load();
    } catch (e) { setError(String(e)); }
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

  // The compact manga spread is always the latest original exchange, regardless
  // of the saved history filters. Expansion returns to those same filters.
  return <section className={`memory-page claude-theme claude-page-memory comic-chapter comic-memory${latestOnly ? ' is-latest-conversation' : ''}`}>
    {embedded && <div className="comic-latest-view" hidden={!latestOnly}>
      <ComicConversation thread={latestThread} active={active && latestOnly} loading={loading} error={error}
        renderText={text => <DialogueHighlightedText text={text} query="" />} />
    </div>}
    <div className="memory-history-view" hidden={latestOnly}>
    {embedded && <div className="comic-record-stamp">{t('comic.historicalRecord')}</div>}
    {!embedded && <ComicChapterHeading number="01" title={t('comic.memoryHeading')} caption={t('comic.memoryCaption')}
      image="train-straps-v20" position="50% 50%">
      <div className="memory-page-actions">
        <button className="memory-refresh" type="button" onClick={() => void load()} title={t('comic.refresh')} aria-label={t('comic.refresh')}><RefreshCw size={17} className={loading ? 'spinning' : ''} /></button>
      </div>
    </ComicChapterHeading>}
    <p className="memory-page-intro">{t('comic.memoryIntro')}</p>

    <nav className="memory-layer-grid" aria-label={t('comic.memoryCategories')}>
      {TABS.map((tab) => <button key={tab.key} type="button" className={`memory-layer-card ${layer === tab.key ? 'active' : ''}`} aria-pressed={layer === tab.key} onClick={() => setLayer(tab.key)}>
        <span className="memory-layer-icon"><tab.icon size={19} strokeWidth={1.7} /></span>
        <span className="memory-layer-copy"><strong>{localize(tab.title)}</strong><small>{localize(tab.subtitle)}</small></span>
        <span className="memory-layer-count">{tab.key === 'profile' ? '↗' : counts[tab.key]}</span>
      </button>)}
    </nav>

    {layer === 'profile' ? <div className="memory-profile-panel">{(['vivian', 'nana'] as const).map(owner => <div key={owner}><h3>{t('comic.memoryProfileUnderstanding', { name: owner === 'vivian' ? 'Vivian' : 'Nana' })}</h3><UserProfilePage characterId={owner} embedded /></div>)}</div> : <><div className="memory-list-toolbar">
      <div className="memory-list-heading"><activeTab.icon size={17} /><strong>{localize(activeTab.title)}</strong><span>{counts[layer]} {layer === 'recent' ? t('comic.memoryGroup') : t('comic.memoryItem')}</span></div>
      <label className="memory-search"><Search size={16} /><input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t('comic.memorySearchPlaceholder')} aria-label={t('comic.memorySearch')} /></label>
    </div>
    {layer === 'recent' && <p className="memory-list-note">{t('comic.memoryRecentNote')}</p>}
    {layer === 'episodes' && <p className="memory-list-note">{t('comic.memoryEventNote')}</p>}
    {error && <p className="memory-error" role="alert">{error}</p>}
    {notice && <p className="memory-list-note" role="status">{notice}</p>}
    {loading ? <div className="memory-empty">{t('comic.memoryLoading')}</div> : (layer === 'recent' ? visibleThreads.length === 0 : layer === 'episodes' ? visibleEvents.length === 0 : visible.length === 0) ? <div className="memory-empty"><Clock3 size={24} /><strong>{query ? t('comic.memoryNoMatch') : t('comic.memoryEmpty', { name: localize(activeTab.title) })}</strong><span>{query ? t('comic.memoryTryKeywords') : layer === 'episodes' ? t('comic.memoryEmptyEvents') : t('comic.memoryEmptyHint')}</span></div> : layer === 'episodes' ? <div className="memory-event-list">
      {visibleEvents.map((event) => <article key={event.id} className="memory-event-card">
        <div className="comic-record-stamp"><span>{t('comic.sharedExperience')}</span>{event.time > 0 && <time>{dateText(event.time)}</time>}</div>
        <div className="memory-thread-header"><span className="memory-thread-title"><BookOpen size={15} /><SearchHighlightedText text={event.title} query={query} /></span><span className="memory-thread-count">{t('comic.memoryEventCounts', { conversations: event.conversationIds.length, progress: event.progress.length })}</span></div>
        <ol className="memory-event-progress">{event.progress.map((progress) => <li key={progress.source_message_id}>
          <div className="memory-entry-top"><span className={`memory-event-phase phase-${progress.phase}`}>{({ planned: t('comic.memoryPlanned'), started: t('comic.memoryStarted'), progressed: t('comic.memoryProgress'), completed: t('comic.memoryComplete'), cancelled: t('comic.memoryCancel') })[progress.phase]}</span><time>{t('comic.memoryRecorded')}{dateText(progress.recorded_at)}</time></div>
          <p className="memory-entry-content"><SearchHighlightedText text={progress.detail} query={query} /></p>
          <details className="memory-entry-evidence"><summary>{t('comic.memoryEvidence')}</summary><p>「<SearchHighlightedText text={progress.source_quote} query={query} />」</p><button className="memory-evidence-link" type="button" onClick={() => showOriginal(progress.conversationId)}>{t('comic.memorySource')}<ArrowUpRight size={13} /></button></details>
        </li>)}</ol>
      </article>)}
    </div> : layer === 'recent' ? <div className="memory-thread-list">
      {visibleThreads.map((thread) => <article className={`memory-thread${focused === thread.id ? ' memory-thread-focused' : ''}`} id={`memory-conversation-${encodeURIComponent(thread.id)}`} key={thread.id}>
        <div className="memory-thread-header"><span className="memory-thread-title"><MessageCircle size={15} />{thread.title}</span><span className="memory-thread-count">{t('comic.memoryMessages', { n: thread.turns.length })}</span><time>{dateText(thread.startedAt ?? thread.time)}{thread.startedAt !== thread.time && thread.startedAt ? ` — ${dateText(thread.time)}` : ''}</time></div>
        {cardViews.has(`${thread.owner}:thread:${thread.id}`) && summaries.get(thread.id)?.content ? <p className="memory-entry-content"><SearchHighlightedText text={summaries.get(thread.id)!.content} query={query} /></p> : <div className="memory-thread-turns">{(expanded.has(thread.id) || query.trim() ? thread.turns : thread.turns.slice(0, 6)).map((turn, index) => <div className={`memory-thread-turn${turn.speaker ? '' : ' memory-thread-turn-plain'}`} key={turn.id ?? `${thread.id}-${index}`}>
          {turn.speaker && <span className="memory-thread-speaker" title={turn.audience ? t('comic.audience', { name: displaySpeaker(turn.audience) }) : undefined}>{displaySpeaker(turn.speaker)}</span>}
          <div><p><DialogueHighlightedText text={turn.text} query={query} /></p>{turn.sticker && <StickerImage sticker={turn.sticker} size={96} />}</div>
        </div>)}</div>}
        <div className="memory-session-actions">
          {!cardViews.has(`${thread.owner}:thread:${thread.id}`) && thread.turns.length > 6 && !query.trim() && <button type="button" onClick={() => setExpanded((value) => {
            const next = new Set(value); if (next.has(thread.id)) next.delete(thread.id); else next.add(thread.id); return next;
          })}>{expanded.has(thread.id) ? t('comic.memoryCollapse') : t('comic.memoryExpandMessages', { n: thread.turns.length })}</button>}
          {thread.canonical && <button type="button" disabled={!!organizing} onClick={() => void organize(thread.id, thread.owner)}><Sparkles size={14} />{organizing === `${thread.owner}:${thread.id}` ? t('comic.memoryOrganizing') : summaries.get(thread.id)?.metadata?.summary_status === 'partial' ? t('comic.memoryContinue') : summaries.has(thread.id) ? t('comic.memoryUpdateSummary') : t('comic.memoryOrganize')}</button>}
          {summaries.get(thread.id)?.content && <button type="button" onClick={() => toggleCardView(`${thread.owner}:thread:${thread.id}`)}>{cardViews.has(`${thread.owner}:thread:${thread.id}`) ? t('comic.memoryOriginal') : t('comic.memoryViewSummary')}<ArrowUpRight size={13} /></button>}
        </div>
      </article>)}
    </div> : <div className="memory-entry-grid">
      {visible.map((item, index) => {
        const rawQuote = typeof item.metadata?.source_quote === 'string' ? item.metadata.source_quote : '';
        const quote = rawQuote;
        const hooks = (item.open_hooks ?? []).filter((hook) => hook.closed_at == null);
        const body = item.content;
        const conversationId = String(item.metadata?.conversation_id ?? '');
        const hasOriginal = recentThreads.some((thread) => thread.id === conversationId && thread.canonical);
        const topics = Array.isArray(item.metadata?.topics) ? item.metadata.topics.filter((topic): topic is string => typeof topic === 'string').slice(0, 5) : [];
        const evidence = Array.isArray(item.metadata?.evidence_sources) ? item.metadata.evidence_sources.filter((source): source is { quote: string; conversation_id?: string } =>
          typeof source === 'object' && source !== null && typeof source.quote === 'string' && source.quote !== rawQuote) : [];
        return <article key={item.id} className={`memory-entry memory-entry-${layer}${index === 0 ? ' memory-opening-panel' : ''}`}>
          {index === 0 && !embedded && <ComicChapterInset image={character === 'vivian' ? 'vivian-seat-selected-v24' : 'nana-nai-face-adjusted-v19'}
            caption={t('comic.memoryKept')} position={character === 'vivian' ? '50% 30%' : '50% 35%'} />}
          <div className="memory-narration-body">
          <div className="memory-entry-top"><span className="memory-entry-kind">{localize(categoryName(item))}</span><time>{dateText(typeof item.metadata?.started_at === 'number' ? item.metadata.started_at : item.created_at)}</time></div>
          {typeof item.metadata?.title === 'string' && <h3 className="memory-session-title">{item.metadata.title}</h3>}
          <p className="memory-entry-content"><SearchHighlightedText text={body} query={query} /></p>
          {topics.length > 0 && <div className="memory-topic-tags">{topics.map((topic) => <span key={topic}>{topicName(topic) === topic ? topic : localize(topicName(topic))}</span>)}</div>}
          {quote && quote !== body && <div className="memory-entry-evidence"><span>{t('comic.memoryQuote')}</span>「<SearchHighlightedText text={quote} query={query} />」</div>}
          {hooks.map((hook, index) => <div className="memory-entry-hook" key={index}><span>{t('comic.memoryFollowUp')}</span><SearchHighlightedText text={hook.condition} query={query} /></div>)}
          {evidence.length > 0 && <details className="memory-entry-evidence"><summary>{t('comic.memoryMoreEvidence', { n: evidence.length })}</summary>{evidence.map((source, index) => <p key={index}>「<SearchHighlightedText text={source.quote} query={query} />」{recentThreads.some((thread) => thread.id === source.conversation_id && thread.canonical) && <button className="memory-evidence-link" type="button" onClick={() => showOriginal(source.conversation_id!)}>{t('comic.memoryView')}</button>}</p>)}</details>}
          <div className="memory-session-actions">
            {hasOriginal ? <button type="button" onClick={() => showOriginal(conversationId)}>{t('comic.memoryOriginal')}<ArrowUpRight size={13} /></button>
              : <span>{quote ? t('comic.memoryExtracted') : t('comic.memoryNoEvidence')}</span>}
            {item.metadata?.summary_status === 'partial' && <span>{t('comic.memoryPartialCount', { done: String(item.metadata.completed_parts), total: String(item.metadata.total_parts) })}</span>}
            {isMemorySummary(item) && Array.isArray(item.metadata?.source_message_ids) && <span>{t('comic.memoryCoverage', { n: item.metadata.source_message_ids.length })}</span>}
            {item.metadata?.important_event === true && <span>{t('comic.memoryImportant')}</span>}
          </div>
          </div>
        </article>;
      })}
    </div>}</>}
    </div>
  </section>;
};

export default MemoryPage;
