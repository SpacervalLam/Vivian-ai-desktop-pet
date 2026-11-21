/**
 * 图片大图查看器（全屏遮罩 + 居中放大）
 *
 * 供聊天窗口与记忆管理面板共用：点击图片缩略图 / 描述文字时呼出，
 * 点击遮罩或按 Esc 关闭。支持超大图自适应缩放（不超过视口 92%）。
 */
import React, { useEffect } from 'react';

interface ImageViewerProps {
  /** 图片 data URL 或可访问 URL；为空时不渲染 */
  src: string | null;
  /** 关闭回调 */
  onClose: () => void;
  /** 顶部标题（可选） */
  title?: string;
}

const ImageViewer: React.FC<ImageViewerProps> = ({ src, onClose, title }) => {
  useEffect(() => {
    if (!src) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [src, onClose]);

  if (!src) return null;

  return (
    <div
      onClick={onClose}
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 9999,
        background: 'rgba(0,0,0,0.88)',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 24,
        backdropFilter: 'blur(6px)',
        WebkitBackdropFilter: 'blur(6px)',
        animation: 'vivian-fade-in 160ms ease-out',
      }}
    >
      <div
        style={{
          position: 'absolute',
          top: 16,
          right: 16,
          width: 36,
          height: 36,
          borderRadius: '50%',
          background: 'rgba(255,255,255,0.14)',
          border: 'none',
          color: '#fff',
          fontSize: 20,
          cursor: 'pointer',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
        }}
        onClick={(e) => {
          e.stopPropagation();
          onClose();
        }}
        title="关闭 (Esc)"
      >
        ×
      </div>
      {title && (
        <div
          style={{
            color: 'rgba(255,255,255,0.7)',
            fontSize: 13,
            marginBottom: 12,
            maxWidth: '80%',
            textAlign: 'center',
            lineHeight: 1.5,
          }}
        >
          {title}
        </div>
      )}
      <img
        src={src}
        alt={title || '图片预览'}
        onClick={(e) => e.stopPropagation()}
        style={{
          maxWidth: '92vw',
          maxHeight: '88vh',
          objectFit: 'contain',
          borderRadius: 8,
          boxShadow: '0 8px 40px rgba(0,0,0,0.6)',
          userSelect: 'none',
          WebkitUserDrag: 'none',
        } as React.CSSProperties}
      />
      <style>{`@keyframes vivian-fade-in{from{opacity:0}to{opacity:1}}`}</style>
    </div>
  );
};

export default ImageViewer;
