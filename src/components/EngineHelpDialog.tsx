/**
 * 引擎说明书弹窗（TTS / ASR 共用外壳）
 *
 * 说明书的排版与交互只在这里实现一次，TTS、ASR 两侧只负责把 i18n 文案摊成
 * `EngineHelpDoc[]`。这样两处说明书的视觉、键盘行为、深浅色适配不会各自跑偏。
 *
 * 结构：
 *   ┌──────────────────────────────────────┐
 *   │ 图标  标题 / 副标题              ✕   │  头部
 *   ├────────────┬─────────────────────────┤
 *   │ 引擎列表   │ 是什么 / 准备工作 /      │  主体
 *   │ (左侧栏)   │ 字段含义 / 推荐 / FAQ    │
 *   ├────────────┴─────────────────────────┤
 *   │ ESC 关闭 · ← → 切换引擎   ‹ ›  关闭   │  底部
 *   └──────────────────────────────────────┘
 *
 * 配色全部走 `--panel-*` 主题变量，深色/浅色各一套，不再硬编码纸色。
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { BookOpen, ChevronLeft, ChevronRight, X } from 'lucide-react';

/** 单个引擎的说明书内容（已本地化） */
export interface EngineHelpDoc {
  /** 与引擎枚举一一对应的稳定 id */
  key: string;
  /** 展示名，如 Edge-TTS / 豆包 */
  name: string;
  /** 这是什么 */
  what: string;
  /** 准备工作，逐条一句（允许保留原文里的「1. 」前缀，渲染时会被剥掉） */
  prep: string[];
  /** 各字段含义 */
  fields: { name: string; desc: string }[];
  /** 推荐配置 */
  recommend: string;
  /** 常见问题 */
  faq: { q: string; a: string }[];
}

/** 章节标题的本地化文案 */
export interface EngineHelpSectionLabels {
  what: string;
  prep: string;
  fields: string;
  recommend: string;
  faq: string;
}

interface EngineHelpDialogProps {
  open: boolean;
  /** 打开时定位到哪个引擎 */
  initialKey: string;
  docs: EngineHelpDoc[];
  title: string;
  subtitle: string;
  /** 左栏列表标题，如「TTS 后端」 */
  listLabel?: string;
  sectionLabels: EngineHelpSectionLabels;
  onClose: () => void;
}

/** 关闭动画时长，与 CSS keyframes 保持一致 */
const CLOSE_MS = 180;

