/**
 * 消息气泡控制器
 *
 * 管理 Live2D 上方文本气泡的完整生命周期：创建、更新、追加、流式更新、关闭。
 *
 * 在 Tauri 版本中，气泡 UI 由 zustand store 中的 `currentBubble` 字段驱动渲染，
 * 本控制器作为 store 的轻量包装层，便于业务逻辑层（ChatController / 主动对话回调等）调用。
 *
 * 流式模式下支持逐字打字机效果：每次 showStreamingBubble 传入累积全文，
 * 控制器内部逐字揭示，营造字符级流式输出的视觉感受。
 */

import { useAppStore } from '../stores/useAppStore';

/** 空文本/取消时的最短停留时间 */
const MIN_CLOSE_MS = 3000;
/** 打字机每字间隔（ms）：原 setInterval(35ms)*2字 ≈ 17.5ms/字 */
const TYPEWRITER_MS_PER_CHAR = 17.5;
/** 单帧最大揭示字符数：防止掉帧后一次性蹦出过多文本 */
const TYPEWRITER_MAX_CHARS_PER_FRAME = 4;

/** 流式气泡兜底 auto-close 时长（ms）。
 *  showStreamingBubble 不设置 auto-close（等待 showBubble 接管），
 *  但当 TTS 播放卡死、flushSync 无限等待时，showBubble 永远不会被调用，
 *  导致气泡永久停留。这里设置 30s 兜底，确保异常情况下气泡最终消失。 */
const STREAMING_FALLBACK_CLOSE_MS = 30_000;

/** 气泡跨角色元数据 */
export interface BubbleOptions {
  /** 是否跨角色对话（角色对另一个角色说话） */
  crossCharacter?: boolean;
  /** 收听人名称 */
  listenerName?: string;
}

/**
 * 根据文本长度计算气泡显示时长（毫秒）。
 *
 * 每字 150ms 模拟阅读速度，最少 4 秒、无上限，
 * 使短消息有足够停留时间、长消息有足够时间读完。
 */
function computeDuration(text: string): number {
  if (!text) return MIN_CLOSE_MS;
  return Math.max(4_000, text.length * 150);
}

class BubbleControllerClass {
  private streamingBubble = false;
  private autoCloseTimer: ReturnType<typeof setTimeout> | null = null;
  /** 打字机 rAF 句柄 */
  private typewriterRaf: number | null = null;
  /** 打字机时间累积（ms）：基于帧间隔累积，对齐帧率平滑揭示 */
  private typewriterAccumMs = 0;
  /** 打字机上一帧时间戳 */
  private typewriterLastTs: number | null = null;
  /** 当前已揭示给用户看到的文本 */
  private displayedText = '';
  /** 流式累积的完整目标文本 */
  private targetText = '';
  /** 已结算气泡 ID 计数器 */
  private settledIdCounter = 0;
  /** 已结算气泡的自动关闭定时器（按 id 索引） */
  private settledTimers = new Map<number, ReturnType<typeof setTimeout>>();
  /** 流式气泡兜底 auto-close 定时器（防止 TTS 卡死导致气泡永久停留） */
  private streamingFallbackTimer: ReturnType<typeof setTimeout> | null = null;

  get currentBubble(): string | null {
    return useAppStore.getState().currentBubble;
  }

  get hasActiveBubble(): boolean {
    return this.currentBubble !== null;
  }

  /**
   * 显示新气泡（替换当前气泡）。
   *
   * durationMs <= 0 或未传时按文本长度自动计算（computeDuration）。
   * 直接操作 store state 而非 store.showBubble，避免 store 内部计时器
   * 与本控制器的 autoCloseTimer 产生双重计时竞争。
   */
  showBubble(text: string, durationMs?: number, options?: BubbleOptions): void {
    this.clearAutoCloseTimer();
    this.clearAllSettledTimers();
    this.stopTypewriter();
    this.streamingBubble = false;
    this.displayedText = text;
    this.targetText = text;
    const store = useAppStore.getState();
    store.clearBubbleTimer();
    store.clearSettledBubbles();
    useAppStore.setState({
      currentBubble: text,
      bubbleCrossCharacter: !!options?.crossCharacter,
      bubbleListenerName: options?.listenerName ?? null,
    });
    const duration = durationMs && durationMs > 0 ? durationMs : computeDuration(text);
    this.startAutoClose(duration);
  }

