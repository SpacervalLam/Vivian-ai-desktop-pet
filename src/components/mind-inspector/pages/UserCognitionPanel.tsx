/**
 * User Cognition Panel —— 用户认知系统三页模块
 *
 * 对应用户设计的 State → Event → Knowledge 三层架构：
 * 1. Current State（当前状态）：用户当前持续活动（如"睡觉 已 6h"）
 * 2. Behavior Timeline（行为时间线）：近期已封存的行为事件（带 duration）
 * 3. Knowledge（认知信念）：从行为日志提炼的用户习惯 Belief
 *
 * 数据来源：get_world_snapshot 返回的 behaviors / user_beliefs / snapshot.user_presence
 */

import React from 'react';
import { useTranslation } from 'react-i18next';
import { Brain, History, Moon } from 'lucide-react';
import { COLORS, TYPO, SPACING } from '../design-system';
import { Card, EmptyState, SectionTitle, StatusDot, Tag } from '../shared-components';
import type {
  BeliefView,
  UserBehaviorEntryView,
  WorldSnapshotView,
} from '../../../types';

const toMs = (ts: number): number => (ts < 1e12 ? ts * 1000 : ts);

const formatDuration = (secs: number): string => {
  if (secs < 60) return `${secs.toFixed(0)}秒`;
  if (secs < 3600) return `${Math.floor(secs / 60)}分`;
  const h = secs / 3600;
  return `${h.toFixed(1)}小时`;
};

const formatTime = (ts: number): string => {
  const d = new Date(toMs(ts));
  if (Number.isNaN(d.getTime())) return '—';
  const pad = (x: number) => String(x).padStart(2, '0');
  const month = d.getMonth() + 1;
  const day = d.getDate();
  const hh = pad(d.getHours());
  const mm = pad(d.getMinutes());
  return `${month}/${day} ${hh}:${mm}`;
};

const CATEGORY_LABEL_KEY: Record<string, string> = {
  Trait: 'mind_inspector.cognition.category_trait',
  Habit: 'mind_inspector.cognition.category_habit',
  Preference: 'mind_inspector.cognition.category_preference',
  State: 'mind_inspector.cognition.category_state',
  Relationship: 'mind_inspector.cognition.category_relationship',
};

const STATUS_COLOR: Record<string, string> = {
  Stable: COLORS.success,
  Questioning: COLORS.event.mood,
  Superseded: COLORS.textQuaternary,
};

// ── 子区 1：Current State ──

interface CurrentStateProps {
  snapshot: WorldSnapshotView | undefined;
}

const confidenceColor = (c: number): string => {
  if (c >= 0.85) return COLORS.success;
  if (c >= 0.7) return COLORS.event.mood;
  return COLORS.textQuaternary;
};

const SOURCE_LABEL: Record<string, string> = {
  local_classifier: '本地分类器',
  llm_observation: 'LLM 观察',
  return_detected: '回归检测',
  system_clear: '系统清除',
};

