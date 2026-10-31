/**
 * 工作过程统计：把一轮里的工具调用归并成可扫读的类别计数。
 *
 * 只统计“实际发起的调用”，而不是 tool_use + tool_result 两条消息：
 * 同一 tool_call_id 只算一次，结果消息优先于调用意图（避免迟到的意图覆盖结果）。
 * 文件数按不重复路径统计，目录列表不算文件。
 */

type Activity = {
  role?: string;
  tool_name?: string | null;
  tool_arguments?: unknown;
  tool_success?: boolean | null;
  tool_call_id?: string | null;
};
type Translate = (key: string, options?: Record<string, unknown>) => string;

export type ActivityKind = 'edit' | 'read' | 'search' | 'run' | 'browse' | 'plan' | 'image' | 'other';

export interface ActivityFact {
  kind: ActivityKind;
  /** 调用次数 */
  count: number;
  /** 涉及的不重复文件数（仅 edit / read） */
  files: number;
}

export interface WorkActivitySummary {
  facts: ActivityFact[];
  /** 合并后的调用总数 */
  total: number;
  /** 结果失败数 */
  failed: number;
}

/** 摘要行的事实顺序：与用户关心的“编辑 → 读取 → 搜索 → 执行”一致。 */
const KIND_ORDER: ActivityKind[] = ['edit', 'read', 'search', 'run', 'browse', 'plan', 'image', 'other'];

/** 只有这些工具的参数路径才计入“文件数”；list_dir 之类的目录操作只计次数。 */
const FILE_TOOLS = new Set(['read_file', 'write_file', 'edit_file']);

/** 过程阶段：探索（搜/读/看网页）、编辑、验证（执行命令）与其他。 */
export type WorkPhase = 'explore' | 'edit' | 'verify' | 'other';

export function workPhaseOf(toolName: string): WorkPhase {
  const kind = activityKind(toolName);
  if (kind === 'edit') return 'edit';
  if (kind === 'run') return 'verify';
  if (kind === 'search' || kind === 'read' || kind === 'browse') return 'explore';
  return 'other';
}

function activityKind(name: string): ActivityKind {
  return /search|grep|glob|find/.test(name) ? 'search'
    : /read_content|web_fetch|browse|browser/.test(name) ? 'browse'
      : /read_file|list_dir|list_files/.test(name) ? 'read'
        : /write_file|edit_file|patch/.test(name) ? 'edit'
          : /command|terminal|shell/.test(name) ? 'run'
            : /todo|plan/.test(name) ? 'plan'
              : /image/.test(name) ? 'image' : 'other';
}

function argumentPath(args: unknown): string {
  let value = args;
  if (typeof value === 'string') {
    try {
      value = JSON.parse(value);
    } catch {
      return '';
    }
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) return '';
  const record = value as Record<string, unknown>;
  for (const key of ['path', 'file_path', 'file', 'target']) {
    const candidate = record[key];
    if (typeof candidate === 'string' && candidate.trim()) return candidate.trim();
  }
  return '';
}

function normalizePath(path: string): string {
  return path.replace(/\\/g, '/');
}

/** 归并一轮工具消息 → 类别计数。纯函数，便于测试。 */
export function collectWorkActivity(messages: readonly Activity[]): WorkActivitySummary {
  const calls = new Map<string, Activity>();
  let anonymous = 0;
  for (const message of messages) {
    if (!message.tool_name) continue;
    const id = message.tool_call_id || `anonymous-${anonymous++}`;
    const previous = calls.get(id);
    // 迟到的 tool_use 不得覆盖已有结果；同一 id 只保留一条。
    // 结果消息常常不带参数，合并时把调用意图里的 tool_name / tool_arguments 带过来，
    // 否则文件路径统计会丢（页面正常链路已经 reconcile 过，这里兜底未合并的输入）。
    if (!previous) {
      calls.set(id, message);
    } else if (previous.role === 'tool_use' && message.role === 'tool_result') {
      calls.set(id, {
        ...message,
        tool_name: message.tool_name ?? previous.tool_name,
        tool_arguments: message.tool_arguments ?? previous.tool_arguments,
      });
    }
  }

  const counts = new Map<ActivityKind, { count: number; files: Set<string> }>();
  let failed = 0;
  for (const message of calls.values()) {
    const name = message.tool_name ?? '';
    const kind = activityKind(name);
    const entry = counts.get(kind) ?? { count: 0, files: new Set<string>() };
    entry.count += 1;
    if (FILE_TOOLS.has(name)) {
      const path = argumentPath(message.tool_arguments);
      if (path) entry.files.add(normalizePath(path));
    }
    counts.set(kind, entry);
    if (message.tool_success === false) failed += 1;
  }

  const facts = KIND_ORDER
    .filter((kind) => counts.has(kind))
    .map((kind) => {
      const entry = counts.get(kind)!;
      return { kind, count: entry.count, files: entry.files.size };
    });
  return { facts, total: calls.size, failed };
}

const FILE_LABEL: Partial<Record<ActivityKind, string>> = {
  edit: 'workbench.activity_edit_files',
  read: 'workbench.activity_read_files',
};

const OP_LABEL: Record<ActivityKind, string> = {
  edit: 'workbench.activity_edit_ops',
  read: 'workbench.activity_read_ops',
  search: 'workbench.activity_search_ops',
  run: 'workbench.activity_run_ops',
  browse: 'workbench.activity_browse_ops',
  plan: 'workbench.activity_plan_ops',
  image: 'workbench.activity_image_ops',
  other: 'workbench.activity_other_ops',
};

/** 类别 + 数量 → 本地化文案。文件类优先报文件数，没有路径时回退到次数。 */
export function formatWorkActivity(summary: WorkActivitySummary, t: Translate): string[] {
  return summary.facts.map((fact) => {
    const fileKey = FILE_LABEL[fact.kind];
    if (fileKey && fact.files > 0) return t(fileKey, { n: fact.files });
    return t(OP_LABEL[fact.kind], { n: fact.count });
  });
}
