import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { BookOpen, Clock3, Heart, MessageCircle, RefreshCw, Search, UserCircle } from 'lucide-react';
import UserProfilePage from './UserProfilePage';
import { buildRecentThreads, isDialogueMemory, isMemorySummary, prepareMemory, speechLabel, splitSpeechPrefix, type Character, type MemoryRecord } from './memoryPresentation';
import './MemoryPage.css';

type Layer = 'facts' | 'episodes' | 'recent' | 'profile';

const HighlightedText: React.FC<{ text: string; query: string }> = ({ text, query }) => {
  const term = query.trim();
  if (!term) return <>{text}</>;
  const escaped = term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const parts = text.split(new RegExp(`(${escaped})`, 'gi'));
  return <>{parts.map((part, index) => index % 2 === 1
    ? <mark className="memory-search-highlight" key={index}>{part}</mark>
    : <React.Fragment key={index}>{part}</React.Fragment>)}</>;
};

const TABS = [
  { key: 'facts', title: '长期记忆', subtitle: '事实、偏好和约定', icon: Heart },
  { key: 'episodes', title: '共同经历', subtitle: '整理后的对话脉络', icon: BookOpen },
  { key: 'recent', title: '对话记录', subtitle: '每次交流的原始发言', icon: MessageCircle },
  { key: 'profile', title: '用户画像', subtitle: '关于你的了解', icon: UserCircle },
] as const;

const layerOf = (item: MemoryRecord): Layer | null => {
  if (['system_seed', 'environment_preset'].includes(String(item.metadata?.source ?? ''))) return null;
  if (isDialogueMemory(item)) return 'recent';
  if (item.consolidated) return null;
  if (item.memory_type === 'long_term' || item.memory_type === 'important_event') return 'facts';
  if (isMemorySummary(item)) return 'episodes';
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
  if (item.memory_type === 'important_event') return '重要事件';
  if (item.tags.includes('preference')) return '偏好';
  if (item.tags.includes('relationship')) return '关系';
  if (item.tags.includes('user_profile')) return '关于你';
  if (item.tags.includes('project_context')) return '共同话题';
  if (isMemorySummary(item)) return '对话整理';
  return '对话片段';
};

const MemoryPage: React.FC<{ initialLayer?: Layer }> = ({ initialLayer = 'facts' }) => {
  const [character, setCharacter] = useState<Character>('vivian');
  const [layer, setLayer] = useState<Layer>(initialLayer);
  useEffect(() => { setLayer(initialLayer); }, [initialLayer]);
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<MemoryRecord[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const requestId = useRef(0);

  const load = useCallback(async () => {
    const request = ++requestId.current;
    setLoading(true);
    setError('');
    try {
      const result = await invoke<MemoryRecord[]>('get_memories', { characterId: character });
      if (request === requestId.current) setItems(result ?? []);
    } catch (e) {
      if (request === requestId.current) setError(String(e));
    } finally {
      if (request === requestId.current) setLoading(false);
    }
  }, [character]);

  useEffect(() => { void load(); }, [load]);
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void listen<{ character_id?: string | null }>('memory:updated', ({ payload }) => {
      if (!payload?.character_id || payload.character_id === character) void load();
    }).then((cleanup) => {
      if (cancelled) cleanup(); else unlisten = cleanup;
    }).catch(() => {});
    return () => { cancelled = true; unlisten?.(); };
  }, [character, load]);

  const recentThreads = useMemo(() => buildRecentThreads(items.filter((item) => layerOf(item) === 'recent'), character), [items, character]);
  const counts = useMemo(() => items.reduce((acc, item) => {
    const key = layerOf(item);
    if (key && key !== 'recent' && prepareMemory(item, character)) acc[key]++;
    return acc;
  }, { facts: 0, episodes: 0, recent: recentThreads.length, profile: 0 }), [items, character, recentThreads]);

  const visible = useMemo(() => items
    .filter((item) => layerOf(item) === layer && prepareMemory(item, character)?.body.toLowerCase().includes(query.trim().toLowerCase()))
    .sort((a, b) => b.created_at - a.created_at), [items, layer, query, character]);
  const visibleThreads = useMemo(() => recentThreads.filter((thread) => thread.searchText.toLowerCase().includes(query.trim().toLowerCase())), [recentThreads, query]);

  const activeTab = TABS.find((tab) => tab.key === layer)!;

  return <section className="memory-page">
    <header className="memory-page-header">
      <div>
        <p className="memory-eyebrow">MEMORY ARCHIVE · {character.toUpperCase()}</p>
        <h2>记得的事<span className="memory-header-star">✦</span></h2>
        <p className="memory-page-intro">从留下的事实、共同经历，到她主动说起的话。</p>
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
    {layer === 'recent' && <p className="memory-list-note">每张卡片记录一次会话，包含连续发言和多轮交流；之后会逐渐整理成共同经历。</p>}
    {error && <p className="memory-error" role="alert">{error}</p>}
    {loading ? <div className="memory-empty">正在读取记忆…</div> : (layer === 'recent' ? visibleThreads.length === 0 : visible.length === 0) ? <div className="memory-empty"><Clock3 size={24} /><strong>{query ? '没有找到匹配的记忆' : `还没有${activeTab.title}的记录`}</strong><span>{query ? '试试其他关键词' : '有新的交流时，这里会慢慢丰富起来'}</span></div> : layer === 'recent' ? <div className="memory-thread-list">
      {visibleThreads.map((thread) => <article className="memory-thread" key={thread.id}>
        <div className="memory-thread-header"><span className="memory-thread-title"><MessageCircle size={15} />{thread.title}</span><span className="memory-thread-count">{thread.turns.length} 条消息</span><time>{dateText(thread.time)}</time></div>
        <div className="memory-thread-turns">{thread.turns.map((turn, index) => <div className={`memory-thread-turn${turn.speaker ? '' : ' memory-thread-turn-plain'}`} key={`${thread.id}-${index}`}>
          {turn.speaker && <span className="memory-thread-speaker">{turn.speaker}</span>}
          <p><HighlightedText text={turn.text} query={query} /></p>
        </div>)}</div>
      </article>)}
    </div> : <div className="memory-entry-grid">
      {visible.map((item) => {
        const rawQuote = typeof item.metadata?.source_quote === 'string' ? item.metadata.source_quote : '';
        const quote = splitSpeechPrefix(rawQuote).body;
        const hooks = (item.open_hooks ?? []).filter((hook) => hook.closed_at == null);
        const speech = splitSpeechPrefix(item.content);
        return <article key={item.id} className={`memory-entry memory-entry-${layer}`}>
          <div className="memory-entry-top"><span className="memory-entry-kind">{categoryName(item)}</span><time>{dateText(item.created_at)}</time></div>
          {speech.speaker && <span className="memory-entry-speaker">{speechLabel(speech.speaker, speech.audience, character)}</span>}
          <p className="memory-entry-content"><HighlightedText text={speech.body} query={query} /></p>
          {quote && quote !== speech.body && <div className="memory-entry-evidence"><span>来自原话</span>「<HighlightedText text={quote} query={query} />」</div>}
          {hooks.map((hook, index) => <div className="memory-entry-hook" key={index}><span>待跟进</span><HighlightedText text={hook.condition} query={query} /></div>)}
        </article>;
      })}
    </div>}</>}
  </section>;
};

export default MemoryPage;
