import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

export interface ContextMenuPosition {
  x: number;
  y: number;
}

export interface ContextMenuProps {
  position: ContextMenuPosition;
  voiceEnabled: boolean;
  voiceToggleDisabled?: boolean;
  smartPositioningEnabled: boolean;
  onClose: () => void;
  onMemory: () => void;
  onSettings: () => void;
  onChat: () => void;
  onToggleVoice: () => void;
  onToggleSmartPositioning: () => void;
  onQuit: () => void;
}

interface MenuItem {
  key: string;
  label: string;
  onClick: () => void;
  danger?: boolean;
  withCheck?: boolean;
  checked?: boolean;
  disabled?: boolean;
}

export function ContextMenu(props: ContextMenuProps) {
  const { t } = useTranslation();
  const {
    position,
    voiceEnabled,
    voiceToggleDisabled,
    smartPositioningEnabled,
    onClose,
    onMemory,
    onSettings,
    onChat,
    onToggleVoice,
    onToggleSmartPositioning,
    onQuit,
  } = props;

  const menuRef = useRef<HTMLDivElement | null>(null);
  const [adjustedPos, setAdjustedPos] = useState(position);

  // 边界修正，避免菜单超出视窗
  useEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;
    const rect = menu.getBoundingClientRect();
    let x = position.x;
    let y = position.y;
    if (x + rect.width > window.innerWidth) x = window.innerWidth - rect.width - 4;
    if (y + rect.height > window.innerHeight) y = window.innerHeight - rect.height - 4;
    setAdjustedPos({ x: Math.max(4, x), y: Math.max(4, y) });
  }, [position]);

  // 点击外部、窗口失焦或 Escape 关闭
  useEffect(() => {
    const handlePointer = (e: MouseEvent) => {
      if (e.button === 2) return;
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    };
    const handleEsc = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    // 窗口失焦（点击窗口外、切换到其他应用）时关闭菜单
    const handleBlur = () => onClose();
    window.addEventListener('mousedown', handlePointer);
    window.addEventListener('keydown', handleEsc);
    window.addEventListener('blur', handleBlur);
    return () => {
      window.removeEventListener('mousedown', handlePointer);
      window.removeEventListener('keydown', handleEsc);
      window.removeEventListener('blur', handleBlur);
    };
  }, [onClose]);

  const items: MenuItem[] = [
    { key: 'memory', label: t('memory_management'), onClick: onMemory },
    { key: 'settings', label: t('settings'), onClick: onSettings },
    { key: 'chat', label: t('ai_chat'), onClick: onChat },
    {
      key: 'voice',
      label: t('voice_toggle'),
      onClick: onToggleVoice,
      withCheck: true,
      checked: voiceEnabled,
      disabled: voiceToggleDisabled,
    },
    {
      key: 'smart_positioning',
      label: t('config.field_smart_positioning'),
      onClick: onToggleSmartPositioning,
      withCheck: true,
      checked: smartPositioningEnabled,
    },
    { key: 'quit', label: t('quit'), onClick: onQuit, danger: true },
  ];

  return (
    <div
      ref={menuRef}
      className="fade-in scrapbook"
      style={{
        position: 'fixed',
        left: adjustedPos.x,
        top: adjustedPos.y,
        minWidth: 168,
        background: 'var(--panel-surface)',
        backdropFilter: 'blur(12px)',
        border: '1.5px solid var(--panel-border-strong)',
        borderRadius: 12,
        boxShadow: 'var(--panel-shadow-elevated)',
        padding: 6,
        zIndex: 9999,
        userSelect: 'none',
      }}
      onContextMenu={(e) => e.preventDefault()}
      onMouseDown={(e) => e.stopPropagation()}
    >
      {items.map((item, idx) => (
        <div key={item.key}>
          {idx === 3 && (
            <div
              style={{
                height: 1,
                background: 'var(--panel-border)',
                margin: '4px 2px',
              }}
            />
          )}
          <button
            onClick={() => {
              item.onClick();
              onClose();
            }}
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              width: '100%',
              padding: '8px 10px',
              borderRadius: 8,
              fontSize: 13,
              color: item.disabled
                ? 'var(--panel-text-tertiary)'
                : item.danger
                  ? '#E53935'
                  : 'var(--panel-text)',
              textAlign: 'left',
              cursor: item.disabled ? 'not-allowed' : 'pointer',
              opacity: item.disabled ? 0.5 : 1,
              transition: 'transform 0.18s cubic-bezier(0.2,0.8,0.2,1), box-shadow 0.18s ease, background 0.18s ease, border-color 0.18s ease',
              background: 'transparent',
              boxShadow: 'none',
              transform: 'translateY(0) rotate(0)',
              border: '1.5px solid transparent',
            }}
            onMouseEnter={(e) => {
              if (item.disabled) return;
              const btn = e.currentTarget as HTMLButtonElement;
              btn.style.borderColor = 'var(--panel-border-strong)';
              btn.style.transform = 'translateY(-2px) rotate(-0.5deg)';
              btn.style.boxShadow = 'var(--panel-shadow-card)';
            }}
            onMouseLeave={(e) => {
              if (item.disabled) return;
              const btn = e.currentTarget as HTMLButtonElement;
              btn.style.borderColor = 'transparent';
              btn.style.transform = 'translateY(0) rotate(0)';
              btn.style.boxShadow = 'none';
            }}
          >
            <span>{item.label}</span>
            {item.withCheck && (
              <span style={{ opacity: item.checked ? 1 : 0.25 }}>
                {item.checked ? '●' : '○'}
              </span>
            )}
          </button>
        </div>
      ))}
    </div>
  );
}

export default ContextMenu;
