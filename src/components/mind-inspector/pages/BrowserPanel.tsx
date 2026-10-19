import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent as ReactKeyboardEvent, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { ArrowLeft, ArrowRight, RotateCw, X, Plus, Star, Ellipsis, Globe, Search, Terminal as TerminalIcon, VenetianMask, LoaderCircle, Puzzle, Download, History, ChevronRight, Minus } from 'lucide-react';
import { browserBytes, browserEntries, browserLibraryPage, browserLibraryUrl, browserLibraryPages, type BrowserLibraryPage, emptyBrowser, mergeBrowserState, type BrowserState } from './browserModel';
import BrowserSiteIcon from './BrowserSiteIcon';
import { browserSearchEngines, browserSearchEngine, browserSearchInput, BROWSER_SEARCH_ENGINE_KEY, type BrowserSearchEngine } from './browserModel';
import { browserMenuLayout, browserSubmenuDelay } from './browserMenuLayout';
import { browserViewportClip } from './browserOverlayFrame';
import './browserStrings';
import './BrowserPanel.css';
import { browserScrollbarCss } from './browserScrollbarStyle';

let layoutSequence = Date.now() * 1000;
type Menu = 'menu' | 'history' | 'bookmarks' | 'downloads' | 'extensions' | 'settings' | null;
type Submenu = Exclude<Menu, 'menu' | null>;