const CurrentStateSection: React.FC<CurrentStateProps> = ({ snapshot }) => {
  const { t } = useTranslation();
  const presence = snapshot?.user_presence;
  const activity = presence?.current_activity ?? null;

  const isPresent = presence?.presence === 'present';
  const presenceLabel = isPresent
    ? t('mind_inspector.cognition.presence_present')
    : t('mind_inspector.cognition.presence_away');
  const awaySecs = presence?.away_elapsed_secs ?? 0;

  let elapsedLabel = '';
  if (activity) {
    const elapsedSecs = Math.max(0, Math.floor((Date.now() - toMs(activity.started_at)) / 1000));
    elapsedLabel = formatDuration(elapsedSecs);
  } else if (!isPresent && awaySecs > 0) {
    elapsedLabel = formatDuration(awaySecs);
  }

  return (
    <Card>
      <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs, marginBottom: SPACING.sm }}>
        <StatusDot color={activity ? COLORS.accent : (isPresent ? COLORS.success : COLORS.textQuaternary)} pulse={!!activity} />
        <span style={{ ...TYPO.caption, color: COLORS.textTertiary }}>
          {t('mind_inspector.cognition.current_state_title')}
        </span>
      </div>
      {activity ? (
        <>
          <div style={{ ...TYPO.h2, color: COLORS.accent, marginBottom: 4 }}>
            {activity.label}
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs, marginBottom: 4 }}>
            {/* 置信度指示条 */}
            <div
              style={{
                width: 48,
                height: 3,
                borderRadius: 2,
                background: COLORS.subtleBorder,
                overflow: 'hidden',
              }}
            >
              <div
                style={{
                  width: `${Math.round(activity.confidence * 100)}%`,
                  height: '100%',
                  borderRadius: 2,
                  background: confidenceColor(activity.confidence),
                  transition: 'width 0.3s ease',
                }}
              />
            </div>
            <span style={{ ...TYPO.caption, fontSize: 10, color: confidenceColor(activity.confidence) }}>
              {Math.round(activity.confidence * 100)}%
            </span>
          </div>
          <div style={{ ...TYPO.body, fontSize: 12, color: COLORS.textSecondary }}>
            {t('mind_inspector.cognition.since_label', { duration: elapsedLabel })}
          </div>
        </>
      ) : (
        <>
          <div style={{ ...TYPO.h2, color: isPresent ? COLORS.success : COLORS.textQuaternary, marginBottom: 4 }}>
            {presenceLabel}
          </div>
          <div style={{ ...TYPO.body, fontSize: 12, color: COLORS.textSecondary }}>
            {elapsedLabel
              ? t('mind_inspector.cognition.elapsed_label', { duration: elapsedLabel })
              : t('mind_inspector.cognition.no_activity_hint')}
          </div>
        </>
      )}
    </Card>
  );
};

// ── 子区 2：Behavior Timeline ──

interface BehaviorTimelineProps {
  behaviors: UserBehaviorEntryView[];
}

const BehaviorTimelineSection: React.FC<BehaviorTimelineProps> = ({ behaviors }) => {
  const { t } = useTranslation();

  return (
    <Card style={{ padding: 0 }}>
      <div style={{ padding: SPACING.md, paddingBottom: SPACING.xs }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs }}>
          <History size={14} color={COLORS.textTertiary} strokeWidth={1.5} />
          <span style={{ ...TYPO.caption, color: COLORS.textTertiary }}>
            {t('mind_inspector.cognition.timeline_title')}
          </span>
        </div>
      </div>
      {behaviors.length === 0 ? (
        <div style={{ padding: SPACING.md, paddingTop: 0 }}>
          <EmptyState text={t('mind_inspector.cognition.timeline_empty')} />
        </div>
      ) : (
        <div style={{ maxHeight: 280, overflowY: 'auto', padding: SPACING.sm }}>
          {behaviors.map((entry, idx) => (
            <div
              key={entry.id}
              style={{
                display: 'flex',
                alignItems: 'flex-start',
                gap: SPACING.sm,
                padding: `${SPACING.xs}px 0`,
                borderBottom: idx < behaviors.length - 1 ? `1px solid ${COLORS.subtleBorder}` : 'none',
              }}
            >
              <div
                style={{
                  width: 6,
                  height: 6,
                  borderRadius: '50%',
                  background: COLORS.accent,
                  flexShrink: 0,
                  marginTop: 6,
                  opacity: 0.7,
                }}
              />
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs, flexWrap: 'wrap' }}>
                  <span style={{ ...TYPO.body, fontSize: 12, fontWeight: 500, color: COLORS.textPrimary }}>
                    {entry.activity_label}
                  </span>
                  <Tag color={COLORS.event.observation}>
                    {formatDuration(entry.duration_secs)}
                  </Tag>
                  {/* 置信度 */}
                  <span style={{ ...TYPO.caption, fontSize: 10, color: confidenceColor(entry.confidence) }}>
                    {Math.round(entry.confidence * 100)}%
                  </span>
                  {/* 来源标签 */}
                  <span style={{ ...TYPO.caption, fontSize: 10, color: COLORS.textTertiary }}>
                    {SOURCE_LABEL[entry.source] ?? entry.source}
                  </span>
                </div>
                <div style={{ ...TYPO.caption, fontSize: 10, color: COLORS.textQuaternary, marginTop: 2 }}>
                  {formatTime(entry.started_at)} → {formatTime(entry.ended_at)}
                </div>
              </div>
            </div>
          ))}
        </div>
      )}
    </Card>
  );
};

// ── 子区 3：Knowledge / Beliefs ──

