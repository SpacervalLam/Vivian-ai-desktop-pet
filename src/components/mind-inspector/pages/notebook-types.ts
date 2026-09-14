/**
 * 笔记本共享类型（NotebookPage 与 NoteWysiwyg 可视化编辑器共用）
 */

import type { CSSProperties } from 'react';

export interface Cover {
  title: string;
  subtitle?: string;
  emoji?: string;
  background?: string;
}

/** 块的文本行内样式（可视化编辑模式） */
export interface BlockStyle {
  color?: string;
  font_size?: number;
  bold?: boolean;
  italic?: boolean;
  align?: 'left' | 'center' | 'right';
}

export type Block =
  | { type: 'heading'; text: string; level: number; style?: BlockStyle }
  | { type: 'paragraph'; text: string; style?: BlockStyle }
  | { type: 'card'; title?: string; body: string; emoji?: string; style?: BlockStyle }
  | { type: 'quote'; text: string; author?: string; style?: BlockStyle }
  | { type: 'list'; items: string[]; ordered?: boolean; style?: BlockStyle }
  | { type: 'tags'; items: string[] }
  | { type: 'image'; url: string; caption?: string }
  | { type: 'divider'; emoji?: string }
  | { type: 'callout'; text: string; emoji?: string; style?: BlockStyle }
  | { type: 'table'; headers: string[]; rows: string[][]; caption?: string }
  | {
      type: 'chart';
      chart_type: string;
      title?: string;
      categories: string[];
      series: { name: string; data: number[] }[];
    }
  | { type: 'mermaid'; code: string; caption?: string }
  | { type: 'custom'; html: string };

export interface NoteBook {
  id: string;
  title: string;
  char_id: string;
  created_at: number;
  updated_at: number;
  tags: string[];
  layout: string;
  palette: string;
  cover: Cover | null;
  blocks: Block[];
}

export type CharacterId = 'vivian' | 'nana';

export type BlockType = Block['type'];

/** 文本类块（可在可视化编辑器中直接行内编辑文本） */
export type TextBlockType =
  | 'heading'
  | 'paragraph'
  | 'card'
  | 'quote'
  | 'list'
  | 'callout';

export function isTextBlockType(t: BlockType): t is TextBlockType {
  return t === 'heading' || t === 'paragraph' || t === 'card' || t === 'quote' || t === 'list' || t === 'callout';
}

/** 携带行内样式的文本类块（heading/paragraph/card/quote/list/callout） */
export type StyledBlock = Extract<Block, { type: TextBlockType }>;

/** 判断 block 是否为可样式化的文本块，并收窄其类型 */
export function isStyledBlock(block: Block): block is StyledBlock {
  return isTextBlockType(block.type);
}

/** 将块样式转换为 React.CSSProperties（供可视化渲染层应用） */
export function blockStyleToCss(style?: BlockStyle): CSSProperties {
  if (!style) return {};
  const css: CSSProperties = {};
  if (style.color) css.color = style.color;
  if (style.font_size) css.fontSize = style.font_size;
  if (style.bold) css.fontWeight = 700;
  if (style.italic) css.fontStyle = 'italic';
  if (style.align) css.textAlign = style.align;
  return css;
}