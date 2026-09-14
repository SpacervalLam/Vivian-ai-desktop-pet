import { useMemo, useRef } from 'react';
import { setCharacterId } from '../characterContext';
import { ChibiPetCanvas, type ChibiPetCanvasHandle } from './ChibiPetCanvas';

export default function SkeletalRigPreview() {
  const petRef = useRef<ChibiPetCanvasHandle | null>(null);
  const character = useMemo(() => {
    const value = new URLSearchParams(window.location.search).get('character');
    return value?.toLowerCase() === 'nana' ? 'nana' : 'vivian';
  }, []);
  const previewScale = 2;
  setCharacterId(character);
  const controlButtonStyle = {
    minWidth: 112,
    height: 42,
    padding: '0 18px',
    border: '1px solid rgba(91, 61, 125, .28)',
    borderRadius: 12,
    background: '#76549a',
    color: '#fff',
    fontSize: 15,
    fontWeight: 700,
    lineHeight: '42px',
    cursor: 'pointer',
    boxShadow: '0 7px 18px rgba(77, 48, 105, .22)',
  } as const;

  return (
    <main
      style={{
        minHeight: '100vh',
        display: 'flex',
        flexDirection: 'column',
        gap: 18,
        alignItems: 'center',
        justifyContent: 'center',
        background: 'linear-gradient(135deg, #f4effb, #fff8fb)',
      }}
    >
      <section
        style={{
          position: 'relative',
          width: 213.2 * previewScale,
          height: 246.8 * previewScale,
          overflow: 'hidden',
          borderRadius: 20,
          background: 'repeating-conic-gradient(#e9e4ef 0 25%, #fff 0 50%) 50% / 18px 18px',
          boxShadow: '0 20px 60px rgba(60, 39, 80, .18)',
        }}
      >
        <ChibiPetCanvas ref={petRef} previewMode ambientMotionEnabled={false} />
      </section>
      <nav
        style={{
          position: 'fixed',
          top: 22,
          right: 22,
          zIndex: 100,
          display: 'flex',
          flexDirection: 'column',
          gap: 10,
          padding: 14,
          border: '1px solid rgba(91, 61, 125, .16)',
          borderRadius: 16,
          background: 'rgba(255, 255, 255, .94)',
          boxShadow: '0 14px 40px rgba(60, 39, 80, .18)',
        }}
        aria-label="逐帧动画预览控制"
      >
        <strong style={{ color: '#51396d', fontSize: 14, textAlign: 'center' }}>逐帧动画预览</strong>
        <button style={controlButtonStyle} type="button" onClick={() => petRef.current?.previewWalk('left')}>
          向左走
        </button>
        <button style={controlButtonStyle} type="button" onClick={() => petRef.current?.previewWalk('right')}>
          向右走
        </button>
        <button style={controlButtonStyle} type="button" onClick={() => petRef.current?.previewTurn('left')}>
          正面转向左
        </button>
        <button style={controlButtonStyle} type="button" onClick={() => petRef.current?.previewTurn('right')}>
          正面转向右
        </button>
        <button style={controlButtonStyle} type="button" onClick={() => petRef.current?.previewBlink()}>
          眨眼
        </button>
      </nav>
    </main>
  );
}
