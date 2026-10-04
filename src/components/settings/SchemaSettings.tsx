import React, { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { NumberField } from './SettingsFields';

interface Setting {
  key: string;
  type: 'integer';
  min: number;
  max: number;
  step: number;
  default: number;
  title: Record<string, string>;
}

/** Settings owned by the backend module supply their defaults, labels and bounds. */
export default function SchemaSettings({ command, language, get, set }: {
  command: string;
  language: string;
  get: (key: string, fallback: number) => number;
  set: (key: string, value: number) => void;
}) {
  const [settings, setSettings] = useState<Setting[]>([]);
  const [error, setError] = useState('');
  useEffect(() => {
    let alive = true;
    setSettings([]); setError('');
    void invoke<Setting[]>(command).then(value => { if (alive) setSettings(value); })
      .catch(reason => { if (alive) setError(String(reason)); });
    return () => { alive = false; };
  }, [command]);
  const locale = language.split('-')[0];
  return <>
    {error && <p role="alert">{error}</p>}
    {settings.map(setting => <NumberField key={setting.key}
      label={setting.title[locale] ?? setting.title.en ?? setting.key}
      value={get(setting.key, setting.default)} min={setting.min} max={setting.max} step={setting.step}
      onChange={value => { if (Number.isFinite(value)) set(setting.key, Math.min(setting.max, Math.max(setting.min, Math.round(value)))); }} />)}
  </>;
}
