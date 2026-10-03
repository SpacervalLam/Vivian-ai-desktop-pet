import { useAppStore } from '../stores/useAppStore';
import { stripMarkdown } from '../utils/stripMarkdown';
import { stripActions } from '../utils/ActionText';
import { bubbleCharacters, computeDuration, nextBubbleBoundary } from '../utils/bubbleText';
import { STICKER_BUBBLE_DURATION } from '../utils/bubbleContent';
import type { StickerRef } from '../types';

export interface BubbleOptions {
  crossCharacter?: boolean;
  listenerName?: string;
  sticker?: StickerRef;
}

/** A single reveal queue owns segmentation and dwell; the bubble window only renders. */
export class BubbleControllerClass {
  private streamingBubble = false;
  private targetText = '';
  private consumed = 0;
  private revealed = 0;
  private raf: number | null = null;
  private lastTs: number | null = null;
  private elapsed = 0;
  private pauseUntil = 0;
  private segmentRead = false;
  private finalDuration?: number;
  private closeTimer: ReturnType<typeof setTimeout> | null = null;
  private fallbackTimer: ReturnType<typeof setTimeout> | null = null;
  private settledTimers = new Map<number, ReturnType<typeof setTimeout>>();
  private settledId = 0;
  private pendingSticker?: StickerRef;
  private bubbleGeneration = 0;
  private speechHeld = false;

  /** A completion from an older message cannot release the new bubble. */
  holdForSpeech(): () => void {
    const generation = this.bubbleGeneration;
    this.speechHeld = true;
    if (this.closeTimer) clearTimeout(this.closeTimer);
    this.closeTimer = null;
    let released = false;
    return () => {
      if (released || generation !== this.bubbleGeneration) return;
      released = true;
      this.speechHeld = false;
      this.startTypewriter();
    };
  }

  get currentBubble(): string | null { return useAppStore.getState().currentBubble; }
  get hasActiveBubble(): boolean { return this.currentBubble !== null || useAppStore.getState().settledBubbles.length > 0; }
  get isStreaming(): boolean { return this.streamingBubble; }

  private clean(text: string): string {
    return stripActions(stripMarkdown(text).replace(/[（(][^）)]*$/, ''));
  }

  showBubble(text: string, durationMs?: number, options?: BubbleOptions): void {
    this.closeAll();
    this.targetText = this.clean(text);
    this.finalDuration = durationMs;
    this.setOptions(options);
    this.pendingSticker = options?.sticker;
    if (!this.targetText && this.pendingSticker) { this.revealSticker(); return; }
    useAppStore.setState({ currentBubble: '' });
    this.startTypewriter();
  }

  private setOptions(options?: BubbleOptions): void {
    useAppStore.setState({
      bubbleCrossCharacter: !!options?.crossCharacter,
      bubbleListenerName: options?.listenerName ?? null,
    });
  }

  showStreamingBubble(text: string, options?: BubbleOptions): void {
    if (!text) return;
    if (!this.streamingBubble) {
      this.closeAll();
      this.streamingBubble = true;
      this.setOptions(options);
      useAppStore.setState({ currentBubble: '' });
    } else if (options) this.setOptions(options);
    this.targetText = this.clean(text);
    this.revealed = Math.min(this.revealed, Math.max(0, bubbleCharacters(this.targetText).length - this.consumed));
    this.startTypewriter();
    if (this.fallbackTimer) clearTimeout(this.fallbackTimer);
    this.fallbackTimer = setTimeout(() => this.startAutoClose(), 30000);
  }

  /** Drain the reveal queue before timing the final segment. */
  finishStreaming(text: string, options?: BubbleOptions): void {
    if (!this.streamingBubble) { this.showBubble(text, undefined, options); return; }
    this.targetText = this.clean(text);
    if (options) this.setOptions(options);
    this.pendingSticker = options?.sticker;
    this.startAutoClose();
  }

  startAutoClose(durationMs?: number): void {
    this.streamingBubble = false;
    this.finalDuration = durationMs;
    if (this.fallbackTimer) clearTimeout(this.fallbackTimer);
    this.fallbackTimer = null;
    this.startTypewriter();
  }

  updateBubble(text: string): void {
    if (!this.hasActiveBubble) return;
    if (this.closeTimer) clearTimeout(this.closeTimer);
    this.closeTimer = null;
    this.targetText = this.clean(text);
    this.consumed = this.revealed = 0;
    this.pauseUntil = 0;
    this.segmentRead = false;
    this.startTypewriter();
  }

  appendToBubble(text: string, separator = '\n\n'): boolean {
    if (!this.hasActiveBubble || !text) return false;
    this.updateBubble(this.targetText + separator + text);
    return true;
  }

