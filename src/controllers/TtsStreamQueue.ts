/**
 * 流式 TTS 切片与串行播报队列
 *
 * 工作流程：
 * 1. `feed(chunk)` 接收 LLM 流式产出的纯文本片段，累积到内部 buffer
 * 2. 在句末标点（。！？!?）或换行符处切分送播放队列；无边界长文本达到上限时强制切分
 * 3. `pump()` 串行调用 `invoke('speak_text')`，每段播放完成后再播下一段
 * 4. `flush()` 在流式结束（chat:done）时把剩余 buffer 送出
 * 5. `stop()` 在取消生成或用户主动停止时清空队列并停止播放
 *
 * 两种显示模式：
 * - 即时模式（用户对话）：文字立即显示，语音跟进播放（文字先于语音 1~3s）
 * - 同步模式（主动/跨角色）：文字在语音真正开始播放时才显示，音画同步
 *
 * 设计要点：
 * - 句级切片：在句末标点处切分，让首句尽快送合成，长段落不再整段等待
 * - 生成与播报并行：LLM 边产出 chunk，TTS 边播放已切好的片段
 * - 串行队列：避免多段音频重叠播放
 * - 队列积压保护：超过 5 段时合并剩余片段，避免延迟过大
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCharacterId } from '../characterContext';

/** buffer 最大字符数（无换行长文本积压保护，超过则强制切片） */
const MAX_BUFFER_CHARS = 200;

/** 队列积压阈值（超过则合并剩余片段） */
const MAX_QUEUE_SIZE = 5;

/** 段落边界（换行符）— 最高优先级切分点 */
const PARAGRAPH_END_RE = /[\n\r]/;

/** 句末标点（中英文句号、问号、叹号）— 句级切分点 */
const SENTENCE_END_CHARS = new Set(['。', '！', '？', '!', '?']);

/** 句级切片的最大字符数（在不超过此范围的最后一个句末标点处切） */
const MAX_SENTENCE_CHARS = 80;

/** 同步模式下等待 tts:started 的超时（毫秒），超时后直接显示文字 */
const SYNC_START_TIMEOUT_MS = 4000;

/** waitForDrain 超时（毫秒）。
 *  后端 speak_text 在 rodio is_playing 卡死时会永久阻塞，连锁导致
 *  flushSync 卡死、气泡不消失、消息不入记忆图谱。60s 兜底覆盖最长单句朗读。 */
const DRAIN_TIMEOUT_MS = 60_000;

/** 表达层信息 — 与后端 Presentation 结构对齐 */
export interface Presentation {
  expression?: string;
  motion?: string;
  gaze?: string;
  bubble: boolean;
  typing_indicator: boolean;
}

/** 流式播放回调（用于同步模式在语音开始时通知显示文字） */
interface StreamCallbacks {
  /** 首句语音开始播放时调用（用于同步模式下此时才显示文字气泡） */
  onFirstAudioStart?: () => void;
}

/**
 * 找到 buffer 中不超过 maxChars 范围的最后一个句末标点位置。
 * 返回 -1 表示未找到。
 */
function findLastSentenceEnd(buffer: string, maxChars: number): number {
  const searchLen = Math.min(buffer.length, maxChars);
  let lastEnd = -1;
  for (let i = 0; i < searchLen; i++) {
    if (SENTENCE_END_CHARS.has(buffer[i])) {
      lastEnd = i;
    }
  }
  return lastEnd;
}

class TtsStreamQueueClass {
  private queue: string[] = [];
  private buffer = '';
  private speaking = false;
  private enabled = false;
  /** 当前流是否已预热(每轮对话重置) */
  private prewarmed = false;
  /** 当前表达层信息(由 ChatController 的 chat:meta 事件设置,随 speak_text 传入后端) */
  private currentPresentation: Presentation | null = null;
  /** tts:started 事件监听器清理函数 */
  private unlistenStarted?: () => void;
  private unlistenError?: () => void;
  private unlistenFinished?: () => void;
  /** 等待 tts:started 的 Promise resolve 回调队列（每次 speak_text 入队一个） */
  private startResolvers: Array<() => void> = [];
  /** 同步模式回调 */
  private streamCbs: StreamCallbacks | null = null;
  /** 首句是否已触发 onFirstAudioStart（单次触发） */
  private firstAudioFired = false;
  /** 事件监听是否已初始化 */
  private eventsInitialized = false;

  constructor() {
    void this.initEvents();
  }

