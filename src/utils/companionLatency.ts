import { invoke } from '@tauri-apps/api/core';

// Local-window clocks only. No message text, identity, or cross-window subtraction.
const sent = new Map<string, number>();
const paragraphs = new Map<string, { stream: string; started: number }>();
export function beginCompanionTurn(stream: string): void {
  const now = performance.now();
  for (const [id, started] of sent) if (now - started > 300_000) sent.delete(id);
  if (sent.size >= 128) sent.delete(sent.keys().next().value!);
  sent.set(stream, now);
}
export function companionParagraphId(stream: string, message: string): string {
  const started = sent.get(stream);
  if (started !== undefined) {
    sent.delete(stream);
    if (paragraphs.size >= 128) paragraphs.delete(paragraphs.keys().next().value!);
    paragraphs.set(message, { stream, started });
    void invoke('observe_companion_latency', { stage: 'paragraph_ready', elapsedMs: performance.now() - started }).catch(() => {});
  }
  return message;
}
export function companionParagraphCommitted(message: string, element: HTMLElement | null): void {
  const timing = paragraphs.get(message);
  if (!timing) return;
  paragraphs.delete(message);
  if (!element || document.visibilityState !== 'visible') return;
  requestAnimationFrame(() => requestAnimationFrame(() => {
    const bounds = element.getBoundingClientRect();
    if (document.visibilityState !== 'visible' || !element.isConnected || !bounds.width || !bounds.height || bounds.bottom <= 0 || bounds.top >= innerHeight) return;
    void invoke('observe_companion_latency', { stage: 'paragraph_painted', elapsedMs: performance.now() - timing.started }).catch(() => {});
  }));
}
