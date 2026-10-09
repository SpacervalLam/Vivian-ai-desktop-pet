import React, { useEffect, useState } from 'react';
import ReactDOM from 'react-dom';
import { useTranslation } from 'react-i18next';
export const fieldStyle: React.CSSProperties = {
  marginBottom: 18,
};
export const labelStyle: React.CSSProperties = {
  display: 'block',
  fontSize: 13,
  fontWeight: 600,
  color: 'var(--panel-text-secondary)',
  marginBottom: 6,
  paddingLeft: 2,
};
export const inputStyle: React.CSSProperties = {
  width: '100%',
  padding: '9px 12px',
  border: '1.5px solid var(--panel-border)',
  borderRadius: 12,
  background: 'var(--panel-surface)',
  color: 'var(--panel-text)',
  fontSize: 13,
  fontFamily: 'inherit',
  outline: 'none',
  boxSizing: 'border-box',
  boxShadow: 'var(--panel-shadow-subtle)',
  transition: 'border-color 0.15s ease, box-shadow 0.15s ease',
};
export const selectStyle: React.CSSProperties = {
  ...inputStyle,
  appearance: 'none',
  cursor: 'pointer',
  paddingRight: 30,
};
export const sectionTitleStyle: React.CSSProperties = {
  fontSize: 14,
  fontWeight: 700,
  color: 'var(--panel-text)',
  marginBottom: 14,
  paddingBottom: 8,
  paddingLeft: 10,
  borderLeft: '3px solid var(--panel-accent)',
  borderBottom: '1px solid var(--panel-border)',
  letterSpacing: 0.3,
};


export const TextField: React.FC<{
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  type?: 'text' | 'password' | 'number';
  disabled?: boolean;
  list?: string;
  style?: React.CSSProperties;
  help?: string;
}> = ({ label, value, onChange, placeholder, type = 'text', disabled = false, list, style, help }) => (
  <div className="settings-field" style={{ ...fieldStyle, ...style }}>
    <label style={{ ...labelStyle, ...(disabled ? { opacity: 0.5 } : {}) }}>{label}</label>
    <input
      type={type}
      aria-label={label}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      disabled={disabled}
      list={list}
      style={{
        ...inputStyle,
        ...(disabled
          ? {
              cursor: 'not-allowed',
              opacity: 0.5,
            }
          : {}),
      }}
    />
    {help && (
      <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: 6, lineHeight: 1.5 }}>
        {help}
      </div>
    )}
  </div>
);

/// 带浏览按钮的文本输入框：输入框与按钮在同一行水平对齐
/// （用 alignItems: 'flex-end' 让按钮底部与 input 底部齐平，
///   按钮的 padding 与 input 完全一致以保持高度相同）
export const BrowseTextField: React.FC<{
  label: string;
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  onBrowse: () => void;
  browseLabel: string;
  disabled?: boolean;
  list?: string;
}> = ({ label, value, onChange, placeholder, onBrowse, browseLabel, disabled = false, list }) => (
  <div className="settings-field" style={{ ...fieldStyle, marginBottom: 18 }}>
    <label style={{ ...labelStyle, ...(disabled ? { opacity: 0.5 } : {}) }}>{label}</label>
    <div style={{ display: 'flex', gap: 6, alignItems: 'stretch' }}>
      <input
        type="text"
        aria-label={label}
        list={list}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        disabled={disabled}
        style={{
          ...inputStyle,
          flex: 1,
          ...(disabled ? { cursor: 'not-allowed', opacity: 0.5 } : {}),
        }}
      />
      <button
        type="button"
        onClick={onBrowse}
        disabled={disabled}
        style={{
          padding: '8px 12px',
          border: '1.5px solid var(--panel-border)',
          borderRadius: 12,
          background: 'var(--panel-sticker-soft, var(--panel-surface))',
          color: 'var(--panel-text-secondary)',
          fontSize: 11,
          cursor: disabled ? 'not-allowed' : 'pointer',
          fontFamily: 'inherit',
          whiteSpace: 'nowrap',
          boxSizing: 'border-box',
          flexShrink: 0,
          opacity: disabled ? 0.5 : 1,
          boxShadow: 'var(--panel-shadow-subtle)',
        }}
      >
        {browseLabel}
      </button>
    </div>
  </div>
);