  /** 初始化 tts 事件监听（懒加载，首次构造时注册一次） */
  private async initEvents(): Promise<void> {
    if (this.eventsInitialized) return;
    this.eventsInitialized = true;
    try {
      this.unlistenStarted = await listen<{ character_id?: string }>('tts:started', (event) => {
        const cid = getCharacterId();
        if (event.payload?.character_id && cid && event.payload.character_id !== cid) return;
        this.speaking = true;
        const resolve = this.startResolvers.shift();
        if (resolve) resolve();
        if (!this.firstAudioFired) {
          this.firstAudioFired = true;
          this.streamCbs?.onFirstAudioStart?.();
        }
      });
      this.unlistenError = await listen<{ character_id?: string }>('tts:error', () => {
        while (this.startResolvers.length > 0) {
          const resolve = this.startResolvers.shift();
          if (resolve) resolve();
        }
        this.speaking = false;
        if (!this.firstAudioFired) {
          this.firstAudioFired = true;
          this.streamCbs?.onFirstAudioStart?.();
        }
      });
      // tts:finished 后端播放结束（正常或超时强制中断）。
      // 后端 speak_text 可能因 is_playing 卡死而无法返回，但 emit tts:finished
      // 在 speak_with_context 收尾时仍会触发，借此强制唤醒 waitForDrain。
      this.unlistenFinished = await listen<{ character_id?: string }>('tts:finished', (event) => {
        const cid = getCharacterId();
        if (event.payload?.character_id && cid && event.payload.character_id !== cid) return;
        this.speaking = false;
        while (this.startResolvers.length > 0) {
          const resolve = this.startResolvers.shift();
          if (resolve) resolve();
        }
      });
    } catch {
      /* ignore - events may not be available in test environments */
    }
  }

  /** 设置是否启用流式 TTS（由 TTS 配置 + voiceEnabled 共同决定） */
  setEnabled(enabled: boolean): void {
    this.enabled = enabled;
    if (!enabled) {
      this.stop();
    }
  }

  /** 是否已启用 */
  isEnabled(): boolean {
    return this.enabled;
  }

  /** 是否正在播放 */
  isSpeaking(): boolean {
    return this.speaking;
  }

  /**
   * 设置当前表达层信息(由 ChatController 在 chat:meta 事件时调用)
   *
   * 后端 SpeakIntent 会携带此 presentation,Planner 在真正开始播放时
   * 发射 `presentation:start` 事件,前端据此同步播放表情/动作/气泡。
   */
  setPresentation(meta: { expression: string; motion: string; sticker?: string }): void {
    this.currentPresentation = {
      expression: meta.expression || undefined,
      motion: meta.motion || undefined,
      bubble: false,
      typing_indicator: false,
    };
  }

  /**
   * 喂入一个流式 chunk（LLM 产出的纯文本片段）— 即时模式
   *
   * 句级切片：优先在换行符处切（段落边界），其次在句末标点（。！？!?）处切。
   * 这让长段落中的首句能尽快送合成，无需等整段积累完成。
   * 无任何边界时，达到 MAX_BUFFER_CHARS 强制切分。
   *
   * 即时模式：文字由调用方立即显示，本方法只负责切片入队 TTS。
   */
  feed(chunk: string): void {
    if (!this.enabled || !chunk) return;
    this.buffer += chunk;

    if (!this.prewarmed) {
      this.prewarmed = true;
      const cid = getCharacterId() ?? undefined;
      void invoke('prewarm_tts', { characterId: cid }).catch(() => {});
    }

    this.processBuffer();
  }

  /**
   * 喂入一个流式 chunk — 同步模式
   *
   * 与 feed() 相同的切片逻辑，但在首句 tts:started 时调用 cbs.onFirstAudioStart，
   * 调用方应在此回调中才开始显示文字气泡，实现音画同步。
   */
  feedSync(chunk: string, cbs: StreamCallbacks): void {
    if (!this.enabled || !chunk) return;
    if (!this.streamCbs) {
      this.streamCbs = cbs;
      this.firstAudioFired = false;
    }
    this.buffer += chunk;

    if (!this.prewarmed) {
      this.prewarmed = true;
      const cid = getCharacterId() ?? undefined;
      void invoke('prewarm_tts', { characterId: cid }).catch(() => {});
    }

    this.processBuffer();
  }

  private processBuffer(): void {
    while (this.buffer.length > 0) {
      const brIdx = this.buffer.search(PARAGRAPH_END_RE);
      if (brIdx !== -1) {
        const piece = this.buffer.slice(0, brIdx + 1);
        this.buffer = this.buffer.slice(brIdx + 1);
        this.enqueue(piece);
        continue;
      }

      const sentIdx = findLastSentenceEnd(this.buffer, MAX_SENTENCE_CHARS);
      if (sentIdx !== -1) {
        const piece = this.buffer.slice(0, sentIdx + 1);
        this.buffer = this.buffer.slice(sentIdx + 1);
        this.enqueue(piece);
        continue;
      }

      if (this.buffer.length >= MAX_BUFFER_CHARS) {
        this.enqueue(this.buffer);
        this.buffer = '';
      }
      break;
    }
  }

