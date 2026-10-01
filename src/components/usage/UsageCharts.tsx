import { useState } from 'react';
import { metricValue, sumUsage, tokenFields, tokenTotal, type Usage, type UsageDay, type UsageMetric } from './usageData';
import type { UsageCopy } from './usageCopy';

export const seriesColors = ['var(--usage-input)', 'var(--usage-output)', 'var(--usage-hit)', 'var(--usage-write)'];
export const formatCount = (value: number) => value.toLocaleString();
export const compactCount = (value: number) => value < 1000 ? String(value) : new Intl.NumberFormat('en', { notation: 'compact', maximumFractionDigits: 1 }).format(value);

export function TokenBreakdown({ usage, copy }: { usage: Usage; copy: UsageCopy }) {
  return <div className="usage-breakdown">
    {tokenFields.map((field, index) => <div key={field}>
      <span className="usage-series-name"><i style={{ background: seriesColors[index] }} />{copy[field]}</span>
      <strong>{formatCount(usage[field])}</strong>
    </div>)}
  </div>;
}

export function CompositionChart({ usage, copy }: { usage: Usage; copy: UsageCopy }) {
  const total = tokenTotal(usage);
  let offset = 0;
  return <section className="usage-card">
    <div className="usage-card-heading"><h3>{copy.composition}</h3><span>Token</span></div>
    <div className="usage-composition">
      <div className="usage-donut">
        <svg viewBox="0 0 160 160" role="img" aria-label={`${copy.composition}: ${tokenFields.map((field) => `${copy[field]} ${formatCount(usage[field])}`).join(', ')}`}>
          <circle cx="80" cy="80" r="60" fill="none" stroke="var(--panel-border)" strokeWidth="19" />
          {tokenFields.map((field, index) => {
            const percent = total ? usage[field] / total * 100 : 0;
            const start = offset;
            offset += percent;
            return percent > 0 && <circle key={field} cx="80" cy="80" r="60" pathLength="100"
              fill="none" stroke={seriesColors[index]} strokeWidth="19" strokeDasharray={`${percent} ${100 - percent}`}
              strokeDashoffset={-start} transform="rotate(-90 80 80)">
              <title>{copy[field]}: {formatCount(usage[field])} · {percent.toFixed(1)}%</title>
            </circle>;
          })}
        </svg>
        <div className="usage-donut-center"><strong title={formatCount(total)}>{compactCount(total)}</strong><span>Token</span></div>
      </div>
      <div className="usage-composition-legend">
        {tokenFields.map((field, index) => <div key={field}>
          <span className="usage-series-name"><i style={{ background: seriesColors[index] }} />{copy[field]}</span>
          <strong>{formatCount(usage[field])}</strong><small>{total ? (usage[field] / total * 100).toFixed(1) : '0'}%</small>
        </div>)}
      </div>
    </div>
  </section>;
}

export function DailyUsageChart({ days, metric, copy, title }: { days: UsageDay[]; metric: UsageMetric; copy: UsageCopy; title?: string }) {
  const [activeDate, setActiveDate] = useState<string | null>(null);
  const active = days.find((day) => day.date === activeDate) ?? [...days].reverse().find((day) => day.requests || tokenTotal(day)) ?? days[days.length - 1];
  const max = Math.max(1, ...days.map((day) => metricValue(day, metric)));
  const chartHeight = 148;
  const step = 616 / Math.max(1, days.length);
  const barWidth = Math.min(34, step * .68);
  return <section className="usage-card">
    <div className="usage-card-heading"><h3>{title ?? copy.daily}</h3><span>{metric === 'tokens' ? copy.tokens : copy.requests}</span></div>
    <svg className="usage-trend-svg" viewBox="0 0 700 220" role="group" aria-label={title ?? copy.daily}>
      {[0, .5, 1].map((ratio) => <g key={ratio}>
        <line x1="60" x2="676" y1={172 - ratio * chartHeight} y2={172 - ratio * chartHeight} stroke="var(--panel-border)" strokeDasharray="3 5" />
        <text x="49" y={176 - ratio * chartHeight} textAnchor="end" className="usage-axis">{compactCount(max * ratio)}</text>
      </g>)}
      {days.map((day, index) => {
        const x = 60 + step * index + (step - barWidth) / 2;
        const value = metricValue(day, metric);
        let top = 172;
        const accessible = `${day.date}: ${day.requests || tokenTotal(day) ? `${copy.tokens} ${formatCount(tokenTotal(day))}, ${copy.requests} ${formatCount(day.requests)}` : copy.noDay}`;
        return <g key={day.date} role="button" tabIndex={active?.date === day.date ? 0 : -1} aria-label={accessible} aria-pressed={active?.date === day.date}
          className="usage-day-bar" onMouseEnter={() => setActiveDate(day.date)} onFocus={() => setActiveDate(day.date)}
          onClick={() => setActiveDate(day.date)} onKeyDown={(event) => {
            if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); setActiveDate(day.date); }
            if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) {
              event.preventDefault();
              const next = event.key === 'Home' ? 0 : event.key === 'End' ? days.length - 1 : Math.max(0, Math.min(days.length - 1, index + (event.key === 'ArrowRight' ? 1 : -1)));
              setActiveDate(days[next].date);
              event.currentTarget.parentElement?.querySelectorAll<SVGGElement>('.usage-day-bar')[next]?.focus();
            }
          }}>
          <title>{accessible}</title>
          <rect x={60 + step * index} y="16" width={step} height="163" fill={active?.date === day.date ? 'var(--panel-accent-soft)' : 'transparent'} rx="3" />
          {metric === 'tokens' ? tokenFields.map((field, series) => {
            const height = (day[field] ?? 0) / max * chartHeight;
            top -= height;
            return <rect key={field} x={x} y={top} width={barWidth} height={height} fill={seriesColors[series]} />;
          }) : <rect x={x} y={172 - value / max * chartHeight} width={barWidth} height={value / max * chartHeight} rx="3" fill="var(--usage-input)" />}
          {!day.requests && !tokenTotal(day) && <line x1={x} x2={x + barWidth} y1="172" y2="172" stroke="var(--panel-text-tertiary)" strokeDasharray="2 2" />}
          {(days.length <= 7 || index === 0 || index === days.length - 1 || index % Math.ceil(days.length / 5) === 0) &&
            <text x={x + barWidth / 2} y="197" textAnchor="middle" className="usage-axis">{day.date.slice(5).replace('-', '/')}</text>}
        </g>;
      })}
    </svg>
    {active && <div className="usage-day-detail" aria-live="polite">
      <span>{active.date}</span>
      {active.requests || tokenTotal(active) ? <><strong>{formatCount(tokenTotal(active))} Token</strong><span>{formatCount(active.requests)} {copy.calls}</span></>
        : <span>{copy.noDay}</span>}
    </div>}
    {active && (active.requests || tokenTotal(active)) ? <TokenBreakdown usage={sumUsage([active])} copy={copy} /> : null}
  </section>;
}