  /** 更新当前气泡文本（直接覆盖，无打字机效果） */
  updateBubble(text: string): void {
    if (this.currentBubble === null) return;
    this.displayedText = text;
    this.targetText = text;
    useAppStore.setState({ currentBubble: text });
  }

  /** 向当前气泡追加文本（默认双换行分隔） */
  appendToBubble(additionalText: string, separator = '\n\n'): boolean {
    if (this.currentBubble === null) return false;
    if (!additionalText) return false;
    const oldText = this.currentBubble;
    const newText = oldText ? oldText + separator + additionalText : additionalText;
    this.updateBubble(newText);
    return true;
  }

  /**
   * 创建或更新流式气泡（逐字打字机效果）。
   *
   * 传入的 text 是当前累积全文，控制器内部逐字揭示，
   * 新 chunk 到达时更新目标文本，打字机继续追赶。
   */
  showStreamingBubble(text: string, options?: BubbleOptions): void {
    if (!text) return;
    this.clearAutoCloseTimer();
    this.streamingBubble = true;
    useAppStore.getState().clearBubbleTimer();

    this.targetText = text;

    if (this.currentBubble === null) {
      // 首次显示：从空开始逐字揭示
      this.displayedText = '';
      useAppStore.setState({
        currentBubble: '',
        ...(options ? {
          bubbleCrossCharacter: !!options.crossCharacter,
          bubbleListenerName: options.listenerName ?? null,
        } : {}),
      });
    } else if (options) {
      // 已有气泡但传入了 options：更新跨角色状态
      useAppStore.setState({
        bubbleCrossCharacter: !!options.crossCharacter,
        bubbleListenerName: options.listenerName ?? null,
      });
    }

    // 启动或继续打字机
    this.startTypewriter();

    // 兜底 auto-close：showStreamingBubble 本身不设置关闭时间，
    // 等待 showBubble 在流式结束后接管。但如果 TTS 后端卡死导致
    // flushSync 无限等待，showBubble 永远不会被调用，气泡就会永久停留。
    // 这里启动兜底定时器，确保异常情况下气泡也能消失；正常流程下
    // showBubble -> startAutoClose 会调用 clearStreamingFallbackTimer 清除。
    this.clearStreamingFallbackTimer();
    this.streamingFallbackTimer = setTimeout(() => {
      console.warn('[BubbleController] 流式气泡兜底 auto-close 触发（流式结束未被 showBubble 接管）');
      this.startAutoClose();
    }, STREAMING_FALLBACK_CLOSE_MS);
  }

  /**
   * 结算当前段落并准备下一段。
   *
   * 将已完成段落分离到独立的已结算气泡（各自独立显示、独立自动关闭），
   * 然后在活跃气泡中继续流式显示 nextText。
   * 旧气泡不会被顶掉，而是保留在自己的位置直到自动关闭。
   */
  settleSegment(completedText: string, nextText: string): void {
    // 将已完成段落分离到独立气泡
    if (completedText) {
      const id = ++this.settledIdCounter;
      const duration = computeDuration(completedText);
      useAppStore.getState().addSettledBubble({ id, text: completedText, duration });

      // 为已结算气泡启动独立的自动关闭定时器
      const timer = setTimeout(() => {
        useAppStore.getState().removeSettledBubble(id);
        this.settledTimers.delete(id);
      }, duration);
      this.settledTimers.set(id, timer);
    }

    // 在活跃气泡中继续流式显示 nextText
    this.clearAutoCloseTimer();
    this.stopTypewriter();
    this.streamingBubble = true;
    this.displayedText = '';
    this.targetText = nextText;
    useAppStore.getState().clearBubbleTimer();
    useAppStore.setState({ currentBubble: nextText ? '' : null });

    if (nextText) {
      this.startTypewriter();
    }
  }

  /**
   * 启动自动关闭计时器（流式结束后调用）。
   *
   * durationMs <= 0 或未传时按当前目标文本长度自动计算。
   * 同时清除 store 内部计时器，避免双重计时竞争。
   */
  startAutoClose(durationMs?: number): void {
    this.streamingBubble = false;
    this.clearAutoCloseTimer();
    this.clearStreamingFallbackTimer();
    useAppStore.getState().clearBubbleTimer();
    // 流式结束：立即显示完整文本，停止打字机
    this.flushTypewriter();
    const duration = durationMs && durationMs > 0 ? durationMs : computeDuration(this.targetText);
    this.autoCloseTimer = setTimeout(() => {
      this.closeAll();
    }, duration);
  }

