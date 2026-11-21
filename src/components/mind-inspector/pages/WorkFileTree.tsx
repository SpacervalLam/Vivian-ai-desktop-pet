import React, { useLayoutEffect, useMemo, useRef } from 'react';
import { prepareFileTreeInput, type GitStatusEntry, type FileTreeRowDecorationRenderer } from '@pierre/trees';
import { FileTree, useFileTree, useFileTreeSelector } from '@pierre/trees/react';
import './WorkFileTree.css';

interface Props {
  paths: readonly string[];
  label: string;
  query?: string;
  selectedPath?: string;
  gitStatus?: readonly GitStatusEntry[];
  initiallyExpanded?: boolean;
  flatten?: boolean;
  onSelect: (path: string) => void;
  onExpandedPathsChange?: (paths: readonly string[]) => void;
  renderRowDecoration?: FileTreeRowDecorationRenderer;
}

/** Path identities survive refreshes; only the visible rows are mounted. */
export default React.memo(function WorkFileTree({ paths, label, query = '', selectedPath, gitStatus,
  initiallyExpanded = false, flatten = false, onSelect, onExpandedPathsChange, renderRowDecoration }: Props) {
  const callbacks = useRef({ onSelect, onExpandedPathsChange, renderRowDecoration });
  callbacks.current = { onSelect, onExpandedPathsChange, renderRowDecoration };
  const updating = useRef(false);
  const { model } = useFileTree({ paths: [], initialExpansion: initiallyExpanded ? 'open' : 'closed',
    flattenEmptyDirectories: flatten, density: 'compact', icons: 'standard', stickyFolders: true,
    fileTreeSearchMode: 'hide-non-matches',
    renderRowDecoration: context => callbacks.current.renderRowDecoration?.(context) ?? null,
    onSelectionChange: selected => { if (!updating.current && selected.length) callbacks.current.onSelect(selected[selected.length - 1]); },
  });
  const preparedInput = useMemo(() => prepareFileTreeInput(paths), [paths]);
  const directories = useMemo(() => paths.filter(path => path.endsWith('/')), [paths]);
  const visibleCount = useFileTreeSelector(model, tree => tree.getVisibleCount());

  useLayoutEffect(() => {
    const expanded = directories.filter(path => {
      const item = model.getItem(path);
      return item && 'isExpanded' in item ? item.isExpanded() : initiallyExpanded;
    });
    updating.current = true;
    try { model.resetPaths({ preparedInput, initialExpandedPaths: expanded }); }
    finally { updating.current = false; }
  }, [model, preparedInput, directories, initiallyExpanded]);

  useLayoutEffect(() => model.subscribe(() => {
    if (updating.current || !callbacks.current.onExpandedPathsChange) return;
    callbacks.current.onExpandedPathsChange(directories.filter(path => {
      const item = model.getItem(path);
      return item && 'isExpanded' in item && item.isExpanded();
    }));
  }), [model, directories]);

  useLayoutEffect(() => { model.setSearch(query.trim() || null); }, [model, query]);
  useLayoutEffect(() => { model.setGitStatus(gitStatus); }, [model, gitStatus]);
  useLayoutEffect(() => {
    if (selectedPath === undefined) return;
    updating.current = true;
    try {
      for (const path of model.getSelectedPaths()) if (path !== selectedPath) model.getItem(path)?.deselect();
      if (selectedPath) model.getItem(selectedPath)?.select();
    } finally { updating.current = false; }
  }, [model, selectedPath, preparedInput]);

  // React 18 serializes className literally on custom elements; style the wrapper.
  return <div className="work-file-tree" style={{ height: Math.min(252, Math.max(36, visibleCount * model.getItemHeight() + 16)) }}>
    <FileTree model={model} aria-label={label} style={{ display: 'block', height: '100%' }} />
  </div>;
});
