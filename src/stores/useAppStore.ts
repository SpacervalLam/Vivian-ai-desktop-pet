import { create } from 'zustand';
import type { MoodState, StickerRef } from '../types';

/** 已结算的气泡段（从流式气泡中分离出来，独立显示并自动关闭） */
export interface SettledBubble {
  id: number;
  text: string;
  sticker?: StickerRef;
  /** 显示时长（ms），由文本长度决定，最少 4s */
  duration: number;
}

interface AppState {
  // 状态
  /**
   * 语音朗读开关（后端 `TtsConfig.enabled` 的前端镜像）。
   * 唯一真相源在后端：托盘「语音开关」与设置窗口 TTS 面板都写后端，
   * 前端只读镜像，不持有独立的静音状态（否则会出现菜单显示开但后端拒绝合成的双轨）。
   */
  ttsEnabled: boolean;
  currentBubble: string | null;
  /** 跨角色对话标记：当前气泡是角色对另一个角色说的话（非对用户） */
  bubbleCrossCharacter: boolean;
  /** 跨角色对话的收听人名称（显示在气泡角落的标签） */
  bubbleListenerName: string | null;
  /** 已结算的气泡段列表（独立窗口/位置显示，各自自动关闭） */
  settledBubbles: SettledBubble[];
  currentMood: MoodState | null;
  /** 当前角色在场状态（online/busy/rest/offline），驱动桌宠行为与自主行为跳过 */
  presenceState: string | null;
  /**
   * 最近一次 LLM 在 JSON 中判定的用户情绪（如 happy/sad/angry/neutral）。
   * 由 chat:done 事件写入，作为 proactive tick 的真实 user_emotion 来源。
   * 不要用 currentMood.primary_emotion 替代——那是 Vivian 自身的 mood，不是用户情绪。
   */
  lastUserEmotion: string;
  /** 用户自定义头像 data URL（null 表示使用默认头像） */
  userAvatarUrl: string | null;

  // 动作
  setTtsEnabled: (value: boolean) => void;
  showBubble: (text: string) => void;
  /** 清除 store 内部气泡计时器（供 BubbleController 在流式场景调用） */
  clearBubbleTimer: () => void;
  /** 添加已结算气泡段 */
  addSettledBubble: (bubble: SettledBubble) => void;
  /** 移除指定已结算气泡段 */
  removeSettledBubble: (id: number) => void;
  setMood: (mood: MoodState | null) => void;
  setPresenceState: (state: string | null) => void;
  setLastUserEmotion: (emotion: string) => void;
  setUserAvatarUrl: (url: string | null) => void;
}

let bubbleTimer: ReturnType<typeof setTimeout> | null = null;
const BUBBLE_DURATION = 5000;

const clearBubbleTimer = () => {
  if (bubbleTimer !== null) {
    clearTimeout(bubbleTimer);
    bubbleTimer = null;
  }
};

export const useAppStore = create<AppState>((set) => ({
  ttsEnabled: false,
  currentBubble: null,
  bubbleCrossCharacter: false,
  bubbleListenerName: null,
  settledBubbles: [],
  currentMood: null,
  presenceState: null,
  lastUserEmotion: '',
  userAvatarUrl: null,

  setTtsEnabled: (value) => set({ ttsEnabled: value }),

  showBubble: (text) => {
    clearBubbleTimer();
    set({ currentBubble: text, bubbleCrossCharacter: false, bubbleListenerName: null });
    bubbleTimer = setTimeout(() => {
      set({ currentBubble: null });
      bubbleTimer = null;
    }, BUBBLE_DURATION);
  },

  clearBubbleTimer,

  addSettledBubble: (bubble) => set((s) => ({
    settledBubbles: [...s.settledBubbles, bubble],
  })),

  removeSettledBubble: (id) => set((s) => ({
    settledBubbles: s.settledBubbles.filter((b) => b.id !== id),
  })),

  setMood: (mood) => set({ currentMood: mood }),
  setPresenceState: (state) => set({ presenceState: state }),
  setLastUserEmotion: (emotion) => set({ lastUserEmotion: emotion }),
  setUserAvatarUrl: (url) => set({ userAvatarUrl: url }),
}));
