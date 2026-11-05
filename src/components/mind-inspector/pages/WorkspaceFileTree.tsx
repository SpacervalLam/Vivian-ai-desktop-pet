import React, { useState, useEffect, useRef, useCallback, useMemo } from 'react';
import { invoke, Channel } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { Loader2, Search } from 'lucide-react';
import WorkFileTree from './WorkFileTree';
import { workspaceTreeInput, type WorkspaceFileNode as FileNode } from './workspaceTreeModel';
import './changeReviewStrings';

const WorkspaceFileTree: React.FC<{
  root: string;
  onOpenFile: (path: string) => void;
}> = ({ root, onOpenFile }) => {
  const { t } = useTranslation();
  const [roots, setRoots] = useState<FileNode[] | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set([root]));
  const [loading, setLoading] = useState<Set<string>>(new Set());
  const [error, setError] = useState('');
  /** 单层懒加载目录缓存：key=目录路径 → 子节点列表（随组件实例隔离，切换工作区自动重建） */
  const depthCache = useRef<Record<string, FileNode[]>>({}).current;
  const [filter, setFilter] = useState('');

  // 切换工作区时重置 UI；旧请求由下面 effect 的清理函数隔离。
  useEffect(() => {
    setError('');
    setFilter('');
    setRoots(null);
    setExpanded(new Set([root]));
    setLoading(new Set());
    for (const k of Object.keys(depthCache)) delete depthCache[k];
  }, [root, depthCache]);

  useEffect(() => {
    if (!root) return;
    let cancelled = false;
    let refreshing = false;
    let timer: number | undefined;
    let watchId: string | undefined;
    const directories = [root, ...Array.from(expanded).filter((path) => path !== root)];
    const normalize = (path: string) => path.replace(/\\/g, '/').replace(/\/$/, '');
    const pending = new Set<string>(directories);
    const isHidden = () => document.visibilityState === 'hidden';
    const refresh = async () => {
      if (cancelled || refreshing || isHidden() || !pending.size) return;
      refreshing = true;
      const changed = Array.from(pending);
      pending.clear();
      setLoading(new Set(changed.filter((path) => path !== root && depthCache[path] === undefined)));
      try {
        const results = await Promise.allSettled(changed.map((directory) =>
          invoke<FileNode[]>('coding_list_dir_tree', { directory, maxDepth: 1 })));
        if (cancelled) return;
        for (const path of Object.keys(depthCache)) {
          if (!expanded.has(path)) delete depthCache[path];
        }
        results.forEach((result, index) => {
          const path = changed[index];
          if (result.status === 'fulfilled') {
            const children = new Set(result.value.filter((node) => node.is_dir).map((node) => normalize(node.path)));
            setExpanded((previous) => {
              const removed = Array.from(previous).filter((entry) => {
                const normalized = normalize(entry);
                return normalized.slice(0, normalized.lastIndexOf('/')) === normalize(path) && !children.has(normalized);
              });
              if (!removed.length) return previous;
              return new Set(Array.from(previous).filter((entry) => !removed.some((removedPath) =>
                entry === removedPath || normalize(entry).startsWith(normalize(removedPath) + '/'))));
            });
          }
          if (path === root) {
            if (result.status === 'fulfilled') {
              setRoots(result.value || []);
              setError('');
            } else setError(String(result.reason));
          } else depthCache[path] = result.status === 'fulfilled' ? result.value || [] : [];
        });
        setLoading(new Set());
      } finally {
        refreshing = false;
        if (!cancelled && pending.size && !isHidden()) schedule();
      }
    };
    const schedule = () => {
      // 合并连续创建/删除/重命名事件；持续变更时也不会无限推迟刷新。
      if (cancelled || timer !== undefined) return;
      timer = window.setTimeout(() => { timer = undefined; void refresh(); }, 100);
    };
    const onVisible = () => { if (pending.size) schedule(); };
    const channel = new Channel<string>();
    channel.onmessage = (directory) => {
      if (cancelled) return;
      for (const path of directories) {
        if (!directory || normalize(path) === normalize(directory)) pending.add(path);
      }
      if (pending.size) schedule();
    };
    // 先建立监听，再读取快照，避免初始读取与注册监听之间漏掉变更。
    void invoke<string>('coding_watch_directories', { directories, onChange: channel }).then((id) => {
      if (cancelled) {
        void invoke('coding_unwatch_directories', { id }).catch(console.error);
      } else {
        watchId = id;
        void refresh();
      }
    }).catch((error) => {
      if (!cancelled) {
        setError(String(error));
        console.error('Workspace filesystem watch failed', error);
      }
    });
    window.addEventListener('focus', onVisible);
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
      if (watchId) void invoke('coding_unwatch_directories', { id: watchId }).catch(console.error);
      window.removeEventListener('focus', onVisible);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [root, expanded, depthCache]);

  const input = useMemo(() => workspaceTreeInput(root, roots ?? [], depthCache), [root, roots, depthCache, loading]);
  const onExpandedPathsChange = useCallback((paths: readonly string[]) => {
    const next = new Set([root, ...paths.flatMap(path => {
      const node = input.nodes.get(path);
      return node ? [node.path] : [];
    })]);
    setExpanded(previous => previous.size === next.size && [...previous].every(path => next.has(path)) ? previous : next);
  }, [root, input]);
  const onSelect = useCallback((path: string) => {
    const node = input.nodes.get(path);
    if (node && !node.is_dir) onOpenFile(node.path);
  }, [input, onOpenFile]);

  return <div className="workspace-file-tree">
    <label className="workspace-file-tree-search"><Search size={12} aria-hidden="true" />
      <input value={filter} onChange={event => setFilter(event.target.value)}
        onKeyDown={event => { if (event.key === 'Escape') setFilter(''); }}
        placeholder={t('changeReview.treeSearch')} aria-label={t('changeReview.treeSearch')} />
    </label>
    {error && <div className="codex-tree-error" role="alert">{error}</div>}
    {roots === null ? <div className="codex-tree-loading" role="status"><Loader2 size={13} className="codex-spin" /></div>
      : roots.length === 0 ? <div className="codex-tree-empty">{t('mind_inspector.code_tree_no_files', { defaultValue: '工作区为空' })}</div>
      : <WorkFileTree paths={input.paths} query={filter} label={t('mind_inspector.code_workspace_files')}
          onSelect={onSelect} onExpandedPathsChange={onExpandedPathsChange} />}
  </div>;
};

export default WorkspaceFileTree;
