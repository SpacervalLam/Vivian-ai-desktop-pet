/**
 * NoteWysiwyg — 笔记可视化（所见即所得）编辑器
 *
 * 在编辑模式下，将笔记 blocks 渲染为接近预览的效果，用户可直接：
 * - 点击选中某个 HTML 元素块（出现选中高亮 + 顶部浮动样式工具条）
 * - 直接在元素上编辑文本（contentEditable，失焦提交）
 * - 通过工具条修改颜色 / 字号 / 加粗 / 斜体 / 对齐
 *
 * 仅文本类块（heading/paragraph/card/quote/list/callout）支持行内文本与样式编辑；
 * 非文本块（图表/Mermaid/图片等）选中后仅提供删除 / 上下移动。
 */

import React, { useCallback, useRef, useState } from 'react';
import {
  Trash2,
  ChevronUp,
  ChevronDown,
  ChevronLeft,
  AlignLeft,
  AlignCenter,
  AlignRight,
  Bold,
  Italic,
  Type,
} from 'lucide-react';
import { COLORS, TYPO, SPACING, EASE, DURATION } from '../design-system';
import {
  Block,
  BlockStyle,
  isStyledBlock,
  blockStyleToCss,
} from './notebook-types';

// 预设文本颜色
const COLOR_OPTIONS: { label: string; value: string }[] = [
  { label: '默认', value: '' },
  { label: '红', value: '#e74c3c' },
  { label: '橙', value: '#e67e22' },
  { label: '金', value: '#d4a017' },
  { label: '绿', value: '#27ae60' },
  { label: '青', value: '#16a085' },
  { label: '蓝', value: '#2980b9' },
  { label: '紫', value: '#8e44ad' },
  { label: '粉', value: '#e84393' },
];

const FONT_SIZES = [12, 14, 16, 18, 20, 24, 28, 32, 40];

const TOOL_BTN: React.CSSProperties = {
  display: 'inline-flex',
  alignItems: 'center',
  gap: 4,
  padding: '5px 9px',
  borderRadius: '2px 10px 2px 10px',
  border: `1.5px solid ${COLORS.border}`,
  background: COLORS.bgSurface,
  color: COLORS.textSecondary,
  fontSize: 12,
  cursor: 'pointer',
  fontFamily: TYPO.fontFamily,
  transition: `all ${DURATION.fast}s ${EASE.swift}`,
};

// ============================================================
// 行内可编辑文本
// ============================================================

const EditableText: React.FC<{
  value: string;
  onCommit: (v: string) => void;
  style?: React.CSSProperties;
  tag?: 'div' | 'span';
  placeholder?: string;
}> = ({ value, onCommit, style, tag = 'div', placeholder }) => {
  const ref = useRef<HTMLDivElement>(null);
  const [focused, setFocused] = useState(false);

  // 外部值变化且未聚焦时同步到 DOM（避免编辑中光标跳位）
  React.useEffect(() => {
    if (!focused && ref.current && ref.current.textContent !== value) {
      ref.current.textContent = value;
    }
  }, [value, focused]);

  const Tag = tag as 'div';

  return (
    <Tag
      ref={ref}
      contentEditable
      suppressContentEditableWarning
      spellCheck={false}
      style={{
        ...style,
        outline: 'none',
        minHeight: 8,
        cursor: 'text',
        caretColor: COLORS.accent,
        borderRadius: 2,
      }}
      data-placeholder={placeholder}
      onFocus={() => {
        setFocused(true);
        const el = ref.current;
        if (el && value === '') {
          // 空内容聚焦时补一个零宽空格以定位光标
          el.textContent = '\u200b';
        }
      }}
      onBlur={(e) => {
        setFocused(false);
        let v = ref.current?.textContent ?? '';
        v = v.replace(/\u200b/g, '').trimEnd();
        onCommit(v);
      }}
      onKeyDown={(e) => {
        if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          (e.currentTarget as HTMLDivElement).blur();
        }
      }}
    />
  );
};

// ============================================================
// 样式工具条
// ============================================================

