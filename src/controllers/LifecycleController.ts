/**
 * 生命周期控制器
 *
 * 聚焦前端可见的「启动问候」子流程：
 *  - 首次见面判定：通过 localStorage 的 `vivian_has_met` 标志判定
 *  - 问候语生成：调用后端 `get_startup_greeting` 命令（后端内部做时间感知 + LLM 调用）
 *  - 问候语显示：通过 BubbleController 在 Live2D 上方显示气泡
 *  - 问候持久化：将问候作为 assistant 消息写入 zustand store 的聊天历史，
 *                这样打开 AI 对话窗口时即可看到这条问候；同时标记首次见面已完成
 *
 * Rust 后端在启动时完成，前端无需编排；本控制器仅保留与 UI 强相关的问候流程。
 */

import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { BubbleController } from './BubbleController';
import { useAppStore } from '../stores/useAppStore';
import { getCharacterId } from '../characterContext';
import type { StartupGreeting } from '../types';

const HAS_MET_KEY = 'vivian_has_met';

export interface InitGreetingResult {
  /** 实际显示的问候语（可能为空字符串） */
  greeting: string;
  /** 是否为首次见面 */
  isFirstMeeting: boolean;
  /** LLM 调用失败时的错误信息（greeting 为空时可能携带，供上层 show toast） */
  error?: string | null;
}

class LifecycleControllerClass {
  /** 单次进程内防重复问候标记 */
  private greetingShown = false;

  /** 判断是否为首次见面（localStorage 无 has_met 标志） */
  isFirstMeeting(): boolean {
    try {
      return localStorage.getItem(HAS_MET_KEY) !== '1';
    } catch {
      // localStorage 不可用时降级为「非首次」，避免每次启动都自我介绍
      return false;
    }
  }

  /** 标记首次见面已完成 */
  private markMet(): void {
    try {
      localStorage.setItem(HAS_MET_KEY, '1');
    } catch {
      // ignore
    }
  }

  /** 清除首次见面标记，使下次启动恢复到「未初次启动」状态 */
  resetMet(): void {
    try {
      localStorage.removeItem(HAS_MET_KEY);
    } catch {
      // ignore
    }
  }

  /**
   * 显示问候气泡并写入聊天历史（可由调用方控制时机，实现音画同步）
   */
  showGreetingBubble(greeting: string): void {
    const ts = new Date().toISOString();
    try {
      BubbleController.showBubble(greeting);
    } catch (e) {
      console.warn('[Lifecycle] 显示问候气泡失败:', e);
    }
    try {
      void emit('chat:assistant_message', { content: greeting, timestamp: ts, character_id: getCharacterId() ?? undefined, channel: 'proactive' });
    } catch (e) {
      console.warn('[Lifecycle] 广播问候事件失败:', e);
    }
  }

  /**
   * 启动问候主入口
   *
   * 应在 App.tsx 启动 useEffect 中、UI 稳定后调用一次。
   *
   * @param options.syncWithAudio 为 true 时不立即显示气泡，由调用方在 TTS 就绪后
   *   调用 showGreetingBubble() 显示，实现音画同步。
   * @returns 问候结果；greeting 为空表示本次未生成问候
   */
  async initGreeting(options?: { syncWithAudio?: boolean }): Promise<InitGreetingResult> {
    if (this.greetingShown) {
      return { greeting: '', isFirstMeeting: false };
    }
    this.greetingShown = true;

    const isFirstMeeting = this.isFirstMeeting();
    const syncWithAudio = options?.syncWithAudio ?? false;

    let greeting = '';
    let error: string | null = null;
    try {
      const resp = await invoke<StartupGreeting>('get_startup_greeting', {
        characterId: getCharacterId() ?? undefined,
      });
      greeting = (resp?.greeting ?? '').trim();
      error = resp?.error ?? null;
    } catch (e) {
      console.warn('[Lifecycle] 获取启动问候失败:', e);
      error = String(e);
    }

    if (!greeting) {
      if (isFirstMeeting) {
        this.markMet();
      }
      return { greeting: '', isFirstMeeting, error };
    }

    if (!syncWithAudio) {
      this.showGreetingBubble(greeting);
    }

    if (isFirstMeeting) {
      this.markMet();
    }

    console.info(
      `[Lifecycle] 启动问候${isFirstMeeting ? '(首次见面)' : ''}${syncWithAudio ? '(等待语音同步)' : ''}: ${greeting}`,
    );

    return { greeting, isFirstMeeting, error: null };
  }
}

/** 生命周期控制器单例 */
export const LifecycleController = new LifecycleControllerClass();

export default LifecycleController;