  /** A sticker has its own dwell and never enters the typewriter or spoken text. */
  showSticker(sticker: StickerRef, options?: BubbleOptions): void {
    this.showBubble('', undefined, { ...options, sticker });
  }

  private revealSticker(): void {
    const sticker = this.pendingSticker;
    if (!sticker) return;
    this.pendingSticker = undefined;
    const id = ++this.settledId;
    useAppStore.getState().addSettledBubble({ id, text: '', sticker, duration: STICKER_BUBBLE_DURATION });
    this.settledTimers.set(id, setTimeout(() => {
      useAppStore.getState().removeSettledBubble(id);
      this.settledTimers.delete(id);
      if (!this.hasActiveBubble) this.closeAll();
    }, STICKER_BUBBLE_DURATION));
  }

  private startTypewriter(): void {
    if (this.raf !== null || this.closeTimer !== null) return;
    const step = (ts: number) => {
      this.raf = null;
      const dt = this.lastTs === null ? 0 : Math.min(64, ts - this.lastTs);
      this.lastTs = ts;
      const allChars = bubbleCharacters(this.targetText);
      if (this.revealed === 0) {
        while (/\s/.test(allChars[this.consumed] ?? '\u0000')) this.consumed++;
      }
      const remaining = allChars.slice(this.consumed);
      const boundary = nextBubbleBoundary(remaining, !this.streamingBubble);
      const length = boundary ?? remaining.length;
      if (!this.speechHeld && !this.streamingBubble && this.revealed >= length && remaining.length <= length) this.revealSticker();
      if (this.segmentRead && this.revealed < length) {
        this.segmentRead = false;
        this.pauseUntil = 0;
      }
      if (ts >= this.pauseUntil) {
        if (this.segmentRead && boundary !== null && remaining.length > length) {
          const text = remaining.slice(0, length).join('').trim();
          const id = ++this.settledId;
          // Full reading dwell already elapsed; briefly retain the previous bubble as context.
          useAppStore.getState().addSettledBubble({ id, text, duration: 1200 });
          this.settledTimers.set(id, setTimeout(() => {
            useAppStore.getState().removeSettledBubble(id);
            this.settledTimers.delete(id);
          }, 1200));
          this.consumed += length;
          this.revealed = 0;
          this.elapsed = 0;
          this.segmentRead = false;
          useAppStore.setState({ currentBubble: '' });
        } else if (this.revealed < length) {
          const interval = Math.max(14, 30 - (length - this.revealed) / 5);
          this.elapsed = Math.min(96, this.elapsed + dt);
          const count = Math.min(4, Math.floor(this.elapsed / interval));
          if (count) {
            this.elapsed -= count * interval;
            this.revealed = Math.min(length, this.revealed + count);
            useAppStore.setState({ currentBubble: remaining.slice(0, this.revealed).join('') });
          }
        } else if (boundary !== null && !this.segmentRead) {
          this.segmentRead = true;
          this.pauseUntil = ts + (remaining.length <= length && this.finalDuration && this.finalDuration > 0
            ? this.finalDuration : computeDuration(remaining.slice(0, length).join('')));
        } else if (!this.streamingBubble && remaining.length <= length) {
          this.lastTs = null;
          if (this.speechHeld) return;
          this.closeTimer = setTimeout(() => {
            this.closeTimer = null;
            if (useAppStore.getState().settledBubbles.some(bubble => bubble.sticker)) {
              useAppStore.setState({ currentBubble: null });
            } else this.closeAll();
          }, remaining.length ? 0 : 3000);
          return;
        }
      }
      if (this.streamingBubble && this.revealed >= remaining.length
        && (boundary === null || (this.segmentRead && ts >= this.pauseUntil))) {
        this.lastTs = null;
        return;
      }
      this.raf = requestAnimationFrame(step);
    };
    this.raf = requestAnimationFrame(step);
  }

  closeAll(): void {
    this.bubbleGeneration++;
    this.speechHeld = false;
    if (this.raf !== null) cancelAnimationFrame(this.raf);
    if (this.closeTimer) clearTimeout(this.closeTimer);
    if (this.fallbackTimer) clearTimeout(this.fallbackTimer);
    for (const timer of this.settledTimers.values()) clearTimeout(timer);
    this.settledTimers.clear();
    this.raf = null;
    this.closeTimer = this.fallbackTimer = null;
    this.lastTs = null;
    this.elapsed = this.pauseUntil = this.consumed = this.revealed = 0;
    this.segmentRead = this.streamingBubble = false;
    this.targetText = '';
    this.finalDuration = undefined;
    this.pendingSticker = undefined;
    useAppStore.getState().clearBubbleTimer();
    useAppStore.setState({ currentBubble: null, settledBubbles: [], bubbleCrossCharacter: false, bubbleListenerName: null });
  }
}

export const BubbleController = new BubbleControllerClass();
export { computeDuration };
export default BubbleController;