/// 分组小标题（GPT-SoVITS 面板内部使用）
export const subsectionTitleStyle: React.CSSProperties = {
  marginTop: 20,
  marginBottom: 10,
  fontSize: 11,
  color: 'var(--panel-text-tertiary)',
  fontWeight: 600,
  letterSpacing: 0.5,
  textTransform: 'uppercase',
};

export const SelectField: React.FC<{
  label: string;
  value: string;
  onChange: (v: string) => void;
  options: { value: string; label: string; disabled?: boolean }[];
  labelExtra?: React.ReactNode;
}> = ({ label, value, onChange, options, labelExtra }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const selectRef = React.useRef<HTMLDivElement>(null);
  const dropdownRef = React.useRef<HTMLDivElement>(null);
  const buttonRef = React.useRef<HTMLButtonElement>(null);
  const [dropdownPos, setDropdownPos] = React.useState<{ top: number; left: number; width: number; upward: boolean } | null>(null);

  const updateDropdownPosition = React.useCallback(() => {
    if (buttonRef.current) {
      const rect = buttonRef.current.getBoundingClientRect();
      const dropdownHeight = Math.min(options.length * 40 + 2, 200);
      const spaceBelow = window.innerHeight - rect.bottom;
      const upward = spaceBelow < dropdownHeight + 8 && rect.top > dropdownHeight + 8;
      setDropdownPos({
        top: upward ? rect.top - 4 : rect.bottom + 4,
        left: rect.left,
        width: rect.width,
        upward,
      });
    }
  }, [options.length]);

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      const target = e.target as Node;
      if (
        (selectRef.current && selectRef.current.contains(target)) ||
        (dropdownRef.current && dropdownRef.current.contains(target))
      ) {
        return;
      }
      setOpen(false);
    };
    if (open) {
      document.addEventListener('mousedown', handleClickOutside);
      updateDropdownPosition();
      window.addEventListener('scroll', updateDropdownPosition, true);
      window.addEventListener('resize', updateDropdownPosition);
    }
    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
      window.removeEventListener('scroll', updateDropdownPosition, true);
      window.removeEventListener('resize', updateDropdownPosition);
    };
  }, [open, updateDropdownPosition]);

  const selectedOption = options.find((o) => o.value === value);

  return (
    <div className="settings-field" style={fieldStyle} ref={selectRef}>
      <div style={{ ...labelStyle, display: 'flex', alignItems: 'center', gap: 6 }}>
        <span>{label}</span>
        {labelExtra}
      </div>
      <div>
        <button
          ref={buttonRef}
          type="button"
          aria-label={`${label}: ${selectedOption?.label ?? t('common.please_select')}`}
          aria-expanded={open}
          onClick={() => setOpen(!open)}
          onKeyDown={(event) => {
            if (event.key === 'Escape') setOpen(false);
          }}
          style={{
            ...selectStyle,
            textAlign: 'left',
            position: 'relative',
            background: 'var(--panel-bg-surface-elevated)',
            width: '100%',
          }}
        >
          <span style={{ color: selectedOption ? 'var(--panel-text)' : 'var(--panel-text-tertiary)' }}>
            {selectedOption ? selectedOption.label : t('common.please_select')}
          </span>
          <span
            style={{
              position: 'absolute',
              right: 10,
              top: '50%',
              transform: `translateY(-50%) ${open ? 'rotate(180deg)' : 'rotate(0deg)'}`,
              transition: 'transform 0.2s ease',
              color: 'var(--panel-text-tertiary)',
              fontSize: 10,
            }}
          >
            ▾
          </span>
        </button>
        {open && dropdownPos && ReactDOM.createPortal(
          <div
            ref={dropdownRef}
            style={{
              position: 'fixed',
              top: dropdownPos.top,
              left: dropdownPos.left,
              width: dropdownPos.width,
              background: 'var(--panel-surface)',
              border: '1.5px solid var(--panel-border-strong)',
              borderRadius: 10,
              boxShadow: 'var(--panel-shadow-elevated)',
              zIndex: 10000,
              maxHeight: 200,
              overflowY: 'auto',
              animation: 'fadeIn 0.15s ease-out',
            }}
          >
            {options.map((o) => (
              <button
                key={o.value}
                type="button"
                disabled={o.disabled}
                onClick={() => {
                  if (o.disabled) return;
                  onChange(o.value);
                  setOpen(false);
                }}
                style={{
                  width: '100%',
                  textAlign: 'left',
                  padding: '10px 14px',
                  background: o.value === value ? 'var(--panel-selected-bg)' : 'transparent',
                  color: o.disabled
                    ? 'var(--panel-text-tertiary)'
                    : o.value === value
                      ? 'var(--panel-selected-text)'
                      : 'var(--panel-text)',
                  border: 'none',
                  cursor: o.disabled ? 'not-allowed' : 'pointer',
                  fontSize: 13,
                  fontFamily: 'inherit',
                  opacity: o.disabled ? 0.5 : 1,
                  transition: 'background 0.1s ease',
                }}
                onMouseEnter={(e) => {
                  if (o.disabled || o.value === value) return;
                  e.currentTarget.style.background = 'var(--panel-bg-hover)';
                }}
                onMouseLeave={(e) => {
                  if (o.disabled || o.value === value) return;
                  e.currentTarget.style.background = 'transparent';
                }}
              >
                {o.label}
                {o.disabled && (
                  <span style={{ marginLeft: 6, fontSize: 11, color: 'var(--panel-text-tertiary)' }}>
                    {t('common.coming_soon')}
                  </span>
                )}
              </button>
            ))}
          </div>,
          document.body
        )}
      </div>
    </div>
  );
};