const EngineHelpDialog: React.FC<EngineHelpDialogProps> = ({
  open,
  initialKey,
  docs,
  title,
  subtitle,
  listLabel,
  sectionLabels,
  onClose,
}) => {
  const { t } = useTranslation();
  const [activeKey, setActiveKey] = useState(initialKey);
  // visible：是否挂载；closing：是否正在播退出动画
  const [visible, setVisible] = useState(open);
  const [closing, setClosing] = useState(false);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  // 打开时定位到传入的引擎
  useEffect(() => {
    if (open) setActiveKey(initialKey);
  }, [open, initialKey]);

  // open → visible/closing 状态机（保留退出动画）
  useEffect(() => {
    if (open) {
      setVisible(true);
      setClosing(false);
      return;
    }
    if (!visible) return;
    setClosing(true);
    const timer = setTimeout(() => {
      setVisible(false);
      setClosing(false);
    }, CLOSE_MS);
    return () => clearTimeout(timer);
    // 故意不依赖 visible：否则 visible 变化会重启动画计时
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const index = useMemo(() => {
    const i = docs.findIndex((d) => d.key === activeKey);
    return i < 0 ? 0 : i;
  }, [docs, activeKey]);

  const step = useCallback(
    (delta: number) => {
      if (docs.length === 0) return;
      const next = (index + delta + docs.length) % docs.length;
      setActiveKey(docs[next].key);
    },
    [docs, index],
  );

  // ESC 关闭、左右键切换引擎
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
      } else if (e.key === 'ArrowRight') {
        step(1);
      } else if (e.key === 'ArrowLeft') {
        step(-1);
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [open, onClose, step]);

  // 切换引擎后内容回到顶部，避免停在上一页的滚动位置
  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
  }, [activeKey]);

  if (!visible) return null;

  const active = docs[index];

  return (
    <>
      <div
        onClick={onClose}
        style={{
          position: 'fixed',
          inset: 0,
          zIndex: 1200,
          background: 'var(--panel-overlay)',
          backdropFilter: 'blur(10px) saturate(120%)',
          WebkitBackdropFilter: 'blur(10px) saturate(120%)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          padding: '4vh 3vw',
          animation: closing
            ? `engineHelpMaskOut ${CLOSE_MS}ms ease-in forwards`
            : 'engineHelpMaskIn 160ms ease-out',
        }}
      >
        <div
          role="dialog"
          aria-modal="true"
          aria-label={title}
          onClick={(e) => e.stopPropagation()}
          style={{
            width: 'min(900px, 94vw)',
            maxHeight: '82vh',
            display: 'flex',
            flexDirection: 'column',
            overflow: 'hidden',
            borderRadius: 18,
            background: 'var(--panel-surface)',
            border: '1px solid var(--panel-border)',
            boxShadow: 'var(--panel-shadow-elevated)',
            animation: closing
              ? `engineHelpOut ${CLOSE_MS}ms cubic-bezier(0.4, 0, 1, 1) forwards`
              : 'engineHelpIn 200ms cubic-bezier(0.16, 1, 0.3, 1)',
          }}
        >
          {/* ── 头部 ── */}
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 12,
              padding: '16px 18px 16px 20px',
              borderBottom: '1px solid var(--panel-border-light)',
              background: 'var(--panel-elevated)',
              flexShrink: 0,
            }}
          >
            <div
              style={{
                width: 34,
                height: 34,
                borderRadius: 10,
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                background: 'var(--panel-accent-muted)',
                color: 'var(--panel-accent)',
                flexShrink: 0,
              }}
            >
              <BookOpen size={17} strokeWidth={1.8} />
            </div>
            <div style={{ flex: 1, minWidth: 0 }}>
              <div
                style={{
                  fontSize: 15,
                  fontWeight: 600,
                  color: 'var(--panel-text)',
                  letterSpacing: 0.2,
                  whiteSpace: 'nowrap',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                }}
              >
                {title}
              </div>
              <div
                style={{
                  fontSize: 11.5,
                  color: 'var(--panel-text-tertiary)',
                  marginTop: 2,
                  whiteSpace: 'nowrap',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                }}
              >
                {subtitle}
              </div>
            </div>
            <button
              type="button"
              onClick={onClose}
              aria-label="close"
              style={{
                width: 30,
                height: 30,
                display: 'inline-flex',
                alignItems: 'center',
                justifyContent: 'center',
                borderRadius: 9,
                border: '1px solid transparent',
                background: 'transparent',
                color: 'var(--panel-text-secondary)',
                cursor: 'pointer',
                flexShrink: 0,
              }}
              onMouseEnter={(e) => {
                e.currentTarget.style.background = 'var(--panel-bg-hover)';
                e.currentTarget.style.color = 'var(--panel-text)';
              }}
              onMouseLeave={(e) => {
                e.currentTarget.style.background = 'transparent';
                e.currentTarget.style.color = 'var(--panel-text-secondary)';
              }}
            >
              <X size={16} strokeWidth={2} />
            </button>
          </div>

          {/* ── 主体：左引擎列表 + 右内容 ── */}
          <div style={{ display: 'flex', flex: 1, minHeight: 0 }}>
            <div
              style={{
                width: 198,
                flexShrink: 0,
                padding: '14px 10px',
                borderRight: '1px solid var(--panel-border-light)',
                overflowY: 'auto',
                background: 'var(--panel-bg)',
              }}
            >
              {listLabel && (
                <div
                  style={{
                    fontSize: 10.5,
                    fontWeight: 600,
                    letterSpacing: 0.9,
                    textTransform: 'uppercase',
                    color: 'var(--panel-text-quaternary)',
                    padding: '0 10px 8px',
                  }}
                >
                  {listLabel}
                </div>
              )}
              {docs.map((doc, i) => {
                const isActive = i === index;
                return (
                  <button
                    key={doc.key}
                    type="button"
                    onClick={() => setActiveKey(doc.key)}
                    style={{
                      width: '100%',
                      display: 'flex',
                      alignItems: 'center',
                      gap: 9,
                      padding: '8px 10px',
                      marginBottom: 2,
                      borderRadius: 9,
                      border: '1px solid',
                      borderColor: isActive ? 'var(--panel-accent-muted)' : 'transparent',
                      background: isActive ? 'var(--panel-accent-soft)' : 'transparent',
                      color: isActive ? 'var(--panel-accent-bright)' : 'var(--panel-text-secondary)',
                      fontSize: 12.5,
                      fontWeight: isActive ? 600 : 400,
                      textAlign: 'left',
                      cursor: 'pointer',
                      transition: 'background 120ms ease, color 120ms ease',
                    }}
                    onMouseEnter={(e) => {
                      if (!isActive) e.currentTarget.style.background = 'var(--panel-bg-hover)';
                    }}
                    onMouseLeave={(e) => {
                      if (!isActive) e.currentTarget.style.background = 'transparent';
                    }}
                  >
                    <span
                      style={{
                        width: 3,
                        height: 14,
                        borderRadius: 2,
                        background: isActive ? 'var(--panel-accent)' : 'var(--panel-border)',
                        flexShrink: 0,
                      }}
                    />
                    <span
                      style={{
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {doc.name}
                    </span>
                  </button>
                );
              })}
            </div>

            <div
              ref={scrollRef}
              style={{
                flex: 1,
                minWidth: 0,
                overflowY: 'auto',
                padding: '20px 24px 26px',
              }}
            >
              {active && <EngineHelpDocView doc={active} labels={sectionLabels} />}
            </div>
          </div>

          {/* ── 底部 ── */}
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              gap: 12,
              padding: '10px 16px',
              borderTop: '1px solid var(--panel-border-light)',
              background: 'var(--panel-elevated)',
              flexShrink: 0,
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
              <span
                style={{
                  fontSize: 10,
                  fontWeight: 600,
                  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
                  color: 'var(--panel-text-tertiary)',
                  border: '1px solid var(--panel-border)',
                  borderRadius: 5,
                  padding: '1px 6px',
                  lineHeight: '14px',
                }}
              >
                ESC
              </span>
              <span style={{ fontSize: 10.5, color: 'var(--panel-text-quaternary)' }}>
                {t('common.close')}
              </span>
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
              <NavButton label="prev" onClick={() => step(-1)}>
                <ChevronLeft size={15} strokeWidth={2} />
              </NavButton>
              <span
                style={{
                  fontSize: 11,
                  color: 'var(--panel-text-tertiary)',
                  fontVariantNumeric: 'tabular-nums',
                  minWidth: 34,
                  textAlign: 'center',
                }}
              >
                {index + 1}/{docs.length}
              </span>
              <NavButton label="next" onClick={() => step(1)}>
                <ChevronRight size={15} strokeWidth={2} />
              </NavButton>
              <button
                type="button"
                onClick={onClose}
                style={{
                  marginLeft: 8,
                  padding: '6px 16px',
                  borderRadius: 9,
                  border: '1px solid var(--panel-border)',
                  background: 'var(--panel-bg-surface-elevated)',
                  color: 'var(--panel-text)',
                  fontSize: 12.5,
                  fontWeight: 600,
                  cursor: 'pointer',
                  fontFamily: 'inherit',
                }}
              >
                {t('common.close')}
              </button>
            </div>
          </div>
        </div>
      </div>

      {/* 动画：一次注入 */}
      <style>{`
        @keyframes engineHelpMaskIn { from { opacity: 0 } to { opacity: 1 } }
        @keyframes engineHelpMaskOut { from { opacity: 1 } to { opacity: 0 } }
        @keyframes engineHelpIn {
          from { opacity: 0; transform: translateY(12px) scale(0.985) }
          to { opacity: 1; transform: translateY(0) scale(1) }
        }
        @keyframes engineHelpOut {
          from { opacity: 1; transform: translateY(0) scale(1) }
          to { opacity: 0; transform: translateY(8px) scale(0.99) }
        }
      `}</style>
    </>
  );
};

