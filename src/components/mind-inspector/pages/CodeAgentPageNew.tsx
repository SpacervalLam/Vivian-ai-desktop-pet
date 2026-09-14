/**
 * CodeAgentPage — Codex 布局 + 手账风格
 *
 * - 布局仿 Codex：左侧任务会话栏 / 中央对话流 / 右侧检查器（概览 + 终端）
 * - 视觉：暖纸 + 点阵底纹 + 纸胶带 + 便签 + 线格输入区
 * - 后端连接保持 coding_* 命令与 coding:* 事件流（发送/取消/模式/权限/模型/推理/工作区）
 */

import React, { useState, useEffect, useLayoutEffect, useRef, useCallback, useMemo, lazy, Suspense } from 'react';
import { useTranslation } from 'react-i18next';
import { listen, emit, type UnlistenFn } from '@tauri-apps/api/event';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { open as openDialog, confirm as confirmDialog } from '@tauri-apps/plugin-dialog';
import { raiseWindow } from '../../../utils/windowRaiser';
import {
  Plus, Trash2, ChevronDown, ChevronRight, Loader2,
  FileText, FilePlus, FileEdit, Terminal as TerminalIcon, Search, FolderTree,
  Wrench, FolderOpen, Braces, XCircle, X, Image as ImageIcon,
  Folder, Lock, Shield, Sparkles, Check, Cpu, Zap, Code2,
  Activity, Send, Square, ArrowDown, ArrowUp, List,
  PanelLeftClose, PanelLeftOpen, PanelRightClose, PanelRightOpen, SlidersHorizontal, Ellipsis,
  Goal, ClipboardList, History, MessageSquare, Download, Copy, ThumbsUp, ThumbsDown, GitFork, Mic, Brain,
  HelpCircle, FileDiff, FileCode, Palette, Globe, Settings, File as FileIcon, Info,
} from 'lucide-react';
import './CodeAgentPage.css';
import TrajectoryPanel from './TrajectoryPanel';
import { MarkdownFileContext, MarkdownText, FileChip } from './codeMarkdown';
import { SourceFileView } from './SourceFileView';
import DOMPurify from 'dompurify';

const TerminalPanel = lazy(() => import('./TerminalPanel'));

/**
 * 把 markdown 文件链接补成绝对路径。
 *
 * 绝对路径（盘符 / UNC / 以 / 开头）原样返回；相对路径拼到会话工作区下。
 * 没有工作区时只能原样返回，交由预览面板报错——比静默丢弃更容易排查。
 */
function resolveWorkspacePath(path: string, cwd?: string): string {
  const p = path.trim();
  if (/^[A-Za-z]:[\\/]/.test(p) || p.startsWith('\\\\') || p.startsWith('/')) return p;
  if (!cwd) return p;
  const base = cwd.replace(/[\\/]+$/, '');
  return `${base}/${p.replace(/^\.\//, '')}`;
}

// ============ 类型 ============

/**
 * 消息角色。
 *
 * `notice` 是宿主自己产生的**中性状态消息**（上下文压缩等），与 `error` 分开：
 * 后者代表执行失败、渲染成红色警示，前者只是提示，混用会把正常状态读成故障。
 */
type CodingRole = 'user' | 'assistant' | 'tool_use' | 'tool_result' | 'error' | 'notice';

interface CodingMessage {
  role: CodingRole;
  content: string;
  images?: CodingImageView[] | null;
  file_refs?: CodingFileRef[] | null;
  /** show_widget 工具渲染的可视化组件（原始 SVG，随消息持久化） */
  widgets?: CodingWidget[] | null;
  /** 任务执行期间排队的插话（后端构建 LLM 上下文时加插话标注） */
  interjected?: boolean | null;
  /** 任务执行期间用户给出的引导（后端构建 LLM 上下文时加引导标注） */
  guided?: boolean | null;
  tool_name?: string | null;
  tool_arguments?: unknown;
  tool_success?: boolean | null;
  tool_call_id?: string | null;
  tool_duration_ms?: number | null;
  timestamp: number;
}

/** 用户消息附带的文件引用（@-mention 注入上下文）。 */
interface CodingFileRef {
  path: string;
  content?: string | null;
  error?: string | null;
}

interface CodingTokenUsage {
  input_tokens: number;
  output_tokens: number;
  cache_read_tokens: number;
  cache_write_tokens: number;
}

interface CodingStats {
  turns: number;
  steps: number;
  llm_ms: number;
  tool_ms: number;
  usage: CodingTokenUsage;
  first_token_ms?: number;
  first_token_calls?: number;
}

/** 会话内一条文件变更（按文件聚合：累计增删行数 + 最近一次变更的 unified diff）。 */
interface FileChange {
  path: string;
  added: number;
  removed: number;
  diff: string;
}

/** 本轮改动清单项（宿主按工具执行结果生成，前端渲染成回复末尾的「修改文件」列表）。 */
interface CodingFileChangeView {
  /** 相对工作目录的路径（不在工作目录内时为绝对路径） */
  path: string;
  added: number;
  removed: number;
  /** 首个变更行号；整文件写入没有 diff 时为 undefined */
  line?: number;
}

interface CodingSession {
  session_id: string;
  char_id: string;
  working_directory: string;
  title: string;
  mode: string;
  permission?: 'read_only' | 'workspace_write' | 'full_access' | string;
  model_id?: string | null;
  reasoning_level?: 'low' | 'medium' | 'high' | string;
  messages: CodingMessage[];
  status: 'idle' | 'running' | 'canceled';
  updated_at: number;
  /** 会话目标（/goal 设置） */
  goal?: string | null;
  /** 计划模式开关（/plan） */
  plan_mode?: boolean;
  /** 已批准执行方案（/plan approve 固化） */
  plan?: string | null;
  /** 单条消息级反馈（消息下标 → "up" / "down"） */
  message_feedback?: Record<number, string>;
  /**
   * 单条助手回复由宿主记录的本轮改动清单（消息下标 → 改动项）。
   *
   * 与 `content` 分开存放：这是界面元数据，不进 LLM 上下文；也不依赖模型在
   * 正文里自述「修改文件」小节——工具执行成功的那一刻宿主就知道改了什么。
   */
  message_changes?: Record<number, CodingFileChangeView[]>;
  stats?: CodingStats | null;
  /** 会话产物文件（write_file / edit_file 成功写入的路径） */
  deliverables?: string[];
  /** 会话内文件变更清单（变更页展示） */
  file_changes?: FileChange[];
  /** 上下文窗口（tokens，自动压缩阈值基准） */
  context_window?: number;
  /** 最近一次 LLM 请求的真实上下文规模（输入侧 token：input + cache_read + cache_write） */
  last_context_tokens?: number;
  /** 最近一次请求的上下文构成估算（[system, 工具定义, 对话消息] tokens） */
  last_context_breakdown?: [number, number, number];
}

interface FileNode {
  name: string;
  path: string;
  is_dir: boolean;
  children?: FileNode[];
}

// ============ 工具 Hook ============

/**
 * 横向滚动容器滚轮转水平滚动：把滚轮的纵向 deltaY 折算到 scrollLeft，
 * 并阻止页面纵向滚动（方便横向页签栏用滚轮左右滑动）。
 */
function useWheelHorizontalScroll<T extends HTMLElement>(): React.RefObject<T> {
  const ref = useRef<T>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (Math.abs(e.deltaY) <= 0 && Math.abs(e.deltaX) <= 0) return;
      const dx = e.deltaX || e.deltaY;
      const max = el.scrollWidth - el.clientWidth;
      if (max <= 0) return;
      const next = Math.min(Math.max(el.scrollLeft + dx, 0), max);
      if (next !== el.scrollLeft) {
        el.scrollLeft = next;
        e.preventDefault();
      }
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, []);
  return ref;
}

// ============ 常量 ============

const MODES: Array<{ key: 'standard' | 'code' | 'minimal'; label: string; hint: string }> = [
  { key: 'standard', label: '标准模式', hint: '功能完整的编码 Agent，支持文件编辑、Shell、文件与网页检索、Skills、计划、目标、子代理和工作流。' },
  { key: 'code', label: '代码模式', hint: '通过 Code Mode SDK 呈现工具，让模型用一个 TypeScript 程序组合多步操作。' },
  { key: 'minimal', label: '极简模式', hint: '仅提供持久 bash 与 str_replace_editor 的双工具编码 Agent。' },
];

const PERMISSIONS: Array<{ key: 'read_only' | 'workspace_write' | 'full_access'; label: string; icon: React.ReactNode; hint: string }> = [
  { key: 'read_only', label: 'Read Only', icon: <Lock size={14} />, hint: '只读权限' },
  { key: 'workspace_write', label: 'Workspace Write', icon: <Shield size={14} />, hint: '工作区写入' },
  { key: 'full_access', label: 'Full access', icon: <Sparkles size={14} />, hint: '完全访问' },
];

const REASONING_LEVELS: Array<{ key: 'low' | 'medium' | 'high'; label: string }> = [
  { key: 'low', label: 'Low' },
  { key: 'medium', label: 'Medium' },
  { key: 'high', label: 'High' },
];

interface SlashCommand {
  cmd: string;
  label: string;
  description: string;
  icon: React.ReactNode;
}

// 斜杠命令集：goal / plan / compact / memory / permission / feedback / export
const SLASH_COMMANDS: SlashCommand[] = [
  { cmd: '/goal', label: '设置目标', description: '为长期任务设置或查看目标', icon: <Goal size={14} /> },
  { cmd: '/plan', label: '计划模式', description: '进入或退出计划模式', icon: <ClipboardList size={14} /> },
  { cmd: '/compact', label: '压缩历史', description: '压缩较早的对话历史（同时沉淀项目记忆）', icon: <History size={14} /> },
  { cmd: '/memory', label: '项目记忆', description: '查看/提炼/追加跨会话的项目记忆（应用数据目录）', icon: <Brain size={14} /> },
  { cmd: '/permission', label: '切换权限', description: '切换权限预设（沙箱模式 + 审批策略）', icon: <Shield size={14} /> },
  { cmd: '/feedback', label: '反馈', description: '记录关于本次会话的反馈', icon: <MessageSquare size={14} /> },
  { cmd: '/export', label: '导出会话', description: '将会话日志下载为 ZIP 压缩包', icon: <Download size={14} /> },
];

// ============ 斜杠命令模糊匹配 ============

/** 边界加分：命令名开头或 -/_ 分隔符之后优先 */
function boundaryBonus(name: string, index: number): number {
  return index === 0 || name.charAt(index - 1) === '-' || name.charAt(index - 1) === '_' ? 8 : 0;
}

/** 有序子序列模糊评分，无匹配返回 undefined */
function fuzzyScore(name: string, query: string): number | undefined {
  if (query === '') return 0;
  if (query.length > name.length) return undefined;
  const noMatch = Number.NEGATIVE_INFINITY;
  let previous = Array<number>(name.length).fill(noMatch);
  for (let index = 0; index < name.length; index++) {
    if (name.charAt(index) === query.charAt(0)) previous[index] = 1 + boundaryBonus(name, index) - index;
  }
  for (let queryIndex = 1; queryIndex < query.length; queryIndex++) {
    const current = Array<number>(name.length).fill(noMatch);
    let bestGapped = noMatch;
    for (let index = 0; index < name.length; index++) {
      const gappedIndex = index - 2;
      if (gappedIndex >= 0) {
        const prior = previous[gappedIndex] ?? noMatch;
        if (prior !== noMatch) bestGapped = Math.max(bestGapped, prior + gappedIndex);
      }
      if (name.charAt(index) !== query.charAt(queryIndex)) continue;
      const bonus = 1 + boundaryBonus(name, index);
      const adjacent = index > 0 ? previous[index - 1] ?? noMatch : noMatch;
      if (adjacent !== noMatch) current[index] = adjacent + bonus + 4;
      if (bestGapped !== noMatch) current[index] = Math.max(current[index] ?? noMatch, bestGapped + bonus + 1 - index);
    }
    previous = current;
  }
  let best = noMatch;
  for (const score of previous) best = Math.max(best, score);
  return best === noMatch ? undefined : best;
}

/** 按输入字母模糊筛选斜杠命令：前缀优先，其次评分，最后目录顺序 */
function filterSlashCommands(query: string): SlashCommand[] {
  const q = query.toLowerCase();
  if (q === '') return SLASH_COMMANDS;
  const ranked: { cmd: SlashCommand; prefix: boolean; score: number; index: number }[] = [];
  SLASH_COMMANDS.forEach((cmd, index) => {
    const name = cmd.cmd.slice(1).toLowerCase();
    const label = cmd.label.toLowerCase();
    const nameScore = fuzzyScore(name, q);
    const labelScore = fuzzyScore(label, q);
    const score = nameScore ?? labelScore;
    if (score === undefined) return;
    ranked.push({
      cmd,
      prefix: name.startsWith(q),
      // 命令名命中加权，优先于中文标签命中
      score: score + (nameScore !== undefined ? 100 : 0),
      index,
    });
  });
  ranked.sort((a, b) =>
    Number(b.prefix) - Number(a.prefix) || b.score - a.score || a.index - b.index);
  return ranked.map((r) => r.cmd);
}

const TOOL_META: Record<string, { icon: React.ReactNode; label: string }> = {
  read_file: { icon: <FileText size={14} />, label: '读取文件' },
  write_file: { icon: <FilePlus size={14} />, label: '写入文件' },
  edit_file: { icon: <FileEdit size={14} />, label: '编辑文件' },
  run_command: { icon: <TerminalIcon size={14} />, label: '执行命令' },
  grep_search: { icon: <Search size={14} />, label: '搜索代码' },
  list_dir: { icon: <FolderTree size={14} />, label: '目录结构' },
  compose_program: { icon: <Braces size={14} />, label: '组合程序' },
};

function toolMeta(name: string) {
  return TOOL_META[name] ?? { icon: <Wrench size={14} />, label: name };
}

// ============ 格式化 ============

function formatTokens(n: number): string {
  const scaled = (v: number): string =>
    v >= 100 ? String(Math.round(v)) : String(Math.round(v * 10) / 10);
  if (n < 1_000) return String(n);
  if (n < 1_000_000) return `${scaled(n / 1_000)}K`;
  return `${scaled(n / 1_000_000)}M`;
}

function formatDuration(ms: number): string {
  const s = ms / 1_000;
  if (s < 60) return `${Math.round(s * 10) / 10}s`;
  const whole = Math.round(s);
  return `${Math.floor(whole / 60)}m${whole % 60}s`;
}

function formatClock(ms: number): string {
  const s = Math.floor(ms / 1_000);
  const m = Math.floor(s / 60);
  return m > 0 ? `${m}m${String(s % 60).padStart(2, '0')}s` : `${s}s`;
}

function cacheHitPercent(u: CodingTokenUsage): number | null {
  const denom = u.input_tokens + u.cache_read_tokens + u.cache_write_tokens;
  return denom === 0 ? null : Math.round((u.cache_read_tokens / denom) * 100);
}

function diffStats(text: string): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const l of text.split('\n')) {
    if (l.startsWith('+++')) continue;
    if (l.startsWith('---')) continue;
    if (l.startsWith('+')) added += 1;
    else if (l.startsWith('-')) removed += 1;
  }
  return { added, removed };
}

// ============ 会话统计行 ============

const StatsLine: React.FC<{ stats?: CodingStats | null }> = ({ stats }) => {
  const { t } = useTranslation();
  if (!stats) return null;
  const groups: string[] = [];
  if (stats.steps > 0) {
    groups.push(t('mind_inspector.code_stats_turns', { turns: stats.turns, steps: stats.steps }));
    const durations: string[] = [];
    if (stats.llm_ms > 0) durations.push(`LLM ${formatDuration(stats.llm_ms)}`);
    if (stats.tool_ms > 0) durations.push(t('mind_inspector.code_stats_tool_time', { d: formatDuration(stats.tool_ms) }));
    if (durations.length > 0) groups.push(durations.join(' · '));
  }
  const fTokenMs = stats.first_token_ms ?? 0;
  const fTokenCalls = stats.first_token_calls ?? 0;
  if (fTokenCalls > 0 && fTokenMs > 0) {
    const avgFirst = fTokenMs / fTokenCalls / 1000;
    const streamMs = Math.max(0, stats.llm_ms - fTokenMs);
    const tokPerSec = streamMs > 0 ? stats.usage.output_tokens / (streamMs / 1000) : 0;
    groups.push(t('mind_inspector.code_stats_first_token', { avg: avgFirst.toFixed(1), tps: Math.round(tokPerSec) }));
  }
  const u = stats.usage;
  if (u && (u.input_tokens + u.cache_read_tokens + u.cache_write_tokens > 0 || u.output_tokens > 0)) {
    const hit = cacheHitPercent(u);
    if (hit !== null) groups.push(t('mind_inspector.code_stats_cache_hit', { p: hit }));
    const billed = u.input_tokens + u.cache_read_tokens + u.cache_write_tokens;
    groups.push(t('mind_inspector.code_stats_tokens', { in: formatTokens(billed), out: formatTokens(u.output_tokens) }));
  }
  if (groups.length === 0) return null;
  return (
    <div className="codex-stats-line" title={groups.join(' | ')}>
      {groups.map((g, i) => (
        <React.Fragment key={g}>
          {i > 0 && <span style={{ margin: '0 10px' }} aria-hidden>|</span>}
          <span>{g}</span>
        </React.Fragment>
      ))}
    </div>
  );
};

// ============ 图片附件 ============

const IMAGE_MIME_TYPES = new Set(['image/png', 'image/jpeg', 'image/webp', 'image/gif']);

interface DraftAttachment {
  id: string;
  file: File;
  previewUrl: string;
}

interface CodingImageView {
  media_type: string;
  data: string;
  name?: string | null;
}

/** show_widget 工具渲染的可视化组件（原始 SVG，前端经 dompurify 白名单渲染）。 */
interface CodingWidget {
  title: string;
  kind: string;
  code: string;
}

let attachmentSeq = 0;
function nextAttachmentId(): string {
  attachmentSeq += 1;
  return `att-${Date.now()}-${attachmentSeq}`;
}

function fileToBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = String(reader.result ?? '');
      resolve(result.slice(result.indexOf(',') + 1));
    };
    reader.onerror = () => reject(new Error('读取图片失败'));
    reader.readAsDataURL(file);
  });
}

