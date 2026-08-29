import React, { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

/**
 * TTS 后端使用说明书（抽屉式）
 *
 * 从右侧滑入的覆盖层，包含 5 个 TTS 后端的详细使用说明：
 * - EdgeTTS（免费在线）
 * - Azure 认知服务（云端高级）
 * - GPT-SoVITS（自托管克隆）
 * - Fish Speech（自托管/云端克隆）
 * - MiniMax（云端流式合成）
 *
 * 每个后端包含 5 个章节：
 * 1. 这是什么？
 * 2. 准备工作（步骤化）
 * 3. 各字段含义
 * 4. 推荐配置
 * 5. 常见问题（FAQ）
 */
export type TtsBackendKey = 'edgetts' | 'azure' | 'gptsovits' | 'fishspeech' | 'minimax' | 'doubao';

interface TtsHelpDrawerProps {
  open: boolean;
  /** 初始打开的后端页签，默认 edgetts */
  initialBackend?: TtsBackendKey;
  onClose: () => void;
}

const BACKEND_KEYS: TtsBackendKey[] = ['edgetts', 'azure', 'gptsovits', 'fishspeech', 'minimax', 'doubao'];

const TtsHelpDrawer: React.FC<TtsHelpDrawerProps> = ({ open, initialBackend = 'edgetts', onClose }) => {
  const { t } = useTranslation();
  const [activeBackend, setActiveBackend] = React.useState<TtsBackendKey>(initialBackend);
  // visible: 实际挂载状态; closing: 是否正在播放退出动画
  const [visible, setVisible] = useState(open);
  const [closing, setClosing] = useState(false);

  // 打开时同步 initialBackend
  useEffect(() => {
    if (open) {
      setActiveBackend(initialBackend);
    }
  }, [open, initialBackend]);

  // 同步 open → visible / closing 状态机
  useEffect(() => {
    if (open) {
      setVisible(true);
      setClosing(false);
    } else if (visible) {
      // 触发退出动画,动画结束后卸载
      setClosing(true);
      const timer = setTimeout(() => {
        setVisible(false);
        setClosing(false);
      }, 220);
      return () => clearTimeout(timer);
    }
  }, [open]); // 故意不依赖 visible,避免重复触发

  // ESC 关闭
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [open, onClose]);

  if (!visible) return null;

  // 后端名称
  const backendName = (k: TtsBackendKey): string => t(`config.tts_help_${k}_name`);

  // 抽屉动画类:进入 slideInRight,退出 slideOutRight
  const drawerAnim = closing ? 'slideOutRight 0.22s ease-in forwards' : 'slideInRight 0.25s ease-out';
  const maskAnim = closing ? 'fadeOut 0.22s ease-in forwards' : 'fadeIn 0.2s ease-out';

  return (
    <>
      {/* 遮罩层 */}
      <div
        onClick={onClose}
        style={{
          position: 'fixed',
          inset: 0,
          background: 'rgba(0,0,0,0.5)',
          zIndex: 9998,
          animation: maskAnim,
        }}
      />
      {/* 抽屉本体 */}
      <div
        style={{
          position: 'fixed',
          top: 0,
          right: 0,
          bottom: 0,
          width: 'min(560px, 90vw)',
          background: '#FAF8F5',
          borderLeft: '2px solid #D4CFC7',
          boxShadow: '-4px 0 20px rgba(0,0,0,0.08), inset 0 0 60px rgba(250,248,245,0.5)',
          zIndex: 9999,
          display: 'flex',
          flexDirection: 'column',
          animation: drawerAnim,
        }}
      >
        {/* 头部 */}
        <div
          style={{
            padding: '18px 22px',
            borderBottom: '2px solid #D4CFC7',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            flexShrink: 0,
            background: '#F5F3EF',
          }}
        >
          <div>
            <div style={{ fontSize: 20, fontWeight: 600, color: '#2C2C2C', fontFamily: '"Caveat", "Ma Shan Zheng", "Dancing Script", cursive' }}>
              {t('config.tts_help_title')}
            </div>
            <div style={{ fontSize: 12, color: '#8B8680', marginTop: 4 }}>
              {t('config.tts_help_subtitle')}
            </div>
          </div>
          <button
            onClick={onClose}
            style={{
              background: 'transparent',
              border: '1.5px solid #D4CFC7',
              borderRadius: 0,
              color: '#8B8680',
              width: 32,
              height: 32,
              cursor: 'pointer',
              fontSize: 16,
              fontFamily: 'inherit',
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
            }}
            title="ESC"
          >
            ✕
          </button>
        </div>

        {/* 后端页签 */}
        <div
          style={{
            display: 'flex',
            gap: 6,
            padding: '10px 14px',
            borderBottom: '1px solid #E0DCD6',
            overflowX: 'auto',
            flexShrink: 0,
            background: '#FAF8F5',
          }}
        >
          {BACKEND_KEYS.map((k) => (
            <button
              key={k}
              onClick={() => setActiveBackend(k)}
              style={{
                padding: '5px 14px',
                border: '1.5px solid #D4CFC7',
                borderRadius: 0,
                background: activeBackend === k ? '#FFFDE7' : 'transparent',
                color: activeBackend === k ? '#4A4A4A' : '#8B8680',
                fontSize: 13,
                cursor: 'pointer',
                fontFamily: '"Caveat", "Ma Shan Zheng", "Dancing Script", cursive',
                whiteSpace: 'nowrap',
                fontWeight: activeBackend === k ? 600 : 400,
                transform: activeBackend === k ? 'rotate(-1deg)' : 'rotate(0)',
              }}
            >
              {backendName(k)}
            </button>
          ))}
        </div>

        {/* 内容滚动区 */}
        <div
          style={{
            flex: 1,
            overflowY: 'auto',
            padding: '24px 28px',
            backgroundImage: 'radial-gradient(circle, #E8E4DD 1px, transparent 1px)',
            backgroundSize: '20px 20px',
          }}
        >
          <BackendHelpContent backend={activeBackend} />
        </div>
      </div>

      {/* 关键帧动画（一次性注入） */}
      <style>{`
        @keyframes slideInRight {
          from { transform: translateX(100%); opacity: 0; }
          to { transform: translateX(0); opacity: 1; }
        }
        @keyframes slideOutRight {
          from { transform: translateX(0); opacity: 1; }
          to { transform: translateX(100%); opacity: 0; }
        }
        @keyframes fadeIn {
          from { opacity: 0; }
          to { opacity: 1; }
        }
        @keyframes fadeOut {
          from { opacity: 1; }
          to { opacity: 0; }
        }
      `}</style>
    </>
  );
};

