/**
 * 笔记本存储类型（页面编辑器与 HTML 包装语法共用）
 *
 * 笔记不使用 emoji：Cover 与各块都没有 emoji 字段，视觉层次靠标题层级、
 * 配色与卡片结构表达。旧的 note.json 里若残留该字段，反序列化时被忽略。
 */

export interface Cover {
  title: string;
  subtitle?: string;
  background?: string;
}

/** 已有内容块的文本样式，编辑页面时保留。 */
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
  | { type: 'card'; title?: string; body: string; style?: BlockStyle }
  | { type: 'quote'; text: string; author?: string; style?: BlockStyle }
  | { type: 'list'; items: string[]; ordered?: boolean; style?: BlockStyle }
  | { type: 'tags'; items: string[] }
  | { type: 'image'; url: string; caption?: string }
  | { type: 'divider' }
  | { type: 'callout'; text: string; style?: BlockStyle }
  | { type: 'table'; headers: string[]; rows: string[][]; caption?: string }
  | {
      type: 'chart';
      chart_type: string;
      title?: string;
      categories: string[];
      series: { name: string; data: number[] }[];
    }
  | { type: 'mermaid'; code: string; caption?: string }
  | { type: 'custom'; html: string; page_source?: string; markdown?: string }
  /**
   * 不渲染的元数据：存进 note.json、能被知识库检索到，但页面上看不到。
   *
   * 用于「数据来源说明」「检索可靠性评估」这类给智能体自己看的内容——
   * 渲染出来会打断笔记的叙事感。前端编辑器不提供编辑入口（它是采集链路的产物），
   * 但类型上必须存在，否则 TS 会认为后端返回的块类型不完整。
   */
  | { type: 'meta'; key: string; text: string };

export interface NoteBook {
  id: string;
  title: string;
  char_id: string;
  created_at: number;
  updated_at: number;
  tags: string[];
  layout: string;
  palette: string;
  /**
   * 自定义 CSS（与 palette 互斥）。
   *
   * 有值时后端以它为唯一主题来源，忽略 palette；空白串等同未提供。
   * 渲染时注入在预设 CSS 之后，同优先级下后写先生效。
   */
  custom_css?: string | null;
  cover: Cover | null;
  blocks: Block[];
}


export type CharacterId = 'vivian' | 'nana';
