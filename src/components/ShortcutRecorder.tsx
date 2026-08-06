/**
 * 快捷键录制组件 - 现代风格
 *
 * 交互模式：
 * 1. 点击输入框进入录制模式（显示"请按下快捷键组合…"）
 * 2. 用户按下组合键，自动捕获并退出录制模式
 * 3. 按 Escape 取消录制
 * 4. 按 Backspace/Delete 清除当前快捷键
 * 5. "清除"按钮清空，"恢复默认"按钮重置为默认值
 *
 * 捕获的组合键转换为 Tauri accelerator 格式：
 *   "CommandOrControl+Shift+V" / "Control+Alt+T" / "Super+Space" 等
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { register, unregister, isRegistered } from '@tauri-apps/plugin-global-shortcut';

export interface ShortcutRecorderProps {
  /** 当前快捷键值（Tauri accelerator 格式） */
  value: string;
  /** 默认快捷键值 */
  defaultValue: string;
  /** 快捷键变化时触发（已通过格式校验，未通过冲突检测时返回 false） */
  onChange: (shortcut: string) => Promise<ConflictResult>;
  /** 是否禁用 */
  disabled?: boolean;
  /** 标签 i18n key（默认 config.field_shortcut） */
  labelKey?: string;
  /** 帮助文案 i18n key（默认 config.shortcut_help） */
  helpKey?: string;
}

export interface ConflictResult {
  ok: boolean;
  reason?: 'conflict' | 'invalid';
}

/** 将 KeyboardEvent 转换为 Tauri accelerator 字符串 */
function eventToAccelerator(e: KeyboardEvent): string | null {
  const parts: string[] = [];

  if (e.ctrlKey) parts.push('Control');
  if (e.shiftKey) parts.push('Shift');
  if (e.altKey) parts.push('Alt');
  if (e.metaKey) parts.push('Super');

  // 修饰键单独按下不构成快捷键
  const isModifierOnly =
    e.code.startsWith('Control') ||
    e.code.startsWith('Shift') ||
    e.code.startsWith('Alt') ||
    e.code.startsWith('Meta') ||
    e.code.startsWith('OS');

  if (isModifierOnly) return null;

  // 映射主键
  let mainKey = '';
  if (e.code.startsWith('Key')) {
    mainKey = e.code.slice(3); // KeyA → A
  } else if (e.code.startsWith('Digit')) {
    mainKey = e.code.slice(5); // Digit1 → 1
  } else if (e.code.startsWith('F') && /^F\d+$/.test(e.code)) {
    mainKey = e.code; // F1-F12
  } else {
    // 特殊键映射
    const specialMap: Record<string, string> = {
      Space: 'Space',
      Enter: 'Enter',
      Tab: 'Tab',
      Backquote: '`',
      Minus: '-',
      Equal: '=',
      BracketLeft: '[',
      BracketRight: ']',
      Backslash: '\\',
      Semicolon: ';',
      Quote: "'",
      Comma: ',',
      Period: '.',
      Slash: '/',
      ArrowUp: 'Up',
      ArrowDown: 'Down',
      ArrowLeft: 'Left',
      ArrowRight: 'Right',
      Home: 'Home',
      End: 'End',
      PageUp: 'PageUp',
      PageDown: 'PageDown',
      Insert: 'Insert',
      Delete: 'Delete',
      Numpad0: 'Num0',
      Numpad1: 'Num1',
      Numpad2: 'Num2',
      Numpad3: 'Num3',
      Numpad4: 'Num4',
      Numpad5: 'Num5',
      Numpad6: 'Num6',
      Numpad7: 'Num7',
      Numpad8: 'Num8',
      Numpad9: 'Num9',
    };
    mainKey = specialMap[e.code] ?? '';
  }

  if (!mainKey) return null;
  parts.push(mainKey);

  // 去重（同一修饰键只保留一次）
  const deduped: string[] = [];
  for (const p of parts) {
    if (!deduped.includes(p)) deduped.push(p);
  }
  return deduped.join('+');
}

/** 校验快捷键格式：至少一个修饰键 + 一个普通按键 */
function isValidShortcut(shortcut: string): boolean {
  if (!shortcut) return false;
  const parts = shortcut.split('+');
  if (parts.length < 2) return false;
  const modifiers = ['Control', 'Shift', 'Alt', 'Super', 'Command', 'CommandOrControl'];
  const hasModifier = parts.some((p) => modifiers.includes(p));
  const hasMain = parts.some((p) => !modifiers.includes(p));
  return hasModifier && hasMain;
}

/** 将 accelerator 字符串格式化为用户可读的显示文本 */
function formatForDisplay(shortcut: string): string {
  if (!shortcut) return '';
  return shortcut
    .replace(/CommandOrControl/gi, 'Ctrl')
    .replace(/Control/g, 'Ctrl')
    .replace(/Super/g, 'Win')
    .replace(/\+/g, ' + ');
}

