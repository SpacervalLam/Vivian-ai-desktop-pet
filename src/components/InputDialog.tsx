import React, { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, emit, type UnlistenFn } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useTranslation } from 'react-i18next';
import { getCharacterId } from '../characterContext';
import { ArrowUp, LockKeyhole, Mic, Square, Users, X } from 'lucide-react';
import './InputDialog.css';

export interface InputDialogProps {
  onSend?: (text: string, whisper?: boolean) => void;
  onClose?: () => void;
  /** 独立输入窗口按下 ESC 时的附加回调 */
  onEscape?: () => void;
  visible?: boolean;
  /** 挂载后自动启动语音识别（语音快捷键触发时为 true） */
  autoStartVoice?: boolean;
  /** 群发模式：居中显示，发送时同时向所有角色发送消息 */
  broadcast?: boolean;
  /** Independent cursor-positioned input window. */
  standalone?: boolean;
  /** 显式指定目标角色 ID（共享窗口场景，覆盖 getCharacterId()） */
  characterId?: string;
}

const InputDialog: React.FC<InputDialogProps> = ({
  onSend,
  onClose,
  onEscape,
  visible = true,
  autoStartVoice = false,
  broadcast = false,
  standalone = false,
  characterId: characterIdProp,
}) => {
  const { t } = useTranslation();
  const [value, setValue] = useState('');
  /** 图片草稿（直接对话侧边栏/私聊输入，可预览取消，随文本一起发送） */
  const [draftImages, setDraftImages] = useState<{ id: string; dataUrl: string; name: string; mime: string }[]>([]);
  const draftSeqRef = useRef(0);
  const [recording, setRecording] = useState(false);
  const [show, setShow] = useState(visible);
  // 悄悄话模式：仅私聊模式可用，Tab 切换；开启后其他在线角色不会旁观记录此对话
  const [whisper, setWhisper] = useState(false);
  const whisperRef = useRef(false);
  useEffect(() => { whisperRef.current = whisper; }, [whisper]);
  const inputRef = useRef<HTMLInputElement>(null);
  // 用 ref 跟踪 recording 最新值，确保卸载清理函数能读到当前状态
  const recordingRef = useRef(false);
  useEffect(() => { recordingRef.current = recording; }, [recording]);
  // 标记用户是否主动点击停止，用于区分"用户手动停止"vs"静音超时自动停止"
  const userStoppedRef = useRef(false);
  // 本次录音开始时输入框已有文本长度（润色只处理 ASR 追加的尾部）
  const asrBaseLenRef = useRef(0);
  // 用户主动停止标记：stopRecording 设置 / startRecording 复位，不随 asr 事件重置
  const manualStopRef = useRef(false);

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

  useEffect(() => {
    setShow(visible);
  }, [visible]);

  // broadcast 模式（常驻窗口）：监听窗口焦点变化同步内部 show state。
  // 快捷键 toggle 时调用 win.show()+setFocus()，组件感知到聚焦后恢复输入框可见状态。
  useEffect(() => {
    if (!broadcast) return;
    let unlistenFocus: UnlistenFn | undefined;
    let cancelled = false;
    void (async () => {
      const win = getCurrentWindow();
      unlistenFocus = await win.onFocusChanged(({ payload: focused }) => {
        if (focused) {
          setShow(true);
          requestAnimationFrame(() => inputRef.current?.focus());
        }
      });
      // 迟到 resolve 兜底：cleanup 先于监听器注册完成时，当场解绑防泄漏
      if (cancelled) { unlistenFocus?.(); unlistenFocus = undefined; }
    })();
    return () => {
      cancelled = true;
      unlistenFocus?.();
      unlistenFocus = undefined;
    };
  }, [broadcast]);



  // 组件卸载时自动停止录音，防止 AsrManager.is_recording 状态泄漏到其他窗口
  // （如 ChatWindow 调用 start_recognition 时会因 "已在进行中" 而失败）
  useEffect(() => {
    return () => {
      if (recordingRef.current) {
        void invoke('stop_recognition', { characterId: charId ?? undefined }).catch(() => {});
      }
    };
  }, []);

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
      // 共享快捷输入窗口由宿主负责关闭。
      if (broadcast && !standalone) {
        void getCurrentWindow().hide().catch(() => {});
      }
    }, 200);
  };

  /** 添加图片到草稿 */
  const addDraftImages = useCallback((files: File[]) => {
    if (files.length === 0) return;
    const results: { id: string; dataUrl: string; name: string; mime: string }[] = [];
    let done = 0;
    const total = files.length;
    for (const file of files) {
      if (!file.type.startsWith('image/')) continue;
      const reader = new FileReader();
      reader.onload = () => {
        draftSeqRef.current += 1;
        results.push({
          id: `draft-${draftSeqRef.current}`,
          dataUrl: String(reader.result ?? ''),
          name: file.name || '图片',
          mime: file.type,
        });
        done += 1;
        if (done === total) setDraftImages((prev) => [...prev, ...results]);
      };
      reader.onerror = () => { done += 1; };
      reader.readAsDataURL(file);
    }
  }, []);

  const handleSend = async () => {
    const text = value.trim();
    const hasImages = draftImages.length > 0;
    if (!text && !hasImages) return;
    const pendingDrafts = draftImages;
    // 文本 / onSend / broadcast 照旧
    if (text) {
      if (broadcast) {
        // 群发模式：通知各角色窗口通过 ChatController 发送（注册 session 以启用 TTS + 气泡）
        void emit('broadcast:send_message', { text });
      } else {
        onSend?.(text, whisper);
      }
    }
    // 图片
    if (hasImages) {
      setDraftImages([]);
      const sent = await sendDraftImagesWith(pendingDrafts);
      if (sent < pendingDrafts.length) {
        setDraftImages(pendingDrafts.slice(sent));
      }
    }
    setValue('');
    handleClose();
  };

  /** 发送指定草稿图片集合（供 handleSend 使用，避免依赖过期 state） */
  const sendDraftImagesWith = useCallback(async (drafts: typeof draftImages) => {
    if (drafts.length === 0) return 0;
    let sent = 0;
    for (let i = 0; i < drafts.length; i += 1) {
      const img = drafts[i];
      try {
        const base64 = img.dataUrl.split(',')[1] ?? '';
        if (!base64) continue;
        const tmpPath = await invoke<string>('save_temp_image', { base64Data: base64, mime: img.mime });
        if (broadcast) {
          void emit('broadcast:image_message', { sourcePath: tmpPath });
        } else {
          await invoke('send_image_message', { sourcePath: tmpPath, characterId: charId, channel: 'direct' });
        }
        sent += 1;
      } catch (e) {
        console.warn('[InputDialog] 图片发送失败:', e);
      }
    }
    return sent;
  }, [broadcast, charId]);

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

  /** 用 LLM 润色输入框中 ASR 追加的尾部文本（识别结束后调用，尽力而为）
   *  返回润色后的完整输入框内容；失败/文本过短/用户已编辑时原样返回 */
  const polishAsrTail = useCallback(async (): Promise<string> => {
    const current = inputRef.current?.value ?? '';
    const baseLen = asrBaseLenRef.current;
    if (baseLen > current.length) return current;
    const base = current.slice(0, baseLen);
    const asrPart = current.slice(baseLen).trim();
    if (asrPart.length < 2) return current;
    try {
      const polished = await invoke<string>('polish_asr_text', { text: asrPart });
      const trimmed = (polished ?? '').trim();
      if (!trimmed || inputRef.current?.value !== current) return current;
      const next = base.trim() ? `${base.replace(/\s+$/, '')} ${trimmed}` : trimmed;
      setValue(next);
      return next;
    } catch {
      return current;
    }
  }, []);

  const startRecording = async () => {
    try {
      userStoppedRef.current = false;
      manualStopRef.current = false;
      asrBaseLenRef.current = inputRef.current?.value.length ?? 0;
      await invoke('start_recognition', { characterId: charId ?? undefined });
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
    manualStopRef.current = !silent;
    try {
      await invoke('stop_recognition', { characterId: charId ?? undefined });
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
              // 先用 LLM 润色 ASR 追加的尾部文本，再走自动发送
              void (async () => {
                if (!cancelled) await polishAsrTail();
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
              })();
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
  }, [recording, broadcast, onSend, onClose, polishAsrTail]);

  // 用户手动停止录音后，对输入框中 ASR 追加的尾部文本做 LLM 润色
  // （静音超时自动停止由上方 stopped 分支在发送前润色，此处跳过）
  const prevRecordingRef = useRef(false);
  useEffect(() => {
    const was = prevRecordingRef.current;
    prevRecordingRef.current = recording;
    if (!was || recording || !manualStopRef.current) return;
    const timer = window.setTimeout(() => {
      void polishAsrTail();
    }, 300);
    return () => window.clearTimeout(timer);
  }, [recording, polishAsrTail]);

  if (!show) return null;

  const canSend = value.trim().length > 0 || draftImages.length > 0;

  const mode = broadcast ? 'broadcast' : charId === 'nana' ? 'nana' : 'vivian';
  const name = broadcast ? t('input_dialog.broadcast_label', { defaultValue: '广播' }) : mode === 'nana' ? 'Nana' : 'Vivian';
  return (
    <div className={'quick-compose-shell' + (standalone ? ' is-standalone' : '')}
      onPaste={(event) => {
        const files = Array.from(event.clipboardData.files).filter(file => file.type.startsWith('image/'));
        if (files.length) { event.preventDefault(); addDraftImages(files); }
      }}
      onMouseDown={(event) => { if (event.target === event.currentTarget) handleClose(); }}>
      <section className={`quick-compose quick-compose--${mode}${whisper && !broadcast ? ' is-whisper' : ''}`}
        aria-label={name} onMouseDown={event => event.stopPropagation()}>
        <header className="quick-compose-header">
          <span className="quick-compose-identity" aria-hidden>{broadcast ? <Users size={18} /> : mode === 'nana' ? 'N' : 'V'}</span>
          <span className="quick-compose-name">{name}</span>
          {!broadcast && <button className="quick-compose-whisper" type="button" aria-pressed={whisper}
            title={t('input_dialog.whisper_hint')} onClick={() => setWhisper(previous => !previous)}>
            <LockKeyhole size={13} />{whisper ? t('input_dialog.whisper_label', { defaultValue: '悄悄话' }) : null}
          </button>}
          <button className="quick-compose-close" type="button" aria-label={t('input_dialog.close', { defaultValue: '关闭' })} title="Esc" onClick={handleClose}><X size={17} /></button>
        </header>
        {draftImages.length > 0 && <div className="quick-compose-drafts">
          {draftImages.map(image => <button type="button" key={image.id} title={t('input_dialog.remove_image', { defaultValue: '移除图片' })}
            onClick={() => setDraftImages(images => images.filter(item => item.id !== image.id))}>
            <img src={image.dataUrl} alt={image.name} /><span><X size={10} /></span>
          </button>)}
        </div>}
        <div className="quick-compose-row">
          <input ref={inputRef} type="text" value={value} placeholder={t(placeholderKey)} aria-label={t(placeholderKey)}
            onChange={event => setValue(event.target.value)} onKeyDown={handleKeyDown} />
          <button className={`quick-compose-voice${recording ? ' is-recording' : ''}`} type="button" onClick={toggleRecording}
            aria-label={recording ? t('input_dialog.stop_recording') : t('input_dialog.voice_input')} aria-pressed={recording}
            title={recording ? t('input_dialog.stop_recording') : t('input_dialog.voice_input')}>
            {recording ? <Square size={15} fill="currentColor" /> : <Mic size={19} />}
          </button>
          <button className="quick-compose-send" type="button" onClick={() => void handleSend()} disabled={!canSend}
            aria-label={t('input_dialog.send')} title={t('input_dialog.send')}><ArrowUp size={20} strokeWidth={2.4} /></button>
        </div>
      </section>
    </div>
  );
};

export default InputDialog;
