import React, { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

/**
 * 配置引导弹窗
 *
 * 展示关键配置项（主LLM/嵌入模型/语音/路由矩阵）及不配置的后果，
 * 首次打开设置窗口时自动弹出，后续可通过标题栏按钮手动打开。
 * 弹窗语言独立跟随系统语言（zh/en/ja），其余系统语言回退英语。
 */

export interface SetupGuideModalProps {
  open: boolean;
  onClose: () => void;
  onGoLlm: () => void;
  onGoMemory: () => void;
  onGoVoice: () => void;
  /** 跳转到网络搜索配置页签 */
  onGoSearch: () => void;
}

const detectGuideLng = (): string => {
  const locale =
    (typeof navigator !== 'undefined' && navigator.language) || 'en';
  const lower = locale.toLowerCase();
  if (lower.startsWith('zh')) return 'zh-CN';
  if (lower.startsWith('ja')) return 'ja';
  if (lower.startsWith('en')) return 'en';
  return 'en';
};

// 与设置窗口一致的设计令牌
const C = {
  bg: 'var(--panel-bg)',
  border: 'var(--panel-border)',
  textPrimary: 'var(--panel-text)',
  textSecondary: 'var(--panel-text-secondary)',
  cardBg: 'var(--panel-bg-surface)',
  primaryBtn: 'var(--panel-accent)',
  secondaryBtnBg: 'var(--panel-surface)',
} as const;

const SetupGuideModal: React.FC<SetupGuideModalProps> = ({
  open,
  onClose,
  onGoLlm,
  onGoMemory,
  onGoVoice,
  onGoSearch,
}) => {
  const { t } = useTranslation();
  const [show, setShow] = useState(open);
  const [lng] = useState<string>(detectGuideLng);

  useEffect(() => {
    setShow(open);
  }, [open]);

  const handleClose = useCallback(() => {
    setShow(false);
    window.setTimeout(() => onClose(), 160);
  }, [onClose]);

  useEffect(() => {
    if (!show) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        handleClose();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [show, handleClose]);

  if (!show) return null;

  const tr = (key: string) => t(key, { lng });

  const tagColor: Record<string, string> = {
    required: '#E53935',
    recommended: '#FB8C00',
    optional: 'var(--panel-text-tertiary)',
  };

  const pathCard = (path: string, tag: string, tagKey: string, desc: string) => (
    <div
      style={{
        padding: '10px 12px',
        borderRadius: 8,
        background: C.cardBg,
        border: `1.5px solid ${C.border}`,
        marginBottom: 8,
      }}
    >
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: 8,
        }}
      >
        <span
          style={{
            fontSize: 13,
            fontWeight: 600,
            color: C.textPrimary,
          }}
        >
          {path}
        </span>
        <span
          style={{
            fontSize: 11,
            fontWeight: 500,
            color: '#fff',
            background: tagColor[tag] ?? 'var(--panel-text-tertiary)',
            borderRadius: 4,
            padding: '1px 6px',
            lineHeight: 1.5,
            flexShrink: 0,
          }}
        >
          {tr(tagKey)}
        </span>
      </div>
      <div
        style={{
          fontSize: 12,
          color: C.textSecondary,
          marginTop: 4,
          lineHeight: 1.6,
        }}
      >
        {desc}
      </div>
    </div>
  );

  const secondaryBtnStyle: React.CSSProperties = {
    padding: '6px 12px',
    borderRadius: 8,
    fontSize: 12,
    color: C.textPrimary,
    border: `1.5px solid ${C.border}`,
    background: C.secondaryBtnBg,
    cursor: 'pointer',
    fontFamily: 'inherit',
    whiteSpace: 'nowrap',
    boxShadow: '0 1px 2px rgba(0,0,0,0.04)',
  };

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 10000,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'rgba(0, 0, 0, 0.25)',
        backdropFilter: 'blur(2px)',
        WebkitBackdropFilter: 'blur(2px)',
        animation: 'vivian-setup-fade 0.18s ease',
      }}
      onClick={handleClose}
    >
      <style>{`
        @keyframes vivian-setup-fade {
          from { opacity: 0; }
          to { opacity: 1; }
        }
        @keyframes vivian-setup-rise {
          from { opacity: 0; transform: translateY(10px) scale(0.96); }
          to { opacity: 1; transform: translateY(0) scale(1); }
        }
      `}</style>
      <div
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 616,
          maxWidth: '90vw',
          padding: '24px 24px 20px',
          borderRadius: 16,
          background: C.bg,
          border: `1.5px solid ${C.border}`,
          boxShadow: '0 12px 40px rgba(0, 0, 0, 0.15)',
          fontFamily:
            '-apple-system, BlinkMacSystemFont, "PingFang SC", "Microsoft YaHei", "Segoe UI", sans-serif',
          color: C.textPrimary,
          animation: 'vivian-setup-rise 0.22s cubic-bezier(0.2, 0.8, 0.2, 1)',
        }}
      >
        {/* 标题栏 */}
        <div style={{ marginBottom: 14 }}>
          <span
            style={{
              fontSize: 16,
              fontWeight: 700,
              color: C.textPrimary,
            }}
          >
            {tr('config.setup_guide.title')}
          </span>
        </div>

        {/* 说明文字 */}
        <div
          style={{
            fontSize: 13,
            lineHeight: 1.7,
            color: C.textSecondary,
            marginBottom: 14,
            wordBreak: 'break-word',
          }}
        >
          {tr('config.setup_guide.desc')}
        </div>

        {/* 配置项路径卡片 — 按 必填 → 推荐 → 可选 排列 */}
        {pathCard(
          tr('config.setup_guide.llm_path'),
          'required',
          'config.setup_guide.llm_tag',
          tr('config.setup_guide.llm_desc'),
        )}
        {pathCard(
          tr('config.setup_guide.memory_path'),
          'recommended',
          'config.setup_guide.memory_tag',
          tr('config.setup_guide.memory_desc'),
        )}
        {pathCard(
          tr('config.setup_guide.search_path'),
          'recommended',
          'config.setup_guide.search_tag',
          tr('config.setup_guide.search_desc'),
        )}
        {pathCard(
          tr('config.setup_guide.voice_path'),
          'optional',
          'config.setup_guide.voice_tag',
          tr('config.setup_guide.voice_desc'),
        )}
        {pathCard(
          tr('config.setup_guide.routing_path'),
          'optional',
          'config.setup_guide.routing_tag',
          tr('config.setup_guide.routing_desc'),
        )}

        {/* 按钮区 — 强制单行 */}
        <div
          style={{
            display: 'flex',
            gap: 8,
            justifyContent: 'flex-end',
            alignItems: 'center',
            marginTop: 16,
          }}
        >
          <button
            onClick={handleClose}
            style={{
              padding: '6px 10px',
              borderRadius: 8,
              fontSize: 12,
              color: C.textSecondary,
              border: 'none',
              background: 'transparent',
              cursor: 'pointer',
              fontFamily: 'inherit',
              whiteSpace: 'nowrap',
            }}
          >
            {tr('config.setup_guide.later')}
          </button>
          <button onClick={onGoVoice} style={secondaryBtnStyle}>
            {tr('config.setup_guide.go_voice')}
          </button>
          <button onClick={onGoSearch} style={secondaryBtnStyle}>
            {tr('config.setup_guide.go_search')}
          </button>
          <button onClick={onGoMemory} style={secondaryBtnStyle}>
            {tr('config.setup_guide.go_memory')}
          </button>
          <button
            onClick={onGoLlm}
            style={{
              padding: '6px 14px',
              borderRadius: 8,
              fontSize: 12,
              fontWeight: 600,
              color: '#fff',
              border: 'none',
              background: C.primaryBtn,
              cursor: 'pointer',
              fontFamily: 'inherit',
              whiteSpace: 'nowrap',
            }}
          >
            {tr('config.setup_guide.go_llm')}
          </button>
        </div>
      </div>
    </div>
  );
};

export default SetupGuideModal;