  /** 流式结束：把剩余 buffer 送出 */
  flush(): void {
    if (!this.enabled) return;
    if (this.buffer.trim()) {
      this.enqueue(this.buffer);
      this.buffer = '';
    }
  }

  /**
   * 同步模式 flush：送出剩余 buffer 并返回 Promise 在语音结束时 resolve
   *
   * 用于同步模式下流式结束后，等待播放完成。
   */
  async flushSync(): Promise<void> {
    if (!this.enabled) return;
    if (this.buffer.trim()) {
      this.enqueue(this.buffer);
      this.buffer = '';
    }
    await this.waitForDrain();
    this.streamCbs = null;
    this.firstAudioFired = false;
  }

  /**
   * 朗读完整文本（非流式，即时模式）。
   * 与流式片段共享同一串行队列。
   */
  speak(text: string): void {
    if (!text) return;
    this.enqueue(text);
  }

  /**
   * 朗读完整文本（同步模式）。
   *
   * 返回 Promise，在语音真正开始播放时 resolve（tts:started 触发）。
   * 有 SYNC_START_TIMEOUT_MS 超时兜底，超时后即使语音未就绪也 resolve，
   * 避免文字永远不显示。
   */
  speakSync(text: string): Promise<void> {
    return new Promise((resolve) => {
      if (!text || !this.enabled) {
        resolve();
        return;
      }

      let resolved = false;
      const doResolve = () => {
        if (!resolved) {
          resolved = true;
          resolve();
        }
      };

      const timeoutId = window.setTimeout(doResolve, SYNC_START_TIMEOUT_MS);
      this.startResolvers.push(() => {
        window.clearTimeout(timeoutId);
        doResolve();
      });
      this.enqueue(text);
    });
  }

  /** 等待队列中所有片段播放完成。
   *  带超时兜底：后端 speak_text 永久阻塞时（如 rodio is_playing 卡死），
   *  超时后强制清空队列并 resolve，避免 flushSync 无限等待导致
   *  气泡不消失、消息不入记忆图谱。 */
  async waitForDrain(): Promise<void> {
    if (this.queue.length === 0 && !this.speaking) return;
    const deadline = Date.now() + DRAIN_TIMEOUT_MS;
    while ((this.queue.length > 0 || this.speaking) && Date.now() < deadline) {
      await new Promise((r) => window.setTimeout(r, 50));
    }
    if (this.queue.length > 0 || this.speaking) {
      console.warn('[TtsStreamQueue] waitForDrain 超时，强制清空队列（避免卡死）');
      this.queue = [];
      this.speaking = false;
      this.startResolvers = [];
      try {
        await invoke('stop_speaking', { characterId: getCharacterId() ?? undefined });
      } catch {
        /* ignore */
      }
    }
  }

  /** 清空队列并停止播放 */
  async stop(): Promise<void> {
    this.queue = [];
    this.buffer = '';
    this.prewarmed = false;
    this.currentPresentation = null;
    this.streamCbs = null;
    this.firstAudioFired = false;
    this.startResolvers = [];
    try {
      await invoke('stop_speaking', { characterId: getCharacterId() ?? undefined });
    } catch {
      /* ignore */
    }
  }

  /** 清空 buffer 但不停止当前播放（用于新一轮对话开始前） */
  resetBuffer(): void {
    this.buffer = '';
    this.queue = [];
    this.prewarmed = false;
    this.currentPresentation = null;
    this.streamCbs = null;
    this.firstAudioFired = false;
    this.startResolvers = [];
  }

  /** 加入播放队列并触发 pump */
  private enqueue(text: string): void {
    const trimmed = text.trim();
    if (!trimmed) return;
    if (!/[\u4e00-\u9fa5a-zA-Z0-9]/.test(trimmed)) return;

    if (this.queue.length >= MAX_QUEUE_SIZE) {
      const merged = this.queue.join('');
      this.queue = [merged];
    }

    this.queue.push(trimmed);
    void this.pump();
  }

  /** 串行播放队列，合成并行流水线 */
  private async pump(): Promise<void> {
    if (this.speaking) return;
    this.speaking = true;
    const cid = getCharacterId() ?? undefined;

    while (this.queue.length > 0) {
      // 预取队列中所有待播句：后端各自独立连接并行合成
      // 已缓存的句子会立即返回，不会重复请求
      for (const text of this.queue) {
        void invoke('prefetch_tts', { text, characterId: cid }).catch(() => {});
      }

      const text = this.queue.shift()!;
      try {
        await invoke('speak_text', {
          text,
          characterId: cid,
          presentation: this.currentPresentation,
        });
      } catch (e) {
        console.warn('[TtsStreamQueue] speak_text 失败:', e);
        const resolve = this.startResolvers.shift();
        if (resolve) resolve();
      }
    }
    this.speaking = false;
  }
}

/** 流式 TTS 队列单例 */
export const TtsStreamQueue = new TtsStreamQueueClass();

export default TtsStreamQueue;
