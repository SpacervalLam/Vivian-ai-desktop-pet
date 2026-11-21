import React, { useMemo, useRef, useState, useEffect } from 'react';
import { parsePatchFiles, type CodeViewItem, type DiffLineEventBaseProps, type LineEventBaseProps } from '@pierre/diffs';
import { CodeView } from '@pierre/diffs/react';
import { useVirtualizer } from '@tanstack/react-virtual';
import { useTranslation } from 'react-i18next';
import type { DiffLine, FileDiff } from './changeReviewModel';

interface Props {
  path: string;
  diff: FileDiff;
  rows: readonly DiffLine[];
  onComment?: (row: DiffLine, side: 'old' | 'new') => void;
  maxHeight?: number;
}

const PREFERENCES_KEY = 'vivian.diff-view.v1';
function readPreferences(): { layout: 'unified' | 'split'; wrap: boolean } {
  try {
    const saved = JSON.parse(localStorage.getItem(PREFERENCES_KEY) ?? 'null');
    return { layout: saved?.layout === 'split' ? 'split' : 'unified', wrap: saved?.wrap === true };
  } catch { return { layout: 'unified', wrap: false }; }
}

function useDiffTheme() {
  const resolve = () => document.documentElement.dataset.theme === 'dark' ? 'dark' as const
    : document.documentElement.dataset.theme === 'light' ? 'light' as const
    : window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' as const : 'light' as const;
  const [theme, setTheme] = useState(resolve);
  useEffect(() => {
    const update = () => setTheme(resolve());
    const observer = new MutationObserver(update);
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme'] });
    media.addEventListener('change', update);
    return () => { observer.disconnect(); media.removeEventListener('change', update); };
  }, []);
  return theme;
}

/** Truncated patches and combined conflict diffs still show every captured byte. */
function RawPatch({ path, rows, onComment, maxHeight = 440 }: Props) {
  const { t } = useTranslation();
  const container = useRef<HTMLDivElement>(null);
  const virtual = useVirtualizer({ count: rows.length, getScrollElement: () => container.current,
    estimateSize: () => 22, overscan: 10 });
  const width = useMemo(() => Math.max(100, ...rows.map(row => row.text.length + 12)), [rows]);
  return <div ref={container} className="change-review-diff change-review-raw" style={{ height: Math.min(maxHeight, Math.max(80, rows.length * 22)) }} aria-label={path} tabIndex={0}>
    <div style={{ height: virtual.getTotalSize(), minWidth: `${Math.min(width, 2000)}ch`, position: 'relative' }}>
      {virtual.getVirtualItems().map(item => {
        const row = rows[item.index];
        return <div key={item.key} className={`change-review-line ${row.kind}`}
          style={{ position: 'absolute', top: item.start, height: item.size, width: '100%' }}>
          <span className="change-review-gutter">{(['old', 'new'] as const).map(side => {
            const line = side === 'old' ? row.oldLine : row.newLine;
            return line === undefined ? <span key={side} /> : !onComment ? <span key={side}>{line}</span> : <button key={side} type="button"
              title={`${t('changeReview.comment')} · ${t(`changeReview.${side}`)} ${line}`}
              aria-label={`${t('changeReview.comment')} ${t(`changeReview.${side}`)} ${line}`}
              onClick={() => onComment(row, side)}>{line}</button>;
          })}</span><code>{row.text || ' '}</code>
        </div>;
      })}
    </div>
  </div>;
}

export default React.memo(function ReviewDiffView(props: Props) {
  const { path, diff, rows, onComment, maxHeight = 440 } = props;
  const { t } = useTranslation();
  const themeType = useDiffTheme();
  const [preferences, setPreferences] = useState(readPreferences);
  const parsed = useMemo(() => {
    try {
      const files = parsePatchFiles(diff.patch, { cacheKeyPrefix: diff.revision, disableLineMatching: true, throwOnError: true }).flatMap(patch => patch.files);
      return files.length ? files : null;
    } catch { return null; }
  }, [diff.patch, diff.revision]);
  const items = useMemo<CodeViewItem<undefined>[]>(() => (parsed ?? []).map((fileDiff, index) => ({
    id: `${path}:${diff.revision}:${index}`, type: 'diff', fileDiff,
  })), [parsed, path, diff.revision]);
  const lineMaps = useMemo(() => {
    const old = new Map<number, DiffLine>(), next = new Map<number, DiffLine>();
    for (const row of rows) {
      if (row.oldLine !== undefined) old.set(row.oldLine, row);
      if (row.newLine !== undefined) next.set(row.newLine, row);
    }
    return { old, new: next };
  }, [rows]);
  const height = Math.min(maxHeight, Math.max(160, (parsed ?? []).reduce((total, file) => total
    + file.hunks.reduce((lines, hunk) => lines + (preferences.layout === 'split' ? hunk.splitLineCount : hunk.unifiedLineCount) + 2, 0), 0) * 22 + 16));
  useEffect(() => {
    try { localStorage.setItem(PREFERENCES_KEY, JSON.stringify(preferences)); } catch { /* Display preferences are optional. */ }
  }, [preferences]);
  const options = useMemo(() => ({
    theme: { light: 'pierre-light', dark: 'pierre-dark' }, themeType, diffStyle: preferences.layout,
    overflow: preferences.wrap ? 'wrap' as const : 'scroll' as const,
    diffIndicators: 'bars' as const, lineDiffType: 'word-alt' as const, disableFileHeader: true,
    tokenizeMaxLength: 150_000, tokenizeMaxLineLength: 2000, maxLineDiffLength: 2000,
    enableLineSelection: true, lineHoverHighlight: 'both' as const,
    onLineNumberClick: (event: LineEventBaseProps | DiffLineEventBaseProps) => {
      if (event.type !== 'diff-line') return;
      const side = event.annotationSide === 'deletions' ? 'old' : 'new';
      const row = lineMaps[side].get(event.lineNumber);
      if (row) onComment?.(row, side);
    },
  }), [preferences, lineMaps, onComment, themeType]);

  if (!parsed) return <><p className="change-review-hint">{t('changeReview.rawPatch')}</p><RawPatch {...props} /></>;
  if (parsed.every(file => file.hunks.length === 0)) return <>
    <p>{t('changeReview.noDiff')}</p><pre className="change-review-metadata">{diff.patch}</pre>
  </>;
  return <>
    <div className="change-review-diff-toolbar" role="group" aria-label={t('changeReview.display')}>
      {(['unified', 'split'] as const).map(layout => <button key={layout} type="button" aria-pressed={preferences.layout === layout}
        onClick={() => setPreferences(current => ({ ...current, layout }))}>{t(`changeReview.${layout}`)}</button>)}
      <button type="button" aria-pressed={preferences.wrap} onClick={() => setPreferences(current => ({ ...current, wrap: !current.wrap }))}>{t('changeReview.wrap')}</button>
    </div>
    <div className="change-review-code-region" role="region" aria-label={path}>
      <CodeView className="change-review-code-view" style={{ height }} items={items} options={options}
        containerRef={element => { if (element) element.tabIndex = 0; }} />
    </div>
  </>;
});
