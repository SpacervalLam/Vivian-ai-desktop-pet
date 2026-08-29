/**
 * User Profile 页 — 用户画像查看与编辑（身份资料卡风格）
 *
 * 数据源：invoke('get_user_facts', { characterId })
 * 编辑：invoke('set_user_fact' / 'pin_user_fact' / 'delete_user_fact')
 *
 * 分层展示：
 * - 身份卡 Hero：姓名 + 关键标签（年龄/性别/职业/所在地）— 角色主题渐变
 * - L0 基础身份（姓名/年龄/性别/职业/所在地）— 可编辑、可锁定
 * - L0.5 结构化偏好（生日/作息/常用网站/喜欢的游戏/兴趣爱好）— 可编辑、可锁定
 * - L1 近期状态（最近目标/当前项目/近期偏好）— 只读，由对话中自动抽取
 * - L2 自由事实 — 可新增/删除
 *
 * 视觉风格：iOS 面板（磨砂玻璃 + continuous corners），与 WorldPage 一致。
 */

import React, { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { invoke } from '@tauri-apps/api/core';
import {
  Lock,
  LockOpen,
  Pencil,
  Check,
  X,
  Plus,
  Trash2,
  UserCircle,
  Sparkles,
  Target,
  Briefcase,
  Heart,
  Compass,
  Lightbulb,
  ThumbsUp,
  ThumbsDown,
  Clock,
} from 'lucide-react';
import {
  COLORS,
  TYPO,
  SPACING,
  RADIUS,
  EASE,
  DURATION,
  CHARACTER_ACCENT,
} from '../design-system';
import {
  Card,
  SectionTitle,
  EmptyState,
  Tag,
  IconButton,
} from '../shared-components';

// ============================================================
// 类型定义（与后端 commands/user_facts.rs 对齐）
// ============================================================

interface UserFactView {
  fact_type: string;
  label: string;
  content: string;
  confidence: number;
  timestamp: number;
  is_pinned: boolean;
  is_manual: boolean;
}

interface L1RecentState {
  recent_goals: string[];
  current_projects: string[];
  recent_preferences: string[];
  generated_at: number;
  round_count: number;
}

interface UserProfileView {
  basic_facts: UserFactView[];
  recent_state: L1RecentState;
  custom_facts: UserFactView[];
}

// 兴趣画像（与后端 commands/discovery.rs 对齐）
interface InterestDomainView {
  domain: string;
  weight: number;
  source: string;
  evidence_count: number;
  state: string;
}

interface ProbeView {
  domain: string;
  category: string;
  reason: string;
  specifics: string[];
  probe_mode: string;
  confidence: number;
  confirmation_count: number;
  confirmation_threshold: number;
}

interface DiscoveryProfileView {
  interests: InterestDomainView[];
  disliked_topics: string[];
  exploration_openness: number;
  probes: ProbeView[];
  store_available: number;
  store_total: number;
  store_recommended: number;
}

type CharacterId = 'vivian' | 'nana';

// L0 + L0.5 字段定义（顺序与后端 ordered_types 一致）
const BASIC_FIELD_DEFS: Array<{ type: string; layer: 'L0' | 'L0.5' }> = [
  { type: 'name', layer: 'L0' },
  { type: 'age', layer: 'L0' },
  { type: 'gender', layer: 'L0' },
  { type: 'occupation', layer: 'L0' },
  { type: 'location', layer: 'L0' },
  { type: 'birthday', layer: 'L0.5' },
  { type: 'sleep_schedule', layer: 'L0.5' },
  { type: 'favorite_website', layer: 'L0.5' },
  { type: 'favorite_game', layer: 'L0.5' },
  { type: 'hobby', layer: 'L0.5' },
];

// 入场揭示动画关键帧（一次性注入，并尊重系统“减弱动态效果”设置）
const REVEAL_KEYFRAMES_ID = 'mind-inspector-profile-reveal';
if (
  typeof document !== 'undefined' &&
  !document.getElementById(REVEAL_KEYFRAMES_ID)
) {
  const style = document.createElement('style');
  style.id = REVEAL_KEYFRAMES_ID;
  style.textContent = `
@keyframes mind-inspector-rise {
  from { opacity: 0; transform: translateY(10px); }
  to { opacity: 1; transform: translateY(0); }
}
@media (prefers-reduced-motion: reduce) {
  #mind-inspector-profile-root [data-reveal] {
    animation: none !important;
    opacity: 1 !important;
  }
}`;
  document.head.appendChild(style);
}

// 入场封装：为子区块提供渐次浮现动画（delay 为秒）
const Reveal: React.FC<{ delay?: number; style?: React.CSSProperties; children?: React.ReactNode }> = ({
  delay = 0,
  style,
  children,
}) => (
  <div
    data-reveal
    style={{
      animation: `mind-inspector-rise ${DURATION.slow}s ${EASE.decel} both`,
      animationDelay: `${delay}s`,
      ...style,
    }}
  >
    {children}
  </div>
);

// ============================================================
// 工具函数
// ============================================================

const toMs = (ts: number): number => (ts < 1e12 ? ts * 1000 : ts);

const formatRelative = (ts: number, t: TFunction): string => {
  if (!ts || ts <= 0) return '—';
  const diff = Math.max(0, Date.now() - toMs(ts));
  const min = 60_000;
  const hour = 3_600_000;
  const day = 86_400_000;
  if (diff < min) return t('mind_inspector.common.just_now');
  if (diff < hour) return t('mind_inspector.common.minutes_ago', { n: Math.floor(diff / min) });
  if (diff < day) return t('mind_inspector.common.hours_ago', { n: Math.floor(diff / hour) });
  if (diff < 7 * day) return t('mind_inspector.common.days_ago', { n: Math.floor(diff / day) });
  const d = new Date(toMs(ts));
  const pad = (x: number) => String(x).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
};

// ============================================================
// CharacterTabs — 角色切换（胶囊风格）
// ============================================================

interface CharacterTabsProps {
  character: CharacterId;
  setCharacter: (c: CharacterId) => void;
  t: TFunction;
}

const CharacterTabs: React.FC<CharacterTabsProps> = ({ character, setCharacter, t }) => {
  const chars: CharacterId[] = ['vivian', 'nana'];
  return (
    <div
      style={{
        display: 'inline-flex',
        gap: 4,
        padding: 4,
        borderRadius: RADIUS.pill,
        background: COLORS.subtleBg,
        border: `1px solid ${COLORS.subtleBorder}`,
      }}
    >
      {chars.map((c) => {
        const active = c === character;
        const accent = CHARACTER_ACCENT[c];
        return (
          <button
            key={c}
            type="button"
            onClick={() => setCharacter(c)}
            style={{
              padding: '6px 18px',
              border: 'none',
              borderRadius: RADIUS.pill,
              cursor: 'pointer',
              background: active ? `${accent}22` : 'transparent',
              color: active ? accent : COLORS.textSecondary,
              fontFamily: TYPO.fontFamily,
              fontSize: 14,
              fontWeight: active ? 600 : 500,
              transition: `all ${DURATION.normal}s ${EASE.swift}`,
            }}
          >
            {t(`mind_inspector.common.char_${c}`)}
          </button>
        );
      })}
    </div>
  );
};

// ============================================================
// RowDivider — 详情列表行之间的细分割线
// ============================================================

const RowDivider: React.FC = () => (
  <div
    aria-hidden
    style={{
      height: 1,
      marginLeft: 122,
      marginRight: SPACING.cardPadding,
      background: COLORS.border,
    }}
  />
);

// ============================================================
// IdentityRow — 单条资料字段（详情列表行：标签 + 值 + 内联编辑 + 锁定）
// ============================================================

interface IdentityRowProps {
  fact: UserFactView | null;
  label: string;
  placeholder: string;
  onSave: (content: string) => Promise<void>;
  onTogglePin: () => Promise<void>;
  saving: boolean;
}

const IdentityRow: React.FC<IdentityRowProps> = ({
  fact,
  label,
  placeholder,
  onSave,
  onTogglePin,
  saving,
}) => {
  const { t } = useTranslation();
  const [hovered, setHovered] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editing && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editing]);

  const startEdit = () => {
    setDraft(fact?.content ?? '');
    setEditing(true);
  };

  const cancelEdit = () => {
    setEditing(false);
    setDraft('');
  };

  const confirmEdit = async () => {
    const trimmed = draft.trim();
    if (!trimmed || trimmed === fact?.content) {
      setEditing(false);
      return;
    }
    await onSave(trimmed);
    setEditing(false);
  };

  const isEmpty = !fact || !fact.content;
  const isPinned = fact?.is_pinned ?? false;
  const showActions = hovered || editing || isPinned || fact?.is_manual;

  return (
    <div
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: SPACING.sm,
        padding: `${SPACING.sm + 2}px ${SPACING.cardPadding}px`,
        minHeight: 44,
        background: hovered && !editing ? COLORS.bgHover : 'transparent',
        transition: `background ${DURATION.fast}s ${EASE.swift}`,
      }}
    >
      {/* 标签 */}
      <span
        style={{
          ...TYPO.micro,
          color: COLORS.textTertiary,
          minWidth: 96,
          flexShrink: 0,
        }}
      >
        {label}
      </span>

      {/* 内容 / 编辑框 */}
      {editing ? (
        <input
          ref={inputRef}
          type="text"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void confirmEdit();
            if (e.key === 'Escape') cancelEdit();
          }}
          placeholder={placeholder}
          style={{
            flex: 1,
            minWidth: 0,
            padding: '4px 8px',
            border: `1px solid ${COLORS.accent}`,
            borderRadius: RADIUS.xs,
            background: COLORS.bgDeep,
            color: COLORS.textPrimary,
            fontFamily: TYPO.fontFamily,
            fontSize: 14,
            outline: 'none',
          }}
        />
      ) : (
        <span
          onDoubleClick={startEdit}
          title={isEmpty ? placeholder : fact!.content}
          style={{
            flex: 1,
            minWidth: 0,
            ...TYPO.body,
            color: isEmpty ? COLORS.textQuaternary : COLORS.textPrimary,
            cursor: 'text',
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            whiteSpace: 'nowrap',
          }}
        >
          {isEmpty ? placeholder : fact!.content}
        </span>
      )}

      {/* 来源/时间标记 */}
      {!editing && !isEmpty && (
        <span
          style={{
            display: 'inline-flex',
            alignItems: 'center',
            gap: 4,
            flexShrink: 0,
            ...TYPO.micro,
            color: COLORS.textQuaternary,
            opacity: hovered ? 1 : 0.6,
            transition: `opacity ${DURATION.fast}s ${EASE.swift}`,
          }}
        >
          {fact!.is_manual && (
            <Tag color={COLORS.accent} style={{ padding: '1px 6px', fontSize: 10 }}>
              {t('mind_inspector.profile.manual_badge')}
            </Tag>
          )}
          {formatRelative(fact!.timestamp, t)}
        </span>
      )}

      {/* 操作按钮（hover / 锁定 / 手动时显示） */}
      <div
        style={{
          display: 'flex',
          gap: 2,
          flexShrink: 0,
          opacity: showActions ? 1 : 0,
          transition: `opacity ${DURATION.fast}s ${EASE.swift}`,
        }}
      >
        {editing ? (
          <>
            <IconButton
              onClick={confirmEdit}
              title={t('mind_inspector.common.success')}
              disabled={saving}
            >
              <Check size={15} />
            </IconButton>
            <IconButton onClick={cancelEdit} title={t('mind_inspector.profile.cancel')}>
              <X size={15} />
            </IconButton>
          </>
        ) : (
          <>
            <IconButton onClick={startEdit} title={t('mind_inspector.profile.edit')}>
              <Pencil size={14} />
            </IconButton>
            <IconButton
              onClick={onTogglePin}
              title={t(isPinned ? 'mind_inspector.profile.unlock' : 'mind_inspector.profile.lock')}
              active={isPinned}
            >
              {isPinned ? <Lock size={14} /> : <LockOpen size={14} />}
            </IconButton>
          </>
        )}
      </div>
    </div>
  );
};

