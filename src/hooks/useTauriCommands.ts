import { useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCharacterId } from '../characterContext';
import type {
  AppConfig,
  EnvironmentInfo,
  MoodState,
  GptSoVitsServiceState,
  ProactiveTickContext,
  ProactiveTickResponse,
  StartupGreeting,
  TtsConfig,
  UserActivity,
} from '../types';

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

  const updateConfig = useCallback(async (): Promise<void> => {
    return invoke('update_proactive_config', { characterId: getCharacterId() ?? undefined });
  }, []);

  return { getStatus, start, stop, tick, drainMessages, updateConfig };
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
