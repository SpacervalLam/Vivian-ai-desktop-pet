import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useTranslation } from 'react-i18next';
import './workInputStrings';

export interface InputPayload {
  content: string;
  images: { media_type: string; data: string; name?: string | null }[];
  file_refs: { path: string }[];
}
export interface QueuedInput { id: string; kind: 'follow_up' | 'steering'; payload: InputPayload }
export interface InputQueue { revision: number; paused: boolean; items: QueuedInput[] }
const empty: InputQueue = { revision: -1, paused: false, items: [] };

export default function useWorkInputQueue(sessionId: string | null) {
  const { t } = useTranslation();
  const active = useRef(sessionId);
  active.current = sessionId;
  const [state, setState] = useState<{ sessionId: string | null; queue: InputQueue; error: string }>({ sessionId: null, queue: empty, error: '' });
  const [, setPendingRevision] = useState(0);
  const pending = useRef(new Set<string>());
  const retry = useRef<{ sessionId: string; serialized: string; id: string } | null>(null);
  const accept = useCallback((id: string, queue: InputQueue) => {
    if (active.current !== id) return;
    setState(previous => previous.sessionId === id && previous.queue.revision > queue.revision ? previous : { sessionId: id, queue, error: '' });
  }, []);
  const fail = useCallback((id: string, error: unknown) => {
    if (active.current === id) setState(previous => ({ sessionId: id, queue: previous.sessionId === id ? previous.queue : empty, error: String(error) }));
  }, []);

  useEffect(() => {
    if (!sessionId) return;
    let disposed = false, loading = false;
    let unlisten: (() => void) | undefined;
    const load = async () => {
      if (disposed || loading) return;
      loading = true;
      try {
        const queue = await invoke<InputQueue>('coding_input_queue', { sessionId, action: 'list' });
        if (!disposed) accept(sessionId, queue);
      } catch (error) { if (!disposed) fail(sessionId, error); }
      finally { loading = false; }
    };
    void listen<{ session_id: string; queue: InputQueue }>('coding:input_queue', ({ payload }) => {
      if (!disposed && payload.session_id === sessionId) accept(sessionId, payload.queue);
    }).then(release => { if (disposed) release(); else { unlisten = release; void load(); } }).catch(error => { if (!disposed) { fail(sessionId, error); void load(); } });
    const timer = window.setInterval(() => { if (document.visibilityState === 'visible') void load(); }, 5000);
    return () => { disposed = true; unlisten?.(); window.clearInterval(timer); };
  }, [sessionId, accept, fail]);

  const action = useCallback(async (operation: string, id?: string, payload?: InputPayload) => {
    if (!sessionId) throw new Error(t('workInput.noSession'));
    if (pending.current.has(sessionId)) throw new Error(t('workInput.busy'));
    pending.current.add(sessionId); setPendingRevision(value => value + 1);
    try {
      const queue = await invoke<InputQueue>('coding_input_queue', { sessionId, action: operation, id,
        message: payload?.content, images: payload?.images, fileRefs: payload?.file_refs });
      accept(sessionId, queue);
      return queue;
    } catch (error) { fail(sessionId, error); throw error; }
    finally { pending.current.delete(sessionId); setPendingRevision(value => value + 1); }
  }, [sessionId, accept, fail, t]);

  const enqueue = useCallback(async (payload: InputPayload) => {
    if (!sessionId) throw new Error(t('workInput.noSession'));
    const serialized = JSON.stringify(payload);
    if (retry.current?.sessionId !== sessionId || retry.current.serialized !== serialized) {
      retry.current = { sessionId, serialized, id: crypto.randomUUID() };
    }
    const attempt = retry.current;
    await action('add', attempt.id, payload);
    if (retry.current === attempt) retry.current = null;
  }, [sessionId, action, t]);

  return { queue: state.sessionId === sessionId ? state.queue : empty,
    error: state.sessionId === sessionId ? state.error : '', busy: !!sessionId && pending.current.has(sessionId), action, enqueue };
}
