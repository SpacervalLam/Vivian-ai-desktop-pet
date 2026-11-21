import React, { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { invoke } from '@tauri-apps/api/core';

/** 技能条目（对齐后端 commands::plugins::SkillEntryInfo） */
export interface SkillEntry {
  name: string;
  description: string;
  scope: string | null;
  /** builtin / user / plugin */
  origin: string;
  /** folder / flat / inline */
  kind: string;
  dir: string | null;
  resources: string[];
  editable: boolean;
  body_len: number;
}

interface SkillResourceInfo {
  path: string;
  size: number;
}

/** 技能详情（对齐后端 commands::plugins::SkillDetailInfo） */
interface SkillDetail {
  name: string;
  description: string;
  origin: string;
  kind: string;
  dir: string | null;
  body: string;
  keywords: string[];
  resources: SkillResourceInfo[];
  editable: boolean;
}

interface SkillIssue {
  level: string;
  code: string;
  message: string;
}

/** 校验结果（对齐后端 skills::validate::SkillValidation） */
export interface SkillValidation {
  ok: boolean;
  issues: SkillIssue[];
}

interface ResourceDraft {
  path: string;
  content: string;
}

interface Props {
  skills: SkillEntry[] | null;
  skillsDir?: string;
  onChanged: () => Promise<void> | void;
}

const muted: React.CSSProperties = { fontSize: 12, color: 'var(--panel-text-secondary)' };
const label: React.CSSProperties = { fontSize: 12, color: 'var(--panel-text-secondary)', marginBottom: 4 };
const input: React.CSSProperties = {
  width: '100%',
  boxSizing: 'border-box',
  fontSize: 13,
  padding: '6px 8px',
  borderRadius: 6,
  border: '1px solid var(--panel-border)',
  background: 'var(--panel-bg, transparent)',
  color: 'var(--panel-text)',
};
const badge: React.CSSProperties = {
  fontSize: 11,
  color: 'var(--panel-text-tertiary)',
  flexShrink: 0,
};

/** 设置窗口「技能」区：盘点、查看、创建/编辑/删除用户技能。 */
const SkillsManager: React.FC<Props> = ({ skills, skillsDir, onChanged }) => {
  const { t } = useTranslation();
  const [detail, setDetail] = useState<SkillDetail | null>(null);
  const [editing, setEditing] = useState(false);
  const [creating, setCreating] = useState(false);
  const [form, setForm] = useState({
    name: '',
    description: '',
    keywords: '',
    body: '',
    resources: [] as ResourceDraft[],
  });
  const [validation, setValidation] = useState<SkillValidation | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const kindLabel = (kind: string) =>
    kind === 'folder'
      ? t('config.plugins.skill_kind_folder')
      : kind === 'flat'
        ? t('config.plugins.skill_kind_flat')
        : t('config.plugins.skill_kind_inline');

  const originLabel = (origin: string) =>
    origin === 'plugin'
      ? t('config.plugins.origin_plugin')
      : origin === 'builtin'
        ? t('config.plugins.builtin')
        : t('config.plugins.origin_user');

  const resetMessages = () => {
    setError(null);
    setValidation(null);
  };

  const openDetail = async (name: string) => {
    resetMessages();
    try {
      const loaded = await invoke<SkillDetail>('read_skill_detail', { name });
      setDetail(loaded);
      setCreating(false);
      setEditing(false);
      setForm({
        name: loaded.name,
        description: loaded.description,
        keywords: loaded.keywords.join(', '),
        body: loaded.body,
        resources: [],
      });
    } catch (e) {
      setError(t('config.plugins.skill_load_failed', { error: String(e) }));
    }
  };

  const startCreate = () => {
    resetMessages();
    setCreating(true);
    setEditing(true);
    setDetail(null);
    setForm({ name: '', description: '', keywords: '', body: '', resources: [] });
  };

  const startEdit = () => {
    resetMessages();
    setEditing(true);
  };

  const backToList = async () => {
    setEditing(false);
    setCreating(false);
    setDetail(null);
    resetMessages();
    await onChanged();
  };

  const runValidate = async () => {
    if (!form.name.trim() || !form.body.trim()) {
      setValidation({ ok: false, issues: [{ level: 'error', code: 'missing', message: t('config.plugins.skill_body') }] });
      return;
    }
    setBusy(true);
    resetMessages();
    try {
      const content = `---\nname: ${form.name.trim()}\ndescription: ${form.description.replace(/[\r\n]+/g, ' ').trim()}\nkeywords: ${form.keywords.trim()}\n---\n\n${form.body.trim()}\n`;
      const result = await invoke<SkillValidation>('validate_skill', { content });
      setValidation(result);
    } catch (e) {
      setError(t('config.plugins.skill_validate_failed', { error: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    setBusy(true);
    resetMessages();
    try {
      const saved = await invoke<SkillDetail>('save_skill', {
        name: form.name.trim(),
        description: form.description,
        body: form.body,
        keywords: form.keywords.split(',').map((k) => k.trim()).filter(Boolean),
        resources: form.resources.map((r) => ({ path: r.path.trim(), content: r.content })),
        createNew: creating,
      });
      setDetail(saved);
      setCreating(false);
      setEditing(false);
      setForm((prev) => ({ ...prev, resources: [] }));
      await onChanged();
    } catch (e) {
      setError(t('config.plugins.skill_save_failed', { error: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  const remove = async (name: string) => {
    setBusy(true);
    setError(null);
    try {
      await invoke('delete_skill', { name });
      setConfirmDelete(null);
      setDetail(null);
      setEditing(false);
      await onChanged();
    } catch (e) {
      setError(t('config.plugins.skill_delete_failed', { error: String(e) }));
    } finally {
      setBusy(false);
    }
  };

  // ---------- 编辑器 / 详情 ----------
  if (editing || detail) {
    const header = creating || detail?.name || '—';
    return (
      <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
          <button onClick={backToList} style={{ fontSize: 12 }}>
            {t('config.plugins.skill_back')}
          </button>
          <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--panel-text)' }}>{header}</span>
          {detail && <span style={badge}>{originLabel(detail.origin)} · {kindLabel(detail.kind)}</span>}
          {detail && !detail.editable && <span style={badge}>{t('config.plugins.skill_readonly')}</span>}
        </div>

        {error && <div style={{ fontSize: 12, color: 'var(--panel-text-secondary)' }}>{error}</div>}

        {editing ? (
          <>
            <div>
              <div style={label}>{t('config.plugins.skill_name')}</div>
              <input
                style={input}
                value={form.name}
                disabled={!creating}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
              />
            </div>
            <div>
              <div style={label}>{t('config.plugins.skill_description')}</div>
              <input
                style={input}
                value={form.description}
                onChange={(e) => setForm({ ...form, description: e.target.value })}
              />
            </div>
            <div>
              <div style={label}>{t('config.plugins.skill_keywords')}</div>
              <input
                style={input}
                value={form.keywords}
                onChange={(e) => setForm({ ...form, keywords: e.target.value })}
              />
            </div>
            <div>
              <div style={label}>{t('config.plugins.skill_body')}</div>
              <textarea
                style={{ ...input, minHeight: 180, fontFamily: 'var(--font-mono, monospace)' }}
                value={form.body}
                onChange={(e) => setForm({ ...form, body: e.target.value })}
              />
            </div>

            <div>
              <div style={label}>{t('config.plugins.skill_resources')}</div>
              {form.resources.map((resource, index) => (
                <div key={index} style={{ display: 'flex', flexDirection: 'column', gap: 4, marginBottom: 8 }}>
                  <div style={{ display: 'flex', gap: 6 }}>
                    <input
                      style={input}
                      placeholder={t('config.plugins.skill_resource_path_hint')}
                      value={resource.path}
                      onChange={(e) => {
                        const next = [...form.resources];
                        next[index] = { ...resource, path: e.target.value };
                        setForm({ ...form, resources: next });
                      }}
                    />
                    <button
                      onClick={() =>
                        setForm({ ...form, resources: form.resources.filter((_, i) => i !== index) })
                      }
                      style={{ fontSize: 12 }}
                    >
                      {t('config.plugins.delete')}
                    </button>
                  </div>
                  <textarea
                    style={{ ...input, minHeight: 70 }}
                    value={resource.content}
                    onChange={(e) => {
                      const next = [...form.resources];
                      next[index] = { ...resource, content: e.target.value };
                      setForm({ ...form, resources: next });
                    }}
                  />
                </div>
              ))}
              <button
                onClick={() => setForm({ ...form, resources: [...form.resources, { path: '', content: '' }] })}
                style={{ fontSize: 12 }}
              >
                {t('config.plugins.skill_add_resource')}
              </button>
            </div>

            {validation && (
              <div style={{ ...muted, whiteSpace: 'pre-wrap' }}>
                {validation.ok ? t('config.plugins.skill_validation_ok') : t('config.plugins.skill_validation_failed')}
                {validation.issues.map((issue, index) => (
                  <div key={index}>[{issue.level}] {issue.code}: {issue.message}</div>
                ))}
              </div>
            )}

            <div style={{ display: 'flex', gap: 8 }}>
              <button onClick={runValidate} disabled={busy} style={{ fontSize: 12 }}>
                {t('config.plugins.skill_validate')}
              </button>
              <button onClick={save} disabled={busy} style={{ fontSize: 12 }}>
                {t('config.plugins.skill_save')}
              </button>
            </div>
          </>
        ) : (
          detail && (
            <>
              <div style={muted}>{detail.description}</div>
              {detail.dir && <div style={badge}>{detail.dir}</div>}
              <div style={{ whiteSpace: 'pre-wrap', fontSize: 13, color: 'var(--panel-text)' }}>{detail.body}</div>
              {detail.resources.length > 0 && (
                <div>
                  <div style={label}>{t('config.plugins.skill_resources')}</div>
                  {detail.resources.map((resource) => (
                    <div key={resource.path} style={badge}>
                      {resource.path} · {Math.max(1, Math.round(resource.size / 1024))} KB
                    </div>
                  ))}
                </div>
              )}
              {detail.editable && (
                <div style={{ display: 'flex', gap: 8 }}>
                  <button onClick={startEdit} style={{ fontSize: 12 }}>
                    {t('config.plugins.skill_edit')}
                  </button>
                  {confirmDelete === detail.name ? (
                    <>
                      <span style={muted}>
                        {t('config.plugins.skill_delete_confirm', { name: detail.name })}
                      </span>
                      <button onClick={() => remove(detail.name)} disabled={busy} style={{ fontSize: 12 }}>
                        {t('config.plugins.skill_delete')}
                      </button>
                      <button onClick={() => setConfirmDelete(null)} style={{ fontSize: 12 }}>
                        ×
                      </button>
                    </>
                  ) : (
                    <button onClick={() => setConfirmDelete(detail.name)} style={{ fontSize: 12 }}>
                      {t('config.plugins.skill_delete')}
                    </button>
                  )}
                </div>
              )}
            </>
          )
        )}
      </div>
    );
  }

  // ---------- 列表 ----------
  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
        <button onClick={startCreate} style={{ fontSize: 12 }}>
          {t('config.plugins.skill_new')}
        </button>
        {skillsDir && <span style={badge}>{skillsDir}</span>}
      </div>
      {error && <div style={{ fontSize: 12, color: 'var(--panel-text-secondary)' }}>{error}</div>}
      {skills === null ? (
        <div style={{ fontSize: 13, color: 'var(--panel-text-secondary)' }}>{t('mind_inspector.common.loading')}</div>
      ) : skills.length === 0 ? (
        <div style={{ fontSize: 13, color: 'var(--panel-text-secondary)' }}>
          {t('config.plugins.no_skills')}
        </div>
      ) : (
        skills.map((skill) => (
          <div
            key={skill.name}
            title={skill.description || skill.name}
            onClick={() => openDetail(skill.name)}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              padding: '6px 12px',
              borderBottom: '1px solid var(--panel-border)',
              cursor: 'pointer',
            }}
          >
            <span style={{ fontSize: 13, fontWeight: 500, color: 'var(--panel-text)', minWidth: 120 }}>
              {skill.name}
            </span>
            <span style={badge}>
              {originLabel(skill.origin)} · {kindLabel(skill.kind)}
              {skill.origin === 'plugin' ? ` · ${skill.name.split('/')[0]}` : ''}
            </span>
            <span
              style={{
                flex: 1,
                fontSize: 12,
                color: 'var(--panel-text-secondary)',
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap',
              }}
            >
              {skill.description || '—'}
            </span>
          </div>
        ))
      )}
    </div>
  );
};

export default SkillsManager;
