import { listen } from '@tauri-apps/api/event';
import { Upload, Info, Check, AlertCircle, RotateCw, UserRound } from 'lucide-react';
import { useEffect, useState } from 'react';
import { invoke, convertFileSrc } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { useTranslation } from 'react-i18next';
import { characterAssetSource, type CharacterPackageStatus } from '../../characterPackages';
import './CharacterPackageSettings.css';

function CharacterThumbnail({ portrait, atlas = null }: { portrait: string | null; atlas?: string | null }) {
  const [failedPortrait, setFailedPortrait] = useState<string>();
  const [failedAtlas, setFailedAtlas] = useState<string>();
  const showPortrait = portrait && portrait !== failedPortrait;
  const source = showPortrait ? portrait : atlas;
  return (
    <span className="character-pack-avatar" aria-hidden="true">
      {source && (showPortrait || source !== failedAtlas)
        ? <img className={showPortrait ? 'character-pack-portrait' : 'character-pack-atlas'}
            src={source} alt="" onError={() => (showPortrait ? setFailedPortrait : setFailedAtlas)(source)} />
        : <UserRound size={18} />}
    </span>
  );
}

export default function CharacterPackageSettings() {
  const { i18n } = useTranslation();
  const zh = i18n.language.startsWith('zh');
  const [status, setStatus] = useState<CharacterPackageStatus>();
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<{kind:'ok'|'error';text:string}>();
  const refresh = () => invoke<CharacterPackageStatus>('character_package_status').then(setStatus);
  useEffect(() => { let disposed=false;let stop:(()=>void)|undefined;refresh().catch(e=>setNotice({kind:'error',text:String(e)}));
    listen<{phase:string;error?:string}>('character-package:changed',event=>{void refresh();if(event.payload.error)setNotice({kind:'error',text:event.payload.error});else if(event.payload.phase==='loaded')setNotice({kind:'ok',text:zh?'角色已装载。':'Character loaded.'});}).then(fn=>{if(disposed)fn();else stop=fn;});
    return()=>{disposed=true;stop?.();};
  }, []);
  const install = async () => {
    setBusy(true); setNotice(undefined);
    try {
      const source = await open({ multiple: false, filters: [{ name: zh ? '角色包' : 'Character package', extensions: ['zip'] }] });
      if (typeof source !== 'string') return;
      await invoke('import_character_package', { source });
      await refresh();
    } catch (e) { setNotice({kind:'error',text:String(e)}); }
    finally { setBusy(false); }
  };
  const residentState = (p: NonNullable<CharacterPackageStatus['installed']>[number]) => p.pending_update ? (zh ? '等待空闲更新' : 'Waiting for idle') : status?.active.some(a => a.manifest.id === p.id) ? (status.initialized && !status.initialized.includes(p.id) ? (zh ? (status.initializing ? '正在初始化' : '初始化失败，请查看错误') : 'Not initialized; check errors') : (zh ? '已入住' : 'Resident')) : (zh ? '正在准备入住' : 'Preparing arrival');
  return <section className="character-pack-settings" aria-label={zh ? '角色包' : 'Character packages'}>
    <header className="character-pack-toolbar"><div><h4>{zh?'角色包':'Character packages'}</h4><p>{zh?'管理已入住的伙伴，或导入新的角色包。':'Manage your companions or import a new character package.'}</p></div>
      <button className="character-pack-import" disabled={busy} onClick={() => void install()}><Upload size={14}/>{busy ? (zh ? '正在导入…' : 'Importing…') : (zh ? '导入角色包 ZIP' : 'Import character ZIP')}</button>
    </header>
    <div className="character-pack-residents">
      {['Vivian','Nana'].map(name=><div className="character-pack-resident" key={name}><CharacterThumbnail
        portrait={`/icons/charaters/${name.toLowerCase()}.webp`}
        atlas={`/chibi/${name.toLowerCase()}-atlas.webp`}
      /><div><strong>{name}</strong><small>{zh?'原始角色':'Original character'}</small></div><span className="character-state"><Check size={11}/>{zh?'已内置':'Included'}</span></div>)}
      {status?.installed.map(p => <div className="character-pack-resident" key={p.id}><CharacterThumbnail
        key={p.avatar_path ?? p.atlas_path ?? p.id}
        portrait={p.avatar_path ? convertFileSrc(p.avatar_path) : null}
        atlas={p.atlas_path ? convertFileSrc(p.atlas_path) : characterAssetSource(`/chibi/${p.id}-atlas.webp`)}
      /><div><strong>{p.name}</strong><small>v{p.version}</small></div><span className={`character-state${p.pending_update || !status.active.some(a=>a.manifest.id===p.id)?' is-pending':''}`}>{p.pending_update || !status.active.some(a=>a.manifest.id===p.id)?<RotateCw size={11}/>:<Check size={11}/>}<span>{residentState(p)}</span></span></div>)}
    </div>
    <p className="character-pack-hint"><Info size={12}/><span>{zh?'导入后自动入住；已有角色将在空闲时更新。角色入住后不支持卸载。':'New characters move in automatically; updates wait for idle. Residents cannot be uninstalled.'}</span></p>
    {status?.errors?.map((error,i)=><p role="alert" key={i}>{error}</p>)}
    {notice && <p role={notice.kind==='error'?'alert':'status'}>{notice.kind==='error'?<AlertCircle size={13}/>:<Check size={13}/>}{notice.text}</p>}
  </section>;
}
