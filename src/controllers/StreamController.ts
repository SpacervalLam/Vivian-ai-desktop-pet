/**
 * 流式 JSON 解析状态机
 *
 * 将 AI 返回的流式 JSON 块增量解析为可显示文本，提取 text/content 字段。
 * 在 Tauri 版本中后端已经做了 JSON 解析并通过 `chat:chunk` 事件直接推送
 * 纯文本，因此该控制器主要用于前端兼容场景（如未来需要直接对接原始 LLM
 * 流式响应时复用），以及提供 `extractTextFromJson` / `isJsonComplete`
 * 静态工具方法。
 *
 */

export const StreamState = {
  SEARCHING_JSON_START: 'SEARCHING_JSON_START',
  SEARCHING_TEXT_KEY: 'SEARCHING_TEXT_KEY',
  EXTRACTING_TEXT_VALUE: 'EXTRACTING_TEXT_VALUE',
  TEXT_COMPLETE: 'TEXT_COMPLETE',
  NOT_JSON: 'NOT_JSON',
  IN_ARRAY: 'IN_ARRAY',
} as const;

export const StreamSubState = {
  SEARCHING_KEY_START: 'SEARCHING_KEY_START',
  IN_KEY: 'IN_KEY',
  KEY_COMPLETE: 'KEY_COMPLETE',
  SEARCHING_COLON: 'SEARCHING_COLON',
  SEARCHING_VALUE_QUOTE: 'SEARCHING_VALUE_QUOTE',
  SKIPPING_VALUE: 'SKIPPING_VALUE',
} as const;

type StreamStateType = (typeof StreamState)[keyof typeof StreamState];
type StreamSubStateType = (typeof StreamSubState)[keyof typeof StreamSubState];

/**
 * 流式 JSON text 字段提取器。
 *
 * 用法：
 *   const parser = new StreamController();
 *   parser.feed('{"text":"he');
 *   parser.feed('llo"}');
 *   parser.displayText; // 'hello'
 */
export class StreamController {
  private buffer = '';
  private hasContent = false;
  private jsonState: StreamStateType = StreamState.SEARCHING_JSON_START;
  private textContent = '';
  private jsonSearchPos = 0;
  private textEscape = false;
  private keyBuffer = '';
  private textKeySubstate: StreamSubStateType = StreamSubState.SEARCHING_KEY_START;
  private skipBraceCount = 0;
  private skipBracketCount = 0;
  private skipInString = false;
  private skippingNonTarget = false;
  private arrayObjectCount = 0;

  /** 重置所有状态 */
  reset(): void {
    this.buffer = '';
    this.hasContent = false;
    this.jsonState = StreamState.SEARCHING_JSON_START;
    this.textContent = '';
    this.jsonSearchPos = 0;
    this.textEscape = false;
    this.keyBuffer = '';
    this.textKeySubstate = StreamSubState.SEARCHING_KEY_START;
    this.skipBraceCount = 0;
    this.skipBracketCount = 0;
    this.skipInString = false;
    this.skippingNonTarget = false;
    this.arrayObjectCount = 0;
  }

  /** 喂入一个数据块，返回当前可显示的文本 */
  feed(chunk: string): string {
    if (!chunk) return '';
    this.buffer += chunk;
    this.hasContent = true;
    return this.extractTextSync(this.buffer);
  }

  get rawBuffer(): string {
    return this.buffer;
  }

  get displayText(): string {
    if (this.jsonState === StreamState.TEXT_COMPLETE) return this.textContent;
    if (this.jsonState === StreamState.NOT_JSON) return this.buffer;
    if (this.jsonState === StreamState.EXTRACTING_TEXT_VALUE && this.textContent) {
      return this.textContent;
    }
    return '';
  }

  get isTextComplete(): boolean {
    return this.jsonState === StreamState.TEXT_COMPLETE;
  }

  get isNotJson(): boolean {
    return this.jsonState === StreamState.NOT_JSON;
  }

  get hasAnyContent(): boolean {
    return this.hasContent;
  }