const ShortcutRecorder: React.FC<ShortcutRecorderProps> = ({
  value,
  defaultValue,
  onChange,
  disabled,
  labelKey = 'config.field_shortcut',
  helpKey = 'config.shortcut_help',
}) => {
  const { t } = useTranslation();
  const [recording, setRecording] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);

  // 进入/退出录制模式时清理错误
  useEffect(() => {
    if (recording) setError(null);
  }, [recording]);

  // 录制模式下拦截所有键盘事件
  useEffect(() => {
    if (!recording) return;

    const handleKeyDown = async (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();

      // Escape 取消录制
      if (e.key === 'Escape') {
        setRecording(false);
        return;
      }

      // Backspace/Delete 清除快捷键
      if (e.key === 'Backspace' || e.key === 'Delete') {
        setRecording(false);
        if (value) {
          await onChange('');
        }
        return;
      }

      const accelerator = eventToAccelerator(e);
      if (!accelerator) return; // 仅修饰键，等待更多按键

      if (!isValidShortcut(accelerator)) {
        setError(t('toast.shortcut_invalid'));
        setRecording(false);
        return;
      }

      setRecording(false);

      // 如果与当前值相同，不做任何操作
      if (accelerator === value) return;

      // 冲突检测：尝试注册
      try {
        const alreadyRegistered = await isRegistered(accelerator);
        if (alreadyRegistered) {
          // 已被本应用注册（可能是当前快捷键自己），视为有效
          const result = await onChange(accelerator);
          if (!result.ok) {
            setError(
              result.reason === 'conflict'
                ? t('toast.shortcut_conflict', { shortcut: formatForDisplay(accelerator) })
                : t('toast.shortcut_invalid'),
            );
          }
          return;
        }
        // 尝试临时注册以检测系统级冲突
        try {
          await register(accelerator, () => {});
          await unregister(accelerator);
        } catch {
          // 注册失败 = 被其他程序占用
          setError(t('toast.shortcut_conflict', { shortcut: formatForDisplay(accelerator) }));
          return;
        }
        // 无冲突，提交变更
        const result = await onChange(accelerator);
        if (!result.ok) {
          setError(
            result.reason === 'conflict'
              ? t('toast.shortcut_conflict', { shortcut: formatForDisplay(accelerator) })
              : t('toast.shortcut_invalid'),
          );
        }
      } catch {
        setError(t('toast.shortcut_conflict', { shortcut: formatForDisplay(accelerator) }));
      }
    };

    // 捕获阶段最高优先级拦截
    window.addEventListener('keydown', handleKeyDown, true);
    return () => {
      window.removeEventListener('keydown', handleKeyDown, true);
    };
  }, [recording, value, onChange, t]);

  // 点击组件外部退出录制模式
  useEffect(() => {
    if (!recording) return;
    const handleClickOutside = (e: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setRecording(false);
      }
    };
    // 延迟绑定，避免触发录制的点击事件立即关闭
    const timer = setTimeout(() => {
      window.addEventListener('mousedown', handleClickOutside);
    }, 0);
    return () => {
      clearTimeout(timer);
      window.removeEventListener('mousedown', handleClickOutside);
    };
  }, [recording]);

  const handleReset = useCallback(() => {
    setError(null);
    if (defaultValue !== value) {
      void onChange(defaultValue);
    }
  }, [defaultValue, value, onChange]);

  const handleClear = useCallback(() => {
    setError(null);
    if (value) {
      void onChange('');
    }
  }, [value, onChange]);

  const displayText = recording
    ? t('config.shortcut_recorder_recording')
    : value
      ? formatForDisplay(value)
      : t('config.shortcut_recorder_idle');

  return (
    <div style={{ marginBottom: 18 }} ref={containerRef}>
      <label style={{ display: 'block', fontSize: 12, color: 'var(--text-secondary)', marginBottom: 6 }}>
        {t(labelKey)}
      </label>
      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <button
          type="button"
          disabled={disabled}
          onClick={() => setRecording((r) => !r)}
          style={{
            flex: 1,
            padding: '8px 12px',
            border: recording
              ? '1px solid var(--accent, #4f9cff)'
              : '1px solid var(--separator)',
            borderRadius: 6,
            background: recording
              ? 'rgba(79, 156, 255, 0.12)'
              : 'rgba(255,255,255,0.05)',
            color: recording
              ? 'var(--accent, #4f9cff)'
              : 'var(--text-primary)',
            fontSize: 13,
            fontFamily: recording ? 'inherit' : 'monospace',
            textAlign: 'left',
            cursor: disabled ? 'not-allowed' : 'pointer',
            outline: 'none',
            boxSizing: 'border-box',
            transition: 'border-color 0.15s, background 0.15s',
            opacity: disabled ? 0.5 : 1,
          }}
        >
          {displayText}
        </button>
        {value && (
          <button
            type="button"
            onClick={handleClear}
            disabled={disabled}
            style={{
              padding: '8px 12px',
              border: '1px solid var(--separator)',
              borderRadius: 6,
              background: 'rgba(255,255,255,0.05)',
              color: 'var(--text-secondary)',
              fontSize: 12,
              cursor: disabled ? 'not-allowed' : 'pointer',
              whiteSpace: 'nowrap',
            }}
          >
            {t('config.shortcut_recorder_clear')}
          </button>
        )}
        <button
          type="button"
          onClick={handleReset}
          disabled={disabled}
          style={{
            padding: '8px 12px',
            border: '1px solid var(--separator)',
            borderRadius: 6,
            background: 'rgba(255,255,255,0.05)',
            color: 'var(--text-secondary)',
            fontSize: 12,
            cursor: disabled ? 'not-allowed' : 'pointer',
            whiteSpace: 'nowrap',
          }}
        >
          {t('config.shortcut_recorder_reset')}
        </button>
      </div>
      <div style={{ fontSize: 11, color: 'var(--text-tertiary, var(--text-secondary))', marginTop: 4 }}>
        {t(helpKey)}
      </div>
      {error && (
        <div
          style={{
            fontSize: 11,
            color: '#e74c3c',
            marginTop: 4,
            padding: '4px 8px',
            background: 'rgba(231, 76, 60, 0.1)',
            borderRadius: 4,
          }}
        >
          {error}
        </div>
      )}
    </div>
  );
};

export default ShortcutRecorder;

export { formatForDisplay, isValidShortcut };