/** 底部翻页小按钮 */
const NavButton: React.FC<{ label: string; onClick: () => void; children: React.ReactNode }> = ({
  label,
  onClick,
  children,
}) => (
  <button
    type="button"
    aria-label={label}
    onClick={onClick}
    style={{
      width: 26,
      height: 26,
      display: 'inline-flex',
      alignItems: 'center',
      justifyContent: 'center',
      borderRadius: 8,
      border: '1px solid var(--panel-border)',
      background: 'transparent',
      color: 'var(--panel-text-secondary)',
      cursor: 'pointer',
    }}
    onMouseEnter={(e) => {
      e.currentTarget.style.background = 'var(--panel-bg-hover)';
      e.currentTarget.style.color = 'var(--panel-text)';
    }}
    onMouseLeave={(e) => {
      e.currentTarget.style.background = 'transparent';
      e.currentTarget.style.color = 'var(--panel-text-secondary)';
    }}
  >
    {children}
  </button>
);

/** 章节标题 */
const SectionTitle: React.FC<{ children: React.ReactNode; first?: boolean }> = ({
  children,
  first,
}) => (
  <div
    style={{
      display: 'flex',
      alignItems: 'center',
      gap: 8,
      marginTop: first ? 0 : 22,
      marginBottom: 10,
    }}
  >
    <span
      style={{
        width: 14,
        height: 2,
        borderRadius: 1,
        background: 'var(--panel-accent)',
        opacity: 0.75,
        flexShrink: 0,
      }}
    />
    <span
      style={{
        fontSize: 12,
        fontWeight: 600,
        letterSpacing: 0.6,
        textTransform: 'uppercase',
        color: 'var(--panel-text-secondary)',
      }}
    >
      {children}
    </span>
  </div>
);

