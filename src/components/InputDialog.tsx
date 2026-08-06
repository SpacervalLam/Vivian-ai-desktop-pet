import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, emit, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useTranslation } from 'react-i18next';
import { getCharacterId } from '../characterContext';

export interface InputDialogProps {
  onSend?: (text: string, whisper?: boolean) => void;
  onClose?: () => void;
  /** 按下 ESC 时的附加回调（sideChat 用：关闭输入框的同时解锁窗口） */
  onEscape?: () => void;
  visible?: boolean;
  /** 挂载后自动启动语音识别（语音快捷键触发时为 true） */
  autoStartVoice?: boolean;
  /** 群发模式：居中显示，发送时同时向所有角色发送消息 */
  broadcast?: boolean;
  /** 侧边栏模式：在 SideChat 窗口内渲染，跳过点击穿透管理，关闭时不隐藏窗口 */
  sideChat?: boolean;
  /** 显式指定目标角色 ID（共享窗口场景，覆盖 getCharacterId()） */
  characterId?: string;
  /** 广播模式是否激活（sideChat 切换用） */
  broadcastActive?: boolean;
  /** 切换广播模式回调（sideChat 用） */
  onToggleBroadcast?: () => void;
}

const ChatIcon: React.FC = () => (
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round">
    <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
  </svg>
);

const MicIcon: React.FC<{ recording: boolean }> = ({ recording }) => (
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none">
    <path
      d="M12 15a3 3 0 0 0 3-3V6a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3z"
      fill={recording ? '#E53935' : '#6B6B6B'}
    />
    <path
      d="M6 12a6 6 0 0 0 12 0M12 18v3"
      stroke={recording ? '#E53935' : '#6B6B6B'}
      strokeWidth="1.6"
      strokeLinecap="round"
    />
    {recording && (
      <circle cx="12" cy="12" r="11" stroke="#E53935" strokeWidth="1" opacity="0.5">
        <animate attributeName="r" values="11;13;11" dur="1.2s" repeatCount="indefinite" />
        <animate attributeName="opacity" values="0.5;0;0.5" dur="1.2s" repeatCount="indefinite" />
      </circle>
    )}
  </svg>
);

const SendIcon: React.FC<{ disabled: boolean }> = ({ disabled }) => (
  <svg width="18" height="18" viewBox="0 0 24 24" fill="none">
    <path
      d="M4 12l16-8-6 16-3-7-7-1z"
      fill={disabled ? '#9E9E9E' : '#ffffff'}
    />
  </svg>
);

