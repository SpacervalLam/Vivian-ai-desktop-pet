/**
 * 长按进度环（长按桌宠打开心智观察器的视觉反馈）
 *
 * 按住超过显示延迟后由 App 挂载到鼠标按住位置：底层半透明深色轨道 +
 * 上层白色进度弧。进度弧以 stroke-dashoffset 从 12 点方向顺时针填满整圈，
 * 填满即视为长按成立，回调 onComplete。
 *
 * 动画用 Web Animations API 驱动，而不是写成 CSS 声明式动画：
 * CSS 动画要求元素的 `animation-name` 在**首次样式计算时**就能匹配到 @keyframes，
 * 而这里的时长与周界都由 props 和圆几何在运行时算出。此前实现是往 <head> 里
 * 注入 @keyframes，但元素先带着 animation 名出现、规则后到，Chromium 不会为它
 * 回溯启动动画——进度弧会一直停在 0 长度（只剩一条空槽），最终只有兜底定时器
 * 在收尾。WAAPI 的关键帧是调用时构造的动画对象，不存在这个时序窗口。
 *
 * 完成信号以动画的 finished 为准（动画被取消时它会 reject，这种情况不算完成）；
 * 拿不到动画能力时直接判满，保证长按不会被卡住。组件不响应任何指针事件，
 * 不影响按住期间的拖拽/穿透判定。
 */
import { useCallback, useEffect, useRef } from 'react';

const SIZE = 48;
const STROKE = 4.5;
const R = (SIZE - STROKE) / 2;
const CIRCUMFERENCE = 2 * Math.PI * R;

/** 环出现时的淡入时长（毫秒） */
const FADE_IN_MS = 150;

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
  const rootRef = useRef<HTMLDivElement>(null);
  const arcRef = useRef<SVGCircleElement>(null);
  const completedRef = useRef(false);
  const onCompleteRef = useRef(onComplete);
  onCompleteRef.current = onComplete;

  const fireComplete = useCallback(() => {
    if (completedRef.current) return;
    completedRef.current = true;
    onCompleteRef.current();
  }, []);

  useEffect(() => {
    const root = rootRef.current;
    const arc = arcRef.current;
    if (!arc) return;

    const animations: Animation[] = [];
    /** 起一段 WAAPI 动画；元素缺失或内核不支持时返回 null，由调用方决定降级 */
    const start = (
      el: Element | null,
      keyframes: Keyframe[],
      options: KeyframeAnimationOptions,
    ): Animation | null => {
      if (!el) return null;
      try {
        const anim = el.animate(keyframes, options);
        animations.push(anim);
        return anim;
      } catch {
        return null;
      }
    };

    const reduced = !!window.matchMedia?.('(prefers-reduced-motion: reduce)').matches;
    if (reduced) {
      // 减弱动态效果：不播任何动画，直接判满，长按成立与否不受影响
      fireComplete();
      return;
    }

    start(root, [{ opacity: 0 }, { opacity: 1 }], {
      duration: FADE_IN_MS,
      easing: 'ease-out',
      fill: 'forwards',
    });

    const fill = start(
      arc,
      [{ strokeDashoffset: `${CIRCUMFERENCE}px` }, { strokeDashoffset: '0px' }],
      { duration: durationMs, easing: 'linear', fill: 'forwards' },
    );
    if (!fill) {
      // 拿不到动画能力时不阻塞长按：立即判满
      fireComplete();
      return;
    }

    let cancelled = false;
    fill.finished
      .then(() => {
        if (!cancelled) fireComplete();
      })
      .catch(() => {
        /* 被 cancel 中断（卸载 / StrictMode 二次挂载）→ 不算完成 */
      });

    return () => {
      cancelled = true;
      animations.forEach((a) => a.cancel());
    };
  }, [durationMs, fireComplete]);

  return (
    <div
      ref={rootRef}
      style={{
        position: 'fixed',
        left: x,
        top: y,
        width: SIZE,
        height: SIZE,
        transform: 'translate(-50%, -50%)',
        pointerEvents: 'none',
        zIndex: 2147483647,
        // 无动画能力时兜底可见（有动画时由 WAAPI 覆盖）
        opacity: 1,
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
          ref={arcRef}
          cx={SIZE / 2}
          cy={SIZE / 2}
          r={R}
          fill="none"
          stroke="#FFFFFF"
          strokeWidth={STROKE}
          strokeLinecap="round"
          strokeDasharray={CIRCUMFERENCE}
          // 初值即「零进度」：动画建立前的第一帧不能先闪一整圈
          strokeDashoffset={CIRCUMFERENCE}
          // rotate(-90) 把 dash 起点从 3 点挪到 12 点；弧沿路径正方向（顺时针）增长
          transform={`rotate(-90 ${SIZE / 2} ${SIZE / 2})`}
          style={{ filter: 'drop-shadow(0 1px 2px rgba(0, 0, 0, 0.45))' }}
        />
      </svg>
    </div>
  );
}

export default HoldProgressRing;
