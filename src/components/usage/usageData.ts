import taskCatalog from '../../../src-tauri/prompts/routing/task_catalog.json';

export interface Usage {
  input: number;
  output: number;
  hit: number;
  cache_creation: number;
  requests: number;
}
export interface ModelUsage extends Usage { model: string }
export interface TaskUsage extends Usage { task: string }
export interface RouteUsage extends Usage { route: string; models: ModelUsage[] }
export interface UsageDay extends Usage {
  date: string;
  models?: ModelUsage[];
  tasks?: TaskUsage[];
  routes?: RouteUsage[];
}
export interface UsageReport {
  days: UsageDay[];
  models: ModelUsage[];
  tasks: TaskUsage[];
  routes?: RouteUsage[];
  read_error?: string | null;
}
export type UsageMetric = 'tokens' | 'requests';
export type UsageView = 'trend' | 'models' | 'routes' | 'tasks';
export type UsageSelection = { dimension: 'models' | 'routes' | 'tasks' | 'route-model'; key: string; model?: string };
export interface UsageRow extends Usage { key: string }
export const LEGACY_KEY = '__legacy__';
export const usageFields = ['input', 'output', 'hit', 'cache_creation', 'requests'] as const;
export const tokenFields = ['input', 'output', 'hit', 'cache_creation'] as const;
export const emptyUsage = (): Usage => ({ input: 0, output: 0, hit: 0, cache_creation: 0, requests: 0 });
export function sumUsage(rows: Partial<Usage>[]): Usage {
  const total = emptyUsage();
  for (const row of rows) for (const field of usageFields) total[field] += row[field] ?? 0;
  return total;
}
export function tokenTotal(row: Partial<Usage>): number {
  return tokenFields.reduce((sum, field) => sum + (row[field] ?? 0), 0);
}
export const metricValue = (row: Partial<Usage>, metric: UsageMetric) => metric === 'tokens' ? tokenTotal(row) : row.requests ?? 0;

/** Residual records are visible, but never assigned to a model/route we did not observe. */
export function remainder(total: Partial<Usage>, recorded: Partial<Usage>[]): Usage {
  const sum = sumUsage(recorded);
  const result = emptyUsage();
  for (const field of usageFields) result[field] = Math.max(0, (total[field] ?? 0) - sum[field]);
  return result;
}
export function dimensionRows(report: UsageReport, dimension: 'models' | 'routes' | 'tasks', metric: UsageMetric): UsageRow[] {
  const raw = dimension === 'models' ? report.models : dimension === 'tasks' ? report.tasks : report.routes ?? [];
  const rows: UsageRow[] = raw.map((row) => ({ ...sumUsage([row]), key: 'model' in row ? row.model : 'task' in row ? row.task : row.route }));
  const residual = remainder(sumUsage(report.days), raw);
  if (tokenTotal(residual) || residual.requests) rows.push({ ...residual, key: LEGACY_KEY });
  return rows.sort((a, b) => metricValue(b, metric) - metricValue(a, metric) || a.key.localeCompare(b.key));
}
export function selectedUsage(day: UsageDay, selection: UsageSelection | null): Usage {
  if (!selection) return sumUsage([day]);
  if (selection.dimension === 'route-model') {
    const row = day.routes?.find((route) => route.route === selection.key)?.models.find((model) => model.model === selection.model);
    return sumUsage(row ? [row] : []);
  }
  const rows = selection.dimension === 'models' ? day.models ?? [] : selection.dimension === 'tasks' ? day.tasks ?? [] : day.routes ?? [];
  if (selection.key === LEGACY_KEY) return remainder(day, rows);
  const row = rows.find((row) => ('model' in row ? row.model : 'task' in row ? row.task : row.route) === selection.key);
  return sumUsage(row ? [row] : []);
}
export function selectedDays(report: UsageReport, selection: UsageSelection | null): UsageDay[] {
  return report.days.map((day) => ({ date: day.date, ...selectedUsage(day, selection) }));
}

/** Names match the routing matrix. Purpose tags are a separate perspective. */
export const routeLabelKeys: Record<string, string> = Object.fromEntries(
  taskCatalog.map(({ id, labelKey }) => [id, labelKey]),
);
