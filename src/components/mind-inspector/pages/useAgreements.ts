import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { MemoryRecord } from './memoryPresentation';
import { agreementViews, type AgreementView } from './agreementPresentation';

export function useAgreements() {
  const [records, setRecords] = useState<AgreementView[]>([]);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(true);
  const request = useRef(0);
  const reload = useCallback(async () => {
    const id = ++request.current;
    try {
      const values = await Promise.all((['vivian', 'nana'] as const).map(async owner =>
        agreementViews(owner, await invoke<MemoryRecord[]>('get_memories', { characterId: owner }))));
      if (id === request.current) { setRecords(values.flat().sort((a, b) => b.task.updated_at - a.task.updated_at)); setError(''); }
    } catch (e) { if (id === request.current) setError(String(e)); }
    finally { if (id === request.current) setLoading(false); }
  }, []);
  useEffect(() => {
    void reload(); let cancelled = false; let unlisten: (() => void) | undefined;
    void listen('memory:updated', () => void reload()).then(fn => { if (cancelled) fn(); else unlisten = fn; });
    return () => { cancelled = true; ++request.current; unlisten?.(); };
  }, [reload]);
  return { records, error, loading, reload };
}
