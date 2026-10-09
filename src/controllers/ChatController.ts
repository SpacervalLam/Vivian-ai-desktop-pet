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
import { BubbleController } from './BubbleController';
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
  /** 收到 expression/motion meta 事件（在 text 流式之前触发，用于提前播放 桌宠动画） */
  onMeta?: (meta: { expression: string; expressionDurationMs?: number; motion: string }) => void;
}

/** 单条消息的流式会话状态 */
interface StreamSession {
  id: string;
  characterId?: string;
  /** 累积的流式文本 */
  text: string;
  streamParser: StreamController;
  resolve: (response: AiResponse) => void;
  reject: (error: Error) => void;
  /** 消息渠道：wechat / direct / proactive */
  channel: string;
  /** Layer 2 即时反应是否已触发（避免重复触发） */
  instantReactLayer2Fired: boolean;
  /** 当前会话是否已实际创建流式气泡（避免误用上一条回复的气泡） */
  bubbleStarted: boolean;
  presentation?: { expression: string; motion: string; expressionDurationMs?: number };
  /** 看门狗定时器：长时间无任何进展时兜底结算这条流 */
  watchdogTimer?: number;
}

/**
 * 流式看门狗超时（毫秒）。
 *
 * 后端正常/异常路径都会发终态事件，但一旦后端卡死（抢不到 think_lock、或 think 内部
 * 死锁），就什么都不会发。此时若前端不兜底，界面会永远停在「思考中」、桌宠也会一直
 * 循环思考动作。取值要明显大于一次正常回复（含工具调用的长回合）。
 */