const StyleToolbar: React.FC<{
  style: BlockStyle;
  onPatch: (patch: Partial<BlockStyle>) => void;
}> = ({ style, onPatch }) => {
  const activeBtn = (active: boolean): React.CSSProperties => ({
    ...TOOL_BTN,
    background: active ? COLORS.accentMuted : COLORS.bgSurface,
    color: active ? COLORS.accentBright : COLORS.textSecondary,
    borderColor: active ? COLORS.borderAccent : COLORS.border,
  });

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 6,
        flexWrap: 'wrap',
        padding: '8px 10px',
        background: `linear-gradient(180deg, ${COLORS.bgSurface} 0%, ${COLORS.bgBase} 100%)`,
        border: `1.5px solid ${COLORS.border}`,
        borderRadius: '3px 14px 3px 14px',
        boxShadow: '0 4px 14px rgba(0,0,0,0.08)',
      }}
    >
      {/* 文本颜色 */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
        {COLOR_OPTIONS.map((c) => (
          <button
            key={c.value || 'default'}
            title={c.label}
            onClick={() => onPatch({ color: c.value || undefined })}
            style={{
              width: 20,
              height: 20,
              borderRadius: '2px 8px 2px 8px',
              border: `2px solid ${style.color === c.value && c.value ? COLORS.accentBright : COLORS.border}`,
              background: c.value || 'linear-gradient(135deg,#fff,#eee)',
              cursor: 'pointer',
              padding: 0,
              position: 'relative',
            }}
          >
            {!c.value && (
              <span
                style={{
                  position: 'absolute',
                  left: 2,
                  right: 2,
                  top: '50%',
                  height: 2,
                  background: COLORS.textTertiary,
                  transform: 'rotate(-45deg)',
                }}
              />
            )}
          </button>
        ))}
      </div>

      <div style={{ width: 1, height: 22, background: COLORS.border }} />

      {/* 字号 */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 3 }}>
        <Type size={13} color={COLORS.textSecondary} />
        <select
          value={style.font_size ?? ''}
          onChange={(e) =>
            onPatch({ font_size: e.target.value ? Number(e.target.value) : undefined })
          }
          style={{
            ...TOOL_BTN,
            appearance: 'auto',
            padding: '4px 6px',
            cursor: 'pointer',
          }}
        >
          <option value="">默认</option>
          {FONT_SIZES.map((s) => (
            <option key={s} value={s}>
              {s}px
            </option>
          ))}
        </select>
      </div>

      {/* 加粗 / 斜体 */}
      <button style={activeBtn(!!style.bold)} title="加粗" onClick={() => onPatch({ bold: !style.bold })}>
        <Bold size={13} />
      </button>
      <button style={activeBtn(!!style.italic)} title="斜体" onClick={() => onPatch({ italic: !style.italic })}>
        <Italic size={13} />
      </button>

      {/* 对齐 */}
      <button style={activeBtn(style.align === 'left')} title="左对齐" onClick={() => onPatch({ align: 'left' })}>
        <AlignLeft size={13} />
      </button>
      <button style={activeBtn(style.align === 'center')} title="居中" onClick={() => onPatch({ align: 'center' })}>
        <AlignCenter size={13} />
      </button>
      <button style={activeBtn(style.align === 'right')} title="右对齐" onClick={() => onPatch({ align: 'right' })}>
        <AlignRight size={13} />
      </button>
    </div>
  );
};

// ============================================================
// 单块渲染（预览镜像 + 选中高亮）
// ============================================================