  /** 立即关闭所有气泡 */
  closeAll(): void {
    this.clearAutoCloseTimer();
    this.clearStreamingFallbackTimer();
    this.clearAllSettledTimers();
    this.stopTypewriter();
    this.streamingBubble = false;
    this.displayedText = '';
    this.targetText = '';
    useAppStore.getState().hideBubble();
    useAppStore.getState().clearSettledBubbles();
    useAppStore.setState({ bubbleCrossCharacter: false, bubbleListenerName: null });
  }

  /** 重新定位当前气泡（窗口移动后调用 - 在 Tauri 版本中由 React 布局自动处理） */
  repositionCurrent(): void {
    // React 渲染会自动跟随窗口位置，无需显式操作
  }

  private clearAutoCloseTimer(): void {
    if (this.autoCloseTimer !== null) {
      clearTimeout(this.autoCloseTimer);
      this.autoCloseTimer = null;
    }
  }

  /** 清除流式气泡兜底 auto-close 定时器 */
  private clearStreamingFallbackTimer(): void {
    if (this.streamingFallbackTimer !== null) {
      clearTimeout(this.streamingFallbackTimer);
      this.streamingFallbackTimer = null;
    }
  }

  /** 清除所有已结算气泡的自动关闭定时器 */
  private clearAllSettledTimers(): void {
    for (const timer of this.settledTimers.values()) {
      clearTimeout(timer);
    }
    this.settledTimers.clear();
  }

  /**
   * 启动打字机（requestAnimationFrame 驱动，对齐帧率）。
   *
   * 用 rAF 替代 setInterval：每帧最多一次 setState，避免帧间多次触发重渲染；
   * 基于帧间隔时间累积计算应揭示字符数，掉帧时自动平滑追赶，
   * 不会像 setInterval 那样在主线程繁忙时积压后密集触发。
   */
  private startTypewriter(): void {
    if (this.typewriterRaf !== null) return;
    this.typewriterAccumMs = 0;
    this.typewriterLastTs = null;
    const step = (ts: number) => {
      if (this.typewriterRaf === null) return; // 已被 stopTypewriter 取消
      if (this.typewriterLastTs === null) {
        this.typewriterLastTs = ts;
      }
      const dt = ts - this.typewriterLastTs;
      this.typewriterLastTs = ts;
      this.typewriterAccumMs += dt;

      if (this.displayedText.length >= this.targetText.length) {
        this.stopTypewriter();
        return;
      }

      // 按时间累积计算应揭示字符数，平滑追赶
      let charsToReveal = Math.floor(this.typewriterAccumMs / TYPEWRITER_MS_PER_CHAR);
      if (charsToReveal > 0) {
        // 限制单帧最大揭示数，防止严重掉帧后一次蹦出过多
        charsToReveal = Math.min(charsToReveal, TYPEWRITER_MAX_CHARS_PER_FRAME);
        this.typewriterAccumMs -= charsToReveal * TYPEWRITER_MS_PER_CHAR;
        const nextLen = Math.min(
          this.displayedText.length + charsToReveal,
          this.targetText.length
        );
        if (nextLen > this.displayedText.length) {
          this.displayedText = this.targetText.slice(0, nextLen);
          useAppStore.setState({ currentBubble: this.displayedText });
        }
      }
      this.typewriterRaf = requestAnimationFrame(step);
    };
    this.typewriterRaf = requestAnimationFrame(step);
  }

  /** 停止打字机（不刷新显示） */
  private stopTypewriter(): void {
    if (this.typewriterRaf !== null) {
      cancelAnimationFrame(this.typewriterRaf);
      this.typewriterRaf = null;
    }
    this.typewriterLastTs = null;
    this.typewriterAccumMs = 0;
  }

  /** 立即显示完整目标文本并停止打字机 */
  private flushTypewriter(): void {
    this.stopTypewriter();
    if (this.targetText && this.displayedText !== this.targetText) {
      this.displayedText = this.targetText;
      useAppStore.setState({ currentBubble: this.targetText });
    }
  }

  /** 是否处于流式模式 */
  get isStreaming(): boolean {
    return this.streamingBubble;
  }
}

/** 气泡控制器单例 */
export const BubbleController = new BubbleControllerClass();

export { computeDuration };
export default BubbleController;