const AttachmentRail: React.FC<{
  items: DraftAttachment[];
  onOpen: (item: DraftAttachment) => void;
  onRemove: (item: DraftAttachment) => void;
}> = ({ items, onOpen, onRemove }) => {
  const railRef = useRef<HTMLDivElement | null>(null);
  const countRef = useRef<number | null>(null);
  const [edges, setEdges] = useState({ left: false, right: false });

  const updateEdges = useCallback(() => {
    const el = railRef.current;
    if (!el) return;
    const left = el.scrollLeft > 1;
    const right = el.scrollLeft < el.scrollWidth - el.clientWidth - 1;
    setEdges((prev) => (prev.left === left && prev.right === right ? prev : { left, right }));
  }, []);

  useLayoutEffect(() => {
    const grew = countRef.current !== null && items.length > countRef.current;
    countRef.current = items.length;
    const el = railRef.current;
    if (!el) return;
    if (grew) el.scrollLeft = el.scrollWidth - el.clientWidth;
    updateEdges();
  }, [items.length, updateEdges]);

  useEffect(() => {
    const el = railRef.current;
    if (!el) return;
    let disconnect = () => {};
    if (typeof ResizeObserver !== 'undefined') {
      const observer = new ResizeObserver(updateEdges);
      observer.observe(el);
      disconnect = () => observer.disconnect();
    }
    const onWheel = (event: WheelEvent) => {
      if (event.deltaY === 0) return;
      event.preventDefault();
      el.scrollBy({
        left: event.deltaX !== 0
          ? event.deltaX
          : Math.sign(event.deltaY) * Math.min(Math.abs(event.deltaY), 60),
        behavior: 'auto',
      });
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => {
      disconnect();
      el.removeEventListener('wheel', onWheel);
    };
  }, [updateEdges]);

  const page = (direction: -1 | 1) => {
    const el = railRef.current;
    if (!el) return;
    el.scrollBy({ left: direction * Math.max(el.clientWidth - 64, 200), behavior: 'smooth' });
  };

  const { t } = useTranslation();

  return (
    <div style={{ position: 'relative', minWidth: 0 }}>
      <div ref={railRef} onScroll={updateEdges} className="codex-attachment-rail">
        {items.map((item) => (
          <div key={item.id} className="codex-attachment-item">
            <button
              type="button"
              title={item.file.name || t('mind_inspector.code_attach_image')}
              onClick={() => onOpen(item)}
              className="codex-attachment-thumb"
            >
              <img src={item.previewUrl} alt={item.file.name || t('mind_inspector.code_attach_image')} />
            </button>
            <button
              type="button"
              title={t('mind_inspector.code_attach_remove')}
              onClick={() => onRemove(item)}
              className="codex-attachment-remove"
            >
              <X size={9} />
            </button>
          </div>
        ))}
      </div>
      {edges.left && (
        <button
          type="button"
          onClick={() => page(-1)}
          className="codex-icon-btn"
          style={{ position: 'absolute', left: 2, top: '50%', transform: 'translateY(-50%)', zIndex: 2, width: 24, height: 24 }}
        >
          <ChevronRight size={12} style={{ transform: 'rotate(180deg)' }} />
        </button>
      )}
      {edges.right && (
        <button
          type="button"
          onClick={() => page(1)}
          className="codex-icon-btn"
          style={{ position: 'absolute', right: 2, top: '50%', transform: 'translateY(-50%)', zIndex: 2, width: 24, height: 24 }}
        >
          <ChevronRight size={12} />
        </button>
      )}
    </div>
  );
};

const DropOverlay: React.FC<{ disabled?: boolean }> = ({ disabled }) => {
  const { t } = useTranslation();
  return (
  <div role="status" className="codex-drop-overlay">
    <div className="codex-drop-overlay-inner">
      <ImageIcon size={40} strokeWidth={1.4} style={{ color: 'var(--codex-ink-soft)' }} />
      <div style={{ marginTop: 16, fontSize: 20, fontWeight: 600, lineHeight: 28 }}>
        {disabled ? t('mind_inspector.code_attach_unsupported') : t('mind_inspector.code_attach_drop')}
      </div>
      {!disabled && (
        <div style={{ marginTop: 12, fontSize: 13, color: 'var(--codex-ink-faint)' }}>
          {t('mind_inspector.code_attach_support')}
        </div>
      )}
    </div>
  </div>
  );
};

const ImageLightbox: React.FC<{ src: string; alt: string; onClose: () => void }> = ({ src, alt, onClose }) => (
  <div onClick={onClose} className="codex-lightbox">
    <div className="codex-lightbox-backdrop" />
    <img src={src} alt={alt} className="codex-lightbox-img" />
  </div>
);

// ============ 代码块 / 文件工具卡片 ============

const FILE_TOOLS = new Set(['read_file', 'write_file', 'edit_file']);

function isFileTool(name: string): boolean {
  return FILE_TOOLS.has(name);
}

function filePathFromArgs(argsJson: string): string {
  if (!argsJson) return '';
  try {
    const o = JSON.parse(argsJson);
    const v = o?.path ?? o?.file_path ?? o?.file ?? o?.target;
    return typeof v === 'string' ? v : '';
  } catch {
    return '';
  }
}

function looksLikeDiff(text: string): boolean {
  if (!text) return false;
  return /^\+\+\+/m.test(text) || /^---/m.test(text) || /^@@/m.test(text);
}

const DiffCodeBlock: React.FC<{ text: string; maxHeight?: number }> = ({ text, maxHeight }) => {
  if (!looksLikeDiff(text)) {
    return <pre className="codex-pre" style={{ maxHeight }}>{text}</pre>;
  }
  const lines = text.split('\n');
  return (
    <div className="codex-pre" style={{ maxHeight, padding: '6px 0' }}>
      {lines.map((l, i) => {
        let cls = '';
        if (l.startsWith('@@')) cls = 'codex-diff-hunk';
        else if (l.startsWith('+++') || l.startsWith('---')) cls = 'codex-diff-meta';
        else if (l.startsWith('+')) cls = 'codex-diff-add';
        else if (l.startsWith('-')) cls = 'codex-diff-del';
        return (
          <div key={i} className={`codex-diff-line ${cls}`}>
            {l === '' ? '\u00A0' : l}
          </div>
        );
      })}
    </div>
  );
};

const HEAD_TAIL_MAX_LINES = 40;
const HEAD_TAIL_HEAD = 24;
const HEAD_TAIL_TAIL = 12;

const FoldButton: React.FC<{ label: string; onClick: () => void }> = ({ label, onClick }) => (
  <button type="button" onClick={onClick} className="codex-fold-btn">
    {label}
  </button>
);

const HeadTailText: React.FC<{ text: string }> = ({ text }) => {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const lines = text.split('\n');
  if (lines.length <= HEAD_TAIL_MAX_LINES || expanded) {
    return (
      <div>
        <pre className="codex-pre" style={{ maxHeight: expanded ? 480 : 360 }}>{text}</pre>
        {expanded && lines.length > HEAD_TAIL_MAX_LINES && (
          <FoldButton label={t('mind_inspector.code_fold_collapse', { n: lines.length })} onClick={() => setExpanded(false)} />
        )}
      </div>
    );
  }
  const head = lines.slice(0, HEAD_TAIL_HEAD).join('\n');
  const tail = lines.slice(-HEAD_TAIL_TAIL).join('\n');
  const hidden = lines.length - HEAD_TAIL_HEAD - HEAD_TAIL_TAIL;
  return (
    <div>
      <pre className="codex-pre" style={{ margin: 0, borderBottomLeftRadius: 0, borderBottomRightRadius: 0 }}>{head}</pre>
      <FoldButton label={t('mind_inspector.code_fold_expand', { n: hidden })} onClick={() => setExpanded(true)} />
      <pre className="codex-pre" style={{ marginTop: 0, borderTopLeftRadius: 0, borderTopRightRadius: 0 }}>{tail}</pre>
    </div>
  );
};

const NumberedFileBody: React.FC<{ text: string }> = ({ text }) => {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const lines = (text || t('mind_inspector.code_empty_text')).split('\n');
  const renderLines = (from: number, slice: string[]) =>
    slice.map((l, i) => (
      <div key={from + i} className="codex-numbered-line">
        <span className="codex-numbered-gutter">{from + i + 1}</span>
        <span style={{ color: 'var(--codex-ink)' }}>{l === '' ? '\u00A0' : l}</span>
      </div>
    ));
  if (lines.length <= HEAD_TAIL_MAX_LINES || expanded) {
    return (
      <div>
        <div className="codex-pre" style={{ padding: '12px 14px 12px 0', maxHeight: expanded ? 480 : 360 }}>
          {renderLines(0, lines)}
        </div>
        {expanded && lines.length > HEAD_TAIL_MAX_LINES && (
          <FoldButton label={t('mind_inspector.code_fold_collapse', { n: lines.length })} onClick={() => setExpanded(false)} />
        )}
      </div>
    );
  }
  const head = lines.slice(0, HEAD_TAIL_HEAD);
  const tail = lines.slice(-HEAD_TAIL_TAIL);
  const hidden = lines.length - HEAD_TAIL_HEAD - HEAD_TAIL_TAIL;
  return (
    <div>
      <div className="codex-pre" style={{ padding: '12px 14px 12px 0' }}>{renderLines(0, head)}</div>
      <FoldButton label={t('mind_inspector.code_fold_expand', { n: hidden })} onClick={() => setExpanded(true)} />
      <div className="codex-pre" style={{ padding: '12px 14px 12px 0', marginTop: 0 }}>
        {renderLines(lines.length - tail.length, tail)}
      </div>
    </div>
  );
};

const FileToolBody: React.FC<{ path: string; result?: string; name: string }> = ({ path, result, name }) => {
  const { t } = useTranslation();
  // edit_file 的结果 JSON 内嵌后端生成的 unified diff，优先取出渲染；
  // 其余工具（或 JSON 解析失败）回退到对结果文本本身做 diff 探测
  const editDiff = (() => {
    if (name !== 'edit_file') return '';
    const o = tryParseJsonObject(result || '');
    const d = o && typeof o.diff === 'string' ? o.diff : '';
    return d.trim() ? d : '';
  })();
  const diffText = editDiff || (looksLikeDiff(result || '') ? (result || '') : '');
  const isDiff = !!diffText;
  const ds = isDiff ? diffStats(diffText) : null;
  return (
    <div style={{ borderTop: '1px dashed var(--codex-line-light)' }}>
      <div className="codex-file-banner">
        <FileText size={12} style={{ flexShrink: 0, color: 'var(--codex-ink-faint)' }} />
        <span className="codex-file-banner-path">{path || t('mind_inspector.code_file_no_path')}</span>
        <span className="codex-file-banner-tag">{isDiff ? 'diff' : name === 'read_file' ? t('mind_inspector.code_tool_content_tag') : name}</span>
      </div>
      {result !== undefined ? (
        <div className="codex-tool-body">
          {isDiff ? (
            <>
              <DiffCodeBlock text={diffText} maxHeight={360} />
              {ds && (ds.added > 0 || ds.removed > 0) && (
                <div style={{
                  display: 'flex', alignItems: 'center', gap: 8, marginTop: 6, padding: '2px 10px',
                  fontFamily: 'var(--codex-mono)', fontSize: 12, color: 'var(--codex-ink-faint)', userSelect: 'none',
                }}>
                  <span style={{ color: 'var(--codex-success)' }}>+{ds.added}</span>
                  <span style={{ color: 'var(--codex-danger)' }}>−{ds.removed}</span>
                  <span style={{ color: 'var(--codex-ink-faint)' }}>·</span>
                  <span>{t('mind_inspector.code_changes_count', { n: ds.added + ds.removed })}</span>
                </div>
              )}
            </>
          ) : name === 'read_file' ? (
            <NumberedFileBody text={result} />
          ) : (
            <HeadTailText text={result || t('mind_inspector.code_empty_text')} />
          )}
        </div>
      ) : (
        <div className="codex-tool-body" style={{ fontSize: 13, color: 'var(--codex-ink-faint)' }}>{t('mind_inspector.code_wait_result')}</div>
      )}
    </div>
  );
};

/** show_widget 组件卡片：受控渲染（dompurify 白名单）+ 失败降级 + 源码展开。 */
const WidgetCard: React.FC<{ widget: CodingWidget }> = ({ widget }) => {
  const [showSource, setShowSource] = useState(false);
  const sanitized = useMemo(() => {
    try {
      return DOMPurify.sanitize(widget.code);
    } catch {
      return '';
    }
  }, [widget.code]);
  const ok = sanitized.includes('<svg');

  return (
    <div className="codex-widget-card">
      <div className="codex-widget-card-head">
        <span className="codex-widget-card-title">{widget.title}</span>
        <button
          type="button"
          className="codex-widget-card-toggle"
          onClick={() => setShowSource((s) => !s)}
        >
          {showSource ? '收起源码' : '源码'}
        </button>
      </div>
      {ok ? (
        <div className="codex-widget-card-body" dangerouslySetInnerHTML={{ __html: sanitized }} />
      ) : (
        <div className="codex-widget-card-error">
          <XCircle size={14} style={{ flexShrink: 0 }} />
          <span>组件内容未通过安全过滤，无法渲染</span>
        </div>
      )}
      {showSource && (
        <pre className="codex-pre codex-widget-source">{widget.code}</pre>
      )}
    </div>
  );
};

// ============ 工具卡片紧凑摘要 ============
// 目标：非文件工具不再统一渲染"大 IN/OUT 框"，而是按工具类型给出一行关键信息
// （参数中的命令/模式/路径、结果中的匹配数/退出码/条目数等），
// 详情（完整 IN/OUT）留在展开态；空参数/空结果不再制造大片空白。

function tryParseJsonObject(s: string): Record<string, unknown> | null {
  if (!s) return null;
  try {
    const v = JSON.parse(s);
    return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

/** 兼容 {data:{...}} 包装（部分工具结果外层带 data 键） */
function unwrapToolData(o: Record<string, unknown>): Record<string, unknown> {
  return o.data && typeof o.data === 'object' && !Array.isArray(o.data)
    ? (o.data as Record<string, unknown>)
    : o;
}

function compactValue(v: unknown, max = 60): string {
  if (v === null || v === undefined) return 'null';
  const s = typeof v === 'string' ? v : JSON.stringify(v);
  if (!s) return '';
  return s.length > max ? s.slice(0, max) + '…' : s;
}

const TOOL_ARG_KEY_ORDER = ['command', 'pattern', 'query', 'path', 'file_path', 'file', 'target', 'directory'];

/** 参数摘要：优先取关键字段（命令/模式/路径等）；空参数返回 ''（区别于"有参数但为空"） */
function toolArgSummary(argsJson: string, tr?: (key: string, opts?: Record<string, unknown>) => string): string {
  const o = tryParseJsonObject(argsJson);
  if (o) {
    for (const k of TOOL_ARG_KEY_ORDER) {
      const v = o[k];
      if (typeof v === 'string' && (v as string).trim()) return `${k}: ${compactValue(v as string, 80)}`;
      if (typeof v === 'number') return `${k}: ${v}`;
    }
    const entries = Object.entries(o).filter(([, v]) => v !== null && v !== undefined && v !== '');
    if (entries.length === 0) return '';
    if (entries.length <= 2) return entries.map(([k, v]) => `${k}: ${compactValue(v)}`).join(' · ');
    return tr ? tr('mind_inspector.code_args_count', { n: entries.length }) : `${entries.length} 个参数`;
  }
  const t = (argsJson || '').trim();
  return t ? compactValue(t, 80) : '';
}

/** 结果摘要：按工具类型提取关键信息（一行）；无结果返回 '' */
function toolResultSummary(name: string, result: string | undefined, success: boolean | null | undefined, tr?: (key: string, opts?: Record<string, unknown>) => string): string {
  if (result === undefined) return '';
  const t = (result || '').trim();
  if (!t) return success === false
    ? (tr ? tr('mind_inspector.code_tool_failed_no_out') : '执行失败（无输出）')
    : (tr ? tr('mind_inspector.code_empty_text') : '（无返回内容）');
  const o = tryParseJsonObject(t);
  if (o) {
    const d = unwrapToolData(o);
    if (name === 'grep_search') {
      const m = Array.isArray(d.matches) ? d.matches.length : typeof d.matches === 'number' ? d.matches : null;
      const fs = typeof d.files_scanned === 'number' ? d.files_scanned : null;
      if (m !== null) {
        const hit = m > 0
          ? (tr ? tr('mind_inspector.code_grep_found', { n: m }) : `找到 ${m} 处匹配`)
          : (tr ? tr('mind_inspector.code_grep_not_found') : '未找到匹配');
        return `${hit}${fs !== null ? ` · ${tr ? tr('mind_inspector.code_scan_files', { n: fs }) : `扫描 ${fs} 个文件`}` : ''}`;
      }
    }
    if (name === 'list_dir') {
      const n = typeof d.entries === 'number' ? d.entries : Array.isArray(d.entries) ? d.entries.length : null;
      if (n !== null) return tr ? tr('mind_inspector.code_entries_count', { n }) : `${n} 个条目`;
    }
    if (name === 'run_command') {
      const code = typeof d.exit_code === 'number' ? d.exit_code : null;
      const err = typeof d.stderr === 'string' ? d.stderr.trim() : '';
      const out = typeof d.stdout === 'string' ? d.stdout.trim() : '';
      const head = (err || out).split('\n')[0] || '';
      const status = code !== null
        ? (code === 0
          ? (tr ? tr('mind_inspector.code_exit_success') : '✓ 成功')
          : (tr ? tr('mind_inspector.code_exit_code', { n: code }) : `✕ 退出码 ${code}`))
        : success === false
          ? (tr ? tr('mind_inspector.code_run_failed') : '✕ 失败')
          : (tr ? tr('mind_inspector.code_done') : '完成');
      return `${status}${head ? ` · ${compactValue(head, 80)}` : ''}`;
    }
    if (name === 'edit_file' || name === 'write_file') {
      if (typeof d.matches === 'number') return tr ? tr('mind_inspector.code_edit_changes', { n: d.matches }) : `改动 ${d.matches} 处`;
      // edit_file 结果内嵌 unified diff → 摘要行直接给 +新增/−删除 行数
      if (typeof d.diff === 'string' && d.diff.trim()) {
        const st = diffStats(d.diff);
        if (st.added > 0 || st.removed > 0) return `+${st.added} −${st.removed}`;
      }
      if (typeof d.path === 'string') return `${tr ? tr('mind_inspector.code_path_label') : '路径'} ${compactValue(d.path, 60)}`;
      if (d.ok === true) return tr ? tr('mind_inspector.code_written') : '已写入';
    }
    // 通用对象：取前两个非空键（数组只报数量）
    const pairs = Object.entries(d).filter(
      ([, v]) => v !== null && v !== undefined && v !== '' && !(Array.isArray(v) && v.length === 0),
    );
    if (pairs.length > 0) {
      return pairs.slice(0, 2).map(([k, v]) => `${k}: ${compactValue(Array.isArray(v) ? (tr ? tr('mind_inspector.code_items_count', { n: v.length }) : `${v.length} 项`) : v)}`).join(' · ');
    }
  }
  // 纯文本：取首行截断
  return compactValue(t.split('\n')[0], 100);
}

// ============ 工具调用 IN/OUT 展示（全新实现） ============
// 结构：IN/OUT 各占一行，左侧标签、右侧内容；内容用 <pre> 渲染，
// 合法 JSON 自动美化缩进并分色高亮，超长内容头/尾截断 + 展开。

/** 输入/输出文本：可解析为 JSON 时按 2 空格缩进美化；否则原样返回 */
function formatInOut(text: string): string {
  if (!text) return text;
  try {
    return JSON.stringify(JSON.parse(text), null, 2);
  } catch {
    return text;
  }
}

/** 超长截断阈值（按行） */
const IN_OUT_MAX_LINES = 40;
const IN_OUT_HEAD = 24;
const IN_OUT_TAIL = 12;

/** IN/OUT 内容是否视为"空"：空白、空字符串、空对象 {}、空数组 []、null 都算空，空则不渲染该栏 */
function isInOutEmpty(text: string): boolean {
  const t = text.trim();
  return !t || t === '{}' || t === '[]' || t === 'null';
}

/**
 * JSON 语法高亮：对文本做轻量 tokenize 后返回带色 span 序列。
 * 键/冒号 → 紫；字符串 → 棕；数字 → 青；true/false/null → 琥珀；其余原样文本。
 */
function highlightJson(text: string): React.ReactNode[] {
  const re = /("(?:[^"\\]|\\.)*")(\s*:)?|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|true|false|null/g;
  const nodes: React.ReactNode[] = [];
  let last = 0;
  let k = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text)) !== null) {
    if (m.index > last) nodes.push(text.slice(last, m.index));
    const [full, str, colon] = m;
    if (str !== undefined) {
      if (colon !== undefined) {
        nodes.push(
          <span key={k++} className="codex-inout-k">{str}</span>,
          <span key={k++} className="codex-inout-c">{colon}</span>,
        );
      } else {
        nodes.push(<span key={k++} className="codex-inout-s">{str}</span>);
      }
    } else if (full === 'true' || full === 'false' || full === 'null') {
      nodes.push(<span key={k++} className="codex-inout-w">{full}</span>);
    } else {
      nodes.push(<span key={k++} className="codex-inout-n">{full}</span>);
    }
    last = m.index + full.length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

/** 单条 IN/OUT 内容块：<pre> 渲染 + 超长截断 + 展开按钮 */
const InOutBlock: React.FC<{ text: string; failed?: boolean }> = ({ text, failed }) => {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const body = useMemo(() => formatInOut(text), [text]);
  const lines = body.split('\n');
  const tooLong = lines.length > IN_OUT_MAX_LINES;
  const renderPre = (chunk: string) => (
    <pre className={`codex-inout-pre${failed ? ' codex-inout-pre-err' : ''}`}>
      {highlightJson(chunk)}
    </pre>
  );
  if (!tooLong || expanded) {
    return (
      <div className="codex-inout-body">
        {renderPre(body)}
        {expanded && tooLong && (
          <FoldButton label={t('mind_inspector.code_fold_collapse', { n: lines.length })} onClick={() => setExpanded(false)} />
        )}
      </div>
    );
  }
  const head = lines.slice(0, IN_OUT_HEAD).join('\n');
  const tail = lines.slice(-IN_OUT_TAIL).join('\n');
  const hidden = lines.length - IN_OUT_HEAD - IN_OUT_TAIL;
  return (
    <div className="codex-inout-body">
      {renderPre(head)}
      <FoldButton label={t('mind_inspector.code_fold_expand', { n: hidden })} onClick={() => setExpanded(true)} />
      {renderPre(tail)}
    </div>
  );
};

/** IN/OUT 行：内容占主列，右上角徽章标注「输入/输出」 */
const InOutRow: React.FC<{ label: string; text: string; failed?: boolean }> = ({ label, text, failed }) => (
  <div className="codex-inout-row">
    <div className="codex-inout-cell"><InOutBlock text={text} failed={failed} /></div>
    <span className="codex-inout-badge">{label}</span>
  </div>
);

const ToolCallCard: React.FC<{
  name: string;
  argumentsJson: string;
  result?: string;
  success?: boolean | null;
  running?: boolean;
  durationMs?: number | null;
}> = ({ name, argumentsJson, result, success, running, durationMs }) => {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(!!running);
  const meta = toolMeta(name);
  const toolLabel = t(`mind_inspector.code_tool_${name}`, { defaultValue: meta.label });
  const tr = (key: string, opts?: Record<string, unknown>) => t(key, { ...opts, defaultValue: '' });
  const statusColor = running
    ? 'var(--codex-accent)'
    : success === true
      ? 'var(--codex-success)'
      : success === false
        ? 'var(--codex-danger)'
        : 'var(--codex-ink-faint)';

  const prevRunning = useRef(!!running);
  const prevSuccess = useRef(success);
  useEffect(() => {
    if (running) {
      setExpanded(true);
    } else if (prevRunning.current && !running) {
      setExpanded(success !== true);
    } else if (prevSuccess.current == null && success != null) {
      setExpanded(success !== true);
    }
    prevRunning.current = !!running;
    prevSuccess.current = success;
  }, [running, success]);

  const argSum = toolArgSummary(argumentsJson, tr);
  const resSum = toolResultSummary(name, result, success, tr);

  return (
    <div className="codex-tool-card">
      <button type="button" onClick={() => setExpanded((v) => !v)} className="codex-tool-header">
        <span style={{ color: statusColor, display: 'inline-flex', flexShrink: 0 }}>{meta.icon}</span>
        <span className="codex-tool-badge">{t('mind_inspector.code_tool_call')}</span>
        <span style={{ fontWeight: 600, color: 'var(--codex-ink)' }}>{toolLabel}</span>
        <span className="codex-tool-name">{name}</span>
        {durationMs != null && !running && (
          <span className="codex-tool-duration">{formatDuration(durationMs)}</span>
        )}
        {running ? (
          <span className="codex-tool-status codex-tool-status-run">
            <Loader2 size={12} className="codex-spin" />
            {t('mind_inspector.code_tool_running')}
          </span>
        ) : success === true ? (
          <span className="codex-tool-status codex-tool-status-ok">{t('mind_inspector.code_tool_done')}</span>
        ) : success === false ? (
          <span className="codex-tool-status codex-tool-status-err">{t('mind_inspector.code_tool_failed')}</span>
        ) : null}
        <span style={{ color: 'var(--codex-ink-soft)', display: 'inline-flex', flexShrink: 0 }}>
          {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </span>
      </button>
      {isFileTool(name) ? (
        expanded && <FileToolBody path={filePathFromArgs(argumentsJson)} result={result} name={name} />
      ) : (
        <>
          {/* 紧凑摘要行：仅折叠态显示（展开后由完整 IN/OUT 取代，避免重复占位/双虚线） */}
          {!expanded && (argSum || resSum) && (
            <div className="codex-tool-summary">
              {argSum && (
                <span className="codex-tool-summary-arg">{argSum}</span>
              )}
              {resSum && (
                <span className={`codex-tool-summary-res${success === false ? ' codex-tool-summary-err' : ''}`}>
                  {resSum}
                </span>
              )}
            </div>
          )}
          {/* 展开后的完整详情：IN/OUT（内容为空则该栏不显示；都空则整个详情不渲染） */}
          {expanded && (
            <div className="codex-tool-detail">
              {!isInOutEmpty(argumentsJson) || (result !== undefined && !isInOutEmpty(result)) ? (
                <div className="codex-inout-box">
                  {!isInOutEmpty(argumentsJson) && (
                    <InOutRow label={t('mind_inspector.code_inout_in')} text={argumentsJson.trim()} />
                  )}
                  {!isInOutEmpty(argumentsJson) && result !== undefined && !isInOutEmpty(result) && (
                    <div className="codex-inout-divider" />
                  )}
                  {result !== undefined && !isInOutEmpty(result) && (
                    <InOutRow label={t('mind_inspector.code_inout_out')} text={result.trim()} failed={success === false} />
                  )}
                </div>
              ) : null}
            </div>
          )}
        </>
      )}
    </div>
  );
};

// ============ 单轮工作过程分组（连续工具卡片收纳） ============

/** 聊天流渲染项：普通消息 或 一组连续的工具调用消息 */
type ChatRenderItem =
  | { kind: 'msg'; msg: CodingMessage; index: number }
  | { kind: 'group'; msgs: CodingMessage[]; index: number; settled: boolean };

/**
 * 把消息列表切分为渲染项：连续的 tool_use/tool_result 消息聚为一组，
 * 其余消息原样透传。组 ≥2 条工具消息才成组（单张卡片直接渲染，避免一层空壳）。
 *
 * `settled`（组已收尾）判定：组后出现了总结（assistant）/下一轮 user 消息，
 * 或会话已不在运行态——此时分组自动折叠成一行摘要。
 */
function groupChatMessages(messages: CodingMessage[], running: boolean): ChatRenderItem[] {
  const items: ChatRenderItem[] = [];
  let i = 0;
  while (i < messages.length) {
    const m = messages[i];
    if (m.role === 'tool_use' || m.role === 'tool_result') {
      let j = i;
      const group: CodingMessage[] = [];
      while (j < messages.length && (messages[j].role === 'tool_use' || messages[j].role === 'tool_result')) {
        group.push(messages[j]);
        j += 1;
      }
      if (group.length >= 2) {
        const followed = messages
          .slice(j)
          .some((x) => x.role === 'assistant' || x.role === 'user');
        items.push({ kind: 'group', msgs: group, index: i, settled: followed || !running });
      } else {
        group.forEach((msg, k) => items.push({ kind: 'msg', msg, index: i + k }));
      }
      i = j;
    } else {
      items.push({ kind: 'msg', msg: m, index: i });
      i += 1;
    }
  }
  return items;
}

/** 工作过程分组容器：折叠时一行摘要（步数/文件/耗时），展开时逐步工具卡片。 */
const ToolProcessGroup: React.FC<{
  msgs: CodingMessage[];
  settled: boolean;
  sessionRunning: boolean;
  cwd: string;
}> = ({ msgs, settled, sessionRunning, cwd }) => {
  const { t } = useTranslation();
  // 进行中的组默认展开（实时观察）；历史组默认折叠。轮次完成瞬间自动收纳，
  // 之后用户可自由开合（不再被程序强制）
  const [expanded, setExpanded] = useState(!settled);
  const prevSettled = useRef(settled);
  useEffect(() => {
    if (!prevSettled.current && settled) {
      setExpanded(false);
    }
    prevSettled.current = settled;
  }, [settled]);

  // 组统计：步数 / 失败数 / 涉及文件数 / 总耗时 / 未完成步骤
  const stats = useMemo(() => {
    let steps = 0;
    let failed = 0;
    let ms = 0;
    let started = 0;
    const files = new Set<string>();
    for (const m of msgs) {
      if (m.role === 'tool_result') {
        steps += 1;
        if (m.tool_success === false) failed += 1;
        ms += m.tool_duration_ms ?? 0;
        if (
          (m.tool_name === 'write_file' || m.tool_name === 'edit_file') &&
          m.tool_success !== false
        ) {
          const argsJson =
            typeof m.tool_arguments === 'string'
              ? m.tool_arguments
              : JSON.stringify(m.tool_arguments ?? '');
          const p = filePathFromArgs(argsJson);
          if (p) files.add(p);
        }
      } else if (m.role === 'tool_use' && m.tool_name) {
        started += 1;
      }
    }
    return { steps, failed, ms, files: files.size, pending: Math.max(0, started - steps) };
  }, [msgs]);

  const metaParts: string[] = [];
  if (stats.steps > 0) metaParts.push(`${stats.steps} 步`);
  if (stats.pending > 0) metaParts.push(`${stats.pending} 步进行中`);
  if (stats.files > 0) metaParts.push(`${stats.files} 个文件`);
  if (stats.ms > 0) metaParts.push(formatDuration(stats.ms));

  return (
    <div className={`codex-tool-group${expanded ? ' expanded' : ''}`}>
      <button type="button" className="codex-tool-group-header" onClick={() => setExpanded((v) => !v)}>
        <List size={13} style={{ color: 'var(--codex-ink-faint)', flexShrink: 0 }} />
        <span className="codex-tool-group-label">
          {t('mind_inspector.code_tool_group_label', { defaultValue: '工作过程' })}
        </span>
        {metaParts.length > 0 && (
          <span className="codex-tool-group-meta">{metaParts.join(' · ')}</span>
        )}
        {stats.failed > 0 && (
          <span className="codex-tool-group-fail">✕ {stats.failed}</span>
        )}
        {settled ? (
          <span className="codex-tool-group-status codex-tool-group-status-ok">
            {t('mind_inspector.code_tool_group_done', { defaultValue: '已完成' })}
          </span>
        ) : (
          <span className="codex-tool-group-status codex-tool-group-status-run">
            <Loader2 size={12} className="codex-spin" />
            {t('mind_inspector.code_tool_group_running', { defaultValue: '进行中…' })}
          </span>
        )}
        <span
          className="codex-tool-group-chevron"
          style={{ color: 'var(--codex-ink-soft)', display: 'inline-flex', flexShrink: 0 }}
        >
          <ChevronDown size={14} />
        </span>
      </button>
      <div className={`codex-tool-group-reveal${expanded ? ' open' : ''}`}>
        <div className="codex-tool-group-body">
          {msgs.map((msg, i) => {
            // 聚合落库的"工具调用意图"桩消息：参数与结果已由 tool_result 承载，跳过
            if (msg.role === 'tool_use' && !msg.tool_name) return null;
            const wfRun =
              msg.role === 'tool_result' &&
              msg.tool_name === 'run_workflow' &&
              tryParseWorkflowRun(msg.content);
            if (wfRun) {
              return <WorkflowVizCard key={`${msg.tool_call_id ?? i}-${i}`} run={wfRun} />;
            }
            const lsp =
              msg.role === 'tool_result' && msg.tool_name === 'lsp_query'
                ? parseLspResult(msg.content)
                : null;
            if (lsp) {
              return (
                <LspVizCard
                  key={`${msg.tool_call_id ?? i}-${i}`}
                  parsed={lsp}
                  cwd={cwd}
                />
              );
            }
            return (
              <ToolCallCard
                key={`${msg.tool_call_id ?? i}-${i}`}
                name={msg.tool_name ?? ''}
                argumentsJson={
                  msg.tool_arguments
                    ? typeof msg.tool_arguments === 'string'
                      ? msg.tool_arguments
                      : JSON.stringify(msg.tool_arguments)
                    : ''
                }
                result={msg.role === 'tool_result' ? msg.content : undefined}
                success={msg.tool_success ?? null}
                running={msg.role === 'tool_use' && sessionRunning}
                durationMs={msg.tool_duration_ms ?? null}
              />
            );
          })}
        </div>
      </div>
    </div>
  );
};

// ============ 工作流可视化（run_workflow 结果卡片） ============

interface WorkflowStepViz {
  index: number;
  tool: string;
  success: boolean;
  parallel?: boolean;
  summary: string;
}

interface WorkflowRunViz {
  name: string;
  total: number;
  succeeded: number;
  failed: number;
  steps: WorkflowStepViz[];
}

/** 尝试把 run_workflow 的工具结果字符串解析为结构化运行数据；不是工作流结果时返回 null。 */
function tryParseWorkflowRun(raw: string | undefined): WorkflowRunViz | null {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw);
    if (
      v && typeof v === 'object' &&
      typeof v.name === 'string' &&
      typeof v.total === 'number' &&
      Array.isArray(v.steps)
    ) {
      return v as WorkflowRunViz;
    }
    return null;
  } catch {
    return null;
  }
}

