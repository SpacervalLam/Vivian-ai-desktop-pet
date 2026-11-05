export interface WorkspaceFileNode {
  name: string;
  path: string;
  is_dir: boolean;
  children?: WorkspaceFileNode[];
}

export function relativeTreePath(root: string, path: string): string {
  const base = root.replace(/\\/g, '/').replace(/\/$/, '');
  const normalized = path.replace(/\\/g, '/');
  return normalized.slice(base.length + 1);
}

export function workspaceTreeInput(root: string, roots: readonly WorkspaceFileNode[], cache: Record<string, WorkspaceFileNode[]>) {
  const nodes = new Map<string, WorkspaceFileNode>();
  const visit = (items: readonly WorkspaceFileNode[]) => {
    for (const node of items) {
      const path = relativeTreePath(root, node.path) + (node.is_dir ? '/' : '');
      nodes.set(path, node);
      if (node.is_dir) visit(cache[node.path] ?? node.children ?? []);
    }
  };
  visit(roots);
  return { paths: [...nodes.keys()], nodes };
}