const InputDialog: React.FC<InputDialogProps> = ({
  onSend,
  onClose,
  onEscape,
  visible = true,
  autoStartVoice = false,
  broadcast = false,
  sideChat = false,
  characterId: characterIdProp,
}) => {
  const { t } = useTranslation();
  const [value, setValue] = useState('');
  const [recording, setRecording] = useState(false);
  const [show, setShow] = useState(visible);
  // 悄悄话模式：仅私聊模式可用，Tab 切换；开启后其他在线角色不会旁观记录此对话
  const [whisper, setWhisper] = useState(false);
  const whisperRef = useRef(false);
  useEffect(() => { whisperRef.current = whisper; }, [whisper]);
  const inputRef = useRef<HTMLInputElement>(null);
  // sideChat 模式输入胶囊容器：点击其外部（窗口背景）时关闭输入框
  const sideChatCapsuleRef = useRef<HTMLDivElement>(null);
  // 用 ref 跟踪 recording 最新值，确保卸载清理函数能读到当前状态
  const recordingRef = useRef(false);
  useEffect(() => { recordingRef.current = recording; }, [recording]);
  // 标记用户是否主动点击停止，用于区分"用户手动停止"vs"静音超时自动停止"
  const userStoppedRef = useRef(false);

  // 按模式选择占位文本：
  // - broadcast → 与 Vivian 和 Nana 聊天
  // - whisper → 和 xx 说悄悄话（按当前角色 ID 选择）
  // - 私聊模式 → 按当前角色 ID 选择对应占位文本
  const charId = characterIdProp ?? getCharacterId();
  const placeholderKey = broadcast
    ? 'input_dialog.placeholder_broadcast'
    : whisper
      ? (charId === 'nana' ? 'input_dialog.whisper_nana' : 'input_dialog.whisper_vivian')
      : (charId === 'nana' ? 'input_dialog.placeholder_nana' : 'input_dialog.placeholder_vivian');

  // 计算输入框宽度：
  // - broadcast 模式：固定 560px（独立窗口居中）
  // - sideChat 模式：按 SideChat 窗口宽度（300px）自适应
  // - 角色私聊模式：按窗口宽度自适应，Nana 窗口较宽，缩小到窗口宽度的 1/1.3
  const [dialogWidth, setDialogWidth] = useState<number>(560);
  useEffect(() => {
    if (broadcast) {
      setDialogWidth(560);
      return;
    }
    if (sideChat) {
      setDialogWidth(280);
      return;
    }
    const charId = getCharacterId();
    // Nana 窗口宽度 422px，输入框宽度 = 422 / 1.3 ≈ 325px
    // Vivian 窗口宽度 355.33px，输入框宽度 = 355.33 / 1.0 = 355.33px（兜底 560 被 maxWidth 夹住）
    if (charId === 'nana') {
      const winWidth = 422;
      setDialogWidth(Math.round(winWidth / 1.3));
    } else {
      setDialogWidth(560);
    }
  }, [broadcast, sideChat]);

  useEffect(() => {
    setShow(visible);
  }, [visible]);

  // broadcast 模式（常驻窗口）：监听窗口焦点变化同步内部 show state。
  // 快捷键 toggle 时调用 win.show()+setFocus()，组件感知到聚焦后恢复输入框可见状态。
  useEffect(() => {
    if (!broadcast) return;
    let unlistenFocus: UnlistenFn | undefined;
    void (async () => {
      const win = getCurrentWindow();
      unlistenFocus = await win.onFocusChanged(({ payload: focused }) => {
        if (focused) {
          setShow(true);
          requestAnimationFrame(() => inputRef.current?.focus());
        }
      });
    })();
    return () => {
      unlistenFocus?.();
    };
  }, [broadcast]);

  // sideChat 模式：窗口失焦时关闭输入框（用户点击了侧边栏窗口外部）
  useEffect(() => {
    if (!sideChat) return;
    let unlistenFocus: UnlistenFn | undefined;
    void (async () => {
      const win = getCurrentWindow();
      unlistenFocus = await win.onFocusChanged(({ payload: focused }) => {
        if (!focused) handleClose();
      });
    })();
    return () => {
      unlistenFocus?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sideChat]);

  // sideChat 模式：点击窗口背景（输入胶囊外部）关闭输入框
  useEffect(() => {
    if (!sideChat) return;
    const onDown = (e: MouseEvent) => {
      const cap = sideChatCapsuleRef.current;
      if (cap && !cap.contains(e.target as Node)) handleClose();
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sideChat]);

  // 组件卸载时自动停止录音，防止 AsrManager.is_recording 状态泄漏到其他窗口
  // （如 ChatWindow 调用 start_recognition 时会因 "已在进行中" 而失败）
  useEffect(() => {
    return () => {
      if (recordingRef.current) {
        void invoke('stop_recognition', { characterId: getCharacterId() ?? undefined }).catch(() => {});
      }
    };
  }, []);

  // 角色窗口的私聊 InputDialog：挂载时暂停点击穿透，卸载时恢复。
  // 角色窗口中心矩形外的区域会 setIgnoreCursorEvents(true) 导致 input 无法聚焦，
  // suspend_click_through 强制全窗口响应鼠标，确保输入框可交互。
  // broadcast / sideChat 模式不涉及角色窗口点击穿透，跳过。
  useEffect(() => {
    if (broadcast || sideChat) return;
    void invoke('suspend_click_through', { reason: 'input_dialog' }).catch(() => {});
    return () => {
      void invoke('resume_click_through', { reason: 'input_dialog' }).catch(() => {});
    };
  }, [broadcast, sideChat]);

  // 语音快捷键触发时自动启动录音：
  // 依赖 autoStartVoice，使其从 false→true 时也能触发 startRecording
  // （配合 App.tsx 的「按下立即弹窗、长按 1 秒升级为语音」交互）
  useEffect(() => {
    if (autoStartVoice) {
      void startRecording();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoStartVoice]);

  // 语音识别期间：程序化 setValue 不会自动移动光标/滚动，手动同步到末尾
  useEffect(() => {
    if (!recording) return;
    const el = inputRef.current;
    if (!el) return;
    const len = el.value.length;
    el.setSelectionRange(len, len);
    el.scrollLeft = el.scrollWidth;
  }, [value, recording]);

  useEffect(() => {
    if (show) {
      setValue('');
      setWhisper(false);
      // 输入框显示后需主动聚焦：角色窗口可能被其他应用遮挡，
      // 独立窗口（broadcast）刚创建时焦点不在 WebView 上。
      const focusInput = () => {
        inputRef.current?.focus();
        const len = inputRef.current?.value.length ?? 0;
        inputRef.current?.setSelectionRange(len, len);
      };
      void getCurrentWindow()
        .setFocus()
        .catch(() => {})
        .finally(() => {
          requestAnimationFrame(focusInput);
          setTimeout(focusInput, 100);
        });
    }
  }, [show]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        onEscape?.();
        handleClose();
      }
    };
    if (show) window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [show]);

  const handleClose = () => {
    setShow(false);
    stopRecording();
    window.setTimeout(() => {
      onClose?.();
      // broadcast 独立窗口模式：ESC/发送后隐藏窗口（sideChat 模式窗口常驻，不隐藏）
      if (broadcast && !sideChat) {
        void getCurrentWindow().hide().catch(() => {});
      }
    }, 200);
  };

  const handleSend = async () => {
    const text = value.trim();
    if (!text) return;
    if (broadcast) {
      // 群发模式：通知各角色窗口通过 ChatController 发送（注册 session 以启用 TTS + 气泡）
      void emit('broadcast:send_message', { text });
    } else {
      onSend?.(text, whisper);
    }
    setValue('');
    handleClose();
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    // Tab 键切换悄悄话模式（仅私聊模式可用）
    if (e.key === 'Tab' && !broadcast) {
      e.preventDefault();
      setWhisper((w) => !w);
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      void handleSend();
    }
  };

  const startRecording = async () => {
    try {
      userStoppedRef.current = false;
      await invoke('start_recognition', { characterId: getCharacterId() ?? undefined });
      setRecording(true);
    } catch (e) {
      const msg = typeof e === 'string' ? e : (e instanceof Error ? e.message : String(e));
      console.warn('语音识别启动失败:', e);
      // 语音启动失败常含多行诊断 hint（如 WinRT 0x800455A0 的修复建议），
      // 用 error 类型 + 较长 duration 让用户能完整读完排查步骤。
      void emit('toast:show', { message: msg, type: 'error', duration: 15000, key: Date.now() });
    }
  };

  const stopRecording = async (silent = false) => {
    if (!recording) return;
    userStoppedRef.current = !silent;
    try {
      await invoke('stop_recognition', { characterId: getCharacterId() ?? undefined });
    } catch (e) {
      const msg = typeof e === 'string' ? e : (e instanceof Error ? e.message : String(e));
      console.warn('语音识别停止失败:', e);
      void emit('toast:show', { message: msg, type: 'warning', duration: 6000, key: Date.now() });
    } finally {
      setRecording(false);
    }
  };

  const toggleRecording = () => {
    if (recording) {
      void stopRecording();
    } else {
      void startRecording();
    }
  };

  // 录音期间监听 ASR 事件
  // - started：后端确认识别已启动（含异常自动重启），同步前端状态
  // - final_result：追加到已确认文本
  // - partial_result：替换尾部未确认片段
  // - stopped：后端停止识别（静音超时自动停止 / 用户手动停止 / 异常结束）
  //   静音超时自动停止时自动发送已识别内容
  // - error：打印警告日志
  const asrPartialRef = useRef('');
  const valueRef = useRef('');
  const autoSendTriggeredRef = useRef(false);
  useEffect(() => { valueRef.current = value; }, [value]);

  useEffect(() => {
    if (!recording) {
      asrPartialRef.current = '';
      autoSendTriggeredRef.current = false;
      return;
    }
    autoSendTriggeredRef.current = false;
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    (async () => {
      unlisten = await listen<{
        type: string;
        text?: string;
        confidence?: number;
        message?: string;
      }>('asr:event', (e) => {
        const { type, text } = e.payload;
        if (type === 'started') {
          // 后端确认识别已启动（包括异常后自动重启的场景），同步前端状态
          asrPartialRef.current = '';
          autoSendTriggeredRef.current = false;
          userStoppedRef.current = false;
          setRecording(true);
        } else if (type === 'final_result' && text) {
          setValue((prev) => {
            const base = prev.slice(0, prev.length - asrPartialRef.current.length);
            asrPartialRef.current = '';
            const separator = base === '' || base.endsWith(' ') ? '' : ' ';
            return base + separator + text;
          });
        } else if (type === 'partial_result' && text) {
          setValue((prev) => {
            const base = prev.slice(0, prev.length - asrPartialRef.current.length);
            asrPartialRef.current = text;
            return base + text;
          });
        } else if (type === 'stopped') {
          if (cancelled || autoSendTriggeredRef.current) return;
          asrPartialRef.current = '';
          setRecording(false);
          // 非用户主动停止（静音超时自动停止）→ 自动发送已识别的内容
          if (!userStoppedRef.current) {
            autoSendTriggeredRef.current = true;
            // 延迟一小段时间发送，确保 final_result 的 setValue 已经应用
            setTimeout(() => {
              if (cancelled) return;
              setValue((currentValue) => {
                const trimmed = currentValue.trim();
                if (trimmed) {
                  if (broadcast) {
                    (async () => {
                      try {
                        const result = await invoke<{ active_id: string; characters: Array<{ id: string; name: string; online: boolean }> }>('list_characters');
                        for (const c of result.characters) {
                          const streamId = `broadcast-${c.id}-${Date.now()}`;
                          void invoke('send_message_stream', {
                            message: trimmed,
                            streamId,
                            characterId: c.id,
                            channel: 'direct',
                          }).catch((err) => {
                            console.warn(`[broadcast] 发送到 ${c.id} 失败:`, err);
                          });
                        }
                      } catch (err) {
                        console.warn('[broadcast] 获取角色列表失败:', err);
                      }
                    })();
                  } else {
                    onSend?.(trimmed, whisperRef.current);
                  }
                  setShow(false);
                  window.setTimeout(() => {
                    onClose?.();
                    if (broadcast) {
                      void getCurrentWindow().hide().catch(() => {});
                    }
                  }, 200);
                }
                return '';
              });
            }, 150);
          }
          userStoppedRef.current = false;
        } else if (type === 'error') {
          console.warn('ASR 错误:', e.payload.message);
        }
      });
      if (cancelled) unlisten?.();
    })();
    return () => {
      cancelled = true;
      unlisten?.();
      asrPartialRef.current = '';
    };
  }, [recording, broadcast, onSend, onClose]);

  if (!show) return null;

  const canSend = value.trim().length > 0;
  // 悄悄话模式：深色配色，仅私聊模式生效（broadcast 永远 false）
  const isWhisper = whisper && !broadcast;
  // 深色外观：悄悄话 或 sideChat（匹配深色消息气泡）
  const isDark = isWhisper || sideChat;

  // === sideChat 模式：iOS iMessage 风格 ===
  if (sideChat) {
    return (
      <div
        style={{
          flexShrink: 0,
          alignSelf: 'center',
          display: 'flex',
          justifyContent: 'center',
          background: 'transparent',
          animation: 'vivian-input-fade 0.2s ease',
        }}
      >
        <style>{`
          @keyframes vivian-input-fade {
            from { opacity: 0; }
            to { opacity: 1; }
          }
          @keyframes vivian-input-rise {
            from { opacity: 0; transform: translateY(8px); }
            to { opacity: 1; transform: translateY(0); }
          }
        `}</style>
        <div
          ref={sideChatCapsuleRef}
          style={{
            width: 276,
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            padding: '7px 8px 7px 14px',
            background: 'rgba(45, 45, 55, 0.75)',
            backdropFilter: 'blur(20px) saturate(180%)',
            WebkitBackdropFilter: 'blur(20px) saturate(180%)',
            borderRadius: 22,
            border: '1px solid rgba(255, 255, 255, 0.12)',
            boxShadow: '0 1px 4px rgba(0, 0, 0, 0.3)',
            animation: 'vivian-input-rise 0.2s ease',
          }}
          onMouseDown={(e) => e.stopPropagation()}
          onDoubleClick={(e) => e.stopPropagation()}
        >
          <input
            ref={inputRef}
            type="text"
            value={value}
            placeholder={t(placeholderKey)}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={handleKeyDown}
            style={{
              flex: 1,
              border: 'none',
              outline: 'none',
              background: 'transparent',
              color: '#F0F0F0',
              fontSize: 14,
              fontFamily: 'inherit',
              padding: '6px 0',
              minWidth: 0,
              caretColor: '#0A84FF',
            }}
          />
          <button
            onClick={toggleRecording}
            aria-label="voice"
            title={recording ? t('input_dialog.stop_recording') : t('input_dialog.voice_input')}
            style={{
              width: 28,
              height: 28,
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              border: 'none',
              borderRadius: '50%',
              background: recording ? 'rgba(229, 57, 53, 0.15)' : 'transparent',
              cursor: 'pointer',
              flexShrink: 0,
            }}
          >
            <MicIcon recording={recording} />
          </button>
          <button
            onClick={handleSend}
            disabled={!canSend}
            aria-label="send"
            title={t('input_dialog.send')}
            style={{
              width: 30,
              height: 30,
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              border: 'none',
              borderRadius: '50%',
              background: canSend ? '#0A84FF' : 'rgba(255, 255, 255, 0.08)',
              cursor: canSend ? 'pointer' : 'default',
              flexShrink: 0,
              transition: 'background 0.2s ease',
            }}
          >
            <svg width="16" height="16" viewBox="0 0 24 24" fill="none">
              <path
                d="M12 19V5M5 12l7-7 7 7"
                stroke={canSend ? '#fff' : '#666'}
                strokeWidth="2.2"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
          </button>
        </div>
      </div>
    );
  }

  // === 默认模式（角色私聊 / broadcast） ===
  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        display: 'flex',
        alignItems: broadcast ? 'center' : 'flex-end',
        justifyContent: 'center',
        paddingBottom: broadcast ? 0 : 80,
        zIndex: 9000,
        background: 'transparent',
        animation: 'vivian-input-fade 0.2s ease',
      }}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) handleClose();
      }}
    >
      <style>{`
        @keyframes vivian-input-fade {
          from { opacity: 0; }
          to { opacity: 1; }
        }
        @keyframes vivian-input-rise {
          from { opacity: 0; transform: translateY(12px) scale(0.98); }
          to { opacity: 1; transform: translateY(0) scale(1); }
        }
      `}</style>
      <div
        style={{
          width: dialogWidth,
          maxWidth: '90vw',
          display: 'flex',
          alignItems: 'center',
          gap: 6,
          padding: '8px 10px 8px 14px',
          background: isDark ? 'rgba(38, 38, 44, 0.98)' : 'rgba(255,255,255,0.98)',
          backdropFilter: 'blur(20px) saturate(180%)',
          WebkitBackdropFilter: 'blur(20px) saturate(180%)',
          borderRadius: 16,
          boxShadow: isDark ? '0 8px 24px rgba(0, 0, 0, 0.45)' : '0 8px 24px rgba(0, 0, 0, 0.12)',
          border: isDark ? '1.5px solid #4A4A55' : '1.5px solid #E0DCD6',
          animation: 'vivian-input-rise 0.22s cubic-bezier(0.2, 0.8, 0.2, 1)',
          fontFamily: 'inherit',
        }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <span
          style={{
            color: isDark ? '#8A8A95' : '#9E9E9E',
            display: 'flex',
            alignItems: 'center',
            flexShrink: 0,
            marginRight: 4,
          }}
        >
          <ChatIcon />
        </span>
        <input
          ref={inputRef}
          type="text"
          value={value}
          placeholder={t(placeholderKey)}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={handleKeyDown}
          title={!whisper && !broadcast ? t('input_dialog.whisper_hint') : undefined}
          style={{
            flex: 1,
            border: 'none',
            outline: 'none',
            background: 'transparent',
            color: isDark ? '#EAEAEA' : '#2C2C2C',
            fontSize: 15,
            fontFamily: 'inherit',
            padding: '8px 4px',
            minWidth: 0,
            caretColor: isDark ? '#B0B0C0' : '#2C2C2C',
          }}
        />
        <button
          onClick={toggleRecording}
          aria-label="voice"
          title={recording ? t('input_dialog.stop_recording') : t('input_dialog.voice_input')}
          style={{
            width: 32,
            height: 32,
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            border: 'none',
            borderRadius: 10,
            background: recording ? 'rgba(229, 57, 53, 0.10)' : 'transparent',
            cursor: 'pointer',
            flexShrink: 0,
            transition: 'background 0.2s ease',
          }}
        >
          <MicIcon recording={recording} />
        </button>
        <button
          onClick={handleSend}
          disabled={!canSend}
          aria-label="send"
          title={t('input_dialog.send')}
          style={{
            width: 36,
            height: 32,
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            border: 'none',
            borderRadius: 10,
            background: canSend ? (isDark ? '#5C5C8A' : '#2C2C2C') : 'transparent',
            cursor: canSend ? 'pointer' : 'not-allowed',
            flexShrink: 0,
            transition: 'all 0.2s ease',
            marginLeft: 2,
          }}
        >
          <SendIcon disabled={!canSend} />
        </button>
      </div>
    </div>
  );
};

export default InputDialog;