// ============================================================
// CustomFactItem — 自由事实条目（可删除）
// ============================================================

interface CustomFactItemProps {
  fact: UserFactView;
  onDelete: () => Promise<void>;
  deleting: boolean;
}

const CustomFactItem: React.FC<CustomFactItemProps> = ({ fact, onDelete, deleting }) => {
  const { t } = useTranslation();
  const [hovered, setHovered] = useState(false);
  return (
    <div
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: SPACING.sm,
        padding: `${SPACING.sm}px ${SPACING.md}px`,
        borderRadius: RADIUS.md,
        background: COLORS.bgSurface,
        border: `1px solid ${COLORS.subtleBorder}`,
        transition: `border-color ${DURATION.fast}s ${EASE.swift}`,
      }}
    >
      <span
        style={{
          flex: 1,
          minWidth: 0,
          ...TYPO.body,
          color: COLORS.textPrimary,
          wordBreak: 'break-word',
        }}
      >
        {fact.content}
      </span>
      <span
        style={{
          ...TYPO.micro,
          color: COLORS.textQuaternary,
          flexShrink: 0,
        }}
      >
        {formatRelative(fact.timestamp, t)}
      </span>
      <IconButton
        onClick={onDelete}
        title={t('mind_inspector.profile.delete')}
        disabled={deleting}
        style={{ color: hovered ? COLORS.danger : undefined }}
      >
        <Trash2 size={14} />
      </IconButton>
    </div>
  );
};