const EditableBlock: React.FC<{
  block: Block;
  selected: boolean;
  onSelect: () => void;
  onPatch: (patch: Partial<Block>) => void;
  onRemove: () => void;
  onMove: (dir: -1 | 1) => void;
}> = ({ block, selected, onSelect, onPatch, onRemove, onMove }) => {
  const sty = isStyledBlock(block) ? blockStyleToCss(block.style) : {};

  const wrapStyle: React.CSSProperties = {
    position: 'relative',
    borderRadius: '3px 12px 3px 12px',
    padding: 4,
    transition: `box-shadow ${DURATION.fast}s ${EASE.swift}, background ${DURATION.fast}s ${EASE.swift}`,
    cursor: 'pointer',
    background: selected ? 'rgba(255,255,255,0.5)' : 'transparent',
    boxShadow: selected ? `0 0 0 2px ${COLORS.accent}, 0 6px 20px rgba(0,0,0,0.10)` : 'none',
  };

  const iconBtn = (danger?: boolean): React.CSSProperties => ({
    display: 'inline-flex',
    alignItems: 'center',
    justifyContent: 'center',
    width: 26,
    height: 26,
    borderRadius: '2px 9px 2px 9px',
    border: 'none',
    background: danger ? '#e74c3c' : COLORS.bgSurface,
    color: danger ? '#fff' : COLORS.textSecondary,
    cursor: 'pointer',
    boxShadow: '0 1px 4px rgba(0,0,0,0.12)',
  });

  const commitText = (key: string) => (v: string) => onPatch({ [key]: v } as Partial<Block>);

  const renderInner = (): React.ReactNode => {
    switch (block.type) {
      case 'heading':
        return (
          <EditableText
            value={block.text}
            onCommit={commitText('text')}
            style={{
              ...sty,
              fontSize: block.level === 1 ? 28 : block.level === 3 ? 18 : 22,
              fontWeight: 700,
              color: COLORS.textPrimary,
              fontFamily: TYPO.fontFamily,
              lineHeight: 1.4,
            }}
          />
        );
      case 'paragraph':
        return (
          <EditableText
            value={block.text}
            onCommit={commitText('text')}
            style={{
              ...sty,
              fontSize: 15,
              color: COLORS.textPrimary,
              fontFamily: TYPO.fontFamily,
              lineHeight: 1.8,
              whiteSpace: 'pre-wrap',
            }}
          />
        );
      case 'card':
        return (
          <div
            style={{
              background: COLORS.bgSurface,
              border: `1.5px solid ${COLORS.border}`,
              borderRadius: '3px 14px 3px 14px',
              padding: SPACING.md,
              boxShadow: '0 2px 8px rgba(0,0,0,0.05)',
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 6 }}>
              {block.emoji && <span>{block.emoji}</span>}
              {block.title !== undefined && (
                <EditableText
                  value={block.title || ''}
                  onCommit={(v) => onPatch({ title: v } as Partial<Block>)}
                  style={{ fontSize: 16, fontWeight: 700, color: COLORS.accentBright, fontFamily: TYPO.fontFamily }}
                />
              )}
            </div>
            <EditableText
              value={block.body}
              onCommit={commitText('body')}
              style={{
                ...sty,
                fontSize: 14,
                color: COLORS.textPrimary,
                fontFamily: TYPO.fontFamily,
                lineHeight: 1.7,
                whiteSpace: 'pre-wrap',
              }}
            />
          </div>
        );
      case 'quote':
        return (
          <div
            style={{
              borderLeft: `4px solid ${COLORS.accent}`,
              background: COLORS.accentMuted,
              borderRadius: '3px 12px 3px 12px',
              padding: '12px 16px',
            }}
          >
            <EditableText
              value={block.text}
              onCommit={commitText('text')}
              style={{
                ...sty,
                fontSize: 15,
                color: COLORS.textPrimary,
                fontFamily: TYPO.fontFamily,
                fontStyle: 'italic',
                lineHeight: 1.7,
              }}
            />
            {block.author && (
              <div style={{ marginTop: 6, fontSize: 12, color: COLORS.textSecondary, textAlign: 'right' }}>
                — {block.author}
              </div>
            )}
          </div>
        );
      case 'list': {
        const items = block.items.length ? block.items : [''];
        return (
          <div style={{ paddingLeft: 4 }}>
            {items.map((item, i) => (
              <div key={i} style={{ display: 'flex', gap: 8, alignItems: 'flex-start' }}>
                <span style={{ color: COLORS.accent, fontSize: 14, lineHeight: '1.8', flexShrink: 0 }}>
                  {block.ordered ? `${i + 1}.` : '•'}
                </span>
                <EditableText
                  value={item}
                  onCommit={(v) => {
                    const next = [...items];
                    next[i] = v;
                    onPatch({ items: next } as Partial<Block>);
                  }}
                  style={{
                    ...sty,
                    flex: 1,
                    fontSize: 14,
                    color: COLORS.textPrimary,
                    fontFamily: TYPO.fontFamily,
                    lineHeight: 1.8,
                  }}
                />
              </div>
            ))}
          </div>
        );
      }
      case 'tags':
        return (
          <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
            {block.items.map((item, i) => (
              <span
                key={i}
                style={{
                  background: COLORS.accentMuted,
                  color: COLORS.accentBright,
                  padding: '3px 10px',
                  borderRadius: '2px 12px 2px 12px',
                  fontSize: 12,
                  fontFamily: TYPO.fontFamily,
                }}
              >
                {item}
              </span>
            ))}
          </div>
        );
      case 'image':
        return (
          <div>
            <img
              src={block.url}
              alt={block.caption || ''}
              style={{ maxWidth: '100%', borderRadius: '3px 12px 3px 12px', display: 'block' }}
            />
            {block.caption && (
              <div style={{ marginTop: 4, fontSize: 12, color: COLORS.textSecondary, textAlign: 'center' }}>
                {block.caption}
              </div>
            )}
          </div>
        );
      case 'divider':
        return (
          <div style={{ display: 'flex', alignItems: 'center', gap: 10, color: COLORS.textTertiary }}>
            <div style={{ flex: 1, height: 1, background: COLORS.border }} />
            <span>{block.emoji || '✿'}</span>
            <div style={{ flex: 1, height: 1, background: COLORS.border }} />
          </div>
        );
      case 'callout':
        return (
          <div
            style={{
              background: COLORS.accentMuted,
              border: `1.5px solid ${COLORS.borderAccent}`,
              borderRadius: '3px 14px 3px 14px',
              padding: '12px 16px',
              display: 'flex',
              gap: 10,
              alignItems: 'flex-start',
            }}
          >
            <span style={{ fontSize: 18 }}>{block.emoji || '💡'}</span>
            <EditableText
              value={block.text}
              onCommit={commitText('text')}
              style={{
                ...sty,
                flex: 1,
                fontSize: 14,
                color: COLORS.textPrimary,
                fontFamily: TYPO.fontFamily,
                lineHeight: 1.7,
                whiteSpace: 'pre-wrap',
              }}
            />
          </div>
        );
      case 'table': {
        const headers = block.headers;
        const rows = block.rows;
        return (
          <div style={{ overflowX: 'auto' }}>
            <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 13 }}>
              <thead>
                <tr>
                  {headers.map((h, i) => (
                    <th
                      key={i}
                      style={{
                        background: COLORS.accentMuted,
                        color: COLORS.textPrimary,
                        padding: '8px 10px',
                        border: `1px solid ${COLORS.border}`,
                        fontFamily: TYPO.fontFamily,
                      }}
                    >
                      {h}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {rows.map((row, ri) => (
                  <tr key={ri}>
                    {row.map((c, ci) => (
                      <td
                        key={ci}
                        style={{
                          padding: '8px 10px',
                          border: `1px solid ${COLORS.border}`,
                          color: COLORS.textPrimary,
                          fontFamily: TYPO.fontFamily,
                        }}
                      >
                        {c}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        );
      }
      case 'chart':
        return (
          <div
            style={{
              height: 120,
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'center',
              color: COLORS.textSecondary,
              background: COLORS.subtleBg,
              borderRadius: '3px 12px 3px 12px',
              fontSize: 13,
              fontFamily: TYPO.fontFamily,
            }}
          >
            📊 图表（{block.chart_type}）：{block.title || '未命名'}
          </div>
        );
      case 'mermaid':
        return (
          <div
            style={{
              padding: '12px 16px',
              background: COLORS.subtleBg,
              borderRadius: '3px 12px 3px 12px',
              color: COLORS.textSecondary,
              fontSize: 13,
              fontFamily: TYPO.fontFamily,
              whiteSpace: 'pre-wrap',
            }}
          >
            🔰 Mermaid 图
            {block.caption && <div style={{ marginTop: 4 }}>{block.caption}</div>}
          </div>
        );
      case 'custom':
        return (
          <div style={{ color: COLORS.textSecondary, fontSize: 13, fontFamily: TYPO.fontFamily }}>
            自定义 HTML 片段（请在表单模式编辑）
          </div>
        );
      default:
        return null;
    }
  };

  return (
    <div style={wrapStyle} onClick={onSelect}>
      {renderInner()}
      {selected && (
        <div
          style={{
            position: 'absolute',
            top: -14,
            right: 8,
            display: 'flex',
            gap: 4,
            zIndex: 5,
          }}
          onClick={(e) => e.stopPropagation()}
        >
          <button style={iconBtn()} title="上移" onClick={() => onMove(-1)}>
            <ChevronUp size={13} />
          </button>
          <button style={iconBtn()} title="下移" onClick={() => onMove(1)}>
            <ChevronDown size={13} />
          </button>
          <button style={iconBtn(true)} title="删除" onClick={onRemove}>
            <Trash2 size={13} />
          </button>
        </div>
      )}
    </div>
  );
};

// ============================================================
// WysiwygEditor 主组件
// ============================================================

export const WysiwygEditor: React.FC<{
  blocks: Block[];
  onUpdateBlock: (idx: number, patch: Partial<Block>) => void;
  onRemoveBlock: (idx: number) => void;
  onMoveBlock: (idx: number, dir: -1 | 1) => void;
  onAddBlock: (type: Block['type']) => void;
  t: (key: string, opts?: Record<string, unknown>) => string;
}> = ({ blocks, onUpdateBlock, onRemoveBlock, onMoveBlock, onAddBlock, t }) => {
  const [selectedIdx, setSelectedIdx] = useState<number | null>(null);
  const selected = selectedIdx !== null ? blocks[selectedIdx] : null;

  const patchStyle = useCallback(
    (patch: Partial<BlockStyle>) => {
      if (selectedIdx === null || !selected || !isStyledBlock(selected)) return;
      const base = selected.style || {};
      onUpdateBlock(selectedIdx, { style: { ...base, ...patch } } as Partial<Block>);
    },
    [selectedIdx, selected, onUpdateBlock],
  );

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: SPACING.sm }}>
      {/* 选中块的样式工具条（吸顶） */}
      {selected && isStyledBlock(selected) ? (
        <div style={{ position: 'sticky', top: 0, zIndex: 10 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginBottom: 6 }}>
            <span
              style={{
                fontSize: 11,
                fontWeight: 600,
                color: COLORS.accent,
                background: COLORS.accentMuted,
                padding: '2px 10px',
                borderRadius: '2px 10px 2px 10px',
                fontFamily: TYPO.fontFamily,
              }}
            >
              {selected.type}
            </span>
            <span style={{ fontSize: 12, color: COLORS.textTertiary, fontFamily: TYPO.fontFamily }}>
              {t('notebook.click_edit_hint')}
            </span>
          </div>
          <StyleToolbar style={selected.style || {}} onPatch={patchStyle} />
        </div>
      ) : (
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 6,
            padding: '6px 10px',
            background: COLORS.subtleBg,
            borderRadius: '3px 10px 3px 10px',
            color: COLORS.textTertiary,
            fontSize: 12,
            fontFamily: TYPO.fontFamily,
          }}
        >
          <ChevronLeft size={13} />
          {t('notebook.wysiwyg_hint')}
        </div>
      )}

      {/* 画布 */}
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          gap: SPACING.md,
          padding: 8,
        }}
      >
        {blocks.map((block, idx) => (
          <EditableBlock
            key={idx}
            block={block}
            selected={selectedIdx === idx}
            onSelect={() => setSelectedIdx(idx)}
            onPatch={(patch) => onUpdateBlock(idx, patch)}
            onRemove={() => {
              setSelectedIdx(null);
              onRemoveBlock(idx);
            }}
            onMove={(dir) => onMoveBlock(idx, dir)}
          />
        ))}
        {blocks.length === 0 && (
          <div style={{ padding: SPACING.md, textAlign: 'center', color: COLORS.textTertiary, fontSize: 13, fontFamily: TYPO.fontFamily }}>
            {t('notebook.add_block')}
          </div>
        )}
      </div>
    </div>
  );
};