  private extractTextSync(buffer: string): string {
    if (!buffer) return '';
    if (this.jsonState === StreamState.TEXT_COMPLETE) return this.textContent;
    if (this.jsonState === StreamState.NOT_JSON) return buffer;

    let i = this.jsonSearchPos;
    while (i < buffer.length) {
      const char = buffer[i];

      if (this.jsonState === StreamState.SEARCHING_JSON_START) {
        if (char === '{') {
          this.jsonState = StreamState.SEARCHING_TEXT_KEY;
          this.textKeySubstate = StreamSubState.SEARCHING_KEY_START;
          this.keyBuffer = '';
          i += 1;
          continue;
        } else if (char === '[') {
          this.jsonState = StreamState.IN_ARRAY;
          this.arrayObjectCount = 1;
          i += 1;
          continue;
        } else if (!/\s/.test(char)) {
          this.jsonState = StreamState.NOT_JSON;
          return buffer;
        } else {
          if (i > 20) {
            this.jsonState = StreamState.NOT_JSON;
            return buffer;
          }
        }
        i += 1;
      } else if (this.jsonState === StreamState.IN_ARRAY) {
        if (char === '{' && this.arrayObjectCount === 1) {
          this.jsonState = StreamState.SEARCHING_TEXT_KEY;
          this.textKeySubstate = StreamSubState.SEARCHING_KEY_START;
          this.keyBuffer = '';
          i += 1;
          continue;
        } else if (char === '[') {
          this.arrayObjectCount += 1;
        } else if (char === ']') {
          this.arrayObjectCount -= 1;
          if (this.arrayObjectCount === 0) {
            this.jsonState = StreamState.NOT_JSON;
            return buffer;
          }
        } else if (char === '}') {
          this.textKeySubstate = StreamSubState.SEARCHING_KEY_START;
          this.keyBuffer = '';
        }
        i += 1;
      } else if (this.jsonState === StreamState.SEARCHING_TEXT_KEY) {
        i = this.processSearchingTextKey(buffer, i, char);
      } else if (this.jsonState === StreamState.EXTRACTING_TEXT_VALUE) {
        if (this.textEscape) {
          this.textContent += char;
          this.textEscape = false;
          i += 1;
          continue;
        }
        if (char === '\\') {
          this.textEscape = true;
          i += 1;
          continue;
        }
        if (char === '"') {
          this.jsonState = StreamState.TEXT_COMPLETE;
          i += 1;
          continue;
        }
        this.textContent += char;
        i += 1;
      } else {
        i += 1;
      }
    }

    this.jsonSearchPos = i;
    // 通过 string 比较绕过 TypeScript 基于控制流的类型收窄
    // （循环中状态可能变更为 NOT_JSON，但 TS 静态分析无法识别）
    if ((this.jsonState as string) === StreamState.NOT_JSON) return buffer;
    return this.textContent;
  }

  private processSearchingTextKey(buffer: string, i: number, char: string): number {
    const substate = this.textKeySubstate;

    if (substate === StreamSubState.SEARCHING_KEY_START) {
      if (char === '"') {
        this.textKeySubstate = StreamSubState.IN_KEY;
        this.keyBuffer = '';
        return i + 1;
      } else if (char === '}' || char === ',' || char === ' ' || char === '\t' || char === '\n' || char === '\r') {
        return i + 1;
      } else {
        this.jsonState = StreamState.NOT_JSON;
        return i + 1;
      }
    }

    if (substate === StreamSubState.IN_KEY) {
      if (char === '\\') {
        if (i + 1 >= buffer.length) return i;
        this.keyBuffer += char;
        return i + 2;
      }
      if (char === '"') {
        this.textKeySubstate = StreamSubState.KEY_COMPLETE;
        const keyName = this.keyBuffer;
        this.keyBuffer = '';
        if (keyName === 'text' || keyName === 'content') {
          this.textKeySubstate = StreamSubState.SEARCHING_COLON;
          return i + 1;
        }
        this.textKeySubstate = StreamSubState.SEARCHING_COLON;
        this.skipBraceCount = 0;
        this.skipBracketCount = 0;
        this.skipInString = false;
        this.skippingNonTarget = true;
        return i + 1;
      }
      this.keyBuffer += char;
      return i + 1;
    }

    if (substate === StreamSubState.SEARCHING_COLON) {
      if (char === ' ' || char === '\t' || char === '\n' || char === '\r') return i + 1;
      if (char === ':') {
        this.textKeySubstate = StreamSubState.SEARCHING_VALUE_QUOTE;
        return i + 1;
      }
      this.textKeySubstate = StreamSubState.SEARCHING_KEY_START;
      return i + 1;
    }

    if (substate === StreamSubState.SEARCHING_VALUE_QUOTE) {
      if (char === ' ' || char === '\t' || char === '\n' || char === '\r') return i + 1;
      if (this.skippingNonTarget) {
        if (char === '"') {
          this.textKeySubstate = StreamSubState.SKIPPING_VALUE;
          this.skipInString = true;
          return i + 1;
        } else if (char === '{') {
          this.textKeySubstate = StreamSubState.SKIPPING_VALUE;
          this.skipBraceCount = 1;
          return i + 1;
        } else if (char === '[') {
          this.textKeySubstate = StreamSubState.SKIPPING_VALUE;
          this.skipBracketCount = 1;
          return i + 1;
        }
        this.textKeySubstate = StreamSubState.SKIPPING_VALUE;
        return i + 1;
      }
      if (char === '"') {
        this.jsonState = StreamState.EXTRACTING_TEXT_VALUE;
        this.textEscape = false;
        return i + 1;
      }
      this.textKeySubstate = StreamSubState.SEARCHING_KEY_START;
      return i + 1;
    }

    if (substate === StreamSubState.SKIPPING_VALUE) {
      return this.processSkippingValue(i, char);
    }

    return i + 1;
  }

