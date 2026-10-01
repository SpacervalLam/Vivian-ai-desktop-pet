export const bubbleCharacters = (text: string): string[] => Array.from(text);

export function computeDuration(text: string): number {
  if (!text.trim()) return 3000;
  const words = text.match(/[A-Za-z0-9]+(?:['’-][A-Za-z0-9]+)*/g) ?? [];
  const nonLatin = bubbleCharacters(text).filter((char) => !/[\s\x00-\x7f]/.test(char)).length;
  return Math.max(3000, Math.min(18000, 1200 + nonLatin * 180 + words.length * 280));
}

/** Prefer complete sentences and paragraphs, with a Unicode-safe hard limit. */
export function nextBubbleBoundary(chars: string[], final: boolean): number | null {
  let preferred = 0;
  for (let i = 0; i < Math.min(chars.length, 100); i++) {
    const char = chars[i];
    if (char === '\n' && i > 0) return i + 1;
    if (/[。！？!?；;]/.test(char) || (char === '.' && /\s/.test(chars[i + 1] ?? ''))) {
      if (i + 1 >= 32) {
        let end = i + 1;
        while (end < 100 && /[”’」』）)"'。！？!?]/.test(chars[end] ?? '\u0000')) end++;
        if (end < chars.length || final) return end;
      }
    }
    if (i >= 55 && /[\s，,、：:]/.test(char)) preferred = i + 1;
  }
  if (chars.length >= 100) return preferred || 100;
  return final && chars.length ? chars.length : null;
}
