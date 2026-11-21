/**
 * 预览页「就地改写」的浮卡与 diff 卡片。
 *
 * 交互链路（全程不产生会话消息，也不落工作区记录）：
 *   1. 在预览正文里抹黑选中一段 → 浮卡出现，两个动作：添加到对话 / 编辑
 *   2. 点「编辑」→ 浮卡就地变成一行输入框，填修改要求，回车提交
 *   3. 提交走 `rewrite_preview_selection`（一次性 LLM 调用，无会话上下文），
 *      结果以预览页浮层卡片展示 原文 / 改写 对照
 *   4. 卡片底部三个按钮：编辑（继续改，原地再来一轮）/ 拒绝（丢弃）/ 接受（写盘）
 *
 * 卡片只负责展示与转发回调，写盘与缓存刷新由宿主 PreviewPanel 做 —— 那里才拿得到
 * 文件内容与预览缓存。
 */

import React, { useState } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { ArrowUp, Loader2 } from 'lucide-react';

/** 一次待裁决的就地改写 */
export interface PendingEdit {
  /** 所属页签（文件路径） */
  path: string;
  /** 受影响源码行的 0-based 区间 [start, end)，写盘时替换这一段 */
  start: number;
  end: number;
  /**
   * 卡片插在哪儿：
   * - markdown 渲染流：块索引（用于定位选中内容）
   * - 源码视图：1-based 行号（用于定位选中内容）
   */
  anchor: number;
  /** 在规范化后的完整文件内容中的精确替换区间 [start, end)。 */
  replaceStart: number;
  replaceEnd: number;
  /** 生成 diff 时选区所在位置的视口矩形；diff 卡片不参与正文布局。 */
  anchorRect: SelectionRect;
  /** 用户选中的原文 */
  original: string;
  /** 模型改写结果 */
  rewritten: string;
  /** 上一次的编辑指令，「编辑」按钮回填用 */
  instruction: string;
  /**
   * 选中处所在整块的原文，只作为改写时的语境（代词指代、术语一致）传给模型。
   * 存在卡片上是为了让「编辑」再改一轮时不必依赖 DOM 选区 —— 那时选区早就没了。
   */
  context: string;
}

/** 一次选区的落点：改哪几行、卡片插哪儿、拿什么当语境 */
export interface SelectionTarget {
  /** 选中的原文 */
  text: string;
  /** 受影响源码行的 0-based 区间 [start, end)，写盘时替换这一段 */
  start: number;
  end: number;
  /** 卡片插入锚点：markdown 用块索引，源码视图用 1-based 行号 */
  anchor: number;
  /** 在规范化后的完整文件内容中的精确替换区间；无法映射时仅支持“添加到对话”。 */
  replaceStart: number | null;
  replaceEnd: number | null;
  /** 选区的视口矩形，用于把后续 diff 卡片放到选中行附近。 */
  anchorRect: SelectionRect;
  /** 选中处所在整块的原文，只作为改写语境传给模型 */
  context: string;
}

export interface SelectionRect {
  top: number;
  bottom: number;
  left: number;
  right: number;
}

function lineStartsOf(source: string): number[] {
  const starts = [0];
  for (let i = 0; i < source.length; i += 1) {
    if (source[i] === '\n') starts.push(i + 1);
  }
  return starts;
}

/** 在块对应的源码区间内找选中文本，避免接受时覆盖同一块里未选中的内容。 */
function findSourceSelection(source: string, text: string, regionStart: number, regionEnd: number): [number, number] | null {
  const region = source.slice(regionStart, regionEnd);
  const candidates = [text, text.trim()].filter(Boolean);
  for (const candidate of candidates) {
    const index = region.indexOf(candidate);
    if (index >= 0 && region.indexOf(candidate, index + candidate.length) < 0) {
      return [regionStart + index, regionStart + index + candidate.length];
    }
  }

  // DOM 选区可能把 CRLF / 连续空白折叠成单个换行或空格，做一次保守的空白归一化匹配。
  const compact = (value: string) => value.replace(/\s+/g, ' ').trim();
  const compactText = compact(text);
  if (!compactText) return null;
  const firstToken = text.trim().split(/\s+/)[0];
  let cursor = 0;
  while (cursor < region.length) {
    const next = region.indexOf(firstToken, cursor);
    if (next < 0) break;
    for (let end = next + firstToken.length; end <= region.length; end += 1) {
      if (compact(region.slice(next, end)) === compactText) return [regionStart + next, regionStart + end];
    }
    cursor = next + 1;
  }
  return null;
}

/**
 * 把 DOM 选区解析成「要改哪几行」。
 *
 * 两条渲染路径的锚点不同：
 * - markdown 渲染流：块带 `data-md-block`（块索引），经块序列换算成源码行区间
 * - 源码视图：行带 `data-source-line`（1-based），直接换算
 *
 * 只认这两类元素 —— 卡片自身、页签栏、工具栏都不在判定范围内，所以不会套娃。
 */