// ============================================================
// L1StateCard — 近期状态卡片（只读）
// ============================================================

interface L1StateCardProps {
  icon: React.ReactNode;
  title: string;
  items: string[];
  emptyText: string;
  accent: string;
}

const L1StateCard: React.FC<L1StateCardProps> = ({
  icon,
  title,
  items,
  emptyText,
  accent,
}) => (
  <Card style={{ flex: 1, minWidth: 0 }}>
    <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs, marginBottom: SPACING.sm }}>
      <span style={{ color: accent, display: 'inline-flex' }}>{icon}</span>
      <span style={{ ...TYPO.h3, color: COLORS.textPrimary }}>{title}</span>
      <span
        style={{
          ...TYPO.micro,
          color: COLORS.textQuaternary,
          marginLeft: 'auto',
        }}
      >
        {items.length}
      </span>
    </div>
    {items.length === 0 ? (
      <div style={{ ...TYPO.body, color: COLORS.textQuaternary, fontSize: 13 }}>
        {emptyText}
      </div>
    ) : (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        {items.map((item, i) => (
          <div
            key={`${title}-${i}`}
            style={{
              display: 'flex',
              alignItems: 'flex-start',
              gap: SPACING.xs,
              padding: `${SPACING.xs}px ${SPACING.sm}px`,
              borderRadius: RADIUS.xs,
              background: COLORS.bgSurface,
            }}
          >
            <span
              aria-hidden
              style={{
                marginTop: 6,
                width: 5,
                height: 5,
                borderRadius: RADIUS.pill,
                background: accent,
                flexShrink: 0,
              }}
            />
            <span
              style={{
                ...TYPO.body,
                fontSize: 13.5,
                color: COLORS.textSecondary,
                wordBreak: 'break-word',
              }}
            >
              {item}
            </span>
          </div>
        ))}
      </div>
    )}
  </Card>
);

// ============================================================
// AddCustomFact — 新增自由事实输入条
// ============================================================

const AddCustomFact: React.FC<{ onAdd: (content: string) => Promise<void> }> = ({ onAdd }) => {
  const { t } = useTranslation();
  const [value, setValue] = useState('');
  const [adding, setAdding] = useState(false);

  const submit = async () => {
    const trimmed = value.trim();
    if (!trimmed) return;
    setAdding(true);
    try {
      await onAdd(trimmed);
      setValue('');
    } finally {
      setAdding(false);
    }
  };

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: SPACING.xs,
        padding: `${SPACING.sm}px ${SPACING.md}px`,
        borderRadius: RADIUS.md,
        border: `1px dashed ${COLORS.border}`,
        background: 'transparent',
      }}
    >
      <Plus size={16} style={{ color: COLORS.textTertiary, flexShrink: 0 }} />
      <input
        type="text"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') void submit();
        }}
        placeholder={t('mind_inspector.profile.add_custom_placeholder')}
        style={{
          flex: 1,
          minWidth: 0,
          border: 'none',
          background: 'transparent',
          color: COLORS.textPrimary,
          fontFamily: TYPO.fontFamily,
          fontSize: 14,
          outline: 'none',
        }}
      />
      {value.trim() && (
        <IconButton onClick={submit} title={t('mind_inspector.profile.add')} disabled={adding} active>
          <Check size={15} />
        </IconButton>
      )}
    </div>
  );
};

