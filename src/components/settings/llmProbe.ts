export interface ProbeTarget {
  reasoning?: {mode:string;effort?:string|null;budget_tokens?:number|null}|null; reasoningOverrides?:Record<string,unknown>|null;
  sendTemperature?: boolean; sendMaxTokens?: boolean;
  key: string; providerType: string; model: string; endpoint: string;
  apiKey: string; apiSecret: string; appId: string;
}
export interface ProbeResult {
  state: 'testing' | 'ok' | 'error' | 'skipped';
  error?: string; elapsedMs?: number; reply?: string;
  errorKind?: 'region' | 'quota'; retrySeconds?: number;
}
// Credentials are used only in memory for identity, never logged or persisted.
const cooldowns = new Map<string, number>();
export function probeIdentity(t: ProbeTarget): string {
  return JSON.stringify([t.providerType, t.endpoint.replace(/\/+$/, ''),
    t.model, t.apiKey, t.apiSecret, t.appId, t.sendTemperature ?? true, t.sendMaxTokens ?? true, t.reasoning ?? null, t.reasoningOverrides ?? null]);
}
export function classifyProbeError(error: string): Partial<ProbeResult> {
  if (/user location is not supported/i.test(error)) return { errorKind: 'region' };
  if (/429|RESOURCE_EXHAUSTED/.test(error)) {
    const delay = error.match(/"retryDelay"\s*:\s*"([\d.]+)s"/)
      ?? error.match(/retry in\s+([\d.]+)s/i);
    return { errorKind: 'quota', retrySeconds: Math.max(1, Math.ceil(Number(delay?.[1] ?? 60))) };
  }
  return {};
}
export async function runProbeBatch(
  targets: ProbeTarget[], probe: (target: ProbeTarget) => Promise<ProbeResult>,
  publish: (keys: string[], result: ProbeResult) => void, now = Date.now,
): Promise<void> {
  const groups = new Map<string, ProbeTarget[]>();
  for (const target of targets) {
    if (!target.model || !target.endpoint) { publish([target.key], { state: 'skipped' }); continue; }
    const id = probeIdentity(target);
    groups.set(id, [...(groups.get(id) ?? []), target]);
  }
  // Sequential probes avoid a burst against shared provider quotas.
  for (const [id, group] of groups) {
    const keys = group.map(t => t.key);
    const remaining = Math.ceil(((cooldowns.get(id) ?? 0) - now()) / 1000);
    if (remaining > 0) {
      publish(keys, { state: 'error', errorKind: 'quota', retrySeconds: remaining }); continue;
    }
    cooldowns.delete(id);
    let result: ProbeResult;
    try { result = await probe(group[0]); }
    catch (error) { result = { state: 'error', error: String(error) }; }
    if (result.state === 'error') {
      result = { ...result, ...classifyProbeError(result.error ?? '') };
      if (result.errorKind === 'quota') cooldowns.set(id, now() + (result.retrySeconds ?? 60) * 1000);
    }
    publish(keys, result);
  }
}