export function resolveSelectionTarget(
  range: Range,
  mdBlocks: { span: { start: number; end: number } }[],
  source = '',
): SelectionTarget | null {
  const text = range.toString();
  if (!text.trim()) return null;

  const elOf = (n: Node | null): HTMLElement | null =>
    n instanceof HTMLElement ? n : (n?.parentElement ?? null);
  const startEl = elOf(range.startContainer)?.closest<HTMLElement>('[data-md-block], [data-source-line]');
  const endEl = elOf(range.endContainer)?.closest<HTMLElement>('[data-md-block], [data-source-line]');
  if (!startEl || !endEl) return null;

  const context = (startEl.textContent ?? '').trim().slice(0, 600);
  const rect = range.getBoundingClientRect();
  const anchorRect: SelectionRect = {
    top: rect.top,
    bottom: rect.bottom,
    left: rect.left,
    right: rect.right,
  };

  if (startEl.dataset.mdBlock !== undefined && endEl.dataset.mdBlock !== undefined) {
    const lo = Math.min(Number(startEl.dataset.mdBlock), Number(endEl.dataset.mdBlock));
    const hi = Math.max(Number(startEl.dataset.mdBlock), Number(endEl.dataset.mdBlock));
    const bs = mdBlocks[lo];
    const be = mdBlocks[hi];
    if (!bs || !be) return null;
    const normalizedSource = source.replace(/\r\n?/g, '\n');
    const starts = lineStartsOf(normalizedSource);
    const regionStart = starts[bs.span.start] ?? normalizedSource.length;
    const regionEnd = starts[be.span.end] ?? normalizedSource.length;
    const replacement = findSourceSelection(normalizedSource, text, regionStart, regionEnd);
    return {
      text,
      start: bs.span.start,
      end: be.span.end,
      anchor: hi,
      replaceStart: replacement?.[0] ?? null,
      replaceEnd: replacement?.[1] ?? null,
      anchorRect,
      context,
    };
  }

  if (startEl.dataset.sourceLine && endEl.dataset.sourceLine) {
    const lo = Math.min(Number(startEl.dataset.sourceLine), Number(endEl.dataset.sourceLine));
    const hi = Math.max(Number(startEl.dataset.sourceLine), Number(endEl.dataset.sourceLine));
    const normalizedSource = source.replace(/\r\n?/g, '\n');
    const starts = lineStartsOf(normalizedSource);
    const regionStart = starts[lo - 1] ?? normalizedSource.length;
    const regionEnd = starts[hi] ?? normalizedSource.length;
    const replacement = findSourceSelection(normalizedSource, text, regionStart, regionEnd);
    return {
      text,
      start: lo - 1,
      end: hi,
      anchor: hi,
      replaceStart: replacement?.[0] ?? null,
      replaceEnd: replacement?.[1] ?? null,
      anchorRect,
      context,
    };
  }
  return null;
}

// ============ 选区浮卡 ============

export const SelectionBubble: React.FC<{
  x: number;
  y: number;
  /** menu：两个动作；edit：输入修改要求 */
  mode: 'menu' | 'edit';
  instruction: string;
  busy: boolean;
  error: string | null;
  /**
   * 当前文件能否就地改写。超大文件只加载了首段，此时写盘会把未加载的部分截断，
   * 所以「编辑」要禁用（「添加到对话」不受影响）。
   */
  canEdit: boolean;
  editDisabledReason?: string;
  onInstructionChange: (v: string) => void;
  onAddToChat: () => void;
  onStartEdit: () => void;
  onSubmit: () => void;
  onCancel: () => void;
}> = ({ x, y, mode, instruction, busy, error, canEdit, editDisabledReason, onInstructionChange, onAddToChat, onStartEdit, onSubmit, onCancel }) => {
  const { t } = useTranslation();
  return createPortal(
    <div
      /* portal 到 body，带上 codex-theme 才拿得到主题变量 */
      className="codex-theme codex-selbubble"
      style={{ left: x, top: y }}
      /* 关键：点按钮时不抢走正文选区，否则 selectionchange 会先把浮卡收掉 */
      onMouseDown={(e) => e.preventDefault()}
    >
      <div className="codex-selbubble-pill">
        {mode === 'menu' ? (
          <>
            <button type="button" className="codex-selbubble-btn" onClick={onAddToChat}>
              {t('mind_inspector.code_sel_add_to_chat')}
            </button>
            <span className="codex-selbubble-sep" />
            <button
              type="button"
              className="codex-selbubble-btn"
              disabled={!canEdit}
              title={canEdit ? undefined : editDisabledReason}
              onClick={onStartEdit}
            >
              {t('mind_inspector.code_sel_edit')}
            </button>
          </>
        ) : (
          <span className="codex-selbubble-inputwrap">
            <input
              autoFocus
              value={instruction}
              placeholder={t('mind_inspector.code_sel_edit_placeholder')}
              onChange={(e) => onInstructionChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') { e.preventDefault(); onSubmit(); }
                if (e.key === 'Escape') { e.preventDefault(); onCancel(); }
              }}
            />
            <button
              type="button"
              className="codex-selbubble-send"
              disabled={busy || instruction.trim() === ''}
              title={t('mind_inspector.code_sel_edit_submit')}
              onClick={onSubmit}
            >
              {busy ? <Loader2 size={13} className="codex-spin" /> : <ArrowUp size={13} />}
            </button>
          </span>
        )}
      </div>
      {error && <div className="codex-selbubble-error">{error}</div>}
    </div>,
    document.body,
  );
};

