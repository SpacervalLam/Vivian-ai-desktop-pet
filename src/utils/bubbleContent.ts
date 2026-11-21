import type { StickerRef } from '../types';

/** Outer bubble size in logical pixels; the native window also includes shadow/tail space. */
export const STICKER_BUBBLE_SIZE = 30;
export const STICKER_BUBBLE_DURATION = 4000;

export function bubbleContentKey(settled: ReadonlyArray<{ text: string; sticker?: StickerRef }>, text: string | null): string {
  return JSON.stringify([...settled.map(b => b.sticker
    ? { sticker: [b.sticker.character_id, b.sticker.id, b.sticker.version] }
    : b.text), ...(text ? [text] : [])]);
}