/** 工作流运行可视化卡片：名称/成败计数/进度条 + 步骤按「顺序 / 并行组」分组展示。 */
const WorkflowVizCard: React.FC<{ run: WorkflowRunViz }> = ({ run }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(true);
  const pct = run.total > 0 ? Math.round((run.succeeded / run.total) * 100) : 0;
  const allOk = run.failed === 0;

  // 连续 parallel 步骤聚为一组，其余为顺序步骤
  const clusters: { parallel: boolean; steps: WorkflowStepViz[] }[] = [];
  for (const s of run.steps.slice().sort((a, b) => a.index - b.index)) {
    const last = clusters[clusters.length - 1];
    const isPar = !!s.parallel;
    if (last && last.parallel === isPar) {
      last.steps.push(s);
    } else {
      clusters.push({ parallel: isPar, steps: [s] });
    }
  }

  return (
    <div className="codex-wf-card">
      <button type="button" className="codex-wf-header" onClick={() => setOpen((v) => !v)}>
        <span className="codex-wf-seed" style={{ color: allOk ? 'var(--codex-success)' : 'var(--codex-danger)' }}>
          <GitFork size={14} />
        </span>
        <span className="codex-wf-name" style={{ fontWeight: 600, color: 'var(--codex-ink)' }}>
          {t('mind_inspector.code_workflow', { name: run.name || t('mind_inspector.code_workflow_unnamed') })}
        </span>
        <span className="codex-wf-count">
          {t('mind_inspector.code_workflow_success', { succeeded: run.succeeded, total: run.total })}
        </span>
        <span className="codex-wf-chips">
          {allOk ? (
            <span className="codex-wf-chip ok">{t('mind_inspector.code_workflow_all_ok')}</span>
          ) : (
            <span className="codex-wf-chip fail">{t('mind_inspector.code_workflow_failed_steps', { n: run.failed })}</span>
          )}
        </span>
        <span style={{ color: 'var(--codex-ink-soft)', display: 'inline-flex', flexShrink: 0 }}>
          {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </span>
      </button>
      <div className="codex-wf-progress">
        <div className="codex-wf-progress-bar" style={{ width: `${pct}%`, background: allOk ? 'var(--codex-success)' : 'var(--codex-danger-soft)' }} />
      </div>
      {open && (
        <div className="codex-wf-body">
          {clusters.map((c, ci) => (
            <div key={ci} className="codex-wf-cluster">
              {c.parallel && <div className="codex-wf-cluster-tag">{t('mind_inspector.code_workflow_parallel', { n: c.steps.length })}</div>}
              {c.steps.map((s) => (
                <div key={s.index} className="codex-wf-step" title={s.summary}>
                  <span className="codex-wf-step-idx">{s.index + 1}</span>
                  <span className="codex-wf-step-tool">{s.tool}</span>
                  <span className="codex-wf-step-status" style={{ color: s.success ? 'var(--codex-success)' : 'var(--codex-danger)' }}>
                    {s.success ? '✓' : '✕'}
                  </span>
                  <span className="codex-wf-step-summary">{s.summary}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
      )}
    </div>
  );
};

// ============ LSP 语义导航（lsp_query 结果卡片） ============

interface LspLocation {
  path: string;
  line: number;
  column: number;
}

interface LspParsed {
  kind: string;
  locations: LspLocation[];
  hover: string | null;
}

/** 从 LSP hover 结果中提取可读文本（兼容字符串 / 数组 / {value} / markdown）。 */
function extractHoverText(result: unknown): string {
  const contents = (result as { contents?: unknown })?.contents ?? result;
  const flatten = (c: unknown): string =>
    typeof c === 'string' ? c : c && typeof c === 'object' && 'value' in (c as object)
      ? String((c as { value: unknown }).value)
      : '';
  if (typeof contents === 'string') return contents;
  if (Array.isArray(contents)) return contents.map(flatten).filter(Boolean).join('\n\n');
  if (contents && typeof contents === 'object') {
    const vals = (contents as { value?: unknown }).value;
    if (typeof vals === 'string') return vals;
    if (Array.isArray(vals)) return vals.map(flatten).filter(Boolean).join('\n\n');
  }
  return flatten(contents) || JSON.stringify(result).slice(0, 2000);
}

/** 把 lsp_query 工具结果解析为结构化导航数据；不是 LSP 结果时返回 null。 */
function parseLspResult(raw: string | undefined): LspParsed | null {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw);
    if (!v || typeof v !== 'object' || typeof v.kind !== 'string') return null;
    const kind: string = v.kind;
    const result = (v as { result?: unknown }).result;
    if (kind === 'hover') {
      return { kind, locations: [], hover: extractHoverText(result) || null };
    }
    if (!Array.isArray(result)) {
      return { kind, locations: [], hover: null };
    }
    const locations: LspLocation[] = [];
    for (const loc of result as { uri?: string; range?: { start?: { line?: number; character?: number } } }[]) {
      const uri = loc?.uri;
      if (!uri) continue;
      const start = loc?.range?.start ?? {};
      // file:///C:/path → C:\path（本工程为 Windows 目标）
      const path = uri.replace(/^file:\/\//, '').replace(/\//g, '\\');
      locations.push({
        path,
        line: (start.line ?? 0) + 1,
        column: (start.character ?? 0) + 1,
      });
    }
    return { kind, locations, hover: null };
  } catch {
    return null;
  }
}

const LSP_KIND_LABEL: Record<string, string> = {
  go_to_definition: 'mind_inspector.code_lsp_go_to_definition',
  find_references: 'mind_inspector.code_lsp_find_references',
  go_to_implementation: 'mind_inspector.code_lsp_go_to_implementation',
  hover: 'mind_inspector.code_lsp_hover',
};

/** LSP 语义查询结果卡片：locations 按文件分组为可点击的 `path:line:column` 导航行；hover 渲染文本。 */
const LspVizCard: React.FC<{ parsed: LspParsed; cwd: string }> = ({ parsed, cwd }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(true);
  const labelKey = LSP_KIND_LABEL[parsed.kind];
  const label = labelKey ? t(labelKey, { defaultValue: parsed.kind }) : parsed.kind;

  // 按文件分组（相对工作目录展示）
  const groups = new Map<string, LspLocation[]>();
  for (const loc of parsed.locations) groups.set(loc.path, [...(groups.get(loc.path) ?? []), loc]);

  const relative = (p: string): string => {
    if (cwd && p.toLowerCase().startsWith(cwd.toLowerCase())) {
      return p.slice(cwd.length).replace(/^[\\/]/, '') || p;
    }
    if (p.startsWith('\\')) return p.slice(1);
    return p;
  };

  const openFile = (p: string) => {
    void import('@tauri-apps/plugin-shell').then((m) => m.open(p));
  };

  return (
    <div className="codex-lsp-card">
      <button type="button" className="codex-lsp-header" onClick={() => setOpen((v) => !v)}>
        <span className="codex-lsp-seed">
          <Search size={13} />
        </span>
        <span style={{ fontWeight: 600, color: 'var(--codex-ink)' }}>LSP · {label}</span>
        <span className="codex-lsp-count">
          {parsed.hover != null ? t('mind_inspector.code_lsp_hover') : t('mind_inspector.code_lsp_locations', { n: parsed.locations.length })}
        </span>
        <span style={{ color: 'var(--codex-ink-soft)', display: 'inline-flex', flexShrink: 0 }}>
          {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </span>
      </button>
      {open && (
        <div className="codex-lsp-body">
          {parsed.hover != null ? (
            <div className="codex-lsp-hover">{parsed.hover || t('mind_inspector.code_lsp_hover_empty')}</div>
          ) : groups.size === 0 ? (
            <div className="codex-lsp-empty">{t('mind_inspector.code_lsp_no_match')}</div>
          ) : (
            [...groups.entries()].map(([path, locs]) => (
              <div key={path} className="codex-lsp-group">
                <div className="codex-lsp-file" title={path}>
                  {relative(path) || path}
                </div>
                {locs.map((l, i) => (
                  <button
                    key={`${l.line}-${l.column}-${i}`}
                    type="button"
                    className="codex-lsp-loc"
                    title={t('mind_inspector.code_lsp_open_title', { path: `${l.path}:${l.line}:${l.column}` })}
                    onClick={() => openFile(l.path)}
                  >
                    <span className="codex-lsp-loc-pos">
                      :{l.line}:{l.column}
                    </span>
                    <span className="codex-lsp-loc-hint">{t('mind_inspector.code_lsp_open')}</span>
                  </button>
                ))}
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
};

// ============ 工具轮次预算耗尽提示 ============

interface TurnProgress {
  rounds: number;
  failed: number;
  tools: { name: string; label: string; count: number }[];
  files: string[];
}

/** 从当前会话消息统计本轮（最后一次用户消息之后）的工具调用进展 */
function computeTurnProgress(messages: CodingMessage[]): TurnProgress {
  const lastUserIdx = [...messages].reverse().findIndex((m) => m.role === 'user');
  const start = lastUserIdx < 0 ? 0 : messages.length - 1 - lastUserIdx;
  const toolCounts = new Map<string, number>();
  const files = new Set<string>();
  let rounds = 0;
  let failed = 0;
  for (let i = start; i < messages.length; i++) {
    const m = messages[i];
    if (m.role !== 'tool_result') continue;
    rounds += 1;
    const name = m.tool_name ?? '?';
    toolCounts.set(name, (toolCounts.get(name) ?? 0) + 1);
    if (m.tool_success === false) failed += 1;
    if (isFileTool(name)) {
      const raw = typeof m.tool_arguments === 'string' ? m.tool_arguments : JSON.stringify(m.tool_arguments ?? {});
      const p = filePathFromArgs(raw);
      if (p) files.add(p);
    }
  }
  const tools = [...toolCounts.entries()]
    .sort((a, b) => b[1] - a[1])
    .map(([name, count]) => ({ name, label: toolMeta(name).label, count }));
  return { rounds, failed, tools, files: [...files].slice(0, 5) };
}

/** 预算耗尽后的去向选择条：展示进展 + 继续 / 补充说明后继续 / 停止 */
const BudgetStopBanner: React.FC<{
  progress: TurnProgress;
  hint: boolean;
  onContinue: () => void;
  onRefine: () => void;
  onStop: () => void;
}> = ({ progress, hint, onContinue, onRefine, onStop }) => {
  const { t } = useTranslation();
  return (
  <div className="codex-budget-banner">
    <div className="codex-budget-head">
      <span className="codex-budget-title">{t('mind_inspector.code_budget_title')}</span>
      <span className="codex-budget-sub">
        {t('mind_inspector.code_budget_rounds', { n: progress.rounds })}
        {progress.failed > 0 ? t('mind_inspector.code_budget_failed', { n: progress.failed }) : ''}
      </span>
    </div>
    {(progress.tools.length > 0 || progress.files.length > 0) && (
      <div className="codex-budget-body">
        {progress.tools.length > 0 && (
          <div className="codex-budget-chips">
            {progress.tools.map((tl) => (
              <span key={tl.name} className="codex-budget-chip" title={tl.name}>
                {t(`mind_inspector.code_tool_${tl.name}`, { defaultValue: tl.label })} ×{tl.count}
              </span>
            ))}
          </div>
        )}
        {progress.files.length > 0 && (
          <div className="codex-budget-files">
            {progress.files.map((f) => (
              <span key={f} className="codex-budget-file">{f}</span>
            ))}
          </div>
        )}
      </div>
    )}
    {hint && <div className="codex-budget-hint">{t('mind_inspector.code_budget_hint')}</div>}
    <div className="codex-budget-actions">
      <button type="button" onClick={onContinue} className="codex-budget-btn primary">{t('mind_inspector.code_budget_continue')}</button>
      <button type="button" onClick={onRefine} className="codex-budget-btn">{t('mind_inspector.code_budget_refine')}</button>
      <button type="button" onClick={onStop} className="codex-budget-btn ghost">{t('mind_inspector.code_budget_stop')}</button>
    </div>
  </div>
  );
};

/** 常驻目标/计划条：显示会话目标与计划模式状态，可直接编辑目标 / 批准方案 / 退出计划。 */
const GoalPlanBar: React.FC<{
  goal?: string | null;
  plan?: string | null;
  planMode: boolean;
  onRun: (text: string) => void;
}> = ({ goal, plan, planMode, onRun }) => {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  if (!goal && !planMode) return null;
  return (
    <div className="codex-goalplan-bar">
      {goal && (
        <div className="codex-goalplan-row">
          <span className="codex-goalplan-seed">
            <Goal size={13} />
          </span>
          {editing ? (
            <>
              <input
                autoFocus
                className="codex-goalplan-input"
                value={draft}
                placeholder={t('mind_inspector.code_goal_placeholder')}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && draft.trim()) {
                    onRun(`/goal ${draft.trim()}`);
                    setEditing(false);
                  } else if (e.key === 'Escape') {
                    setEditing(false);
                  }
                }}
              />
              <button type="button" className="codex-goalplan-btn" disabled={!draft.trim()} onClick={() => { onRun(`/goal ${draft.trim()}`); setEditing(false); }}>{t('mind_inspector.code_goal_save')}</button>
            </>
          ) : (
            <>
              <span className="codex-goalplan-text" title={goal}>{goal}</span>
              <button type="button" className="codex-goalplan-btn" onClick={() => { setDraft(goal); setEditing(true); }}>{t('mind_inspector.code_goal_edit')}</button>
              <button type="button" className="codex-goalplan-btn ghost" onClick={() => onRun('/goal 清除')}>{t('mind_inspector.code_goal_clear')}</button>
            </>
          )}
        </div>
      )}
      {planMode && (
        <div className="codex-goalplan-row">
          <span className="codex-goalplan-seed">
            <ClipboardList size={13} />
          </span>
          <span className="codex-goalplan-tag">{t('mind_inspector.code_plan_mode')}</span>
          {plan ? (
            <span className="codex-goalplan-text" title={plan}>
              {t('mind_inspector.code_plan_approved', { plan: plan.length > 80 ? `${plan.slice(0, 80)}…` : plan })}
            </span>
          ) : (
            <span className="codex-goalplan-muted">{t('mind_inspector.code_plan_muted')}</span>
          )}
          {!plan && (
            <button type="button" className="codex-goalplan-btn primary" onClick={() => onRun('/plan approve')}>{t('mind_inspector.code_plan_approve')}</button>
          )}
          <button type="button" className="codex-goalplan-btn ghost" onClick={() => onRun('/plan off')}>{t('mind_inspector.code_plan_exit')}</button>
        </div>
      )}
    </div>
  );
};

const MessageBubble: React.FC<{
  msg: CodingMessage;
  onOpenImage?: (src: string, alt: string) => void;
}> = ({ msg, onOpenImage }) => {
  const { t } = useTranslation();
  if (msg.role === 'user') {
    const imgs = msg.images ?? [];
    const refs = msg.file_refs ?? [];
    return (
      <div className="codex-msg-user">
        {refs.length > 0 && (
          <div className="codex-msg-refs">
            {refs.map((r, i) => (
              <span key={i} className="codex-msg-ref" title={r.error ? r.error : r.path}>
                <FileText size={11} />
                {r.error ? (
                  <span className="codex-msg-ref-error">{t('mind_inspector.code_ref_error', { path: r.path, error: r.error })}</span>
                ) : (
                  r.path
                )}
              </span>
            ))}
          </div>
        )}
        {imgs.length > 0 && (
          <div className="codex-msg-images">
            {imgs.map((img, i) => (
              <button
                key={i}
                type="button"
                title={img.name || t('mind_inspector.code_attach_image')}
                onClick={() => onOpenImage?.(`data:${img.media_type};base64,${img.data}`, img.name || t('mind_inspector.code_attach_image'))}
                className="codex-msg-img-btn"
              >
                <img src={`data:${img.media_type};base64,${img.data}`} alt={img.name || t('mind_inspector.code_attach_image')} />
              </button>
            ))}
          </div>
        )}
        <div className="codex-msg-user-bubble">{msg.content}</div>
      </div>
    );
  }
  if (msg.role === 'assistant') {
    // 智能体图片消息（send_image 工具）：渲染图片缩略图 + 可选说明文本
    // 可视化组件消息（show_widget 工具）：渲染受控 SVG 卡片 + 可选说明文本
    const imgs = msg.images ?? [];
    const widgets = msg.widgets ?? [];
    if (imgs.length > 0 || widgets.length > 0) {
      return (
        <div className="codex-msg-assistant-with-images">
          {widgets.length > 0 && (
            <div className="codex-widgets">
              {widgets.map((w, i) => (
                <WidgetCard key={i} widget={w} />
              ))}
            </div>
          )}
          {imgs.length > 0 && (
            <div className="codex-msg-images codex-msg-images-assistant">
              {imgs.map((img, i) => (
                <button
                  key={i}
                  type="button"
                  title={img.name || t('mind_inspector.code_attach_image')}
                  onClick={() => onOpenImage?.(`data:${img.media_type};base64,${img.data}`, img.name || t('mind_inspector.code_attach_image'))}
                  className="codex-msg-img-btn"
                >
                  <img src={`data:${img.media_type};base64,${img.data}`} alt={img.name || t('mind_inspector.code_attach_image')} />
                </button>
              ))}
            </div>
          )}
          {msg.content && <div className="codex-msg-assistant"><MarkdownText text={msg.content} /></div>}
        </div>
      );
    }
    return <div className="codex-msg-assistant"><MarkdownText text={msg.content} /></div>;
  }
  if (msg.role === 'error') {
    return (
      <div className="codex-msg-error">
        <XCircle size={14} style={{ marginTop: 2, color: 'var(--codex-danger)', flexShrink: 0 }} />
        <span className="codex-msg-error-text">
          <strong style={{ marginRight: 6 }}>{t('mind_inspector.code_error_label')}</strong>
          {msg.content}
        </span>
      </div>
    );
  }
  if (msg.role === 'notice') {
    return (
      <div className="codex-msg-notice">
        <Info size={14} style={{ marginTop: 2, color: 'var(--codex-ink-faint)', flexShrink: 0 }} />
        <span className="codex-msg-notice-text">{msg.content}</span>
      </div>
    );
  }
  return null;
};

/**
 * 助手回复末尾的「修改文件」清单。
 *
 * 数据来自宿主记录的工具执行结果（`session.message_changes`），**不是模型自述**：
 * 写/改工具执行成功的那一刻宿主就知道动了哪个文件，因此既不会漏报也不会编造。
 * 行号同样来自实际 diff 的首个 hunk，拿不到时只显示文件名。
 */
const ChangedFilesList: React.FC<{ files: CodingFileChangeView[] }> = ({ files }) => {
  const { t } = useTranslation();
  return (
    <div className="codex-changed-files">
      <div className="codex-changed-files-title">
        <FileEdit size={13} strokeWidth={1.8} />
        {t('mind_inspector.code_changed_files', { defaultValue: '修改文件' })}
      </div>
      <div className="codex-changed-files-list">
        {files.map((f) => (
          <FileChip key={f.path} path={f.path} line={f.line} added={f.added} removed={f.removed} />
        ))}
      </div>
    </div>
  );
};

/** 消息行：气泡 + 悬浮操作（复制 / 有帮助 / 没帮助 / 派生）。 */
const MessageRow: React.FC<{
  msg: CodingMessage;
  index: number;
  feedback?: string | null;
  changedFiles?: CodingFileChangeView[] | null;
  onCopy: (content: string) => void;
  onFork: (index: number) => void;
  onFeedback: (index: number, rating: string) => void;
  onOpenImage?: (src: string, alt: string) => void;
}> = ({ msg, index, feedback, changedFiles, onCopy, onFork, onFeedback, onOpenImage }) => {
  const { t } = useTranslation();
  const isUser = msg.role === 'user';
  const hasActions = isUser || msg.role === 'assistant';
  return (
    <div className="codex-msg-row">
      <MessageBubble msg={msg} onOpenImage={onOpenImage} />
      {msg.role === 'assistant' && changedFiles && changedFiles.length > 0 && (
        <ChangedFilesList files={changedFiles} />
      )}
      {hasActions && (
        <div className={`codex-msg-actions${isUser ? ' codex-msg-actions-end' : ''}`}>
          <button
            type="button"
            title={t('mind_inspector.code_copy_message')}
            className="codex-msg-act"
            onClick={() => onCopy(msg.content)}
          >
            <Copy size={12} />
          </button>
          {!isUser && (
            <>
              <button
                type="button"
                title={t('mind_inspector.code_helpful')}
                className={`codex-msg-act ${feedback === 'up' ? 'active' : ''}`}
                onClick={() => onFeedback(index, feedback === 'up' ? '' : 'up')}
              >
                <ThumbsUp size={12} />
              </button>
              <button
                type="button"
                title={t('mind_inspector.code_not_helpful')}
                className={`codex-msg-act ${feedback === 'down' ? 'active' : ''}`}
                onClick={() => onFeedback(index, feedback === 'down' ? '' : 'down')}
              >
                <ThumbsDown size={12} />
              </button>
            </>
          )}
          <button type="button" title={t('mind_inspector.code_fork')} className="codex-msg-act" onClick={() => onFork(index)}>
            <GitFork size={12} />
          </button>
        </div>
      )}
    </div>
  );
};

const FileTreeNode: React.FC<{
  node: FileNode;
  depth: number;
  expandedSet: Set<string>;
  onToggle: (path: string) => void;
}> = ({ node, depth, expandedSet, onToggle }) => {
  const hasChildren = !!node.children && node.children.length > 0;
  const expanded = expandedSet.has(node.path);
  const Icon = node.is_dir ? (expanded ? FolderOpen : FolderTree) : FileText;

  return (
    <div>
      <div
        onClick={() => node.is_dir && onToggle(node.path)}
        className="codex-tree-node"
        style={{ paddingLeft: 7 + depth * 14 }}
        title={node.path}
      >
        {node.is_dir ? (
          <span style={{ color: 'var(--codex-ink-faint)', display: 'inline-flex', flexShrink: 0 }}>
            {expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          </span>
        ) : <span style={{ width: 12, flexShrink: 0 }} />}
        <span style={{ display: 'inline-flex', color: node.is_dir ? 'var(--codex-ink-faint)' : 'var(--codex-ink-faint)', flexShrink: 0 }}>
          <Icon size={14} />
        </span>
        <span className="codex-tree-name">{node.name}</span>
      </div>
      {node.is_dir && expanded && hasChildren && (
        <div>
          {node.children!.sort((a, b) => {
            if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
            return a.name.localeCompare(b.name);
          }).map((c) => (
            <FileTreeNode
              key={c.path}
              node={c}
              depth={depth + 1}
              expandedSet={expandedSet}
              onToggle={onToggle}
            />
          ))}
        </div>
      )}
    </div>
  );
};

// ============ 左侧工作区文件树（懒加载，点击文件打开预览） ============

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

  // 根目录变化（切换会话/工作区）→ 清空缓存并重新加载
  useEffect(() => {
    let cancelled = false;
    setError('');
    setRoots(null);
    setExpanded(new Set([root]));
    for (const k of Object.keys(depthCache)) delete depthCache[k];
    if (!root) return;
    void (async () => {
      try {
        const res = await invoke<FileNode[]>('coding_list_dir_tree', { directory: root, maxDepth: 1 });
        if (!cancelled) setRoots(res || []);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => { cancelled = true; };
  }, [root, depthCache]);

  /** 目录点击：展开/收起；首次展开时懒加载子节点。 */
  const toggleDir = useCallback(async (path: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
        return next;
      }
      next.add(path);
      return next;
    });
    // 尚未缓存子节点 → 懒加载
    if (depthCache[path] === undefined) {
      setLoading((prev) => new Set(prev).add(path));
      try {
        const res = await invoke<FileNode[]>('coding_list_dir_tree', { directory: path, maxDepth: 1 });
        depthCache[path] = res || [];
      } catch {
        depthCache[path] = [];
      } finally {
        setLoading((prev) => {
          const next = new Set(prev);
          next.delete(path);
          return next;
        });
      }
    }
  }, [depthCache]);

  const renderNode = useCallback((node: FileNode, depth: number): React.ReactNode => {
    const isExpanded = expanded.has(node.path);
    const isLoading = loading.has(node.path);
    const children = (node.is_dir ? depthCache[node.path] : undefined) ?? node.children ?? [];
    const hasChildren = node.is_dir && (children.length > 0 || isLoading);
    return (
      <div key={node.path}>
        <div
          role="treeitem"
          aria-expanded={node.is_dir ? isExpanded : undefined}
          className={`codex-tree-node ${!node.is_dir ? 'codex-tree-file' : ''}`}
          style={{ paddingLeft: 7 + depth * 14 }}
          title={node.path}
          onClick={() => {
            if (node.is_dir) void toggleDir(node.path);
            else onOpenFile(node.path);
          }}
        >
          {node.is_dir ? (
            <span style={{ color: 'var(--codex-ink-faint)', display: 'inline-flex', flexShrink: 0 }}>
              {isLoading ? <Loader2 size={12} className="codex-spin" /> : isExpanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
            </span>
          ) : <span style={{ width: 12, flexShrink: 0 }} />}
          <span style={{ display: 'inline-flex', color: 'var(--codex-ink-faint)', flexShrink: 0 }}>
            {node.is_dir ? (isExpanded ? <FolderOpen size={14} /> : <FolderTree size={14} />) : <FileText size={14} />}
          </span>
          <span className="codex-tree-name">{node.name}</span>
        </div>
        {node.is_dir && isExpanded && hasChildren && (
          <div>
            {[...children]
              .sort((a, b) => {
                if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
                return a.name.localeCompare(b.name);
              })
              .map((c) => renderNode(c, depth + 1))}
            {!isLoading && children.length === 0 && (
              <div className="codex-tree-empty" style={{ paddingLeft: 21 + depth * 14 }}>
                {t('mind_inspector.code_tree_empty', { defaultValue: '（空目录）' })}
              </div>
            )}
          </div>
        )}
      </div>
    );
  }, [expanded, loading, toggleDir, onOpenFile, t, depthCache]);

  return (
    <div className="codex-workspace-tree">
      {error ? (
        <div className="codex-tree-error">{error}</div>
      ) : roots === null ? (
        <div className="codex-tree-loading"><Loader2 size={13} className="codex-spin" /></div>
      ) : roots.length === 0 ? (
        <div className="codex-tree-empty">{t('mind_inspector.code_tree_no_files', { defaultValue: '工作区为空' })}</div>
      ) : (
        roots.map((n) => renderNode(n, 0))
      )}
    </div>
  );
};

// ============ 下拉组件 ============

const ModeDropdown: React.FC<{
  value: string;
  onChange: (mode: string) => void;
  disabled?: boolean;
}> = ({ value, onChange, disabled }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);
  const currentMode = MODES.find((m) => m.key === value) || MODES[0];

  useEffect(() => {
    const onDocDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener('mousedown', onDocDown);
    return () => window.removeEventListener('mousedown', onDocDown);
  }, []);

  return (
    <div ref={ref} className="codex-dropdown">
      <button type="button" disabled={disabled} onClick={() => setOpen((o) => !o)} className="codex-dropdown-trigger">
        <span style={{ display: 'inline-flex', flexShrink: 0 }}><Sparkles size={14} /></span>
        <span>{t(`mind_inspector.code_mode_label_${currentMode.key}`, { defaultValue: currentMode.label })}</span>
        <ChevronDown size={12} style={{ color: 'var(--codex-ink-faint)', flexShrink: 0 }} />
      </button>
      {open && (
        <div className="codex-dropdown-menu" style={{ maxWidth: 320 }}>
          {MODES.map((mode) => (
            <button
              key={mode.key}
              type="button"
              onClick={() => { onChange(mode.key); setOpen(false); }}
              className={`codex-dropdown-item ${mode.key === value ? 'selected' : ''}`}
              style={{ flexDirection: 'column', alignItems: 'flex-start', gap: 2 }}
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: 8, width: '100%' }}>
                <span style={{ fontWeight: 600 }}>{t(`mind_inspector.code_mode_label_${mode.key}`, { defaultValue: mode.label })}</span>
                <div style={{ flex: 1 }} />
                {mode.key === value && <span style={{ display: 'inline-flex', color: 'var(--codex-ink)' }}><Check size={14} /></span>}
              </div>
              <span className="codex-dropdown-hint">{t(`mind_inspector.code_mode_hint_${mode.key}`, { defaultValue: mode.hint })}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
};

const PermissionDropdown: React.FC<{
  value: string;
  onChange: (permission: string) => void;
}> = ({ value, onChange }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);
  const currentPerm = PERMISSIONS.find((p) => p.key === value) || PERMISSIONS[1];
  const permKey = (key: string): string => {
    const map: Record<string, string> = { read_only: 'code_perm_read_only', workspace_write: 'code_perm_workspace', full_access: 'code_perm_full' };
    return map[key] ?? `code_perm_${key}`;
  };

  useEffect(() => {
    const onDocDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener('mousedown', onDocDown);
    return () => window.removeEventListener('mousedown', onDocDown);
  }, []);

  return (
    <div ref={ref} className="codex-dropdown">
      <button type="button" onClick={() => setOpen((o) => !o)} className="codex-dropdown-trigger">
        <span style={{ display: 'inline-flex', flexShrink: 0 }}>{currentPerm.icon}</span>
        <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{currentPerm.label}</span>
        <ChevronDown size={11} style={{ color: 'var(--codex-ink-faint)', flexShrink: 0 }} />
      </button>
      {open && (
        <div className="codex-dropdown-menu">
          {PERMISSIONS.map((perm) => (
            <button
              key={perm.key}
              type="button"
              onClick={() => { onChange(perm.key); setOpen(false); }}
              className={`codex-dropdown-item ${perm.key === value ? 'selected' : ''}`}
            >
              <span style={{ display: 'inline-flex', color: 'var(--codex-ink-faint)' }}>{perm.icon}</span>
              <span style={{ flex: 1 }}>{perm.label}</span>
              <span className="codex-dropdown-hint">{t(`mind_inspector.${permKey(perm.key)}`, { defaultValue: perm.hint })}</span>
              {perm.key === value && <span style={{ display: 'inline-flex', color: 'var(--codex-ink)' }}><Check size={14} /></span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
};

const ModelDropdown: React.FC<{
  model: string;
  reasoningLevel: string;
  onModelChange: (model: string) => void;
  onReasoningChange: (level: string) => void;
  /** 无工作模型却尝试发送时，由父级置位以高亮 + 自动展开下拉框。 */
  highlight?: boolean;
  /** 打开设置窗口 LLM 页（空状态按钮回调）。 */
  onOpenSettings?: () => void;
}> = ({ model, reasoningLevel, onModelChange, onReasoningChange, highlight, onOpenSettings }) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);
  const [models, setModels] = useState<Array<{ id: string; name: string }>>([]);

  useEffect(() => {
    const onDocDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener('mousedown', onDocDown);
    return () => window.removeEventListener('mousedown', onDocDown);
  }, []);

  useEffect(() => {
    const loadModels = async () => {
      try {
        const list = await invoke<Array<{ id: string; name: string }>>('coding_list_available_models');
        setModels(list || []);
      } catch {
        // 拉取失败等同"无可用工作模型"，走空状态引导去设置页
        setModels([]);
      }
    };
    void loadModels();
  }, []);

  // 无工作模型却发送时高亮：自动展开下拉框，让空状态引导按钮立即可见
  useEffect(() => {
    if (highlight) setOpen(true);
  }, [highlight]);

  const currentLevel = REASONING_LEVELS.find((l) => l.key === reasoningLevel) || REASONING_LEVELS[2];
  const displayModel = models.find((m) => m.id === model)?.name ?? (model || models[0]?.name || '');

  return (
    <div ref={ref} className="codex-dropdown">
      <button type="button" onClick={() => setOpen((o) => !o)} className={`codex-dropdown-trigger${highlight ? ' codex-model-highlight' : ''}`}>
        <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', maxWidth: 170 }}>{displayModel}</span>
        <span style={{ color: 'var(--codex-ink-faint)', flexShrink: 0 }}>{currentLevel.label}</span>
        <ChevronDown size={11} style={{ color: 'var(--codex-ink-faint)', flexShrink: 0 }} />
      </button>
      {open && (
        <div className="codex-dropdown-menu" style={{ minWidth: 240, maxWidth: 320 }}>
          <div className="codex-dropdown-label">{t('mind_inspector.code_model_label')}</div>
          {models.length === 0 ? (
            <div className="codex-dropdown-empty">
              <span className="codex-dropdown-empty-text">{t('mind_inspector.code_no_work_model')}</span>
              {onOpenSettings && (
                <button
                  type="button"
                  className="codex-dropdown-empty-btn"
                  onClick={() => { onOpenSettings(); setOpen(false); }}
                >
                  <Settings size={14} />
                  <span>{t('mind_inspector.code_open_llm_settings')}</span>
                </button>
              )}
            </div>
          ) : (
            models.map((m) => (
              <button
                key={m.id}
                type="button"
                onClick={() => { onModelChange(m.id); setOpen(false); }}
                className={`codex-dropdown-item ${m.id === model ? 'selected' : ''}`}
              >
                <span style={{ display: 'inline-flex', color: 'var(--codex-ink-faint)' }}><Cpu size={14} /></span>
                <span style={{ flex: 1, fontSize: 14, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{m.name}</span>
                {m.id === model && <span style={{ display: 'inline-flex', color: 'var(--codex-ink)' }}><Check size={14} /></span>}
              </button>
            ))
          )}
          <div className="codex-dropdown-sep" />
          <div className="codex-dropdown-label">{t('mind_inspector.code_reasoning_label')}</div>
          {REASONING_LEVELS.map((level) => (
            <button
              key={level.key}
              type="button"
              onClick={() => { onReasoningChange(level.key); setOpen(false); }}
              className={`codex-dropdown-item ${level.key === reasoningLevel ? 'selected' : ''}`}
            >
              <span style={{ display: 'inline-flex', color: 'var(--codex-ink-faint)' }}><Zap size={14} /></span>
              <span style={{ flex: 1, fontSize: 14 }}>{level.label}</span>
              {level.key === reasoningLevel && <span style={{ display: 'inline-flex', color: 'var(--codex-ink)' }}><Check size={14} /></span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
};

const SlashCommandMenu: React.FC<{
  position: { x: number; y: number };
  query: string;
  onSelect: (cmd: string) => void;
  onClose: () => void;
}> = ({ position, query, onSelect, onClose }) => {
  const { t } = useTranslation();
  const [selected, setSelected] = useState(0);
  const ref = useRef<HTMLDivElement | null>(null);

  const filtered = filterSlashCommands(query);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelected((s) => Math.min(s + 1, filtered.length - 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelected((s) => Math.max(s - 1, 0));
      } else if (e.key === 'Enter') {
        if (filtered[selected]) {
          e.preventDefault();
          onSelect(filtered[selected].cmd);
        }
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [filtered, selected, onSelect, onClose]);

  useEffect(() => {
    setSelected(0);
  }, [query]);

  // 键盘移动时保持选中项可见
  useEffect(() => {
    ref.current?.querySelector('.codex-slash-item.selected')?.scrollIntoView({ block: 'nearest' });
  }, [selected, filtered]);

  if (filtered.length === 0) {
    return (
      <div ref={ref} className="codex-slash-menu" style={{ left: position.x, top: position.y }}>
        <div className="codex-slash-empty">{t('mind_inspector.code_slash_no_match')}</div>
      </div>
    );
  }

  return (
    <div ref={ref} className="codex-slash-menu" style={{ left: position.x, top: position.y }}>
      {filtered.map((cmd, index) => (
        <button
          key={cmd.cmd}
          type="button"
          onClick={() => onSelect(cmd.cmd)}
          onMouseEnter={() => setSelected(index)}
          className={`codex-slash-item ${index === selected ? 'selected' : ''}`}
        >
          <span style={{ display: 'inline-flex', color: 'var(--codex-ink-faint)', flexShrink: 0 }}>{cmd.icon}</span>
          <span className="codex-slash-main">
            <span className="codex-slash-title">{t(`mind_inspector.code_slash_${cmd.cmd.slice(1)}`, { defaultValue: cmd.label })}</span>
            <span className="codex-slash-desc">{t(`mind_inspector.code_slash_${cmd.cmd.slice(1)}_desc`, { defaultValue: cmd.description })}</span>
          </span>
          <span className="codex-slash-cmd">{cmd.cmd}</span>
        </button>
      ))}
    </div>
  );
};

/** @-mention 文件选择：列出工作目录文件，按标签/路径模糊筛选，回车或点击选中。 */
const FileRefMenu: React.FC<{
  position: { x: number; y: number };
  query: string;
  files: { path: string; label: string }[];
  onSelect: (path: string) => void;
  onClose: () => void;
}> = ({ position, query, files, onSelect, onClose }) => {
  const { t } = useTranslation();
  const [selected, setSelected] = useState(0);
  const ref = useRef<HTMLDivElement | null>(null);
  const q = query.trim().toLowerCase();
  const filtered = q
    ? files.filter((f) => f.label.toLowerCase().includes(q) || f.path.toLowerCase().includes(q))
    : files;

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelected((s) => Math.min(s + 1, filtered.length - 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelected((s) => Math.max(s - 1, 0));
      } else if (e.key === 'Enter') {
        if (filtered[selected]) {
          e.preventDefault();
          onSelect(filtered[selected].path);
        }
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [filtered, selected, onSelect, onClose]);

  useEffect(() => {
    setSelected(0);
  }, [query]);

  useEffect(() => {
    ref.current?.querySelector('.codex-slash-item.selected')?.scrollIntoView({ block: 'nearest' });
  }, [selected, filtered]);

  if (filtered.length === 0) {
    return (
      <div ref={ref} className="codex-slash-menu" style={{ left: position.x, top: position.y }}>
        <div className="codex-slash-empty">{t('mind_inspector.code_at_no_match')}</div>
      </div>
    );
  }
  return (
    <div ref={ref} className="codex-slash-menu codex-at-menu" style={{ left: position.x, top: position.y }}>
      <div className="codex-at-header">
        <FileText size={12} /> {t('mind_inspector.code_at_header')}
      </div>
      {filtered.slice(0, 60).map((f, index) => (
        <button
          key={f.path}
          type="button"
          onClick={() => onSelect(f.path)}
          onMouseEnter={() => setSelected(index)}
          className={`codex-slash-item ${index === selected ? 'selected' : ''}`}
        >
          <span style={{ display: 'inline-flex', color: 'var(--codex-ink-faint)', flexShrink: 0 }}>
            <FileText size={13} />
          </span>
          <span className="codex-slash-main">
            <span className="codex-slash-title">{f.label.split('/').pop()}</span>
            <span className="codex-slash-desc">{f.label}</span>
          </span>
        </button>
      ))}
    </div>
  );
};

// 会话产物卡片：列出本会话生成/修改的文件（write_file / edit_file 成功记录）。
const DeliverablesCard: React.FC<{ cwd: string; deliverables: string[] }> = ({ cwd, deliverables }) => {
  const { t } = useTranslation();
  const rel = (p: string) => {
    const norm = p.replace(/\\/g, '/');
    const cwdNorm = cwd.replace(/\\/g, '/').replace(/\/+$/, '');
    return norm.startsWith(cwdNorm + '/') ? norm.slice(cwdNorm.length + 1) : norm;
  };
  return (
    <div className="codex-info-card">
      <div className="codex-info-title">
        <FileText size={13} /> {t('mind_inspector.code_deliverables')} <span className="codex-deliverable-count">{deliverables.length}</span>
      </div>
      {deliverables.length === 0 ? (
        <div className="codex-empty-note">{t('mind_inspector.code_no_deliverables')}</div>
      ) : (
        <div className="codex-deliverable-list">
          {deliverables.map((p) => (
            <div key={p} className="codex-deliverable-row" title={p}>
              <span className="codex-deliverable-icon"><FileText size={12} /></span>
              <span className="codex-deliverable-name">{rel(p).split('/').pop()}</span>
              <span className="codex-deliverable-path">{rel(p)}</span>
              <button
                type="button"
                title={t('mind_inspector.code_copy_path')}
                className="codex-icon-btn"
                style={{ width: 22, height: 22, flexShrink: 0 }}
                onClick={(e) => {
                  e.stopPropagation();
                  void navigator.clipboard?.writeText(p);
                }}
              >
                <Check size={12} />
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
};

// ============ 页面主体 ============

type WorkTodoItem = { content: string; status: 'pending' | 'in_progress' | 'completed' };

/** 工作智能体给出的一个候选方向。 */
type WorkQuestionOption = { label: string; description?: string | null };

/** 后端广播的子 agent 运行状态（`coding:subagent`）。 */
type SubagentRun = { depth: number; task: string };

/** 后端广播的提问请求（`coding:question`）。 */
type WorkQuestionRequest = {
  question_id: number;
  session_id: string;
  question: string;
  context?: string | null;
  options: WorkQuestionOption[];
  multi_select: boolean;
};

/**
 * 概览页「上下文空间」卡片：显示上下文占用百分比与 system / 工具定义 / 对话消息
 * 的构成占比（参考 deepseek-harness 的 ContextMeter 思路）。
 * 数据来自会话：last_context_tokens（最近一次请求真实输入侧 token）、
 * context_window（模型窗口）、last_context_breakdown（后端估算的构成 token）。
 * 尚无任何请求数据（used/win 缺失）时整卡不渲染。
 */
const ContextSpaceCard: React.FC<{ session: CodingSession | null }> = ({ session }) => {
  const { t } = useTranslation();
  const win = session?.context_window ?? 0;
  const used = session?.last_context_tokens ?? 0;
  const breakdown = session?.last_context_breakdown;
  if (!win || !used) return null;
  const percent = Math.min(100, Math.round((used / win) * 100));
  const parts = [
    { key: 'sys', label: t('mind_inspector.code_context_system'), tokens: breakdown?.[0] ?? 0, cls: 'codex-ctx-system' },
    { key: 'tools', label: t('mind_inspector.code_context_tools'), tokens: breakdown?.[1] ?? 0, cls: 'codex-ctx-tools' },
    { key: 'msg', label: t('mind_inspector.code_context_messages'), tokens: breakdown?.[2] ?? 0, cls: 'codex-ctx-messages' },
  ];
  const total = parts.reduce((a, p) => a + p.tokens, 0);
  // 条形总长保持 provider 上报的精确占用百分比；构成占比仅比例化彩色分段。
  const segments =
    total > 0
      ? parts.map((p) => ({ ...p, width: (p.tokens / total) * percent })).filter((s) => s.width > 0)
      : [{ key: 'total', label: '', tokens: used, cls: '', width: percent }];
  return (
    <div className="codex-info-card">
      <div className="codex-info-title">
        <Braces size={13} />
        {t('mind_inspector.code_context_space')}
        <span className="codex-context-percent">{percent}%</span>
      </div>
      <div className="codex-context-figures">
        ~{formatTokens(used)} / {formatTokens(win)}
      </div>
      <div className="codex-context-bar" title={`${percent}%`}>
        {segments.map((s) => (
          <div
            key={s.key}
            className={`codex-context-seg${s.cls ? ` ${s.cls}` : ''}`}
            style={{ width: `${s.width}%` }}
          />
        ))}
      </div>
      {total > 0 && (
        <div className="codex-context-rows">
          {parts.map((p) => (
            <div key={p.key} className="codex-context-row">
              <span className={`codex-context-swatch ${p.cls}`} aria-hidden />
              <span className="codex-context-label">{p.label}</span>
              <span className="codex-context-tokens">~{formatTokens(p.tokens)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
};

/** 按文件扩展名选择类型图标（变更列表行）。 */
function fileTypeIcon(path: string): React.ReactNode {
  const ext = (path.split('.').pop() ?? '').toLowerCase();
  const common = { size: 14, strokeWidth: 1.8, style: { flexShrink: 0, color: 'var(--codex-ink-faint)' } } as const;
  switch (ext) {
    case 'ts': case 'tsx': case 'js': case 'jsx': case 'rs': case 'go': case 'py':
    case 'java': case 'c': case 'cpp': case 'h': case 'rb': case 'kt': case 'swift':
      return <FileCode {...common} />;
    case 'json': return <Braces {...common} />;
    case 'css': case 'scss': case 'less': return <Palette {...common} />;
    case 'md': case 'txt': return <FileText {...common} />;
    case 'html': case 'htm': case 'vue': return <Globe {...common} />;
    case 'toml': case 'yaml': case 'yml': case 'ini': case 'cfg': case 'env': case 'lock':
      return <Settings {...common} />;
    default: return <FileIcon {...common} />;
  }
}

// ============ 右侧「预览」页：多页签文件预览 ============

/** coding_read_file 命令返回的文件读取结果 */
interface CodingFileRead {
  path: string;
  name: string;
  kind: 'text' | 'image' | 'pdf' | 'binary';
  content?: string | null;
  size: number;
  total_lines?: number | null;
  truncated?: boolean;
}

/** 单文件预览状态：path、加载状态、内容 */
interface PreviewItem {
  path: string;
  loading: boolean;
  error: string;
  data: CodingFileRead | null;
}

/** 从会话消息中提取的「对话文档」候选：read/write/edit 类工具涉及的路径 */
function collectChatDocs(messages: CodingMessage[] | undefined | null): string[] {
  const set = new Set<string>();
  for (const m of messages ?? []) {
    if (m.role === 'tool_result' && m.tool_name) {
      if (['read_file', 'write_file', 'edit_file'].includes(m.tool_name)) {
        if (m.tool_arguments) {
          const a = m.tool_arguments as Record<string, unknown>;
          if (typeof a.path === 'string') set.add(a.path);
        }
      }
    }
    if (m.file_refs) {
      for (const r of m.file_refs) set.add(r.path);
    }
  }
  return [...set];
}

const PreviewPanel: React.FC<{
  tabs: { path: string; key: string }[];
  activePath: string | null;
  onSelect: (path: string) => void;
  onClose: (path: string) => void;
  onOpenFromChat: (path: string) => void;
  messages: CodingMessage[];
  baseKey: string;
  target?: { path: string; line: number; revision: number } | null;
}> = ({ tabs, activePath, onSelect, onClose, onOpenFromChat, messages, baseKey, target }) => {
  const { t } = useTranslation();
  const tabsRef = useWheelHorizontalScroll<HTMLDivElement>();
  // path → 预览状态缓存（按会话隔离，切换会话自动重建）
  const cache = useRef<Map<string, PreviewItem>>(new Map()).current;

  const ensure = useCallback((path: string): PreviewItem => {
    const existing = cache.get(path);
    if (existing) return existing;
    const item: PreviewItem = { path, loading: true, error: '', data: null };
    cache.set(path, item);
    void (async () => {
      try {
        const data = await invoke<CodingFileRead>('coding_read_file', { path });
        cache.set(path, { path, loading: false, error: '', data });
      } catch (e) {
        cache.set(path, { path, loading: false, error: String(e), data: null });
      }
      // 触发重渲染（forceUpdate 语义：set 一个局部计数）
      rerender.current += 1;
      setBump(rerender.current);
    })();
    return item;
  }, [cache]);

  // 懒加载：当前激活 tab 第一次渲染时触发读取
  const [bump, setBump] = useState(0);
  const rerender = useRef(0);
  const activeItem = activePath ? ensure(activePath) : null;

  // 缓存会随会话切换被丢弃，但 ref 生命周期更长：根切换时整体重建
  useEffect(() => {
    cache.clear();
    setBump((v) => v + 1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [baseKey]);

  const docPaths = useMemo(() => collectChatDocs(messages), [messages]);

  const renderContent = (item: PreviewItem | null) => {
    if (!item) {
      return (
        <div className="codex-preview-empty">
          {docPaths.length > 0 && (
            <div className="codex-preview-chatdocs">
              <div className="codex-preview-chatdocs-title">
                {t('mind_inspector.code_preview_chat_docs')}
              </div>
              {docPaths.map((p) => (
                <button key={p} type="button" className="codex-preview-chatdoc" onClick={() => onOpenFromChat(p)} title={p}>
                  <FileText size={11} />
                  <span>{p.split(/[\\/]/).pop()}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      );
    }
    if (item.loading) {
      return (
        <div className="codex-preview-loading">
          <Loader2 size={16} className="codex-spin" />
        </div>
      );
    }
    if (item.error) {
      return <div className="codex-preview-error">{item.error}</div>;
    }
    const d = item.data!;
    if (d.kind === 'image') {
      return (
        <div className="codex-preview-image">
          <img src={convertFileSrc(d.path)} alt={d.name} />
        </div>
      );
    }
    if (d.kind === 'pdf') {
      return (
        <iframe
          className="codex-preview-pdf"
          title={d.name}
          src={convertFileSrc(d.path)}
        />
      );
    }
    if (d.kind === 'binary') {
      return (
        <div className="codex-preview-binary">
          <FileIcon size={22} strokeWidth={1.4} style={{ color: 'var(--codex-ink-faint)' }} />
          <div>{t('mind_inspector.code_preview_binary', { name: d.name, size: formatTokens(Math.max(1, d.size / 1000)) })}</div>
        </div>
      );
    }
    // text：代码走带高亮/行号的源码视图，markdown 走渲染视图
    const isMarkdown = /\.(md|markdown)$/i.test(d.path);
    return (
      <div className="codex-preview-text">
        {isMarkdown
          ? <MarkdownText text={d.content ?? ''} keyPrefix="preview" />
          : (
            <SourceFileView
              key={d.path}
              data={d}
              targetLine={target?.path === d.path ? target.line : undefined}
              navigationRevision={target?.path === d.path ? target.revision : undefined}
            />
          )}
      </div>
    );
  };

  return (
    <div className="codex-preview">
      {/* 顶部多页签（无上限，横向滚动） */}
      <div className="codex-preview-tabs" ref={tabsRef} role="tablist">
        {tabs.length === 0 ? (
          <div className="codex-preview-no-tab">{t('mind_inspector.code_preview_no_tab')}</div>
        ) : (
          tabs.map((tab) => {
            const active = tab.path === activePath;
            return (
              <div
                key={tab.key}
                role="tab"
                aria-selected={active}
                className={`codex-preview-tab ${active ? 'active' : ''}`}
                onClick={() => onSelect(tab.path)}
                title={tab.path}
              >
                <FileText size={11} />
                <span>{tab.path.split(/[\\/]/).pop()}</span>
                <button
                  type="button"
                  className="codex-preview-tab-close"
                  title={t('mind_inspector.code_preview_close')}
                  onClick={(e) => { e.stopPropagation(); onClose(tab.path); }}
                >
                  <X size={10} />
                </button>
              </div>
            );
          })
        )}
      </div>
      {/* 内容区 */}
      <div className="codex-preview-body">{renderContent(activeItem)}</div>
    </div>
  );
};

/**
 * 右侧「变更」页：列出会话中所有被 write_file / edit_file 修改的文件，
 * 每行显示 文件类型图标 + 文件名 + +−行数；点击展开该文件的具体 diff。
 */
const ChangeListCard: React.FC<{ changes: FileChange[] }> = ({ changes }) => {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<number | null>(null);
  const sel = selected !== null && selected < changes.length ? changes[selected] : null;
  return (
    <div className="codex-info-card">
      <div className="codex-info-title">
        <FileDiff size={13} />
        {t('mind_inspector.code_changes_title')}
        <span className="codex-deliverable-count">{changes.length}</span>
      </div>
      {changes.length === 0 ? (
        <div className="codex-changes-empty">{t('mind_inspector.code_changes_empty')}</div>
      ) : (
        <>
          <div className="codex-changes-list">
            {changes.map((c, i) => {
              const base = c.path.split(/[\\/]/).pop() ?? c.path;
              return (
                <button
                  key={c.path}
                  type="button"
                  className={`codex-changes-item${selected === i ? ' selected' : ''}`}
                  title={c.path}
                  onClick={() => setSelected(selected === i ? null : i)}
                >
                  {fileTypeIcon(c.path)}
                  <span className="codex-changes-name">{base}</span>
                  <span className="codex-changes-stats">
                    {c.added > 0 && <span className="codex-changes-add">+{c.added}</span>}
                    {c.removed > 0 && <span className="codex-changes-rem">−{c.removed}</span>}
                    {c.added === 0 && c.removed === 0 && <span className="codex-changes-none">·</span>}
                  </span>
                </button>
              );
            })}
          </div>
          {sel && (
            <div className="codex-changes-diff">
              <div className="codex-file-banner">
                <FileText size={12} style={{ flexShrink: 0, color: 'var(--codex-ink-faint)' }} />
                <span className="codex-file-banner-path">{sel.path}</span>
                <span className="codex-file-banner-tag">diff</span>
              </div>
              {sel.diff.trim() ? (
                <DiffCodeBlock text={sel.diff} maxHeight={420} />
              ) : (
                <div className="codex-changes-no-diff">{t('mind_inspector.code_changes_no_diff')}</div>
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
};

/**
 * 工作智能体独立待办（随编程会话持久化，与陪伴智能体 todo 完全分离）。
 * 会话切换时一次性拉取；此后监听 work_todo:changed 实时刷新——
 * 与工作智能体通过 work_todo_write 工具修改的是同一份数据，也可手动编辑。
 *
 * 存储是**整表替换**语义：手动增/删/勾选一律取回整表、本地改好、整体回写。
 */
const WorkTodosCard: React.FC<{ sessionId: string }> = ({ sessionId }) => {
  const { t } = useTranslation();
  const [items, setItems] = useState<WorkTodoItem[]>([]);
  const [draft, setDraft] = useState('');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');

  const refresh = useCallback(async () => {
    if (!sessionId) return;
    setLoading(true);
    try {
      setItems(await invoke<WorkTodoItem[]>('coding_get_work_todos', { sessionId }));
    } catch {
      /* ignore */
    } finally {
      setLoading(false);
    }
  }, [sessionId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    let un: UnlistenFn | undefined;
    let cancelled = false;
    void (async () => {
      try {
        un = await listen<{ session_id: string; items: WorkTodoItem[] }>('work_todo:changed', (e) => {
          if (e.payload.session_id === sessionId) setItems(e.payload.items);
        });
      } catch {
        /* ignore */
      }
      if (cancelled && un) un();
    })();
    return () => {
      cancelled = true;
      if (un) un();
    };
  }, [sessionId]);

  /** 整表回写：本地改好整张表再提交；被后端拒绝时展示原因并回滚到服务端状态。 */
  const commit = (next: WorkTodoItem[]) => {
    setError('');
    void invoke<WorkTodoItem[]>('coding_write_work_todos', { sessionId, todos: next })
      .then(setItems)
      .catch((e) => {
        setError(typeof e === 'string' ? e : String(e));
        void refresh();
      });
  };

  const add = () => {
    const content = draft.trim();
    if (!content) return;
    if (items.some((it) => it.content === content)) {
      setError(t('mind_inspector.code_todo_duplicate'));
      return;
    }
    setDraft('');
    commit([...items, { content, status: 'pending' }]);
  };

  /** 勾选：completed ⇄ pending。 */
  const toggle = (i: number) => {
    const next: WorkTodoItem[] = items.map((it, idx) =>
      idx === i ? { ...it, status: it.status === 'completed' ? 'pending' : 'completed' } : it,
    );
    commit(next);
  };

  /** 推进状态；置为 in_progress 时把其他进行中项退回 pending（单 active 纪律）。 */
  const setStatus = (i: number, status: WorkTodoItem['status']) => {
    const next: WorkTodoItem[] = items.map((it, idx) => {
      if (idx === i) return { ...it, status };
      if (status === 'in_progress' && it.status === 'in_progress') {
        return { ...it, status: 'pending' };
      }
      return it;
    });
    commit(next);
  };

  const remove = (i: number) => {
    commit(items.filter((_, idx) => idx !== i));
  };

  const statusLabel = (s: string) => {
    if (s === 'in_progress') return t('mind_inspector.code_todo_in_progress');
    if (s === 'completed') return t('mind_inspector.code_todo_completed');
    return t('mind_inspector.code_todo_pending');
  };

  const cnt = (s: string) => items.filter((it) => it.status === s).length;

  return (
    <div className="codex-info-card">
      <div className="codex-info-title">
        <ClipboardList size={13} /> {t('mind_inspector.code_inspector_todo')}{' '}
        <span className="codex-deliverable-count">
          {cnt('completed')}/{items.length}
        </span>
      </div>
      <div className="codex-worktodo-add">
        <input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') add();
          }}
          placeholder={t('mind_inspector.code_todo_placeholder')}
          className="codex-worktodo-input"
        />
        <button
          type="button"
          className="codex-icon-btn"
          style={{ width: 24, height: 24, flexShrink: 0 }}
          onClick={add}
          title={t('mind_inspector.code_todo_add')}
        >
          <Plus size={13} />
        </button>
      </div>
      {error ? <div className="codex-worktodo-error">{error}</div> : null}
      {items.length === 0 ? (
        <div className="codex-empty-note">
          {loading ? '…' : t('mind_inspector.code_todo_empty')}
        </div>
      ) : (
        <div className="codex-worktodo-list">
          {items.map((it, i) => (
            <div key={it.content} className={`codex-worktodo-row ${it.status}`}>
              <input
                type="checkbox"
                checked={it.status === 'completed'}
                onChange={() => toggle(i)}
                title={t('mind_inspector.code_todo_check')}
              />
              {it.status === 'pending' ? (
                <button
                  type="button"
                  className="codex-worktodo-start"
                  onClick={() => setStatus(i, 'in_progress')}
                  title={t('mind_inspector.code_todo_start')}
                >
                  ▶
                </button>
              ) : null}
              <span className="codex-worktodo-title">{it.content}</span>
              <span className={`codex-worktodo-status codex-worktodo-status-${it.status}`}>
                {statusLabel(it.status)}
              </span>
              <button
                type="button"
                className="codex-worktodo-del"
                onClick={() => remove(i)}
                title={t('mind_inspector.code_todo_remove')}
              >
                ×
              </button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
};

/**
 * 工作智能体询问卡片：推进方向分叉时模型出选择题，用户点选（或自己写、或跳过）
 * 后回传，后端唤醒挂起中的 `work_ask_user`，loop 随即继续。
 */
const WorkQuestionCard: React.FC<{ question: WorkQuestionRequest; onDone: () => void }> = ({ question, onDone }) => {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<string[]>([]);
  const [custom, setCustom] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState('');

  const toggle = (label: string) => {
    setError('');
    setSelected((prev) => {
      if (question.multi_select) {
        return prev.includes(label) ? prev.filter((x) => x !== label) : [...prev, label];
      }
      return prev[0] === label ? [] : [label];
    });
    // 单选模式下"选选项"与"自由输入"互斥，后端会拒绝两者并存的提交
    if (!question.multi_select) setCustom('');
  };

  const onCustom = (value: string) => {
    setCustom(value);
    setError('');
    if (!question.multi_select && value.trim()) setSelected([]);
  };

  const submit = (skip: boolean) => {
    setSubmitting(true);
    void invoke('coding_respond_question', {
      questionId: question.question_id,
      answer: { selected: skip ? [] : selected, custom: custom.trim() || null },
    })
      .then(onDone)
      .catch((e) => {
        setError(typeof e === 'string' ? e : String(e));
        setSubmitting(false);
      });
  };

  const canSubmit = selected.length > 0 || custom.trim().length > 0;

  return (
    <div className="codex-ask-card">
      <div className="codex-ask-head">
        <HelpCircle size={13} />
        <span>{t('mind_inspector.code_ask_title')}</span>
        {question.multi_select ? <span className="codex-ask-multi">多选</span> : null}
      </div>
      <div className="codex-ask-question">{question.question}</div>
      {question.context ? <div className="codex-ask-context">{question.context}</div> : null}
      <div className="codex-ask-options">
        {question.options.map((opt) => (
          <button
            key={opt.label}
            type="button"
            className={`codex-ask-option ${selected.includes(opt.label) ? 'active' : ''}`}
            onClick={() => toggle(opt.label)}
          >
            <span className="codex-ask-option-label">{opt.label}</span>
            {opt.description ? <span className="codex-ask-option-desc">{opt.description}</span> : null}
          </button>
        ))}
      </div>
      <input
        value={custom}
        onChange={(e) => onCustom(e.target.value)}
        placeholder={t('mind_inspector.code_ask_custom')}
        className="codex-ask-custom"
      />
      {error ? <div className="codex-ask-error">{error}</div> : null}
      <div className="codex-ask-actions">
        <button
          type="button"
          className="codex-ask-submit"
          disabled={submitting || !canSubmit}
          onClick={() => submit(false)}
        >
          {t('mind_inspector.code_ask_submit')}
        </button>
        <button
          type="button"
          className="codex-ask-skip"
          disabled={submitting}
          onClick={() => submit(true)}
        >
          {t('mind_inspector.code_ask_skip')}
        </button>
      </div>
    </div>
  );
};

const CodeAgentPage: React.FC = () => {
  const { t } = useTranslation();
  /** 工作智能体挂起中的提问；非空时在输入区上方显示待答卡片。 */
  const [ask, setAsk] = useState<WorkQuestionRequest | null>(null);
  /** 正在运行的子 agent（委派出去的子任务），给用户一个"还在跑"的信号。 */
  const [subagent, setSubagent] = useState<SubagentRun | null>(null);
  /** 后台子任务的运行计数（work_delegate 的 background 模式）。 */
  const [bgJobs, setBgJobs] = useState(0);
  /** 后台子任务的运行计数（work_delegate 的 background 模式）。 */
  const [sessions, setSessions] = useState<CodingSession[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [messages, setMessages] = useState<CodingMessage[]>([]);
  const [running, setRunning] = useState(false);
  const [input, setInput] = useState('');
  const [creating, setCreating] = useState(false);

  // 页内错误提示：memory 窗口不承载 ToastWindow，跨窗口 toast 事件可能丢失，
  // 工作页的错误统一在此处内联展示，不再依赖 toast 窗口。
  const [pageError, setPageError] = useState<string | null>(null);
  const pageErrorTimerRef = useRef<number | null>(null);
  const notifyError = useCallback((msg: string) => {
    setPageError(msg);
    if (pageErrorTimerRef.current) window.clearTimeout(pageErrorTimerRef.current);
    pageErrorTimerRef.current = window.setTimeout(() => setPageError(null), 8000);
  }, []);

  const [fileTree, setFileTree] = useState<FileNode[]>([]);
  const [fileTreeLoading, setFileTreeLoading] = useState(false);
  const [expandedDirs, setExpandedDirs] = useState<Set<string>>(new Set());
  const [thinking, setThinking] = useState(false);
  const [thinkingText, setThinkingText] = useState('');
  /** 正在自动压缩上下文：仅在 thinking 期间有意义，用于把「正在思考」换成「正在压缩上下文」 */
  const [compacting, setCompacting] = useState(false);
  const [streamingText, setStreamingText] = useState('');
  const [permission, setPermission] = useState<string>('workspace_write');
  const [modelName, setModelName] = useState('');
  /** 无工作模型却尝试发送时置位，高亮模型下拉框引导去配置。 */
  const [modelHighlight, setModelHighlight] = useState(false);
  // 高亮若干秒后自动熄灭，避免脉动动画无限持续
  useEffect(() => {
    if (!modelHighlight) return;
    const id = window.setTimeout(() => setModelHighlight(false), 8000);
    return () => window.clearTimeout(id);
  }, [modelHighlight]);
  const [reasoningLevel, setReasoningLevel] = useState<string>('high');
  const [slashMenu, setSlashMenu] = useState<{
    visible: boolean;
    query: string;
    position: { x: number; y: number };
  }>({ visible: false, query: '', position: { x: 0, y: 0 } });
  // @-mention 文件引用：菜单状态 + 待发送引用 + 可筛选文件列表
  const [atMenu, setAtMenu] = useState<{
    visible: boolean;
    query: string;
    position: { x: number; y: number };
  }>({ visible: false, query: '', position: { x: 0, y: 0 } });
  const [draftRefs, setDraftRefs] = useState<{ path: string; label: string }[]>([]);
  const [atFiles, setAtFiles] = useState<{ path: string; label: string }[]>([]);
  // 单条消息级反馈（下标 → "up" / "down"）
  const [msgFeedback, setMsgFeedback] = useState<Record<number, string>>({});
  const [stats, setStats] = useState<CodingStats | null>(null);
  const [deliverables, setDeliverables] = useState<string[]>([]);
  const [queue, setQueue] = useState<string[]>([]);
  // 引导消息：不打断当前任务，待当前 run 结束后优先发送（后端加"引导"标注）
  const guideRef = useRef<string | null>(null);
  const [turnStart, setTurnStart] = useState<number | null>(null);
  const [turnElapsed, setTurnElapsed] = useState(0);
  const [atBottom, setAtBottom] = useState(true);
  const atBottomRef = useRef(true);
  // 工具轮次预算耗尽：任务被硬停止后，弹出去向选择条（继续 / 补充说明后继续 / 停止）
  const [budgetStopped, setBudgetStopped] = useState(false);
  const [budgetHint, setBudgetHint] = useState(false);

  // 左右侧边栏收纳与宽度
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [rightCollapsed, setRightCollapsed] = useState(false);
  const [leftWidth, setLeftWidth] = useState(268);
  const [rightWidth, setRightWidth] = useState(360);
  const resizeRef = useRef<{ side: 'left' | 'right'; startX: number; startWidth: number } | null>(null);

  // 会话列表视图
  const [sessionView, setSessionView] = useState<'workspace' | 'flat'>('workspace');
  const [sessionSort, setSessionSort] = useState<'manual' | 'recent'>('recent');
  const [sessionQuery, setSessionQuery] = useState('');
  const [sessionSearchOpen, setSessionSearchOpen] = useState(false);
  const [sessionToolsOpen, setSessionToolsOpen] = useState(false);
  const [manualOrder, setManualOrder] = useState<string[]>([]);
  const [workspaceTitles, setWorkspaceTitles] = useState<Record<string, string>>({});
  const [workspaceMenuOpen, setWorkspaceMenuOpen] = useState<string | null>(null);
  const [renamingWorkspace, setRenamingWorkspace] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState('');

  // 右侧检查器：概览 / 轨迹 / 变更 / 预览 / 终端（待办清单已并入概览页）
  const [rightTab, setRightTab] = useState<'overview' | 'trajectory' | 'changes' | 'preview' | 'terminal'>('overview');
  const [termTabs, setTermTabs] = useState<{ id: string; label: string; workingDirectory: string }[]>([]);
  const [activeTermTab, setActiveTermTab] = useState<string | null>(null);
  const termTabCounter = useRef(0);
  // 横向页签栏滚轮左右滑动
  const inspectorTabsRef = useWheelHorizontalScroll<HTMLDivElement>();
  const termTabsRef = useWheelHorizontalScroll<HTMLDivElement>();

  // 预览：多页签（无上限）打开的文件
  const [previewTabs, setPreviewTabs] = useState<{ path: string; key: string }[]>([]);
  const [activePreview, setActivePreview] = useState<string | null>(null);
  const [previewTarget, setPreviewTarget] = useState<{ path: string; line: number; revision: number } | null>(null);
  const previewCounter = useRef(0);

  /** 打开一个文件到预览页：已开则切换过去，未开则新增页签。 */
  const openPreview = useCallback((path: string, line?: number) => {
    if (!path) return;
    setPreviewTarget(line && line > 0 ? { path, line, revision: Date.now() } : null);
    setPreviewTabs((prev) => {
      if (prev.some((t) => t.path === path)) {
        setActivePreview(path);
        return prev;
      }
      previewCounter.current += 1;
      setActivePreview(path);
      return [...prev, { path, key: `pv-${previewCounter.current}` }];
    });
    setRightTab('preview');
  }, []);

  /** 关闭一个预览页签；关闭最后一个时回到空态。 */
  const closePreview = useCallback((path: string) => {
    setPreviewTabs((prev) => {
      const next = prev.filter((t) => t.path !== path);
      if (activePreview === path) {
        setActivePreview(next.length > 0 ? next[next.length - 1].path : null);
      }
      return next;
    });
  }, [activePreview]);

  const [draftImages, setDraftImages] = useState<DraftAttachment[]>([]);
  const [dragActive, setDragActive] = useState(false);
  const [lightbox, setLightbox] = useState<{ src: string; alt: string } | null>(null);
  const dragDepthRef = useRef(0);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);
  const activeIdRef = useRef<string | null>(null);
  activeIdRef.current = activeId;
  const runningRef = useRef(false);
  runningRef.current = running;

  // 语音输入：录音状态 + ASR 累积（partial 尾部替换 / final 追加）
  const [voiceRecording, setVoiceRecording] = useState(false);
  const voiceRecordingRef = useRef(false);
  voiceRecordingRef.current = voiceRecording;
  const asrPartialRef = useRef('');
  // 本次录音开始时输入框已有文本长度（润色只处理 ASR 追加的尾部）
  const asrBaseLenRef = useRef(0);

  const activeSession = sessions.find((s) => s.session_id === activeId) ?? null;
  const termOpen = rightTab === 'terminal';

  /** markdown 回复里的文件链接：相对路径按会话工作区补全后送预览面板 */
  const openFileFromMarkdown = useCallback((path: string, line?: number) => {
    openPreview(resolveWorkspacePath(path, activeSession?.working_directory), line);
  }, [openPreview, activeSession?.working_directory]);

  const mdFileCtx = useMemo(() => ({ onOpenFile: openFileFromMarkdown }), [openFileFromMarkdown]);

  // 产物列表跟随活动会话；运行中由 coding:deliverable 事件增量追加
  useEffect(() => {
    setDeliverables(activeSession?.deliverables ?? []);
  }, [activeSession]);

  // 消息级反馈跟随活动会话
  useEffect(() => {
    setMsgFeedback(activeSession?.message_feedback ?? {});
  }, [activeSession]);

  // 宿主记录的本轮改动清单（消息下标 → 改动项），跟随活动会话
  const msgChanges = useMemo(() => activeSession?.message_changes ?? {}, [activeSession]);

  // 手动排序：会话列表变化时同步 manualOrder
  useEffect(() => {
    setManualOrder((prev) => {
      const ids = sessions.map((s) => s.session_id);
      const next = prev.filter((id) => ids.includes(id));
      for (const id of ids) {
        if (!next.includes(id)) next.push(id);
      }
      return next;
    });
  }, [sessions]);

  // 无工作区时 cwd 为空串，后端 ConPTY 以当前目录启动
  const ensureTermTab = useCallback((): string | null => {
    const cwd = activeSession?.working_directory ?? '';
    const existing = termTabs.find((t) => t.workingDirectory === cwd);
    if (existing) {
      setActiveTermTab(existing.id);
      return existing.id;
    }
    termTabCounter.current += 1;
    const id = `term-${Date.now()}-${termTabCounter.current}`;
    const label = cwd ? (cwd.split(/[\\/]/).pop() || cwd) : t('mind_inspector.code_tab_terminal');
    setTermTabs((prev) => [...prev, { id, label, workingDirectory: cwd }]);
    setActiveTermTab(id);
    return id;
  }, [activeSession, termTabs, t]);

  // 切到终端视图且无终端时自动开一个
  useEffect(() => {
    if (rightTab === 'terminal' && termTabs.length === 0) {
      ensureTermTab();
    }
  }, [rightTab, termTabs.length, ensureTermTab]);

  const toggleRightSidebar = useCallback(() => {
    setRightCollapsed((v) => !v);
  }, []);

  const startResize = useCallback((e: React.MouseEvent, side: 'left' | 'right') => {
    e.preventDefault();
    e.stopPropagation();
    resizeRef.current = {
      side,
      startX: e.clientX,
      startWidth: side === 'left' ? leftWidth : rightWidth,
    };
  }, [leftWidth, rightWidth]);

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      const r = resizeRef.current;
      if (!r) return;
      if (r.side === 'left') {
        setLeftWidth(Math.max(180, Math.min(460, r.startWidth + (e.clientX - r.startX))));
      } else {
        setRightWidth(Math.max(260, Math.min(640, r.startWidth - (e.clientX - r.startX))));
      }
    };
    const onUp = () => {
      resizeRef.current = null;
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
    return () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
  }, []);

  const handleAddTermTab = useCallback(() => {
    const cwd = activeSession?.working_directory ?? '';
    termTabCounter.current += 1;
    const id = `term-${Date.now()}-${termTabCounter.current}`;
    const label = cwd ? (cwd.split(/[\\/]/).pop() || cwd) : t('mind_inspector.code_tab_terminal');
    setTermTabs((prev) => [...prev, { id, label, workingDirectory: cwd }]);
    setActiveTermTab(id);
    setRightTab('terminal');
  }, [activeSession, t]);

  const handleCloseTermTab = useCallback((id: string) => {
    setTermTabs((prev) => {
      const next = prev.filter((t) => t.id !== id);
      setActiveTermTab((cur) => (cur === id ? (next.length > 0 ? next[next.length - 1].id : null) : cur));
      if (next.length === 0) setRightTab('overview');
      return next;
    });
  }, []);

  const addImages = useCallback((files: readonly File[]) => {
    const images = files.filter((f) => IMAGE_MIME_TYPES.has(f.type));
    if (images.length !== files.length) {
      void emit('toast:show', {
        message: t('mind_inspector.code_img_only'),
        type: 'warning', duration: 3000, key: Date.now(),
      });
    }
    if (images.length === 0) return;
    setDraftImages((prev) => [
      ...prev,
      ...images.map((file) => ({
        id: nextAttachmentId(),
        file,
        previewUrl: URL.createObjectURL(file),
      })),
    ]);
  }, [t]);

  const removeImage = useCallback((id: string) => {
    setDraftImages((prev) => {
      const target = prev.find((d) => d.id === id);
      if (target) URL.revokeObjectURL(target.previewUrl);
      return prev.filter((d) => d.id !== id);
    });
  }, []);

  const handleAttachClick = useCallback(async () => {
    try {
      const picked = await openDialog({
        directory: false,
        multiple: true,
        defaultPath: activeSession?.working_directory ?? undefined,
        filters: [{ name: t('mind_inspector.code_attach_image'), extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif'] }],
      });
      if (picked === null) return;
      const paths = Array.isArray(picked) ? picked : [picked];
      const files: File[] = [];
      for (const p of paths) {
        try {
          const resp = await fetch(convertFileSrc(p)).catch(() => null);
          if (!resp?.ok) continue;
          const blob = await resp.blob();
          const mime = blob.type || 'image/png';
          files.push(new File([blob], p.split(/[\\/]/).pop() || 'image', { type: mime }));
        } catch { /* 跳过不可读文件 */ }
      }
      if (files.length === 0) {
        notifyError(t('mind_inspector.code_img_read_failed'));
      }
      addImages(files);
    } catch (e) {
      notifyError(t('mind_inspector.code_img_add_failed', { e: String(e) }));
    }
  }, [activeSession, addImages, t, notifyError]);

  useEffect(() => {
    const fileTransfer = (event: DragEvent): DataTransfer | null => {
      const dt = event.dataTransfer;
      if (dt === null || !dt.types.includes('Files')) return null;
      return dt;
    };
    const reset = (): void => {
      dragDepthRef.current = 0;
      setDragActive(false);
    };
    const onDragEnter = (event: DragEvent): void => {
      if (fileTransfer(event) === null) return;
      event.preventDefault();
      dragDepthRef.current += 1;
      setDragActive(true);
    };
    const onDragOver = (event: DragEvent): void => {
      const dt = fileTransfer(event);
      if (dt === null) return;
      event.preventDefault();
      dt.dropEffect = 'copy';
    };
    const onDragLeave = (event: DragEvent): void => {
      if (fileTransfer(event) === null) return;
      dragDepthRef.current = Math.max(0, dragDepthRef.current - 1);
      if (dragDepthRef.current === 0) setDragActive(false);
    };
    const onDrop = (event: DragEvent): void => {
      const dt = fileTransfer(event);
      if (dt === null) return;
      event.preventDefault();
      reset();
      addImages([...dt.files]);
    };
    document.addEventListener('dragenter', onDragEnter);
    document.addEventListener('dragover', onDragOver);
    document.addEventListener('dragleave', onDragLeave);
    document.addEventListener('drop', onDrop);
    window.addEventListener('dragend', reset);
    return () => {
      document.removeEventListener('dragenter', onDragEnter);
      document.removeEventListener('dragover', onDragOver);
      document.removeEventListener('dragleave', onDragLeave);
      document.removeEventListener('drop', onDrop);
      window.removeEventListener('dragend', reset);
    };
  }, [addImages]);

  const loadFileTree = useCallback(async (wd: string, expandRoot = true) => {
    if (!wd) { setFileTree([]); return; }
    setFileTreeLoading(true);
    try {
      const result = await invoke<FileNode[]>('coding_list_dir_tree', {
        directory: wd, maxDepth: 1,
      });
      setFileTree(result || []);
      if (expandRoot) {
        setExpandedDirs(new Set([wd]));
      }
    } catch {
      setFileTree([]);
    } finally {
      setFileTreeLoading(false);
    }
  }, []);

  const refreshSessions = useCallback(async () => {
    try {
      const list = await invoke<CodingSession[]>('coding_list_sessions');
      setSessions(list);
      return list;
    } catch {
      return [];
    }
  }, []);

  const loadActiveModelId = useCallback(async (): Promise<string | null> => {
    try {
      const info = await invoke<{ active_id: string | null }>('get_work_models');
      return info?.active_id ?? null;
    } catch {
      return null;
    }
  }, []);

  /** 打开设置窗口并跳到 LLM 页（已存在则聚焦 + 热切换页签）。 */
  const openLlmSettings = useCallback(async () => {
    let win: WebviewWindow | null = null;
    try {
      win = await WebviewWindow.getByLabel('config');
    } catch {
      win = null;
    }
    if (win) {
      try {
        await raiseWindow(win, 'config');
        void emit('config:open-tab', { tab: 'ai' });
        return;
      } catch {
        win = null;
      }
    }
    new WebviewWindow('config', {
      url: '/?view=config&tab=ai',
      title: t('config.title'),
      width: 768,
      height: 624,
      minWidth: 768,
      minHeight: 624,
      decorations: false,
      transparent: false,
      shadow: true,
      center: true,
    });
  }, [t]);

  useEffect(() => {
    void (async () => {
      const list = await refreshSessions();
      if (list.length > 0 && !activeIdRef.current) {
        const latest = [...list].sort((a, b) => b.updated_at - a.updated_at)[0];
        setActiveId(latest.session_id);
        setMessages(latest.messages ?? []);
        setPermission(latest.permission ?? 'workspace_write');
        setReasoningLevel(latest.reasoning_level ?? 'high');
        setModelName(latest.model_id ?? (await loadActiveModelId()) ?? '');
        void loadFileTree(latest.working_directory);
      } else if (list.length === 0) {
        // 无任何会话（未选择工作区）时也要加载全局激活工作模型，让模型下拉框显示正确
        const active = await loadActiveModelId();
        if (active) setModelName(active);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const switchSession = useCallback((s: CodingSession) => {
    setActiveId(s.session_id);
    setMessages(s.messages ?? []);
    setRunning(s.status === 'running');
    setThinking(false);
    setStreamingText('');
    setThinkingText('');
    setStats(s.stats ?? null);
    setQueue([]);
    // 切换会话清空预览页签
    setPreviewTabs([]);
    setActivePreview(null);
    setPermission(s.permission ?? 'workspace_write');
    setReasoningLevel(s.reasoning_level ?? 'high');
    if (s.model_id) setModelName(s.model_id);
    void loadFileTree(s.working_directory);
  }, [loadFileTree]);

  /** 在指定目录新建会话并切过去。返回新建会话，失败返回 null（错误已页内提示）。 */
  const createSessionInWorkspace = useCallback(async (dir: string): Promise<CodingSession | null> => {
    if (creating) return null;
    setCreating(true);
    try {
      const session = await invoke<CodingSession>('coding_new_session', {
        charId: 'vivian',
        workingDirectory: dir,
      });
      const list = await refreshSessions();
      const fresh = list.find((s) => s.session_id === session.session_id) || session;
      switchSession(fresh);
      setInput('');
      inputRef.current?.focus();
      return fresh;
    } catch (e) {
      notifyError(t('mind_inspector.code_create_ws_failed', { e: String(e) }));
      return null;
    } finally {
      setCreating(false);
    }
  }, [creating, refreshSessions, switchSession, t, notifyError]);

  /** 保证存在一个可用会话：已有则复用，无则弹目录选择并新建。返回会话 id，取消/失败返回 null。 */
  const ensureSession = useCallback(async (): Promise<string | null> => {
    if (activeId) return activeId;
    let dir: string | null = null;
    try {
      dir = await openDialog({ directory: true, multiple: false });
    } catch (e) {
      notifyError(t('mind_inspector.code_select_dir_failed', { e: String(e) }));
      return null;
    }
    if (typeof dir !== 'string' || !dir) return null;
    const created = await createSessionInWorkspace(dir);
    return created?.session_id ?? null;
  }, [activeId, createSessionInWorkspace, t, notifyError]);

  const handleCreate = useCallback(async () => {
    if (creating) return;
    let dir: string | null = null;
    try {
      dir = await openDialog({ directory: true, multiple: false });
    } catch (e) {
      notifyError(t('mind_inspector.code_select_dir_failed', { e: String(e) }));
      return;
    }
    if (typeof dir !== 'string' || !dir) return;
    await createSessionInWorkspace(dir);
  }, [creating, createSessionInWorkspace, t, notifyError]);

  const handleDelete = useCallback(async (id: string) => {
    const target = sessions.find((s) => s.session_id === id);
    const title = target?.title?.trim() || t('mind_inspector.code_untitled', { defaultValue: '未命名' });
    // Tauri WebView 中 window.confirm 可能被静默吞掉（不弹窗直接放行），
    // 用 dialog 插件的原生确认框保证弹窗真实可见
    const ok = await confirmDialog(
      t('mind_inspector.code_delete_session_confirm', { title }),
      { title: t('mind_inspector.code_delete_confirm_title', { defaultValue: '删除会话' }), kind: 'warning' },
    );
    if (!ok) return;
    try {
      await invoke('coding_delete_session', { sessionId: id });
      const list = await refreshSessions();
      if (activeIdRef.current === id) {
        if (list.length > 0) switchSession(list[0]);
        else { setActiveId(null); setMessages([]); setFileTree([]); }
      }
    } catch { /* ignore */ }
  }, [sessions, refreshSessions, switchSession, t]);

  const handleSend = useCallback(async () => {
    const text = input.trim();
    const hasImages = draftImages.length > 0;
    const hasRefs = draftRefs.length > 0;
    if (!text && !hasImages && !hasRefs) return;
    if (running) {
      if (text) setQueue((q) => [...q, text]);
      return;
    }
    // 空态首次发送：先保证有会话（无则弹目录选择新建），再继续发送
    const sid = await ensureSession();
    if (!sid) return;
    // 无工作模型时无法路由：高亮模型下拉框引导去设置，不发送
    const workModelId = await loadActiveModelId();
    if (!workModelId) {
      setModelHighlight(true);
      notifyError(t('mind_inspector.code_no_work_model'));
      return;
    }
    setBudgetStopped(false);
    setBudgetHint(false);
    setRunning(true);
    atBottomRef.current = true;
    setAtBottom(true);
    const images: CodingImageView[] = [];
    for (const d of draftImages) {
      try {
        images.push({
          media_type: d.file.type,
          data: await fileToBase64(d.file),
          name: d.file.name || null,
        });
      } catch { /* 单张读取失败跳过 */ }
    }
    if (text || images.length > 0 || draftRefs.length > 0) {
      setMessages((prev) => [
        ...prev,
        {
          role: 'user',
          content: text,
          images: images.length > 0 ? images : undefined,
          file_refs: draftRefs.length > 0 ? draftRefs.map((r) => ({ path: r.path })) : undefined,
          timestamp: Date.now(),
        },
      ]);
    }
    setInput('');
    const sentDrafts = draftImages;
    setDraftImages([]);
    const sentRefs = draftRefs;
    setDraftRefs([]);
    try {
      await invoke('coding_send_message', {
        sessionId: sid,
        message: text,
        images: images.length > 0 ? images : undefined,
        fileRefs: sentRefs.length > 0 ? sentRefs.map((r) => ({ path: r.path })) : undefined,
      });
      for (const d of sentDrafts) URL.revokeObjectURL(d.previewUrl);
    } catch (e) {
      setInput(text);
      setDraftImages(sentDrafts);
      setDraftRefs(sentRefs);
      setMessages((prev) => [...prev, { role: 'error', content: String(e), timestamp: Date.now() }]);
      setRunning(false);
    }
  }, [input, running, draftImages, draftRefs, ensureSession, loadActiveModelId, notifyError]);

  useEffect(() => {
    if (running || !activeId) return;
    // 引导消息：优先于普通排队消息，当前任务结束后发送（后端加"引导"标注）
    const guide = guideRef.current;
    if (guide !== null) {
      guideRef.current = null;
      setRunning(true);
      setBudgetStopped(false);
      setBudgetHint(false);
      atBottomRef.current = true;
      setAtBottom(true);
      setMessages((prev) => [...prev, { role: 'user', content: guide, timestamp: Date.now() }]);
      void (async () => {
        try {
          // guided: 用户给出的引导，后端构建 LLM 上下文时加引导标注
          await invoke('coding_send_message', { sessionId: activeId, message: guide, guided: true });
        } catch (e) {
          setMessages((prev) => [...prev, { role: 'error', content: String(e), timestamp: Date.now() }]);
          setRunning(false);
        }
      })();
      return;
    }
    if (queue.length === 0) return;
    const [next, ...rest] = queue;
    setQueue(rest);
    setRunning(true);
    setBudgetStopped(false);
    setBudgetHint(false);
    atBottomRef.current = true;
    setAtBottom(true);
    setMessages((prev) => [...prev, { role: 'user', content: next, timestamp: Date.now() }]);
    void (async () => {
      try {
        // interjected: 任务执行期间排队的插话，后端构建 LLM 上下文时加插话标注
        await invoke('coding_send_message', { sessionId: activeId, message: next, interjected: true });
      } catch (e) {
        setMessages((prev) => [...prev, { role: 'error', content: String(e), timestamp: Date.now() }]);
        setRunning(false);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [running, queue, activeId]);

  useEffect(() => {
    if (running) {
      setTurnStart((prev) => prev ?? Date.now());
    } else {
      setTurnStart(null);
      setTurnElapsed(0);
    }
  }, [running]);

  useEffect(() => {
    if (turnStart === null) return;
    setTurnElapsed(Math.max(0, Date.now() - turnStart));
    const id = setInterval(() => setTurnElapsed(Math.max(0, Date.now() - turnStart)), 1000);
    return () => clearInterval(id);
  }, [turnStart]);

  const handleCancel = useCallback(async () => {
    if (!activeId) return;
    try { await invoke('coding_cancel_session', { sessionId: activeId }); }
    catch { /* ignore */ }
  }, [activeId]);

  /** 引导：不打断当前任务，把该条标记为引导消息，待当前 run 结束后优先发送（后端加引导标注）。 */
  const handleGuideSend = useCallback((index: number) => {
    if (!activeId) return;
    const target = queue[index];
    if (target === undefined) return;
    setQueue((prev) => prev.filter((_, j) => j !== index));
    guideRef.current = target;
  }, [activeId, queue]);

  /** 编辑待发消息：写回输入框并移出队列。 */
  const handleEditQueued = useCallback((index: number) => {
    setQueue((prev) => {
      const target = prev[index];
      if (target !== undefined) setInput(target);
      return prev.filter((_, j) => j !== index);
    });
    inputRef.current?.focus();
  }, []);

  /** 复制消息内容到剪贴板。 */
  const handleCopyMessage = useCallback((content: string) => {
    void navigator.clipboard?.writeText(content).then(() => {
      void emit('toast:show', {
        message: t('mind_inspector.code_copied'), type: 'success', duration: 2000, key: Date.now(),
      });
    }).catch(() => {});
  }, [t]);

  /** 消息级反馈（up / down，再点取消）。 */
  const handleFeedback = useCallback(async (index: number, rating: string) => {
    if (!activeId) return;
    const prev = msgFeedback[index];
    const next = rating === prev ? '' : rating;
    setMsgFeedback((m) => ({ ...m, [index]: next }));
    try {
      await invoke('coding_set_message_feedback', {
        sessionId: activeId, messageIndex: index, rating: next,
      });
    } catch {
      setMsgFeedback((m) => ({ ...m, [index]: prev }));
    }
  }, [activeId, msgFeedback]);

  /** 从指定消息派生新会话并切换到它。 */
  const handleFork = useCallback(async (index: number) => {
    if (!activeId) return;
    try {
      const fork = await invoke<CodingSession>('coding_fork_session', {
        sessionId: activeId, messageIndex: index,
      });
      const list = await refreshSessions();
      switchSession(fork);
      void list;
    } catch (e) {
      notifyError(t('mind_inspector.code_fork_failed', { e: String(e) }));
    }
  }, [activeId, refreshSessions, switchSession, t, notifyError]);

  const handleSetMode = useCallback(async (mode: string) => {
    if (!activeId || running) return;
    if ((activeSession?.mode ?? 'standard') === mode) return;
    try {
      await invoke('coding_set_mode', { sessionId: activeId, mode });
      setSessions((prev) => prev.map((s) => (s.session_id === activeId ? { ...s, mode } : s)));
    } catch { /* ignore */ }
  }, [activeId, running, activeSession]);

  const toggleDir = useCallback((path: string) => {
    setExpandedDirs((prev) => {
      const n = new Set(prev);
      if (n.has(path)) { n.delete(path); } else { n.add(path); }
      return n;
    });
  }, []);

  // 切换会话 / 任务结束时同步挂起中的提问：面板重挂载或刷新后卡片不能凭空消失
  useEffect(() => {
    if (!activeId) {
      setAsk(null);
      return;
    }
    void invoke<WorkQuestionRequest | null>('coding_pending_question', { sessionId: activeId })
      .then(setAsk)
      .catch(() => setAsk(null));
  }, [activeId, running]);

  useEffect(() => {
    const unlistens: UnlistenFn[] = [];
    let cancelled = false;
    void (async () => {
      const add = async (name: string, handler: (payload: unknown) => void) => {
        const un = await listen(name, (e) => handler(e.payload));
        if (cancelled) { un(); return; }
        unlistens.push(un);
      };
      const guard = (p: unknown): p is { session_id: string } =>
        !!p && typeof (p as { session_id?: unknown }).session_id === 'string';
      const isMine = (p: unknown) => guard(p) && p.session_id === activeIdRef.current;
      const append = (msg: CodingMessage) => setMessages((prev) => [...prev, msg]);

      await add('coding:question', (p) => {
        if (!isMine(p)) return;
        setAsk(p as WorkQuestionRequest);
      });
      await add('coding:job', (p) => {
        if (!isMine(p)) return;
        const ev = p as { status: string };
        if (ev.status === 'started') setBgJobs((n) => n + 1);
        else setBgJobs((n) => Math.max(0, n - 1));
      });
      await add('coding:subagent', (p) => {
        if (!isMine(p)) return;
        const ev = p as { phase: string; depth: number; task?: string };
        if (ev.phase === 'started') {
          setSubagent({ depth: ev.depth ?? 1, task: ev.task ?? '' });
        } else {
          setSubagent(null);
        }
      });
      await add('coding:thinking', (p) => {
        if (!isMine(p)) return;
        setThinking(true);
        setThinkingText('');
      });
      // 自动压缩上下文：开始/结束是宿主状态，不是错误。
      // 全程保持 thinking 指示器，避免压缩结束到主请求首 token 之间出现空档。
      await add('coding:compact_start', (p) => {
        if (!isMine(p)) return;
        setThinking(true);
        setThinkingText('');
        setCompacting(true);
      });
      await add('coding:compact_end', (p) => {
        if (!isMine(p)) return;
        setCompacting(false);
      });
      // 宿主中性提示（压缩完成等）：落成 notice 消息，不走错误通道
      await add('coding:notice', (p) => {
        if (!isMine(p)) return;
        const { kind, message } = p as { kind?: string; message: string };
        if (kind === 'compact') setCompacting(false);
        append({ role: 'notice', content: message, timestamp: Date.now() });
      });
      await add('coding:thinking_chunk', (p) => {
        if (!isMine(p)) return;
        setThinking(true);
        setThinkingText((prev) => prev + (p as { content: string }).content);
      });
      await add('coding:chunk', (p) => {
        if (!isMine(p)) return;
        setThinking(false);
        setThinkingText('');
        setStreamingText((prev) => prev + (p as { content: string }).content);
      });
      await add('coding:assistant_message', (p) => {
        if (!isMine(p)) return;
        setThinking(false);
        setThinkingText('');
        setStreamingText('');
        // images：send_image 工具推送的智能体图片消息（base64 内联，随会话持久化）
        const images = (p as { images?: CodingImageView[] | null }).images ?? null;
        // widgets：show_widget 工具推送的可视化组件（原始 SVG，随会话持久化）
        const widgets = (p as { widgets?: CodingWidget[] | null }).widgets ?? null;
        append({ role: 'assistant', content: (p as { content: string }).content, images, widgets, timestamp: Date.now() });
      });
      await add('coding:tool_call', (p) => {
        if (!isMine(p)) return;
        setThinking(false);
        setThinkingText('');
        setStreamingText('');
        const { id, name, arguments: args } = p as { id: string; name: string; arguments: unknown };
        append({
          role: 'tool_use', content: '',
          tool_name: name, tool_arguments: args, tool_call_id: id, timestamp: Date.now(),
        });
      });
      await add('coding:tool_result', (p) => {
        if (!isMine(p)) return;
        const { id, name, success, result, duration_ms } = p as {
          id: string; name: string; success: boolean; result: string; duration_ms?: number;
        };
        setMessages((prev) => {
          const idx = [...prev].reverse().findIndex((m) => m.role === 'tool_use' && m.tool_call_id === id);
          if (idx < 0) {
            // 页面重载/切换会话后错过 coding:tool_call 事件：本地没有对应 tool_use，
            // 直接追加独立结果卡片，避免工具结果凭空丢失
            return [...prev, {
              role: 'tool_result' as const,
              content: result,
              tool_name: name ?? null,
              tool_arguments: null,
              tool_success: success,
              tool_call_id: id ?? null,
              tool_duration_ms: duration_ms ?? null,
              timestamp: Date.now(),
            }];
          }
          const realIdx = prev.length - 1 - idx;
          const updated = [...prev];
          updated[realIdx] = {
            ...updated[realIdx],
            role: 'tool_result',
            tool_success: success,
            content: result,
            tool_duration_ms: duration_ms ?? null,
          };
          return updated;
        });
      });
      await add('coding:error', (p) => {
        if (!isMine(p)) return;
        const message = (p as { message: string }).message;
        setThinking(false);
        setThinkingText('');
        setStreamingText('');
        append({ role: 'error', content: message, timestamp: Date.now() });
        // 工具轮次预算耗尽硬停止 → 弹出去向选择条。
        // 注意区分"自动续轮"提示（含"自动续轮"，任务仍在继续）与硬停止
        // （含"自动停止"/"可发送新消息继续"）。
        if (
          message.includes('已达到单轮最大工具调用轮数') &&
          (message.includes('自动停止') || message.includes('可发送新消息继续'))
        ) {
          setBudgetStopped(true);
          setBudgetHint(false);
        }
      });
      await add('coding:turn_done', (p) => {
        if (!isMine(p)) return;
        setThinking(false);
        setThinkingText('');
        setCompacting(false);
        setStreamingText('');
        setRunning(false);
        const turnStats = (p as { stats?: CodingStats | null }).stats;
        if (turnStats) setStats(turnStats);
        void (async () => {
          const list = await refreshSessions();
          const cur = list.find((s) => s.session_id === activeIdRef.current);
          if (cur) {
            setMessages(cur.messages ?? []);
            setStats(cur.stats ?? turnStats ?? null);
            setDeliverables(cur.deliverables ?? []);
            void loadFileTree(cur.working_directory, false);
          }
        })();
      });
      // 运行中写入文件成功 → 增量追加到产物面板
      await add('coding:deliverable', (p) => {
        if (!isMine(p)) return;
        const path = (p as { path: string }).path;
        setDeliverables((prev) => (prev.includes(path) ? prev : [...prev, path]));
      });
    })();
    return () => { cancelled = true; unlistens.forEach((fn) => fn()); };
  }, [refreshSessions, loadFileTree]);

  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const isBottom = el.scrollHeight - el.scrollTop - el.clientHeight <= 24;
    atBottomRef.current = isBottom;
    setAtBottom(isBottom);
  }, []);

  const scrollToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    atBottomRef.current = true;
    setAtBottom(true);
  }, []);

  useEffect(() => {
    const el = scrollRef.current;
    if (el && atBottomRef.current) el.scrollTop = el.scrollHeight;
  }, [messages, thinking, streamingText, thinkingText, queue]);

  const canSend = input.trim().length > 0 || draftImages.length > 0 || draftRefs.length > 0;
  const isEmptyChat = !activeSession || messages.length === 0;

  // 预算耗尽横幅的进展数据（仅横幅显示时计算）
  const budgetProgress = useMemo(
    () => (budgetStopped ? computeTurnProgress(messages) : null),
    [budgetStopped, messages],
  );

  // 切换会话时清除预算耗尽提示
  useEffect(() => {
    setBudgetStopped(false);
    setBudgetHint(false);
  }, [activeId]);

  /** 发送一条新的用户消息（供"继续"按钮等复用；无图片） */
  const sendContinuation = useCallback((text: string) => {
    if (!activeId || running) return;
    setRunning(true);
    atBottomRef.current = true;
    setAtBottom(true);
    setBudgetStopped(false);
    setBudgetHint(false);
    setInput('');
    setMessages((prev) => [...prev, { role: 'user', content: text, timestamp: Date.now() }]);
    void invoke('coding_send_message', { sessionId: activeId, message: text }).catch((e) => {
      setMessages((prev) => [...prev, { role: 'error', content: String(e), timestamp: Date.now() }]);
      setRunning(false);
    });
  }, [activeId, running]);

  const handleSlashCommandSelect = useCallback((cmd: string) => {
    setSlashMenu((prev) => ({ ...prev, visible: false }));
    // 把命令插入输入框（带尾随空格，方便直接输入参数），保持输入框聚焦
    setInput(cmd + ' ');
    inputRef.current?.focus();
  }, []);

  /** 拉取工作目录文件列表（@-mention 菜单用，深度 4 覆盖常见嵌套）。 */
  const loadAtFiles = useCallback(async () => {
    const cwd = activeSession?.working_directory;
    if (!cwd) {
      setAtFiles([]);
      return;
    }
    try {
      const tree = await invoke<FileNode[]>('coding_list_dir_tree', { directory: cwd, maxDepth: 4 });
      const out: { path: string; label: string }[] = [];
      const walk = (nodes: FileNode[], prefix: string) => {
        for (const n of nodes) {
          const label = prefix ? `${prefix}/${n.name}` : n.name;
          if (n.is_dir) {
            walk(n.children ?? [], label);
          } else {
            out.push({ path: n.path, label });
          }
        }
      };
      walk(tree ?? [], '');
      setAtFiles(out);
    } catch {
      setAtFiles([]);
    }
  }, [activeSession]);

  const handleInputChange = useCallback((value: string, event?: React.ChangeEvent<HTMLTextAreaElement>) => {
    setInput(value);
    const text = value;
    const rect = event?.target.getBoundingClientRect();
    const lastWord = text.split(/\s+/).pop() ?? '';

    // @-mention：当前词以 @ 开头时打开文件选择菜单
    if (lastWord.startsWith('@') && activeSession) {
      setSlashMenu((prev) => ({ ...prev, visible: false }));
      if (rect) {
        setAtMenu({
          visible: true,
          query: lastWord.slice(1),
          position: { x: rect.left, y: rect.bottom + 8 },
        });
        void loadAtFiles();
      }
      return;
    }
    setAtMenu((prev) => ({ ...prev, visible: false }));

    // 斜杠命令：仅当位于输入开头（第一个词）
    if (text.startsWith('/') && activeSession) {
      // 命令名后已输入空格（开始填写参数）→ 关闭菜单
      const rest = text.slice(1);
      const firstWord = rest.split(/\s+/)[0] ?? '';
      if (rest.length > firstWord.length) {
        setSlashMenu((prev) => ({ ...prev, visible: false }));
        return;
      }
      const query = firstWord.toLowerCase();
      const filtered = filterSlashCommands(query);
      if (filtered.length > 0) {
        if (rect) {
          setSlashMenu({
            visible: true,
            query,
            position: { x: rect.left, y: rect.bottom + 8 },
          });
        }
      } else {
        setSlashMenu((prev) => ({ ...prev, visible: query.length > 0 }));
      }
    } else {
      setSlashMenu((prev) => ({ ...prev, visible: false }));
    }
  }, [activeSession, loadAtFiles]);

  // ── 语音输入 ──
  // 录音期间监听 ASR 事件，把识别结果实时写入输入框（与 ChatWindow 相同的追加/替换逻辑）
  useEffect(() => {
    if (!voiceRecording) { asrPartialRef.current = ''; return; }
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    (async () => {
      unlisten = await listen<{ type: string; text?: string; message?: string }>('asr:event', (e) => {
        const { type, text } = e.payload;
        if (type === 'final_result' && text) {
          setInput((prev) => {
            const base = prev.slice(0, prev.length - asrPartialRef.current.length);
            asrPartialRef.current = '';
            const separator = base === '' || base.endsWith(' ') ? '' : ' ';
            return base + separator + text;
          });
        } else if (type === 'partial_result' && text) {
          setInput((prev) => {
            const base = prev.slice(0, prev.length - asrPartialRef.current.length);
            asrPartialRef.current = text;
            return base + text;
          });
        } else if (type === 'stopped') {
          setVoiceRecording(false);
        } else if (type === 'error') {
          console.warn('[code] ASR 错误:', e.payload.message);
        }
      });
      if (cancelled) unlisten?.();
    })();
    return () => {
      cancelled = true;
      unlisten?.();
      asrPartialRef.current = '';
    };
  }, [voiceRecording]);

  // 录音结束后，对输入框中 ASR 追加的尾部文本做 LLM 润色
  // 延迟 300ms 等待尾部 final_result 写入；用户已编辑或润色失败时保留原文
  const prevVoiceRecordingRef = useRef(false);
  useEffect(() => {
    const was = prevVoiceRecordingRef.current;
    prevVoiceRecordingRef.current = voiceRecording;
    if (!was || voiceRecording) return;
    const timer = window.setTimeout(() => {
      void (async () => {
        const el = inputRef.current;
        if (!el) return;
        const current = el.value;
        const baseLen = asrBaseLenRef.current;
        if (baseLen > current.length) return;
        const base = current.slice(0, baseLen);
        const asrPart = current.slice(baseLen).trim();
        if (asrPart.length < 2) return;
        try {
          const polished = await invoke<string>('polish_asr_text', { text: asrPart });
          const trimmed = (polished ?? '').trim();
          if (!trimmed || inputRef.current?.value !== current) return;
          setInput(base.trim() ? `${base.replace(/\s+$/, '')} ${trimmed}` : trimmed);
        } catch {
          // 润色失败保留原文
        }
      })();
    }, 300);
    return () => window.clearTimeout(timer);
  }, [voiceRecording]);

  const toggleVoiceRecording = useCallback(async () => {
    const isRecording = voiceRecordingRef.current;
    try {
      if (isRecording) {
        await invoke('stop_recognition');
        setVoiceRecording(false);
      } else {
        asrBaseLenRef.current = inputRef.current?.value.length ?? 0;
        await invoke('start_recognition');
        setVoiceRecording(true);
      }
    } catch (e) {
      const msg = typeof e === 'string' ? e : (e instanceof Error ? e.message : String(e));
      console.warn('[code] 语音识别切换失败:', e);
      notifyError(msg);
    }
  }, [notifyError]);

  // 离开页面（切换页签）时自动停止录音，防止 AsrManager 状态泄漏
  useEffect(() => {
    return () => {
      if (voiceRecordingRef.current) {
        void invoke('stop_recognition').catch(() => {});
      }
    };
  }, []);

  /** 选中文件引用：替换 @query 词为 @label，并加入待发送引用列表。 */
  const handleFileRefSelect = useCallback((path: string) => {
    setAtMenu((prev) => ({ ...prev, visible: false }));
    const cwd = activeSession?.working_directory ?? '';
    const normCwd = cwd.replace(/\\/g, '/').replace(/\/+$/, '');
    const normP = path.replace(/\\/g, '/');
    const label = normP.startsWith(normCwd + '/') ? normP.slice(normCwd.length + 1) : normP;
    const words = input.split(/\s+/);
    const lastIdx = words.length - 1;
    if (lastIdx >= 0 && words[lastIdx].startsWith('@')) {
      words[lastIdx] = '@' + label;
    } else {
      words.push('@' + label);
    }
    setInput(words.join(' '));
    setDraftRefs((prev) => (prev.some((r) => r.path === path) ? prev : [...prev, { path, label }]));
    inputRef.current?.focus();
  }, [input, activeSession]);

  /** 移除待发送文件引用。 */
  const removeRef = useCallback((path: string) => {
    setDraftRefs((prev) => prev.filter((r) => r.path !== path));
  }, []);

  const handlePermissionChange = useCallback((perm: string) => {
    setPermission(perm);
    if (activeId) {
      void invoke('coding_set_permission', { sessionId: activeId, permission: perm }).catch(() => {});
    }
  }, [activeId]);

  const handleModelChange = useCallback((modelId: string) => {
    setModelName(modelId);
    setModelHighlight(false);
    if (!activeId) return;
    void invoke('coding_set_model', { sessionId: activeId, modelId }).catch(() => {});
    void (async () => {
      try {
        await invoke('select_work_model', { modelId });
        const info = await invoke<{ active_id: string | null }>('get_work_models').catch(() => null);
        const active = info?.active_id;
        if (active) setModelName(active);
      } catch (e) {
        notifyError(t('mind_inspector.code_switch_model_failed', { e: String(e) }));
      }
    })();
  }, [activeId, t, notifyError]);

  const handleReasoningChange = useCallback((level: string) => {
    setReasoningLevel(level);
    if (activeId) {
      void invoke('coding_set_reasoning_level', { sessionId: activeId, level }).catch(() => {});
    }
  }, [activeId]);

  // 手动排序：上移/下移会话
  const moveSession = useCallback((id: string, dir: -1 | 1) => {
    setManualOrder((prev) => {
      const idx = prev.indexOf(id);
      if (idx < 0) return prev;
      const target = idx + dir;
      if (target < 0 || target >= prev.length) return prev;
      const next = [...prev];
      const [item] = next.splice(idx, 1);
      next.splice(target, 0, item);
      return next;
    });
  }, []);

  // 工作区标题
  const workspaceDisplayName = useCallback((dir: string) => {
    return workspaceTitles[dir] || dir.split(/[\\/]/).pop() || dir;
  }, [workspaceTitles]);

  // 删除工作区（删除该工作区下的所有会话）
  const deleteWorkspace = useCallback(async (dir: string) => {
    const ids = sessions
      .filter((s) => s.working_directory === dir)
      .map((s) => s.session_id);
    for (const id of ids) {
      try {
        await invoke('coding_delete_session', { sessionId: id });
      } catch { /* ignore */ }
    }
    const list = await refreshSessions();
    if (activeIdRef.current && !list.some((s) => s.session_id === activeIdRef.current)) {
      if (list.length > 0) switchSession(list[0]);
      else {
        setActiveId(null);
        setMessages([]);
        setFileTree([]);
      }
    }
  }, [sessions, refreshSessions, switchSession]);

  // 重命名工作区（本地显示标题）
  const startRenameWorkspace = useCallback((dir: string) => {
    setRenamingWorkspace(dir);
    setRenameDraft(workspaceDisplayName(dir));
    setWorkspaceMenuOpen(null);
  }, [workspaceDisplayName]);

  const commitRenameWorkspace = useCallback(() => {
    if (renamingWorkspace && renameDraft.trim()) {
      setWorkspaceTitles((prev) => ({
        ...prev,
        [renamingWorkspace]: renameDraft.trim(),
      }));
    }
    setRenamingWorkspace(null);
    setRenameDraft('');
  }, [renamingWorkspace, renameDraft]);

  // 会话列表：搜索过滤 + 排序
  const visibleSessions = sessions.filter((s) => {
    const q = sessionQuery.trim().toLowerCase();
    if (!q) return true;
    return (
      (s.title || '').toLowerCase().includes(q) ||
      s.working_directory.toLowerCase().includes(q)
    );
  });

  const orderedSessions = [...visibleSessions].sort((a, b) => {
    if (sessionSort === 'recent') return b.updated_at - a.updated_at;
    const ai = manualOrder.indexOf(a.session_id);
    const bi = manualOrder.indexOf(b.session_id);
    return (ai < 0 ? Number.MAX_SAFE_INTEGER : ai) - (bi < 0 ? Number.MAX_SAFE_INTEGER : bi);
  });

  const sessionGroups = new Map<string, CodingSession[]>();
  for (const s of orderedSessions) {
    const key = s.working_directory;
    if (!sessionGroups.has(key)) sessionGroups.set(key, []);
    sessionGroups.get(key)!.push(s);
  }

  const modeLabel = MODES.find((m) => m.key === (activeSession?.mode || 'standard'))?.label ?? '标准模式';
  const permissionLabel = PERMISSIONS.find((p) => p.key === permission)?.label ?? 'Workspace Write';
  const reasoningLabel = REASONING_LEVELS.find((l) => l.key === reasoningLevel)?.label ?? 'High';

  const composer = (
    <div className="codex-composer">
      {draftImages.length > 0 && (
        <AttachmentRail
          items={draftImages}
          onOpen={(item) => setLightbox({ src: item.previewUrl, alt: item.file.name || t('mind_inspector.code_attach_image') })}
          onRemove={(item) => removeImage(item.id)}
        />
      )}
      {draftRefs.length > 0 && (
        <div className="codex-fileref-rail">
          <span className="codex-fileref-rail-label">
            <FileText size={11} /> {t('mind_inspector.code_at_header')}
          </span>
          {draftRefs.map((r) => (
            <span key={r.path} className="codex-fileref-chip" title={r.path}>
              <span className="codex-fileref-name">{r.label.split('/').pop()}</span>
              <span className="codex-fileref-path">{r.label}</span>
              <button
                type="button"
                className="codex-fileref-remove"
                title={t('mind_inspector.code_attach_remove')}
                onClick={() => removeRef(r.path)}
              >
                <X size={10} />
              </button>
            </span>
          ))}
        </div>
      )}
      {queue.length > 0 && (
        <div className="codex-queue">
          <div className="codex-queue-card">
            <div className="codex-queue-head">
              <span className="codex-queue-label">{t('mind_inspector.code_queue_label')}</span>
              <span style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--codex-ink)' }}>{t('mind_inspector.code_queue_count', { n: queue.length })}</span>
            </div>
            {queue.map((q, i) => (
              <div key={`${i}-${q.slice(0, 12)}`} className="codex-queue-row">
                <span className="codex-queue-num">#{i + 1}</span>
                <span className="codex-queue-text">{q}</span>
                <span className="codex-queue-actions">
                  <button
                    type="button"
                    title={t('mind_inspector.code_queue_guide')}
                    onClick={() => handleGuideSend(i)}
                    className="codex-queue-btn codex-queue-btn-send"
                  >
                    <ArrowUp size={14} strokeWidth={2.4} />
                  </button>
                  <button
                    type="button"
                    title={t('mind_inspector.code_queue_edit')}
                    onClick={() => handleEditQueued(i)}
                    className="codex-queue-btn"
                  >
                    <FileEdit size={13} />
                  </button>
                  <button
                    type="button"
                    title={t('mind_inspector.code_queue_remove')}
                    onClick={() => setQueue((prev) => prev.filter((_, j) => j !== i))}
                    className="codex-queue-btn codex-queue-btn-remove"
                  >
                    <Trash2 size={13} />
                  </button>
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
      <div className="codex-composer-input-row">
        <textarea
          ref={inputRef}
          value={input}
          onChange={(e) => handleInputChange(e.target.value, e)}
          onPaste={(e) => {
            const files = Array.from(e.clipboardData?.files ?? []);
            if (files.length > 0) {
              e.preventDefault();
              addImages(files);
            }
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              void handleSend();
            } else if (e.key === 'Escape') {
              if (slashMenu.visible) setSlashMenu((prev) => ({ ...prev, visible: false }));
              if (atMenu.visible) setAtMenu((prev) => ({ ...prev, visible: false }));
            }
          }}
          placeholder={activeSession
            ? (running
              ? t('mind_inspector.code_input_thinking')
              : t('mind_inspector.code_input_compose'))
            : t('mind_inspector.code_input_no_session')}
          rows={1}
          className="codex-composer-textarea"
        />
      </div>
      <div className="codex-composer-toolbar">
        <div className="codex-composer-left">
          <button
            type="button"
            title={t('mind_inspector.code_attach', { defaultValue: '添加图片' })}
            onClick={() => void handleAttachClick()}
            className="codex-icon-btn"
          >
            <Plus size={15} />
          </button>
          {activeSession && (
            <PermissionDropdown value={permission} onChange={handlePermissionChange} />
          )}
        </div>
        <div className="codex-composer-right">
          {/* 模型选择下拉框始终展示：即便未选择工作区（无激活会话）也允许切换全局工作模型 */}
          <ModelDropdown
            model={modelName}
            reasoningLevel={reasoningLevel}
            onModelChange={handleModelChange}
            onReasoningChange={handleReasoningChange}
            highlight={modelHighlight}
            onOpenSettings={openLlmSettings}
          />
          <button
            type="button"
            onClick={() => void toggleVoiceRecording()}
            title={voiceRecording ? t('mind_inspector.code_voice_stop') : t('mind_inspector.code_voice_start')}
            className={`codex-voice-btn${voiceRecording ? ' recording' : ''}`}
          >
            <Mic size={14} />
          </button>
          {running ? (
            <button
              type="button"
              onClick={() => void handleCancel()}
              title={t('mind_inspector.code_stop')}
              className="codex-send-btn codex-stop"
            >
              <Square size={13} />
            </button>
          ) : (
            <button
              type="button"
              onClick={() => void handleSend()}
              disabled={!canSend}
              title={canSend ? t('mind_inspector.code_send_hint') : t('mind_inspector.code_send_disabled')}
              className="codex-send-btn"
            >
              <Send size={14} />
            </button>
          )}
        </div>
      </div>
    </div>
  );

  return (
    <MarkdownFileContext.Provider value={mdFileCtx}>
    <div className="codex-theme workbench-root">
      {/* ===== 左侧：任务会话栏 ===== */}
      <aside className={`codex-sidebar ${leftCollapsed ? 'collapsed' : ''}`} style={{ width: leftCollapsed ? 54 : leftWidth }}>
        <div className="codex-brand">
          {!leftCollapsed && (
            <span className="codex-brand-title">
              <Code2 size={22} strokeWidth={1.6} />
              Work
            </span>
          )}
          <button
            type="button"
            onClick={() => setLeftCollapsed((v) => !v)}
            className="codex-sidebar-collapse-btn"
            title={leftCollapsed ? t('mind_inspector.code_expand_left') : t('mind_inspector.code_collapse_left')}
            aria-label={leftCollapsed ? t('mind_inspector.code_expand_left') : t('mind_inspector.code_collapse_left')}
          >
            {leftCollapsed ? <PanelLeftOpen size={16} /> : <PanelLeftClose size={16} />}
          </button>
        </div>
        {!leftCollapsed && (
          <button type="button" onClick={() => void handleCreate()} disabled={creating} className="codex-new-btn">
            <Plus size={15} />
            {t('mind_inspector.code_new_session', { defaultValue: '新建任务' })}
          </button>
        )}

        {!leftCollapsed && (
        <div className="codex-sidebar-scroll">
          <div className="codex-sidebar-section">
            <div className="codex-sidebar-label codex-session-label">
              <FileText size={12} />
              {t('mind_inspector.code_sessions', { defaultValue: '任务' })}
              <div className="codex-session-tools">
                <button
                  type="button"
                  className="codex-session-tool-btn"
                  title={t('mind_inspector.code_search_sessions')}
                  onClick={() => setSessionSearchOpen((v) => !v)}
                >
                  <Search size={12} />
                </button>
                <button
                  type="button"
                  className="codex-session-tool-btn"
                  title={t('mind_inspector.code_view_sort')}
                  onClick={() => setSessionToolsOpen((v) => !v)}
                >
                  <SlidersHorizontal size={12} />
                </button>
              </div>
            </div>

            {sessionSearchOpen && (
              <div className="codex-session-search">
                <Search size={12} />
                <input
                  autoFocus
                  value={sessionQuery}
                  onChange={(e) => setSessionQuery(e.target.value)}
                  placeholder={t('mind_inspector.code_search_placeholder')}
                />
                {sessionQuery && (
                  <button type="button" onClick={() => setSessionQuery('')} title={t('mind_inspector.code_clear')}>
                    <X size={10} />
                  </button>
                )}
              </div>
            )}

            {sessionToolsOpen && (
              <div className="codex-session-tools-panel">
                <div className="codex-session-tools-title">{t('mind_inspector.code_group_by')}</div>
                <button
                  type="button"
                  className={`codex-session-tools-option ${sessionView === 'workspace' ? 'active' : ''}`}
                  onClick={() => setSessionView('workspace')}
                >
                  <FolderTree size={13} />
                  {t('mind_inspector.code_by_workspace')}
                </button>
                <button
                  type="button"
                  className={`codex-session-tools-option ${sessionView === 'flat' ? 'active' : ''}`}
                  onClick={() => setSessionView('flat')}
                >
                  <List size={13} />
                  {t('mind_inspector.code_flat_list')}
                </button>
                <div className="codex-session-tools-sep" />
                <div className="codex-session-tools-title">{t('mind_inspector.code_sort_by')}</div>
                <button
                  type="button"
                  className={`codex-session-tools-option ${sessionSort === 'recent' ? 'active' : ''}`}
                  onClick={() => setSessionSort('recent')}
                >
                  <Activity size={13} />
                  {t('mind_inspector.code_recent')}
                </button>
                <button
                  type="button"
                  className={`codex-session-tools-option ${sessionSort === 'manual' ? 'active' : ''}`}
                  onClick={() => setSessionSort('manual')}
                >
                  <ArrowUp size={13} />
                  {t('mind_inspector.code_manual')}
                </button>
              </div>
            )}

            <div className="codex-session-list">
              {orderedSessions.length === 0 && (
                <div className="codex-empty-note">
                  {sessions.length === 0
                    ? t('mind_inspector.code_no_sessions', { defaultValue: '暂无会话' })
                    : t('mind_inspector.code_no_match_sessions')}
                </div>
              )}

              {sessionView === 'flat'
                ? orderedSessions.map((s) => {
                    const active = s.session_id === activeId;
                    return (
                      <div
                        key={s.session_id}
                        onClick={() => switchSession(s)}
                        className={`codex-session-item ${active ? 'active' : ''}`}
                      >
                        {sessionSort === 'manual' && (
                          <div className="codex-session-manual">
                            <button
                              type="button"
                              title={t('mind_inspector.code_move_up')}
                              onClick={(e) => { e.stopPropagation(); moveSession(s.session_id, -1); }}
                            >
                              <ArrowUp size={10} />
                            </button>
                            <button
                              type="button"
                              title={t('mind_inspector.code_move_down')}
                              onClick={(e) => { e.stopPropagation(); moveSession(s.session_id, 1); }}
                            >
                              <ArrowDown size={10} />
                            </button>
                          </div>
                        )}
                        <div style={{ minWidth: 0, flex: 1 }}>
                          <div className="codex-session-title">
                            {s.title || t('mind_inspector.code_untitled', { defaultValue: '未命名' })}
                          </div>
                          <div className="codex-session-meta">
                            <FolderOpen size={11} />
                            <span>{s.working_directory.split(/[\\/]/).pop() || s.working_directory}</span>
                            {s.status === 'running' && (
                              <Loader2 size={10} className="codex-spin" style={{ color: 'var(--codex-accent)' }} />
                            )}
                          </div>
                        </div>
                        <button
                          type="button"
                          title={t('mind_inspector.code_delete')}
                          onClick={(e) => { e.stopPropagation(); void handleDelete(s.session_id); }}
                          className="codex-session-delete"
                        >
                          <Trash2 size={11} />
                        </button>
                      </div>
                    );
                  })
                : [...sessionGroups.entries()].map(([dir, items]) => (
                    <div key={dir} className="codex-session-group">
                      <div className="codex-session-group-title">
                        <FolderOpen size={11} />
                        {renamingWorkspace === dir ? (
                          <input
                            autoFocus
                            value={renameDraft}
                            onChange={(e) => setRenameDraft(e.target.value)}
                            onKeyDown={(e) => {
                              if (e.key === 'Enter') commitRenameWorkspace();
                              if (e.key === 'Escape') setRenamingWorkspace(null);
                            }}
                            onBlur={commitRenameWorkspace}
                            className="codex-workspace-rename-input"
                          />
                        ) : (
                          <span>{workspaceDisplayName(dir)}</span>
                        )}
                        <div className="codex-session-group-actions">
                          <button
                            type="button"
                            className="codex-group-more-btn"
                            title={t('mind_inspector.code_workspace_ops')}
                            onClick={(e) => {
                              e.stopPropagation();
                              setWorkspaceMenuOpen((prev) => (prev === dir ? null : dir));
                            }}
                          >
                            <Ellipsis size={13} />
                          </button>
                          <button
                            type="button"
                            className="codex-group-add-btn"
                            title={t('mind_inspector.code_new_workspace_session')}
                            onClick={(e) => {
                              e.stopPropagation();
                              void createSessionInWorkspace(dir);
                            }}
                          >
                            <Plus size={12} />
                          </button>
                        </div>
                        {workspaceMenuOpen === dir && (
                          <div className="codex-group-menu" onClick={(e) => e.stopPropagation()}>
                            <button
                              type="button"
                              onClick={() => startRenameWorkspace(dir)}
                            >
                              {t('mind_inspector.code_rename')}
                            </button>
                            <button
                              type="button"
                              onClick={() => {
                                setWorkspaceMenuOpen(null);
                                void (async () => {
                                  const ok = await confirmDialog(
                                    t('mind_inspector.code_delete_workspace_confirm', { name: workspaceDisplayName(dir) }),
                                    { title: t('mind_inspector.code_delete_confirm_title', { defaultValue: '删除会话' }), kind: 'warning' },
                                  );
                                  if (ok) void deleteWorkspace(dir);
                                })();
                              }}
                            >
                              {t('mind_inspector.code_delete_workspace')}
                            </button>
                          </div>
                        )}
                      </div>
                      {items.map((s) => {
                        const active = s.session_id === activeId;
                        return (
                          <div
                            key={s.session_id}
                            onClick={() => switchSession(s)}
                            className={`codex-session-item ${active ? 'active' : ''}`}
                          >
                            {sessionSort === 'manual' && (
                              <div className="codex-session-manual">
                                <button
                                  type="button"
                                  title={t('mind_inspector.code_move_up')}
                                  onClick={(e) => { e.stopPropagation(); moveSession(s.session_id, -1); }}
                                >
                                  <ArrowUp size={10} />
                                </button>
                                <button
                                  type="button"
                                  title={t('mind_inspector.code_move_down')}
                                  onClick={(e) => { e.stopPropagation(); moveSession(s.session_id, 1); }}
                                >
                                  <ArrowDown size={10} />
                                </button>
                              </div>
                            )}
                            <div style={{ minWidth: 0, flex: 1 }}>
                              <div className="codex-session-title">
                                {s.title || t('mind_inspector.code_untitled', { defaultValue: '未命名' })}
                              </div>
                              <div className="codex-session-meta">
                                <FolderOpen size={11} />
                                <span>{s.working_directory.split(/[\\/]/).pop() || s.working_directory}</span>
                                {s.status === 'running' && (
                                  <Loader2 size={10} className="codex-spin" style={{ color: 'var(--codex-accent)' }} />
                                )}
                              </div>
                            </div>
                            <button
                              type="button"
                              title={t('mind_inspector.code_delete')}
                              onClick={(e) => { e.stopPropagation(); void handleDelete(s.session_id); }}
                              className="codex-session-delete"
                            >
                              <Trash2 size={11} />
                            </button>
                          </div>
                        );
                      })}
                    </div>
                  ))}
            </div>
          </div>

          {/* 工作区文件树：点击文件在右侧「预览」页打开 */}
          {activeSession?.working_directory ? (
            <div className="codex-sidebar-section codex-tree-section">
              <div className="codex-sidebar-label">
                <FolderTree size={12} />
                {t('mind_inspector.code_workspace_files', { defaultValue: '工作区文件' })}
              </div>
              <WorkspaceFileTree
                key={activeSession.session_id}
                root={activeSession.working_directory}
                onOpenFile={openPreview}
              />
            </div>
          ) : null}

        </div>
        )}
      </aside>

      {!leftCollapsed && (
        <div className="codex-resize-handle left" onMouseDown={(e) => startResize(e, 'left')} />
      )}

      {/* ===== 中央：对话区 ===== */}
      <main className="codex-main">
        <header className="codex-topbar">
          <div className="codex-path">
            <FolderOpen size={13} style={{ flexShrink: 0 }} />
            <span className="codex-topbar-context">
              <span className="codex-topbar-title">
                {activeSession?.title || t('mind_inspector.code_untitled', { defaultValue: '未命名' })}
              </span>
              <span className="codex-topbar-path" title={activeSession?.working_directory}>
                {activeSession
                  ? activeSession.working_directory
                  : t('mind_inspector.code_no_workspace', { defaultValue: '未选择工作区' })}
              </span>
            </span>
          </div>
          <div className="codex-topbar-actions">
            <ModeDropdown
              value={activeSession?.mode ?? 'standard'}
              onChange={(mode) => void handleSetMode(mode)}
              disabled={running || !activeSession}
            />
              {running && (
                <span className="codex-working">
                  {t('mind_inspector.code_working', { defaultValue: '工作中…' })}
                  {turnElapsed >= 15_000 && (
                    <span style={{ fontFamily: 'var(--codex-mono)', fontSize: 11.5, color: 'var(--codex-ink-faint)' }}>
                      {formatClock(turnElapsed)}
                    </span>
                  )}
                </span>
              )}
              <button
                type="button"
                onClick={toggleRightSidebar}
                title={rightCollapsed ? t('mind_inspector.code_expand_right') : t('mind_inspector.code_collapse_right')}
                aria-label={rightCollapsed ? t('mind_inspector.code_expand_right') : t('mind_inspector.code_collapse_right')}
                aria-expanded={!rightCollapsed}
                className="codex-icon-btn"
                style={{ background: rightCollapsed ? 'var(--codex-tape-yellow)' : 'var(--codex-tape-blue)' }}
              >
                {rightCollapsed ? <PanelRightOpen size={15} /> : <PanelRightClose size={15} />}
              </button>
            </div>
          </header>

        {pageError && (
          <div className="codex-page-error" role="alert">
            <span className="codex-page-error-text">{pageError}</span>
            <button
              type="button"
              className="codex-page-error-close"
              aria-label={t('mind_inspector.code_close', { defaultValue: '关闭' })}
              onClick={() => setPageError(null)}
            >
              <X size={14} />
            </button>
          </div>
        )}

        <div ref={scrollRef} onScroll={onScroll} className="codex-chat">
          {!activeSession ? (
            <div className="codex-empty">
              <div className="codex-hero">
                <div className="codex-hero-title">
                  {t('mind_inspector.code_hero_title', { defaultValue: '你想让 Vivian & Nana 帮你做什么？' })}
                </div>
                <div className="codex-hero-sub">{t('mind_inspector.code_hero_sub')}</div>
              </div>
              <div style={{ width: '100%', maxWidth: 780 }}>{composer}</div>
            </div>
          ) : messages.length === 0 ? (
            <div className="codex-empty">
              <div className="codex-hero">
                <div className="codex-hero-title">
                  <Code2 size={26} strokeWidth={1.5} />
                  {activeSession.title || t('mind_inspector.code_untitled', { defaultValue: '未命名' })}
                </div>
                <div className="codex-hero-sub">{activeSession.working_directory}</div>
              </div>
              <div style={{ width: '100%', maxWidth: 780 }}>{composer}</div>
            </div>
          ) : (
            <>
              <div className="codex-chat-inner">
                <GoalPlanBar
                  goal={activeSession?.goal ?? null}
                  plan={activeSession?.plan ?? null}
                  planMode={activeSession?.plan_mode ?? false}
                  onRun={(text) => sendContinuation(text)}
                />
                {groupChatMessages(messages, running).map((it) => {
                  if (it.kind === 'group') {
                    return (
                      <ToolProcessGroup
                        key={`grp-${it.index}`}
                        msgs={it.msgs}
                        settled={it.settled}
                        sessionRunning={running}
                        cwd={activeSession?.working_directory ?? ''}
                      />
                    );
                  }
                  const { msg, index: i } = it;
                  if (msg.role === 'tool_use' || msg.role === 'tool_result') {
                    // 后端聚合落库的"工具调用意图"消息（record_assistant_tool_calls：
                    // 无 tool_name / tool_call_id，tool_arguments 是整个调用数组）。
                    // 逐个调用的参数与结果已由 tool_result 消息完整承载，这里跳过渲染，
                    // 否则会出现一张空标题、正文为整段 JSON、永远"运行中"的大空块。
                    if (msg.role === 'tool_use' && !msg.tool_name) return null;
                    const wfRun =
                      msg.role === 'tool_result' &&
                      msg.tool_name === 'run_workflow' &&
                      tryParseWorkflowRun(msg.content);
                    if (wfRun) {
                      return <WorkflowVizCard key={`${msg.tool_call_id ?? i}-${i}`} run={wfRun} />;
                    }
                    const lsp = msg.role === 'tool_result' && msg.tool_name === 'lsp_query'
                      ? parseLspResult(msg.content)
                      : null;
                    if (lsp) {
                      return (
                        <LspVizCard
                          key={`${msg.tool_call_id ?? i}-${i}`}
                          parsed={lsp}
                          cwd={activeSession?.working_directory ?? ''}
                        />
                      );
                    }
                    return (
                      <ToolCallCard
                        key={`${msg.tool_call_id ?? i}-${i}`}
                        name={msg.tool_name ?? ''}
                        argumentsJson={
                          msg.tool_arguments
                            ? typeof msg.tool_arguments === 'string'
                              ? msg.tool_arguments
                              : JSON.stringify(msg.tool_arguments)
                            : ''
                        }
                        result={msg.role === 'tool_result' ? msg.content : undefined}
                        success={msg.tool_success ?? null}
                        running={msg.role === 'tool_use' && running}
                        durationMs={msg.tool_duration_ms ?? null}
                      />
                    );
                  }
                  return (
                    <MessageRow
                      key={i}
                      index={i}
                      msg={msg}
                      feedback={msgFeedback[i] ?? null}
                      changedFiles={msgChanges[i] ?? null}
                      onCopy={handleCopyMessage}
                      onFork={(idx) => void handleFork(idx)}
                      onFeedback={(idx, rating) => void handleFeedback(idx, rating)}
                      onOpenImage={(src, alt) => setLightbox({ src, alt })}
                    />
                  );
                })}
                {streamingText && (
                  <div className="codex-msg-assistant">
                    <MarkdownText text={streamingText} keyPrefix="stream" animate />
                    <span className="codex-cursor" />
                  </div>
                )}
                {thinking && (
                  <div className="codex-thinking">
                    <div className="codex-thinking-status">
                      <Braces size={15} strokeWidth={1.8} className="codex-breathe" style={{ color: 'var(--codex-ink-faint)' }} />
                      <span>
                        {compacting
                          ? t('mind_inspector.code_compacting', { defaultValue: '正在压缩上下文…' })
                          : t('mind_inspector.code_thinking', { defaultValue: '正在思考…' })}
                      </span>
                      <span className="codex-dots"><i /><i /><i /></span>
                    </div>
                    {thinkingText && !compacting && (
                      <div className="codex-thinking-chain">{thinkingText}</div>
                    )}
                  </div>
                )}
              </div>
            </>
          )}
          <div style={{ height: 16 }} />
        </div>

        {!atBottom && (
          <button type="button" title={t('mind_inspector.code_scroll_bottom')} onClick={scrollToBottom} className="codex-to-bottom">
            <ChevronDown size={15} />
          </button>
        )}

        {!isEmptyChat && (
          <div className="codex-composer-wrap">
            {budgetStopped && budgetProgress && (
              <BudgetStopBanner
                progress={budgetProgress}
                hint={budgetHint}
                onContinue={() => sendContinuation(t('mind_inspector.code_continue_task'))}
                onRefine={() => {
                  setBudgetHint(true);
                  inputRef.current?.focus();
                }}
                onStop={() => {
                  setBudgetStopped(false);
                  setBudgetHint(false);
                }}
              />
            )}
            {subagent || bgJobs > 0 ? (
              <div className="codex-subagent-bar">
                <Loader2 size={12} className="codex-breathe" />
                <span>
                  {subagent
                    ? t('mind_inspector.code_subagent_running')
                    : t('mind_inspector.code_bg_jobs_running', { n: bgJobs })}
                </span>
                <span className="codex-subagent-depth">
                  {subagent ? `L${subagent.depth}` : `${bgJobs}`}
                </span>
              </div>
            ) : null}
            {ask && ask.session_id === activeId ? (
              <WorkQuestionCard
                key={ask.question_id}
                question={ask}
                onDone={() => setAsk(null)}
              />
            ) : null}
            <div className="codex-composer-inner">{composer}</div>
          </div>
        )}
        <StatsLine stats={stats} />
      </main>

      {!rightCollapsed && (
        <div className="codex-resize-handle right" onMouseDown={(e) => startResize(e, 'right')} />
      )}

      {/* ===== 右侧：检查器（概览 / 轨迹 / 终端） ===== */}
      <aside className={`codex-inspector ${rightCollapsed ? 'collapsed' : ''}`} style={{ width: rightWidth }}>
          <div className="codex-inspector-tabs" ref={inspectorTabsRef}>
            <button
              type="button"
              onClick={() => setRightTab('overview')}
              className={`codex-inspector-tab ${rightTab === 'overview' ? 'active' : ''}`}
              title={t('mind_inspector.code_inspector_overview')}
            >
              <Activity size={14} />
              <span className="codex-inspector-tab-label">{t('mind_inspector.code_inspector_overview')}</span>
            </button>
            <button
              type="button"
              onClick={() => setRightTab('trajectory')}
              className={`codex-inspector-tab ${rightTab === 'trajectory' ? 'active' : ''}`}
              title={t('mind_inspector.code_inspector_trajectory')}
            >
              <GitFork size={14} />
              <span className="codex-inspector-tab-label">{t('mind_inspector.code_inspector_trajectory')}</span>
            </button>
            <button
              type="button"
              onClick={() => setRightTab('changes')}
              className={`codex-inspector-tab ${rightTab === 'changes' ? 'active' : ''}`}
              title={t('mind_inspector.code_tab_changes')}
            >
              <FileDiff size={14} />
              <span className="codex-inspector-tab-label">{t('mind_inspector.code_tab_changes')}</span>
            </button>
            <button
              type="button"
              onClick={() => setRightTab('preview')}
              className={`codex-inspector-tab ${rightTab === 'preview' ? 'active' : ''}`}
              title={t('mind_inspector.code_tab_preview')}
            >
              <FileText size={14} />
              <span className="codex-inspector-tab-label">{t('mind_inspector.code_tab_preview')}</span>
            </button>
            <button
              type="button"
              onClick={() => setRightTab('terminal')}
              className={`codex-inspector-tab ${rightTab === 'terminal' ? 'active' : ''}`}
              title={t('mind_inspector.code_tab_terminal')}
            >
              <TerminalIcon size={14} />
              <span className="codex-inspector-tab-label">{t('mind_inspector.code_tab_terminal')}</span>
            </button>
          </div>

          {rightTab === 'overview' ? (
            <div className="codex-inspector-pane">
              {/* 待办清单置顶（任务推进的核心） */}
              <WorkTodosCard key={activeId ?? 'none'} sessionId={activeId ?? ''} />

              {/* 上下文空间：上下文占用与构成占比 */}
              <ContextSpaceCard session={activeSession} />

              <DeliverablesCard cwd={activeSession?.working_directory ?? ''} deliverables={deliverables} />

              {queue.length > 0 && (
                <div className="codex-info-card">
                  <div className="codex-info-title">
                    <XCircle size={13} />
                    {t('mind_inspector.code_msg_queue')}
                  </div>
                  {queue.map((q, i) => (
                    <div key={`${i}-${q.slice(0, 12)}`} className="codex-info-row">
                      <span className="codex-info-key">#{i + 1}</span>
                      <span className="codex-info-value">{q}</span>
                    </div>
                  ))}
                </div>
              )}

              <div className="codex-info-card">
                <div className="codex-info-title">
                  <FolderOpen size={13} />
                  {t('mind_inspector.code_work_dir')}
                </div>
                <div className="codex-info-row">
                  <span className="codex-info-value" style={{ maxWidth: '100%', whiteSpace: 'normal' }}>
                    {activeSession ? activeSession.working_directory : t('mind_inspector.code_no_workspace', { defaultValue: '未选择工作区' })}
                  </span>
                </div>
              </div>
            </div>
          ) : rightTab === 'trajectory' ? (
            <div className="codex-inspector-pane codex-trajectory-pane">
              <TrajectoryPanel
                key={activeId ?? 'none'}
                messages={messages}
                running={running}
                toolLabel={(name) => toolMeta(name).label}
              />
            </div>
          ) : rightTab === 'changes' ? (
            <div className="codex-inspector-pane">
              <ChangeListCard changes={activeSession?.file_changes ?? []} />
            </div>
          ) : rightTab === 'preview' ? (
            <div className="codex-inspector-pane codex-preview-pane">
              <PreviewPanel
                baseKey={activeId ?? 'none'}
                tabs={previewTabs}
                activePath={activePreview}
                messages={messages}
                onSelect={setActivePreview}
                onClose={closePreview}
                onOpenFromChat={openPreview}
                target={previewTarget}
              />
            </div>
          ) : (
            <div className="codex-inspector-pane codex-terminal-pane">
              {termTabs.length > 0 ? (
                <>
                  <div className="codex-terminal-tabs" ref={termTabsRef}>
                    {termTabs.map((tab) => {
                      const active = tab.id === activeTermTab;
                      return (
                        <div key={tab.id} className={`codex-terminal-tab ${active ? 'active' : ''}`} onClick={() => setActiveTermTab(tab.id)}>
                          <span>{tab.label}</span>
                          <button
                            type="button"
                            title={t('mind_inspector.code_close_terminal')}
                            onClick={(e) => { e.stopPropagation(); handleCloseTermTab(tab.id); }}
                            className="codex-terminal-close"
                          >
                            <X size={11} />
                          </button>
                        </div>
                      );
                    })}
                    <button type="button" onClick={handleAddTermTab} className="codex-icon-btn" style={{ width: 24, height: 24 }} title={t('mind_inspector.code_new_terminal')}>
                      <Plus size={12} />
                    </button>
                  </div>
                  <div className="codex-terminal-body">
                    {termTabs.map((tab) => (
                      <div
                        key={tab.id}
                        className="codex-terminal-slot"
                        style={{ display: tab.id === activeTermTab ? 'block' : 'none' }}
                      >
                        <Suspense
                          fallback={
                            <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: 12, color: 'var(--codex-ink-faint)', fontSize: 13 }}>
                              <Loader2 size={14} className="codex-spin" />
                              {t('mind_inspector.code_terminal_loading')}
                            </div>
                          }
                        >
                          <TerminalPanel key={tab.id} workingDirectory={tab.workingDirectory} />
                        </Suspense>
                      </div>
                    ))}
                  </div>
                </>
              ) : null}
            </div>
          )}
        </aside>

      {/* ===== 覆盖层 ===== */}
      {dragActive && <DropOverlay />}
      {lightbox && (
        <ImageLightbox
          src={lightbox.src}
          alt={lightbox.alt}
          onClose={() => setLightbox(null)}
        />
      )}
      {slashMenu.visible && (
        <SlashCommandMenu
          position={slashMenu.position}
          query={slashMenu.query}
          onSelect={handleSlashCommandSelect}
          onClose={() => setSlashMenu((prev) => ({ ...prev, visible: false }))}
        />
      )}
      {atMenu.visible && (
        <FileRefMenu
          position={atMenu.position}
          query={atMenu.query}
          files={atFiles}
          onSelect={handleFileRefSelect}
          onClose={() => setAtMenu((prev) => ({ ...prev, visible: false }))}
        />
      )}
    </div>
    </MarkdownFileContext.Provider>
  );
};

export default CodeAgentPage;
