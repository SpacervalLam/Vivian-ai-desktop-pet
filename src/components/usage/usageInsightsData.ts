import { metricValue, sumUsage, tokenTotal, type ModelUsage, type RouteUsage, type TaskUsage, type Usage, type UsageDay, type UsageMetric, type UsageReport } from './usageData';

/** Rebuild only observed dimensions when a heatmap date narrows the report. */
export function reportForDays(source: UsageReport, days: UsageDay[]): UsageReport {
  const models = new Map<string, ModelUsage>();
  const tasks = new Map<string, TaskUsage>();
  const routes = new Map<string, RouteUsage>();
  for (const day of days) {
    for (const row of day.models ?? []) models.set(row.model, { model: row.model, ...sumUsage([models.get(row.model) ?? {}, row]) });
    for (const row of day.tasks ?? []) tasks.set(row.task, { task: row.task, ...sumUsage([tasks.get(row.task) ?? {}, row]) });
    for (const row of day.routes ?? []) {
      const previous = routes.get(row.route);
      const nested = new Map((previous?.models ?? []).map(model => [model.model, model]));
      for (const model of row.models) nested.set(model.model, { model: model.model, ...sumUsage([nested.get(model.model) ?? {}, model]) });
      routes.set(row.route, { route: row.route, ...sumUsage([previous ?? {}, row]), models: [...nested.values()] });
    }
  }
  return { ...source, days, models: [...models.values()], tasks: [...tasks.values()], routes: [...routes.values()] };
}

export function usageInsights(days: UsageDay[], metric: UsageMetric) {
  const total = sumUsage(days);
  const peak = days.reduce<UsageDay | null>((best, day) => metricValue(day, metric) > 0 && (!best || metricValue(day, metric) >= metricValue(best, metric)) ? day : best, null);
  return {
    activeDays: days.filter(day => tokenTotal(day) || day.requests).length,
    dailyAverage: days.length ? tokenTotal(total) / days.length : 0,
    perCall: total.requests ? tokenTotal(total) / total.requests : 0,
    peak,
  };
}

export function inputComposition(usage: Partial<Usage>) {
  const input = usage.input ?? 0, hit = usage.hit ?? 0, write = usage.cache_creation ?? 0;
  const total = input + hit + write;
  return { total, hitRate: total ? hit / total : null, parts: [input, hit, write] };
}

export function heatmapCells(days: UsageDay[], metric: UsageMetric) {
  const visible = days.slice(-56);
  const maximum = Math.max(1, ...visible.map(day => metricValue(day, metric)));
  const padding = visible.length ? (new Date(`${visible[0].date}T12:00:00`).getDay() + 6) % 7 : 0;
  return {
    padding,
    cells: visible.map(day => ({ day, level: metricValue(day, metric) ? Math.max(1, Math.ceil(Math.sqrt(metricValue(day, metric) / maximum) * 4)) : 0 })),
  };
}
