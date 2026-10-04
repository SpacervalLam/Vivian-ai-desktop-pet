export type RequestBodyMode = 'legacy' | 'merge' | 'replace';
export function decodeRequestBody(config: Record<string, unknown> | null | undefined): {mode: RequestBodyMode; body: unknown} {
  const request = config?.$request as {mode?: string; body?: unknown} | undefined;
  if (request && (request.mode === 'merge' || request.mode === 'replace')) return {mode: request.mode, body: request.body};
  return {mode: config ? 'legacy' : 'merge', body: config ?? {}};
}
export function encodeRequestBody(mode: RequestBodyMode, text: string): Record<string, unknown> | null {
  if (!text.trim()) {
    if (mode === 'replace') throw new Error('完全替换模式需要填写 JSON 请求体 / Replacement requires a JSON body');
    return null;
  }
  const body: unknown = JSON.parse(text);
  if (!body || typeof body !== 'object' || Array.isArray(body)) throw new Error('请求体必须是 JSON 对象 / Request body must be a JSON object');
  return mode === 'legacy' ? body as Record<string, unknown> : {$request: {mode, body}};
}
/** Dynamic references preserve the conversation, tools and schema for each request. */
export function dynamicRequestTemplate(base: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(Object.keys(base).map(key => [key, {$requestRef: `/${key.replace(/~/g, '~0').replace(/\//g, '~1')}`} ]));
}
