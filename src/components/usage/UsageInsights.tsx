import { CalendarDays, ChevronRight } from 'lucide-react';
import { formatCount, seriesColors } from './UsageCharts';
import { dimensionRows, metricValue, tokenTotal, type UsageDay, type UsageMetric, type UsageReport } from './usageData';
import { heatmapCells, inputComposition, usageInsights } from './usageInsightsData';
import type { UsageCopy } from './usageCopy';
import type { UsageInsightsCopy } from './usageInsightsCopy';

export function UsageActivity({ days, metric, date, onDate, copy, insight }: { days: UsageDay[]; metric: UsageMetric; date: string | null; onDate: (date: string | null) => void; copy: UsageCopy; insight: UsageInsightsCopy }) {
  const { padding, cells } = heatmapCells(days, metric);
  const focusDate = cells.some(cell => cell.day.date === date) ? date : cells[cells.length - 1]?.day.date;
  const active = cells.filter(({ day }) => day.requests || tokenTotal(day)).length;
  return <section className="usage-card usage-activity">
    <div className="usage-card-heading"><h3><CalendarDays size={15} />{insight.activity}</h3><span>{insight.eightWeeks} · {insight.activeDays} {active}</span></div>
    <div className="usage-heatmap-scroll"><div className="usage-heatmap-labels" aria-hidden="true">{insight.weekdays.map((label, index) => <span key={index} style={{ gridRow: index * 2 + 1 }}>{label}</span>)}</div>
      <div className="usage-heatmap" role="group" aria-label={insight.activity}>
        {Array.from({ length: padding }, (_, index) => <span key={`pad-${index}`} aria-hidden="true" />)}
        {cells.map(({ day, level }, index) => {
          const label = `${day.date}: ${day.requests || tokenTotal(day) ? `${formatCount(metricValue(day, metric), copy.locale)} ${metric === 'tokens' ? copy.tokenUnit : copy.calls}` : copy.noDay}`;
          return <button type="button" key={day.date} data-date={day.date} data-level={level} aria-label={label} title={label} aria-pressed={date === day.date}
            tabIndex={focusDate === day.date ? 0 : -1}
            onClick={() => onDate(date === day.date ? null : day.date)} onKeyDown={event => {
              const offset = ({ ArrowUp: -1, ArrowDown: 1, ArrowLeft: -7, ArrowRight: 7 } as Record<string, number>)[event.key];
              if (offset === undefined && event.key !== 'Home' && event.key !== 'End') return;
              event.preventDefault();
              const next = event.key === 'Home' ? 0 : event.key === 'End' ? cells.length - 1 : Math.max(0, Math.min(cells.length - 1, index + offset));
              event.currentTarget.parentElement?.querySelectorAll<HTMLButtonElement>('button')[next]?.focus();
            }} />;
        })}
      </div>
    </div>
    <div className="usage-heatmap-footer"><span>{insight.heatHelp}</span><span>{insight.less}{[0, 1, 2, 3, 4].map(level => <i key={level} data-level={level} />)}{insight.more}</span></div>
    {date && <div className="usage-date-filter"><strong>{date}</strong><button type="button" onClick={() => onDate(null)}>{insight.clearDay}</button></div>}
  </section>;
}

export function UsageRhythm({ days, metric, copy, insight }: { days: UsageDay[]; metric: UsageMetric; copy: UsageCopy; insight: UsageInsightsCopy }) {
  const stats = usageInsights(days, metric);
  return <div className="usage-rhythm">
    <div><span>{insight.activeDays}</span><strong>{stats.activeDays}<small> / {days.length}</small></strong></div>
    <div><span>{insight.dailyAverage}</span><strong>{formatCount(Math.round(stats.dailyAverage), copy.locale)}</strong></div>
    <div><span>{insight.perCall}</span><strong>{formatCount(Math.round(stats.perCall), copy.locale)}</strong></div>
    <div><span>{insight.peak}</span><strong>{stats.peak?.date ?? '—'}</strong><small>{stats.peak ? `${formatCount(metricValue(stats.peak, metric), copy.locale)} ${metric === 'tokens' ? copy.tokenUnit : copy.calls}` : insight.noPeak}</small></div>
  </div>;
}

export function UsageCache({ report, copy, insight, onModels }: { report: UsageReport; copy: UsageCopy; insight: UsageInsightsCopy; onModels: () => void }) {
  const models = dimensionRows(report, 'models', 'tokens').filter(row => inputComposition(row).total).sort((a, b) => inputComposition(b).total - inputComposition(a).total).slice(0, 5);
  return <section className="usage-card usage-cache">
    <div className="usage-card-heading"><h3>{insight.cacheTitle}</h3><button type="button" onClick={onModels}>{insight.viewModels}<ChevronRight size={13} /></button></div>
    <p className="usage-help">{insight.cacheHelp}</p><h4>{insight.cacheModels}</h4>
    {models.map(row => {
      const composition = inputComposition(row);
      const label = row.key === '__legacy__' ? copy.legacy : row.key === 'unattributed' ? copy.unknownModel : row.key;
      return <div className="usage-cache-row" key={row.key}>
        <div><span title={label}>{label}</span><strong>{((composition.hitRate ?? 0) * 100).toFixed(1)}%</strong></div>
        <div className="usage-input-stack" role="img" aria-label={`${label}: ${copy.input} ${row.input}, ${copy.hit} ${row.hit}, ${copy.cache_creation} ${row.cache_creation}`}>
          {composition.parts.map((value, index) => <span key={index} title={`${[copy.input, copy.hit, copy.cache_creation][index]}: ${formatCount(value, copy.locale)}`} style={{ width: `${value / composition.total * 100}%`, background: seriesColors[[0, 2, 3][index]] }} />)}
        </div>
      </div>;
    })}
    {!models.length && <p className="usage-help">{insight.noCache}</p>}
    <div className="usage-cache-legend">{(['input', 'hit', 'cache_creation'] as const).map((key, index) => <span className="usage-series-name" key={key}><i style={{ background: seriesColors[[0, 2, 3][index]] }} />{copy[key]}</span>)}</div>
  </section>;
}
