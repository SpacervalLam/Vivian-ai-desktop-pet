/**
 * 长按进度环（长按桌宠打开心智观察器的视觉反馈）
 *
 * 按住超过显示延迟后由 App 挂载到鼠标按住位置：底层半透明深色轨道 +
 * 上层白色进度弧。进度弧以 stroke-dashoffset 动画从 12 点方向顺时针
 * 填满整圈，填满即视为长按成立，回调 onComplete。
 *
 * 完成信号以 onAnimationEnd 为主、等时长 setTimeout 兜底（防系统禁用
 * 动画等场景下动画事件不触发导致回调丢失）；重复触发由调用方幂等去重。
 * 组件不响应任何指针事件，不影响按住期间的拖拽/穿透判定。
 */
import { useEffect, useRef } from 'react';

const SIZE = 48;
const STROKE = 4.5;
const R = (SIZE - STROKE) / 2;
const CIRCUMFERENCE = 2 * Math.PI * R;

/** keyframes 注入 <style> 标签的 id（幂等） */
const KEYFRAMES_ID = 'hold-progress-ring-keyframes';

function ensureKeyframes(): void {
  if (document.getElementById(KEYFRAMES_ID)) return;
  const style = document.createElement('style');
  style.id = KEYFRAMES_ID;
  style.textContent = `
@keyframes hold-ring-fill {
  from { stroke-dashoffset: ${CIRCUMFERENCE}; }
  to { stroke-dashoffset: 0; }
}
@keyframes hold-ring-fade-in {
  from { opacity: 0; }
  to { opacity: 1; }
}`;
  document.head.appendChild(style);
}

interface HoldProgressRingProps {
  /** 环心横坐标（client 坐标，逻辑像素） */
  x: number;
  /** 环心纵坐标（client 坐标，逻辑像素） */
  y: number;
  /** 填满整环的动画时长（毫秒） */
  durationMs: number;
  /** 进度环填满时的回调（只会触发一次） */
  onComplete: () => void;
}

export function HoldProgressRing({ x, y, durationMs, onComplete }: HoldProgressRingProps) {
  const completedRef = useRef(false);
  const onCompleteRef = useRef(onComplete);
  onCompleteRef.current = onComplete;

  useEffect(() => {
    ensureKeyframes();
    // 兜底计时器：动画事件不可用时仍按时触发（动画正常结束时先到的是
    // onAnimationEnd，此处因 completedRef 幂等不会重复回调）
    const timer = window.setTimeout(() => {
      if (completedRef.current) return;
      completedRef.current = true;
      onCompleteRef.current();
    }, durationMs + 150);
    return () => window.clearTimeout(timer);
  }, [durationMs]);

  const fireComplete = () => {
    if (completedRef.current) return;
    completedRef.current = true;
    onCompleteRef.current();
  };

  return (
    <div
      style={{
        position: 'fixed',
        left: x,
        top: y,
        width: SIZE,
        height: SIZE,
        transform: 'translate(-50%, -50%)',
        pointerEvents: 'none',
        zIndex: 2147483647,
        animation: 'hold-ring-fade-in .15s ease-out both',
      }}
    >
      <svg width={SIZE} height={SIZE} viewBox={`0 0 ${SIZE} ${SIZE}`}>
        <circle
          cx={SIZE / 2}
          cy={SIZE / 2}
          r={R}
          fill="none"
          stroke="rgba(0, 0, 0, 0.30)"
          strokeWidth={STROKE}
        />
        <circle
          cx={SIZE / 2}
          cy={SIZE / 2}
          r={R}
          fill="none"
          stroke="#FFFFFF"
          strokeWidth={STROKE}
          strokeLinecap="round"
          strokeDasharray={CIRCUMFERENCE}
          strokeDashoffset={CIRCUMFERENCE}
          transform={`rotate(-90 ${SIZE / 2} ${SIZE / 2})`}
          style={{
            animation: `hold-ring-fill ${durationMs}ms linear forwards`,
            filter: 'drop-shadow(0 1px 2px rgba(0, 0, 0, 0.45))',
          }}
          onAnimationEnd={(e) => {
            if (e.animationName === 'hold-ring-fill') fireComplete();
          }}
        />
      </svg>
    </div>
  );
}

export default HoldProgressRing;
