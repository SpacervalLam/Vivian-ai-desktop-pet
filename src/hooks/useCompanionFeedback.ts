import { useEffect, useState, type RefObject } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { getCharacterId } from '../characterContext';
import { BubbleController } from '../controllers/BubbleController';
import { TtsStreamQueue } from '../controllers/TtsStreamQueue';
import { useAppStore } from '../stores/useAppStore';
import type { ModelRendererHandle } from '../components/ModelCanvas';
import { playCompanionTone } from '../utils/companionSound';
import { openReminderWindow } from '../utils/reminderWindow';

export interface CompanionStatus {
  quiet_reason: 'focus' | 'game' | 'quiet_hours' | null;
  cpu_pressure: boolean;
  memory_pressure: boolean;
  network_connected: boolean | null;
}
export function useCompanionFeedback(pet: RefObject<ModelRendererHandle | null>): CompanionStatus | null {
  const [status, setStatus] = useState<CompanionStatus | null>(null);
  useEffect(() => {
    let cancelled = false;
    let inFlight = false;
    let lastQuiet: CompanionStatus['quiet_reason'] | undefined;
    const cleanup: UnlistenFn[] = [];
    const refresh = async () => {
      if (cancelled || inFlight) return;
      inFlight = true;
      try {
        const next = await invoke<CompanionStatus>('companion_status');
        if (!cancelled) {
          if (next.quiet_reason !== lastQuiet) {
            if (lastQuiet === 'quiet_hours') pet.current?.setExpression('wake');
            if (next.quiet_reason === 'quiet_hours' && !useAppStore.getState().currentBubble && !TtsStreamQueue.isSpeaking()) {
              pet.current?.setExpression('sleep');
            }
            lastQuiet = next.quiet_reason;
          }
          setStatus(previous => JSON.stringify(previous) === JSON.stringify(next) ? previous : next);
        }
      }
      catch { /* Backend startup or shutdown. */ }
      finally { inFlight = false; }
    };
    const add = async <T,>(name: string, handler: (payload: T) => void) => {
      const unlisten = await listen<T>(name, event => { if (!cancelled) handler(event.payload); });
      if (cancelled) unlisten(); else cleanup.push(unlisten);
    };
    void (async () => {
      await add<{ character_id: string; content: string; motion: string; sound: boolean; kind: string }>('companion:feedback', p => {
        const store = useAppStore.getState();
        if (p.character_id !== getCharacterId() || !p.content || store.currentBubble || TtsStreamQueue.isSpeaking()
          || ['busy', 'rest', 'offline'].includes(store.presenceState ?? '')) return;
        pet.current?.setExpression(p.motion, ['tired', 'umbrella', 'music'].includes(p.motion) ? 5000 : 2500);
        BubbleController.showBubble(p.content);
        if (p.sound) playCompanionTone();
      });
      await add<{ character_id: string; notice?: { important: boolean } }>('reminder:deliver', p => {
        if (p.character_id !== getCharacterId()) return;
        void openReminderWindow(p.notice?.important).then(() => {
          if (!cancelled && !useAppStore.getState().currentBubble && !TtsStreamQueue.isSpeaking()) {
            pet.current?.setExpression('remind', 2500);
          }
        }).catch(error => console.warn('[Reminder] window failed:', error));
      });
      await add<{ character_id: string; content: string; motion: string }>('companion:interaction', p => {
        if (p.character_id !== getCharacterId() || !p.content) return;
        if (!useAppStore.getState().currentBubble && !TtsStreamQueue.isSpeaking()) {
          pet.current?.setExpression(p.motion, 2500);
          BubbleController.showBubble(p.content);
        }
      });
      await add('config:saved', () => { void refresh(); });
      await refresh();
      // Restore a persisted inbox after restart, once in the main character window.
      if (!cancelled) {
        const notices = await invoke<Array<{ important: boolean }>>('pending_reminder_notices');
        if (!cancelled && notices.length) await openReminderWindow(notices.some(n => n.important));
      }
    })().catch(error => console.warn('[Companion] feedback initialization failed:', error));
    const interval = window.setInterval(() => { void refresh(); }, 5000);
    return () => { cancelled = true; window.clearInterval(interval); cleanup.forEach(unlisten => unlisten()); };
  }, [pet]);
  return status;
}
