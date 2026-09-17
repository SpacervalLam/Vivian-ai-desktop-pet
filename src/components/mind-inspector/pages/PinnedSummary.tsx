/**
 * PinnedSummary — 工作页「置顶摘要」
 *
 *   1. 环境信息 —— git 仓库状态（分支 / 变更行数 / 领先落后 / 最近提交），
 *      外加两个写操作入口（提交或推送、比较分支）。写操作一律先弹确认框。
 *   2. 来源 —— 插件 / 技能 / MCP 三类外部来源的汇总，可展开明细。
 *
 * **它是主工作区右侧的信息列**（`codex-main-col` 的绝对定位子元素，紧贴对话
 * 滚动条的左边）：宽度由父组件按主工作区实测宽度算好，写进祖先的
 * `--codex-pinned-w` / `--codex-pinned-w-expanded`，组件自己只读不持有；同时
 * 父组件让对话区 / 输入区 / 统计行统一右缩 `--codex-pinned-reserve`，所以面板
 * 浮在那块留白上、不会盖住正文。窗口太窄时先压面板、再压对话区（策略见
 * `CodeAgentPageNew.tsx` 的 PINNED_W_*）。收起时宽度归零 + `overflow:hidden`，
 * 靠内层固定展开宽度做裁切而不是逐帧重排。
 *
 * **整块的收起 / 呼出由顶栏按钮控制**（`visible` prop，按钮夹在模式下拉与右侧
 * 检查器按钮之间）——面板紧贴顶栏下方，开关放在它正上方那条栏上最顺手，
 * 也避免用户为了关掉面板去翻别处。可见性状态由父组件持有并落盘。
 * 每张卡片内部仍可单独折叠（只看 git 或只看来源）。
 *
 * 为什么不做成右侧检查器的第六个页签：摘要的价值全在「随时可见」。塞进页签后
 * 用户得先切过去才知道当前分支和有没有未提交改动，那就不叫摘要了。
 *
 * 数据源：后端 `commands::git::*`（只读探测 + 两个显式写操作）与既有的
 * `list_plugins` / `list_skills` / `list_mcp_servers`。这里不新增任何聚合命令——
 * 三份清单的字段在设置页已经定型，再包一层后端结构只会多一处要同步的地方。
 */

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';
import { confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import {
  GitBranch, GitCompare, FolderGit2, Plug, Sparkles, Link2,
  ChevronDown, ChevronRight, RefreshCw, Loader2, TriangleAlert,
  Upload, Check, Plus, Trash2,
} from 'lucide-react';

// ============ 后端结构的前端镜像 ============

/** 对齐 `commands::git::GitCommitInfo` */
interface GitCommitInfo {
  hash: string;
  subject: string;
  author: string;
  /** ISO 8601 */
  time: string;
}

/** 对齐 `commands::git::GitRepoStatus` */
interface GitRepoStatus {
  is_repo: boolean;
  root: string;
  branch: string;
  detached: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
  added: number;
  removed: number;
  changed_files: number;
  staged: number;
  unstaged: number;
  untracked: number;
  dirty: boolean;
  last_commit: GitCommitInfo | null;
  remote_url: string | null;
  error: string | null;
}

/** 对齐 `commands::git::GitBranchList` */
interface GitBranchList {
  local: string[];
  remote: string[];
  default_base: string | null;
}

/** 对齐 `commands::git::GitBranchDiff` */
interface GitBranchDiff {
  base: string;
  current: string;
  ahead: number;
  behind: number;
  added: number;
  removed: number;
  commits: GitCommitInfo[];
  error: string | null;
}

/** 对齐 `commands::git::GitActionResult` */
interface GitActionResult {
  ok: boolean;
  steps: { step: string; ok: boolean; output: string }[];
  error: string | null;
}

/** 对齐 `commands::git::GitWorktreeInfo` */
interface GitWorktreeInfo {
  path: string;
  name: string;
  branch: string;
  detached: boolean;
  is_main: boolean;
  locked: boolean;
  dirty: boolean;
  changed_files: number;
  head: string;
}

/** 对齐 `commands::plugins::PluginInventoryEntry` */
interface PluginEntry {
  key: string;
  name: string;
  version: string;
  description: string;
  skills: string[];
  tools: string[];
  mcp_servers: string[];
  providers: string[];
  status: string;
  /** trusted / changed / untrusted */
  trust: string;
  reason?: string | null;
  dir: string;
}

/** 对齐 `commands::plugins::SkillEntryInfo` */
interface SkillEntry {
  name: string;
  description: string;
  scope: string | null;
  origin: string;
  body_len: number;
}

/** `list_mcp_servers` 返回的运行时条目 */
interface McpServerEntry {
  id: string;
  name: string;
  enabled: boolean;
  tool_count: number;
  alive: boolean;
}

// ============ 常量 ============

/** 状态轮询间隔。git status 在大仓库上是秒级操作，8 秒既够新鲜又不至于反复触发。 */
const POLL_MS = 8000;

/** 行数超过这个量级就不再逐位显示，避免把一行挤爆 */
function formatCount(n: number): string {
  if (n >= 100000) return `${Math.round(n / 1000)}k`;
  return String(n);
}

/** ISO 时间 → 「今天 14:05」/「09-12 14:05」 */
function shortTime(iso: string): string {
  if (!iso) return '';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  const pad = (v: number) => String(v).padStart(2, '0');
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  const now = new Date();
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();
  return sameDay ? hm : `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hm}`;
}

/** 取路径最后一段，作为「本地」那一行的显示名 */
function baseName(p: string): string {
  const parts = p.replace(/[\\/]+$/, '').split(/[\\/]/);
  return parts[parts.length - 1] || p;
}

// ============ 组件 ============

interface PinnedSummaryProps {
  /** 当前会话主工作区；空串表示无工作区（此时环境信息整块降级为提示） */
  workingDirectory: string;
  /** 整块面板是否展开，由顶栏按钮切换 */
  visible: boolean;
}

const PinnedSummary: React.FC<PinnedSummaryProps> = ({ workingDirectory, visible }) => {
  const { t } = useTranslation();

  /** 两张卡片各自的展开态。整块显隐归 `visible` 管，这里只管卡内内容。 */
  const [envOpen, setEnvOpen] = useState(true);
  const [status, setStatus] = useState<GitRepoStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const [refreshedAt, setRefreshedAt] = useState(0);

  // 「提交或推送」
  const [commitOpen, setCommitOpen] = useState(false);
  const [commitMessage, setCommitMessage] = useState('');
  const [busy, setBusy] = useState(false);
  const [actionResult, setActionResult] = useState<GitActionResult | null>(null);

  // 「比较分支」
  const [compareOpen, setCompareOpen] = useState(false);
  const [branches, setBranches] = useState<GitBranchList | null>(null);
  const [compareBase, setCompareBase] = useState('');
  const [diff, setDiff] = useState<GitBranchDiff | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);

  // ---- 工作树（隔离执行）----
  // 列表只在区块展开时拉：它是比 git status 更重的探测（每棵工作树各跑一次 status），
  // 没必要跟着 8 秒轮询一起跑。
  const [worktreesOpen, setWorktreesOpen] = useState(false);
  const [worktrees, setWorktrees] = useState<GitWorktreeInfo[] | null>(null);
  const [worktreesLoading, setWorktreesLoading] = useState(false);
  const [newWorktreeName, setNewWorktreeName] = useState('');
  const [worktreeBusy, setWorktreeBusy] = useState(false);
  const [worktreeError, setWorktreeError] = useState<string | null>(null);

  // 「来源」
  const [sourcesOpen, setSourcesOpen] = useState(false);
  const [plugins, setPlugins] = useState<PluginEntry[] | null>(null);
  const [skills, setSkills] = useState<SkillEntry[] | null>(null);
  const [mcp, setMcp] = useState<McpServerEntry[] | null>(null);
  const [sourcesError, setSourcesError] = useState<string | null>(null);

  /** 请求序号：工作区切得快时，旧响应不能覆盖新工作区的状态 */
  const reqSeq = useRef(0);
  /** 工作树清单用独立序号：与状态轮询共用会让彼此把对方的响应作废 */
  const wtSeq = useRef(0);

  const toggleEnv = useCallback(() => setEnvOpen((prev) => !prev), []);

  // ---- 环境信息 ----

  const refreshStatus = useCallback(async () => {
    if (!workingDirectory) {
      setStatus(null);
      return;
    }
    const seq = ++reqSeq.current;
    setLoading(true);
    try {
      const res = await invoke<GitRepoStatus>('git_repo_status', { workingDirectory });
      if (seq !== reqSeq.current) return;
      setStatus(res);
      setRefreshedAt(Date.now());
    } catch (e) {
      if (seq !== reqSeq.current) return;
      setStatus({
        is_repo: false, root: '', branch: '', detached: false, upstream: null,
        ahead: 0, behind: 0, added: 0, removed: 0, changed_files: 0,
        staged: 0, unstaged: 0, untracked: 0, dirty: false,
        last_commit: null, remote_url: null, error: String(e),
      });
    } finally {
      if (seq === reqSeq.current) setLoading(false);
    }
  }, [workingDirectory]);

  // 展开（或切工作区）时拉一次。浮层不可见、或环境信息卡收起时都不必白跑 git。
  useEffect(() => {
    if (visible && envOpen) void refreshStatus();
  }, [visible, envOpen, refreshStatus]);

  // 轮询：只在窗口可见时跑。后台窗口每 8 秒起一次 git 进程纯属浪费。
  useEffect(() => {
    if (!visible || !envOpen || !workingDirectory) return;
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') void refreshStatus();
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [visible, envOpen, workingDirectory, refreshStatus]);

  // 写操作完成后立刻回读，让摘要马上反映新状态
  const runAction = useCallback(
    async (message: string, push: boolean) => {
      setBusy(true);
      setActionResult(null);
      try {
        const res = await invoke<GitActionResult>('git_commit_push', {
          workingDirectory,
          message,
          push,
        });
        setActionResult(res);
        if (res.ok) {
          setCommitMessage('');
          setCommitOpen(false);
        }
        await refreshStatus();
      } catch (e) {
        setActionResult({ ok: false, steps: [], error: String(e) });
      } finally {
        setBusy(false);
      }
    },
    [workingDirectory, refreshStatus],
  );

  /** 提交或推送：确认框里写清「会暂存全部改动」——add -A 的副作用必须让用户先知道 */
  const handleCommit = useCallback(
    async (push: boolean) => {
      const msg = commitMessage.trim();
      if (!msg) return;
      const where = baseName(status?.root || workingDirectory);
      const detail = push
        ? t('mind_inspector.pinned_commit_confirm_push', { msg, where })
        : t('mind_inspector.pinned_commit_confirm', { msg, where });
      const ok = await confirmDialog(detail, {
        title: t('mind_inspector.pinned_commit_title'),
        kind: 'warning',
      }).catch(() => false);
      if (!ok) return;
      await runAction(msg, push);
    },
    [commitMessage, runAction, status, t, workingDirectory],
  );

  // ---- 工作树（隔离执行）----
  //
  // 工作树的用途是隔离执行：在仓库同级另开一处干净副本干活，改坏了整棵丢掉，
  // 不必在脏工作区里做反向操作。这里只管生命周期（新建 / 移除 / 查看），
  // 落盘位置与分支逻辑都在后端 commands::git。

  /** 拉工作树清单。失败时置空数组而非抛错：没有工作树本身不是异常状态。 */
  const refreshWorktrees = useCallback(async () => {
    if (!workingDirectory) {
      setWorktrees([]);
      return;
    }
    const seq = ++wtSeq.current;
    setWorktreesLoading(true);
    try {
      const res = await invoke<GitWorktreeInfo[]>('git_worktree_list', { workingDirectory });
      if (seq !== wtSeq.current) return;
      setWorktrees(res);
      setWorktreeError(null);
    } catch (e) {
      if (seq !== wtSeq.current) return;
      setWorktrees([]);
      setWorktreeError(String(e));
    } finally {
      if (seq === wtSeq.current) setWorktreesLoading(false);
    }
  }, [workingDirectory]);

  // 只在区块展开时拉。工作树探测是「每棵各跑一次 git status」，比状态轮询重，
  // 跟着 8 秒定时器一起跑没有意义。
  useEffect(() => {
    if (visible && worktreesOpen) void refreshWorktrees();
  }, [visible, worktreesOpen, refreshWorktrees]);

  // 换工作区就作废上一次的清单，否则会显示成别的仓库的工作树。
  useEffect(() => {
    setWorktrees(null);
    setWorktreeError(null);
  }, [workingDirectory]);

  /** 新建隔离工作树。确认框写清落盘位置，免得建完不知道东西去哪了。 */
  const handleAddWorktree = useCallback(async () => {
    const name = newWorktreeName.trim();
    if (!name) return;
    const where = baseName(status?.root || workingDirectory);
    const ok = await confirmDialog(
      t('mind_inspector.pinned_worktree_add_confirm', { name, where }),
      { title: t('mind_inspector.pinned_worktree_add_title'), kind: 'info' },
    ).catch(() => false);
    if (!ok) return;
    setWorktreeBusy(true);
    setWorktreeError(null);
    try {
      const res = await invoke<GitActionResult>('git_worktree_add', {
        workingDirectory,
        slug: name,
        base: '',
      });
      if (res.ok) {
        setNewWorktreeName('');
        await refreshWorktrees();
      } else {
        setWorktreeError(res.error || null);
      }
    } catch (e) {
      setWorktreeError(String(e));
    } finally {
      setWorktreeBusy(false);
    }
  }, [newWorktreeName, status, workingDirectory, refreshWorktrees, t]);

  /**
   * 移除工作树。
   *
   * 一律先弹确认：工作树里可能是做了一半的活或游离提交，不该被一次误触丢掉。
   * 措辞按脏净分轻重，脏的连改动规模一并说清。
   */
  const handleRemoveWorktree = useCallback(
    async (wt: GitWorktreeInfo) => {
      setWorktreeBusy(true);
      setWorktreeError(null);
      try {
        const call = (force: boolean) =>
          invoke<GitActionResult>('git_worktree_remove', {
            workingDirectory,
            path: wt.path,
            force,
          });

        // 一次确认就够：脏净判断取自已拉取的清单，措辞按它分轻重。
        // force 跟着这次判断走；后端在 force=false 时仍会挡住脏工作树，
        // 万一清单快照过期，由它兜底。
        const ok = await confirmDialog(
          wt.dirty
            ? t('mind_inspector.pinned_worktree_dirty_confirm', {
                name: wt.name,
                n: wt.changed_files,
              })
            : t('mind_inspector.pinned_worktree_remove_confirm', { name: wt.name }),
          { title: t('mind_inspector.pinned_worktree_remove'), kind: 'warning' },
        ).catch(() => false);
        if (!ok) return;

        const res = await call(wt.dirty);
        if (!res.ok) {
          setWorktreeError(res.error || null);
          return;
        }
        await refreshWorktrees();
        await refreshStatus();
      } catch (e) {
        setWorktreeError(String(e));
      } finally {
        setWorktreeBusy(false);
      }
    },
    [workingDirectory, refreshWorktrees, refreshStatus, t],
  );

  /** 展开「比较分支」：拉分支清单并默认选中基准分支 */
  const openCompare = useCallback(async () => {
    const next = !compareOpen;
    setCompareOpen(next);
    if (!next || !workingDirectory) return;
    setDiff(null);
    try {
      const list = await invoke<GitBranchList>('git_list_branches', { workingDirectory });
      setBranches(list);
      const base = compareBase || list.default_base || list.remote[0] || list.local[0] || '';
      setCompareBase(base);
      if (base) {
        setDiffLoading(true);
        const d = await invoke<GitBranchDiff>('git_branch_diff', { workingDirectory, base });
        setDiff(d);
      }
    } catch (e) {
      setDiff({ base: '', current: '', ahead: 0, behind: 0, added: 0, removed: 0, commits: [], error: String(e) });
    } finally {
      setDiffLoading(false);
    }
  }, [compareBase, compareOpen, workingDirectory]);

  const pickBase = useCallback(
    async (base: string) => {
      setCompareBase(base);
      if (!base) return;
      setDiffLoading(true);
      try {
        setDiff(await invoke<GitBranchDiff>('git_branch_diff', { workingDirectory, base }));
      } catch (e) {
        setDiff({ base, current: '', ahead: 0, behind: 0, added: 0, removed: 0, commits: [], error: String(e) });
      } finally {
        setDiffLoading(false);
      }
    },
    [workingDirectory],
  );

  // ---- 来源 ----

  const loadSources = useCallback(async () => {
    try {
      const [p, s, m] = await Promise.all([
        invoke<PluginEntry[]>('list_plugins'),
        invoke<SkillEntry[]>('list_skills'),
        invoke<McpServerEntry[]>('list_mcp_servers'),
      ]);
      setPlugins(p);
      setSkills(s);
      setMcp(m);
      setSourcesError(null);
    } catch (e) {
      setSourcesError(String(e));
    }
  }, []);

  // 挂载时先取一次，折叠态下也要能显示计数徽章
  useEffect(() => {
    void loadSources();
  }, [loadSources]);

  /** 已装载（status=loaded）的插件数，跳过的插件不该计入「已连接」 */
  const loadedPlugins = useMemo(
    () => (plugins ?? []).filter((p) => p.status === 'loaded'),
    [plugins],
  );
  const aliveMcp = useMemo(() => (mcp ?? []).filter((m) => m.alive), [mcp]);

  const hasWorkspace = Boolean(workingDirectory);
  const isRepo = Boolean(status?.is_repo);
  const dirty = Boolean(status?.dirty);

  // ============ 渲染 ============

  // 不随收起卸载：卸载掉就没有元素可做宽度动画了（和两侧边栏同一个理由）。
  // 收起态由 CSS 负责——横向宽度归零、纵向可见区归零、退出 tab 序列。
  return (
    <div
      className={`codex-pinned${visible ? '' : ' collapsed'}`}
      aria-hidden={!visible}
    >
      {/* 内层固定为展开宽度并靠右对齐，裁切交给外层：横向由 `overflow:hidden`，
          纵向由 `clip-path` 从上往下揭开 / 从下往上收掉。所以动画期间内容是
          「被一块从右上角扩张的窗口逐步揭开」——既不逐帧压窄重排（宽度归零时
          文字会算出 0 宽），也不横向平移（左对齐的话整块会跟着左移 250px，
          看起来像从右边滑进来）。
          外层与内层的宽度都来自祖先 `.codex-main-col` 上的 `--codex-pinned-w`
          与 `--codex-pinned-w-expanded`（见 CodeAgentPage.css），组件自己不持有
          宽度——宽度策略只该有一个出处，就在父组件那三个 PINNED_W_* 常量里。 */}
      <div className="codex-pinned-inner">
        {/* ---------- 环境信息 ---------- */}
        <section className="codex-pinned-card">
          <button
            type="button"
            className="codex-pinned-head"
            onClick={toggleEnv}
            title={envOpen
              ? t('mind_inspector.pinned_collapse')
              : t('mind_inspector.pinned_expand')}
            aria-expanded={envOpen}
          >
            {envOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
            <span className="codex-pinned-head-title">
              {t('mind_inspector.pinned_env_info')}
            </span>
            {/* 卡内收起时用一个圆点提示「有未提交改动」，否则收起来就彻底失明了 */}
            {!envOpen && dirty ? <span className="codex-pinned-dot" aria-hidden /> : null}
          </button>

          {envOpen && (
            <div className="codex-pinned-body">
              {!hasWorkspace ? (
                <div className="codex-pinned-hint">
                  <TriangleAlert size={13} />
                  {t('mind_inspector.pinned_no_workspace')}
                </div>
              ) : status?.error ? (
                <div className="codex-pinned-hint err">
                  <TriangleAlert size={13} />
                  {status.error}
                </div>
              ) : !isRepo ? (
                <div className="codex-pinned-hint">
                  <FolderGit2 size={13} />
                  {t('mind_inspector.pinned_not_repo')}
                </div>
              ) : (
                <>
                  {/* 变更：+N -M 与改动文件数 */}
                  <div className="codex-pinned-row">
                    <span className="codex-pinned-key">
                      <GitCompare size={12} />
                      {t('mind_inspector.pinned_changes')}
                    </span>
                    <span className="codex-pinned-val">
                      {dirty ? (
                        <>
                          <em className="codex-diff-add">+{formatCount(status!.added)}</em>
                          <em className="codex-diff-del">-{formatCount(status!.removed)}</em>
                          <span className="codex-pinned-muted">
                            {t('mind_inspector.pinned_files', { n: status!.changed_files })}
                          </span>
                        </>
                      ) : (
                        <span className="codex-pinned-muted codex-pinned-clean">
                          <Check size={12} />
                          {t('mind_inspector.pinned_clean')}
                        </span>
                      )}
                    </span>
                  </div>

                  {/* 本地：仓库目录 */}
                  <div className="codex-pinned-row">
                    <span className="codex-pinned-key">
                      <FolderGit2 size={12} />
                      {t('mind_inspector.pinned_local')}
                    </span>
                    <span className="codex-pinned-val mono" title={status!.root}>
                      {baseName(status!.root)}
                    </span>
                  </div>

                  {/* 分支 */}
                  <div className="codex-pinned-row">
                    <span className="codex-pinned-key">
                      <GitBranch size={12} />
                      {t('mind_inspector.pinned_branch')}
                    </span>
                    <span className="codex-pinned-val mono" title={status!.branch || 'detached HEAD'}>
                      {status!.branch || t('mind_inspector.pinned_detached')}
                    </span>
                  </div>

                  {/* 与远端的同步状态：没有 PR 集成，就不假装有；这里如实说上游情况 */}
                  <div className="codex-pinned-row">
                    <span className="codex-pinned-key">
                      <Upload size={12} />
                      {t('mind_inspector.pinned_sync')}
                    </span>
                    <span className="codex-pinned-val">
                      {!status!.upstream ? (
                        <span className="codex-pinned-muted">
                          {t('mind_inspector.pinned_no_upstream')}
                        </span>
                      ) : (
                        <>
                          {status!.ahead > 0 && (
                            <em className="codex-pinned-ahead">
                              {t('mind_inspector.pinned_ahead', { n: status!.ahead })}
                            </em>
                          )}
                          {status!.behind > 0 && (
                            <em className="codex-pinned-behind">
                              {t('mind_inspector.pinned_behind', { n: status!.behind })}
                            </em>
                          )}
                          {status!.ahead === 0 && status!.behind === 0 && (
                            <span className="codex-pinned-muted codex-pinned-clean">
                              <Check size={12} />
                              {t('mind_inspector.pinned_in_sync')}
                            </span>
                          )}
                        </>
                      )}
                    </span>
                  </div>

                  {/* 最近提交 */}
                  {status!.last_commit && (
                    <div className="codex-pinned-commit" title={status!.last_commit.subject}>
                      <span className="codex-pinned-hash">{status!.last_commit.hash}</span>
                      <span className="codex-pinned-subject">{status!.last_commit.subject}</span>
                      <span className="codex-pinned-time">{shortTime(status!.last_commit.time)}</span>
                    </div>
                  )}

                  {/* 操作行 */}
                  <div className="codex-pinned-actions">
                    <button
                      type="button"
                      className={`codex-pinned-btn${dirty ? ' primary' : ''}`}
                      onClick={() => { setCommitOpen((v) => !v); setCompareOpen(false); setActionResult(null); }}
                      disabled={!dirty || busy}
                      title={dirty
                        ? t('mind_inspector.pinned_commit_hint')
                        : t('mind_inspector.pinned_nothing_to_commit')}
                    >
                      <Upload size={12} />
                      {t('mind_inspector.pinned_commit_or_push')}
                    </button>
                    <button
                      type="button"
                      className={`codex-pinned-btn${compareOpen ? ' active' : ''}`}
                      onClick={() => void openCompare()}
                    >
                      <GitCompare size={12} />
                      {t('mind_inspector.pinned_compare')}
                    </button>
                    <button
                      type="button"
                      className="codex-pinned-icon-btn"
                      onClick={() => void refreshStatus()}
                      title={t('mind_inspector.pinned_refresh')}
                    >
                      <RefreshCw size={12} className={loading ? 'codex-spin' : undefined} />
                    </button>
                  </div>

                  {/* 提交或推送：内联小表单 */}
                  {commitOpen && (
                    <div className="codex-pinned-form">
                      <input
                        className="codex-pinned-input"
                        value={commitMessage}
                        onChange={(e) => setCommitMessage(e.target.value)}
                        placeholder={t('mind_inspector.pinned_commit_placeholder')}
                        disabled={busy}
                        onKeyDown={(e) => {
                          if (e.key === 'Enter' && !e.shiftKey) {
                            e.preventDefault();
                            void handleCommit(false);
                          }
                        }}
                      />
                      <div className="codex-pinned-form-row">
                        <button
                          type="button"
                          className="codex-pinned-btn"
                          disabled={busy || !commitMessage.trim()}
                          onClick={() => void handleCommit(false)}
                        >
                          {busy ? <Loader2 size={12} className="codex-spin" /> : <Check size={12} />}
                          {t('mind_inspector.pinned_commit')}
                        </button>
                        <button
                          type="button"
                          className="codex-pinned-btn primary"
                          disabled={busy || !commitMessage.trim()}
                          onClick={() => void handleCommit(true)}
                        >
                          {busy ? <Loader2 size={12} className="codex-spin" /> : <Upload size={12} />}
                          {t('mind_inspector.pinned_commit_and_push')}
                        </button>
                      </div>
                    </div>
                  )}

                  {/* 比较分支：基准选择 + 差异摘要 */}
                  {compareOpen && (
                    <div className="codex-pinned-form">
                      <select
                        className="codex-pinned-select"
                        value={compareBase}
                        onChange={(e) => void pickBase(e.target.value)}
                      >
                        <option value="">{t('mind_inspector.pinned_pick_base')}</option>
                        {branches && branches.remote.length > 0 && (
                          <optgroup label={t('mind_inspector.pinned_remote_branches')}>
                            {branches.remote.map((b) => <option key={b} value={b}>{b}</option>)}
                          </optgroup>
                        )}
                        {branches && branches.local.length > 0 && (
                          <optgroup label={t('mind_inspector.pinned_local_branches')}>
                            {branches.local.map((b) => <option key={b} value={b}>{b}</option>)}
                          </optgroup>
                        )}
                      </select>
                      {diffLoading ? (
                        <div className="codex-pinned-hint">
                          <Loader2 size={12} className="codex-spin" />
                          {t('mind_inspector.pinned_comparing')}
                        </div>
                      ) : diff?.error ? (
                        <div className="codex-pinned-hint err">
                          <TriangleAlert size={12} />
                          {diff.error}
                        </div>
                      ) : diff ? (
                        <div className="codex-pinned-diff">
                          <div className="codex-pinned-diff-line">
                            {t('mind_inspector.pinned_diff_summary', {
                              ahead: diff.ahead,
                              behind: diff.behind,
                            })}
                            <em className="codex-diff-add">+{formatCount(diff.added)}</em>
                            <em className="codex-diff-del">-{formatCount(diff.removed)}</em>
                          </div>
                          {diff.commits.slice(0, 5).map((c) => (
                            <div key={c.hash} className="codex-pinned-diff-commit" title={c.subject}>
                              <span className="codex-pinned-hash">{c.hash}</span>
                              <span className="codex-pinned-subject">{c.subject}</span>
                            </div>
                          ))}
                          {diff.commits.length > 5 && (
                            <div className="codex-pinned-muted">
                              {t('mind_inspector.pinned_more_commits', { n: diff.commits.length - 5 })}
                            </div>
                          )}
                        </div>
                      ) : null}
                    </div>
                  )}

                  {/* 工作树：隔离执行的入口与清单 */}
                  <div className="codex-pinned-sub">
                    <button
                      type="button"
                      className="codex-pinned-subhead"
                      onClick={() => setWorktreesOpen((v) => !v)}
                      aria-expanded={worktreesOpen}
                    >
                      {worktreesOpen ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
                      <FolderGit2 size={12} />
                      <span className="codex-pinned-sub-title">
                        {t('mind_inspector.pinned_worktrees')}
                      </span>
                      {worktrees && worktrees.length > 0 && (
                        <span className="codex-pinned-muted">{worktrees.length}</span>
                      )}
                    </button>

                    {worktreesOpen && (
                      <div className="codex-pinned-sub-body">
                        {worktreesLoading && !worktrees ? (
                          <div className="codex-pinned-hint">
                            <Loader2 size={12} className="codex-spin" />
                            {t('mind_inspector.pinned_loading')}
                          </div>
                        ) : (
                          <>
                            {(worktrees ?? []).map((wt) => (
                              <div key={wt.path} className="codex-pinned-wt">
                                <div className="codex-pinned-wt-row">
                                  <span className="codex-pinned-wt-name" title={wt.path}>
                                    {wt.name}
                                  </span>
                                  {wt.is_main && (
                                    <span className="codex-pinned-wt-tag">
                                      {t('mind_inspector.pinned_worktree_main')}
                                    </span>
                                  )}
                                  {wt.dirty && (
                                    <span className="codex-pinned-wt-tag warn">
                                      {t('mind_inspector.pinned_worktree_changed', {
                                        n: wt.changed_files,
                                      })}
                                    </span>
                                  )}
                                  {!wt.is_main && (
                                    <button
                                      type="button"
                                      className="codex-pinned-icon-btn"
                                      onClick={() => void handleRemoveWorktree(wt)}
                                      disabled={worktreeBusy}
                                      title={t('mind_inspector.pinned_worktree_remove')}
                                    >
                                      <Trash2 size={11} />
                                    </button>
                                  )}
                                </div>
                                <div className="codex-pinned-wt-meta mono">
                                  {wt.detached
                                    ? t('mind_inspector.pinned_detached')
                                    : wt.branch || wt.head}
                                </div>
                              </div>
                            ))}

                            {worktrees && worktrees.length === 0 && (
                              <div className="codex-pinned-hint">
                                {t('mind_inspector.pinned_worktree_none')}
                              </div>
                            )}

                            <div className="codex-pinned-form-row">
                              <input
                                className="codex-pinned-input"
                                value={newWorktreeName}
                                onChange={(e) => setNewWorktreeName(e.target.value)}
                                placeholder={t('mind_inspector.pinned_worktree_placeholder')}
                                disabled={worktreeBusy}
                                onKeyDown={(e) => {
                                  if (e.key === 'Enter') {
                                    e.preventDefault();
                                    void handleAddWorktree();
                                  }
                                }}
                              />
                              <button
                                type="button"
                                className="codex-pinned-btn"
                                disabled={worktreeBusy || !newWorktreeName.trim()}
                                onClick={() => void handleAddWorktree()}
                              >
                                {worktreeBusy
                                  ? <Loader2 size={12} className="codex-spin" />
                                  : <Plus size={12} />}
                                {t('mind_inspector.pinned_worktree_add')}
                              </button>
                            </div>

                            {worktreeError && (
                              <div className="codex-pinned-hint err">
                                <TriangleAlert size={12} />
                                {worktreeError}
                              </div>
                            )}
                          </>
                        )}
                      </div>
                    )}
                  </div>

                  {/* 写操作结果：逐条步骤 + 失败原因 */}
                  {actionResult && (
                    <div className={`codex-pinned-result${actionResult.ok ? '' : ' err'}`}>
                      {actionResult.steps.map((s) => (
                        <div key={s.step} className="codex-pinned-result-step">
                          {s.ok ? <Check size={11} /> : <TriangleAlert size={11} />}
                          <span className="codex-pinned-result-name">{s.step}</span>
                          {s.ok && s.output
                            ? <span className="codex-pinned-result-out">{s.output.split('\n')[0]}</span>
                            : null}
                        </div>
                      ))}
                      {actionResult.error && (
                        <div className="codex-pinned-result-err">{actionResult.error}</div>
                      )}
                    </div>
                  )}

                  {refreshedAt > 0 && (
                    <div className="codex-pinned-foot">
                      {t('mind_inspector.pinned_updated_at', { time: shortTime(new Date(refreshedAt).toISOString()) })}
                    </div>
                  )}
                </>
              )}
            </div>
          )}
        </section>

        {/* ---------- 来源 ---------- */}
        <section className="codex-pinned-card">
          <button
            type="button"
            className="codex-pinned-head"
            onClick={() => setSourcesOpen((v) => !v)}
            aria-expanded={sourcesOpen}
          >
            {sourcesOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
            <span className="codex-pinned-head-title">
              {t('mind_inspector.pinned_sources')}
            </span>
          </button>

          {sourcesOpen && (
            <div className="codex-pinned-body">
              {sourcesError ? (
                <div className="codex-pinned-hint err">
                  <TriangleAlert size={13} />
                  {sourcesError}
                </div>
              ) : (
                <>
                  <div className="codex-pinned-row">
                    <span className="codex-pinned-key">
                      <Plug size={12} />
                      {t('mind_inspector.pinned_plugins')}
                    </span>
                    <span className="codex-pinned-val mono">
                      {loadedPlugins.length}
                      {plugins && plugins.length !== loadedPlugins.length
                        ? <span className="codex-pinned-muted"> / {plugins.length}</span>
                        : null}
                    </span>
                  </div>
                  <div className="codex-pinned-row">
                    <span className="codex-pinned-key">
                      <Sparkles size={12} />
                      {t('mind_inspector.pinned_skills')}
                    </span>
                    <span className="codex-pinned-val mono">{skills?.length ?? 0}</span>
                  </div>
                  <div className="codex-pinned-row">
                    <span className="codex-pinned-key">
                      <Link2 size={12} />
                      {t('mind_inspector.pinned_mcp')}
                    </span>
                    <span className="codex-pinned-val mono">
                      {aliveMcp.length}
                      {mcp && mcp.length !== aliveMcp.length
                        ? <span className="codex-pinned-muted"> / {mcp.length}</span>
                        : null}
                    </span>
                  </div>

                  {/* 插件明细：信任状态是这里最该被看见的信息——
                       untrusted / changed 意味着它当前不生效或需要重新确认 */}
                  {loadedPlugins.length > 0 && (
                    <div className="codex-pinned-list">
                      {loadedPlugins.slice(0, 8).map((p) => (
                        <div key={p.key} className="codex-pinned-list-item" title={p.dir}>
                          <span className={`codex-trust-dot ${p.trust}`} aria-hidden />
                          <span className="codex-pinned-list-name">{p.name}</span>
                          <span className="codex-pinned-muted">
                            {p.skills.length + p.tools.length}
                          </span>
                        </div>
                      ))}
                      {loadedPlugins.length > 8 && (
                        <div className="codex-pinned-muted">
                          {t('mind_inspector.pinned_more_plugins', { n: loadedPlugins.length - 8 })}
                        </div>
                      )}
                    </div>
                  )}

                  <div className="codex-pinned-actions">
                    <button
                      type="button"
                      className="codex-pinned-icon-btn"
                      onClick={() => void loadSources()}
                      title={t('mind_inspector.pinned_refresh')}
                    >
                      <RefreshCw size={12} />
                    </button>
                  </div>
                </>
              )}
            </div>
          )}
        </section>
      </div>
    </div>
  );
};

export default PinnedSummary;
