import { useState } from 'react';
import { useTranslation } from 'react-i18next';

/** Mount children only on expansion, and page wide arrays/objects in bounded batches. */
function JsonNode({ name, value }: { name: string; value: unknown }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const [limit, setLimit] = useState(50);
  const [expandedText, setExpandedText] = useState(false);
  if (value !== null && typeof value === 'object') {
    const entries = Object.entries(value);
    const array = Array.isArray(value);
    return <details className="workflow-json-node" onToggle={event => setOpen(event.currentTarget.open)}>
      <summary><b>{name}</b> <span>{array ? `[${entries.length}]` : `{${entries.length}}`}</span></summary>
      {open && <div className="workflow-json-children">
        {entries.slice(0, limit).map(([key, child]) => <JsonNode key={key} name={key} value={child} />)}
        {entries.length > limit && <button type="button" onClick={() => setLimit(n => n + 50)}>{t('workflow.more', { n: Math.min(50, entries.length - limit) })}</button>}
      </div>}
    </details>;
  }
  const text = typeof value === 'string' ? value : JSON.stringify(value) ?? String(value);
  return <div className="workflow-json-value"><b>{name}</b>: <span>{!expandedText && text.length > 2000 ? `${text.slice(0, 2000)}…` : text}</span>
    {!expandedText && text.length > 2000 && <button type="button" onClick={() => setExpandedText(true)}>{t('workflow.expand')}</button>}
  </div>;
}

export default function WorkflowJson({ label, value }: { label: string; value: unknown }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState('');
  return <section className="workflow-json-section">
    <header><strong>{label}</strong><button type="button" onClick={async () => {
      try {
        await navigator.clipboard.writeText(JSON.stringify(value, null, 2) ?? 'null');
        setCopied(true); setError('');
      } catch (cause) { setError(t('workflow.copyFailed', { error: String(cause) })); }
    }}>{t(copied ? 'workflow.copied' : 'workflow.copy')}</button></header>
    {error && <p role="alert">{error}</p>}
    <JsonNode key={label} name={label} value={value} />
  </section>;
}