export const NumberField: React.FC<{
  label: string;
  value: number;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
  step?: number;
  help?: string;
}> = ({ label, value, onChange, min, max, step, help }) => (
  <div className="settings-field" style={fieldStyle}>
    <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
      <label style={labelStyle}>{label}</label>
      {help && (
        <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', lineHeight: 1.4 }}>
          {help}
        </span>
      )}
    </div>
    <input
      type="number"
      aria-label={label}
      value={value}
      onChange={(e) => onChange(Number(e.target.value))}
      min={min}
      max={max}
      step={step}
      style={inputStyle}
    />
  </div>
);

export const SliderField: React.FC<{
  label: string;
  value: number;
  onChange: (v: number) => void;
  min: number;
  max: number;
  step: number;
  format?: (v: number) => string;
  help?: string;
}> = ({ label, value, onChange, min, max, step, format, help }) => (
  <div className="settings-field" style={fieldStyle}>
    <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 6 }}>
      <div style={{ display: 'flex', flexDirection: 'column', gap: 2, flex: 1, minWidth: 0 }}>
        <label style={{ ...labelStyle, marginBottom: 0 }}>{label}</label>
        {help && (
          <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', lineHeight: 1.4 }}>
            {help}
          </span>
        )}
      </div>
      <span style={{ fontSize: 12, color: 'var(--panel-text)', fontVariantNumeric: 'tabular-nums' }}>
        {format ? format(value) : value}
      </span>
    </div>
    <input
      type="range"
      aria-label={label}
      value={value}
      onChange={(e) => onChange(Number(e.target.value))}
      min={min}
      max={max}
      step={step}
      style={{ width: '100%', accentColor: 'var(--panel-accent)' }}
    />
  </div>
);

export const ToggleField: React.FC<{
  label: string;
  value: boolean;
  onChange: (v: boolean) => void;
  help?: string;
}> = ({ label, value, onChange, help }) => (
  <div className="settings-field" style={{ ...fieldStyle, display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
    <div style={{ display: 'flex', flexDirection: 'column', gap: 2, flex: 1, minWidth: 0 }}>
      <label style={{ ...labelStyle, marginBottom: 0 }}>{label}</label>
      {help && (
        <span style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', lineHeight: 1.4 }}>
          {help}
        </span>
      )}
    </div>
    <button
      type="button"
      role="switch"
      aria-checked={value}
      aria-label={label}
      onClick={() => onChange(!value)}
      style={{
        width: 40,
        height: 22,
        borderRadius: 11,
        border: 'none',
        background: value ? 'var(--panel-accent)' : 'var(--panel-toggle-off)',
        position: 'relative',
        cursor: 'pointer',
        transition: 'background 0.2s ease',
      }}
    >
      <span
        style={{
          position: 'absolute',
          top: 2,
          left: value ? 20 : 2,
          width: 18,
          height: 18,
          borderRadius: '50%',
          background: 'var(--panel-surface)',
          transition: 'left 0.2s ease',
          boxShadow: 'var(--panel-shadow-subtle)',
        }}
      />
    </button>
  </div>
);