// ============ 就地改写的 diff 卡片 ============

export const PreviewEditCard: React.FC<{
  edit: PendingEdit;
  busy: boolean;
  error: string | null;
  /** 「编辑」提交新一轮改写指令 */
  onRefine: (instruction: string) => void;
  onReject: () => void;
  onAccept: () => void;
}> = ({ edit, busy, error, onRefine, onReject, onAccept }) => {
  const { t } = useTranslation();
  const [refining, setRefining] = useState(false);
  const [draft, setDraft] = useState('');
  const cardRef = React.useRef<HTMLDivElement | null>(null);
  const [placement, setPlacement] = useState<{ left: number; top: number; above: boolean }>({
    left: (edit.anchorRect.left + edit.anchorRect.right) / 2,
    top: edit.anchorRect.bottom + 8,
    above: false,
  });

  React.useLayoutEffect(() => {
    const update = () => {
      const card = cardRef.current;
      const width = card?.offsetWidth ?? 560;
      const height = card?.offsetHeight ?? 260;
      const margin = 12;
      const center = (edit.anchorRect.left + edit.anchorRect.right) / 2;
      const left = Math.min(Math.max(center, width / 2 + margin), window.innerWidth - width / 2 - margin);
      const above = edit.anchorRect.top >= height + margin;
      setPlacement({
        left,
        top: above ? edit.anchorRect.top - 8 : Math.min(edit.anchorRect.bottom + 8, window.innerHeight - margin),
        above,
      });
    };
    update();
    window.addEventListener('resize', update);
    window.addEventListener('scroll', update, true);
    return () => {
      window.removeEventListener('resize', update);
      window.removeEventListener('scroll', update, true);
    };
  }, [edit.anchorRect, edit.rewritten, refining, error]);

  const startRefine = () => {
    setDraft(edit.instruction);
    setRefining(true);
  };

  const submitRefine = () => {
    const v = draft.trim();
    if (!v || busy) return;
    onRefine(v);
    setRefining(false);
  };

  return createPortal(
    <div
      className={`codex-theme codex-editcard-overlay${placement.above ? ' is-above' : ''}`}
      style={{ left: placement.left, top: placement.top }}
      data-edit-card="1"
    >
      {/* user-select: none —— 卡片本身不参与「抹黑选中」的判定，避免套娃 */}
      <div ref={cardRef} className="codex-editcard">
      <div className="codex-editcard-before">{edit.original}</div>
      <div className="codex-editcard-after">{edit.rewritten}</div>

      {refining && (
        <div className="codex-editcard-refine">
          <input
            autoFocus
            value={draft}
            placeholder={t('mind_inspector.code_sel_edit_placeholder')}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') { e.preventDefault(); submitRefine(); }
              if (e.key === 'Escape') { e.preventDefault(); setRefining(false); }
            }}
          />
          <button
            type="button"
            className="codex-selbubble-send"
            disabled={busy || draft.trim() === ''}
            title={t('mind_inspector.code_sel_edit_submit')}
            onClick={submitRefine}
          >
            {busy ? <Loader2 size={13} className="codex-spin" /> : <ArrowUp size={13} />}
          </button>
        </div>
      )}

      {error && <div className="codex-editcard-error">{error}</div>}

      <div className="codex-editcard-actions">
        <button type="button" className="codex-editcard-btn" onClick={startRefine} disabled={busy}>
          {t('mind_inspector.code_sel_edit')}
        </button>
        <button type="button" className="codex-editcard-btn codex-editcard-deny" onClick={onReject} disabled={busy}>
          {t('mind_inspector.code_sel_reject')}
        </button>
        <button type="button" className="codex-editcard-btn codex-editcard-accept" onClick={onAccept} disabled={busy}>
          {busy ? <Loader2 size={13} className="codex-spin" /> : null}
          {t('mind_inspector.code_sel_accept')}
        </button>
      </div>
      </div>
    </div>,
    document.body,
  );
};

export default PreviewEditCard;
