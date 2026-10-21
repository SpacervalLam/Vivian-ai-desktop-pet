import React, { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useTranslation } from 'react-i18next';
import { Check, UserPlus, Users } from 'lucide-react';
import './Sns.css';
export interface SnsPerson { id: string; name: string; online?: boolean; avatar?: string }
export interface SnsContact { id: string; name: string; persona: string; avatar: string; model: Record<string, unknown> | null }
export interface SnsGroup { id: string; name: string; members: string[]; updated_at: number; preview: string }
export type SnsPanel = { kind: 'contact' | 'group'; id?: string };
/**
 * 子页标题的唯一来源：聊天窗口导航栏直接用，组件内部只留着 aria-label。
 * 曾经导航栏和表单头部各算一份，现在只留这一处。
 */
export const snsPanelTitle = (panel: SnsPanel, t: (key: string) => string) =>
  panel.kind === 'contact' ? t(panel.id ? 'chat.sns_edit_contact' : 'chat.sns_add_contacts')
    : t(panel.id ? 'chat.sns_group_info' : 'chat.sns_create_group');
interface Props { panel: SnsPanel; people: SnsPerson[]; groups: SnsGroup[]; onSaved: (kind: 'contact' | 'group', item: SnsContact | SnsGroup) => void; onDeleted: (kind: 'contact' | 'group', id: string) => void }
export default function SnsManager({ panel, people, groups, onSaved, onDeleted }: Props) {
  const { t } = useTranslation();
  const [name, setName] = useState(''); const [persona, setPersona] = useState(''); const [avatar, setAvatar] = useState('');
  const [members, setMembers] = useState<string[]>([]);
  const [loading, setLoading] = useState(!!panel.id); const [busy, setBusy] = useState(false); const [error, setError] = useState(''); const [confirmDelete, setConfirmDelete] = useState(false);
  /** 编辑态下联系人原有的模型配置。UI 不再暴露单好友模型覆盖（一律跟随默认陪伴模型），
   *  但保存时原样带回，避免编辑人设顺手把历史配置清空。 */
  const [originalModel, setOriginalModel] = useState<Record<string, unknown>>({});
  /** 头像大图预览：点头像呼出，下方带「更改头像」。 */
  const [avatarPreview, setAvatarPreview] = useState(false);
  /** 群聊设置页的成员选择面板（点成员网格末尾的「+」展开）。 */
  const [memberPickerOpen, setMemberPickerOpen] = useState(false);
  const avatarInput = useRef<HTMLInputElement>(null);
  const pickAvatar = () => avatarInput.current?.click();
  useEffect(() => {
    let cancelled = false;
    if (!panel.id) return;
    void (async () => {
      try {
        if (panel.kind === 'contact') {
          const p = await invoke<SnsContact>('sns_get_contact', { id: panel.id }); if (cancelled) return;
          setName(p.name); setPersona(p.persona); setAvatar(p.avatar); setOriginalModel(p.model ?? {});
        } else { const g = groups.find(g => g.id === panel.id); if (!g) throw new Error(t('chat.sns_missing_group')); setName(g.name); setMembers(g.members); }
      } catch (e) { if (!cancelled) setError(String(e)); } finally { if (!cancelled) setLoading(false); }
    })();
    return () => { cancelled = true; };
  }, [panel.id, panel.kind]);
  const save = async (event: React.FormEvent) => {
    event.preventDefault(); setBusy(true); setError('');
    try {
      if (panel.kind === 'contact') {
        const input = { name: name.trim(), persona: persona.trim(), avatar, model: Object.keys(originalModel).length ? originalModel : null };
        const item = await invoke<SnsContact>('sns_save_contact', { id: panel.id ?? null, input }); onSaved('contact', item);
      } else { const item = await invoke<SnsGroup>('sns_save_group', { id: panel.id ?? null, name: name.trim(), members }); onSaved('group', item); }
    } catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  const remove = async () => {
    if (!panel.id) return;
    if (!confirmDelete) { setConfirmDelete(true); return; }
    setBusy(true); setError('');
    try { await invoke(panel.kind === 'contact' ? 'sns_delete_contact' : 'sns_delete_group', { id: panel.id }); onDeleted(panel.kind, panel.id); }
    catch (e) { setError(String(e)); } finally { setBusy(false); }
  };
  const uploadAvatar = (file?: File) => {
    if (!file) return; if (file.size > 6 * 1024 * 1024) { setError(t('chat.sns_avatar_size')); return; }
    const reader = new FileReader(); reader.onload = () => setAvatar(String(reader.result)); reader.onerror = () => setError(t('chat.sns_image_failed')); reader.readAsDataURL(file);
  };
  const title = snsPanelTitle(panel, t);
  /** 主按钮前缀图标：新建给「加人/建群」，编辑给「勾」。保存中不显示（只留文字）。 */
  const SubmitIcon = busy ? null : panel.id ? Check : panel.kind === 'contact' ? UserPlus : Users;
  // 在流子页：顶部让给灵动岛 / 状态栏 / 导航栏，返回与标题都由聊天窗口导航栏负责
  return <section className="sns-subpage" aria-label={title}>
    {loading ? <p className="sns-hint">{t('chat.partner_model_loading')}</p> : <form className="sns-form" onSubmit={save}>
      {panel.kind === 'contact' && <div className="sns-avatar-editor">
        {avatar
          // 有头像：整块可点，呼出大图预览（大图下方带「更改头像」）
          ? <button type="button" className="sns-avatar" onClick={() => setAvatarPreview(true)} aria-label={t('chat.sns_view_avatar')}><img src={avatar} alt={name || t('chat.sns_avatar')} /></button>
          // 没头像：灰框加号即上传入口
          : <button type="button" className="sns-upload" onClick={pickAvatar} aria-label={t('chat.sns_upload_avatar')}>
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M12 5v14M5 12h14" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
              </svg>
            </button>}
      </div>}
      {/* 名字 / 人设卡片：联系人两态 + 创建群聊。群聊设置页的名字走下方单行设置项。 */}
      {(panel.kind === 'contact' || (panel.kind === 'group' && !panel.id)) && <section className="sns-card">
        <label className="sns-field">{t(panel.kind === 'contact' ? 'chat.sns_name' : 'chat.sns_group_name')}<input value={name} onChange={e => setName(e.target.value)} readOnly={panel.kind === 'contact' && !!panel.id} maxLength={80} required autoFocus={!panel.id} /></label>
        {panel.kind === 'contact' && panel.id && <p className="sns-hint sns-hint-inline">{t('chat.sns_name_fixed')}</p>}
        {panel.kind === 'contact' && <label className="sns-field">{t('chat.sns_persona')}<textarea value={persona} onChange={e => setPersona(e.target.value)} rows={8} required placeholder={t('chat.sns_persona_hint')} /></label>}
      </section>}
      {/* 群聊设置页（编辑已有群）：参考聊天信息页版式——成员网格 + 单行设置项。
          创建群聊仍是「名字 + 邀请好友」勾选列表，不走这套。 */}
      {panel.kind === 'group' && panel.id && <>
        <div className="sns-member-grid">
          {people.filter(person => members.includes(person.id)).map(person => (
            <div className="sns-grid-cell" key={person.id}>
              <img src={person.avatar || `/icons/charaters/${person.id}.webp`} alt="" onError={e => { e.currentTarget.src = '/favicon.ico'; }} />
              <span>{person.name}</span>
            </div>
          ))}
          <div className="sns-grid-cell">
            <button type="button" className="sns-grid-add" onClick={() => setMemberPickerOpen(open => !open)} aria-label={t('chat.sns_members')} aria-expanded={memberPickerOpen}>
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <path d="M12 5v14M5 12h14" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
              </svg>
            </button>
            <span aria-hidden="true">&nbsp;</span>
          </div>
        </div>
        {memberPickerOpen && <section className="sns-card">
          <p className="sns-card-title">{t('chat.sns_members')}</p>
          {people.map(person => <label className="sns-member" key={person.id}>
            <img src={person.avatar || `/icons/charaters/${person.id}.webp`} alt="" onError={e => { e.currentTarget.src = '/favicon.ico'; }} />
            <span>{person.name}</span>
            <input type="checkbox" checked={members.includes(person.id)} onChange={e => setMembers(previous => e.target.checked ? [...previous, person.id] : previous.filter(id => id !== person.id))} />
          </label>)}
        </section>}
        <section className="sns-card">
          <label className="sns-settings-row">
            <span>{t('chat.sns_group_name')}</span>
            <input value={name} onChange={e => setName(e.target.value)} maxLength={80} required />
          </label>
        </section>
      </>}
      {/* 创建群聊：名字 + 邀请好友勾选列表 */}
      {panel.kind === 'group' && !panel.id && <section className="sns-card">
        <p className="sns-card-title">{t('chat.sns_members')}</p>
        {people.map(person => <label className="sns-member" key={person.id}>
          <img src={person.avatar || `/icons/charaters/${person.id}.webp`} alt="" onError={e => { e.currentTarget.src = '/favicon.ico'; }} />
          <span>{person.name}</span>
          <input type="checkbox" checked={members.includes(person.id)} onChange={e => setMembers(previous => e.target.checked ? [...previous, person.id] : previous.filter(id => id !== person.id))} />
        </label>)}
      </section>}
      {error && <p className="sns-error" role="alert">{error}</p>}
      <button className="sns-primary" type="submit" disabled={busy || !name.trim() || (panel.kind === 'group' && !members.length)}>
        {SubmitIcon && <SubmitIcon size={19} strokeWidth={1.9} aria-hidden="true" />}
        {busy ? t('chat.sns_saving') : t(panel.id ? 'chat.sns_save' : panel.kind === 'contact' ? 'chat.sns_add_friend' : 'chat.sns_create_group')}
      </button>
      {panel.id && <div className="sns-delete">
        {confirmDelete && <p className="sns-delete-hint">{t(panel.kind === 'contact' ? 'chat.sns_delete_contact_hint' : 'chat.sns_dissolve_hint')}</p>}
        <div className="sns-delete-actions">
          {confirmDelete && <button type="button" className="sns-ghost" onClick={() => setConfirmDelete(false)} disabled={busy}>{t('chat.btn_cancel')}</button>}
          <button type="button" className="sns-danger" onClick={() => void remove()} disabled={busy}>{t(confirmDelete ? 'chat.sns_confirm_delete' : panel.kind === 'contact' ? 'chat.sns_delete_contact' : 'chat.sns_dissolve')}</button>
        </div>
      </div>}
    </form>}
    {/* 头像大图：点暗处关闭；「更改头像」走同一个隐藏 file input */}
    {avatarPreview && avatar && <div className="sns-avatar-modal" onClick={() => setAvatarPreview(false)} role="dialog" aria-label={t('chat.sns_view_avatar')}>
      <img className="sns-avatar-modal-img" src={avatar} alt={name || t('chat.sns_avatar')} />
      <button type="button" className="sns-avatar-modal-action" onClick={e => { e.stopPropagation(); pickAvatar(); }}>{t('chat.sns_change_avatar')}</button>
    </div>}
    <input ref={avatarInput} className="sns-file-input" type="file" accept="image/*" onChange={e => { const file = e.target.files?.[0]; e.target.value = ''; uploadAvatar(file); }} />
  </section>;
}