// ============================================================
// DiscoverySection — 兴趣画像分区（兴趣域 / 探针 / 不喜欢 / 开放度）
// ============================================================

const SOURCE_LABEL_KEY: Record<string, string> = {
  seed: 'mind_inspector.profile.discovery_source_seed',
  feedback: 'mind_inspector.profile.discovery_source_feedback',
  probe: 'mind_inspector.profile.discovery_source_probe',
};

/** 权重条 + 兴趣域行（hover 显示删除） */
const InterestRow: React.FC<{
  interest: InterestDomainView;
  accent: string;
  onDelete: () => void;
  deleting: boolean;
  t: TFunction;
}> = ({ interest, accent, onDelete, deleting, t }) => {
  const [hovered, setHovered] = useState(false);
  const pct = Math.round(interest.weight * 100);
  return (
    <div
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: SPACING.sm,
        padding: `${SPACING.sm}px ${SPACING.md}px`,
        borderRadius: RADIUS.md,
        background: COLORS.bgSurface,
        border: `1px solid ${COLORS.subtleBorder}`,
        transition: `border-color ${DURATION.fast}s ${EASE.swift}`,
      }}
    >
      <span
        style={{
          ...TYPO.body,
          color: COLORS.textPrimary,
          minWidth: 72,
          flexShrink: 0,
          wordBreak: 'break-word',
        }}
      >
        {interest.domain}
      </span>
      {/* 权重条 */}
      <div
        style={{
          flex: 1,
          minWidth: 0,
          height: 6,
          borderRadius: RADIUS.pill,
          background: COLORS.subtleBg,
          overflow: 'hidden',
        }}
      >
        <div
          style={{
            width: `${pct}%`,
            height: '100%',
            borderRadius: RADIUS.pill,
            background: accent,
            opacity: 0.55 + interest.weight * 0.45,
            transition: `width ${DURATION.normal}s ${EASE.swift}`,
          }}
        />
      </div>
      <span style={{ ...TYPO.micro, color: COLORS.textQuaternary, flexShrink: 0, width: 34, textAlign: 'right' }}>
        {pct}%
      </span>
      <Tag
        color={interest.source === 'probe' ? accent : COLORS.textTertiary}
        style={{ flexShrink: 0, padding: '1px 6px', fontSize: 10 }}
      >
        {t(SOURCE_LABEL_KEY[interest.source] ?? 'mind_inspector.profile.discovery_source_seed')}
      </Tag>
      <IconButton
        onClick={onDelete}
        title={t('mind_inspector.profile.delete')}
        disabled={deleting}
        style={{ color: hovered ? COLORS.danger : undefined, opacity: hovered ? 1 : 0.35 }}
      >
        <Trash2 size={13} />
      </IconButton>
    </div>
  );
};

/** 兴趣探针卡片（reason + specifics + 三态回应按钮） */
const ProbeCard: React.FC<{
  probe: ProbeView;
  accent: string;
  onRespond: (response: 'confirm' | 'reject' | 'defer') => void;
  busy: boolean;
  t: TFunction;
}> = ({ probe, accent, onRespond, busy, t }) => (
  <Card
    style={{
      display: 'flex',
      flexDirection: 'column',
      gap: SPACING.sm,
      borderColor: `${accent}44`,
    }}
  >
    <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs }}>
      <Lightbulb size={15} style={{ color: accent, flexShrink: 0 }} />
      <span style={{ ...TYPO.body, color: COLORS.textPrimary, fontWeight: 600 }}>
        {probe.domain}
      </span>
      <Tag color={accent} style={{ padding: '1px 6px', fontSize: 10 }}>
        {probe.probe_mode}
      </Tag>
      <span
        style={{
          ...TYPO.micro,
          color: COLORS.textQuaternary,
          marginLeft: 'auto',
          flexShrink: 0,
        }}
      >
        {probe.confirmation_count}/{probe.confirmation_threshold}
      </span>
    </div>
    <div style={{ ...TYPO.body, fontSize: 13, color: COLORS.textSecondary, lineHeight: 1.55 }}>
      {probe.reason}
    </div>
    {probe.specifics.length > 0 && (
      <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap' }}>
        {probe.specifics.map((s, i) => (
          <Tag key={i} color={COLORS.textTertiary} style={{ padding: '1px 8px', fontSize: 10, fontWeight: 400 }}>
            {s}
          </Tag>
        ))}
      </div>
    )}
    <div style={{ display: 'flex', gap: SPACING.xs, marginTop: 2 }}>
      <button
        type="button"
        disabled={busy}
        onClick={() => onRespond('confirm')}
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          gap: 5,
          padding: '5px 12px',
          border: `1px solid ${accent}55`,
          borderRadius: RADIUS.pill,
          background: `${accent}18`,
          color: accent,
          fontFamily: TYPO.fontFamily,
          fontSize: 12.5,
          fontWeight: 600,
          cursor: busy ? 'not-allowed' : 'pointer',
          opacity: busy ? 0.5 : 1,
        }}
      >
        <ThumbsUp size={13} />
        {t('mind_inspector.profile.probe_confirm')}
      </button>
      <button
        type="button"
        disabled={busy}
        onClick={() => onRespond('reject')}
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          gap: 5,
          padding: '5px 12px',
          border: `1px solid ${COLORS.subtleBorder}`,
          borderRadius: RADIUS.pill,
          background: 'transparent',
          color: COLORS.textSecondary,
          fontFamily: TYPO.fontFamily,
          fontSize: 12.5,
          fontWeight: 500,
          cursor: busy ? 'not-allowed' : 'pointer',
          opacity: busy ? 0.5 : 1,
        }}
      >
        <ThumbsDown size={13} />
        {t('mind_inspector.profile.probe_reject')}
      </button>
      <button
        type="button"
        disabled={busy}
        onClick={() => onRespond('defer')}
        style={{
          display: 'inline-flex',
          alignItems: 'center',
          gap: 5,
          padding: '5px 12px',
          border: `1px solid ${COLORS.subtleBorder}`,
          borderRadius: RADIUS.pill,
          background: 'transparent',
          color: COLORS.textSecondary,
          fontFamily: TYPO.fontFamily,
          fontSize: 12.5,
          fontWeight: 500,
          cursor: busy ? 'not-allowed' : 'pointer',
          opacity: busy ? 0.5 : 1,
        }}
      >
        <Clock size={13} />
        {t('mind_inspector.profile.probe_defer')}
      </button>
    </div>
  </Card>
);

