export interface McpConfig {
  id: string;
  name: string;
  transport: 'stdio' | 'http';
  command: string;
  args: string[];
  env: Record<string, string>;
  cwd: string | null;
  url?: string | null;
  headers?: Record<string, string>;
  enabled: boolean;
  source_plugin?: string | null;
}

/** JSON arrays are lossless; quoted command arguments remain supported for older users. */
export function parseMcpArguments(input: string): string[] {
  const text = input.trim();
  if (text.startsWith('[')) {
    const value: unknown = JSON.parse(text);
    if (!Array.isArray(value) || value.some(v => typeof v !== 'string')) throw new Error('Arguments must be a JSON array of strings');
    return value;
  }
  const args: string[] = [];
  let current = '', quote = '', started = false;
  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (quote) {
      if (char === quote) quote = '';
      else if (quote === '"' && char === '\\' && text[i + 1] === '"') { current += '"'; i++; }
      else current += char;
    } else if (char === '"' || char === "'") { quote = char; started = true; }
    else if (/\s/.test(char)) { if (started) { args.push(current); current = ''; started = false; } }
    else { current += char; started = true; }
  }
  if (quote) throw new Error('Unclosed quote in arguments; use a JSON array for complex paths');
  if (started) args.push(current);
  return args;
}

export function parseMcpMap(input: string): Record<string, string> {
  if (!input.trim()) return {};
  if (input.trim().startsWith('{')) {
    const value: unknown = JSON.parse(input);
    if (!value || typeof value !== 'object' || Array.isArray(value) || Object.values(value).some(v => typeof v !== 'string')) throw new Error('Expected an object with string values');
    return value as Record<string, string>;
  }
  const pairs = input.split(/\r?\n/).filter(line => line.trim() && !line.trimStart().startsWith('#')).map(line => {
    const separator = line.indexOf('=');
    if (separator <= 0) throw new Error('Use one KEY=VALUE per line, or a JSON object');
    return [line.slice(0, separator).trim(), line.slice(separator + 1)];
  });
  return Object.fromEntries(pairs);
}
