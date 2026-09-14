/**
 * 图谱共享类型 — 节点 / 边 / 布局 / 时间刻度
 */

export type CharacterId = 'vivian' | 'nana';

export type NodeType =
  | 'user'
  | 'agent'
  | 'belief'
  | 'episode'
  | 'dialogue'
  | 'wechat'
  | 'topic_summary'
  | 'important_event'
  | 'goal'
  | 'relationship'
  | 'inner_thought'
  | 'diary'
  | 'reading'
  | 'session_summary';

export interface GraphNode {
  id: string;
  type: NodeType;
  label: string;
  color: string;
  importance: number;
  preview: string;
  timestamp: number;
  side: 'left' | 'right';
  /** 对话节点的真实说话者（'user' 或角色 ID），含旧数据前缀自愈结果 */
  speaker?: string;
  /** 后端会话 ID（metadata.session_id 或 episode_id），用于图谱分组 */
  sessionId?: string;
  memoryId?: string;
  metadata?: Record<string, unknown>;
  /** 旁观对话标记：true 表示该对话节点是旁观者记录的他人对话，前端用半透明渲染 */
  bystander?: boolean;
  /** session_summary 节点：被摘要的子节点 ID 列表（点击可展开/收起） */
  childIds?: string[];
  /** session_summary 节点的展开状态（前端 state 管理，不持久化） */
  expanded?: boolean;
  /** 标记为已摘要的原始对话节点，渲染时折叠到对应的 session_summary 父节点下 */
  summarized?: boolean;
  /** 父 session_summary 节点 ID（summarized 节点用） */
  parentSummaryId?: string;
}

export interface GraphEdge {
  source: string;
  target: string;
  kind: 'timeline' | 'relation' | 'summary_child';
}

export interface LayoutNode {
  id: string;
  x: number;
  y: number;
  fixed: boolean;
  offsetX: number;
  offsetY: number;
  vx: number;
  vy: number;
}

export interface TimeTick {
  y: number;
  label: string;
  timestamp: number;
}