const DiscoverySection: React.FC<{
  character: CharacterId;
  accent: string;
  t: TFunction;
}> = ({ character, accent, t }) => {
  const [data, setData] = useState<DiscoveryProfileView | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [dislikeDraft, setDislikeDraft] = useState('');
  const [bangumiUser, setBangumiUser] = useState('');
  const [bangumiMsg, setBangumiMsg] = useState<string | null>(null);

  const load = useCallback(() => {
    setLoading(true);
    setError(null);
    invoke<DiscoveryProfileView>('get_discovery_profile', { characterId: character })
      .then((res) => setData(res))
      .catch((e) => setError(String(e)))
      .finally(() => setLoading(false));
  }, [character]);

  useEffect(() => {
    setData(null);
    load();
  }, [load]);

  const handleDeleteInterest = async (domain: string) => {
    setBusyKey(`interest-${domain}`);
    try {
      await invoke('remove_discovery_interest', { characterId: character, domain });
      load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const handleRemoveDislike = async (topic: string) => {
    setBusyKey(`dislike-${topic}`);
    try {
      await invoke('remove_discovery_dislike', { characterId: character, topic });
      load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const handleAddDislike = async () => {
    const trimmed = dislikeDraft.trim();
    if (!trimmed) return;
    setBusyKey('add-dislike');
    try {
      await invoke('add_discovery_dislike', { characterId: character, topic: trimmed });
      setDislikeDraft('');
      load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const handleProbeRespond = async (domain: string, response: string) => {
    setBusyKey(`probe-${domain}`);
    try {
      await invoke('respond_interest_probe', {
        characterId: character,
        domain,
        response,
      });
      load();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const handleBangumiImport = async () => {
    const username = bangumiUser.trim();
    if (!username) return;
    setBusyKey('bangumi-import');
    setBangumiMsg(null);
    try {
      const domains = await invoke<string[]>('bootstrap_from_bangumi', {
        characterId: character,
        username,
      });
      setBangumiMsg(t('mind_inspector.profile.bangumi_import_done', { n: domains.length }));
      load();
    } catch (e) {
      setBangumiMsg(String(e));
    } finally {
      setBusyKey(null);
    }
  };

  if (loading && !data) {
    return (
      <section>
        <SectionTitle style={{ marginBottom: SPACING.sm }}>
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
            <Compass size={14} />
            {t('mind_inspector.profile.section_discovery')}
          </span>
        </SectionTitle>
        <EmptyState spinner text={t('mind_inspector.common.loading')} style={{ padding: SPACING.md }} />
      </section>
    );
  }

  if (error && !data) {
    return (
      <section>
        <SectionTitle style={{ marginBottom: SPACING.sm }}>
          <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
            <Compass size={14} />
            {t('mind_inspector.profile.section_discovery')}
          </span>
        </SectionTitle>
        <EmptyState
          text={t('mind_inspector.common.load_failed', { error })}
          style={{ padding: SPACING.md }}
        />
      </section>
    );
  }

  const opennessPct = Math.round(data!.exploration_openness * 100);

  return (
    <section>
      <SectionTitle style={{ marginBottom: SPACING.sm }}>
        <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
          <Compass size={14} />
          {t('mind_inspector.profile.section_discovery')}
          <span
            style={{
              ...TYPO.micro,
              color: COLORS.textQuaternary,
              fontWeight: 400,
              marginLeft: SPACING.xs,
              textTransform: 'none',
              letterSpacing: 0,
            }}
          >
            {t('mind_inspector.profile.discovery_meta', {
              available: data!.store_available,
              recommended: data!.store_recommended,
            })}
          </span>
        </span>
      </SectionTitle>

      {error && (
        <div style={{ ...TYPO.micro, color: COLORS.danger, marginBottom: SPACING.sm }}>
          {t('mind_inspector.common.failed')}: {error}
        </div>
      )}

      {/* 兴趣域 */}
      {data!.interests.length === 0 ? (
        <EmptyState
          text={t('mind_inspector.profile.discovery_interests_empty')}
          style={{ padding: SPACING.md }}
        />
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: SPACING.xs, marginBottom: SPACING.md }}>
          {data!.interests.map((it) => (
            <InterestRow
              key={it.domain}
              interest={it}
              accent={accent}
              onDelete={() => handleDeleteInterest(it.domain)}
              deleting={busyKey === `interest-${it.domain}`}
              t={t}
            />
          ))}
        </div>
      )}

      {/* Bangumi 公开收藏导入 */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: SPACING.xs,
          padding: `${SPACING.xs}px ${SPACING.sm}px`,
          borderRadius: RADIUS.md,
          border: `1px dashed ${COLORS.border}`,
          marginBottom: SPACING.md,
        }}
      >
        <Compass size={14} style={{ color: COLORS.textTertiary, flexShrink: 0 }} />
        <input
          type="text"
          value={bangumiUser}
          onChange={(e) => setBangumiUser(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') void handleBangumiImport();
          }}
          placeholder={t('mind_inspector.profile.bangumi_import_placeholder')}
          style={{
            flex: 1,
            minWidth: 0,
            border: 'none',
            background: 'transparent',
            color: COLORS.textPrimary,
            fontFamily: TYPO.fontFamily,
            fontSize: 13,
            outline: 'none',
          }}
        />
        {bangumiUser.trim() && (
          <IconButton
            onClick={handleBangumiImport}
            disabled={busyKey === 'bangumi-import'}
            active
            title={t('mind_inspector.profile.bangumi_import')}
          >
            <Check size={14} />
          </IconButton>
        )}
        {bangumiMsg && (
          <span style={{ ...TYPO.micro, color: COLORS.textQuaternary, flexShrink: 0 }}>
            {bangumiMsg}
          </span>
        )}
      </div>

      {/* 兴趣探针 */}
      {data!.probes.length > 0 && (
        <>
          <div
            style={{
              ...TYPO.micro,
              color: COLORS.textTertiary,
              marginBottom: SPACING.sm,
              textTransform: 'none',
              letterSpacing: 0,
            }}
          >
            {t('mind_inspector.profile.probe_subtitle')}
          </div>
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'repeat(auto-fill, minmax(300px, 1fr))',
              gap: SPACING.sm,
              marginBottom: SPACING.md,
            }}
          >
            {data!.probes.map((p) => (
              <ProbeCard
                key={p.domain}
                probe={p}
                accent={accent}
                onRespond={(resp) => handleProbeRespond(p.domain, resp)}
                busy={busyKey === `probe-${p.domain}`}
                t={t}
              />
            ))}
          </div>
        </>
      )}

      {/* 不喜欢主题 + 探索开放度 */}
      <div style={{ display: 'flex', gap: SPACING.cardGap, flexWrap: 'wrap' }}>
        <Card style={{ flex: 1.4, minWidth: 260 }}>
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: SPACING.xs,
              marginBottom: SPACING.sm,
            }}
          >
            <ThumbsDown size={14} style={{ color: COLORS.danger }} />
            <span style={{ ...TYPO.h3, color: COLORS.textPrimary }}>
              {t('mind_inspector.profile.discovery_dislikes')}
            </span>
          </div>
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: SPACING.xs,
              padding: `${SPACING.xs}px ${SPACING.sm}px`,
              borderRadius: RADIUS.md,
              border: `1px dashed ${COLORS.border}`,
              marginBottom: data!.disliked_topics.length > 0 ? SPACING.sm : 0,
            }}
          >
            <Plus size={14} style={{ color: COLORS.textTertiary, flexShrink: 0 }} />
            <input
              type="text"
              value={dislikeDraft}
              onChange={(e) => setDislikeDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void handleAddDislike();
              }}
              placeholder={t('mind_inspector.profile.discovery_add_dislike_placeholder')}
              style={{
                flex: 1,
                minWidth: 0,
                border: 'none',
                background: 'transparent',
                color: COLORS.textPrimary,
                fontFamily: TYPO.fontFamily,
                fontSize: 13,
                outline: 'none',
              }}
            />
            {dislikeDraft.trim() && (
              <IconButton onClick={handleAddDislike} disabled={busyKey === 'add-dislike'} active>
                <Check size={14} />
              </IconButton>
            )}
          </div>
          {data!.disliked_topics.length === 0 ? (
            <div style={{ ...TYPO.body, fontSize: 13, color: COLORS.textQuaternary }}>
              {t('mind_inspector.profile.discovery_dislikes_empty')}
            </div>
          ) : (
            <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
              {data!.disliked_topics.map((topic) => (
                <Tag
                  key={topic}
                  color={COLORS.danger}
                  style={{ cursor: 'pointer', padding: '2px 8px' }}
                >
                  <span
                    onClick={() => handleRemoveDislike(topic)}
                    title={t('mind_inspector.profile.delete')}
                    style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}
                  >
                    {topic}
                    <X size={11} />
                  </span>
                </Tag>
              ))}
            </div>
          )}
        </Card>

        <Card style={{ flex: 1, minWidth: 200 }}>
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: SPACING.xs,
              marginBottom: SPACING.sm,
            }}
          >
            <Sparkles size={14} style={{ color: accent }} />
            <span style={{ ...TYPO.h3, color: COLORS.textPrimary }}>
              {t('mind_inspector.profile.discovery_openness')}
            </span>
          </div>
          <div
            style={{
              height: 6,
              borderRadius: RADIUS.pill,
              background: COLORS.subtleBg,
              overflow: 'hidden',
              marginBottom: SPACING.xs,
            }}
          >
            <div
              style={{
                width: `${opennessPct}%`,
                height: '100%',
                borderRadius: RADIUS.pill,
                background: accent,
                transition: `width ${DURATION.normal}s ${EASE.swift}`,
              }}
            />
          </div>
          <div style={{ ...TYPO.micro, color: COLORS.textQuaternary }}>
            {t('mind_inspector.profile.discovery_openness_hint', { n: opennessPct })}
          </div>
        </Card>
      </div>
    </section>
  );
};