/**
 * 单个后端的帮助内容
 *
 * 结构：
 * - 这是什么？(what)
 * - 准备工作 (prep_1..prep_5)
 * - 各字段含义 (fields_*)
 * - 推荐配置 (recommend)
 * - 常见问题 (faq_q/a, 多组)
 */
const BackendHelpContent: React.FC<{ backend: TtsBackendKey }> = ({ backend }) => {
  const { t } = useTranslation();

  // 各后端的字段键列表
  const fieldKeys: Record<TtsBackendKey, string[]> = {
    edgetts: ['rate', 'volume'],
    azure: ['key', 'region', 'style', 'style_degree', 'role', 'pitch', 'output_format'],
    gptsovits: ['url', 'ref_audio', 'prompt_text'],
    fishspeech: ['url', 'key', 'character', 'format', 'ref_audio', 'ref_text'],
    minimax: ['key', 'voice_id', 'model', 'format', 'sample_rate'],
    doubao: ['appid', 'access_token', 'cluster', 'voice_type', 'format', 'sample_rate'],
  };

  // 各后端的 prep 步骤数
  const prepCounts: Record<TtsBackendKey, number> = {
    edgetts: 3,
    azure: 4,
    gptsovits: 5,
    fishspeech: 5,
    minimax: 5,
    doubao: 5,
  };

  // 各后端的 FAQ 问答数
  const faqCounts: Record<TtsBackendKey, number> = {
    edgetts: 2,
    azure: 4,
    gptsovits: 3,
    fishspeech: 3,
    minimax: 3,
    doubao: 3,
  };

  const prepCount = prepCounts[backend];
  const faqCount = faqCounts[backend];
  const fields = fieldKeys[backend];

  const sectionStyle: React.CSSProperties = {
    marginTop: 28,
    marginBottom: 12,
    fontSize: 15,
    fontWeight: 600,
    color: '#4A4A4A',
    paddingBottom: 8,
    borderBottom: '2px solid #D4CFC7',
    fontFamily: '"Caveat", "Ma Shan Zheng", "Dancing Script", cursive',
  };

  const itemStyle: React.CSSProperties = {
    fontSize: 14,
    color: '#5C5C5C',
    lineHeight: 1.8,
    marginBottom: 10,
    paddingLeft: 6,
    fontFamily: '"Ma Shan Zheng", "Caveat", "Dancing Script", cursive',
  };

  return (
    <div>
      {/* 这是什么？ */}
      <div style={sectionStyle}>{t('config.tts_help_section_what')}</div>
      <div style={itemStyle}>{t(`config.tts_help_${backend}_what`)}</div>

      {/* 准备工作 */}
      <div style={sectionStyle}>{t('config.tts_help_section_prep')}</div>
      {Array.from({ length: prepCount }, (_, i) => i + 1).map((n) => (
        <div key={`prep-${n}`} style={itemStyle}>
          {t(`config.tts_help_${backend}_prep_${n}`)}
        </div>
      ))}

      {/* 各字段含义 */}
      <div style={sectionStyle}>{t('config.tts_help_section_fields')}</div>
      {fields.map((f) => (
        <div key={`field-${f}`} style={{ ...itemStyle, paddingLeft: 0 }}>
          <span style={{ fontWeight: 600, color: '#4A4A4A', fontFamily: 'monospace' }}>
            {f}
          </span>
          <span style={{ color: '#5C5C5C', marginLeft: 6 }}>
            {t(`config.tts_help_${backend}_fields_${f}`)}
          </span>
        </div>
      ))}

      {/* 推荐配置 */}
      <div style={sectionStyle}>{t('config.tts_help_section_recommend')}</div>
      <div
        style={{
          ...itemStyle,
          padding: '14px 16px',
          background: '#FFFDE7',
          borderRadius: 0,
          border: '1.5px solid #E8E4DD',
          borderLeft: '4px solid #D4CFC7',
        }}
      >
        {t(`config.tts_help_${backend}_recommend`)}
      </div>

      {/* 常见问题 */}
      <div style={sectionStyle}>{t('config.tts_help_section_faq')}</div>
      {Array.from({ length: faqCount }, (_, i) => i + 1).map((n) => (
        <div key={`faq-${n}`} style={{ marginBottom: 16 }}>
          <div style={{ ...itemStyle, fontWeight: 600, color: '#4A4A4A', marginBottom: 6, fontSize: 14.5 }}>
            {t(`config.tts_help_${backend}_faq_q${n}`)}
          </div>
          <div style={{ ...itemStyle, paddingLeft: 16, fontStyle: 'italic', color: '#6B6B6B' }}>
            {t(`config.tts_help_${backend}_faq_a${n}`)}
          </div>
        </div>
      ))}
    </div>
  );
};

export default TtsHelpDrawer;