  private processSkippingValue(i: number, char: string): number {
    if (this.skipInString) {
      if (char === '"' && !this.isEscapedAt(i)) {
        this.skipInString = false;
        if (this.skipBraceCount === 0 && this.skipBracketCount === 0) {
          this.textKeySubstate = StreamSubState.SEARCHING_KEY_START;
          this.skippingNonTarget = false;
        }
      }
      return i + 1;
    }
    if (char === '"') {
      this.skipInString = true;
      return i + 1;
    }
    if (char === '{') {
      this.skipBraceCount += 1;
      return i + 1;
    }
    if (char === '}') {
      this.skipBraceCount -= 1;
      if (this.skipBraceCount === 0 && this.skipBracketCount === 0) {
        this.textKeySubstate = StreamSubState.SEARCHING_KEY_START;
        this.skippingNonTarget = false;
      }
      return i + 1;
    }
    if (char === '[') {
      this.skipBracketCount += 1;
      return i + 1;
    }
    if (char === ']') {
      this.skipBracketCount -= 1;
      if (this.skipBraceCount === 0 && this.skipBracketCount === 0) {
        this.textKeySubstate = StreamSubState.SEARCHING_KEY_START;
        this.skippingNonTarget = false;
      }
      return i + 1;
    }
    if (char === ',' && this.skipBraceCount === 0 && this.skipBracketCount === 0) {
      this.textKeySubstate = StreamSubState.SEARCHING_KEY_START;
      this.skippingNonTarget = false;
      return i + 1;
    }
    return i + 1;
  }

  private isEscapedAt(_i: number): boolean {
    // 简化处理：仅用于 skip 模式下，不严格回溯反斜杠
    return false;
  }

  /** 从完整 JSON 负载中提取文本字段 */
  static extractTextFromJson(payload: string): string {
    try {
      const data = JSON.parse(payload);
      const extract = (obj: unknown): string => {
        if (obj && typeof obj === 'object') {
          const rec = obj as Record<string, unknown>;
          for (const key of ['text', 'content']) {
            const v = rec[key];
            if (typeof v === 'string' && v.trim()) return v.trim();
          }
          for (const value of Object.values(rec)) {
            if (value && typeof value === 'object') {
              const r = extract(value);
              if (r) return r;
            }
          }
          return '';
        }
        if (Array.isArray(obj)) {
          for (const item of obj) {
            const r = extract(item);
            if (r) return r;
          }
        }
        return '';
      };
      return extract(data);
    } catch {
      return '';
    }
  }

  /** 检查文本是否包含完整 JSON 结构 */
  static isJsonComplete(text: string): boolean {
    if (!text) return false;
    const openChar = text.startsWith('{') ? '{' : text.startsWith('[') ? '[' : null;
    if (!openChar) return false;
    const closeChar = openChar === '{' ? '}' : ']';
    let count = 0;
    let inString = false;
    let escape = false;
    for (const char of text) {
      if (escape) {
        escape = false;
        continue;
      }
      if (char === '\\') {
        escape = true;
        continue;
      }
      if (char === '"') {
        inString = !inString;
        continue;
      }
      if (!inString) {
        if (char === openChar) count += 1;
        else if (char === closeChar) {
          count -= 1;
          if (count === 0) return true;
        }
      }
    }
    return count === 0;
  }
}

export default StreamController;
