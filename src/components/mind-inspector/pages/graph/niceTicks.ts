/**
 * 整齐时间刻度生成
 *
 * 在给定的时间范围内，按 5 分钟 ~ 1 年的阶梯选取合适步长，
 * 并把刻度对齐到本地时区的整点 / 整日 / 整月，返回刻度及其标签。
 */

export interface NiceTick {
  ts: number;
  label: string;
  major: boolean;
}

const MINUTE = 60 * 1000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

type TickFmt = 'time' | 'date' | 'month';

interface StepDef {
  step: number;
  alignDay: boolean;
  fmt: TickFmt;
}

const STEPS: StepDef[] = [
  { step: 5 * MINUTE, alignDay: false, fmt: 'time' },
  { step: 15 * MINUTE, alignDay: false, fmt: 'time' },
  { step: 30 * MINUTE, alignDay: false, fmt: 'time' },
  { step: HOUR, alignDay: false, fmt: 'time' },
  { step: 3 * HOUR, alignDay: false, fmt: 'time' },
  { step: 6 * HOUR, alignDay: false, fmt: 'time' },
  { step: 12 * HOUR, alignDay: false, fmt: 'time' },
  { step: DAY, alignDay: true, fmt: 'date' },
  { step: 2 * DAY, alignDay: true, fmt: 'date' },
  { step: 7 * DAY, alignDay: true, fmt: 'date' },
  { step: 14 * DAY, alignDay: true, fmt: 'date' },
  { step: 30 * DAY, alignDay: true, fmt: 'month' },
  { step: 90 * DAY, alignDay: true, fmt: 'month' },
  { step: 180 * DAY, alignDay: true, fmt: 'month' },
  { step: 365 * DAY, alignDay: true, fmt: 'month' },
];

const pad = (n: number): string => (n < 10 ? `0${n}` : `${n}`);

const formatTick = (ts: number, fmt: TickFmt): string => {
  const d = new Date(ts);
  if (fmt === 'time') return `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  if (fmt === 'date') return `${d.getMonth() + 1}/${d.getDate()}`;
  return `${d.getFullYear()}/${d.getMonth() + 1}`;
};

export function niceTicks(minTs: number, maxTs: number, targetCount: number): NiceTick[] {
  if (maxTs <= minTs) return [];
  const span = maxTs - minTs;
  const rawStep = span / Math.max(1, targetCount);
  let def = STEPS[STEPS.length - 1];
  for (const s of STEPS) {
    if (s.step >= rawStep) {
      def = s;
      break;
    }
  }

  const ticks: NiceTick[] = [];

  // 月级刻度：按整月遍历
  if (def.fmt === 'month') {
    const monthStep = Math.max(1, Math.round(def.step / (30 * DAY)));
    const d = new Date(minTs);
    d.setDate(1);
    d.setHours(0, 0, 0, 0);
    d.setMonth(Math.floor(d.getMonth() / monthStep) * monthStep);
    while (d.getTime() < minTs) d.setMonth(d.getMonth() + monthStep);
    while (d.getTime() <= maxTs) {
      ticks.push({ ts: d.getTime(), label: formatTick(d.getTime(), 'month'), major: true });
      d.setMonth(d.getMonth() + monthStep);
    }
    return ticks;
  }

  // 日级刻度：按本地日历整日遍历（自动处理跨月与夏令时）
  if (def.alignDay) {
    const days = Math.max(1, Math.round(def.step / DAY));
    const d = new Date(minTs);
    d.setHours(0, 0, 0, 0);
    d.setDate(Math.floor((d.getDate() - 1) / days) * days + 1);
    while (d.getTime() < minTs) d.setDate(d.getDate() + days);
    while (d.getTime() <= maxTs) {
      ticks.push({ ts: d.getTime(), label: formatTick(d.getTime(), 'date'), major: true });
      d.setDate(d.getDate() + days);
    }
    return ticks;
  }

  // 时间级刻度：对齐到本地时区的整步长
  const step = def.step;
  const tzOff = -new Date(minTs).getTimezoneOffset() * MINUTE;
  const start = Math.ceil((minTs + tzOff) / step) * step - tzOff;
  for (let ts = start; ts <= maxTs; ts += step) {
    ticks.push({ ts, label: formatTick(ts, 'time'), major: false });
  }
  return ticks;
}