const STREAM_IDLE_TIMEOUT_MS = 180_000;

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

  /** 缓存最近的 meta 事件（expression/motion），供 handler 或外部检查 */
  lastMeta: { expression: string; expressionDurationMs?: number; motion: string } = { expression: '', motion: '' };

  /** 初始化事件监听（应在 App 启动时调用一次） */
  async init(): Promise<void> {
    this.cleanup();
    // Backend-owned image turns share the same bubble, expression and TTS stream lifecycle.
    this.unlisteners.push(await listen<{ stream_id: string; character_id?: string; channel?: string; source?: string }>('chat:start', event => {
      const { stream_id: id, character_id: characterId, channel, source } = event.payload;
      if (source !== 'image' || !id || characterId !== getCharacterId() || this.sessions.has(id)) return;
      this.sessions.set(id, {
        id, characterId, channel: channel ?? 'wechat', text: '', streamParser: new StreamController(),
        resolve: () => {}, reject: () => {}, instantReactLayer2Fired: false, bubbleStarted: false,
      });
      this.armWatchdog(id);
      this.handlers.onThinkingStarted?.(id);
    }));
    // chat:meta 事件在 chat:chunk 之前到达，用于提前播放 桌宠动画
    this.unlisteners.push(
      await listen<{ expression?: string; expression_duration_ms?: number; motion?: string; stream_id?: string; character_id?: string }>('chat:meta', (event) => {
        // 按 stream_id 过滤：忽略不属于本窗口的 meta 事件，防止其他角色的表情/动作在当前角色桌宠上播放
        const metaSid = event.payload.stream_id ?? '';
        const session = this.sessions.get(metaSid);
        if (!session) return;
        const meta = {
          expression: event.payload.expression ?? '',
          expressionDurationMs: event.payload.expression_duration_ms,
          motion: event.payload.motion ?? '',
        };
        session.presentation = {
          expression: meta.expression || session.presentation?.expression || '',
          motion: meta.motion || session.presentation?.motion || '',
          expressionDurationMs: meta.expressionDurationMs ?? session.presentation?.expressionDurationMs,
        };
        this.lastMeta = session.presentation;
        // meta 先于正文到达，也算「后端在推进」：重置空闲看门狗
        this.armWatchdog(metaSid);
        TtsStreamQueue.beginStream(metaSid, session.characterId);
        // 同步表达层信息到 TTS 队列,后续 speak_text 调用会携带 presentation
        TtsStreamQueue.setPresentation(meta);
        if (!TtsStreamQueue.isEnabled()) this.handlers.onMeta?.(meta);
      }),
    );
    let pendingPresentation: Parameters<NonNullable<ChatHandlers['onMeta']>>[0] | undefined;
    const applyPendingPresentation = () => {
      if (!pendingPresentation) return;
      this.handlers.onMeta?.(pendingPresentation);
      pendingPresentation = undefined;
    };
    this.unlisteners.push(await listen<{ speaker_id: string; presentation?: { expression?: string; motion?: string; expression_duration_ms?: number } }>('presentation:start', event => {
      if (event.payload.speaker_id !== getCharacterId()) return;
      const presentation = event.payload.presentation;
      pendingPresentation = presentation ? { expression: presentation.expression ?? '', motion: presentation.motion ?? '', expressionDurationMs: presentation.expression_duration_ms } : undefined;
      // Planner start precedes synthesis; apply expression at actual audio start.
      if (!TtsStreamQueue.isEnabled()) applyPendingPresentation();
    }));
    for (const name of ['tts:started', 'tts:error']) {
      this.unlisteners.push(await listen<{ character_id?: string }>(name, event => {
        if (event.payload.character_id && event.payload.character_id !== getCharacterId()) return;
        applyPendingPresentation();
      }));
    }
    this.unlisteners.push(await listen<{ speaker_id: string }>('presentation:stop', event => {
      if (event.payload.speaker_id === getCharacterId()) pendingPresentation = undefined;
    }));
    // chat:inline_meta 事件在流式输出过程中即时触发（内联标签扫描器剥离 <e>/<m> 标签），
    // 让表情/动作在文字流式输出过程中即时切换，无需等待 ExpressionMotionRunnable 的第二次 LLM 调用。
    this.unlisteners.push(
      await listen<{ type: string; name: string; duration_ms?: number | null; stream_id?: string; character_id?: string }>('chat:inline_meta', (event) => {
        const metaSid = event.payload.stream_id ?? '';
        const session = this.sessions.get(metaSid);
        if (!session) return;
        const { type, name } = event.payload;
        // 将 discriminated 格式映射为 onMeta 的 flat 格式
        const meta = {
          expression: type === 'expression' ? name : '',
          expressionDurationMs: type === 'expression' ? (event.payload.duration_ms ?? undefined) : undefined,
          motion: type === 'motion' ? name : '',
        };
        session.presentation = {
          expression: meta.expression || session.presentation?.expression || '',
          motion: meta.motion || session.presentation?.motion || '',
          expressionDurationMs: meta.expressionDurationMs ?? session.presentation?.expressionDurationMs,
        };
        this.lastMeta = session.presentation;
        // meta 先于正文到达，也算「后端在推进」：重置空闲看门狗
        this.armWatchdog(metaSid);
        TtsStreamQueue.beginStream(metaSid, session.characterId);
        TtsStreamQueue.setPresentation(meta);
        if (!TtsStreamQueue.isEnabled()) this.handlers.onMeta?.(meta);
      }),
    );
    this.unlisteners.push(
      await listen<{ text: string; stream_id?: string }>('chat:chunk', (event) => {
        const chunk = event.payload.text;
        const sid = event.payload.stream_id ?? '';
        const session = this.sessions.get(sid);
        if (!session) return;
        TtsStreamQueue.beginStream(sid, session.characterId);
        // 有 chunk 说明后端在正常推进：重置空闲看门狗
        this.armWatchdog(sid);
        // 按 stream_id 路由：只累积当前 session 的文本
        session.text += chunk;
        session.streamParser.feed(chunk);
        if (!session.bubbleStarted) BubbleController.closeAll();
        BubbleController.showStreamingBubble(session.text);
        session.bubbleStarted = true;
        // 流式切片送 TTS 队列（后端串行化保证同一时刻只有一个流产 chunk）
        TtsStreamQueue.feed(chunk);
        this.handlers.onChunk?.(chunk, session.text, sid);

        // Layer 2: AI 文本首段完成时触发即时反应（覆盖 Layer 1）
        // 触发条件：出现换行符 或 累积文本达 40 字符（仅触发一次）
        if (!session.instantReactLayer2Fired && !session.presentation?.expression) {
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
        sticker?: AiResponse['sticker'];
        user_emotion?: string;
        stream_id?: string;
      }>('chat:done', (event) => {
        const sid = event.payload.stream_id ?? '';
        const session = this.sessions.get(sid);
        if (!session) return;
        const finalText = event.payload.text || session.text;
        TtsStreamQueue.beginStream(sid, session.characterId);
        if (session.presentation) TtsStreamQueue.setPresentation(session.presentation);
        if (!session.text.trim() && finalText.trim()) {
          TtsStreamQueue.setPresentation({ expression: event.payload.expression ?? session.presentation?.expression ?? '', motion: event.payload.motion ?? session.presentation?.motion ?? '' });
          TtsStreamQueue.feed(finalText);
        }
        if (!TtsStreamQueue.isEnabled() && (event.payload.expression || event.payload.motion)) {
          this.handlers.onMeta?.({ expression: event.payload.expression ?? '', motion: event.payload.motion ?? '' });
        }
        // 流式结束：把 TTS 队列中剩余的 buffer 送出
        TtsStreamQueue.flush();
        // 捕获 LLM 在 JSON 中判定的真实用户情绪，供 proactive tick 使用
        // （不能用 currentMood.primary_emotion，那是 Vivian 自身的 mood）
        const userEmotion = event.payload.user_emotion ?? '';
        if (userEmotion) {
          useAppStore.getState().setLastUserEmotion(userEmotion);
        }
        if (!finalText && !event.payload.sticker) {
          // 空文本（LLM 真正返回空内容）时跳过对话历史写入，避免污染记忆
          this.finishSessionEmpty(sid);
          return;
        }
        this.finishSession(sid, finalText, {
          text: finalText,
          sticker: event.payload.sticker,
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
    // These terminal paths do not emit chat:done; settle their sessions as well.
    for (const eventName of ['chat:config_error', 'chat:presence_blocked', 'chat:yielded']) {
      this.unlisteners.push(await listen<{ stream_id?: string }>(eventName, event => {
        this.finishSessionCancelled(event.payload.stream_id ?? '');
      }));
    }
    // 广播消息：由广播窗口发出，各角色窗口各自通过 sendMessage 走完整流程（session + TTS + 气泡）。
    //
    // 渠道必须是 'broadcast' 而不是 'direct'：
    // 广播的语义是「用户只说了一遍、当众对所有人说的」，每个角色是同时被搭话的听众之一。
    // 若复用 'direct'，后端会把这句话当成对该角色的当面私聊，并因为 channel=='direct'
    // 触发第三者旁观路径——其他角色会收到「你刚听到用户和 X 的对话」，于是两个角色
    // 各自都认定"用户把同一句话分别跟我说了一遍"。'broadcast' 渠道专治这个误判。
    this.unlisteners.push(
      await listen<{ text: string }>('broadcast:send_message', (event) => {
        const text = event.payload.text;
        if (!text) return;
        void this.sendMessage(text, undefined, 'broadcast');
      }),
    );
    // 广播图片：与文本广播同源（InputDialog 群发模式发出）。
    // 必须显式传本窗口的角色 id —— send_image_message 在 characterId 为空时回退到
    // 全局 active_character_id，那会让所有角色窗口都往同一个角色身上发。
    this.unlisteners.push(
      await listen<{ sourcePath: string }>('broadcast:image_message', (event) => {
        const sourcePath = event.payload?.sourcePath;
        if (!sourcePath) return;
        void invoke('send_image_message', {
          sourcePath,
          characterId: getCharacterId() ?? undefined,
          channel: 'broadcast',
        }).catch((err) => {
          console.warn('[ChatController] 广播图片发送失败:', err);
        });
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
   * @param channel 消息渠道（"wechat" 聊天面板可见 / "direct" 面对面 / "broadcast" 当众广播）
   * @returns 完整响应（流式结束后 resolve）
   */
  async sendMessage(message: string, characterId?: string, channel?: string, whisper?: boolean, fileMetadata?: Record<string, unknown>): Promise<AiResponse> {
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

    const streamId = generateStreamId();
    this.handlers.onThinkingStarted?.(streamId);

    // Layer 1: 用户消息到达瞬间触发即时情绪反应（不等 AI 回复）
    this.triggerInstantReact(message, targetCharId, 'user');

    return new Promise<AiResponse>((resolve, reject) => {
      const session: StreamSession = {
        id: streamId,
        characterId: targetCharId,
        text: '',
        streamParser: new StreamController(),
        resolve,
        reject,
        channel: ch,
        instantReactLayer2Fired: false,
        bubbleStarted: false,
      };
      this.sessions.set(streamId, session);
      this.armWatchdog(streamId);

      void emit('chat:waiting', { stream_id: streamId, character_id: targetCharId })
        .catch(() => {})
        .then(() => invoke('send_message_stream', { message, streamId, characterId: targetCharId, channel: ch, whisper: whisper ?? false, fileMetadata })).catch((err) => {
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
    const targetCharId = characterId ?? getCharacterId() ?? undefined;
    const ch = 'direct';

    const streamId = generateStreamId();
    this.handlers.onThinkingStarted?.(streamId);

    return new Promise<AiResponse | null>((resolve) => {
      const session: StreamSession = {
        id: streamId,
        characterId: targetCharId,
        text: '',
        streamParser: new StreamController(),
        resolve: resolve as (response: AiResponse) => void,
        reject: () => resolve(null),
        channel: ch,
        instantReactLayer2Fired: false,
        bubbleStarted: false,
      };
      this.sessions.set(streamId, session);
      this.armWatchdog(streamId);

      void emit('chat:waiting', { stream_id: streamId, character_id: targetCharId })
        .catch(() => {})
        .then(() => invoke('wake_from_presence', { characterId: targetCharId, streamId })).catch((err) => {
        this.finishSessionWithError(streamId, String(err));
      });
    });
  }

  /**
   * 重新武装看门狗：每次收到 chunk / meta 都调用，实现「空闲计时」语义。
   *
   * 必须在 session 建好之后调用；session 结束时由 `clearWatchdog` 撤销。
   */
  private armWatchdog(sid: string): void {
    const session = this.sessions.get(sid);
    if (!session) return;
    if (session.watchdogTimer !== undefined) window.clearTimeout(session.watchdogTimer);
    session.watchdogTimer = window.setTimeout(() => {
      const current = this.sessions.get(sid);
      if (!current) return;
      current.watchdogTimer = undefined;
      // 走到这里说明后端既没有 chunk 也没有终态事件——按失败结算，
      // 让 thinking 归位、桌宠退出思考循环，并给用户一个可操作的提示。
      this.finishSessionWithError(
        sid,
        i18n.t('stream_timeout', { defaultValue: '回复超时，本次请求已放弃，请重试' }),
      );
    }, STREAM_IDLE_TIMEOUT_MS);
  }

  /** 撤销看门狗（session 结算时调用） */
  private clearWatchdog(sid: string): void {
    const session = this.sessions.get(sid);
    if (session?.watchdogTimer !== undefined) {
      window.clearTimeout(session.watchdogTimer);
      session.watchdogTimer = undefined;
    }
  }

  /** 正常完成一个 session */
  private finishSession(sid: string, finalText: string, response: AiResponse): void {
    const session = this.sessions.get(sid);
    if (!session) return;
    const ch = session.channel;
    this.clearWatchdog(sid);
    this.sessions.delete(sid);
    void emit('chat:waiting-ended', { stream_id: sid, character_id: session.characterId });
    // 通知其他窗口（如 ChatWindow）立即追加 AI 回复
    const assistantTimestamp = new Date().toISOString();
    void emit('chat:assistant_message', {
      content: finalText,
      sticker: response.sticker,
      timestamp: assistantTimestamp,
      stream_id: sid,
      character_id: session.characterId ?? getCharacterId() ?? undefined,
      channel: ch,
    });
    // 正常情况下 chunk 已创建流式气泡，这里只需结算并启动自动关闭。
    // 但某些模型会只回传 chat:done，或首个 chunk 在子窗口初始化期间丢失；此前
    // 这种回复仍会进入 聊天窗口，却没有任何 currentBubble 可供结算，因而桌宠沉默。
    // 用最终文本补建气泡，保证 done 是气泡展示的可靠兜底。
    if (session.bubbleStarted) {
      BubbleController.finishStreaming(finalText, { sticker: response.sticker ?? undefined });
    } else if (finalText.trim()) {
      BubbleController.showBubble(finalText, undefined, { sticker: response.sticker ?? undefined });
    } else if (response.sticker) {
      BubbleController.showSticker(response.sticker);
    }
    if (TtsStreamQueue.isEnabled() && finalText.trim()) {
      const release = BubbleController.holdForSpeech();
      void TtsStreamQueue.waitForDrain().finally(release);
    }
    this.handlers.onResponseReceived?.(response, sid);
    session.resolve(response);
  }

  /** 空文本完成：不写入历史，仅清理 session */
  private finishSessionEmpty(sid: string): void {
    const session = this.sessions.get(sid);
    if (!session) return;
    this.clearWatchdog(sid);
    this.sessions.delete(sid);
    void emit('chat:waiting-ended', { stream_id: sid, character_id: session.characterId });
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
    this.clearWatchdog(sid);
    this.sessions.delete(sid);
    void emit('chat:waiting-ended', { stream_id: sid, character_id: session.characterId });
    // API 错误通过 toast 提示，不写入对话历史、不展示气泡，避免兜底文案污染记忆
    void emit('toast:show', { message: error, type: 'error', duration: 5000, key: Date.now() });
    this.handlers.onError?.(error, sid);
    session.reject(new Error(error));
  }

  /** 取消完成一个 session */
  private finishSessionCancelled(sid: string): void {
    const session = this.sessions.get(sid);
    if (!session) return;
    this.clearWatchdog(sid);
    this.sessions.delete(sid);
    void emit('chat:waiting-ended', { stream_id: sid, character_id: session.characterId });
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
   * 通过 emit chat:instant_react 事件通知前端立即应用情绪反应。
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