interface KnowledgeSectionProps {
  beliefs: BeliefView[];
}

const KnowledgeSection: React.FC<KnowledgeSectionProps> = ({ beliefs }) => {
  const { t } = useTranslation();

  return (
    <Card style={{ padding: 0 }}>
      <div style={{ padding: SPACING.md, paddingBottom: SPACING.xs }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs }}>
          <Brain size={14} color={COLORS.textTertiary} strokeWidth={1.5} />
          <span style={{ ...TYPO.caption, color: COLORS.textTertiary }}>
            {t('mind_inspector.cognition.knowledge_title')}
          </span>
        </div>
      </div>
      {beliefs.length === 0 ? (
        <div style={{ padding: SPACING.md, paddingTop: 0 }}>
          <EmptyState text={t('mind_inspector.cognition.knowledge_empty')} />
        </div>
      ) : (
        <div style={{ maxHeight: 280, overflowY: 'auto', padding: SPACING.sm }}>
          {beliefs.map((b, idx) => {
            const status = b.status ?? 'Stable';
            const statusColor = STATUS_COLOR[status] ?? COLORS.textTertiary;
            const categoryLabel = CATEGORY_LABEL_KEY[b.category]
              ? t(CATEGORY_LABEL_KEY[b.category])
              : b.category;
            return (
              <div
                key={b.id}
                style={{
                  padding: `${SPACING.xs}px 0`,
                  borderBottom: idx < beliefs.length - 1 ? `1px solid ${COLORS.subtleBorder}` : 'none',
                }}
              >
                <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs, flexWrap: 'wrap', marginBottom: 2 }}>
                  <span style={{ ...TYPO.body, fontSize: 12, fontWeight: 500, color: COLORS.textPrimary, flex: 1, minWidth: 0 }}>
                    {b.statement}
                  </span>
                  <Tag color={statusColor}>
                    {Math.round(b.confidence * 100)}%
                  </Tag>
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: SPACING.xs, flexWrap: 'wrap' }}>
                  <span style={{ ...TYPO.caption, fontSize: 10, color: COLORS.textQuaternary }}>
                    {categoryLabel}
                  </span>
                  {b.metric && b.value != null && (
                    <span style={{ ...TYPO.caption, fontSize: 10, color: COLORS.event.observation }}>
                      · {b.metric} = {b.value.toFixed(1)}
                    </span>
                  )}
                  {b.match_labels && b.match_labels.length > 0 && (
                    <span style={{ ...TYPO.caption, fontSize: 10, color: COLORS.textTertiary }}>
                      · {b.match_labels.join('/')}
                    </span>
                  )}
                  {status === 'Questioning' && (
                    <span style={{ ...TYPO.caption, fontSize: 10, color: COLORS.event.mood }}>
                      · {t('mind_inspector.cognition.status_questioning')}
                    </span>
                  )}
                  {b.contradiction_count && b.contradiction_count > 0 && (
                    <span style={{ ...TYPO.caption, fontSize: 10, color: COLORS.textQuaternary }}>
                      · {t('mind_inspector.cognition.contradictions', { n: b.contradiction_count })}
                    </span>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </Card>
  );
};

// ── 主面板 ──

interface UserCognitionPanelProps {
  snapshot: WorldSnapshotView | undefined;
  behaviors: UserBehaviorEntryView[];
  beliefs: BeliefView[];
}

const UserCognitionPanel: React.FC<UserCognitionPanelProps> = ({ snapshot, behaviors, beliefs }) => {
  const { t } = useTranslation();

  return (
    <div>
      <SectionTitle style={{ marginBottom: SPACING.md }}>
        <span style={{ display: 'inline-flex', alignItems: 'center', gap: SPACING.xs }}>
          <Moon size={14} color={COLORS.accent} strokeWidth={1.5} />
          {t('mind_inspector.cognition.panel_title')}
        </span>
      </SectionTitle>
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: 'minmax(180px, 1fr) minmax(0, 2fr) minmax(0, 2fr)',
          gap: SPACING.cardGap,
          width: '100%',
        }}
      >
        <CurrentStateSection snapshot={snapshot} />
        <BehaviorTimelineSection behaviors={behaviors} />
        <KnowledgeSection beliefs={beliefs} />
      </div>
    </div>
  );
};

export default React.memo(UserCognitionPanel);