// ============================================================
// UserProfilePage
// ============================================================

const UserProfilePage: React.FC = () => {
  const { t } = useTranslation();
  const [character, setCharacter] = useState<CharacterId>('vivian');
  const [profile, setProfile] = useState<UserProfileView | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyField, setBusyField] = useState<string | null>(null);

  const requestSeq = useRef(0);

  const loadProfile = useCallback((charId: CharacterId) => {
    const seq = ++requestSeq.current;
    setLoading(true);
    setError(null);
    invoke<UserProfileView>('get_user_facts', { characterId: charId })
      .then((res) => {
        if (seq !== requestSeq.current) return;
        setProfile(res);
      })
      .catch((e) => {
        if (seq === requestSeq.current) setError(String(e));
      })
      .finally(() => {
        if (seq === requestSeq.current) setLoading(false);
      });
  }, []);

  useEffect(() => {
    setProfile(null);
    loadProfile(character);
  }, [character, loadProfile]);

  // === 编辑操作 ===
  const handleSaveFact = async (factType: string, content: string) => {
    setBusyField(factType);
    try {
      await invoke('set_user_fact', {
        characterId: character,
        factType,
        content,
        pinned: null,
      });
      loadProfile(character);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyField(null);
    }
  };

  const handleTogglePin = async (factType: string, currentPinned: boolean) => {
    setBusyField(`pin-${factType}`);
    try {
      await invoke('pin_user_fact', {
        characterId: character,
        factType,
        pinned: !currentPinned,
      });
      loadProfile(character);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyField(null);
    }
  };

  const handleDeleteCustom = async (content: string) => {
    setBusyField(`del-${content}`);
    try {
      await invoke('delete_user_fact', {
        characterId: character,
        factType: 'custom',
        content,
      });
      loadProfile(character);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyField(null);
    }
  };

  const handleAddCustom = async (content: string) => {
    setBusyField('add-custom');
    try {
      await invoke('set_user_fact', {
        characterId: character,
        factType: 'custom',
        content,
        pinned: null,
      });
      loadProfile(character);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyField(null);
    }
  };

  // === 派生数据 ===
  const basicMap = useMemo(() => {
    const m = new Map<string, UserFactView>();
    if (profile) {
      for (const f of profile.basic_facts) m.set(f.fact_type, f);
    }
    return m;
  }, [profile]);

  const l0Fields = BASIC_FIELD_DEFS.filter((d) => d.layer === 'L0');
  const l05Fields = BASIC_FIELD_DEFS.filter((d) => d.layer === 'L0.5');

  const accent = CHARACTER_ACCENT[character];
  const l1 = profile?.recent_state;
  const hasL1Data =
    !!l1 &&
    (l1.recent_goals.length > 0 ||
      l1.current_projects.length > 0 ||
      l1.recent_preferences.length > 0);

  // === 渲染：加载中 ===
  if (loading && !profile) {
    return (
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column', gap: SPACING.md }}>
        <CharacterTabs character={character} setCharacter={setCharacter} t={t} />
        <EmptyState
          spinner
          text={t('mind_inspector.common.loading')}
          style={{ flex: 1 }}
        />
      </div>
    );
  }

  // === 渲染：加载失败 ===
  if (error && !profile) {
    return (
      <div style={{ flex: 1, display: 'flex', flexDirection: 'column', gap: SPACING.md }}>
        <CharacterTabs character={character} setCharacter={setCharacter} t={t} />
        <EmptyState
          icon={<UserCircle size={32} />}
          text={t('mind_inspector.common.load_failed', { error })}
          style={{ flex: 1 }}
        />
      </div>
    );
  }

  return (
    <div
      id="mind-inspector-profile-root"
      style={{
        flex: 1,
        display: 'flex',
        flexDirection: 'column',
        gap: SPACING.lg,
        minHeight: 0,
        overflowY: 'auto',
        paddingRight: 4,
      }}
    >
      {/* 顶部：角色切换 + 错误提示 */}
      <Reveal delay={0}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: SPACING.md }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.md }}>
            <CharacterTabs character={character} setCharacter={setCharacter} t={t} />
            <span style={{ ...TYPO.body, color: COLORS.textTertiary, fontSize: 13.5 }}>
              {t('mind_inspector.profile.subtitle', {
                char: t(`mind_inspector.common.char_${character}`),
              })}
            </span>
          </div>
          {error && (
            <span style={{ ...TYPO.micro, color: COLORS.danger }}>
              {t('mind_inspector.common.failed')}: {error}
            </span>
          )}
        </div>
      </Reveal>

      {/* === L0 基础身份 === */}
      <Reveal delay={0.12}>
        <section>
          <SectionTitle style={{ marginBottom: SPACING.sm }}>
            <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
              <UserCircle size={14} />
              {t('mind_inspector.profile.section_basic')}
            </span>
          </SectionTitle>
          <Card style={{ padding: 0, overflow: 'hidden' }}>
            {l0Fields.map((def, idx) => (
              <Fragment key={def.type}>
                {idx > 0 && <RowDivider />}
                <IdentityRow
                  fact={basicMap.get(def.type) ?? null}
                  label={t(`mind_inspector.profile.field_${def.type}`)}
                  placeholder={t(`mind_inspector.profile.placeholder_${def.type}`)}
                  onSave={(content) => handleSaveFact(def.type, content)}
                  onTogglePin={() => handleTogglePin(def.type, basicMap.get(def.type)?.is_pinned ?? false)}
                  saving={busyField === def.type}
                />
              </Fragment>
            ))}
          </Card>
        </section>
      </Reveal>

      {/* === L0.5 结构化偏好 === */}
      <Reveal delay={0.18}>
        <section>
          <SectionTitle style={{ marginBottom: SPACING.sm }}>
            <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
              <Heart size={14} />
              {t('mind_inspector.profile.section_preferences')}
            </span>
          </SectionTitle>
          <Card style={{ padding: 0, overflow: 'hidden' }}>
            {l05Fields.map((def, idx) => (
              <Fragment key={def.type}>
                {idx > 0 && <RowDivider />}
                <IdentityRow
                  fact={basicMap.get(def.type) ?? null}
                  label={t(`mind_inspector.profile.field_${def.type}`)}
                  placeholder={t(`mind_inspector.profile.placeholder_${def.type}`)}
                  onSave={(content) => handleSaveFact(def.type, content)}
                  onTogglePin={() => handleTogglePin(def.type, basicMap.get(def.type)?.is_pinned ?? false)}
                  saving={busyField === def.type}
                />
              </Fragment>
            ))}
          </Card>
        </section>
      </Reveal>

      {/* === 兴趣画像（发现引擎：兴趣域 / 探针 / 不喜欢 / 开放度） === */}
      <Reveal delay={0.21}>
        <DiscoverySection character={character} accent={accent} t={t} />
      </Reveal>

      {/* === L1 近期状态（只读） === */}
      <Reveal delay={0.24}>
        <section>
          <SectionTitle style={{ marginBottom: SPACING.sm }}>
            <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
              <Sparkles size={14} />
              {t('mind_inspector.profile.section_recent')}
              {l1 && l1.round_count > 0 && (
                <span
                  style={{
                    ...TYPO.micro,
                    color: COLORS.textQuaternary,
                    fontWeight: 400,
                    marginLeft: SPACING.xs,
                    textTransform: 'none',
                    letterSpacing: 0,
                  }}
                >
                  {t('mind_inspector.profile.recent_meta', {
                    rounds: l1.round_count,
                    time: formatRelative(l1.generated_at, t),
                  })}
                </span>
              )}
            </span>
          </SectionTitle>
          {!hasL1Data ? (
            <EmptyState
              text={t('mind_inspector.profile.recent_empty')}
              style={{ padding: SPACING.md }}
            />
          ) : (
            <div style={{ display: 'flex', gap: SPACING.cardGap, flexWrap: 'wrap' }}>
              <L1StateCard
                icon={<Target size={16} />}
                title={t('mind_inspector.profile.recent_goals')}
                items={l1!.recent_goals}
                emptyText={t('mind_inspector.profile.recent_empty')}
                accent={accent}
              />
              <L1StateCard
                icon={<Briefcase size={16} />}
                title={t('mind_inspector.profile.recent_projects')}
                items={l1!.current_projects}
                emptyText={t('mind_inspector.profile.recent_empty')}
                accent={accent}
              />
              <L1StateCard
                icon={<Heart size={16} />}
                title={t('mind_inspector.profile.recent_preferences')}
                items={l1!.recent_preferences}
                emptyText={t('mind_inspector.profile.recent_empty')}
                accent={accent}
              />
            </div>
          )}
        </section>
      </Reveal>

      {/* === L2 自由事实 === */}
      <Reveal delay={0.3}>
        <section>
          <SectionTitle style={{ marginBottom: SPACING.sm }}>
            <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
              <Plus size={14} />
              {t('mind_inspector.profile.section_custom')}
              {profile && profile.custom_facts.length > 0 && (
                <span
                  style={{
                    ...TYPO.micro,
                    color: COLORS.textQuaternary,
                    fontWeight: 400,
                    marginLeft: SPACING.xs,
                    textTransform: 'none',
                    letterSpacing: 0,
                  }}
                >
                  {profile.custom_facts.length}
                </span>
              )}
            </span>
          </SectionTitle>
          <div style={{ display: 'flex', flexDirection: 'column', gap: SPACING.xs }}>
            <AddCustomFact onAdd={handleAddCustom} />
            {profile && profile.custom_facts.length === 0 ? (
              <EmptyState
                text={t('mind_inspector.profile.custom_empty')}
                style={{ padding: SPACING.md }}
              />
            ) : (
              profile!.custom_facts.map((fact) => (
                <CustomFactItem
                  key={`${fact.content}-${fact.timestamp}`}
                  fact={fact}
                  onDelete={() => handleDeleteCustom(fact.content)}
                  deleting={busyField === `del-${fact.content}`}
                />
              ))
            )}
          </div>
        </section>
      </Reveal>
    </div>
  );
};

export default UserProfilePage;