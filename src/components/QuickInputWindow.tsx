import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emit, listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import InputDialog from './InputDialog';

interface InputConfig { character_id: string; broadcast: boolean; auto_voice: boolean }

export default function QuickInputWindow() {
  const [config, setConfig] = useState<InputConfig | null>(null);
  const close = useCallback(() => {
    setConfig(null);
    void getCurrentWindow().hide().catch(console.warn);
  }, []);
  useEffect(() => {
    let disposed = false;
    const cleanups: Array<() => void> = [];
    void (async () => {
      const configured = await listen<InputConfig>('quick_input:configure', event => {
        if (!disposed) setConfig(event.payload);
      });
      if (disposed) { configured(); return; }
      cleanups.push(configured);
      const focused = await getCurrentWindow().onFocusChanged(({ payload }) => {
        if (!disposed && !payload) close();
      });
      if (disposed) { focused(); return; }
      cleanups.push(focused);
      const applyTheme = (theme: unknown) => {
        document.documentElement.dataset.theme = theme === 'light' || theme === 'dark' ? theme : 'system';
      };
      const themed = await listen<{ theme: string }>('config:theme-changed', event => {
        if (!disposed) applyTheme(event.payload.theme);
      });
      if (disposed) { themed(); return; }
      cleanups.push(themed);
      const theme = await invoke<string | null>('get_config', { key: 'base.theme' }).catch(() => null);
      if (disposed) return;
      applyTheme(theme);
      await invoke('quick_input_ready');
    })().catch(console.warn);
    return () => { disposed = true; cleanups.forEach(cleanup => cleanup()); };
  }, [close]);
  if (!config) return null;
  return <InputDialog key={`${config.character_id}:${config.broadcast}`} standalone visible
    characterId={config.character_id} broadcast={config.broadcast} autoStartVoice={config.auto_voice}
    onClose={close} onSend={(text, whisper) => {
      void emit('quick_input:send_message', { text, whisper, character_id: config.character_id });
    }} />;
}