/** 剥掉「1. 」「2、」这类前缀（i18n 文案自带序号，与步骤徽标重复） */
function stripLeadingNumber(text: string): string {
  return text.replace(/^\s*\d+\s*[.、)．）]\s*/, '');
}

/**
 * 把字段描述拆成「名称 + 正文」
 *
 * 现有文案都写成 `<名称>：<说明>`（英文是 `Name: ...`）。拆开后名称做小标题、
 * 原文的字段 id 退化为代码徽标，比「id + 整段说明（说明里又重复一遍名称）」干净。
 * 没有冒号的文案整体当正文，标题回退到字段 id。
 */
function splitFieldText(desc: string, fallbackTitle: string) {
  const m = desc.match(/^([^：:]{1,24})[：:]\s*([\s\S]+)$/);
  if (!m) return { title: fallbackTitle, body: desc };

  let title = m[1].trim();
  // 文案里常写成「音色 ID（voice_id）：…」，而字段 id 已经单独做成徽标，
  // 括号里再重复一遍就啰嗦了 —— 只剥掉「括号里的 id」这一种形态。
  // 不能做裸子串替换：字段 id 是 `key` 时会把「API Key」削成「API」。
  const bracketed = new RegExp(`[（(]\\s*${escapeRegExp(fallbackTitle)}\\s*[）)]`, 'ig');
  if (bracketed.test(title)) {
    const trimmed = title.replace(bracketed, '').trim();
    if (trimmed) title = trimmed;
  }
  return { title, body: m[2].trim() };
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** 单个引擎的说明书渲染 */
const EngineHelpDocView: React.FC<{ doc: EngineHelpDoc; labels: EngineHelpSectionLabels }> = ({
  doc,
  labels,
}) => (
  <div>
    {/* 这是什么 */}
    <SectionTitle first>{labels.what}</SectionTitle>
    <div
      style={{
        fontSize: 13,
        lineHeight: 1.85,
        color: 'var(--panel-text-secondary)',
        padding: '12px 14px',
        borderRadius: 10,
        background: 'var(--panel-accent-soft)',
        borderLeft: '2px solid var(--panel-accent)',
        whiteSpace: 'pre-wrap',
      }}
    >
      {doc.what}
    </div>

    {/* 准备工作 */}
    {doc.prep.length > 0 && (
      <>
        <SectionTitle>{labels.prep}</SectionTitle>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          {doc.prep.map((raw, i) => (
            <div key={`prep-${i}`} style={{ display: 'flex', gap: 10, alignItems: 'flex-start' }}>
              <span
                style={{
                  width: 19,
                  height: 19,
                  borderRadius: '50%',
                  flexShrink: 0,
                  marginTop: 1,
                  display: 'inline-flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  fontSize: 10.5,
                  fontWeight: 700,
                  fontVariantNumeric: 'tabular-nums',
                  background: 'var(--panel-accent-muted)',
                  color: 'var(--panel-accent)',
                }}
              >
                {i + 1}
              </span>
              <span
                style={{
                  fontSize: 13,
                  lineHeight: 1.8,
                  color: 'var(--panel-text-secondary)',
                }}
              >
                {stripLeadingNumber(raw)}
              </span>
            </div>
          ))}
        </div>
      </>
    )}

    {/* 各字段含义 */}
    {doc.fields.length > 0 && (
      <>
        <SectionTitle>{labels.fields}</SectionTitle>
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            borderRadius: 10,
            border: '1px solid var(--panel-border-light)',
            overflow: 'hidden',
          }}
        >
          {doc.fields.map((f, i) => {
            const { title, body } = splitFieldText(f.desc, f.name);
            return (
              <div
                key={`field-${f.name}`}
                style={{
                  padding: '10px 12px',
                  background: i % 2 === 1 ? 'var(--panel-bg-surface)' : 'transparent',
                }}
              >
                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 8,
                    marginBottom: 3,
                    flexWrap: 'wrap',
                  }}
                >
                  <span style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--panel-text)' }}>
                    {title}
                  </span>
                  <code
                    style={{
                      fontSize: 10.5,
                      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace',
                      color: 'var(--panel-accent)',
                      background: 'var(--panel-accent-soft)',
                      border: '1px solid var(--panel-accent-muted)',
                      borderRadius: 5,
                      padding: '0 5px',
                      lineHeight: '16px',
                    }}
                  >
                    {f.name}
                  </code>
                </div>
                <div
                  style={{
                    fontSize: 12.5,
                    lineHeight: 1.75,
                    color: 'var(--panel-text-secondary)',
                  }}
                >
                  {body}
                </div>
              </div>
            );
          })}
        </div>
      </>
    )}

    {/* 推荐配置 */}
    {doc.recommend && (
      <>
        <SectionTitle>{labels.recommend}</SectionTitle>
        <div
          style={{
            fontSize: 12.5,
            lineHeight: 1.85,
            color: 'var(--panel-text)',
            padding: '11px 14px',
            borderRadius: 10,
            background: 'var(--panel-bg-surface-elevated)',
            border: '1px solid var(--panel-border)',
            borderLeft: '2px solid var(--panel-success)',
            whiteSpace: 'pre-wrap',
          }}
        >
          {doc.recommend}
        </div>
      </>
    )}

    {/* 常见问题 */}
    {doc.faq.length > 0 && (
      <>
        <SectionTitle>{labels.faq}</SectionTitle>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
          {doc.faq.map((item, i) => (
            <div key={`faq-${i}`}>
              <div
                style={{
                  display: 'flex',
                  gap: 8,
                  fontSize: 12.5,
                  fontWeight: 600,
                  color: 'var(--panel-text)',
                  lineHeight: 1.6,
                  marginBottom: 4,
                }}
              >
                <span
                  style={{
                    fontSize: 10.5,
                    fontWeight: 700,
                    color: 'var(--panel-accent)',
                    background: 'var(--panel-accent-muted)',
                    borderRadius: 5,
                    width: 18,
                    height: 18,
                    display: 'inline-flex',
                    alignItems: 'center',
                    justifyContent: 'center',
                    flexShrink: 0,
                    marginTop: 1,
                  }}
                >
                  Q
                </span>
                <span>{item.q.replace(/^Q\s*[:：]\s*/, '')}</span>
              </div>
              <div
                style={{
                  fontSize: 12.5,
                  lineHeight: 1.8,
                  color: 'var(--panel-text-secondary)',
                  paddingLeft: 26,
                }}
              >
                {item.a.replace(/^A\s*[:：]\s*/, '')}
              </div>
            </div>
          ))}
        </div>
      </>
    )}
  </div>
);

export default EngineHelpDialog;
