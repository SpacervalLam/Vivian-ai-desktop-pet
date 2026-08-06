import { forwardRef, useEffect, useState, type ComponentProps } from 'react';
import { Live2DCanvas, type Live2DCanvasHandle } from './Live2DCanvas';
import type { ModelKind } from '../types';

export type ModelRendererHandle = Live2DCanvasHandle;
export type ModelCanvasProps = ComponentProps<typeof Live2DCanvas>;

function detectKind(kind: string | undefined): ModelKind {
  const k = (kind ?? 'live2d').toLowerCase();
  if (k === 'mmd' || k === 'vrm' || k === 'pngtuber') return k;
  return 'live2d';
}

export const ModelCanvas = forwardRef<ModelRendererHandle, ModelCanvasProps>(
  (props, ref) => {
    const [kind, setKind] = useState<ModelKind>('live2d');

    useEffect(() => {
      (async () => {
        try {
          const { invoke } = await import('@tauri-apps/api/core');
          const { getCharacterId } = await import('../characterContext');
          const cid = getCharacterId() ?? undefined;
          const info = await invoke<{ model_kind?: string }>('get_model_info', { characterId: cid });
          setKind(detectKind(info.model_kind));
        } catch {
          // 保持默认 live2d
        }
      })();
    }, []);

    if (kind === 'live2d') {
      return <Live2DCanvas ref={ref} {...props} />;
    }

    return (
      <div
        style={{
          width: '100%',
          height: '100%',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          color: '#888',
          fontSize: '14px',
          userSelect: 'none',
        }}
      >
        {kind.toUpperCase()} 渲染器尚未实现
      </div>
    );
  }
);

ModelCanvas.displayName = 'ModelCanvas';
