export type DiffScope = 'all' | 'staged' | 'unstaged';
export interface ChangedFile {
  path: string;
  original_path: string | null;
  index_status: string;
  worktree_status: string;
  untracked: boolean;
  conflict: boolean;
}
export interface ChangesSnapshot {
  is_repo: boolean;
  root: string;
  files: ChangedFile[];
  truncated: boolean;
}
export interface FileDiff {
  patch: string;
  revision: string;
  binary: boolean;
  truncated: boolean;
}
export interface DiffLine {
  text: string;
  kind: 'meta' | 'hunk' | 'add' | 'delete' | 'context';
  oldLine?: number;
  newLine?: number;
}
export interface ReviewComment {
  id: string;
  path: string;
  scope: DiffScope;
  revision: string;
  side: 'old' | 'new';
  line: number;
  quote: string;
  body: string;
}

export function diffLines(patch: string): DiffLine[] {
  let oldLine = 0, newLine = 0, oldRemaining = 0, newRemaining = 0;
  return patch.replace(/\r\n/g, '\n').split('\n').map(text => {
    const hunk = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/.exec(text);
    if (hunk) {
      oldLine = Number(hunk[1]); newLine = Number(hunk[3]);
      oldRemaining = Number(hunk[2] ?? 1); newRemaining = Number(hunk[4] ?? 1);
      return { text, kind: 'hunk' };
    }
    if (oldRemaining || newRemaining) {
      if (text.startsWith('+') && newRemaining) {
        newRemaining--; return { text, kind: 'add', newLine: newLine++ };
      }
      if (text.startsWith('-') && oldRemaining) {
        oldRemaining--; return { text, kind: 'delete', oldLine: oldLine++ };
      }
      if (text.startsWith(' ') && oldRemaining && newRemaining) {
        oldRemaining--; newRemaining--;
        return { text, kind: 'context', oldLine: oldLine++, newLine: newLine++ };
      }
    }
    return { text, kind: 'meta' };
  });
}

export function reviewStorageKey(workspace: string): string {
  return `vivian.change-review.v1:${workspace.replace(/\\/g, '/').replace(/\/$/, '')}`;
}

export function readReviewComments(workspace: string): ReviewComment[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(reviewStorageKey(workspace)) ?? '[]');
    if (!Array.isArray(value)) return [];
    return value.filter((c): c is ReviewComment => !!c && typeof c === 'object'
      && typeof c.id === 'string' && typeof c.path === 'string' && typeof c.revision === 'string'
      && ['all', 'staged', 'unstaged'].includes(c.scope) && ['old', 'new'].includes(c.side)
      && Number.isSafeInteger(c.line) && c.line > 0 && typeof c.quote === 'string'
      && typeof c.body === 'string' && c.body.trim().length > 0).slice(0, 100);
  } catch { return []; }
}

export function reviewPrompt(workspace: string, comments: readonly ReviewComment[], t: (key: string) => string): string {
  const blocks = comments.map((comment, index) => {
    const runs = comment.quote.match(/`+/g) ?? [];
    const fence = '`'.repeat(Math.max(3, ...runs.map(run => run.length + 1)));
    return `### ${index + 1}. ${JSON.stringify(comment.path)}\n${t(`changeReview.${comment.scope}`)} · ${t(`changeReview.${comment.side}`)} · L${comment.line}\n\n${fence}\n${comment.quote}\n${fence}\n\n${comment.body}`;
  });
  return `${t('changeReview.reviewInstruction')}\n\n${t('changeReview.workspace')}: ${JSON.stringify(workspace)}\n\n${blocks.join('\n\n')}`;
}
