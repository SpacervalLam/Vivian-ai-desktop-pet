import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { listen, emit } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import MessageBubble, { type BubblePosition } from './MessageBubble';
import type { SettledBubble } from '../stores/useAppStore';
import { bubbleContentKey } from '../utils/bubbleContent';

interface BubbleSnapshot {
  text: string | null;
  settled: SettledBubble[];
  position?: BubblePosition;
  character_id?: string;
  cross_character?: boolean;
  listener_name?: string | null;
}

/** The pet owns timing; this transparent window renders and reports its actual content height. */
export default function BubbleWindow() {
  const [snapshot, setSnapshot] = useState<BubbleSnapshot>({ text: null, settled: [], position: 'top' });
  const container = useRef<HTMLDivElement>(null);
  const myCharId = new URLSearchParams(window.location.search).get('character_id') ?? '';
  const position = snapshot.position ?? 'top';
  const key = bubbleContentKey(snapshot.settled, snapshot.text);
  const hasContent = !!snapshot.text || snapshot.settled.length > 0;

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void getCurrentWindow().setIgnoreCursorEvents(true).catch(() => {});
    void (async () => {
      const stop = await listen<BubbleSnapshot>('bubble:sync', (event) => {
        if ((event.payload.character_id ?? '') !== myCharId) return;
        setSnapshot(event.payload);
      });
      if (cancelled) { stop(); return; }
      unlisten = stop;
      await emit('bubble:ready', { character_id: myCharId });
    })();
    return () => { cancelled = true; unlisten?.(); };
  }, [myCharId]);

  useLayoutEffect(() => {
    const node = container.current;
    if (!node || !hasContent) return;
    const measure = () => {
      void emit('bubble:measured', { character_id: myCharId, key, height: Math.ceil(node.getBoundingClientRect().height) });
    };
    const observer = new ResizeObserver(measure);
    observer.observe(node);
    measure();
    return () => observer.disconnect();
  }, [key, hasContent, myCharId, snapshot.cross_character, snapshot.listener_name]);

  if (!hasContent) return null;
  return (
    <div style={{ position: 'fixed', inset: 0, overflow: 'hidden', background: 'transparent' }}>
      <div ref={container} style={{
        position: 'absolute', left: 0, right: 0,
        ...(position === 'top' ? { bottom: 0 } : { top: 0 }),
        display: 'flex', flexDirection: position === 'top' ? 'column' : 'column-reverse',
        alignItems: 'center', gap: 8, padding: 8,
      }}>
        {snapshot.settled.filter(bubble => !bubble.sticker).map((bubble) => (
          <MessageBubble key={bubble.id} text={bubble.text} duration={0} position={position}
            characterId={myCharId} crossCharacter={snapshot.cross_character}
            listenerName={snapshot.listener_name ?? undefined} />
        ))}
        {snapshot.text && (
          <MessageBubble text={snapshot.text} duration={0} position={position}
            characterId={myCharId} crossCharacter={snapshot.cross_character}
            listenerName={snapshot.listener_name ?? undefined} />
        )}
        {snapshot.settled.filter(bubble => bubble.sticker).map((bubble) => (
          <MessageBubble key={bubble.id} text="" sticker={bubble.sticker} duration={0} position={position}
            characterId={myCharId} crossCharacter={snapshot.cross_character}
            listenerName={snapshot.listener_name ?? undefined} />
        ))}
      </div>
    </div>
  );
}
