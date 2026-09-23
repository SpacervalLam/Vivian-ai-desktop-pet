import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { BookOpen, Clock3, Heart, MessageCircle, RefreshCw, Search, Sparkles, Trash2 } from 'lucide-react';
import './MemoryPage.css';

type Layer = 'facts' | 'episodes' | 'outreach' | 'recent';
type Character = 'vivian' | 'nana';
type MemoryRecord = {
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

const TABS = [
  { key: 'facts', title: '长期记忆', subtitle: '事实、偏好和约定', icon: Heart },
  { key: 'episodes', title: '共同经历', subtitle: '整理后的对话脉络', icon: BookOpen },
  { key: 'outreach', title: '主动问候', subtitle: '她主动发起的交流', icon: Sparkles },
  { key: 'recent', title: '近期对话', subtitle: '等待整理的片段', icon: MessageCircle },
] as const;

const isOutreach = (item: MemoryRecord) => item.memory_type === 'casual_conversation'
  && item.metadata?.perspective !== 'observer'
  && (item.tags.includes('startup_greeting') || item.tags.includes('proactive'));

const layerOf = (item: MemoryRecord): Layer | null => {
  if (item.consolidated || ['system_seed', 'environment_preset'].includes(String(item.metadata?.source ?? ''))) return null;
  if (item.memory_type === 'long_term' || item.memory_type === 'important_event') return 'facts';
  if (item.memory_type === 'session_summary') return 'episodes';
  if (isOutreach(item)) return 'outreach';
  if (item.memory_type === 'short_term' || item.memory_type === 'casual_conversation') return 'recent';
  return null;
};

const dateText = (value: number) => {
  const millis = value < 1e12 ? value * 1000 : value;
  return Number.isFinite(millis) && millis > 0
    ? new Date(millis).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })
    : '时间未知';
};

const displayContent = (content: string) => content.replace(/^\[(?:I say|User says|\w+ says) to everyone\]\s*/i, '').trim();

const categoryName = (item: MemoryRecord) => {
  if (item.tags.includes('startup_greeting')) return '启动问候';
  if (isOutreach(item)) return item.metadata?.channel === 'wechat' ? '主动私聊' : '桌面问候';
  if (item.memory_type === 'important_event') return '重要事件';
  if (item.tags.includes('preference')) return '偏好';
  if (item.tags.includes('relationship')) return '关系';
  if (item.tags.includes('user_profile')) return '关于你';
  if (item.tags.includes('project_context')) return '共同话题';
  if (item.memory_type === 'session_summary') return '对话整理';
  return '对话片段';
};

const MemoryPage: React.FC = () => {
  const [character, setCharacter] = useState<Character>('vivian');
  const [layer, setLayer] = useState<Layer>('facts');
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

  const counts = useMemo(() => items.reduce((acc, item) => {
    const key = layerOf(item);
    if (key) acc[key]++;
    return acc;
  }, { facts: 0, episodes: 0, outreach: 0, recent: 0 }), [items]);

  const visible = useMemo(() => items
    .filter((item) => layerOf(item) === layer && `${item.content} ${item.tags.join(' ')}`.toLowerCase().includes(query.trim().toLowerCase()))
    .sort((a, b) => b.created_at - a.created_at), [items, layer, query]);

  const remove = async (item: MemoryRecord) => {
    if (!window.confirm(`删除这条记忆？\n\n${displayContent(item.content)}`)) return;
    try {
      await invoke('delete_memory', { id: item.id, characterId: character });
      setItems((current) => current.filter((entry) => entry.id !== item.id));
    } catch (e) {
      setError(String(e));
    }
  };

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
        <span className="memory-layer-count">{counts[tab.key]}</span>
      </button>)}
    </nav>

    <div className="memory-list-toolbar">
      <div className="memory-list-heading"><activeTab.icon size={17} /><strong>{activeTab.title}</strong><span>{counts[layer]} 条</span></div>
      <label className="memory-search"><Search size={16} /><input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索这里的记忆" aria-label="搜索记忆" /></label>
    </div>
    {layer === 'recent' && <p className="memory-list-note">近期片段会逐渐整理成共同经历；当前想法只用于此刻，不会永久保存。</p>}
    {layer === 'outreach' && <p className="memory-list-note">包含启动时的问候，以及从桌面或私聊主动发起的交流。</p>}
    {error && <p className="memory-error" role="alert">{error}</p>}
    {loading ? <div className="memory-empty">正在读取记忆…</div> : visible.length === 0 ? <div className="memory-empty"><Clock3 size={24} /><strong>{query ? '没有找到匹配的记忆' : `还没有${activeTab.title}的记录`}</strong><span>{query ? '试试其他关键词' : '有新的交流时，这里会慢慢丰富起来'}</span></div> : <div className="memory-entry-grid">
      {visible.map((item) => {
        const quote = typeof item.metadata?.source_quote === 'string' ? item.metadata.source_quote : '';
        const hooks = (item.open_hooks ?? []).filter((hook) => hook.closed_at == null);
        return <article key={item.id} className={`memory-entry memory-entry-${layer}`}>
          <div className="memory-entry-top"><span className="memory-entry-kind">{categoryName(item)}</span><time>{dateText(item.created_at)}</time><button type="button" className="memory-entry-delete" onClick={() => void remove(item)} title="删除这条记忆" aria-label="删除这条记忆"><Trash2 size={15} /></button></div>
          <p className="memory-entry-content">{displayContent(item.content)}</p>
          {quote && <div className="memory-entry-evidence"><span>来自原话</span>「{quote}」</div>}
          {hooks.map((hook, index) => <div className="memory-entry-hook" key={index}><span>待跟进</span>{hook.condition}</div>)}
        </article>;
      })}
    </div>}
  </section>;
};

export default MemoryPage;
