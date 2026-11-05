import React, { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { diffLines } from './changeReviewModel';
import ReviewDiffView from './ReviewDiffView';
import './changeReviewStrings';
import './ChangesPanel.css';

/** Tool snapshots use the same renderer without adding review actions. */
export default React.memo(function WorkDiffBlock({ text, maxHeight }: { text: string; maxHeight?: number }) {
  const { t } = useTranslation();
  const diff = useMemo(() => ({ patch: text, revision: crypto.randomUUID(), binary: false, truncated: false }), [text]);
  const rows = useMemo(() => diffLines(text), [text]);
  return <div className="change-review work-diff-block">
    <ReviewDiffView path={t('changeReview.title')} diff={diff} rows={rows} maxHeight={maxHeight} />
  </div>;
});
