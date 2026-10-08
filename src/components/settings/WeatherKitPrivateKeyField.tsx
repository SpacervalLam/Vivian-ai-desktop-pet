import React, { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { fieldStyle, inputStyle, labelStyle } from './SettingsFields';

export default function WeatherKitPrivateKeyField({ value, onChange }: {
  value: string; onChange: (value: string) => void;
}) {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(false);
  return (
    <div style={fieldStyle}>
      <label htmlFor="weatherkit-private-key" style={labelStyle}>{t('config.world_apple_weather_key')}</label>
      {visible ? (
        <textarea id="weatherkit-private-key" rows={5} value={value} spellCheck={false}
          autoComplete="off" onChange={(e) => onChange(e.target.value)} style={inputStyle} />
      ) : (
        <input id="weatherkit-private-key" type="password" value={value} autoComplete="new-password"
          onChange={(e) => onChange(e.target.value)} style={inputStyle}
          onPaste={(e) => {
            const text = e.clipboardData.getData('text');
            if (text.includes('-----BEGIN PRIVATE KEY-----')) { e.preventDefault(); onChange(text); }
          }} />
      )}
      <button type="button" onClick={() => setVisible((v) => !v)} aria-controls="weatherkit-private-key"
        aria-pressed={visible}>{visible ? t('config.world_apple_weather_hide_key') : t('config.world_apple_weather_show_key')}</button>
      <div style={{ fontSize: 11, color: 'var(--panel-text-tertiary)', marginTop: 6 }}>
        {t('config.world_apple_weather_key_help')}
      </div>
    </div>
  );
}
