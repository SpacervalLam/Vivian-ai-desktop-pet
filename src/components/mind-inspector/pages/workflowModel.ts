/** Vivian projection of the DSH workflow's turn → request → response → tools layout. */
export interface WorkflowMessage {
  role: 'user' | 'assistant' | 'tool_use' | 'tool_result' | 'error' | 'notice' | 'commentary' | 'thinking';
  content: string;
  timestamp: number;
  interjected?: boolean | null;
  guided?: boolean | null;
  tool_name?: string | null;
  tool_call_id?: string | null;
  tool_arguments?: unknown;
  tool_success?: boolean | null;
  tool_duration_ms?: number | null;
}

export type WorkflowView = 'flow' | 'events';

export interface WorkflowUsage {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
}

export interface WorkflowRequest {
  id: string;
  turn_message_index: number;
  turn: number;
  step: number;
  attempt: number;
  purpose: string;
  model: string;
  started_at: number;
  completed_at: number | null;
  duration_ms: number | null;
  status: 'running' | 'complete' | 'error' | 'interrupted';
  response_preview: string;
  tool_calls: { id: string; name: string }[];
  usage: WorkflowUsage | null;
  error: string | null;
}

export interface WorkflowDetail {
  summary: WorkflowRequest;
  capture_stage: string;
  system: unknown[];
  messages: unknown;
  tools: unknown;
  reasoning: unknown;
  response: unknown;
}

export interface WorkflowTool {
  key: string;
  id: string | null;
  name: string;
  message: WorkflowMessage | null;
  status: WorkflowRequest['status'];
}

export interface WorkflowTurn {
  anchor: number;
  prompt: string;
  calls: { request: WorkflowRequest; tools: WorkflowTool[] }[];
  tools: WorkflowTool[];
  status: WorkflowRequest['status'] | 'waiting';
  durationMs: number | null;
}

/** Event/list reconciliation: an older initial listing cannot overwrite a settled live event. */
export function mergeWorkflowRequests(current: readonly WorkflowRequest[], incoming: readonly WorkflowRequest[]): WorkflowRequest[] {
  const map = new Map(current.map(request => [request.id, request]));
  for (const request of incoming) {
    const previous = map.get(request.id);
    if (!previous || previous.completed_at === null || request.completed_at !== null) map.set(request.id, request);
  }
  return [...map.values()].sort((a, b) => a.started_at - b.started_at || a.id.localeCompare(b.id));
}

/** Tool IDs, rather than names or adjacency, correlate parallel and repeated calls. */
export function deriveWorkflow(messages: readonly WorkflowMessage[], requests: readonly WorkflowRequest[], running: boolean) {
  const turns: WorkflowTurn[] = [];
  const toolMessages = new Map<string, WorkflowMessage>();
  const toolOwners = new Map<string, WorkflowTurn>();
  const toolsByTurn = new Map<number, Map<string, WorkflowTool>>();
  const answered = new Set<number>();
  let active: WorkflowTurn | undefined;
  messages.forEach((message, index) => {
    if (message.role === 'user' && (!active || (!message.interjected && !message.guided))) {
      active = { anchor: index, prompt: message.content, calls: [], tools: [], status: 'waiting', durationMs: null };
      turns.push(active);
      toolsByTurn.set(index, new Map());
    }
    if (active && message.role === 'assistant') answered.add(active.anchor);
    if (!active || (message.role !== 'tool_use' && message.role !== 'tool_result')) return;
    if (message.role === 'tool_use' && !message.tool_name) return;
    const key = `${active.anchor}:${message.tool_call_id || `message:${index}`}`;
    const previous = toolMessages.get(key);
    const name = message.tool_name ?? previous?.tool_name;
    if (!name) return;
    // Live tool_use rows become tool_result rows; persisted histories keep both forms.
    if (previous?.role === 'tool_result' && message.role === 'tool_use') return;
    toolMessages.set(key, { ...previous, ...message, tool_name: name, tool_arguments: message.tool_arguments ?? previous?.tool_arguments });
    toolOwners.set(key, active);
  });
  for (const [key, message] of toolMessages) {
    const owner = toolOwners.get(key)!;
    const tool: WorkflowTool = { key, id: message.tool_call_id ?? null, name: message.tool_name!, message,
      status: message.role === 'tool_result' ? (message.tool_success === false ? 'error' : 'complete') : (running && owner === active ? 'running' : 'interrupted') };
    owner.tools.push(tool);
    if (tool.id) toolsByTurn.get(owner.anchor)!.set(tool.id, tool);
  }
  const sorted = [...requests].sort((a, b) => a.started_at - b.started_at || a.id.localeCompare(b.id));
  for (const request of sorted) {
    // Live UI rows merge tool calls/results; persisted message indices therefore differ.
    const turn = turns[request.turn];
    if (!turn) continue;
    const tools = request.tool_calls.map((call, index): WorkflowTool => toolsByTurn.get(turn.anchor)?.get(call.id) ?? {
      key: `${request.id}:${index}`, id: call.id, name: call.name, message: null,
      status: running && turn === active ? 'running' : 'interrupted',
    });
    turn.calls.push({ request: request.status === 'running' && !running ? { ...request, status: 'interrupted' } : request, tools });
  }
  for (const turn of turns) {
    // Program mode executes a JSON program; its tools are host calls, not native model tool calls.
    const codeCall = turn.calls.find(call => call.request.purpose === 'code');
    if (codeCall) {
      const assigned = new Set(turn.calls.flatMap(call => call.tools.map(tool => tool.key)));
      codeCall.tools.push(...turn.tools.filter(tool => !assigned.has(tool.key)));
    }
    const lastCall = turn.calls[turn.calls.length - 1];
    const failures = lastCall ? lastCall.request.status === 'error' || lastCall.tools.some(tool => tool.status === 'error')
      : turn.tools.some(tool => tool.status === 'error');
    turn.status = running && turn === active ? 'running' : failures ? 'error'
      : lastCall?.request.status === 'interrupted' || turn.tools.some(tool => tool.status === 'interrupted') ? 'interrupted'
      : turn.calls.length || turn.tools.length || answered.has(turn.anchor) ? 'complete' : 'waiting';
    const completed = turn.calls.filter(call => call.request.completed_at !== null);
    const start = messages[turn.anchor]?.timestamp;
    const end = turn.tools.reduce((end, tool) => Math.max(end, tool.message?.timestamp ?? 0),
      completed.reduce((end, call) => Math.max(end, call.request.completed_at!), start ?? 0));
    turn.durationMs = start !== undefined && !running && end > start ? end - start : null;
    if (turn !== active && start !== undefined && end > start) turn.durationMs = end - start;
  }
  return {
    turns, requestCount: requests.length,
    toolCount: turns.reduce((sum, turn) => sum + new Set([...turn.tools.map(tool => tool.key), ...turn.calls.flatMap(call => call.tools.map(tool => tool.key))]).size, 0),
    llmMs: requests.reduce((sum, request) => sum + (request.duration_ms ?? 0), 0),
  };
}

export function workflowDuration(ms: number | null): string {
  if (ms === null) return '—';
  if (ms < 1000) return `${ms} ms`;
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.floor(ms / 60_000)} m ${Math.round((ms % 60_000) / 1000)} s`;
}
