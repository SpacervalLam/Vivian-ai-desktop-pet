/**
 * 聊天控制器
 *
 * 负责将用户输入分发到后端 `send_message_stream` 命令，订阅
 * `chat:chunk` / `chat:done` / `chat:error` / `chat:cancelled` 事件，
 * 并将结果同步到 zustand store 与 BubbleController。
 *
 * 支持多消息并发：每条消息有独立的 stream_id，事件按 stream_id 路由到
 * 对应的 StreamSession，多个流式回复互不干扰。后端通过 brain_lock 串行化
 * brain.think 调用，保证对话历史/记忆/心理系统不被并发写入污染。
 */

import { invoke } from '@tauri-apps/api/core';
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event';
import { useAppStore } from '../stores/useAppStore';
import { BubbleController, computeDuration } from './BubbleController';
import { StreamController } from './StreamController';
import { TtsStreamQueue } from './TtsStreamQueue';
import type { AiResponse, ChatMessage } from '../types';
import { getCharacterId } from '../characterContext';
import i18n from '../i18n';

export interface ChatHandlers {
  /** 收到流式 chunk 时触发（带 streamId） */
  onChunk?: (text: string, fullText: string, streamId: string) => void;
  /** AI 回复完成时触发（带 streamId） */
  onResponseReceived?: (response: AiResponse, streamId: string) => void;
  /** 开始思考时触发（带 streamId） */
  onThinkingStarted?: (streamId: string) => void;
  /** 出错时触发（带 streamId） */
  onError?: (error: string, streamId: string) => void;
  /** 取消生成时触发（带 streamId） */
  onCancelled?: (streamId: string) => void;
  /** augment 回复（增量记忆补充）触发 */
  onAugmentReply?: (text: string) => void;
  /** 收到 expression/motion/sticker meta 事件（在 text 流式之前触发，用于提前播放 Live2D 动画 + 表情包弹窗） */
  onMeta?: (meta: { expression: string; expressionDurationMs?: number; motion: string; sticker?: string }) => void;
}

/** 单条消息的流式会话状态 */
interface StreamSession {
  id: string;
  /** 累积的流式文本 */
  text: string;
  streamParser: StreamController;
  resolve: (response: AiResponse) => void;
  reject: (error: Error) => void;
  /** 消息渠道：wechat / direct / proactive */
  channel: string;
  /** 已结算到 text 中的字符位置（换行分段用） */
  settledUpTo: number;
  /** Layer 2 即时反应是否已触发（避免重复触发） */
  instantReactLayer2Fired: boolean;
}

