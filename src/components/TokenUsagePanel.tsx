import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';

interface UsageRow {
  task: string;
  input: number;
  output: number;
  hit: number;
  requests: number;
}
interface UsageDay { input: number; output: number; hit: number; requests: number }
interface UsageReport { days: UsageDay[]; tasks: UsageRow[] }

const taskNames: Record<string, string> = {
  chat: '对话', reasoning: '工具对话', reflection: '回复后反思',
  inner_monologue: '内心独白', current_thought: '当前想法', proactive_channel: '主动消息渠道判断',
  proactive_message: '主动消息正文', proactive_share: '主动分享',
  consolidation: '记忆整理', unattributed: '未归类',
};

export default function TokenUsagePanel() {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh');
  const [report, setReport] = useState<UsageReport | null>(null);
  const [error, setError] = useState(false);
  const refresh = useCallback(() => {
    void invoke<UsageReport>('get_token_usage', { days: 7 })
      .then((data) => { setReport(data); setError(false); })
      .catch(() => setError(true));
  }, []);
  useEffect(refresh, [refresh]);
  const totals = report?.days.reduce((acc, day) => ({
    input: acc.input + day.input, output: acc.output + day.output,
    hit: acc.hit + day.hit, requests: acc.requests + day.requests,
  }), { input: 0, output: 0, hit: 0, requests: 0 });
  const categorizedRequests = report?.tasks.reduce((sum, task) => sum + task.requests, 0) ?? 0;
  const fmt = (n: number) => n.toLocaleString();

  return <section style={{ marginTop: 28, color: 'var(--panel-text)', fontSize: 12 }}>
    <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
      <strong>{zh ? '近 7 天 Token 用量' : 'Token usage · last 7 days'}</strong>
      <button type="button" onClick={refresh} style={{ background: 'transparent', color: 'var(--panel-accent)', border: 'none', cursor: 'pointer' }}>
        {zh ? '刷新' : 'Refresh'}
      </button>
    </div>
    {error && <p>{zh ? '暂时无法读取用量记录。' : 'Usage report is unavailable.'}</p>}
    {totals && <>
      <p style={{ color: 'var(--panel-text-secondary)' }}>
        {zh ? '请求' : 'Requests'} {fmt(totals.requests)} · {zh ? '非缓存输入' : 'Uncached input'} {fmt(totals.input)} · {zh ? '输出' : 'Output'} {fmt(totals.output)} · {zh ? '缓存读取' : 'Cache reads'} {fmt(totals.hit)}
      </p>
      {report && report.tasks.length > 0 && <div style={{ display: 'grid', gap: 6 }}>
        {report.tasks.slice(0, 8).map((row) => <div key={row.task} style={{ display: 'flex', justifyContent: 'space-between', gap: 12, borderBottom: '1px solid var(--panel-border)', paddingBottom: 4 }}>
          <span>{zh ? (taskNames[row.task] ?? row.task) : row.task}</span>
          <span style={{ fontVariantNumeric: 'tabular-nums', color: 'var(--panel-text-secondary)' }}>
            {fmt(row.requests)} {zh ? '次' : 'calls'} · {fmt(row.input + row.output)} tokens
          </span>
        </div>)}
      </div>}
      {report && totals && categorizedRequests < totals.requests && <p style={{ color: 'var(--panel-text-tertiary)' }}>
        {zh ? '任务分类从本次更新后开始记录；旧记录只有总量，所以上方分类尚不覆盖全部请求。' : 'Task categories begin with this update; older usage has totals only, so the breakdown is partial.'}
      </p>}
    </>}
  </section>;
}
