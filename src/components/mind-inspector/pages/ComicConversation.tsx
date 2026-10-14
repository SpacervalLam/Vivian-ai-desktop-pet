import { useEffect, useRef, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import StickerImage from '../../stickers/StickerImage';
import type { MemoryTurn, RecentThread } from './memoryPresentation';

/** Hide parenthetical stage directions only in the comic conversation view. */
export function stripParentheticalText(text: string): string {
  let previous: string;
  do {
    previous = text;
    text = text.replace(/\([^()（）]*\)|（[^()（）]*）/g, '');
  } while (text !== previous);
  return text.trim();
}

/** Original turns, not a summary. The viewport scrolls; messages never loop. */
export default function ComicConversation({ thread, active, loading, error, renderText }: {
  thread?: RecentThread; active: boolean; loading: boolean; error: string;
  renderText: (text: string) => ReactNode;
}) {
  const { t, i18n } = useTranslation();
  const audienceName = (name: string) => name === '用户' ? t('comic.user') : name === '大家' ? t('comic.everyone') : name;
  const viewport = useRef<HTMLDivElement>(null);
  const previousThread = useRef('');
  const signature = JSON.stringify(thread?.turns.map(turn => [turn.id, turn.text, turn.sticker]) ?? []);

  useEffect(() => {
    if (previousThread.current === thread?.id) return;
    previousThread.current = thread?.id ?? '';
    if (viewport.current) viewport.current.scrollTop = 0;
  }, [thread?.id]);

  useEffect(() => {
    const node = viewport.current;
    if (!node || !active || !thread) return;
    let frame = 0;
    let last = 0;
    let position = node.scrollTop;
    const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
    const step = (now: number) => {
      const selection = window.getSelection();
      if (document.hidden || (selection && !selection.isCollapsed && node.contains(selection.anchorNode))) {
        last = 0;
      } else {
        const remaining = node.scrollHeight - node.clientHeight - node.scrollTop;
        if (remaining <= 1) return;
        if (reduced) { node.scrollTop = node.scrollHeight; return; }
        // Cap time deltas so restoring the window never jumps down the thread.
        // Some WebViews quantize scrollTop to whole CSS pixels. Accumulate the
        // fractional distance separately so slow movement cannot round to zero.
        position += last ? Math.min(now - last, 64) * .010 : 0;
        node.scrollTop = position;
        last = now;
      }
      frame = requestAnimationFrame(step);
    };
    // Reflow (including a sticker loading) can extend a previously finished log.
    const resume = () => { cancelAnimationFrame(frame); last = 0; position = node.scrollTop; frame = requestAnimationFrame(step); };
    const resize = new ResizeObserver(resume);
    resize.observe(node);
    if (node.firstElementChild) resize.observe(node.firstElementChild);
    resume();
    return () => { cancelAnimationFrame(frame); resize.disconnect(); };
  }, [active, thread?.id, signature]);

  const isUser = (turn: MemoryTurn) => turn.speaker === '用户';
  return <div className="comic-conversation">
    {error && <p className="memory-error" role="alert">{error}</p>}
    <div ref={viewport} className="comic-conversation-viewport" aria-label={t('comic.conversationViewport')}>
      <div className="comic-conversation-messages" role="log" aria-label={t('comic.conversationLog')} aria-live="off">
        {thread?.turns.map(turn => ({ ...turn, text: stripParentheticalText(turn.text) }))
          .filter(turn => turn.text || turn.sticker)
          .map((turn, index) => <div key={turn.id ?? `${thread.id}-${index}`}
          className={`comic-message ${isUser(turn) ? 'is-sent' : 'is-received'} ${turn.speaker === 'Nana' ? 'is-nana' : ''}`} data-message-id={turn.id}>
          {!isUser(turn) && <span className="comic-message-avatar" aria-hidden="true">{turn.speaker === 'Nana' ? 'N' : turn.speaker === 'Vivian' ? 'V' : '·'}</span>}
          <div className="comic-message-body">
            {!isUser(turn) && <small className="comic-message-speaker" title={turn.audience ? t('comic.audience', { name: audienceName(turn.audience) }) : undefined}>{turn.speaker}</small>}
            <div className="comic-message-bubble">{turn.text && <p>{renderText(turn.text)}</p>}{turn.sticker && <StickerImage sticker={turn.sticker} size={96} />}</div>
            {turn.timestamp && <time>{new Date(turn.timestamp < 1e12 ? turn.timestamp * 1000 : turn.timestamp).toLocaleTimeString(i18n.resolvedLanguage ?? i18n.language, { hour: '2-digit', minute: '2-digit' })}</time>}
          </div>
        </div>)}
        {!thread && <p className="comic-conversation-empty">{t(loading ? 'comic.conversationLoading' : 'comic.conversationEmpty')}</p>}
      </div>
    </div>
  </div>;
}