/** 生成 stream_id（优先用 crypto.randomUUID，降级到时间戳+随机数） */
function generateStreamId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `s-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

class ChatControllerClass {
  private unlisteners: UnlistenFn[] = [];
  private handlers: ChatHandlers = {};
  /** 活跃的流式会话，按 stream_id 索引 */
  private sessions = new Map<string, StreamSession>();

  /** 设置事件回调 */
  setHandlers(handlers: ChatHandlers): void {
    this.handlers = handlers;
  }

  /** 是否有流式生成正在进行 */
  get isStreaming(): boolean {
    return this.sessions.size > 0;
  }

  /** 缓存最近的 meta 事件（expression/motion/sticker），供 handler 或外部检查 */
  lastMeta: { expression: string; expressionDurationMs?: number; motion: string; sticker?: string } = { expression: '', motion: '' };

  /** 初始化事件监听（应在 App 启动时调用一次） */
  async init(): Promise<void> {
    this.cleanup();
    // chat:meta 事件在 chat:chunk 之前到达，用于提前播放 Live2D 动画
    this.unlisteners.push(
      await listen<{ expression?: string; expression_duration_ms?: number; motion?: string; sticker?: string; stream_id?: string; character_id?: string }>('chat:meta', (event) => {
        // 按 stream_id 过滤：忽略不属于本窗口的 meta 事件，防止其他角色的表情/动作在当前角色 Live2D 上播放
        const metaSid = event.payload.stream_id ?? '';
        if (metaSid && !this.sessions.has(metaSid)) return;
        const meta = {
          expression: event.payload.expression ?? '',
          expressionDurationMs: event.payload.expression_duration_ms,
          motion: event.payload.motion ?? '',
          sticker: event.payload.sticker ?? '',
        };
        this.lastMeta = meta;
        // 同步表达层信息到 TTS 队列,后续 speak_text 调用会携带 presentation
        TtsStreamQueue.setPresentation(meta);
        this.handlers.onMeta?.(meta);
      }),
    );
    // chat:inline_meta 事件在流式输出过程中即时触发（内联标签扫描器剥离 <e>/<m>/<s> 标签），
    // 让表情/动作在文字流式输出过程中即时切换，无需等待 ExpressionMotionRunnable 的第二次 LLM 调用。
    this.unlisteners.push(
      await listen<{ type: string; name: string; duration_ms?: number | null; stream_id?: string; character_id?: string }>('chat:inline_meta', (event) => {
        const metaSid = event.payload.stream_id ?? '';
        if (metaSid && !this.sessions.has(metaSid)) return;
        const { type, name } = event.payload;
        // 将 discriminated 格式映射为 onMeta 的 flat 格式
        const meta = {
          expression: type === 'expression' ? name : '',
          expressionDurationMs: type === 'expression' ? (event.payload.duration_ms ?? undefined) : undefined,
          motion: type === 'motion' ? name : '',
          sticker: type === 'sticker' ? name : undefined,
        };
        this.lastMeta = meta;
        TtsStreamQueue.setPresentation(meta);
        this.handlers.onMeta?.(meta);
      }),
    );
    this.unlisteners.push(
      await listen<{ text: string; stream_id?: string }>('chat:chunk', (event) => {
        const chunk = event.payload.text;
        const sid = event.payload.stream_id ?? '';
        const session = this.sessions.get(sid);
        if (!session) return;
        // 按 stream_id 路由：只累积当前 session 的文本
        session.text += chunk;
        session.streamParser.feed(chunk);
        // 换行分段：检测新换行符，结算已完成段落
        const lastNl = session.text.lastIndexOf('\n');
        if (lastNl >= 0 && lastNl >= session.settledUpTo) {
          const completed = session.text.slice(session.settledUpTo, lastNl + 1).replace(/\n+$/, '');
          session.settledUpTo = lastNl + 1;
          const remaining = session.text.slice(session.settledUpTo);
          if (completed) {
            BubbleController.settleSegment(completed, remaining);
          } else if (remaining) {
            BubbleController.showStreamingBubble(remaining);
          }
        } else {
          // 无新换行：继续流式显示当前段落
          const currentSegment = session.text.slice(session.settledUpTo);
          BubbleController.showStreamingBubble(currentSegment);
        }
        // 流式切片送 TTS 队列（后端串行化保证同一时刻只有一个流产 chunk）
        TtsStreamQueue.feed(chunk);
        this.handlers.onChunk?.(chunk, session.text, sid);

        // Layer 2: AI 文本首段完成时触发即时反应（覆盖 Layer 1）
        // 触发条件：出现换行符 或 累积文本达 40 字符（仅触发一次）
        if (!session.instantReactLayer2Fired) {
          const hasNewline = chunk.includes('\n');
          const textLen = session.text.length;
          if (hasNewline || textLen >= 40) {
            session.instantReactLayer2Fired = true;
            const aiText = session.text.slice(0, 80);
            void this.triggerInstantReact(aiText, undefined, 'ai');
          }
        }
      }),
    );
    this.unlisteners.push(
      await listen<{
        text: string;
        motion?: string;
        expression?: string;
        emotion_score?: number;
        user_emotion?: string;
        stream_id?: string;
      }>('chat:done', (event) => {
        const sid = event.payload.stream_id ?? '';
        const session = this.sessions.get(sid);
        if (!session) return;
        const finalText = event.payload.text || session.text;
        // 流式结束：把 TTS 队列中剩余的 buffer 送出
        TtsStreamQueue.flush();
        // 捕获 LLM 在 JSON 中判定的真实用户情绪，供 proactive tick 使用
        // （不能用 currentMood.primary_emotion，那是 Vivian 自身的 mood）
        const userEmotion = event.payload.user_emotion ?? '';
        if (userEmotion) {
          useAppStore.getState().setLastUserEmotion(userEmotion);
        }
        if (!finalText) {
          // 空文本（LLM 真正返回空内容）时跳过对话历史写入，避免污染记忆
          this.finishSessionEmpty(sid);
          return;
        }
        this.finishSession(sid, finalText, {
          text: finalText,
          motion: event.payload.motion ?? '',
          expression: event.payload.expression ?? '',
          emotion_score: event.payload.emotion_score ?? 0,
        });
      }),
    );
    this.unlisteners.push(
      await listen<{ error: string; stream_id?: string }>('chat:error', (event) => {
        const sid = event.payload.stream_id ?? '';
        if (!this.sessions.has(sid)) return;
        // 出错时停止 TTS 播放，避免继续播放已生成的片段
        void TtsStreamQueue.stop();
        this.finishSessionWithError(sid, event.payload.error);
      }),
    );
    this.unlisteners.push(
      await listen<{ stream_id?: string }>('chat:cancelled', (event) => {
        const sid = event.payload.stream_id ?? '';
        if (!this.sessions.has(sid)) return;
        // 取消生成时停止 TTS 播放
        void TtsStreamQueue.stop();
        this.finishSessionCancelled(sid);
      }),
    );
    // 广播消息：由广播窗口发出，各角色窗口各自通过 sendMessage 走完整流程（session + TTS + 气泡）
    this.unlisteners.push(
      await listen<{ text: string }>('broadcast:send_message', (event) => {
        const text = event.payload.text;
        if (!text) return;
        void this.sendMessage(text, undefined, 'direct');
      }),
    );
  }

  /** 清理事件监听 */
  cleanup(): void {
    for (const un of this.unlisteners) {
      try {
        un();
      } catch {
        /* ignore */
      }
    }
    this.unlisteners = [];
  }

  /**
   * 发送用户消息（流式）。
   *
   * 立即返回 Promise，不阻塞后续发送。多个 sendMessage 可并发调用，
   * 后端会按顺序排队执行，流式输出通过 stream_id 互不干扰。
   *
   * @param message 用户输入文本
   * @param characterId 显式指定目标角色 ID（群发场景使用）；不传则用当前窗口角色身份
   * @param channel 消息渠道（"wechat" 聊天面板可见 / "direct" 仅写入记忆不显示）
   * @returns 完整响应（流式结束后 resolve）
   */
  async sendMessage(message: string, characterId?: string, channel?: string, whisper?: boolean, fileMetadata?: Record<string, unknown>): Promise<AiResponse> {
    const store = useAppStore.getState();
    const targetCharId = characterId ?? getCharacterId() ?? undefined;
    const ch = channel ?? 'wechat';
    // 添加用户消息到历史
    const userMsg: ChatMessage = {
      role: 'user',
      content: message,
      timestamp: new Date().toISOString(),
    };
    // 通知其他窗口（如 ChatWindow）立即追加用户消息
    void emit('chat:user_message', {
      content: message,
      timestamp: userMsg.timestamp,
      character_id: targetCharId,
      channel: ch,
    });
    store.setThinking(true);

    const streamId = generateStreamId();
    this.handlers.onThinkingStarted?.(streamId);

    // Layer 1: 用户消息到达瞬间触发即时情绪反应（不等 AI 回复）
    this.triggerInstantReact(message, targetCharId, 'user');

    return new Promise<AiResponse>((resolve, reject) => {
      const session: StreamSession = {
        id: streamId,
        text: '',
        streamParser: new StreamController(),
        resolve,
        reject,
        channel: ch,
        settledUpTo: 0,
        instantReactLayer2Fired: false,
      };
      this.sessions.set(streamId, session);

      invoke('send_message_stream', { message, streamId, characterId: targetCharId, channel: ch, whisper: whisper ?? false, fileMetadata }).catch((err) => {
        // invoke 本身失败（如命令不存在），直接结束 session
        this.finishSessionWithError(streamId, String(err));
      });
    });
  }

  /** 停止当前生成 */
  async stopGeneration(): Promise<void> {
    try {
      await invoke('stop_generation', { characterId: getCharacterId() ?? undefined });
    } catch (e) {
      console.warn('[ChatController] stop_generation 失败:', e);
    }
  }

  /**
   * 触发醒转交互（从休息/忙碌状态唤醒）。
   *
   * 不写用户消息到前端 store，仅接收 AI 流式回复并展示气泡 + TTS。
   * 后端 wake_from_presence 命令会：
   * 1. 切换 presence 到 Online + 写 presence_log 记忆
   * 2. 构造唤醒语境并走完整 brain.think 流程（心情/表情/记忆/对话历史）
   * 3. 流式 emit chat:meta / chat:chunk / chat:done
   */
  async triggerWakeInteraction(characterId?: string): Promise<AiResponse | null> {
    const store = useAppStore.getState();
    const targetCharId = characterId ?? getCharacterId() ?? undefined;
    const ch = 'direct';
    store.setThinking(true);

    const streamId = generateStreamId();
    this.handlers.onThinkingStarted?.(streamId);

    return new Promise<AiResponse | null>((resolve) => {
      const session: StreamSession = {
        id: streamId,
        text: '',
        streamParser: new StreamController(),
        resolve: resolve as (response: AiResponse) => void,
        reject: () => resolve(null),
        channel: ch,
        settledUpTo: 0,
        instantReactLayer2Fired: false,
      };
      this.sessions.set(streamId, session);

      invoke('wake_from_presence', { characterId: targetCharId, streamId }).catch((err) => {
        this.finishSessionWithError(streamId, String(err));
      });
    });
  }

  /** 显示 AI 对话窗口 */
  showChatWindow(): void {
    useAppStore.getState().setChatOpen(true);
  }

  /** 当所有 session 结束时重置 thinking 状态 */
  private maybeClearThinking(): void {
    if (this.sessions.size === 0) {
      useAppStore.getState().setThinking(false);
    }
  }

  /** 正常完成一个 session */
  private finishSession(sid: string, finalText: string, response: AiResponse): void {
    const session = this.sessions.get(sid);
    if (!session) return;
    const ch = session.channel;
    this.sessions.delete(sid);
    this.maybeClearThinking();
    // 通知其他窗口（如 ChatWindow）立即追加 AI 回复
    const assistantTimestamp = new Date().toISOString();
    void emit('chat:assistant_message', {
      content: finalText,
      timestamp: assistantTimestamp,
      stream_id: sid,
      character_id: getCharacterId() ?? undefined,
      channel: ch,
    });
    // 启动气泡自动关闭：根据文本长度动态计算
    BubbleController.startAutoClose(computeDuration(finalText));
    this.handlers.onResponseReceived?.(response, sid);
    session.resolve(response);
  }

  /** 空文本完成：不写入历史，仅清理 session */
  private finishSessionEmpty(sid: string): void {
    const session = this.sessions.get(sid);
    if (!session) return;
    this.sessions.delete(sid);
    this.maybeClearThinking();
    BubbleController.startAutoClose(3000);
    this.handlers.onResponseReceived?.(
      { text: '', motion: 'idle', expression: '', emotion_score: 0 },
      sid,
    );
    session.resolve({ text: '', motion: 'idle', expression: '', emotion_score: 0 });
  }

  /** 出错完成一个 session */
  private finishSessionWithError(sid: string, error: string): void {
    const session = this.sessions.get(sid);
    if (!session) return;
    this.sessions.delete(sid);
    this.maybeClearThinking();
    // API 错误通过 toast 提示，不写入对话历史、不展示气泡，避免兜底文案污染记忆
    void emit('toast:show', { message: error, type: 'error', duration: 5000, key: Date.now() });
    this.handlers.onError?.(error, sid);
    session.reject(new Error(error));
  }

  /** 取消完成一个 session */
  private finishSessionCancelled(sid: string): void {
    const session = this.sessions.get(sid);
    if (!session) return;
    this.sessions.delete(sid);
    this.maybeClearThinking();
    BubbleController.startAutoClose(3000);
    this.handlers.onCancelled?.(sid);
    // 取消生成：resolve 空响应，避免 Promise 永远挂起
    session.resolve({ text: '', motion: 'idle', expression: '', emotion_score: 0 });
  }

  /** 处理 augment 回复（增量记忆补充） */
  handleAugmentReply(text: string): void {
    if (!text) return;
    BubbleController.appendToBubble(text);
    this.handlers.onAugmentReply?.(text);
  }

  /**
   * 触发即时情绪反应（三层反应系统的 Layer 1/2）
   *
   * 调用后端 analyze_emotion_instant 命令获取低延迟情绪分类结果，
   * 通过 emit chat:instant_react 事件通知前端 useInstantReact hook
   * 立即应用 FACS 参数到 Live2D 模型的 instant 层。
   *
   * 失败时弹 toast 报错（不降级到关键词分析）。
   *
   * @param text 分析文本（用户消息或 AI 回复首段）
   * @param characterId 目标角色 ID
   * @param layer 'user' = Layer 1（用户消息），'ai' = Layer 2（AI 文本首段）
   */
  private async triggerInstantReact(
    text: string,
    characterId: string | undefined,
    layer: 'user' | 'ai',
  ): Promise<void> {
    if (!text || !text.trim()) return;
    try {
      const result = await invoke<{
        emotion: string;
        intensity: number;
        facs: Record<string, number>;
      }>('analyze_emotion_instant', { text, characterId });
      if (!result || !result.facs) return;
      await emit('chat:instant_react', {
        emotion: result.emotion,
        intensity: result.intensity,
        facs: result.facs,
        layer,
        character_id: characterId,
      });
    } catch (e) {
      // 嵌入服务失败：弹 toast 报错，不降级到关键词分析
      const message = typeof e === 'string' ? e : (e as { message?: string })?.message ?? 'unknown';
      await emit('toast:show', {
        message: i18n.t('toast.instant_react_failed', { error: message }),
        type: 'error',
        duration: 5000,
        key: `instant_react_error_${Date.now()}`,
        character_id: characterId,
      });
    }
  }

  /** 从原始 JSON 负载提取文本（工具方法，供未来扩展使用） */
  static extractText(payload: string): string {
    return StreamController.extractTextFromJson(payload);
  }
}

/** 聊天控制器单例 */
export const ChatController = new ChatControllerClass();

export default ChatController;