export default function BrowserPanel({ visible, fullView, tabHost, onActivate, newPageRequest = 0, onNewTerminal }: { newPageRequest?: number; onNewTerminal?: () => void; tabHost?: HTMLElement | null; onActivate?: () => void; visible: boolean; fullView: boolean; onToggleFullView: () => void }) {
  const { t, i18n } = useTranslation();
  const [state, setState] = useState<BrowserState>(emptyBrowser);
  const [error, setError] = useState('');
  const [searchEngine, setSearchEngine] = useState<BrowserSearchEngine>(() => {
    try { return browserSearchEngine(localStorage.getItem(BROWSER_SEARCH_ENGINE_KEY)); } catch { return 'bing'; }
  });
  useEffect(() => {
    try { localStorage.setItem(BROWSER_SEARCH_ENGINE_KEY, searchEngine); } catch { /* Browsing remains usable without preference storage. */ }
  }, [searchEngine]);
  const [ready, setReady] = useState(false);
  const [address, setAddress] = useState('');
  const [editingAddress, setEditingAddress] = useState(false);
  const [menu, setMenu] = useState<Menu>(null);
  const [submenu, setSubmenu] = useState<Submenu | null>(null);
  const [menuPosition, setMenuPosition] = useState<ReturnType<typeof browserMenuLayout> | null>(null);
  const [menuTheme, setMenuTheme] = useState<CSSProperties>({});
  const [query, setQuery] = useState('');
  const [pageQuery, setPageQuery] = useState('');
  const [bookmarkMenu, setBookmarkMenu] = useState<{ url: string; title: string; x: number; y: number } | null>(null);
  const [renamingBookmark, setRenamingBookmark] = useState<{ url: string; title: string } | null>(null);
  const bookmarkMenuRef = useRef<HTMLDivElement>(null);
  const bookmarkNameRef = useRef<HTMLInputElement>(null);
  const [findOpen, setFindOpen] = useState(false);
  const [find, setFind] = useState('');
  const addressRef = useRef<HTMLInputElement>(null);
  const findRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const menuButtonRef = useRef<HTMLButtonElement>(null);
  const submenuRef = useRef<HTMLDivElement>(null);
  const submenuTriggers = useRef<Partial<Record<Submenu, HTMLButtonElement | null>>>({});
  const hoverTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const focusSubmenu = useRef(false);
  const focusedMenu = useRef<Menu>(null);
  const pointerMenu = useRef(true);
  const viewport = useRef<HTMLDivElement>(null);
  const mounted = useRef(true);
  const currentState = useRef(state);
  currentState.current = state;
  const active = state.tabs.find(tab => tab.id === state.active);
  const internalPage = browserLibraryPage(active?.url);
  const tabTitle = (tab: BrowserState["tabs"][number]) => { const page = browserLibraryPage(tab.url); return page ? t(`embeddedBrowser.${page}`) : tab.start ? t("embeddedBrowser.newTab") : tab.title || tab.url; };
  const send = useCallback(async (action: string, fields: Record<string, unknown> = {}) => {
    if (['select', 'new_tab', 'reopen', 'navigate'].includes(action)) onActivate?.();
    try {
      const next = await invoke<BrowserState>('embedded_browser_request', { request: { action, ...fields } });
      if (mounted.current) { setState(current => mergeBrowserState(current, next)); setReady(true); setError(''); }
      return true;
    } catch (cause) { if (mounted.current) setError(String(cause)); return false; }
  }, [onActivate]);
  const handledNewPage = useRef(0);
  useEffect(() => {
    if (!ready || newPageRequest <= handledNewPage.current) return;
    const pending = newPageRequest - handledNewPage.current;
    handledNewPage.current = newPageRequest;
    for (let index = 0; index < pending; index++) void send('new_tab', { input: '' });
  }, [newPageRequest, ready, send]);
  const act = (name: string, text = '') => { if (active) void send('action', { id: active.id, name, text }); };
  const cancelHover = () => { if (hoverTimer.current !== null) clearTimeout(hoverTimer.current); hoverTimer.current = null; };
  const openSubmenu = (next: Submenu, focus = true) => {
    cancelHover(); if (focus) pointerMenu.current = false; focusSubmenu.current = focus; setQuery(''); setSubmenu(next);
    if (submenu === next && focus) (submenuRef.current?.querySelector<HTMLElement>('input') ?? submenuRef.current?.querySelector<HTMLElement>('button:not(:disabled)'))?.focus();
  };
  useEffect(() => { if (!menu || !visible) { cancelHover(); setSubmenu(null); } if (!visible) setMenu(null); return cancelHover; }, [menu, visible]);
  useLayoutEffect(() => {
    if (!menu) return;
    const update = () => {
      const anchor = menuButtonRef.current?.getBoundingClientRect();
      if (!anchor) return;
      setMenuPosition(browserMenuLayout({ width: innerWidth, height: innerHeight }, anchor, menu === 'menu' && !!submenu, submenu ? submenuTriggers.current[submenu]?.getBoundingClientRect().top : undefined));
      const computed = getComputedStyle(menuButtonRef.current!);
      setMenuTheme({ ...Object.fromEntries(['--codex-ink', '--codex-ink-faint', '--codex-accent', '--codex-paper-card', '--codex-line-light', '--codex-paper', '--panel-scrollbar', '--panel-scrollbar-hover'].map(key => [key, computed.getPropertyValue(key)])), fontFamily: computed.fontFamily } as CSSProperties);
    };
    update(); window.addEventListener('resize', update);
    return () => window.removeEventListener('resize', update);
  }, [menu, submenu, fullView]);
  useLayoutEffect(() => {
    if (submenu && focusSubmenu.current) { (submenuRef.current?.querySelector<HTMLElement>('input') ?? submenuRef.current?.querySelector<HTMLElement>('button:not(:disabled)'))?.focus(); focusSubmenu.current = false; }
  }, [submenu]);
  useEffect(() => {
    if (!menu) { focusedMenu.current = null; return; }
    if (menuPosition && menuRef.current && focusedMenu.current !== menu) {
      focusedMenu.current = menu;
      (menuRef.current.querySelector<HTMLElement>('input') ?? menuRef.current.querySelector<HTMLElement>('button:not(:disabled)'))?.focus();
    }
  }, [menu, menuPosition]);

  useEffect(() => {
    mounted.current = true; let disposed = false; const stops: UnlistenFn[] = [];
    void (async () => {
      const events = await listen<BrowserState>('browser:state', event => { if (!disposed) setState(current => mergeBrowserState(current, event.payload)); });
      if (disposed) { events(); return; } stops.push(events);
      const keys = await listen<{ action: string; id: string }>('browser:shortcut', ({ payload }) => {
        if (disposed) return;
        if (payload.action === 'address') { addressRef.current?.focus(); addressRef.current?.select(); }
        else if (payload.action === 'find') { setFindOpen(true); }
        else if (payload.action === 'new_tab') void send('new_tab', { input: '' });
        else if (payload.action === 'private_tab') void send('new_tab', { input: '', private: true });
        else if (payload.action === 'close_tab') void send('close', { id: payload.id });
        else if (payload.action === 'reopen') void send('reopen');
        else if (payload.action === 'next_tab' || payload.action === 'previous_tab') {
          const { tabs, active } = currentState.current;
          const index = tabs.findIndex(tab => tab.id === active);
          const target = tabs[(index + (payload.action === 'next_tab' ? 1 : tabs.length - 1)) % tabs.length];
          if (target) void send('select', { id: target.id });
        }
      });
      if (disposed) { keys(); return; } stops.push(keys);
      await send('init');
    })().catch(cause => { if (!disposed) setError(String(cause)); });
    return () => { disposed = true; mounted.current = false; stops.forEach(stop => stop()); };
  }, [send]);
  useEffect(() => { if (!editingAddress) setAddress(active?.url === 'about:blank' ? '' : active?.url ?? ''); }, [active?.id, active?.url]);
  useEffect(() => { if (visible && active?.start && !internalPage) { addressRef.current?.focus(); addressRef.current?.select(); } }, [active?.id, active?.start, visible]);
  useEffect(() => { if (findOpen) { findRef.current?.focus(); findRef.current?.select(); } }, [findOpen]);
  useEffect(() => {
    if (!menu) return;
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Element && event.target.closest('[data-browser-menu-trigger]')) return;
      if (event.target instanceof Node && !menuRef.current?.contains(event.target) && !menuButtonRef.current?.contains(event.target)) setMenu(null);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault(); cancelHover();
        const child = submenuRef.current?.dataset.submenu as Submenu | undefined;
        if (menu === 'menu' && child) { setSubmenu(null); submenuTriggers.current[child]?.focus(); }
        else { setMenu(null); menuButtonRef.current?.focus(); }
      }
    };
    document.addEventListener('pointerdown', outside);
    document.addEventListener('keydown', escape);
    return () => { document.removeEventListener('pointerdown', outside); document.removeEventListener('keydown', escape); };
  }, [menu]);

  useEffect(() => {
    if (!ready) return;
    let frame = 0; let last = ''; let disposed = false;
    const hide = () => void invoke('embedded_browser_layout', { bounds: { x: 0, y: 0, width: 0, height: 0 }, visible: false, sequence: ++layoutSequence }).catch(() => {});
    const measure = () => {
      if (disposed) return;
      cancelAnimationFrame(frame);
      const element = viewport.current;
      if (element) {
        const rect = element.getBoundingClientRect();
        const bounds = { x: rect.x, y: rect.y, width: rect.width, height: rect.height };
        const overlays = [...document.querySelectorAll<HTMLElement>('dialog[open],[aria-modal="true"],[role="menu"],[data-browser-overlay="true"],.codex-sidebar.hover-peek')].filter(overlay => {
          if (overlay.contains(element) || overlay.closest('[hidden],[aria-hidden="true"]')) return false;
          const style = getComputedStyle(overlay);
          return style.visibility === 'visible' && style.opacity !== '0';
        }).map(overlay => {
          const rect = overlay.getBoundingClientRect();
          // Reserve the final sidebar width on the first animation frame. The
          // webpage keeps its size and position throughout open/close animation.
          return { x: rect.x, y: rect.y, width: Math.max(rect.width, Number(overlay.dataset.browserOverlayWidth) || 0), height: rect.height };
        }).filter(r => r.width > 0 && r.height > 0 && r.x < rect.right && r.x + r.width > rect.left && r.y < rect.bottom && r.y + r.height > rect.top).slice(0, 128);
        const style = getComputedStyle(element);
        const clip = { pixel_ratio: devicePixelRatio, viewport: browserViewportClip(element), overlays, radius: parseFloat(style.borderTopLeftRadius) || 0 };
        const show = visible && document.visibilityState === 'visible';
        const scrollbarCss = browserScrollbarCss(style.getPropertyValue('--panel-scrollbar').trim(), style.getPropertyValue('--panel-scrollbar-hover').trim());
        const signature = JSON.stringify([bounds, show, scrollbarCss, clip]);
        if (signature !== last) {
          last = signature;
          void invoke('embedded_browser_layout', { bounds, visible: show, sequence: ++layoutSequence, scrollbarCss, clip }).catch(cause => { if (!disposed) setError(String(cause)); });
        }
      }
      if (visible) frame = requestAnimationFrame(measure);
    };
    const observer = new MutationObserver(() => { if (visible) measure(); });
    if (visible) {
      observer.observe(document.body, { subtree: true, childList: true, attributes: true, attributeFilter: ['data-browser-overlay', 'data-browser-overlay-width', 'aria-modal', 'open'] });
      measure();
    } else hide();
    return () => { disposed = true; observer.disconnect(); cancelAnimationFrame(frame); hide(); };
  }, [ready, visible]);

  const navigate = () => { const input = browserSearchInput(address, searchEngine); setEditingAddress(false); setMenu(null); if (active) void send('navigate', { id: active.id, input }); else void send('new_tab', { input }); };
  const panel = menu === 'menu' ? submenu : menu ?? internalPage;
  useEffect(() => { setBookmarkMenu(null); setRenamingBookmark(null); }, [panel, menu]);
  useLayoutEffect(() => { bookmarkMenuRef.current?.querySelector('button')?.focus(); }, [bookmarkMenu]);
  useLayoutEffect(() => { if (renamingBookmark) { bookmarkNameRef.current?.focus(); bookmarkNameRef.current?.select(); } }, [renamingBookmark?.url]);
  useEffect(() => {
    if (!bookmarkMenu) return;
    const close = (event: PointerEvent) => { if (event.target instanceof Node && !bookmarkMenuRef.current?.contains(event.target)) setBookmarkMenu(null); };
    document.addEventListener('pointerdown', close);
    return () => document.removeEventListener('pointerdown', close);
  }, [bookmarkMenu]);
  const openLibrary = (page: BrowserLibraryPage) => { cancelHover(); setMenu(null); setBookmarkMenu(null); setQuery(''); void send('new_tab', { input: browserLibraryUrl(page) }); };
  useEffect(() => { setQuery(''); setPageQuery(''); setBookmarkMenu(null); setRenamingBookmark(null); }, [active?.id, internalPage]);
  const openPage = (input: string) => { setMenu(null); if (active) void send('navigate', { id: active.id, input }); else void send('new_tab', { input }); };
  const showMenu = (next: Menu) => { cancelHover(); pointerMenu.current = true; setSubmenu(null); setQuery(''); setMenu(current => current === next ? null : next); };
  const recent = browserEntries(state.history, '').filter((entry, index, all) => all.findIndex(other => other.url === entry.url) === index).slice(0, 6);
  const recommended = state.bookmarks.length ? browserEntries(state.bookmarks, '').slice(0, 4) : [
    { title: t('embeddedBrowser.siteBilibili'), url: 'https://www.bilibili.com/' },
    { title: t('embeddedBrowser.siteXiaohongshu'), url: 'https://www.xiaohongshu.com/' },
    { title: t('embeddedBrowser.siteZhihu'), url: 'https://www.zhihu.com/' },
    { title: t('embeddedBrowser.siteDouyin'), url: 'https://www.douyin.com/' },
  ];
  const entryHost = (url: string) => { try { return new URL(url).hostname.replace(/^www\./, ''); } catch { return url; } };
  const entryTime = (at: number) => {
    const date = new Date(at); const today = date.toDateString() === new Date().toDateString();
    return new Intl.DateTimeFormat(i18n.resolvedLanguage || i18n.language, today ? { hour: 'numeric', minute: '2-digit' } : { month: 'short', day: 'numeric' }).format(date);
  };
  const button = (label: string, handler: () => void, disabled = false) => <button type="button" disabled={disabled} onClick={handler}>{t(`embeddedBrowser.${label}`)}</button>;
  const icon = (label: string, child: ReactNode, handler: () => void, disabled = false) => <button type="button" disabled={disabled} aria-label={t(`embeddedBrowser.${label}`)} title={t(`embeddedBrowser.${label}`)} onClick={handler}>{child}</button>;
  const menuKeys = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'ArrowLeft' && event.currentTarget.dataset.submenu && menu === 'menu') {
      event.preventDefault(); setSubmenu(null); if (submenu) submenuTriggers.current[submenu]?.focus(); return;
    }
    if (event.target instanceof HTMLInputElement || !['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    const controls = [...event.currentTarget.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
    if (!controls.length) return;
    event.preventDefault();
    const index = controls.indexOf(document.activeElement as HTMLButtonElement);
    controls[event.key === 'Home' ? 0 : event.key === 'End' ? controls.length - 1 : (index + (event.key === 'ArrowDown' ? 1 : controls.length - 1)) % controls.length]?.focus();
  };
  const menuItem = (label: string, handler: () => void, options: { disabled?: boolean; shortcut?: string; child?: Submenu } = {}) => <button type="button" role="menuitem" disabled={options.disabled} onClick={handler}
    ref={options.child ? element => { submenuTriggers.current[options.child!] = element; } : undefined}
    aria-haspopup={options.child ? 'menu' : undefined} aria-expanded={options.child ? submenu === options.child : undefined} aria-controls={options.child && submenu === options.child ? 'embedded-browser-submenu' : undefined}
    onPointerEnter={event => { cancelHover(); if (options.child && event.pointerType !== 'touch') hoverTimer.current = setTimeout(() => { if (pointerMenu.current) openSubmenu(options.child!, false); }, options.child === 'settings' ? 500 : browserSubmenuDelay); }} onPointerLeave={cancelHover}
    onKeyDown={event => { if (event.key === 'ArrowRight' && options.child) { event.preventDefault(); openSubmenu(options.child); } }}>
    <span>{t(`embeddedBrowser.${label}`)}</span>{options.shortcut && <kbd>{options.shortcut}</kbd>}{options.child && <ChevronRight size={16} />}
  </button>;
  const libraryContent = (panel: Submenu, inMenu = false) => { const entries = panel === 'history' ? state.history : state.bookmarks; const search = inMenu ? query : pageQuery; const setSearch = inMenu ? setQuery : setPageQuery; return <>
      {(panel === 'history' || panel === 'bookmarks') && <>
        <label className="embedded-browser-menu-search"><Search size={15} /><input value={search} aria-label={t('embeddedBrowser.searchEntries')} placeholder={t('embeddedBrowser.searchEntries')} onChange={event => setSearch(event.target.value)} /></label>
        {panel === 'history' && <div className="embedded-browser-submenu-toolbar">{button('clearHistory', () => void send('clear_history'))}</div>}
        {browserEntries(entries, search).map((entry, index) => <div className="embedded-browser-entry" key={`${entry.at}:${index}`} onContextMenu={event => {
          if (panel !== 'bookmarks' || event.target instanceof HTMLInputElement) return;
          event.preventDefault(); event.stopPropagation();
          const rect = event.currentTarget.getBoundingClientRect();
          const computed = getComputedStyle(event.currentTarget);
          setMenuTheme({ ...Object.fromEntries(['--codex-ink', '--codex-ink-faint', '--codex-accent', '--codex-paper-card', '--codex-line-light', '--codex-paper', '--panel-scrollbar', '--panel-scrollbar-hover'].map(key => [key, computed.getPropertyValue(key)])), fontFamily: computed.fontFamily } as CSSProperties);
          setBookmarkMenu({ url: entry.url, title: entry.title || entry.url, x: Math.max(8, Math.min(event.clientX || rect.left, innerWidth - 176)), y: Math.max(8, Math.min(event.clientY || rect.bottom, innerHeight - 100)) });
        }}>
          <BrowserSiteIcon url={entry.url} size={16} />
          {panel === 'bookmarks' && renamingBookmark?.url === entry.url && (inMenu || !menu) ? <form className="embedded-browser-bookmark-rename" onSubmit={event => { event.preventDefault(); const title = renamingBookmark.title.trim(); if (title) void send('rename_bookmark', { url: entry.url, title }).then(saved => { if (saved) setRenamingBookmark(current => current?.url === entry.url ? null : current); }); }}>
            <input ref={bookmarkNameRef} aria-label={t('embeddedBrowser.bookmarkName')} maxLength={500} value={renamingBookmark.title} onChange={event => setRenamingBookmark({ url: entry.url, title: event.target.value })} onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); setRenamingBookmark(null); } }} />
            <div><button type="submit" disabled={!renamingBookmark.title.trim()}>{t('embeddedBrowser.saveBookmarkName')}</button>{button('cancel', () => setRenamingBookmark(null))}</div>
          </form> : <button type="button" className="embedded-browser-entry-link" onClick={() => openPage(entry.url)}><strong>{entry.title || entry.url}</strong><small>{entry.url}</small></button>}
        </div>)}
        {!browserEntries(entries, search).length && <div className="embedded-browser-menu-empty"><History size={25} /><p>{t('embeddedBrowser.noEntries')}</p></div>}
      </>}
      {panel === 'downloads' && <><div className="embedded-browser-submenu-toolbar">{inMenu ? button('downloads', () => act('downloads')) : button('reload', () => void send('init'))}</div>{!state.downloads.length && <div className="embedded-browser-menu-empty"><Download size={25} /><p>{t('embeddedBrowser.noEntries')}</p></div>}{state.downloads.map(item => <div className="embedded-browser-download" key={item.id}>
        <strong>{item.path.split(/[\\/]/).pop()}</strong>{!inMenu && <small>{item.path}</small>}<small>{t(`embeddedBrowser.download_${item.state}`)} · {browserBytes(item.received)} / {browserBytes(item.total)}</small>
        {item.state !== 2 && <div>{(['pause', 'resume', 'cancel'] as const).map(name => <button type="button" key={name} onClick={() => void send('download', { id: item.id, name })}>{t(`embeddedBrowser.${name}`)}</button>)}</div>}
      </div>)}</>}
      {panel === 'extensions' && <><div className="embedded-browser-submenu-toolbar">{button('installExtension', () => { void open({ directory: true, multiple: false, title: t('embeddedBrowser.extensionFolder') }).then(path => { if (typeof path === 'string') void send('extension', { id: '', name: 'install', path }); }).catch(cause => setError(String(cause))); })}</div>
        {!state.extensions.length && <div className="embedded-browser-menu-empty"><Puzzle size={25} /><p>{t('embeddedBrowser.noEntries')}</p></div>}
        {state.extensions.map(ext => <div className="embedded-browser-entry" key={ext.id}><strong>{ext.name}</strong>
          {button(ext.enabled ? 'disable' : 'enable', () => void send('extension', { id: ext.id, name: ext.enabled ? 'disable' : 'enable' }))}
          {button('remove', () => void send('extension', { id: ext.id, name: 'remove' }))}
        </div>)}
      </>}
  </>; };
  useLayoutEffect(() => {
    if (!visible || !tabHost) return;
    const frame = requestAnimationFrame(() => tabHost.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest', inline: 'nearest' }));
    return () => cancelAnimationFrame(frame);
  }, [state.active, visible, tabHost]);
  const tabBar = (<div className="embedded-browser-tabs" role={tabHost ? undefined : "tablist"} aria-label={tabHost ? undefined : t('embeddedBrowser.title')}>
      {state.tabs.map(tab => <div className={`embedded-browser-tab ${tab.id === state.active && visible ? 'active' : ''}`} key={tab.id}>
        <button type="button" role="tab" aria-selected={tab.id === state.active && visible} title={tab.url} onClick={() => void send('select', { id: tab.id })}>
          {tab.private ? <VenetianMask size={12} /> : tab.start ? <Globe size={12} /> : <BrowserSiteIcon url={tab.url} size={12} />}<span>{tabTitle(tab)}</span>
        </button>
        {icon('closeTab', <X size={12} />, () => void send('close', { id: tab.id }))}
      </div>)}
      {!tabHost && icon('newTab', <Plus size={15} />, () => void send('new_tab', { input: '' }))}
    </div>);
  return <section className="embedded-browser" aria-label={t('embeddedBrowser.title')} onKeyDown={event => {
    if (!event.ctrlKey) return;
    if (event.key.toLowerCase() === 'l') { event.preventDefault(); addressRef.current?.focus(); addressRef.current?.select(); }
    if (event.key.toLowerCase() === 't') { event.preventDefault(); void send(event.shiftKey ? 'reopen' : 'new_tab', { input: '' }); }
    if (event.key.toLowerCase() === 'f') { event.preventDefault(); setFindOpen(true); }
    if (event.key.toLowerCase() === 'w' && active) { event.preventDefault(); void send('close', { id: active.id }); }
    if (event.key.toLowerCase() === 'n' && event.shiftKey) { event.preventDefault(); void send('new_tab', { input: '', private: true }); }
  }}>
    {tabHost ? createPortal(tabBar, tabHost) : tabBar}
    <div className="embedded-browser-controls" role="toolbar" aria-label={t('embeddedBrowser.navigation')}>
      <div className="embedded-browser-nav-capsule">
      {icon('back', <ArrowLeft size={15} />, () => act('back'), !active?.can_back)}
      {icon('forward', <ArrowRight size={15} />, () => act('forward'), !active?.can_forward)}
      <span className="embedded-browser-nav-divider" />
      {icon(active?.loading ? 'stop' : 'reload', active?.loading ? <LoaderCircle className="embedded-browser-spinner" size={16} /> : <RotateCw size={16} />, () => act(active?.loading ? 'stop' : 'reload'), !active || active.start)}
      </div>
      <form className={`embedded-browser-address ${!address ? 'is-empty' : ''}`} onSubmit={event => { event.preventDefault(); navigate(); }}>
        <input ref={addressRef} aria-label={t('embeddedBrowser.address')} placeholder={`${t('embeddedBrowser.address')} · ${t(`embeddedBrowser.searchEngine_${searchEngine}`)}`} value={address} spellCheck={false}
          onFocus={() => setEditingAddress(true)} onBlur={() => setEditingAddress(false)} onChange={event => setAddress(event.target.value)} onKeyDown={event => { if (event.key === 'Escape') { setAddress(active?.url === 'about:blank' ? '' : active?.url ?? ''); setEditingAddress(false); event.currentTarget.blur(); } }} />
        {active && !active.start && icon('bookmark', <Star size={14} fill={state.bookmarks.some(b => b.url === active.url) ? 'currentColor' : 'none'} />, () => void send('bookmark', { id: active.id }))}
      </form>
      <div className="embedded-browser-actions-capsule" data-browser-menu-trigger>
        {icon('extensions', <Puzzle size={16} />, () => showMenu('extensions'))}
        {icon('downloads', <Download size={16} />, () => showMenu('downloads'))}
      </div>
      <button ref={menuButtonRef} type="button" aria-label={t('embeddedBrowser.menu')} title={t('embeddedBrowser.menu')} aria-haspopup="menu" aria-expanded={menu !== null} aria-controls="embedded-browser-menu" onClick={() => showMenu('menu')}><Ellipsis size={19} /></button>
    </div>
    {state.background_tasks > 0 && <div className="embedded-browser-task-status" role="status">{t('embeddedBrowser.backgroundTasks', { count: state.background_tasks })}</div>}
    {findOpen && <form className="embedded-browser-find" onSubmit={event => { event.preventDefault(); act('find', find); }}>
      <Search size={13} /><input ref={findRef} value={find} aria-label={t('embeddedBrowser.find')} placeholder={t('embeddedBrowser.find')} onChange={event => setFind(event.target.value)} />
      {button('previous', () => act('find_previous', find))}{button('next', () => act('find', find))}{icon('close', <X size={13} />, () => { setFindOpen(false); act('find_stop'); })}
    </form>}
    {(error || state.error) && <div className="embedded-browser-error" role="alert">{t('embeddedBrowser.error', { error: error || state.error })}{button('retry', () => void send('init'))}{icon('close', <X size={13} />, () => { setError(''); setState(current => ({ ...current, error: null })); })}</div>}
    {!ready && !error && <p role="status">{t('embeddedBrowser.loading')}</p>}
    {state.permissions.map(permission => <div className="embedded-browser-permission" key={permission.id} role="status">
      <p>{t('embeddedBrowser.permission', { origin: permission.origin, kind: t(`embeddedBrowser.permission_${permission.kind}`, { defaultValue: t('embeddedBrowser.permissionOther', { kind: permission.kind }) }) })}</p>
      {button('allow', () => void send('permission', { id: permission.id, allow: true }))}{button('deny', () => void send('permission', { id: permission.id, allow: false }))}
    </div>)}
    {((menu && menuPosition) || bookmarkMenu) && createPortal(<div ref={menuRef} className="embedded-browser embedded-browser-popup-root" style={menuTheme}
      onPointerMove={event => { if (event.movementX || event.movementY) pointerMenu.current = true; }} onPointerDown={() => { pointerMenu.current = true; }}
      onKeyDownCapture={() => { pointerMenu.current = false; cancelHover(); }}>
      {menu === 'menu' && menuPosition && <div id="embedded-browser-menu" role="menu" aria-label={t('embeddedBrowser.menu')} className="embedded-browser-menu embedded-browser-primary-menu" style={menuPosition.main} onKeyDown={menuKeys}>
        {menuItem('find', () => { setFindOpen(true); setMenu(null); }, { disabled: !active || active.start, shortcut: 'Ctrl+F' })}
        {menuItem('print', () => { act('print'); setMenu(null); }, { disabled: !active || active.start })}
        <div className="embedded-browser-menu-separator" role="separator" />
        <div className="embedded-browser-menu-zoom"><span>{t('embeddedBrowser.zoom')}</span><div>
          {icon('zoomOut', <Minus size={15} />, () => act('zoom_out'), !active || active.zoom <= .25)}
          <output>{Math.round((active?.zoom ?? 1) * 100)}%</output>
          {icon('zoomIn', <Plus size={15} />, () => act('zoom_in'), !active || active.zoom >= 5)}
        </div>{icon('resetZoom', <RotateCw size={16} />, () => act('zoom_reset'), !active)}</div>
        <div className="embedded-browser-menu-separator" role="separator" />
        {menuItem('devtools', () => { act('devtools'); setMenu(null); }, { disabled: !active || active.start })}
        {menuItem('save', () => { act('save'); setMenu(null); }, { disabled: !active || active.start })}
        {menuItem('mute', () => { act('mute'); setMenu(null); }, { disabled: !active || active.start })}
        <div className="embedded-browser-menu-separator" role="separator" />
        {browserLibraryPages.map(item => <div key={item}>{menuItem(item, () => openLibrary(item), { child: item })}</div>)}
        {menuItem('clearData', () => openSubmenu('settings'))}
        <div className="embedded-browser-menu-separator" role="separator" />
        {menuItem('newTab', () => { void send('new_tab', { input: '' }); setMenu(null); }, { shortcut: 'Ctrl+T' })}
        {menuItem('privateTab', () => { void send('new_tab', { input: '', private: true }); setMenu(null); }, { shortcut: 'Ctrl+Shift+N' })}
        {menuItem('reopen', () => { void send('reopen'); setMenu(null); })}
        <div className="embedded-browser-menu-separator" role="separator" />
        {menuItem('settings', () => openSubmenu('settings'), { child: 'settings' })}
      </div>}
      {menu && panel && menuPosition && <div ref={submenuRef} data-submenu={panel} id={menu === 'menu' ? 'embedded-browser-submenu' : 'embedded-browser-menu'} role="menu" aria-label={t(`embeddedBrowser.${panel}`)} className="embedded-browser-menu embedded-browser-submenu" style={menu === 'menu' ? menuPosition.sub : { ...menuPosition.main, width: Math.min(340, innerWidth - 16), left: Math.max(8, menuPosition.main.left - 60) }} onKeyDown={menuKeys}>
      <header><span className="embedded-browser-menu-heading-icon">{panel === 'history' ? <History size={17} /> : panel === 'bookmarks' ? <Star size={17} /> : panel === 'downloads' ? <Download size={17} /> : panel === 'extensions' ? <Puzzle size={17} /> : <Globe size={17} />}</span><strong>{t(`embeddedBrowser.${panel}`)}</strong></header>
      {libraryContent(panel, true)}
      {panel === 'settings' && <div className="embedded-browser-menu-settings">
        <fieldset className="embedded-browser-search-engines"><legend>{t('embeddedBrowser.searchEngine')}</legend>
          {browserSearchEngines.map(engine => <label key={engine}><input type="radio" name="browser-search-engine" value={engine} checked={searchEngine === engine} onChange={() => setSearchEngine(engine)} /><span>{t(`embeddedBrowser.searchEngine_${engine}`)}</span></label>)}
        </fieldset><p>{t('embeddedBrowser.clearDataHint')}</p>{button('clearData', () => { if (window.confirm(t('embeddedBrowser.clearDataConfirm'))) act('clear_data'); })}</div>}
    </div>}
      {bookmarkMenu && panel === 'bookmarks' && <div ref={bookmarkMenuRef} role="menu" aria-label={t('embeddedBrowser.bookmarkActions')} className="embedded-browser-menu embedded-browser-bookmark-context" style={{ left: bookmarkMenu.x, top: bookmarkMenu.y }} onKeyDown={event => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); setBookmarkMenu(null); } else menuKeys(event); }}>
        {menuItem('deleteBookmark', () => { void send('remove_bookmark', { url: bookmarkMenu.url }); setBookmarkMenu(null); })}
        {menuItem('rename', () => { setRenamingBookmark({ url: bookmarkMenu.url, title: bookmarkMenu.title }); setBookmarkMenu(null); })}
      </div>}
    </div>, document.body)}
    <div ref={viewport} className="embedded-browser-viewport">
      {internalPage && <section className="embedded-browser-library-page"><header><h1>{t(`embeddedBrowser.${internalPage}`)}</h1><nav>{browserLibraryPages.map(page => <button type="button" key={page} aria-current={internalPage === page ? 'page' : undefined} onClick={() => { setMenu(null); setQuery(''); void send('navigate', { id: active!.id, input: browserLibraryUrl(page) }); }}>{t(`embeddedBrowser.${page}`)}</button>)}</nav></header>{libraryContent(internalPage)}</section>}
      {(!active || (active.start && !internalPage)) && <div className="embedded-browser-start">
        {active?.private && <p className="embedded-browser-private-hint"><VenetianMask size={18} />{t('embeddedBrowser.privateHint')}</p>}
        <section className="embedded-browser-start-section"><h3>{t('embeddedBrowser.tools')}</h3>
          <div className="embedded-browser-start-tools">
            {onNewTerminal && <button type="button" onClick={onNewTerminal}><TerminalIcon size={18} /><span>{t('mind_inspector.code_new_terminal')}</span><ChevronRight size={16} /></button>}
            <button type="button" onClick={() => { addressRef.current?.focus(); addressRef.current?.select(); }}><Search size={18} /><span>{t('embeddedBrowser.searchWeb')}</span><kbd>Ctrl+L</kbd></button>
            <button type="button" onClick={() => void send('new_tab', { input: '', private: true })}><VenetianMask size={18} /><span>{t('embeddedBrowser.privateTab')}</span><kbd>Ctrl+Shift+N</kbd></button>
            <button type="button" onClick={() => openLibrary('bookmarks')}><Star size={18} /><span>{t('embeddedBrowser.bookmarks')}</span><ChevronRight size={16} /></button>
            <button type="button" onClick={() => openLibrary('downloads')}><Download size={18} /><span>{t('embeddedBrowser.downloads')}</span><ChevronRight size={16} /></button>
          </div>
        </section>
        {!active?.private && <>
          <section className="embedded-browser-start-section"><h3>{t('embeddedBrowser.recommended')}</h3>
            <div className="embedded-browser-start-sites">{recommended.map(entry => <button type="button" key={entry.url} title={entry.url} onClick={() => openPage(entry.url)}>
              <span className="embedded-browser-site-icon"><BrowserSiteIcon url={entry.url} size={28} /></span><strong>{entry.title || entryHost(entry.url)}</strong><small>{entryHost(entry.url)}</small>
            </button>)}</div>
          </section>
          <section className="embedded-browser-start-section"><header><h3>{t('embeddedBrowser.recent')}</h3>{recent.length > 0 && <button type="button" onClick={() => showMenu('history')}>{t('embeddedBrowser.viewAll')}<ChevronRight size={14} /></button>}</header>
            <div className="embedded-browser-start-recent">{recent.map(entry => <button type="button" key={entry.url} title={entry.url} onClick={() => openPage(entry.url)}>
              <span className="embedded-browser-recent-icon"><BrowserSiteIcon url={entry.url} size={19} /></span><span className="embedded-browser-recent-label"><strong>{entry.title || entryHost(entry.url)}</strong><small>{entryHost(entry.url)}</small></span><time dateTime={new Date(entry.at).toISOString()}>{entryTime(entry.at)}</time>
            </button>)}{!recent.length && <p><History size={20} />{t('embeddedBrowser.noRecent')}</p>}</div>
          </section>
        </>}
      </div>}
    </div>
  </section>;
}
