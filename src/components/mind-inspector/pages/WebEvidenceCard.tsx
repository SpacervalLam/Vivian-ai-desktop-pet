import { useTranslation } from 'react-i18next';
import { MarkdownText } from './codeMarkdown';
import { parseWebEvidence } from './webEvidence';
import './WebEvidenceCard.css';

export default function WebEvidenceCard({ result }: { result?: string }) {
  const { t } = useTranslation();
  const evidence = parseWebEvidence(result);
  if (!evidence) return null;
  return <div className="web-evidence-card">
    <div className="web-evidence-caption">{t('config.web_evidence_sources')}</div>
    {evidence.sources.map((source) => <div className="web-evidence-source" key={source.url}>
      <a href={source.url} target="_blank" rel="noopener noreferrer">{source.title || source.url}</a>
      <div className="web-evidence-meta">
        <span>{new URL(source.url).hostname}</span>
        {source.published_at && <span>{source.published_at}</span>}
        {source.engines?.length ? <span>{source.engines.join(' · ')}</span> : null}
      </div>
      {source.snippet && <p>{source.snippet}</p>}
    </div>)}
    {evidence.summary && <details open><summary>{t('config.web_evidence_summary')}</summary><MarkdownText text={evidence.summary} /></details>}
    {evidence.text && <details><summary>{t('config.web_evidence_excerpt')}</summary><MarkdownText text={evidence.text} /></details>}
    {evidence.artifact?.text_path && <MarkdownText text={`[${t('config.web_evidence_saved')}](${evidence.artifact.text_path.replace(/\\/g, '/')})`} />}
    {evidence.artifact?.pdf_path && <MarkdownText text={`[PDF](${evidence.artifact.pdf_path.replace(/\\/g, '/')})`} />}
    {evidence.warnings.map((warning, i) => <p className="web-evidence-warning" key={i}>{warning}</p>)}
  </div>;
}
