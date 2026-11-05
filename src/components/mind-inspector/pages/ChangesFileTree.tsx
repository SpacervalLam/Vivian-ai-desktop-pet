import React, { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import WorkFileTree from './WorkFileTree';
import { changedFileTreeInput, type ChangedFile } from './changeReviewModel';

interface Props {
  files: readonly ChangedFile[];
  selected: string;
  filter: string;
  onSelect: (path: string) => void;
}

export default React.memo(function ChangesFileTree({ files, selected, filter, onSelect }: Props) {
  const { t } = useTranslation();
  const input = useMemo(() => changedFileTreeInput(files, filter), [files, filter]);
  const byPath = useMemo(() => new Map(files.map(file => [file.path, file])), [files]);
  if (!input.paths.length) return <p className="change-review-hint" role="status">{t('changeReview.noMatches')}</p>;
  return <WorkFileTree paths={input.paths} selectedPath={selected} gitStatus={input.gitStatus}
    label={t('changeReview.title')} initiallyExpanded flatten onSelect={path => { if (byPath.has(path)) onSelect(path); }}
    renderRowDecoration={({ item }) => {
      const file = byPath.get(item.path);
      if (file?.conflict) return { text: '!', title: t('changeReview.conflict') };
      return file?.original_path ? { text: '↪', title: `${file.original_path} → ${file.path}` } : null;
    }} />;
});
