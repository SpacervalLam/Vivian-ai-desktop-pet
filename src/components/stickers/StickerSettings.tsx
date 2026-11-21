import { CHARACTER_IDS, characterName } from '../../characters';
import React,{useEffect,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {open} from '@tauri-apps/plugin-dialog';
import {useTranslation} from 'react-i18next';
import StickerImage from './StickerImage';
import {stickerLabel,stickerCategory,stickerTypeLabel} from './stickers';
import type {StickerRef} from '../../types';
import {resourcePackInstalled} from '../../utils/optionalResources';
import {stickerTranslations} from '../../i18n/stickers';
export default function StickerSettings(){
 const {i18n}=useTranslation();const zh=i18n.language.startsWith('zh');
 return <>
  {!resourcePackInstalled('stickers') && <p role="status">{zh?'内置贴纸资源包未安装，可通过安装程序添加。仍可导入和使用自定义贴纸。':'Built-in stickers are not installed. Add the sticker pack in Setup; custom stickers remain available.'}</p>}
  <p style={{fontSize:12,color:'var(--panel-text-secondary)',lineHeight:1.6,margin:'0 0 16px'}}>{zh?'所有桌宠共用按类型分组的贴纸库。角色选择类型后，程序会从该类型中随机选一张；发送频率分别设置。偶尔：间隔五轮；正常：间隔三轮。明确要求发送时可跳过冷却，关闭时始终不发。':'All companions share one sticker library grouped by reaction type. The app randomly picks artwork from the selected type. Frequency settings remain separate. Occasional: five replies apart; normal: three. Explicit requests bypass cooldown; Off always disables stickers.'}</p>
  {CHARACTER_IDS.map(id => <CharacterStickerSettings key={id} character={id} />)}
  <CharacterStickerSettings character="vivian" library />
 </>;
}
function CharacterStickerSettings({character,library=false}:{character:string;library?:boolean}){
 const {i18n}=useTranslation();const zh=i18n.language.startsWith('zh');
 const [rows,setRows]=useState<StickerRef[]>([]);const [frequency,setFrequency]=useState('occasional');
 const [category,setCategory]=useState('happy');
 const [error,setError]=useState('');const [busy,setBusy]=useState(false);const [replace,setReplace]=useState('');const [label,setLabel]=useState('');const [meaning,setMeaning]=useState('');
 const reload=async()=>{const r=await invoke<{frequency:string;stickers:StickerRef[]}>('list_stickers',{characterId:character});setRows(r.stickers);setFrequency(r.frequency);};
 useEffect(()=>{let active=true;setError('');setRows([]);setReplace('');setLabel('');setMeaning('');invoke<{frequency:string;stickers:StickerRef[]}>('list_stickers',{characterId:character}).then(r=>{if(active){setRows(r.stickers);setFrequency(r.frequency);}}).catch(e=>{if(active)setError(String(e));});return()=>{active=false;};},[character]);
 const change=async(value:string)=>{setBusy(true);setError('');try{await invoke('set_sticker_frequency',{characterId:character,frequency:value});setFrequency(value);}catch(e){setError(String(e));}finally{setBusy(false);}};
 const importImage=async()=>{setBusy(true);setError('');try{const path=await open({multiple:false,filters:[{name:'PNG',extensions:['png']}]});if(!path||Array.isArray(path))return;await invoke('import_sticker',{characterId:character,sourcePath:path,label,meaning,category,replaceId:replace||null});await reload();setReplace('');setLabel('');setMeaning('');}catch(e){setError(String(e));}finally{setBusy(false);}};
 const style:React.CSSProperties={padding:'7px 10px',borderRadius:8,border:'1px solid var(--panel-border)',background:'var(--panel-surface)',color:'var(--panel-text)',fontFamily:'inherit'};
 const name=characterName(character);
 const groups=Array.from(new Set(rows.map(stickerCategory))).map(type=>({type,rows:rows.filter(row=>stickerCategory(row)===type)}));
 const types=Array.from(new Set([...Object.keys(stickerTranslations['zh-CN']),...groups.map(group=>group.type)]));
 return <section aria-label={library?(zh?'共享贴纸库':'Shared sticker library'):name} style={{marginTop:16,padding:16,border:'1px solid var(--panel-border)',borderRadius:12}}>
  {!library && <div style={{display:'flex',alignItems:'center',justifyContent:'space-between',gap:12,flexWrap:'wrap'}}>
   <h3 style={{fontSize:14,margin:0}}>{name}</h3>
   <label style={{display:'flex',alignItems:'center',gap:8,fontSize:12}}>{zh?'发送频率':'Frequency'}
    <select aria-label={`${name} ${zh?'表情包频率':'sticker frequency'}`} value={frequency} disabled={busy} onChange={e=>void change(e.target.value)} style={style}><option value="off">{zh?'关闭':'Off'}</option><option value="occasional">{zh?'偶尔':'Occasional'}</option><option value="normal">{zh?'正常':'Normal'}</option></select>
   </label>
  </div>}
  {library && <><h4>{zh?'共享贴纸库':'Shared sticker library'}</h4>{groups.map(group=><section key={group.type} aria-label={stickerTypeLabel(group.type)}><h5 style={{margin:'16px 0 8px'}}>{stickerTypeLabel(group.type)} · {group.rows.length}</h5><div style={{display:'grid',gridTemplateColumns:'repeat(auto-fill,minmax(85px,1fr))',gap:8,margin:'8px 0'}}>{group.rows.map(row=><div key={row.id} style={{display:'flex',alignItems:'center',flexDirection:'column',fontSize:11}}><StickerImage sticker={row} size={80}/><span>{stickerLabel(row)}</span></div>)}</div></section>)}
  <details><summary>{zh?'导入或替换 PNG 贴纸':'Import or replace a PNG sticker'}</summary><div style={{display:'grid',gap:8,marginTop:10}}>
  <select aria-label={zh?'替换目标':'Replacement target'} value={replace} style={style} disabled={busy} onChange={e=>{const row=rows.find(r=>r.id===e.target.value);setReplace(e.target.value);setLabel(row?stickerLabel(row):'');setMeaning(row?.meaning??'');setCategory(row?stickerCategory(row):'happy');}}><option value="">{zh?'新增贴纸':'Add sticker'}</option>{rows.map((row,index)=><option key={row.id} value={row.id}>{zh?'替换':'Replace'} · {stickerTypeLabel(stickerCategory(row))} · {index+1}</option>)}</select>
  <label>{zh?'贴纸类型':'Reaction type'}<select aria-label={zh?'贴纸类型':'Reaction type'} value={category} disabled={busy} onChange={e=>setCategory(e.target.value)} style={style}>{types.map(type=><option key={type} value={type}>{stickerTypeLabel(type)}</option>)}</select></label>
  <input aria-label={zh?'贴纸名称':'Sticker label'} placeholder={zh?'名称（最多 24 字）':'Label (up to 24 characters)'} value={label} maxLength={24} onChange={e=>setLabel(e.target.value)} style={style}/>
  <textarea aria-label={zh?'贴纸含义':'Sticker meaning'} placeholder={zh?'含义和适用场景，供角色选择（最多 120 字）':'Meaning and suitable situations (up to 120 characters)'} value={meaning} maxLength={120} onChange={e=>setMeaning(e.target.value)} style={style}/>
  <button type="button" disabled={busy||!label.trim()||!meaning.trim()} onClick={()=>void importImage()} style={style}>{zh?'选择 PNG 并导入':'Select PNG and import'}</button><small>{zh?'最多 2 MB、2048×2048 像素；替换后，历史消息仍使用旧版本。':'Up to 2 MB and 2048×2048 pixels. Historical messages keep the previous version.'}</small>
  </div></details></>}{error&&<p role="alert" style={{color:'#dc2626'}}>{error}</p>}
 </section>;
}
