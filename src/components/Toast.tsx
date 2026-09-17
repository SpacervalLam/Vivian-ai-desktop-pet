import React, { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

export type ToastType = 'info' | 'success' | 'error' | 'warning';

/** toast 附带的一键操作，后端/主窗口随 toast:show 下发 */
export interface ToastAction {
  /** switch_theme：一键切换界面主题；open_url：用系统默认浏览器打开 url */
  kind: 'switch_theme' | 'open_url';
  /** switch_theme 时的目标主题 */
  theme?: 'light' | 'dark';
  /** open_url 时的目标地址（仅允许 http(s)，打开前由处理方校验） */
  url?: string;
  /** 按钮文案（按界面语言本地化） */
  label: string;
}

export interface ToastProps {
  message: string;
  type?: ToastType;
  duration?: number;
  /** 0-100 进度百分比，提供时在 toast 底部渲染进度条 */
  progress?: number;
  /** 提供时渲染操作按钮，点击触发 onAction */
  action?: ToastAction;
  onAction?: () => void;
  onClose?: () => void;
}

/**
 * 每类的语义色与图标路径。
 *
 * 颜色一律取主题变量而非硬编码色值：设置面板的暖纸主题有深/浅两套、
 * 且可能跟随系统，写死的蓝绿橙在浅色下会显得脏。
 */
const palette: Record<ToastType, { accent: string; path: string }> = {
  info: { accent: 'var(--panel-info)', path: 'M12 11.2v5.3M12 7.6h.01' },
  success: { accent: 'var(--panel-success)', path: 'M7.2 12.4l3.3 3.3L16.8 8.6' },
  error: { accent: 'var(--panel-danger)', path: 'M8.4 8.4l7.2 7.2M15.6 8.4l-7.2 7.2' },
  warning: { accent: 'var(--panel-warning)', path: 'M12 7.4v6.2M12 16.6h.01' },
};

/**
 * 图标。淡色圆底由 SVG 自身绘制（fill=currentColor + opacity），
 * 避免依赖 color-mix 之类的现代 CSS 特性——WebView2 版本不齐时
 * 会整块失效，而 SVG 的透明度到处都能渲染。
 */
const ToastIcon: React.FC<{ type: ToastType }> = ({ type }) => {
  const { accent, path } = palette[type];
  return (
    <svg
      width={19}
      height={19}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden
      style={{ color: accent, flexShrink: 0, marginTop: 1 }}
    >
      <circle cx={12} cy={12} r={10} fill="currentColor" opacity={0.16} />
      <path
        d={path}
        stroke="currentColor"
        strokeWidth={2.1}
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
};

/**
 * 单个 Toast 视觉单元。定位由父容器决定，组件本身只负责
 * 一条 toast 的外观、入场/出场动画与自动关闭计时。
 */
const Toast: React.FC<ToastProps> = ({
  message,
  type = 'info',
  duration = 3000,
  progress,
  action,
  onAction,
  onClose,
}) => {
  const [visible, setVisible] = useState(false);
  const [hovered, setHovered] = useState(false);
  const [actionHovered, setActionHovered] = useState(false);
  const { t } = useTranslation();
  // onClose 存 ref：父组件每次渲染都传入新的箭头函数，若作为 effect 依赖会导致
  // 每次内容更新都重跑 effect——自动关闭计时器被反复重置，高频更新下的 toast 永不消失
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    setVisible(true);
    // duration <= 0 表示持久 toast（如后台任务进度）：不启动自动关闭计时器，
    // 由外部通过同 key 更新 duration>0 或直接移除来关闭。
    if (duration <= 0) return;
    const hideTimer = window.setTimeout(() => setVisible(false), duration);
    const closeTimer = window.setTimeout(() => onCloseRef.current?.(), duration + 250);
    return () => {
      window.clearTimeout(hideTimer);
      window.clearTimeout(closeTimer);
    };
  }, [duration]);

  const colors = palette[type];
  const closable = typeof onClose === 'function';

  return (
    <div
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        position: 'relative',
        overflow: 'hidden',
        transform: visible ? 'translateX(0) scale(1)' : 'translateX(28px) scale(0.94)',
        opacity: visible ? 1 : 0,
        // easeOutExpo 风格的入场：快速起步、缓慢落定，比线性位移更有分量
        transition:
          'transform 320ms cubic-bezier(0.16, 1, 0.3, 1), opacity 200ms ease',
        display: 'flex',
        alignItems: 'flex-start',
        gap: 10,
        minWidth: 260,
        maxWidth: 380,
        padding: '11px 14px 11px 17px',
        borderRadius: 12,
        background: 'var(--panel-elevated)',
        color: 'var(--panel-text)',
        // 外层投影给层次 + 内顶高光给质感（浅色主题下高光极淡，无副作用）
        boxShadow:
          'var(--panel-shadow-elevated), inset 0 1px 0 rgba(255, 255, 255, 0.05)',
        border: '1px solid var(--panel-border)',
        pointerEvents: 'auto',
        fontFamily: 'inherit',
        fontSize: 13,
        lineHeight: 1.55,
      }}
    >
      {/* 左侧语义色条：扫一眼即可分辨类型，不必先读图标 */}
      <span
        style={{
          position: 'absolute',
          left: 0,
          top: 0,
          bottom: 0,
          width: 3,
          background: colors.accent,
        }}
      />
      <ToastIcon type={type} />
      <span style={{ flex: 1, wordBreak: 'break-word', whiteSpace: 'pre-wrap' }}>
        {message}
        {action && onAction && (
          // 动作文案内联在消息末尾而非独立按钮：label 是「换成浅色」这类短动作词，
          // 跟正文连读才成句，也不额外占布局宽度。
          //
          // 下划线必须常驻，不能只在 hover 时出现：强制浅色主题下
          // `--panel-accent` 与 `--panel-text` 同为 #2C2C2C，只靠颜色区分不了可点击，
          // 下划线是唯一跨三套主题都成立的线索。hover 时改用 accent-bright 提亮反馈。
          <button
            type="button"
            onClick={onAction}
            onMouseEnter={() => setActionHovered(true)}
            onMouseLeave={() => setActionHovered(false)}
            style={{
              display: 'inline',
              margin: '0 0 0 6px',
              padding: 0,
              border: 'none',
              background: 'none',
              fontFamily: 'inherit',
              fontSize: 'inherit',
              lineHeight: 'inherit',
              fontWeight: 500,
              color: actionHovered
                ? 'var(--panel-accent-bright)'
                : 'var(--panel-accent)',
              cursor: 'pointer',
              textDecoration: 'underline',
              textUnderlineOffset: 3,
              textDecorationThickness: 1,
              transition: 'color 0.15s ease',
            }}
          >
            {action.label}
          </button>
        )}
      </span>
      {closable && (
        <button
          type="button"
          aria-label={t('common.close')}
          onClick={() => onCloseRef.current?.()}
          style={{
            flexShrink: 0,
            // 关闭按钮固定在右上角（多行文本时也符合直觉），不再随动作按钮居中
            alignSelf: 'flex-start',
            // 常驻占位以保证文本宽度稳定，仅靠透明度控制显隐
            opacity: hovered ? 0.7 : 0,
            marginTop: -2,
            marginRight: -6,
            width: 20,
            height: 20,
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            border: 'none',
            borderRadius: 6,
            background: 'transparent',
            color: 'inherit',
            fontSize: 15,
            lineHeight: 1,
            cursor: 'pointer',
            transition: 'opacity 0.15s ease',
          }}
        >
          ×
        </button>
      )}
      {progress != null && progress >= 0 && (
        <div
          style={{
            position: 'absolute',
            bottom: 0,
            left: 0,
            height: 2,
            // 上限/下限都夹紧：progress 为 0 时不能算出负宽度
            width: `${Math.min(100, Math.max(0, progress))}%`,
            background: colors.accent,
            opacity: 0.85,
            transition: 'width 0.3s ease',
          }}
        />
      )}
    </div>
  );
};

export default Toast;
