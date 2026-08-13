import { create } from 'zustand';
import type { MoodState } from '../types';

/** voiceEnabled 的持久化 key（右键菜单运行时静音状态，跨重启保留） */
const VOICE_ENABLED_STORAGE_KEY = 'vivian.voiceEnabled';

const loadPersistedVoiceEnabled = (): boolean => {
  try {
    return localStorage.getItem(VOICE_ENABLED_STORAGE_KEY) === 'true';
  } catch {
    return false;
  }
};

export const hasPersistedVoiceEnabled = (): boolean => {
  try {
    return localStorage.getItem(VOICE_ENABLED_STORAGE_KEY) !== null;
  } catch {
    return false;
  }
};

/** 已结算的气泡段（从流式气泡中分离出来，独立显示并自动关闭） */
export interface SettledBubble {
  id: number;
  text: string;
  /** 显示时长（ms），由文本长度决定，最少 4s */
  duration: number;
}

interface AppState {
  // 状态
  isInitialized: boolean;
  isThinking: boolean;
  isListening: boolean;
  voiceEnabled: boolean;
  ttsEnabled: boolean;
  currentBubble: string | null;
  /** 跨角色对话标记：当前气泡是角色对另一个角色说的话（非对用户） */
  bubbleCrossCharacter: boolean;
  /** 跨角色对话的收听人名称（显示在气泡角落的标签） */
  bubbleListenerName: string | null;
  /** 已结算的气泡段列表（独立窗口/位置显示，各自自动关闭） */
  settledBubbles: SettledBubble[];
  isChatOpen: boolean;
  isConfigOpen: boolean;
  isMemoryOpen: boolean;
  isDiaryOpen: boolean;
  isInputDialogOpen: boolean;
  /** 语音快捷键触发的 InputDialog 标记：挂载后自动启动语音识别 */
  autoStartVoice: boolean;
  currentMood: MoodState | null;
  /** 当前角色在场状态（online/busy/rest/offline），驱动 Live2D 行为与自主行为跳过 */
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
  setInitialized: (value: boolean) => void;
  setThinking: (value: boolean) => void;
  setListening: (value: boolean) => void;
  setVoiceEnabled: (value: boolean) => void;
  setTtsEnabled: (value: boolean) => void;
  showBubble: (text: string) => void;
  hideBubble: () => void;
  /** 清除 store 内部气泡计时器（供 BubbleController 在流式场景调用） */
  clearBubbleTimer: () => void;
  /** 添加已结算气泡段 */
  addSettledBubble: (bubble: SettledBubble) => void;
  /** 移除指定已结算气泡段 */
  removeSettledBubble: (id: number) => void;
  /** 清除所有已结算气泡段 */
  clearSettledBubbles: () => void;
  toggleChat: () => void;
  setChatOpen: (value: boolean) => void;
  toggleConfig: () => void;
  setConfigOpen: (value: boolean) => void;
  toggleMemory: () => void;
  setMemoryOpen: (value: boolean) => void;
  toggleDiary: () => void;
  setDiaryOpen: (value: boolean) => void;
  showInputDialog: () => void;
  showInputDialogWithVoice: () => void;
  hideInputDialog: () => void;
  setAutoStartVoice: (value: boolean) => void;
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
  isInitialized: false,
  isThinking: false,
  isListening: false,
  voiceEnabled: loadPersistedVoiceEnabled(),
  ttsEnabled: false,
  currentBubble: null,
  bubbleCrossCharacter: false,
  bubbleListenerName: null,
  settledBubbles: [],
  isChatOpen: false,
  isConfigOpen: false,
  isMemoryOpen: false,
  isDiaryOpen: false,
  isInputDialogOpen: false,
  autoStartVoice: false,
  currentMood: null,
  presenceState: null,
  lastUserEmotion: '',
  userAvatarUrl: null,

  setInitialized: (value) => set({ isInitialized: value }),
  setThinking: (value) => set({ isThinking: value }),
  setListening: (value) => set({ isListening: value }),
  setVoiceEnabled: (value) => {
    try {
      localStorage.setItem(VOICE_ENABLED_STORAGE_KEY, value ? 'true' : 'false');
    } catch {
      /* ignore */
    }
    set({ voiceEnabled: value });
  },
  setTtsEnabled: (value) => set({ ttsEnabled: value }),

  showBubble: (text) => {
    clearBubbleTimer();
    set({ currentBubble: text, bubbleCrossCharacter: false, bubbleListenerName: null });
    bubbleTimer = setTimeout(() => {
      set({ currentBubble: null });
      bubbleTimer = null;
    }, BUBBLE_DURATION);
  },

  hideBubble: () => {
    clearBubbleTimer();
    set({ currentBubble: null, bubbleCrossCharacter: false, bubbleListenerName: null });
  },

  clearBubbleTimer,

  addSettledBubble: (bubble) => set((s) => ({
    settledBubbles: [...s.settledBubbles, bubble],
  })),

  removeSettledBubble: (id) => set((s) => ({
    settledBubbles: s.settledBubbles.filter((b) => b.id !== id),
  })),

  clearSettledBubbles: () => set({ settledBubbles: [] }),

  toggleChat: () => set((s) => ({ isChatOpen: !s.isChatOpen })),
  setChatOpen: (value) => set({ isChatOpen: value }),
  toggleConfig: () => set((s) => ({ isConfigOpen: !s.isConfigOpen })),
  setConfigOpen: (value) => set({ isConfigOpen: value }),
  toggleMemory: () => set((s) => ({ isMemoryOpen: !s.isMemoryOpen })),
  setMemoryOpen: (value) => set({ isMemoryOpen: value }),
  toggleDiary: () => set((s) => ({ isDiaryOpen: !s.isDiaryOpen })),
  setDiaryOpen: (value) => set({ isDiaryOpen: value }),
  showInputDialog: () => set({ isInputDialogOpen: true, autoStartVoice: false }),
  showInputDialogWithVoice: () => set({ isInputDialogOpen: true, autoStartVoice: true }),
  hideInputDialog: () => set({ isInputDialogOpen: false, autoStartVoice: false }),
  setAutoStartVoice: (value) => set({ autoStartVoice: value }),

  setMood: (mood) => set({ currentMood: mood }),
  setPresenceState: (state) => set({ presenceState: state }),
  setLastUserEmotion: (emotion) => set({ lastUserEmotion: emotion }),
  setUserAvatarUrl: (url) => set({ userAvatarUrl: url }),
}));

export const closeAllPanels = () => {
  const s = useAppStore.getState();
  s.setChatOpen(false);
  s.setConfigOpen(false);
  s.setMemoryOpen(false);
  s.setDiaryOpen(false);
  s.hideInputDialog();
};
