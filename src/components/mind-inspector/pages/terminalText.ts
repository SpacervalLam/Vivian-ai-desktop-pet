import type { IBuffer } from '@xterm/xterm';

/** Capture the parsed screen and scrollback, joining visually wrapped rows. */
export function terminalScreenText(buffer: Pick<IBuffer, 'length' | 'getLine'>): string {
  const lines: string[] = [];
  let chars = 0;
  for (let i = buffer.length - 1; i >= 0 && chars < 40_000; i--) {
    const wraps = !!buffer.getLine(i + 1)?.isWrapped;
    const text = buffer.getLine(i)?.translateToString(!wraps) ?? '';
    lines.push(text + (wraps ? '' : '\n'));
    chars += text.length + 1;
  }
  return lines.reverse().join('').trimEnd();
}

export function terminalQuote(title: string, cwd: string, id: string, selection: string): string {
  return `\n\n${title} · ${cwd} · ${id}\n${selection.replace(/\r\n?/g, '\n').split('\n').map(line => `> ${line}`).join('\n')}\n\n`;
}
