import { useCallback, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { UnlistenFn } from '@tauri-apps/api/event';
import { getCharacterId } from '../characterContext';
import type {
  AiResponse,
  AppConfig,
  ChatDonePayload,
  ChatErrorPayload,
  EnvironmentInfo,
  MemoryItem,
  MemoryType,
  ModelInfo,
  MoodState,
  GptSoVitsServiceState,
  ProactiveTickContext,
  ProactiveTickResponse,
  RelationshipInfo,
  StartupGreeting,
  SystemInfo,
  ToolInfo,
  TtsConfig,
  UserActivity,
} from '../types';

/** 发送消息（一次性完整响应） */
export function useSendMessage() {
  return useCallback(async (message: string): Promise<AiResponse> => {
    return invoke<AiResponse>('send_message', { message, characterId: getCharacterId() ?? undefined });
  }, []);
}

/** 停止当前生成 */
export function useStopGeneration() {
  return useCallback(async (): Promise<void> => {
    return invoke('stop_generation', { characterId: getCharacterId() ?? undefined });
  }, []);
}

/** 发送本地图片消息（多模态）：后端读取图片、调用 LLM 生成描述并存入记忆 */
export function useSendImageMessage() {
  return useCallback(async (sourcePath: string): Promise<void> => {
    return invoke('send_image_message', { sourcePath, characterId: getCharacterId() ?? undefined });
  }, []);
}

/** 文件文本提取结果 */
export interface FileTextResult {
  filename: string;
  text: string;
  file_type: 'image' | 'text' | 'pdf' | 'unsupported';
  truncated: boolean;
  original_char_count: number;
}

/** 提取文件文本内容（用于拖放文件发送给智能体） */
export function useExtractFileText() {
  return useCallback(async (sourcePath: string): Promise<FileTextResult> => {
    return invoke<FileTextResult>('extract_file_text', { sourcePath });
  }, []);
}

/** 读取已保存的图片文件并返回 data URL（供聊天/记忆面板加载历史图片） */
export function useGetImageDataURL() {
  return useCallback(async (imagePath: string): Promise<string | null> => {
    return invoke<string | null>('get_image_data_url', { imagePath });
  }, []);
}

interface StreamHandlers {
  onChunk?: (text: string) => void;
  onDone?: (payload: ChatDonePayload) => void;
  onError?: (error: string) => void;
}

/** 流式发送消息 - 通过事件订阅 chat:chunk / chat:done / chat:error */
export function useSendMessageStream() {
  return useCallback(
    async (message: string, handlers: StreamHandlers): Promise<void> => {
      const unlisteners: UnlistenFn[] = [];
      const cleanup = () => {
        for (const un of unlisteners) {
          try {
            un();
          } catch {
            /* ignore */
          }
        }
      };

      // 生成 stream_id 用于路由本请求的流式事件
      const streamId =
        typeof crypto !== 'undefined' && crypto.randomUUID
          ? crypto.randomUUID()
          : `s-${Date.now()}-${Math.random().toString(36).slice(2)}`;

      try {
        unlisteners.push(
          await listen<{ text: string; stream_id?: string }>('chat:chunk', (e) => {
            if (e.payload.stream_id !== streamId) return;
            handlers.onChunk?.(e.payload.text);
          }),
        );
        unlisteners.push(
          await listen<ChatDonePayload & { stream_id?: string }>('chat:done', (e) => {
            if (e.payload.stream_id !== streamId) return;
            handlers.onDone?.(e.payload);
            cleanup();
          }),
        );
        unlisteners.push(
          await listen<ChatErrorPayload & { stream_id?: string }>('chat:error', (e) => {
            if (e.payload.stream_id !== streamId) return;
            handlers.onError?.(e.payload.error);
            cleanup();
          }),
        );

        await invoke('send_message_stream', { message, streamId, characterId: getCharacterId() ?? undefined });
      } catch (err) {
        handlers.onError?.(String(err));
        cleanup();
      }
    },
    [],
  );
}

/** 记忆 CRUD */
export function useMemories() {
  const getAll = useCallback(async (): Promise<MemoryItem[]> => {
    return invoke<MemoryItem[]>('get_memories', { characterId: getCharacterId() ?? undefined });
  }, []);

  const add = useCallback(
    async (
      content: string,
      memoryType: MemoryType,
      importance: number,
    ): Promise<MemoryItem> => {
      return invoke<MemoryItem>('add_memory', {
        content,
        memoryType,
        importance,
        characterId: getCharacterId() ?? undefined,
      });
    },
    [],
  );

  const remove = useCallback(async (id: string): Promise<void> => {
    return invoke('delete_memory', { id, characterId: getCharacterId() ?? undefined });
  }, []);

  const clearAll = useCallback(async (): Promise<void> => {
    return invoke('clear_all_memories', { characterId: getCharacterId() ?? undefined });
  }, []);

  const search = useCallback(
    async (query: string, limit = 10): Promise<MemoryItem[]> => {
      return invoke<MemoryItem[]>('search_memories', { query, limit, characterId: getCharacterId() ?? undefined });
    },
    [],
  );

  const getSummary = useCallback(async (): Promise<string> => {
    return invoke<string>('get_memory_summary', { characterId: getCharacterId() ?? undefined });
  }, []);

  return useMemo(
    () => ({ getAll, add, remove, clearAll, search, getSummary }),
    [getAll, add, remove, clearAll, search, getSummary],
  );
}

/** 配置读写 */
export function useConfig() {
  const get = useCallback(async <T = unknown>(key: string): Promise<T> => {
    return invoke<T>('get_config', { key });
  }, []);

  const set = useCallback(async (key: string, value: unknown): Promise<void> => {
    return invoke('set_config', { key, value });
  }, []);

  const getAll = useCallback(async (): Promise<AppConfig> => {
    return invoke<AppConfig>('get_all_config');
  }, []);

  const save = useCallback(async (): Promise<void> => {
    return invoke('save_config');
  }, []);

  const reload = useCallback(async (): Promise<void> => {
    return invoke('reload_config');
  }, []);

  return { get, set, getAll, save, reload };
}

/** 系统信息 */
export function useSystemInfo() {
  return useCallback(async (): Promise<SystemInfo> => {
    const raw = await invoke<Record<string, unknown>>('get_system_info');
    return {
      cpu_usage: Number(raw.cpu_usage ?? 0),
      memory_usage: Number(raw.memory_usage_pct ?? raw.memory_usage ?? 0),
      cpu_count: Number(raw.cpu_count ?? 0),
      total_memory: Number(raw.total_memory ?? 0),
      used_memory: raw.used_memory != null ? Number(raw.used_memory) : undefined,
      available_memory:
        raw.available_memory != null ? Number(raw.available_memory) : undefined,
      uptime: raw.uptime != null ? Number(raw.uptime) : undefined,
      host_name: raw.host_name as string | undefined,
      os_name: raw.os_name as string | undefined,
      os_version: raw.os_version as string | undefined,
    };
  }, []);
}

/** Live2D 模型控制 */
export function useLive2DControl() {
  const playMotion = useCallback(async (motion: string): Promise<void> => {
    return invoke('play_motion', { motion, characterId: getCharacterId() ?? undefined });
  }, []);

  const setExpression = useCallback(
    async (expression: string, durationMs?: number): Promise<void> => {
      return invoke('set_expression', { expression, durationMs, characterId: getCharacterId() ?? undefined });
    },
    [],
  );

  const triggerIdleAction = useCallback(async (): Promise<void> => {
    return invoke('trigger_idle_action', { characterId: getCharacterId() ?? undefined });
  }, []);

  const getModelInfo = useCallback(async (): Promise<ModelInfo> => {
    return invoke<ModelInfo>('get_model_info', { characterId: getCharacterId() ?? undefined });
  }, []);

  return { playMotion, setExpression, triggerIdleAction, getModelInfo };
}

/** 情绪状态 */
export function useMood() {
  const getCurrent = useCallback(async (): Promise<MoodState> => {
    return invoke<MoodState>('get_current_mood', { characterId: getCharacterId() ?? undefined });
  }, []);

  const getHistory = useCallback(async (): Promise<MoodState[]> => {
    return invoke<MoodState[]>('get_mood_history', { characterId: getCharacterId() ?? undefined });
  }, []);

  const setExpression = useCallback(async (expression: string): Promise<void> => {
    return invoke('set_emotion_expression', { expression, characterId: getCharacterId() ?? undefined });
  }, []);

  return { getCurrent, getHistory, setExpression };
}

/** 工具系统 */
export function useTools() {
  const list = useCallback(async (): Promise<ToolInfo[]> => {
    const raw = await invoke<{ tools: ToolInfo[]; total: number }>('list_tools', { characterId: getCharacterId() ?? undefined });
    return raw.tools ?? [];
  }, []);

  return { list };
}

/** 窗口控制 */
export function useWindowControl() {
  const setPosition = useCallback(async (x: number, y: number): Promise<void> => {
    return invoke('set_window_position', { x, y, characterId: getCharacterId() ?? undefined });
  }, []);

  const getPosition = useCallback(async (): Promise<{ x: number; y: number }> => {
    return invoke<{ x: number; y: number }>('get_window_position', { characterId: getCharacterId() ?? undefined });
  }, []);

  const toggleAlwaysOnTop = useCallback(async (): Promise<void> => {
    return invoke('toggle_always_on_top', { characterId: getCharacterId() ?? undefined });
  }, []);

  return { setPosition, getPosition, toggleAlwaysOnTop };
}

/** TTS 语音合成 */
export function useTTS() {
  const getConfig = useCallback(async (): Promise<TtsConfig> => {
    return invoke<TtsConfig>('get_tts_config', { characterId: getCharacterId() ?? undefined });
  }, []);

  const setConfig = useCallback(async (config: TtsConfig): Promise<void> => {
    return invoke('set_tts_config', { config, characterId: getCharacterId() ?? undefined });
  }, []);

  const speak = useCallback(async (text: string): Promise<void> => {
    return invoke('speak_text', { text, characterId: getCharacterId() ?? undefined });
  }, []);

  const stop = useCallback(async (): Promise<void> => {
    return invoke('stop_speaking', { characterId: getCharacterId() ?? undefined });
  }, []);

  const getStatus = useCallback(async (): Promise<boolean> => {
    return invoke<boolean>('get_speaking_status', { characterId: getCharacterId() ?? undefined });
  }, []);

  /** 列出当前后端可用语音 */
  const listVoices = useCallback(async (): Promise<unknown[]> => {
    return invoke<unknown[]>('list_tts_voices', { characterId: getCharacterId() ?? undefined });
  }, []);

  /** 测试当前后端（合成一小段文本不播放） */
  const test = useCallback(async (): Promise<void> => {
    return invoke('test_tts', { characterId: getCharacterId() ?? undefined });
  }, []);

  /** 一键启动 GPT-SoVITS api_v2.py 服务（参数取自当前 TtsConfig） */
  const startGptSoVitsService = useCallback(async (): Promise<GptSoVitsServiceState> => {
    return invoke<GptSoVitsServiceState>('start_gpt_sovits_service', { characterId: getCharacterId() ?? undefined });
  }, []);

  /** 停止 GPT-SoVITS 服务 */
  const stopGptSoVitsService = useCallback(async (): Promise<GptSoVitsServiceState> => {
    return invoke<GptSoVitsServiceState>('stop_gpt_sovits_service', { characterId: getCharacterId() ?? undefined });
  }, []);

  /** 查询 GPT-SoVITS 服务状态 */
  const getGptSoVitsServiceStatus = useCallback(async (): Promise<GptSoVitsServiceState> => {
    return invoke<GptSoVitsServiceState>('get_gpt_sovits_service_status', { characterId: getCharacterId() ?? undefined });
  }, []);

  /** 扫描 GPT-SoVITS 安装目录下的模型文件 */
  const listGptSovitsModels = useCallback(async (): Promise<{
    gpt_models: Array<{ name: string; path: string }>;
    sovits_models: Array<{ name: string; path: string }>;
  }> => {
    return invoke('list_gpt_sovits_models');
  }, []);

  return {
    getConfig,
    setConfig,
    speak,
    stop,
    getStatus,
    listVoices,
    test,
    startGptSoVitsService,
    stopGptSoVitsService,
    getGptSoVitsServiceStatus,
    listGptSovitsModels,
  };
}

/** 主动对话系统 */
export function useProactive() {
  const getStatus = useCallback(async (): Promise<unknown> => {
    return invoke('get_proactive_status', { characterId: getCharacterId() ?? undefined });
  }, []);

  const start = useCallback(async (): Promise<void> => {
    return invoke('start_proactive', { characterId: getCharacterId() ?? undefined });
  }, []);

  const stop = useCallback(async (): Promise<void> => {
    return invoke('stop_proactive', { characterId: getCharacterId() ?? undefined });
  }, []);

  const tick = useCallback(
    async (context: ProactiveTickContext): Promise<ProactiveTickResponse> => {
      return invoke<ProactiveTickResponse>('proactive_tick', { context, characterId: getCharacterId() ?? undefined });
    },
    [],
  );

  const drainMessages = useCallback(async (): Promise<{ messages: unknown[] }> => {
    return invoke<{ messages: unknown[] }>('drain_proactive_messages', { characterId: getCharacterId() ?? undefined });
  }, []);

  const markIgnored = useCallback(async (): Promise<void> => {
    return invoke('mark_proactive_ignored', { characterId: getCharacterId() ?? undefined });
  }, []);

  const updateConfig = useCallback(async (): Promise<void> => {
    return invoke('update_proactive_config', { characterId: getCharacterId() ?? undefined });
  }, []);

  return { getStatus, start, stop, tick, drainMessages, markIgnored, updateConfig };
}

/** 环境信息 */
export function useEnvironment() {
  const getInfo = useCallback(async (): Promise<EnvironmentInfo> => {
    return invoke<EnvironmentInfo>('get_environment_info', { characterId: getCharacterId() ?? undefined });
  }, []);

  const getCurrentState = useCallback(async (): Promise<unknown> => {
    return invoke('get_current_state', { characterId: getCharacterId() ?? undefined });
  }, []);

  const getUserActivity = useCallback(async (): Promise<UserActivity> => {
    return invoke<UserActivity>('get_user_activity', { characterId: getCharacterId() ?? undefined });
  }, []);

  const update = useCallback(
    async (mouseX: number, mouseY: number, activeWindow: string): Promise<void> => {
      return invoke('update_environment', {
        mouseX,
        mouseY,
        activeWindow,
        characterId: getCharacterId() ?? undefined,
      });
    },
    [],
  );

  const getStartupGreeting = useCallback(async (): Promise<StartupGreeting> => {
    return invoke<StartupGreeting>('get_startup_greeting', { characterId: getCharacterId() ?? undefined });
  }, []);

  return { getInfo, getCurrentState, getUserActivity, update, getStartupGreeting };
}

/** 关系系统 */
export function useRelationship() {
  const get = useCallback(async (): Promise<RelationshipInfo> => {
    return invoke<RelationshipInfo>('get_relationship', { characterId: getCharacterId() ?? undefined });
  }, []);

  const getStage = useCallback(async (): Promise<string> => {
    return invoke<string>('get_relationship_stage', { characterId: getCharacterId() ?? undefined });
  }, []);

  const getMilestones = useCallback(
    async (): Promise<{ milestones: RelationshipInfo['milestones']; total: number }> => {
      return invoke<{ milestones: RelationshipInfo['milestones']; total: number }>(
        'get_milestones',
        { characterId: getCharacterId() ?? undefined },
      );
    },
    [],
  );

  const reset = useCallback(async (): Promise<void> => {
    return invoke('reset_relationship', { characterId: getCharacterId() ?? undefined });
  }, []);

  return { get, getStage, getMilestones, reset };
}

/** 在场状态（Presence） */
export interface PresenceStateInfo {
  character_id: string;
  state: string; // "online" | "busy" | "rest" | "offline"
  display_zh: string;
  can_direct: boolean;
  is_in_presence: boolean;
  since: number;
  elapsed_seconds: number;
}

export function usePresence() {
  const getState = useCallback(async (): Promise<PresenceStateInfo> => {
    return invoke<PresenceStateInfo>('get_presence_state', { characterId: getCharacterId() ?? undefined });
  }, []);

  const getAll = useCallback(async (): Promise<PresenceStateInfo[]> => {
    return invoke<PresenceStateInfo[]>('get_all_presence_states');
  }, []);

  const set = useCallback(async (target: string): Promise<{ changed: boolean; current: string }> => {
    return invoke<{ changed: boolean; current: string }>('set_presence_state', {
      target,
      characterId: getCharacterId() ?? undefined,
    });
  }, []);

  return { getState, getAll, set };
}
